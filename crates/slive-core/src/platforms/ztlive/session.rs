//! ZtLive 会话主循环：注册 → 进房 → 心跳 / 保活 / 推送 → 断线重连。

use std::time::Duration;

use tokio::sync::watch;
use tokio::time::timeout;

use crate::error::SliveError;
use crate::models::{LiveDanmuControlEvent, LiveDanmuItem};
use crate::platforms::ztlive::frame::{
    decode_downstream, encode_upstream, DownstreamFrame, EncryptionMode, PacketHeader,
};
use crate::platforms::ztlive::messages::{
    cs_cmd, enter_room_ack, enter_room_payload, heartbeat_payload, keepalive_payload, parse_push,
    push_outcome, register_request, register_response, CMD_KEEPALIVE, CMD_PUSH, CMD_REGISTER,
    CMD_ZT_CS, CS_ENTER_ROOM, CS_HEARTBEAT, CS_USER_EXIT,
};
use crate::platforms::ztlive::proto::{find, get_string, get_u64, parse_fields, Field};
use crate::platforms::ztlive::ZtLiveDanmakuArgs;
use crate::ws::session::{FrameSource, SessionTuning};
use crate::ws::ConnectFactory;

/// AcFun 的 link IM 地址。
pub const WS_URL: &str = "wss://link.xiatou.com/";
/// link IM 应用 id。
pub const APP_ID: i32 = 13;
/// 应用 key。
pub const KPN: &str = "ACFUN_APP";
/// 客户端平台。
pub const KPF: &str = "PC_WEB";
/// 子业务。
pub const SUB_BIZ: &str = "mainApp";
/// SDK 版本（进房命令的 `clientLiveSdkVersion`）。
pub const SDK_VERSION: &str = "kwai-acfun-live-link";
/// 注册请求里 `appInfo.linkVersion` 的取值（Web 客户端口径，与服务端校验无关）。
pub const LINK_VERSION: &str = "1.2.1";
/// 注册应用的展示名。
pub const APP_NAME: &str = "link-sdk";
/// 服务端未下发心跳间隔时的兜底（毫秒）。
pub const DEFAULT_HEARTBEAT_MS: u64 = 10_000;
/// 心跳间隔下限。
pub const MIN_HEARTBEAT_MS: u64 = 1_000;
/// 心跳间隔上限。
pub const MAX_HEARTBEAT_MS: u64 = 60_000;
/// 传输层保活间隔（秒）。
pub const KEEPALIVE_SECS: u64 = 50;
/// 注册 / 进房等待应答的超时。
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

/// 会话运行期状态。
struct Link {
    app_id: i32,
    uid: u64,
    did: String,
    kpn: String,
    sub_biz: String,
    service_token: Vec<u8>,
    service_key: Vec<u8>,
    session_key: Option<Vec<u8>>,
    instance_id: u64,
    seq: u64,
    zt_sequence: u64,
    ticket: String,
    live_id: String,
    attach: String,
}

impl Link {
    fn new(args: &ZtLiveDanmakuArgs) -> Result<Self, SliveError> {
        let service_token = args.service_token.as_bytes().to_vec();
        if service_token.is_empty() {
            return Err(SliveError::invalid_input("AcFun 弹幕服务 token 为空"));
        }
        let service_key = super::crypto::decode_base64(&args.security)?;
        if !matches!(service_key.len(), 16 | 24 | 32) {
            return Err(SliveError::invalid_input(format!(
                "AcFun 弹幕安全密钥长度非法: {}",
                service_key.len()
            )));
        }
        let ticket = args
            .tickets
            .first()
            .filter(|ticket| !ticket.is_empty())
            .cloned()
            .ok_or_else(|| SliveError::invalid_input("AcFun 未下发弹幕票据"))?;
        if args.live_id.is_empty() {
            return Err(SliveError::invalid_input("AcFun 直播 id 为空"));
        }
        Ok(Self {
            app_id: APP_ID,
            uid: args.uid,
            did: args.did.clone(),
            kpn: KPN.to_string(),
            sub_biz: SUB_BIZ.to_string(),
            service_token,
            service_key,
            session_key: None,
            instance_id: 0,
            seq: 1,
            zt_sequence: 0,
            ticket,
            live_id: args.live_id.clone(),
            attach: args.enter_room_attach.clone(),
        })
    }

