//! 小红书分享页端点与状态解析（对照 PureLive `xiaohongshu_api.dart`）。
//!
//! 没有公开目录 / 关键词搜索 / 弹幕：一场直播就是分享页
//! `www.xiaohongshu.com/livestream/<roomId>`，页内
//! `window.__INITIAL_STATE__=` 的 `liveStream` 携带卡片、状态与拉流地址。

use serde_json::Value;

use crate::error::SliveError;

/// Web 根。
pub const ORIGIN: &str = "https://www.xiaohongshu.com";
/// 3.x 的 Android Chrome UA。
pub const USER_AGENT: &str =
    "Mozilla/5.0 (Linux; Android 11) AppleWebKit/537.36 Chrome/87.0.4280.141 Mobile Safari/537.36";
/// 页面超过该字节数视为接口异常。
pub const RESPONSE_LIMIT: usize = 2 * 1024 * 1024;

/// 每场直播的提示：关注的是这一场。
pub const ROOM_SCOPE_NOTICE: &str =
    "小红书每场直播都有新的房间号，这里关注的是这一场。主播下次开播时，请重新导入新的分享链接。";
/// 受限提示。
pub const RESTRICTED_NOTICE: &str =
    "这场直播设置了观看条件（付费、仅限部分观众或限定地区），这里无法播放。";
/// 无法确认是否公开时的提示。
pub const UNKNOWN_ACCESS_NOTICE: &str = "暂时无法确认这场直播是否公开，这里可能无法播放。";

/// 页面 / 媒体请求头（无 cookie）。
pub fn headers() -> Vec<(String, String)> {
    vec![
        ("referer".to_string(), format!("{ORIGIN}/")),
        ("user-agent".to_string(), USER_AGENT.to_string()),
    ]
}

/// 分享页地址。
pub fn room_url(room_id: &str) -> String {
    format!("{ORIGIN}/livestream/{room_id}")
}

/// 房间 id：1–20 位数字且不以 0 开头。
pub fn is_room_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 20
        && bytes[0] != b'0'
        && bytes.iter().all(u8::is_ascii_digit)
}

/// 页内 hydration state（`liveStream` 对象）。
///
/// 找 `window.__INITIAL_STATE__=` 后的 JSON，按括号深度切片；
/// 字符串外的裸 `undefined` 换成 `null`（字符串里的词保持原样）。
pub fn extract_live_stream(html: &str) -> Result<Value, SliveError> {
    if html.len() > RESPONSE_LIMIT {
        return Err(SliveError::Api {
            code: -1,
            message: format!("分享页超过 {RESPONSE_LIMIT} 字节"),
        });
    }
    let marker = "window.__INITIAL_STATE__=";
    let marker_at = html.find(marker).ok_or_else(|| SliveError::Api {
        code: -1,
        message: "分享页没有 hydration state".to_string(),
    })?;
    let rest = &html[marker_at + marker.len()..];
    let state = slice_object(rest).ok_or_else(|| SliveError::Api {
        code: -1,
        message: "分享页 hydration state 不是完整对象".to_string(),
    })?;
    let decoded: Value = serde_json::from_str(&state).map_err(|e| SliveError::Api {
        code: -1,
        message: format!("分享页 hydration state 解析失败：{e}"),
    })?;
    let live_stream = decoded.get("liveStream").filter(|value| value.is_object()).ok_or_else(|| {
        SliveError::Api {
            code: -1,
            message: "分享页缺少 liveStream".to_string(),
        }
    })?;
    Ok(live_stream.clone())
}

/// 从 `{` 开始按深度切出完整对象；字符串外的裸 `undefined` 换成 `null`。
fn slice_object(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut start = 0;
    while start < bytes.len() && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    if bytes.get(start) != Some(&b'{') {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut copied = start;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut index = start;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    out.push_str(&text[copied..=index]);
                    return Some(out);
                }
            }
            _ => {
                if text[index..].starts_with("undefined") {
                    let boundary = |ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '$');
                    let before_ok = text[..index].chars().next_back().map(boundary).unwrap_or(true);
                    let after = index + "undefined".len();
                    let after_ok = text[after..].chars().next().map(boundary).unwrap_or(true);
                    if before_ok && after_ok {
                        out.push_str(&text[copied..index]);
                        out.push_str("null");
                        index = after;
                        copied = after;
                        continue;
                    }
                }
            }
        }
        index += 1;
    }
    None
}

/// 一条拉流地址（`pullConfig` 的一行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullStream {
    pub url: String,
    /// `h264` / `h265`。
    pub codec: &'static str,
    /// `quality_type`（同档位的两种编码算一档）。
    pub quality: String,
    /// `quality_type_name`（如「原画」）。
    pub label: String,
}

