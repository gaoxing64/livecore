//! 斗鱼房间表情与梗弹幕表。
//!
//! 斗鱼把表情拆在两条公开接口里：
//! - `dfans/emojis`：房间表情（`emoList`）与钻粉梗表情（`popularEmojis.list`）；
//! - `popularDanmuConfigs`：横向梗弹幕（如「泰裤辣」）。
//!
//! 聊天协议本身只下发文本占位符（如 `[梗亲亲]`），图片 URL / 尺寸必须由
//! 房间维度拉取。拉取失败时返回空表，弹幕主链路继续工作，只是保留原始文本。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use once_cell::sync::Lazy;
use serde_json::Value;

use crate::http::{HttpRequest, HttpTransport, DEFAULT_USER_AGENT};
use crate::models::LiveMessageEmoticon;

mod static_emoticons;

/// 单条斗鱼表情的展示信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DouyuEmoticon {
    /// 图片地址（已统一 https）。
    pub url: String,
    /// 原始宽（服务端未给时为 `None`）。
    pub width: Option<i32>,
    /// 原始高（服务端未给时为 `None`）。
    pub height: Option<i32>,
    /// 是否按大表情渲染（梗表情 / 横向梗弹幕）；App 层复用 B 站的两行上限。
    pub large: bool,
}

/// `[占位符]` -> 表情展示信息。
pub type DouyuEmoticonTable = HashMap<String, DouyuEmoticon>;

static STATIC_TABLE: Lazy<DouyuEmoticonTable> = Lazy::new(|| {
    static_emoticons::STATIC_EMOTICONS
        .iter()
        .map(|(title, url)| {
            (
                format!("[{title}]"),
                DouyuEmoticon {
                    url: (*url).to_string(),
                    width: None,
                    height: None,
                    large: false,
                },
            )
        })
        .collect()
});

/// 网页端内置静态表情表（只读单例）。
pub fn static_emoticons() -> &'static DouyuEmoticonTable {
    &STATIC_TABLE
}

/// 内置静态表情表副本，供会话按房间合并动态表情。
pub fn static_table() -> DouyuEmoticonTable {
    static_emoticons().clone()
}

const EMOJI_URL: &str = "https://www.douyu.com/japi/interact/comm/dfans/emojis";
const POPULAR_DANMU_URL: &str =
    "https://www.douyu.com/japi/interact/cdn/dFansbadge/popularDanmuConfigs";
const REFERER: &str = "https://www.douyu.com/";

/// 拉取房间表情与梗弹幕；任一接口失败只记录日志，返回已拿到的部分。
pub async fn fetch(http: Arc<dyn HttpTransport>, room_id: &str) -> DouyuEmoticonTable {
    let emoji_request = request(EMOJI_URL, room_id);
    let popular_request = request(POPULAR_DANMU_URL, room_id);
    let mut table = static_table();
    // 表情表是增强信息，不能把弹幕首连一起卡到传输层的 45s 超时。
    let replies = tokio::time::timeout(Duration::from_secs(8), async {
        tokio::join!(http.get(&emoji_request), http.get(&popular_request))
    })
    .await;
    let Ok((emoji_reply, popular_reply)) = replies else {
        log::warn!("[douyu-danmaku] 表情/梗弹幕接口超时，先以空表启动");
        return table;
    };
    match emoji_reply {
        Ok(reply) => match reply.json() {
            Ok(value) => merge_emoji_response(&value, &mut table),
            Err(e) => log::warn!("[douyu-danmaku] 房间表情 JSON 解析失败: {e}"),
        },
        Err(e) => log::warn!("[douyu-danmaku] 房间表情拉取失败: {e}"),
    }
    match popular_reply {
        Ok(reply) => match reply.json() {
            Ok(value) => merge_popular_danmu_response(&value, &mut table),
            Err(e) => log::warn!("[douyu-danmaku] 梗弹幕 JSON 解析失败: {e}"),
        },
        Err(e) => log::warn!("[douyu-danmaku] 梗弹幕拉取失败: {e}"),
    }
    if !table.is_empty() {
        log::info!(
            "[douyu-danmaku] 已加载 {} 个表情/梗弹幕表项（含内置静态表情）",
            table.len()
        );
    }
    table
}

