//! 快手房间详情解析（从 HTML 中提取 __INITIAL_STATE__）。

use serde_json::Value;
use crate::error::SliveError;
use crate::models::*;

/// 从 HTML 中提取 JSON 对象字面量。
///
/// 结构字符（引号/括号）都是 ASCII，UTF-8 多字节字符不会与之冲突，
/// 因此在字节上扫描边界、最后整段切片，避免把 UTF-8 字节逐个当 `char`
/// 转换而损坏中文。
fn extract_object_literal(html: &str, start: usize) -> Option<String> {
    let bytes = html.as_bytes();
    let mut index = start;

    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    if index >= bytes.len() || bytes[index] != b'{' {
        return None;
    }
    let begin = index;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;

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
                    // 结束位置是 ASCII `}`，可直接切片。
                    return Some(replace_bare_undefined(&html[begin..=index]));
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// 只把字符串外的独立 `undefined` 换成 `null`（字符串内的词保持原样）。
fn replace_bare_undefined(raw: &str) -> String {
    const WORD: &str = "undefined";
    if !raw.contains(WORD) {
        return raw.to_string();
    }
    let is_identifier = |ch: char| ch.is_alphanumeric() || ch == '_' || ch == '$';
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut copy_from = 0usize;
    let mut index = 0usize;
    let mut in_string = false;
    let mut escaped = false;

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
        if byte == b'"' {
            in_string = true;
            index += 1;
            continue;
        }
        if raw[index..].starts_with(WORD) {
            let before_ok = raw[..index]
                .chars()
                .next_back()
                .map(|ch| !is_identifier(ch))
                .unwrap_or(true);
            let after = index + WORD.len();
            let after_ok = raw[after..]
                .chars()
                .next()
                .map(|ch| !is_identifier(ch))
                .unwrap_or(true);
            if before_ok && after_ok {
                out.push_str(&raw[copy_from..index]);
                out.push_str("null");
                index = after;
                copy_from = after;
                continue;
            }
        }
        index += 1;
    }
    out.push_str(&raw[copy_from..]);
    out
}

/// 从 HTML 中提取 __INITIAL_STATE__ JSON 对象。
pub fn extract_initial_state(html: &str) -> Result<Value, SliveError> {
    let marker = "window.__INITIAL_STATE__";
    let marker_idx = html.find(marker)
        .ok_or_else(|| SliveError::Api { 
            code: -1, 
            message: "未找到 __INITIAL_STATE__（可能是验证页面）".to_string() 
        })?;
    
    let after_marker = marker_idx + marker.len();
    let eq_idx = html[after_marker..].find('=')
        .ok_or_else(|| SliveError::Api { 
            code: -1, 
            message: "__INITIAL_STATE__ 格式错误".to_string() 
        })? + after_marker + 1;
    
    let json_str = extract_object_literal(html, eq_idx)
        .ok_or_else(|| SliveError::Api { 
            code: -1, 
            message: "__INITIAL_STATE__ 不是完整的 JSON 对象".to_string() 
        })?;
    
    serde_json::from_str(&json_str)
        .map_err(|e| SliveError::decode(format!("解析 __INITIAL_STATE__ 失败：{}", e)))
}

/// 从房间页 HTML 中解析 __INITIAL_STATE__。
pub fn parse_room_html(html: &str, room_id: &str) -> Result<LiveRoomDetail, SliveError> {
    let state = extract_initial_state(html)?;
    
    // 提取房间信息：liveroom.playList[0]
    let room_data = state.pointer("/liveroom/playList/0")
        .ok_or_else(|| SliveError::Api { 
            code: -1, 
            message: "未找到房间信息（liveroom.playList[0]）".to_string() 
        })?;
    
    // 检查错误类型（`errorType` 可能是数字或 `{type: ...}` 对象）。
    if let Some(error) = room_data.get("errorType").filter(|value| !value.is_null()) {
        let error_type = error
            .get("type")
            .and_then(Value::as_i64)
            .or_else(|| error.as_i64())
            .unwrap_or(-1);
        if error_type == 22 {
            return Err(SliveError::Api {
                code: 22,
                message: "房间不存在".to_string(),
            });
        }
        return Err(SliveError::Api {
            code: error_type as i32,
            message: format!("快手房间页错误（errorType {error_type}）"),
        });
    }
    
    let author = room_data.get("author")
        .ok_or_else(|| SliveError::Api { 
            code: -1, 
            message: "未找到主播信息".to_string() 
        })?;
    
    let live_stream = room_data.get("liveStream");
    let is_live = room_data.get("isLiving")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    
    let title = live_stream
        .and_then(|s| s.get("caption"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    
    let cover = live_stream
        .and_then(|s| s.get("poster"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    
    Ok(LiveRoomDetail {
        room_id: room_id.to_string(),
        title,
        cover,
        user_name: author.get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        user_avatar: author.get("avatar")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        online: 0,
        introduction: author.get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        notice: None,
        status: is_live,
        data: None,
        danmaku_data: Some(room_id.to_string()),
        is_record: false,
        url: format!("https://live.kuaishou.com/u/{}", room_id),
    })
}

/// 从 __INITIAL_STATE__ 中提取 playUrls。
pub fn extract_play_urls(state: &Value) -> Option<Value> {
    state.pointer("/liveroom/playList/0/liveStream/playUrls")
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_object_with_chinese_and_semicolons_in_strings() {
        let html = r#"<script>window.__INITIAL_STATE__ = {"liveroom":{"playList":[{"author":{"name":"牢呃（区中区）"},"caption":"a;b，中文"}]}};</script>"#;
        let state = extract_initial_state(html).expect("提取状态");
        assert_eq!(
            state.pointer("/liveroom/playList/0/author/name").and_then(Value::as_str),
            Some("牢呃（区中区）")
        );
        assert_eq!(
            state.pointer("/liveroom/playList/0/caption").and_then(Value::as_str),
            Some("a;b，中文")
        );
    }

    #[test]
    fn bare_undefined_becomes_null_but_strings_are_kept() {
        let html = r#"<script>window.__INITIAL_STATE__ = {"a":undefined,"b":"undefined","c":{"d":[1,2]}};</script>"#;
        let state = extract_initial_state(html).expect("提取状态");
        assert!(state.get("a").map(Value::is_null).unwrap_or(false));
        assert_eq!(state.get("b").and_then(Value::as_str), Some("undefined"));
        assert_eq!(state.pointer("/c/d/1").and_then(Value::as_i64), Some(2));
    }

    #[test]
    fn page_without_state_is_reported() {
        let err = extract_initial_state("<html>验证页</html>").expect_err("无状态必须报错");
        assert!(matches!(err, SliveError::Api { code: -1, .. }));
    }
}