    fn next_seq(&mut self) -> u64 {
        let seq = self.seq;
        self.seq = self.seq.saturating_add(1);
        seq
    }

    fn session_key(&self) -> Result<&[u8], SliveError> {
        self.session_key
            .as_deref()
            .ok_or_else(|| SliveError::decode("会话密钥未就绪"))
    }

    fn header(
        &self,
        encryption_mode: EncryptionMode,
        seq_id: u64,
        payload_len: u32,
    ) -> PacketHeader {
        PacketHeader {
            app_id: self.app_id,
            uid: self.uid,
            instance_id: self.instance_id,
            decoded_payload_len: payload_len,
            encryption_mode,
            seq_id,
            kpn: self.kpn.clone(),
            service_token: (encryption_mode == EncryptionMode::ServiceToken)
                .then(|| self.service_token.clone()),
        }
    }

    fn frame(
        &self,
        encryption_mode: EncryptionMode,
        command: &str,
        seq_id: u64,
        payload: &[u8],
    ) -> Result<Vec<u8>, SliveError> {
        let key = match encryption_mode {
            EncryptionMode::ServiceToken => &self.service_key,
            EncryptionMode::SessionKey => self.session_key()?,
            EncryptionMode::None => &[],
        };
        let header = self.header(encryption_mode, seq_id, payload.len() as u32);
        encode_upstream(&header, key, command, seq_id, &self.sub_biz, payload)
    }

    fn register_frame(&mut self) -> Result<Vec<u8>, SliveError> {
        let payload = register_request(APP_NAME, LINK_VERSION, &self.did, self.uid, &self.kpn, KPF);
        let seq = self.next_seq();
        self.frame(EncryptionMode::ServiceToken, CMD_REGISTER, seq, &payload)
    }

    fn enter_room_frame(&mut self) -> Result<Vec<u8>, SliveError> {
        let payload = enter_room_payload(&self.attach, SDK_VERSION);
        let cmd = cs_cmd(CS_ENTER_ROOM, Some(&payload), &self.ticket, &self.live_id);
        let seq = self.next_seq();
        self.frame(EncryptionMode::SessionKey, CMD_ZT_CS, seq, &cmd)
    }

    fn heartbeat_frame(&mut self, now_ms: u64) -> Result<Vec<u8>, SliveError> {
        let payload = heartbeat_payload(self.zt_sequence, now_ms);
        self.zt_sequence = self.zt_sequence.saturating_add(1);
        let cmd = cs_cmd(CS_HEARTBEAT, Some(&payload), &self.ticket, &self.live_id);
        let seq = self.next_seq();
        self.frame(EncryptionMode::SessionKey, CMD_ZT_CS, seq, &cmd)
    }

    fn keepalive_frame(&mut self) -> Result<Vec<u8>, SliveError> {
        let seq = self.next_seq();
        self.frame(
            EncryptionMode::SessionKey,
            CMD_KEEPALIVE,
            seq,
            &keepalive_payload(),
        )
    }

    fn exit_frame(&mut self) -> Result<Vec<u8>, SliveError> {
        let cmd = cs_cmd(CS_USER_EXIT, None, &self.ticket, &self.live_id);
        let seq = self.next_seq();
        self.frame(EncryptionMode::SessionKey, CMD_ZT_CS, seq, &cmd)
    }

    /// 推送确认：header 的 seq 用推送帧的 seq，业务载荷为空。
    ///
    /// 应答命令必须回显推送自己的命令（`Push.*` 不止一种），只认
    /// `Push.ZtLiveInteractive.Message` 会让其余推送得不到确认。
    fn push_ack_frame(&self, command: &str, push_seq: u64) -> Result<Vec<u8>, SliveError> {
        let header = self.header(EncryptionMode::SessionKey, push_seq, 0);
        encode_upstream(
            &header,
            self.session_key()?,
            command,
            self.seq,
            &self.sub_biz,
            &[],
        )
    }