fn request(url: &str, room_id: &str) -> HttpRequest {
    HttpRequest {
        url: url.to_string(),
        query: vec![("rid".to_string(), room_id.to_string())],
        headers: vec![
            ("user-agent".to_string(), DEFAULT_USER_AGENT.to_string()),
            ("referer".to_string(), REFERER.to_string()),
        ],
    }
}

/// 合并 `dfans/emojis` 的房间表情与钻粉梗表情。
fn merge_emoji_response(value: &Value, table: &mut DouyuEmoticonTable) {
    if api_error(value) {
        return;
    }
    let Some(data) = value.get("data") else {
        return;
    };
    for key in ["emoList", "commEmoList"] {
        if let Some(items) = data.get(key).and_then(Value::as_array) {
            for item in items {
                insert_item(table, item, false);
            }
        }
    }
    if let Some(items) = data
        .get("popularEmojis")
        .and_then(|v| v.get("list"))
        .and_then(Value::as_array)
    {
        // 这里**刻意不**建 `gag:{msgId}` 索引：`chatmsg.gag` 指向的是
        // `popularDanmuConfigs.list[].msgId`，与 `popularEmojis.msgId` 是两套
        // 独立编号（实测前者 194~197/36/37、后者 1~15，但命名空间不同）。
        // 若两者恰好撞号，共用同一个键会让梗弹幕取到错误的图。
        // 这些梗表情靠正文里的 `[梗亲亲]` 文本码命中，不需要 gag 索引。
        for item in items {
            insert_item(table, item, true);
        }
    }
}

/// 合并横向梗弹幕列表。
fn merge_popular_danmu_response(value: &Value, table: &mut DouyuEmoticonTable) {
    if api_error(value) {
        return;
    }
    let Some(items) = value
        .get("data")
        .and_then(|v| v.get("list"))
        .and_then(Value::as_array)
    else {
        return;
    };
    for item in items {
        if let Some(key) = insert_item(table, item, true) {
            if let Some(id) = item
                .get("msgId")
                .map(crate::util::value_to_string)
                .filter(|value| value.chars().all(|ch| ch.is_ascii_digit()))
            {
                if let Some(emoticon) = table.get(&key).cloned() {
                    table.insert(format!("gag:{id}"), emoticon);
                }
            }
        }
    }
}

