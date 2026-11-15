//! ZtLive 业务消息：注册 / 进房 / 心跳载荷构造与推送解析。

use flate2::read::GzDecoder;
use std::io::Read;

use crate::error::SliveError;
use crate::models::{LiveMessage, LiveMessageColor, LiveMessageData, LiveMessageType};

use super::proto::{find, find_all, get_message, get_string, get_u64, parse_fields, Field, Writer};

/// 注册命令。
pub const CMD_REGISTER: &str = "Basic.Register";
/// 保活命令。
pub const CMD_KEEPALIVE: &str = "Basic.KeepAlive";
/// ZtLive 客户端命令。
pub const CMD_ZT_CS: &str = "Global.ZtLiveInteractive.CsCmd";
/// 服务端推送命令。
pub const CMD_PUSH: &str = "Push.ZtLiveInteractive.Message";

/// 进房命令。
pub const CS_ENTER_ROOM: &str = "ZtLiveCsEnterRoom";
/// 心跳命令。
pub const CS_HEARTBEAT: &str = "ZtLiveCsHeartbeat";
/// 退房命令。
pub const CS_USER_EXIT: &str = "ZtLiveCsUserExit";

const COMPRESSION_GZIP: u64 = 2;

/// 构造 `RegisterRequest`。
pub fn register_request(
    app_name: &str,
    sdk_version: &str,
    device_id: &str,
    uid: u64,
    kpn: &str,
    kpf: &str,
) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.message(1, |info| {
        info.string(1, app_name);
        info.string(4, sdk_version);
    });
    writer.message(2, |device| {
        device.uint32(1, 6); // kPlatformH5
        device.string(3, "h5");
        device.string(5, device_id);
    });
    writer.uint32(4, 1); // presenceStatus = online
    writer.uint32(5, 1); // appActiveStatus = foreground
    writer.uint64(8, 0);
    writer.message(11, |common| {
        common.string(1, kpn);
        common.string(2, kpf);
        common.uint64(4, uid);
        common.string(5, device_id);
    });
    writer.finish()
}

/// 解析 `RegisterResponse`，返回会话密钥与实例 id。
pub fn register_response(payload: &[u8]) -> Result<(Vec<u8>, u64), SliveError> {
    let fields = parse_fields(payload)?;
    let session_key = find(&fields, 2)
        .and_then(Field::as_bytes)
        .filter(|key| !key.is_empty())
        .map(<[u8]>::to_vec)
        .ok_or_else(|| SliveError::decode("RegisterResponse 缺少会话密钥"))?;
    let instance_id = get_u64(&fields, 3).unwrap_or(0);
    Ok((session_key, instance_id))
}

/// 构造 `ZtLiveCsCmd`（进房 / 心跳 / 退房共用）。
pub fn cs_cmd(cmd_type: &str, payload: Option<&[u8]>, ticket: &str, live_id: &str) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.string(1, cmd_type);
    if let Some(payload) = payload {
        writer.bytes(2, payload);
    }
    writer.string(3, ticket);
    writer.string(4, live_id);
    writer.finish()
}

/// 构造 `ZtLiveCsEnterRoom` 载荷。
pub fn enter_room_payload(enter_room_attach: &str, sdk_version: &str) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.boolean(1, false);
    writer.uint32(2, 0);
    writer.uint32(3, 0);
    writer.string(4, enter_room_attach);
    writer.string(5, sdk_version);
    writer.finish()
}

/// 解析 `ZtLiveCsEnterRoomAck`，返回心跳间隔毫秒数。
pub fn enter_room_ack(payload: &[u8]) -> Result<u64, SliveError> {
    let fields = parse_fields(payload)?;
    Ok(get_u64(&fields, 1).unwrap_or(0))
}

/// 构造 `ZtLiveCsHeartbeat` 载荷。
pub fn heartbeat_payload(sequence: u64, client_timestamp_ms: u64) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.uint64(1, client_timestamp_ms);
    writer.uint64(2, sequence);
    writer.finish()
}

/// 构造 `KeepAliveRequest`。
pub fn keepalive_payload() -> Vec<u8> {
    let mut writer = Writer::new();
    writer.uint32(1, 1);
    writer.uint32(2, 1);
    writer.finish()
}

/// 推送的外层结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushMessage {
    /// 消息类型（`ZtLiveScActionSignal` 等）。
    pub message_type: String,
    /// 压缩方式（2=GZIP）。
    pub compression_type: u64,
    /// 压缩前的业务载荷。
    pub payload: Vec<u8>,
}

