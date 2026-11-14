//! YouTube InnerTube 请求构造与列表解析（对照 PureLive `youtube_api.dart`）。
//!
//! - 列表/搜索/解析走 Web 客户端上下文（`WEB`），端点不带 API key；
//! - 取流走 `player`（ANDROID 上下文）；当前环境匿名请求会被要求登录/PO token，
//!   此时如实报错（见 [`super::site`]）；
//! - 列表行是 `videoRenderer`；房间 id 是**频道 id**，卡片显示的是当前直播。

use serde_json::{json, Value};

/// "Live" 目的地频道（YouTube 官方直播页）。
pub const LIVE_DESTINATION: &str = "UC4R8DWoMoI7CAwX8_LjQHig";

/// 搜索过滤器 "Live"。
pub const LIVE_FILTER: &str = "EgJAAQ%3D%3D";

/// `INNERTUBE_API_KEY` 的兜底值（页面里找不到时用；取流用）。
pub const FALLBACK_API_KEY: &str = "AIzaSyAO_FJ2SlqU8Q4STEHLGCilw_Y9_11qcW8";

/// Web 客户端上下文（列表 / 搜索 / resolve_url）。
pub fn web_context() -> Value {
    json!({
        "client": {"clientName": "WEB", "clientVersion": "2.20260925.01.00", "hl": "en", "gl": "US"}
    })
}

/// "Live" 目的地列表页。
pub fn browse_live_body() -> Value {
    json!({"context": web_context(), "browseId": LIVE_DESTINATION})
}

/// 频道页（带 `params` 时为该频道的标签页，例如 live）。
pub fn browse_body(browse_id: &str, params: Option<&str>) -> Value {
    match params {
        Some(params) => json!({"context": web_context(), "browseId": browse_id, "params": params}),
        None => json!({"context": web_context(), "browseId": browse_id}),
    }
}

/// 关键词搜索第一页（`params` 过滤为直播）。
pub fn search_body(query: &str) -> Value {
    json!({"context": web_context(), "query": query, "params": LIVE_FILTER})
}

/// 搜索后续页。
pub fn continuation_body(token: &str) -> Value {
    json!({"context": web_context(), "continuation": token})
}

/// `navigation/resolve_url`：把频道 `/live` 链接解析成 browse 端点。
pub fn resolve_body(url: &str) -> Value {
    json!({"context": web_context(), "url": url})
}

/// ANDROID 播放器上下文（PureLive `playerBody`）。
pub fn player_body(video_id: &str) -> Value {
    json!({
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
        "context": {
            "client": {
                "clientName": "ANDROID",
                "clientVersion": "21.08.266",
                "platform": "DESKTOP",
                "clientScreen": "EMBED",
                "clientFormFactor": "UNKNOWN_FACTOR",
                "browserName": "Chrome"
            },
            "user": {"lockedSafetyMode": false},
            "request": {"useSsl": true}
        }
    })
}

/// InnerTube 端点 URL（Web 侧不带 key）。
pub fn api_url(endpoint: &str) -> String {
    format!("https://www.youtube.com/youtubei/v1/{endpoint}?prettyPrint=false")
}

/// `player` 端点（需要 `key`）。
pub fn player_url(api_key: &str) -> String {
    format!(
        "https://www.youtube.com/youtubei/v1/player?key={}",
        crate::util::encode_query_component(api_key)
    )
}

/// 页面请求头（列表 / 详情页）。
pub fn page_headers(video_id: &str) -> Vec<(String, String)> {
    vec![
        (
            "user-agent".to_string(),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36".to_string(),
        ),
        (
            "accept".to_string(),
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".to_string(),
        ),
        ("accept-language".to_string(), "en-US,en;q=0.9".to_string()),
        ("cookie".to_string(), "SOCS=CAI".to_string()),
        (
            "referer".to_string(),
            if video_id.is_empty() {
                "https://www.youtube.com/".to_string()
            } else {
                format!("https://www.youtube.com/watch?v={video_id}")
            },
        ),
    ]
}

/// InnerTube API 请求头（JSON POST）。
pub fn api_headers() -> Vec<(String, String)> {
    vec![
        (
            "user-agent".to_string(),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36".to_string(),
        ),
        ("accept-language".to_string(), "en-US,en;q=0.9".to_string()),
        ("cookie".to_string(), "SOCS=CAI".to_string()),
        ("origin".to_string(), "https://www.youtube.com".to_string()),
        ("referer".to_string(), "https://www.youtube.com/".to_string()),
    ]
}

// ============================================================================
// 列表解析
// ============================================================================

/// 一条直播卡片。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoCard {
    /// 直播视频 id（11 位）。
    pub video_id: String,
    /// 频道 id（房间 id）。
    pub channel_id: String,
    /// 标题。
    pub title: String,
    /// 主播名。
    pub owner: String,
    /// 封面。
    pub cover: String,
    /// 头像。
    pub avatar: String,
    /// 观看人数（`N watching`，没有则 0）。
    pub watching: i32,
    /// 是否会员专属。
    pub members_only: bool,
}