fn insert_item(table: &mut DouyuEmoticonTable, item: &Value, large: bool) -> Option<String> {
    let large = large
        || item
            .get("type")
            .and_then(Value::as_i64)
            .map(|kind| kind >= 2)
            .unwrap_or(false);
    let mut keys = Vec::new();
    for field in ["eid", "text", "name"] {
        let Some(name) = item
            .get(field)
            .map(crate::util::value_to_string)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        for candidate in [name.clone(), name.trim_start_matches("r:").to_string()] {
            if candidate.is_empty() {
                continue;
            }
            let key = if candidate.starts_with('[') && candidate.ends_with(']') {
                candidate
            } else {
                format!("[{candidate}]")
            };
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    if keys.is_empty() {
        return None;
    }
    let url_fields: &[&str] = if large {
        &["webPic", "bigPic", "pic", "appPic"]
    } else {
        &["webPic", "pic", "bigPic", "appPic"]
    };
    let url = url_fields.iter().find_map(|key| {
        item.get(*key)
            .map(crate::util::value_to_string)
            .and_then(|s| normalize_url(&s))
    })?;
    let width = dimension(item, "webPicWidth").or_else(|| dimension(item, "width"));
    let height = dimension(item, "webPicHeight").or_else(|| dimension(item, "height"));
    let emoticon = DouyuEmoticon {
        url,
        width,
        height,
        large,
    };
    for key in &keys {
        table.insert(key.clone(), emoticon.clone());
    }
    Some(keys[0].clone())
}

fn api_error(value: &Value) -> bool {
    let code = value
        .get("error")
        .or_else(|| value.get("code"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if code == 0 {
        return false;
    }
    let message = value
        .get("msg")
        .or_else(|| value.get("message"))
        .map(crate::util::value_to_string)
        .unwrap_or_default();
    log::warn!("[douyu-danmaku] 表情接口返回错误 {code}: {message}");
    true
}

fn dimension(item: &Value, key: &str) -> Option<i32> {
    let value = item.get(key)?;
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|v| v as i64))
        .and_then(|v| i32::try_from(v).ok())
        .filter(|v| *v > 0)
}

fn normalize_url(raw: &str) -> Option<String> {
    let url = raw.trim();
    if url.is_empty() {
        return None;
    }
    if let Some(rest) = url.strip_prefix("http://") {
        return Some(format!("https://{rest}"));
    }
    if url.starts_with("https://") {
        return Some(url.to_string());
    }
    if let Some(rest) = url.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    None
}

/// 从弹幕正文提取表情占位符；未命中表时返回 `None`。
pub fn extract_emoticons(
    content: &str,
    table: &DouyuEmoticonTable,
) -> Option<Vec<LiveMessageEmoticon>> {
    let mut found = Vec::new();
    for token in bracket_tokens(content) {
        let key = format!("[{token}]");
        let Some(emoticon) = table.get(&key) else {
            continue;
        };
        found.push(LiveMessageEmoticon {
            name: Some(key),
            url: emoticon.url.clone(),
            width: emoticon.width,
            height: emoticon.height,
            large: emoticon.large,
            emoticon_unique: None,
        });
    }
    if found.is_empty() {
        None
    } else {
        Some(found)
    }
}

/// 梗弹幕通过 `chatmsg.gag` 指向 `popularDanmuConfigs.list[].msgId`。
pub fn extract_popular_danmu(table: &DouyuEmoticonTable, gag: &str) -> Option<LiveMessageEmoticon> {
    let id = gag.trim();
    if id.is_empty() || !id.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let emoticon = table.get(&format!("gag:{id}"))?;
    Some(LiveMessageEmoticon {
        // 用固定占位符替换正文，既能在聊天区定位图片，也能让弹幕库先创建 item。
        name: Some("[梗弹幕]".to_string()),
        url: emoticon.url.clone(),
        width: emoticon.width,
        height: emoticon.height,
        large: emoticon.large,
        emoticon_unique: None,
    })
}

/// 扫描 `[...]` 占位符；长度按 UTF-16 码元计，和网页端正则口径一致。
fn bracket_tokens(content: &str) -> Vec<&str> {
    let bytes = content.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'[' {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut j = start;
        while j < bytes.len() && bytes[j] != b'[' && bytes[j] != b']' {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b']' && start < j {
            if let Some(token) = content.get(start..j) {
                if !token.is_empty() && token.encode_utf16().count() <= 32 {
                    out.push(token);
                }
            }
            i = j + 1;
        } else {
            i = start;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn static_table_contains_all_builtin_packs() {
        let table = static_emoticons();
        assert_eq!(table.len(), 265, "内置静态表情数量");
        for title in ["财神", "亲亲", "比心", "冲鸭", "晚安", "GG"] {
            let key = format!("[{title}]");
            assert!(table.contains_key(&key), "缺少内置表情 {key}");
            assert!(table[&key].url.starts_with("https://"));
        }
    }

    #[test]
    fn parses_room_and_popular_emojis() {
        let value = json!({
            "error": 0,
            "data": {
                "emoList": [{
                    "eid": "317422热舞",
                    "text": "热舞",
                    "pic": "https://cdn.example.com/rewu.gif",
                    "type": 1
                }],
                "popularEmojis": {
                    "list": [{
                        "name": "梗亲亲",
                        "webPic": "https://cdn.example.com/kiss.png",
                        "webPicWidth": 200.0,
                        "webPicHeight": 200.0
                    }]
                }
            }
        });
        let mut table = DouyuEmoticonTable::new();
        merge_emoji_response(&value, &mut table);
        assert_eq!(table.len(), 3, "房间表情的 eid 和 text 都要建立别名");
        assert_eq!(
            table["[317422热舞]"].url,
            "https://cdn.example.com/rewu.gif"
        );
        assert_eq!(table["[热舞]"].url, "https://cdn.example.com/rewu.gif");
        assert!(!table["[热舞]"].large);
        let kiss = &table["[梗亲亲]"];
        assert!(kiss.large);
        assert_eq!((kiss.width, kiss.height), (Some(200), Some(200)));
    }

    #[test]
    fn live_36252_popular_emoji_matches_observed_message() {
        // 2026-10-31 在 36252 实际收到 `[梗老板大气]`；接口返回 200×200 大表情。
        let value = json!({
            "error": 0,
            "data": {
                "popularEmojis": {
                    "list": [{
                        "msgId": 12,
                        "type": 3,
                        "name": "梗老板大气",
                        "webPic": "https://sta-op.douyucdn.cn/dygev/2024/07/01/93e17f133915e64d4ad365288ae292c2.png",
                        "webPicWidth": 200,
                        "webPicHeight": 200
                    }]
                }
            }
        });
        let mut table = DouyuEmoticonTable::new();
        merge_emoji_response(&value, &mut table);
        let emoticon = extract_emoticons("[梗老板大气]", &table).expect("应命中实际弹幕");
        assert_eq!(emoticon[0].name.as_deref(), Some("[梗老板大气]"));
        assert_eq!(
            emoticon[0].url,
            "https://sta-op.douyucdn.cn/dygev/2024/07/01/93e17f133915e64d4ad365288ae292c2.png"
        );
        assert_eq!(
            (emoticon[0].width, emoticon[0].height),
            (Some(200), Some(200))
        );
        assert!(emoticon[0].large);
    }

    #[test]
    fn parses_popular_danmu() {
        let value = json!({
            "error": 0,
            "data": {
                "list": [{
                    "msgId": 197,
                    "name": "泰裤辣",
                    "webPic": "https://cdn.example.com/taikula.png",
                    "webPicWidth": 266,
                    "webPicHeight": 64
                }]
            }
        });
        let mut table = DouyuEmoticonTable::new();
        merge_popular_danmu_response(&value, &mut table);
        let item = &table["[泰裤辣]"];
        assert_eq!(item.url, "https://cdn.example.com/taikula.png");
        assert_eq!((item.width, item.height), (Some(266), Some(64)));
        assert!(item.large);
        let by_id = extract_popular_danmu(&table, "197").expect("梗弹幕 id 应能命中");
        assert_eq!(by_id.url, "https://cdn.example.com/taikula.png");
        assert_eq!(by_id.name.as_deref(), Some("[梗弹幕]"));
        assert!(extract_popular_danmu(&table, "0").is_none());
    }

    #[test]
    fn extracts_only_known_tokens_in_order() {
        let table = DouyuEmoticonTable::from([
            (
                "[梗亲亲]".to_string(),
                DouyuEmoticon {
                    url: "https://cdn.example.com/kiss.png".to_string(),
                    width: Some(200),
                    height: Some(200),
                    large: true,
                },
            ),
            (
                "[热舞]".to_string(),
                DouyuEmoticon {
                    url: "https://cdn.example.com/rewu.gif".to_string(),
                    width: None,
                    height: None,
                    large: false,
                },
            ),
        ]);
        let got =
            extract_emoticons("作茧很孤独：[梗亲亲] 再来[热舞]", &table).expect("应提取到表情");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name.as_deref(), Some("[梗亲亲]"));
        assert!(got[0].large);
        assert_eq!(got[1].name.as_deref(), Some("[热舞]"));
        assert!(extract_emoticons("[不存在]", &table).is_none());
    }

    #[test]
    fn ignores_api_errors_and_invalid_urls() {
        let mut table = DouyuEmoticonTable::new();
        merge_emoji_response(&json!({"error": 1, "msg": "denied"}), &mut table);
        assert!(table.is_empty());
        merge_popular_danmu_response(
            &json!({"error": 0, "data": {"list": [{"name": "x", "webPic": "ftp://x"}]}}),
            &mut table,
        );
        assert!(table.is_empty());
    }
}
