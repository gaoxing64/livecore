//! YouTube 观看页与 live chat 响应解析（纯函数，无 I/O）。

use chrono::Local;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

use crate::models::{
    LiveMessage, LiveMessageColor, LiveMessageData, LiveMessageType, LiveSuperChatMessage,
};

static INITIAL_CONTINUATION: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#""liveChatRenderer":\{"continuations":\[\{"(?:reload|timed|invalidation)ContinuationData":\{"continuation":"([^"]+)""#,
    )
    .expect("liveChatRenderer 正则")
});

static API_KEY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#""INNERTUBE_API_KEY":"([^"]+)""#).expect("INNERTUBE_API_KEY 正则"));

/// 从观看页 HTML 解出初始聊天 continuation。
pub fn initial_continuation(html: &str) -> Option<String> {
    INITIAL_CONTINUATION
        .captures(html)
        .and_then(|captures| captures.get(1))
        .map(|m| unescape_json_string(m.as_str()))
        .filter(|token| !token.is_empty())
}

/// 从观看页 HTML 解出 InnerTube API key。
pub fn api_key(html: &str) -> Option<String> {
    API_KEY
        .captures(html)
        .and_then(|captures| captures.get(1))
        .map(|m| m.as_str().to_string())
        .filter(|key| !key.is_empty())
}

/// JSON 字符串转义还原（HTML 内联 JSON 常见 `\u003d` / `\u0026`）。
fn unescape_json_string(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Ok(code) = u32::from_str_radix(&hex, 16) {
                    if let Some(ch) = char::from_u32(code) {
                        out.push(ch);
                        continue;
                    }
                }
                out.push_str("\\u");
                out.push_str(&hex);
            }
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// 一条聊天记录：附带原始 `timestampUsec`（重连后的去重水位线）。
#[derive(Debug)]
pub struct ChatMessage {
    /// 平台下发的时间戳（微秒）。
    pub timestamp_usec: Option<u64>,
    /// 翻译后的弹幕。
    pub message: LiveMessage,
}

/// 一页聊天的解析结果。
#[derive(Debug, Default)]
pub struct ChatPage {
    /// 新增聊天 / 醒目留言。
    pub messages: Vec<ChatMessage>,
    /// 下一次轮询的 continuation 与间隔（毫秒）。
    pub next: Option<(String, u64)>,
}

const DEFAULT_POLL_MS: u64 = 5_000;
const MIN_POLL_MS: u64 = 1_000;
const MAX_POLL_MS: u64 = 10_000;

/// 解析 `live_chat/get_live_chat` 响应。
pub fn parse_chat_page(json: &Value) -> ChatPage {
    let Some(continuation) = json
        .pointer("/continuationContents/liveChatContinuation")
        .and_then(Value::as_object)
    else {
        return ChatPage::default();
    };

    let mut messages = Vec::new();
    if let Some(actions) = continuation.get("actions").and_then(Value::as_array) {
        for action in actions {
            let Some(item) = action
                .pointer("/addChatItemAction/item")
                .and_then(Value::as_object)
            else {
                continue;
            };
            if let Some(renderer) = item.get("liveChatTextMessageRenderer") {
                if let Some(message) = text_message(renderer) {
                    messages.push(message);
                }
            } else if let Some(renderer) = item.get("liveChatPaidMessageRenderer") {
                if let Some(message) = paid_message(renderer) {
                    messages.push(message);
                }
            } else if let Some(renderer) = item.get("liveChatTextMessageViewModel") {
                if let Some(message) = text_view_model(renderer) {
                    messages.push(message);
                }
            }
        }
    }

    let next = continuation
        .get("continuations")
        .and_then(Value::as_array)
        .and_then(|continuations| continuations.iter().find_map(next_token));

    ChatPage { messages, next }
}