    fn decode(&self, frame: &[u8]) -> Result<DownstreamFrame, SliveError> {
        decode_downstream(frame, &self.service_key, self.session_key.as_deref())
    }
}

/// 启动 link 会话（每次重连都重新注册并进房）。
pub async fn run_session(
    args: ZtLiveDanmakuArgs,
    mut connect: ConnectFactory,
    tuning: SessionTuning,
    mut stop: watch::Receiver<bool>,
    mut on_item: impl FnMut(LiveDanmuItem) + Send,
) {
    let mut attempt: u32 = 0;
    'session: loop {
        if *stop.borrow_and_update() {
            on_item(LiveDanmuItem::Control(LiveDanmuControlEvent::Closed(
                "已手动停止".to_string(),
            )));
            return;
        }
        let mut link = match Link::new(&args) {
            Ok(link) => link,
            Err(error) => {
                on_item(LiveDanmuItem::Control(LiveDanmuControlEvent::Closed(
                    error.to_string(),
                )));
                return;
            }
        };
        let mut source = match (connect)().await {
            Ok(source) => source,
            Err(error) => {
                log::warn!("[ztlive] 连接失败: {error}");
                if reconnect_or_close(&mut attempt, &tuning, &mut on_item).await {
                    return;
                }
                continue 'session;
            }
        };

        let interval_ms = match handshake(&mut link, &mut *source).await {
            Ok(interval_ms) => interval_ms,
            Err(error) => {
                log::warn!("[ztlive] 握手失败: {error}");
                if reconnect_or_close(&mut attempt, &tuning, &mut on_item).await {
                    return;
                }
                continue 'session;
            }
        };
        attempt = 0;
        on_item(LiveDanmuItem::Control(LiveDanmuControlEvent::Ready));

        match main_loop(
            &mut link,
            &mut *source,
            interval_ms,
            &mut stop,
            &mut on_item,
        )
        .await
        {
            LoopEnd::Stopped => {
                on_item(LiveDanmuItem::Control(LiveDanmuControlEvent::Closed(
                    "已手动停止".to_string(),
                )));
                return;
            }
            LoopEnd::Closed(reason) => {
                on_item(LiveDanmuItem::Control(LiveDanmuControlEvent::Closed(
                    reason,
                )));
                return;
            }
            LoopEnd::Reconnect => {
                if reconnect_or_close(&mut attempt, &tuning, &mut on_item).await {
                    return;
                }
                continue 'session;
            }
        }
    }
}

async fn handshake(link: &mut Link, source: &mut dyn FrameSource) -> Result<u64, SliveError> {
    source.send(link.register_frame()?).await?;
    let response = wait_for(source, link, CMD_REGISTER).await?;
    if response.error_code != 0 {
        return Err(SliveError::Api {
            code: response.error_code,
            message: if response.error_message.is_empty() {
                "link 注册失败".to_string()
            } else {
                response.error_message
            },
        });
    }
    let (session_key, instance_id) = register_response(&response.payload)?;
    link.session_key = Some(session_key);
    link.instance_id = instance_id;

    source.send(link.enter_room_frame()?).await?;
    let response = wait_for(source, link, CMD_ZT_CS).await?;
    if response.error_code != 0 {
        return Err(SliveError::Api {
            code: response.error_code,
            message: response.error_message,
        });
    }
    let ack = parse_cs_ack(&response.payload)?;
    if ack.error_code != 0 {
        return Err(SliveError::Api {
            code: ack.error_code,
            message: if ack.error_message.is_empty() {
                "进房失败".to_string()
            } else {
                ack.error_message
            },
        });
    }
    Ok(ack
        .heartbeat_interval_ms
        .clamp(MIN_HEARTBEAT_MS, MAX_HEARTBEAT_MS))
}

