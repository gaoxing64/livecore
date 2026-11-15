//! YouTube live chat 轮询会话。

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::watch;

use crate::error::SliveError;
use crate::http::{HttpRequest, HttpTransport};
use crate::models::{LiveDanmuControlEvent, LiveDanmuItem};
use crate::platforms::youtube::api;
use crate::ws::session::SessionTuning;

use super::args::YouTubeDanmakuArgs;
use super::parse::{api_key, initial_continuation, parse_chat_page};

/// 观看页地址。
fn watch_url(video_id: &str) -> String {
    format!("https://www.youtube.com/watch?v={video_id}")
}

/// 把注入的 Cookie 并入已有 `cookie` 头（模板头里已有 `SOCS=CAI`）。
fn merge_cookie(mut headers: Vec<(String, String)>, cookie: Option<&str>) -> Vec<(String, String)> {
    let Some(cookie) = cookie.map(str::trim).filter(|text| !text.is_empty()) else {
        return headers;
    };
    match headers
        .iter_mut()
        .find(|(name, _)| name.eq_ignore_ascii_case("cookie"))
    {
        Some((_, value)) => {
            value.push_str("; ");
            value.push_str(cookie);
        }
        None => headers.push(("cookie".to_string(), cookie.to_string())),
    }
    headers
}

async fn open(
    http: &Arc<dyn HttpTransport>,
    video_id: &str,
    cookie: Option<&str>,
) -> Result<(String, String), SliveError> {
    let request = HttpRequest {
        url: watch_url(video_id),
        query: Vec::new(),
        headers: merge_cookie(api::page_headers(video_id), cookie),
    };
    let reply = http.get(&request).await?;
    let continuation = initial_continuation(&reply.text)
        .ok_or_else(|| SliveError::not_found("该视频当前没有直播聊天（可能未开播或聊天已关闭）"))?;
    let key = api_key(&reply.text).unwrap_or_else(|| api::FALLBACK_API_KEY.to_string());
    Ok((continuation, key))
}

/// `live_chat/get_live_chat` 地址。
///
/// `HttpTransport::post` 只读 URL、不拼 `query`，key 必须直接进 URL
/// （`api_url` 已内嵌 `?prettyPrint=false`）。
fn poll_url(api_key: &str) -> String {
    let url = api::api_url("live_chat/get_live_chat");
    if api_key.is_empty() {
        url
    } else {
        format!("{url}&key={}", crate::util::encode_query_component(api_key))
    }
}

async fn poll(
    http: &Arc<dyn HttpTransport>,
    token: &str,
    api_key: &str,
    cookie: Option<&str>,
) -> Result<crate::platforms::youtube::danmaku::parse::ChatPage, SliveError> {
    let body = json!({
        "context": api::web_context(),
        "continuation": token,
    })
    .to_string();
    let request = HttpRequest {
        url: poll_url(api_key),
        query: Vec::new(),
        headers: merge_cookie(api::api_headers(), cookie),
    };
    let reply = http.post(&request, &body, "application/json").await?;
    let value = reply.json()?;
    if let Some(error) = value.get("error") {
        let detail = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("live_chat 接口返回错误");
        let status = error
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let mut message = format!("live_chat 接口返回错误：{detail}");
        if !status.is_empty() {
            message.push_str(&format!("（{status}）"));
        }
        return Err(SliveError::Api { code: -1, message });
    }
    if value.get("continuationContents").is_none() {
        // 结构对不上 ≠ 聊天结束：按瞬时/风控失败处理，交给有界重连。
        return Err(SliveError::decode(
            "live_chat 响应缺少 continuationContents（可能被风控或接口改版）",
        ));
    }
    Ok(parse_chat_page(&value))
}

/// 运行弹幕会话（轮询直到停止 / 关闭 / 重连耗尽）。
pub async fn run_session(
    args: YouTubeDanmakuArgs,
    http: Arc<dyn HttpTransport>,
    tuning: SessionTuning,
    mut stop: watch::Receiver<bool>,
    mut on_item: impl FnMut(LiveDanmuItem) + Send,
) {
    let mut attempt: u32 = 0;
    // 重连会重新抓 initial continuation，首屏会回放最近聊天；按平台时间戳去重。
    let mut watermark_usec: Option<u64> = None;
    'session: loop {
        if *stop.borrow_and_update() {
            on_item(LiveDanmuItem::Control(LiveDanmuControlEvent::Closed(
                "已手动停止".to_string(),
            )));
            return;
        }
        let (mut token, api_key) = match open(&http, &args.video_id, args.cookie.as_deref()).await {
            Ok(pair) => pair,
            Err(error) => {
                log::warn!("[youtube-danmaku] 打开直播间失败: {error}");
                if reconnect_or_close(&mut attempt, &tuning, &mut on_item).await {
                    return;
                }
                continue 'session;
            }
        };
        attempt = 0;
        on_item(LiveDanmuItem::Control(LiveDanmuControlEvent::Ready));

        loop {
            if *stop.borrow_and_update() {
                on_item(LiveDanmuItem::Control(LiveDanmuControlEvent::Closed(
                    "已手动停止".to_string(),
                )));
                return;
            }
            match poll(&http, &token, &api_key, args.cookie.as_deref()).await {
                Ok(page) => {
                    for entry in page.messages {
                        let fresh = match (entry.timestamp_usec, watermark_usec) {
                            (Some(timestamp), Some(mark)) => timestamp > mark,
                            _ => true,
                        };
                        if let Some(timestamp) = entry.timestamp_usec {
                            watermark_usec =
                                Some(watermark_usec.map_or(timestamp, |mark| mark.max(timestamp)));
                        }
                        if fresh {
                            on_item(LiveDanmuItem::Message(Box::new(entry.message)));
                        }
                    }
                    let Some((next, timeout_ms)) = page.next else {
                        on_item(LiveDanmuItem::Control(LiveDanmuControlEvent::Closed(
                            "直播聊天已结束".to_string(),
                        )));
                        return;
                    };
                    token = next;
                    if sleep_or_stop(&mut stop, Duration::from_millis(timeout_ms)).await {
                        on_item(LiveDanmuItem::Control(LiveDanmuControlEvent::Closed(
                            "已手动停止".to_string(),
                        )));
                        return;
                    }
                }
                Err(error) => {
                    log::warn!("[youtube-danmaku] 轮询失败: {error}");
                    if reconnect_or_close(&mut attempt, &tuning, &mut on_item).await {
                        return;
                    }
                    continue 'session;
                }
            }
        }
    }
}

/// 等待下个轮询间隔；`true` 表示收到停止信号。
async fn sleep_or_stop(stop: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    tokio::select! {
        biased;
        changed = stop.changed() => changed.is_err() || *stop.borrow_and_update(),
        _ = tokio::time::sleep(delay) => false,
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