fn next_token(continuation: &Value) -> Option<(String, u64)> {
    for (key, timeout) in [
        ("timedContinuationData", true),
        ("invalidationContinuationData", false),
        ("reloadContinuationData", false),
    ] {
        let Some(data) = continuation.get(key) else {
            continue;
        };
        let Some(token) = data.get("continuation").and_then(Value::as_str) else {
            continue;
        };
        let timeout_ms = if timeout {
            data.get("timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_POLL_MS)
        } else {
            DEFAULT_POLL_MS
        };
        return Some((
            token.to_string(),
            timeout_ms.clamp(MIN_POLL_MS, MAX_POLL_MS),
        ));
    }
    None
}

fn author_name(renderer: &Value) -> String {
    if let Some(name) = renderer
        .pointer("/authorName/simpleText")
        .and_then(Value::as_str)
    {
        return name.to_string();
    }
    if let Some(runs) = renderer
        .pointer("/authorName/runs")
        .and_then(Value::as_array)
    {
        let text = runs_text(runs);
        if !text.is_empty() {
            return text;
        }
    }
    renderer
        .get("authorName")
        .and_then(Value::as_str)
        .or_else(|| {
            renderer
                .pointer("/authorName/content")
                .and_then(Value::as_str)
        })
        .unwrap_or_default()
        .to_string()
}

fn runs_text(runs: &[Value]) -> String {
    let mut text = String::new();
    for run in runs {
        if let Some(part) = run.get("text").and_then(Value::as_str) {
            text.push_str(part);
            continue;
        }
        if let Some(emoji) = run.get("emoji") {
            if let Some(label) = emoji
                .pointer("/image/accessibility/accessibilityData/label")
                .and_then(Value::as_str)
            {
                text.push_str(label);
            }
        }
    }
    text
}

fn message_text(renderer: &Value) -> String {
    renderer
        .pointer("/message/runs")
        .and_then(Value::as_array)
        .map(|runs| runs_text(runs))
        .unwrap_or_default()
}

fn timestamp_usec(renderer: &Value) -> Option<u64> {
    let value = renderer.get("timestampUsec")?;
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<u64>().ok()))
}

fn text_message(renderer: &Value) -> Option<ChatMessage> {
    let message = message_text(renderer);
    if message.is_empty() {
        return None;
    }
    Some(ChatMessage {
        timestamp_usec: timestamp_usec(renderer),
        message: LiveMessage {
            message_type: LiveMessageType::Chat,
            user_name: author_name(renderer),
            user_id: renderer
                .get("authorExternalChannelId")
                .and_then(Value::as_str)
                .map(str::to_string),
            message,
            data: None,
            color: LiveMessageColor::white(),
            emoticons: None,
        },
    })
}

fn text_view_model(renderer: &Value) -> Option<ChatMessage> {
    let message = renderer
        .pointer("/text/content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if message.is_empty() {
        return None;
    }
    let user_name = renderer
        .pointer("/authorName/content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    Some(ChatMessage {
        timestamp_usec: None,
        message: LiveMessage {
            message_type: LiveMessageType::Chat,
            user_name,
            user_id: None,
            message,
            data: None,
            color: LiveMessageColor::white(),
            emoticons: None,
        },
    })
}

fn paid_message(renderer: &Value) -> Option<ChatMessage> {
    let message = message_text(renderer);
    let user_name = author_name(renderer);
    if message.is_empty() && user_name.is_empty() {
        return None;
    }
    let timestamp = timestamp_usec(renderer);
    let amount = renderer
        .pointer("/purchaseAmountText/simpleText")
        .and_then(Value::as_str)
        .map(parse_amount)
        .unwrap_or(0);
    let start = timestamp
        .and_then(|micros| chrono::DateTime::from_timestamp_micros(i64::try_from(micros).ok()?))
        .map(|utc| utc.with_timezone(&Local))
        .unwrap_or_else(Local::now);
    let duration = match amount {
        value if value >= 500 => 600,
        value if value >= 100 => 300,
        value if value >= 50 => 180,
        value if value >= 10 => 120,
        _ => 60,
    };
    let background = renderer
        .get("headerBackgroundColor")
        .and_then(Value::as_i64)
        .map(|color| format!("#{:06x}", (color as u64) & 0x00ff_ffff))
        .unwrap_or_else(|| "#1e88e5".to_string());
    Some(ChatMessage {
        timestamp_usec: timestamp,
        message: LiveMessage {
            message_type: LiveMessageType::SuperChat,
            user_name: user_name.clone(),
            user_id: None,
            message: message.clone(),
            data: Some(LiveMessageData::SuperChat(LiveSuperChatMessage {
                user_name,
                face: String::new(),
                message,
                price: amount,
                start_time: start,
                end_time: start + chrono::Duration::seconds(duration),
                background_color: background.clone(),
                background_bottom_color: background,
            })),
            color: LiveMessageColor::white(),
            emoticons: None,
        },
    })
}