/// 解析 `ZtLiveScMessage`。
pub fn parse_push(payload: &[u8]) -> Result<PushMessage, SliveError> {
    let fields = parse_fields(payload)?;
    Ok(PushMessage {
        message_type: get_string(&fields, 1).unwrap_or_default(),
        compression_type: get_u64(&fields, 2).unwrap_or(0),
        payload: find(&fields, 3)
            .and_then(Field::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default(),
    })
}

impl PushMessage {
    /// 解压业务载荷（GZIP）。
    pub fn body(&self) -> Result<Vec<u8>, SliveError> {
        if self.compression_type != COMPRESSION_GZIP {
            return Ok(self.payload.clone());
        }
        let mut decoder = GzDecoder::new(self.payload.as_slice());
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|e| SliveError::decode(format!("弹幕 GZIP 解压失败: {e}")))?;
        Ok(out)
    }
}

/// 一条推送的处理结果。
#[derive(Debug, Default)]
pub struct PushOutcome {
    /// 可直接上抛的聊天 / 人气消息。
    pub messages: Vec<LiveMessage>,
    /// 票据失效：会话应结束并由上层重新握手。
    pub ticket_invalid: bool,
    /// 直播结束（携带原因）。
    pub closed: Option<String>,
}

/// 解析一条推送为业务消息。
///
/// `last_online` 用于抑制重复的人气消息：状态信号会高频重发同一个值。
pub fn push_outcome(
    push: &PushMessage,
    last_online: &mut Option<i32>,
) -> Result<PushOutcome, SliveError> {
    let mut outcome = PushOutcome::default();
    match push.message_type.as_str() {
        "ZtLiveScTicketInvalid" => outcome.ticket_invalid = true,
        "ZtLiveScStatusChanged" => {
            let body = push.body()?;
            let fields = parse_fields(&body)?;
            match get_u64(&fields, 1).unwrap_or(0) {
                1 => outcome.closed = Some("直播已结束".to_string()),
                // 新直播开播：票据与 liveId 都换了，当前会话无法续用。
                2 => outcome.closed = Some("直播已重新开播，请重新进入直播间".to_string()),
                4 => outcome.closed = Some("直播被封禁".to_string()),
                _ => {}
            }
        }
        "ZtLiveScActionSignal" | "ZtLiveScNotifySignal" | "ZtLiveScStateSignal" => {
            let body = push.body()?;
            let fields = parse_fields(&body)?;
            for item in find_all(&fields, 1) {
                let Some(item_fields) = item.as_bytes().map(parse_fields).transpose()? else {
                    continue;
                };
                let signal_type = get_string(&item_fields, 1).unwrap_or_default();
                let payloads: Vec<&[u8]> = find_all(&item_fields, 2)
                    .filter_map(Field::as_bytes)
                    .collect();
                for payload in payloads {
                    if let Some(message) = signal_message(&signal_type, payload, last_online)? {
                        outcome.messages.push(message);
                    }
                }
            }
        }
        _ => {}
    }
    Ok(outcome)
}

fn signal_message(
    signal_type: &str,
    payload: &[u8],
    last_online: &mut Option<i32>,
) -> Result<Option<LiveMessage>, SliveError> {
    match signal_type {
        "CommonActionSignalComment" => {
            let fields = parse_fields(payload)?;
            let Some(content) = get_string(&fields, 1).filter(|text| !text.is_empty()) else {
                return Ok(None);
            };
            let (user_name, user_id) = get_message(&fields, 3)
                .map(|user| (get_string(&user, 2).unwrap_or_default(), get_u64(&user, 1)))
                .unwrap_or_default();
            Ok(Some(LiveMessage {
                message_type: LiveMessageType::Chat,
                user_name,
                user_id: user_id.map(|id| id.to_string()),
                message: content,
                data: None,
                color: LiveMessageColor::white(),
                emoticons: None,
            }))
        }
        "CommonStateSignalDisplayInfo" => {
            let fields = parse_fields(payload)?;
            let Some(raw) = get_string(&fields, 1) else {
                return Ok(None);
            };
            let Some(online) = parse_online_count(&raw) else {
                return Ok(None);
            };
            if *last_online == Some(online) {
                return Ok(None);
            }
            *last_online = Some(online);
            Ok(Some(LiveMessage {
                message_type: LiveMessageType::Online,
                user_name: String::new(),
                user_id: None,
                message: String::new(),
                data: Some(LiveMessageData::Online(online)),
                color: LiveMessageColor::white(),
                emoticons: None,
            }))
        }
        _ => Ok(None),
    }
}