async fn wait_for(
    source: &mut dyn FrameSource,
    link: &Link,
    command: &str,
) -> Result<DownstreamFrame, SliveError> {
    timeout(RESPONSE_TIMEOUT, async {
        loop {
            let frame = source
                .next_frame()
                .await
                .ok_or_else(|| SliveError::network("连接已关闭"))??;
            let downstream = link.decode(&frame)?;
            if downstream.command == command {
                return Ok(downstream);
            }
            log::debug!(
                "[ztlive] 握手期忽略帧: command={:?} error={} msg={:?} mode={:?} seq={}",
                downstream.command,
                downstream.error_code,
                downstream.error_message,
                downstream.header.encryption_mode,
                downstream.header.seq_id
            );
        }
    })
    .await
    .map_err(|_| SliveError::network(format!("等待 {command} 应答超时")))?
}

struct CsAck {
    error_code: i32,
    error_message: String,
    heartbeat_interval_ms: u64,
}

fn parse_cs_ack(payload: &[u8]) -> Result<CsAck, SliveError> {
    let fields = parse_fields(payload)?;
    let error_code = get_u64(&fields, 2).unwrap_or(0) as i32;
    let error_message = get_string(&fields, 3).unwrap_or_default();
    let heartbeat_interval_ms = match find(&fields, 4).and_then(Field::as_bytes) {
        Some(body) => match enter_room_ack(body) {
            Ok(interval) => interval,
            Err(error) => {
                log::warn!(
                    "[ztlive] 进房应答心跳间隔解析失败，改用 {DEFAULT_HEARTBEAT_MS}ms: {error}"
                );
                DEFAULT_HEARTBEAT_MS
            }
        },
        None => DEFAULT_HEARTBEAT_MS,
    };
    Ok(CsAck {
        error_code,
        error_message,
        heartbeat_interval_ms,
    })
}

enum LoopEnd {
    Stopped,
    Closed(String),
    Reconnect,
}

async fn main_loop(
    link: &mut Link,
    source: &mut dyn FrameSource,
    heartbeat_ms: u64,
    stop: &mut watch::Receiver<bool>,
    on_item: &mut (dyn FnMut(LiveDanmuItem) + Send),
) -> LoopEnd {
    let mut heartbeat = tokio::time::interval(Duration::from_millis(heartbeat_ms));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut keepalive = tokio::time::interval(Duration::from_secs(KEEPALIVE_SECS));
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_online: Option<i32> = None;
    loop {
        tokio::select! {
            biased;
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow_and_update() {
                    if let Ok(frame) = link.exit_frame() {
                        let _ = source.send(frame).await;
                    }
                    return LoopEnd::Stopped;
                }
            }
            _ = heartbeat.tick() => {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as u64)
                    .unwrap_or(0);
                match link.heartbeat_frame(now_ms) {
                    Ok(frame) => {
                        if source.send(frame).await.is_err() {
                            return LoopEnd::Reconnect;
                        }
                    }
                    Err(error) => log::warn!("[ztlive] 心跳构造失败: {error}"),
                }
            }
            _ = keepalive.tick() => {
                match link.keepalive_frame() {
                    Ok(frame) => {
                        if source.send(frame).await.is_err() {
                            return LoopEnd::Reconnect;
                        }
                    }
                    Err(error) => log::warn!("[ztlive] 保活构造失败: {error}"),
                }
            }
            frame = source.next_frame() => {
                let Some(Ok(frame)) = frame else {
                    return LoopEnd::Reconnect;
                };
                let downstream = match link.decode(&frame) {
                    Ok(downstream) => downstream,
                    Err(error) => {
                        log::warn!("[ztlive] 帧解码失败: {error}");
                        continue;
                    }
                };
                if !downstream.command.starts_with("Push.") {
                    continue;
                }
                match link.push_ack_frame(&downstream.command, downstream.header.seq_id) {
                    Ok(ack) => {
                        if let Err(error) = source.send(ack).await {
                            log::warn!("[ztlive] 推送确认发送失败: {error}");
                            return LoopEnd::Reconnect;
                        }
                    }
                    Err(error) => log::warn!("[ztlive] 推送确认构造失败: {error}"),
                }
                if downstream.command != CMD_PUSH {
                    continue;
                }
                match parse_push(&downstream.payload)
                    .and_then(|push| push_outcome(&push, &mut last_online))
                {
                    Ok(outcome) => {
                        for message in outcome.messages {
                            on_item(LiveDanmuItem::Message(Box::new(message)));
                        }
                        if let Some(reason) = outcome.closed {
                            return LoopEnd::Closed(reason);
                        }
                        if outcome.ticket_invalid {
                            return LoopEnd::Closed("弹幕票据失效，请重新进入直播间".to_string());
                        }
                    }
                    Err(error) => log::warn!("[ztlive] 推送解析失败: {error}"),
                }
            }
        }
    }
}