/// 一页列表。
#[derive(Debug, Clone, Default)]
pub struct Listing {
    pub cards: Vec<VideoCard>,
    /// 下一页的 continuation token（仅搜索）。
    pub next: Option<String>,
}

/// 在任意 JSON 里递归找出所有 `videoRenderer`。
fn video_renderers(value: &Value) -> Vec<&Value> {
    fn walk<'a>(value: &'a Value, out: &mut Vec<&'a Value>) {
        match value {
            Value::Object(map) => {
                if let Some(renderer) = map.get("videoRenderer") {
                    out.push(renderer);
                }
                for child in map.values() {
                    walk(child, out);
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, &mut out);
    out
}

fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(value) => {
            if let Some(runs) = value.get("runs").and_then(Value::as_array) {
                return runs
                    .iter()
                    .filter_map(|run| run.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("");
            }
            value
                .get("simpleText")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        }
        None => String::new(),
    }
}

/// 观看数：`13,049 watching` -> 13049。
fn watching_count(value: Option<&Value>) -> i32 {
    let text = text_of(value);
    let digits: String = text
        .split_whitespace()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .collect();
    digits.parse::<i32>().unwrap_or(0)
}

/// 行内直播标记：LIVE 徽章 / LIVE 覆盖层 / `watching` 文案（且不是预告）。
fn is_live(renderer: &Value) -> bool {
    if renderer.get("upcomingEventData").is_some() {
        return false;
    }
    let badge_live = renderer
        .get("badges")
        .and_then(Value::as_array)
        .map(|badges| {
            badges.iter().any(|badge| {
                badge
                    .pointer("/metadataBadgeRenderer/style")
                    .and_then(Value::as_str)
                    == Some("BADGE_STYLE_TYPE_LIVE_NOW")
            })
        })
        .unwrap_or(false);
    if badge_live {
        return true;
    }
    let overlay_live = renderer
        .get("thumbnailOverlays")
        .and_then(Value::as_array)
        .map(|overlays| {
            overlays.iter().any(|overlay| {
                overlay
                    .pointer("/thumbnailOverlayTimeStatusRenderer/style")
                    .and_then(Value::as_str)
                    == Some("LIVE")
            })
        })
        .unwrap_or(false);
    overlay_live || text_of(renderer.get("viewCountText")).contains("watching")
}

fn thumbnails_url(value: Option<&Value>) -> String {
    value
        .and_then(|value| value.get("thumbnails"))
        .and_then(Value::as_array)
        .and_then(|list| list.last())
        .and_then(|last| last.get("url"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// 从 owner 文本里取频道 id 与主播名。
fn owner_of(renderer: &Value) -> Option<(String, String)> {
    let owner = renderer
        .get("ownerText")
        .or_else(|| renderer.get("longBylineText"))
        .or_else(|| renderer.get("shortBylineText"))?;
    let runs = owner.get("runs").and_then(Value::as_array)?;
    let first = runs.first()?;
    let name = first.get("text").and_then(Value::as_str)?.to_string();
    let channel_id = first
        .pointer("/navigationEndpoint/browseEndpoint/browseId")
        .and_then(Value::as_str)
        .map(str::to_string)?;
    Some((channel_id, name))
}

/// 一行渲染器转卡片（非直播 / 字段不全返回 None）。
pub fn card_of(renderer: &Value) -> Option<VideoCard> {
    if !is_live(renderer) {
        return None;
    }
    let video_id = renderer.get("videoId").and_then(Value::as_str)?;
    if video_id.len() != 11 {
        return None;
    }
    let (channel_id, owner) = owner_of(renderer)?;
    if !is_channel_id(&channel_id) {
        return None;
    }
    let title = text_of(renderer.get("title"));
    if title.is_empty() {
        return None;
    }
    let avatar = renderer
        .pointer("/channelThumbnailSupportedRenderers/channelThumbnailWithLinkRenderer/thumbnail")
        .map(|value| thumbnails_url(Some(value)))
        .unwrap_or_default();
    let members_only = renderer
        .get("badges")
        .and_then(Value::as_array)
        .map(|badges| {
            badges.iter().any(|badge| {
                badge
                    .pointer("/metadataBadgeRenderer/style")
                    .and_then(Value::as_str)
                    == Some("BADGE_STYLE_TYPE_MEMBERS_ONLY")
            })
        })
        .unwrap_or(false);
    Some(VideoCard {
        video_id: video_id.to_string(),
        channel_id,
        title,
        owner,
        cover: thumbnails_url(renderer.get("thumbnail")),
        avatar,
        watching: watching_count(renderer.get("viewCountText")),
        members_only,
    })
}

/// `UC` + 22 位频道 id。
pub fn is_channel_id(id: &str) -> bool {
    id.len() == 24 && id.starts_with("UC")
}

/// 11 位视频 id。
pub fn is_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

/// 解析一页列表；`search` 时收集最后一页的 continuation token。
pub fn listing(decoded: &Value, search: bool) -> Listing {
    let mut cards = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for renderer in video_renderers(decoded) {
        let Some(card) = card_of(renderer) else {
            continue;
        };
        if seen.insert((card.channel_id.clone(), card.video_id.clone())) {
            cards.push(card);
        }
    }
    let next = if search && !cards.is_empty() {
        find_continuation(decoded)
    } else {
        None
    };
    Listing { cards, next }
}

fn find_continuation(value: &Value) -> Option<String> {
    fn walk(value: &Value, tokens: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                if let Some(token) = map
                    .get("continuationItemRenderer")
                    .and_then(|item| item.pointer("/continuationEndpoint/continuationCommand/token"))
                    .and_then(Value::as_str)
                {
                    tokens.push(token.to_string());
                }
                for child in map.values() {
                    walk(child, tokens);
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, tokens);
                }
            }
            _ => {}
        }
    }
    let mut tokens = Vec::new();
    walk(value, &mut tokens);
    tokens.pop()
}

/// 从 `resolve_url` 应答里取 browse 端点（频道 `/live` 页）。
pub fn resolve_browse_endpoint(decoded: &Value) -> Option<(String, Option<String>)> {
    let endpoint = decoded.get("endpoint")?;
    let browse_id = endpoint
        .pointer("/browseEndpoint/browseId")
        .and_then(Value::as_str)?
        .to_string();
    let params = endpoint
        .pointer("/browseEndpoint/params")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some((browse_id, params))
}

/// 在播频道的 `/live` 链接会被解析成 `watchEndpoint.videoId`。
pub fn resolve_watch_video_id(decoded: &Value) -> Option<String> {
    decoded
        .pointer("/endpoint/watchEndpoint/videoId")
        .and_then(Value::as_str)
        .filter(|id| is_video_id(id))
        .map(str::to_string)
}

/// 页面 HTML 里的 `INNERTUBE_API_KEY`。
pub fn api_key_of(html: &str) -> Option<String> {
    let marker = "\"INNERTUBE_API_KEY\":\"";
    let start = html.find(marker)? + marker.len();
    let end = html[start..].find('"')? + start;
    Some(html[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_maps_live_rows_and_reads_watching() {
        let value = json!({
            "contents": [
                {"videoRenderer": {
                    "videoId": "DxgDeW-gIcU",
                    "title": {"runs": [{"text": "LIVE TITLE"}]},
                    "ownerText": {"runs": [{"text": "HotSpanish",
                        "navigationEndpoint": {"browseEndpoint": {"browseId": "UC1234567890123456789012"}}}]},
                    "thumbnail": {"thumbnails": [{"url": "https://cdn/s.jpg"}, {"url": "https://cdn/l.jpg"}]},
                    "channelThumbnailSupportedRenderers": {"channelThumbnailWithLinkRenderer": {
                        "thumbnail": {"thumbnails": [{"url": "https://cdn/a.jpg"}]}}},
                    "viewCountText": {"runs": [{"text": "13,049 watching"}]},
                    "thumbnailOverlays": [{"thumbnailOverlayTimeStatusRenderer": {"style": "LIVE"}}]
                }},
                {"videoRenderer": {
                    "videoId": "AAAAAAAAAAA",
                    "title": {"runs": [{"text": "UPCOMING"}]},
                    "ownerText": {"runs": [{"text": "x",
                        "navigationEndpoint": {"browseEndpoint": {"browseId": "UC1234567890123456789012"}}}]},
                    "upcomingEventData": {"startTime": "1"}
                }}
            ]
        });
        let page = listing(&value, true);
        assert_eq!(page.cards.len(), 1, "预告行不算直播");
        let card = &page.cards[0];
        assert_eq!(card.video_id, "DxgDeW-gIcU");
        assert_eq!(card.channel_id, "UC1234567890123456789012");
        assert_eq!(card.owner, "HotSpanish");
        assert_eq!(card.watching, 13049);
        assert_eq!(card.cover, "https://cdn/l.jpg");
        assert_eq!(card.avatar, "https://cdn/a.jpg");
    }

    #[test]
    fn channel_and_video_id_shapes_are_checked() {
        assert!(is_channel_id("UC4R8DWoMoI7CAwX8_LjQHig"));
        assert!(!is_channel_id("UC4R8DWoMoI7CAwX8_LjQHi"));
        assert!(is_video_id("DxgDeW-gIcU"));
        assert!(!is_video_id("short"));
    }

    #[test]
    fn api_key_is_read_from_page() {
        let html = r#"<script>{"INNERTUBE_API_KEY":"AIzaTEST"}</script>"#;
        assert_eq!(api_key_of(html).as_deref(), Some("AIzaTEST"));
        assert_eq!(api_key_of("<html></html>"), None);
    }
}