/// 人气串归一：`"1234"` / `"1.2万"` / `"1 万+"` 都转成整数。
fn parse_online_count(raw: &str) -> Option<i32> {
    let cleaned = raw.trim().replace(['+', ',', ' '], "");
    let multiplier = if cleaned.contains('万') {
        10_000.0
    } else {
        1.0
    };
    let numeric: String = cleaned
        .chars()
        .filter(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect();
    let value = numeric.parse::<f64>().ok()? * multiplier;
    Some(value.round().clamp(0.0, i32::MAX as f64) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gzip(data: &[u8]) -> Vec<u8> {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).expect("压缩");
        encoder.finish().expect("压缩完成")
    }

    fn push(message_type: &str, body: &[u8], compression: u64) -> PushMessage {
        PushMessage {
            message_type: message_type.to_string(),
            compression_type: compression,
            payload: body.to_vec(),
        }
    }

    fn comment_signal(content: &str, name: &str, uid: u64) -> Vec<u8> {
        let mut user = Writer::new();
        user.uint64(1, uid);
        user.string(2, name);
        let user = user.finish();
        let mut comment = Writer::new();
        comment.string(1, content);
        comment.uint64(2, 1_700_000_000_000);
        comment.bytes(3, &user);
        let comment = comment.finish();

        let mut item = Writer::new();
        item.string(1, "CommonActionSignalComment");
        item.bytes(2, &comment);
        let item = item.finish();

        let mut signal = Writer::new();
        signal.bytes(1, &item);
        signal.finish()
    }

    #[test]
    fn comment_signal_becomes_chat_message() {
        let body = comment_signal("你好", "观众", 42);
        let outcome =
            push_outcome(&push("ZtLiveScActionSignal", &body, 0), &mut None).expect("解析");
        assert_eq!(outcome.messages.len(), 1);
        let message = &outcome.messages[0];
        assert_eq!(message.message_type, LiveMessageType::Chat);
        assert_eq!(message.user_name, "观众");
        assert_eq!(message.message, "你好");
        assert_eq!(message.user_id.as_deref(), Some("42"));
    }

    #[test]
    fn gzip_payload_is_inflated() {
        let body = comment_signal("压缩", "主播", 7);
        let compressed = gzip(&body);
        let outcome = push_outcome(
            &push("ZtLiveScActionSignal", &compressed, COMPRESSION_GZIP),
            &mut None,
        )
        .expect("解析");
        assert_eq!(outcome.messages[0].message, "压缩");
    }

    #[test]
    fn online_signal_is_deduplicated() {
        let mut signal = Writer::new();
        let mut display = Writer::new();
        display.string(1, "1234");
        let display = display.finish();
        let mut item = Writer::new();
        item.string(1, "CommonStateSignalDisplayInfo");
        item.bytes(2, &display);
        let item = item.finish();
        signal.bytes(1, &item);
        let body = signal.finish();

        let mut last = None;
        let first = push_outcome(&push("ZtLiveScStateSignal", &body, 0), &mut last).expect("解析");
        assert_eq!(first.messages[0].data, Some(LiveMessageData::Online(1234)));
        let second = push_outcome(&push("ZtLiveScStateSignal", &body, 0), &mut last).expect("解析");
        assert!(second.messages.is_empty());
    }

    #[test]
    fn ticket_invalid_is_flagged() {
        let outcome =
            push_outcome(&push("ZtLiveScTicketInvalid", b"", 0), &mut None).expect("解析");
        assert!(outcome.ticket_invalid);
    }

    #[test]
    fn live_closed_status_ends_session() {
        let mut status = Writer::new();
        status.uint32(1, 1);
        let outcome = push_outcome(
            &push("ZtLiveScStatusChanged", &status.finish(), 0),
            &mut None,
        )
        .expect("解析");
        assert_eq!(outcome.closed.as_deref(), Some("直播已结束"));
    }

    #[test]
    fn new_broadcast_status_ends_session() {
        let mut status = Writer::new();
        status.uint32(1, 2);
        let outcome = push_outcome(
            &push("ZtLiveScStatusChanged", &status.finish(), 0),
            &mut None,
        )
        .expect("解析");
        assert_eq!(
            outcome.closed.as_deref(),
            Some("直播已重新开播，请重新进入直播间")
        );
    }

    #[test]
    fn online_count_accepts_wan_and_plain_forms() {
        assert_eq!(parse_online_count("1234"), Some(1234));
        assert_eq!(parse_online_count("1.2万"), Some(12000));
        assert_eq!(parse_online_count("1 万+"), Some(10000));
        assert_eq!(parse_online_count("12,345"), Some(12345));
        assert_eq!(parse_online_count(""), None);
        assert_eq!(parse_online_count("暂无"), None);
    }

    #[test]
    fn register_response_reads_session_key_and_instance() {
        let mut writer = Writer::new();
        writer.bytes(2, &[9u8; 16]);
        writer.uint64(3, 88);
        let (key, instance) = register_response(&writer.finish()).expect("解析");
        assert_eq!(key, vec![9u8; 16]);
        assert_eq!(instance, 88);
    }

    #[test]
    fn enter_room_ack_reads_interval() {
        let mut writer = Writer::new();
        writer.uint64(1, 10_000);
        assert_eq!(enter_room_ack(&writer.finish()).expect("解析"), 10_000);
    }
}