fn parse_amount(text: &str) -> i32 {
    let numeric: String = text
        .chars()
        .filter(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect();
    numeric
        .parse::<f64>()
        .map(|value| value.round() as i32)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn initial_continuation_is_extracted_and_unescaped() {
        let html = r#"xx{"liveChatRenderer":{"continuations":[{"reloadContinuationData":{"continuation":"abc%3D%3D"}}]},"y":"#;
        let token = initial_continuation(html).expect("continuation");
        assert_eq!(token, "abc%3D%3D");

        let html = r#"{"liveChatRenderer":{"continuations":[{"timedContinuationData":{"continuation":"a\u003db"}}]}"#;
        assert_eq!(initial_continuation(html).as_deref(), Some("a=b"));
    }

    #[test]
    fn missing_continuation_is_none() {
        assert!(initial_continuation("<html></html>").is_none());
    }

    #[test]
    fn api_key_is_extracted() {
        assert_eq!(
            api_key(r#"{"INNERTUBE_API_KEY":"KEY123"}"#).as_deref(),
            Some("KEY123")
        );
    }

    #[test]
    fn chat_text_message_is_parsed() {
        let page = json!({
            "continuationContents": {"liveChatContinuation": {
                "actions": [{"addChatItemAction": {"item": {"liveChatTextMessageRenderer": {
                    "authorName": {"simpleText": "@alice"},
                    "authorExternalChannelId": "UC1",
                    "message": {"runs": [{"text": "hello "}, {"text": "world"}]},
                    "timestampUsec": "1700000000000000"
                }}}}],
                "continuations": [{"timedContinuationData": {"continuation": "next", "timeoutMs": 4810}}]
            }}
        });
        let parsed = parse_chat_page(&page);
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].message.message, "hello world");
        assert_eq!(parsed.messages[0].message.user_name, "@alice");
        assert_eq!(
            parsed.messages[0].timestamp_usec,
            Some(1_700_000_000_000_000)
        );
        assert_eq!(parsed.next, Some(("next".to_string(), 4810)));
    }

    #[test]
    fn view_model_message_is_parsed() {
        let page = json!({
            "continuationContents": {"liveChatContinuation": {
                "actions": [{"addChatItemAction": {"item": {"liveChatTextMessageViewModel": {
                    "authorName": {"content": "@bob "},
                    "text": {"content": "new protocol"}
                }}}}]
            }}
        });
        let parsed = parse_chat_page(&page);
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].message.user_name, "@bob");
        assert_eq!(parsed.messages[0].message.message, "new protocol");
        assert_eq!(parsed.messages[0].timestamp_usec, None);
        assert!(parsed.next.is_none());
    }

    #[test]
    fn paid_message_becomes_super_chat() {
        let page = json!({
            "continuationContents": {"liveChatContinuation": {
                "actions": [{"addChatItemAction": {"item": {"liveChatPaidMessageRenderer": {
                    "authorName": {"simpleText": "@carol"},
                    "message": {"runs": [{"text": "nice"}]},
                    "purchaseAmountText": {"simpleText": "$20.00"},
                    "headerBackgroundColor": 4294901760i64,
                    "timestampUsec": "1700000000000000"
                }}}}],
                "continuations": [{"timedContinuationData": {"continuation": "next"}}]
            }}
        });
        let parsed = parse_chat_page(&page);
        assert_eq!(parsed.messages.len(), 1);
        let message = &parsed.messages[0].message;
        assert_eq!(message.message_type, LiveMessageType::SuperChat);
        let Some(LiveMessageData::SuperChat(super_chat)) = &message.data else {
            panic!("缺少 SuperChat 数据");
        };
        assert_eq!(super_chat.price, 20);
        assert_eq!(super_chat.background_color, "#ff0000");
        assert!(super_chat.end_time > super_chat.start_time);
        assert_eq!(
            parsed.messages[0].timestamp_usec,
            Some(1_700_000_000_000_000)
        );
        assert_eq!(
            super_chat.start_time.timestamp_micros(),
            1_700_000_000_000_000,
            "SuperChat 起始时间取平台时间戳，不用本机时钟"
        );
    }

    #[test]
    fn empty_runs_are_ignored() {
        let page = json!({
            "continuationContents": {"liveChatContinuation": {
                "actions": [{"addChatItemAction": {"item": {"liveChatTextMessageRenderer": {
                    "authorName": {"simpleText": "@x"},
                    "message": {"runs": []}
                }}}}]
            }}
        });
        assert!(parse_chat_page(&page).messages.is_empty());
    }
}