async fn reconnect_or_close(
    attempt: &mut u32,
    tuning: &SessionTuning,
    on_item: &mut (dyn FnMut(LiveDanmuItem) + Send),
) -> bool {
    *attempt += 1;
    if *attempt > tuning.max_reconnect {
        on_item(LiveDanmuItem::Control(LiveDanmuControlEvent::Closed(
            "重连超过最大次数".to_string(),
        )));
        true
    } else {
        on_item(LiveDanmuItem::Control(
            LiveDanmuControlEvent::Reconnecting { attempt: *attempt },
        ));
        tokio::time::sleep(Duration::from_secs(tuning.reconnect_delay_secs)).await;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::LiveMessage;
    use crate::platforms::ztlive::frame::{MAGIC, VERSION};
    use crate::platforms::ztlive::proto::Writer;
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    const SERVICE_KEY: [u8; 16] = *b"0123456789abcdef";
    const SESSION_KEY: [u8; 16] = *b"fedcba9876543210";

    struct ScriptedSource {
        frames: VecDeque<Vec<u8>>,
        sent: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    #[async_trait]
    impl FrameSource for ScriptedSource {
        async fn next_frame(&mut self) -> Option<Result<Vec<u8>, SliveError>> {
            self.frames.pop_front().map(Ok)
        }

        async fn send(&mut self, frame: Vec<u8>) -> Result<(), SliveError> {
            self.sent.lock().expect("帧记录锁").push(frame);
            Ok(())
        }
    }

    fn args() -> ZtLiveDanmakuArgs {
        ZtLiveDanmakuArgs {
            live_id: "1000".to_string(),
            uid: 42,
            did: "web_0123456789abcdef".to_string(),
            service_token: "token".to_string(),
            // base64("0123456789abcdef")
            security: "MDEyMzQ1Njc4OWFiY2RlZg==".to_string(),
            tickets: vec!["ticket".to_string()],
            enter_room_attach: "attach".to_string(),
        }
    }

    /// 手写服务端帧：`DownstreamPayload` 的 field 3 是 errorCode，
    /// 而客户端编码器会写 retryCount=1，不能复用。
    fn downstream_frame(
        mode: EncryptionMode,
        key: &[u8],
        command: &str,
        seq: u64,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut body = Writer::new();
        body.string(1, command);
        body.uint64(2, seq);
        body.uint32(3, 0);
        body.bytes(4, payload);
        let plain = body.finish();
        let plain_len = plain.len();
        let sealed = match mode {
            EncryptionMode::None => plain,
            EncryptionMode::ServiceToken | EncryptionMode::SessionKey => {
                super::super::crypto::encrypt(&plain, key).expect("加密服务端帧")
            }
        };
        let mut header = Writer::new();
        header.uint32(1, APP_ID as u32);
        header.uint32(7, plain_len as u32);
        header.uint32(
            8,
            match mode {
                EncryptionMode::None => 0,
                EncryptionMode::ServiceToken => 1,
                EncryptionMode::SessionKey => 2,
            },
        );
        header.uint64(10, seq);
        header.string(12, KPN);
        let header = header.finish();

        let mut frame = Vec::with_capacity(FRAME_PREFIX + header.len() + sealed.len());
        frame.extend_from_slice(&MAGIC.to_be_bytes());
        frame.extend_from_slice(&VERSION.to_be_bytes());
        frame.extend_from_slice(&(header.len() as u32).to_be_bytes());
        frame.extend_from_slice(&(sealed.len() as u32).to_be_bytes());
        frame.extend_from_slice(&header);
        frame.extend_from_slice(&sealed);
        frame
    }

    const FRAME_PREFIX: usize = 12;

    fn chat_push(text: &str) -> Vec<u8> {
        let mut user = Writer::new();
        user.uint64(1, 7);
        user.string(2, "观众");
        let mut comment = Writer::new();
        comment.string(1, text);
        comment.uint64(2, 1_700_000_000_000);
        comment.bytes(3, &user.finish());
        let mut signal = Writer::new();
        signal.string(1, "CommonActionSignalComment");
        signal.bytes(2, &comment.finish());
        let mut action = Writer::new();
        action.bytes(1, &signal.finish());
        let mut message = Writer::new();
        message.string(1, "ZtLiveScActionSignal");
        message.uint32(2, 0);
        message.bytes(3, &action.finish());
        message.finish()
    }

    #[tokio::test]
    async fn handshake_receives_chat_and_acks_every_push() {
        let mut register = Writer::new();
        register.bytes(2, &SESSION_KEY);
        register.uint64(3, 77);
        let register = register.finish();

        let mut enter_ack = Writer::new();
        enter_ack.uint64(1, 5_000);
        let mut enter = Writer::new();
        enter.string(1, "ZtLiveCsEnterRoomAck");
        enter.uint32(2, 0);
        enter.bytes(4, &enter_ack.finish());
        let enter = enter.finish();

        let frames = vec![
            downstream_frame(
                EncryptionMode::ServiceToken,
                &SERVICE_KEY,
                CMD_REGISTER,
                1,
                &register,
            ),
            downstream_frame(
                EncryptionMode::SessionKey,
                &SESSION_KEY,
                CMD_ZT_CS,
                2,
                &enter,
            ),
            downstream_frame(
                EncryptionMode::SessionKey,
                &SESSION_KEY,
                CMD_PUSH,
                3,
                &chat_push("hi"),
            ),
            downstream_frame(
                EncryptionMode::SessionKey,
                &SESSION_KEY,
                "Push.ZtLiveInteractive.Other",
                4,
                &[],
            ),
        ];

        let sent = Arc::new(Mutex::new(Vec::new()));
        let connect: crate::ws::ConnectFactory = {
            let sent = Arc::clone(&sent);
            Box::new(move || {
                let frames = frames.clone();
                let sent = Arc::clone(&sent);
                Box::pin(async move {
                    let source: Box<dyn FrameSource> = Box::new(ScriptedSource {
                        frames: frames.into(),
                        sent,
                    });
                    Ok::<Box<dyn FrameSource>, SliveError>(source)
                })
            })
        };

        let (_tx, stop) = watch::channel(false);
        let mut tuning = SessionTuning::bilibili();
        tuning.max_reconnect = 0;
        tuning.reconnect_delay_secs = 0;

        let mut items = Vec::new();
        run_session(args(), connect, tuning, stop, |item| items.push(item)).await;

        assert!(items
            .iter()
            .any(|item| matches!(item, LiveDanmuItem::Control(LiveDanmuControlEvent::Ready))));
        let chat: Vec<&LiveMessage> = items
            .iter()
            .filter_map(|item| match item {
                LiveDanmuItem::Message(message) => Some(message.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(chat.len(), 1);
        assert_eq!(chat[0].message, "hi");
        assert_eq!(chat[0].user_name, "观众");

        let sent = sent.lock().expect("帧记录锁").clone();
        let decoded: Vec<DownstreamFrame> = sent
            .iter()
            .map(|frame| {
                decode_downstream(frame, &SERVICE_KEY, Some(&SESSION_KEY)).expect("解码已发帧")
            })
            .collect();
        let commands: Vec<&str> = decoded.iter().map(|frame| frame.command.as_str()).collect();
        assert!(commands.contains(&CMD_REGISTER));
        assert!(commands.contains(&CMD_ZT_CS), "进房与心跳都走 CsCmd");
        let acks: Vec<(String, u64)> = decoded
            .iter()
            .filter(|frame| frame.command.starts_with("Push."))
            .map(|frame| (frame.command.clone(), frame.header.seq_id))
            .collect();
        assert_eq!(
            acks,
            vec![
                (CMD_PUSH.to_string(), 3),
                ("Push.ZtLiveInteractive.Other".to_string(), 4),
            ],
            "每条 Push.* 都要被确认，且回显自己的命令与 seq"
        );
    }
}