/// 解析 `pullConfig`：只接受公开 CDN 上该房间的 `/live/<room>.m3u8|.flv`。
pub fn parse_pull_config(value: Option<&Value>, room_id: &str) -> Result<Vec<PullStream>, SliveError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    if value.as_str() == Some("") {
        return Ok(Vec::new());
    }
    let config = match value {
        Value::String(text) => {
            if text.len() > 65536 {
                return Err(SliveError::Api {
                    code: -1,
                    message: "pullConfig 超过 65536 字符".to_string(),
                });
            }
            serde_json::from_str::<Value>(text).map_err(|error| SliveError::Api {
                code: -1,
                message: format!("pullConfig 不是 JSON：{error}"),
            })?
        }
        other => other.clone(),
    };
    if !config.is_object() {
        return Err(SliveError::Api {
            code: -1,
            message: "pullConfig 不是对象".to_string(),
        });
    }

    let mut streams = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut rows = 0usize;
    for codec in ["h264", "h265"] {
        let Some(list) = config.get(codec) else {
            continue;
        };
        let list = list.as_array().ok_or_else(|| SliveError::Api {
            code: -1,
            message: format!("pullConfig.{codec} 不是数组"),
        })?;
        if list.len() > 32 {
            return Err(SliveError::Api {
                code: -1,
                message: format!("pullConfig.{codec} 超过 32 行"),
            });
        }
        rows += list.len();
        for row in list {
            if let Some(stream) = parse_stream(row, codec, room_id) {
                if seen.insert(format!("{codec}|{}|{}", stream.quality, stream.url)) {
                    streams.push(stream);
                }
            }
        }
    }
    if rows > 0 && streams.is_empty() {
        return Err(SliveError::Api {
            code: -1,
            message: format!("pullConfig 的 {rows} 行都不可用"),
        });
    }
    Ok(streams)
}

fn parse_stream(row: &Value, codec: &'static str, room_id: &str) -> Option<PullStream> {
    let url = row.get("master_url")?.as_str()?;
    let quality = row.get("quality_type")?.as_str()?;
    let label = row.get("quality_type_name")?.as_str()?;
    if url.len() > 4096 || quality.is_empty() || quality.len() > 64 || label.is_empty() || label.len() > 128 {
        return None;
    }
    if !is_media_url(url, room_id) {
        return None;
    }
    Some(PullStream {
        url: url.to_string(),
        codec,
        quality: quality.to_string(),
        label: label.to_string(),
    })
}

/// 地址必须是 `*.xhscdn.com` 上该房间的 `/live/<room>.m3u8|.flv`（无 userinfo/fragment，
/// 端口可省略或为对应默认端口）。
fn is_media_url(url: &str, room_id: &str) -> bool {
    let (scheme, rest) = match url
        .strip_prefix("https://")
        .map(|rest| ("https", rest))
        .or_else(|| url.strip_prefix("http://").map(|rest| ("http", rest)))
    {
        Some(split) => split,
        None => return false,
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.contains('@') {
        return false;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    let default_port = if scheme == "https" { "443" } else { "80" };
    if port.is_some_and(|port| port != default_port) {
        return false;
    }
    let host = host.to_ascii_lowercase();
    if !host.ends_with(".xhscdn.com") {
        return false;
    }
    let path = format!("/{path}");
    if path.contains('#') {
        return false;
    }
    path == format!("/live/{room_id}.m3u8") || path == format!("/live/{room_id}.flv")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_id_shape() {
        assert!(is_room_id("570541076495019830"));
        assert!(!is_room_id("0570541076495019830"));
        assert!(!is_room_id("12a3"));
        assert!(!is_room_id(""));
    }

    #[test]
    fn extraction_keeps_chinese_and_bare_undefined() {
        let html = r#"<script>window.__INITIAL_STATE__={"liveStream":{"pageStatus":"success","roomData":{"roomInfo":{"roomTitle":"中文;标题"}},"x":undefined}};</script>"#;
        let state = extract_live_stream(html).expect("状态");
        assert_eq!(state["pageStatus"], "success");
        assert_eq!(state["roomData"]["roomInfo"]["roomTitle"], "中文;标题");
        assert!(state["x"].is_null());
    }

    #[test]
    fn pull_config_keeps_only_public_room_addresses() {
        let config = serde_json::json!({
            "h264": [
                {"master_url": "http://live-source-play.xhscdn.com/live/123.m3u8", "quality_type": "HD", "quality_type_name": "原画"},
                {"master_url": "https://cdn.example.com/live/123.m3u8", "quality_type": "HD", "quality_type_name": "原画"}
            ],
            "h265": [
                {"master_url": "https://live-source-play.xhscdn.com/live/123.flv", "quality_type": "HD", "quality_type_name": "原画"}
            ]
        });
        let streams = parse_pull_config(Some(&config), "123").expect("地址");
        assert_eq!(streams.len(), 2);
        assert_eq!(streams[0].codec, "h264");
        assert_eq!(streams[1].codec, "h265");
    }
}
