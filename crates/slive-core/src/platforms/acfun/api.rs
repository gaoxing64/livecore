//! ACFUN API 端点与响应解析（对照 PureLive `acfun_api.dart`）。
//!
//! - `api/channel/list`：`channelListData.liveList[]` 房间 + `pcursor` 翻页，
//!   区域过滤在 `channelFilters.liveChannelDisplayFilters[].displayFilters[]`；
//! - `api/live/info?authorId=`：房间详情；
//! - `visitor/login` + `startPlay`：取流（见 [`super::site`]）；
//! - `www.acfun.cn/search?type=user&ajaxpipe=1`：主播搜索（BigPipe 页面）。

use serde_json::Value;

use crate::error::SliveError;
use crate::models::{LiveAnchorItem, LiveRoomItem, LiveSubCategory};

/// 站点根。
const ACFUN_LIVE: &str = "https://live.acfun.cn";
/// `全部` 过滤器的 id（不作为区域列出）。
const ALL_FILTER_ID: i64 = 0;
/// `live/info` 需要类 curl 的 UA（ACFUN 对浏览器 UA 返回精简结构）。
pub const CURL_USER_AGENT: &str = "curl/8.0.0";

/// 目录列表（`count` 为服务端页大小，`pcursor` 空串表示第一页）。
pub fn channel_list(count: i32, pcursor: &str, filters: Option<&str>) -> String {
    let mut url = format!(
        "{ACFUN_LIVE}/api/channel/list?count={count}&pcursor={}",
        crate::util::encode_query_component(pcursor)
    );
    if let Some(filters) = filters {
        url.push_str(&format!(
            "&filters={}",
            crate::util::encode_query_component(filters)
        ));
    }
    url
}

/// 房间详情。
pub fn live_info(author_id: &str) -> String {
    format!(
        "{ACFUN_LIVE}/api/live/info?authorId={}",
        crate::util::encode_query_component(author_id)
    )
}

/// 主播搜索（BigPipe 页面，`pCursor` 是页码）。
pub fn search_user(keyword: &str, page: i32) -> String {
    format!(
        "https://www.acfun.cn/search?keyword={}&type=user&quickViewId=up-list&reqID=1&ajaxpipe=1&pCursor={page}",
        crate::util::encode_query_component(keyword)
    )
}

/// 区域过滤参数：`filterType-filterId`（PureLive `_filters`）。
///
/// 两段都必须存在：缺 `parent_id` 时拼出 `-8` 这类畸形过滤器，
/// 服务端会安静地返回空列表，不如退回「不过滤」。
pub fn area_filter(category: &LiveSubCategory) -> Option<String> {
    let id = category.id.trim();
    let parent = category.parent_id.trim();
    if id.is_empty() || parent.is_empty() {
        return None;
    }
    Some(format!("{parent}-{id}"))
}

// ============================================================================
// 列表与区域
// ============================================================================

/// `channel/list` 的列表数据（`channelListData`，旧结构直接平铺）。
fn list_data(json: &Value) -> Result<&Value, SliveError> {
    let result = json.get("result").and_then(Value::as_i64).unwrap_or(0);
    if result != 0 {
        return Err(SliveError::Api {
            code: result as i32,
            message: format!("channel/list: result {result}"),
        });
    }
    json.get("channelListData")
        .filter(|value| value.is_object())
        .or_else(|| json.get("data").filter(|value| value.is_object()))
        .ok_or_else(|| SliveError::Api {
            code: -1,
            message: "channel/list: 缺少 channelListData".to_string(),
        })
}

/// 区域：`channelFilters.liveChannelDisplayFilters[].displayFilters[]`。
///
/// `channelFilters` 既可能在根上，也可能包在 `data` 里（与 [`list_data`] 的
/// 两种形态保持一致）。
pub fn parse_areas(json: &Value) -> Result<Vec<LiveSubCategory>, SliveError> {
    let _ = list_data(json)?;
    let filters = json
        .get("channelFilters")
        .or_else(|| json.pointer("/data/channelFilters"))
        .filter(|value| value.is_object())
        .ok_or_else(|| SliveError::Api {
            code: -1,
            message: "channel/list: 缺少 channelFilters".to_string(),
        })?;
    let groups = filters
        .get("liveChannelDisplayFilters")
        .and_then(Value::as_array)
        .ok_or_else(|| SliveError::Api {
            code: -1,
            message: "channel/list: 缺少 channelFilters".to_string(),
        })?;

    let mut seen = std::collections::HashSet::new();
    let mut areas = Vec::new();
    for group in groups {
        for filter in group
            .get("displayFilters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let filter_type = filter.get("filterType").and_then(Value::as_i64);
            let filter_id = filter.get("filterId").and_then(Value::as_i64);
            let name = filter
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let (Some(filter_type), Some(filter_id)) = (filter_type, filter_id) else {
                continue;
            };
            if filter_id == ALL_FILTER_ID || name.is_empty() {
                continue;
            }
            if !seen.insert((filter_type, filter_id)) {
                continue;
            }
            areas.push(LiveSubCategory {
                name,
                pic: filter
                    .get("cover")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string),
                id: filter_id.to_string(),
                parent_id: filter_type.to_string(),
            });
        }
    }
    if areas.is_empty() {
        return Err(SliveError::Api {
            code: -1,
            message: "channel/list: 没有区域过滤器".to_string(),
        });
    }
    Ok(areas)
}

/// 目录页：房间列表与下一页 `pcursor`（`no_more` / 空串表示结束）。
pub fn parse_directory(json: &Value) -> Result<(Vec<LiveRoomItem>, Option<String>), SliveError> {
    let data = list_data(json)?;
    let list = data
        .get("liveList")
        .and_then(Value::as_array)
        .ok_or_else(|| SliveError::Api {
            code: -1,
            message: "channel/list: 缺少 liveList".to_string(),
        })?;

    let mut items = Vec::new();
    for item in list {
        let author_id = item
            .get("authorId")
            .map(crate::util::value_to_string)
            .unwrap_or_default();
        if author_id.is_empty() || author_id == "0" {
            continue;
        }
        let user = item.get("user").filter(|value| value.is_object());
        let user_name = user
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if user_name.is_empty() {
            continue;
        }
        let cover = item
            .get("coverUrls")
            .and_then(Value::as_array)
            .and_then(|covers| covers.first())
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        items.push(LiveRoomItem {
            room_id: author_id,
            title: item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            cover,
            user_name,
            online: item
                .get("onlineCount")
                .and_then(Value::as_i64)
                .unwrap_or(0) as i32,
        });
    }

    let cursor = data
        .get("pcursor")
        .map(crate::util::value_to_string)
        .unwrap_or_default();
    let next = if cursor.is_empty() || cursor == "no_more" {
        None
    } else {
        Some(cursor)
    };
    Ok((items, next))
}

// ============================================================================
// 搜索（`ajaxpipe=1` 返回 `{"html": "<卡片片段>"}`）
// ============================================================================

/// 从 `ajaxpipe` 响应里解析主播卡片。
///
/// 卡片形如 `<div class="search-up" data-up-exposure-log='{"up_id":...,"is_on_live":""}'>`
/// 内含 `<a href="/u/<id>">名字</a>` 与 `<img class="up__avatar" src=...>`。
pub fn parse_search(body: &str) -> Vec<LiveAnchorItem> {
    // 响应可能是纯 JSON，也可能带 `/*<!-- fetch-stream -->*/` 尾巴。
    let trimmed = body.split("/*<!-- fetch-stream -->*/").next().unwrap_or(body);
    let html = serde_json::from_str::<Value>(trimmed.trim())
        .ok()
        .and_then(|value| {
            value
                .get("html")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.to_string());

    let mut anchors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for card in split_cards(&html) {
        let Some(log) = card_log(card) else {
            continue;
        };
        let id = log
            .get("up_id")
            .map(crate::util::value_to_string)
            .unwrap_or_default();
        if id.is_empty() || id == "0" || !seen.insert(id.clone()) {
            continue;
        }
        let Some(name) = anchor_name(card, &id) else {
            continue;
        };
        anchors.push(LiveAnchorItem {
            room_id: id,
            avatar: attribute_value(card, "up__avatar", "src").unwrap_or_default(),
            user_name: name,
            live_status: log
                .get("is_on_live")
                .map(crate::util::value_to_string)
                .map(|flag| !flag.is_empty() && flag != "null" && flag != "false")
                .unwrap_or(false),
        });
    }
    anchors
}

/// 按 `class="search-up"` 切出每张卡片片段。
fn split_cards(html: &str) -> Vec<&str> {
    const MARKER: &str = "class=\"search-up\"";
    let mut cards = Vec::new();
    let mut rest = html;
    while let Some(position) = rest.find(MARKER) {
        let card = &rest[position..];
        // 卡片以 `</div>` 收尾：取到下一个卡片标记前，尽量覆盖整张卡。
        let end = card[MARKER.len()..]
            .find(MARKER)
            .map(|offset| MARKER.len() + offset)
            .unwrap_or(card.len());
        cards.push(&card[..end]);
        rest = &card[end..];
    }
    cards
}

/// 卡片上 `data-up-exposure-log` 里的 JSON。
fn card_log(card: &str) -> Option<Value> {
    let marker = "data-up-exposure-log='";
    let start = card.find(marker)? + marker.len();
    let rest = &card[start..];
    let end = rest.find('\'')?;
    serde_json::from_str(rest[..end].trim()).ok()
}

/// 卡片里指向 `/u/<id>` 的名字文本。
fn anchor_name(card: &str, id: &str) -> Option<String> {
    let needle = format!("href=\"/u/{id}\"");
    let mut rest = card;
    while let Some(position) = rest.find(&needle) {
        let after = &rest[position + needle.len()..];
        if let Some(open) = after.find('>') {
            let text = &after[open + 1..];
            if let Some(close) = text.find("</a>") {
                let name = text[..close].trim();
                if !name.is_empty() && !name.contains('<') {
                    return Some(name.to_string());
                }
            }
        }
        rest = after;
    }
    None
}

/// 卡片内带某 class 的 `<img src=...>`。
fn attribute_value(card: &str, class: &str, attribute: &str) -> Option<String> {
    let marker = format!("class=\"{class}");
    let start = card.find(&marker)?;
    let tag_start = card[..start].rfind('<')?;
    let tag_end = card[start..].find('>')? + start;
    let tag = &card[tag_start..=tag_end];
    let needle = format!("{attribute}=\"");
    let value_start = tag.find(&needle)? + needle.len();
    let value_end = tag[value_start..].find('"')? + value_start;
    Some(tag[value_start..value_end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn areas_skip_the_all_filter_and_duplicates() {
        let value = json!({
            "channelListData": {"liveList": [], "pcursor": "no_more"},
            "channelFilters": {
                "liveChannelDisplayFilters": [
                    {"displayFilters": [
                        {"filterType": 1, "filterId": 0, "name": "全部"},
                        {"filterType": 1, "filterId": 8, "name": "游戏", "cover": "https://cdn/g.png"},
                        {"filterType": 1, "filterId": 8, "name": "游戏"}
                    ]}
                ]
            }
        });
        let areas = parse_areas(&value).expect("解析区域");
        assert_eq!(areas.len(), 1);
        assert_eq!(areas[0].id, "8");
        assert_eq!(areas[0].parent_id, "1");
        assert_eq!(areas[0].name, "游戏");
    }

    #[test]
    fn directory_maps_cards_and_cursor() {
        let value = json!({
            "channelListData": {
                "pcursor": "abc",
                "liveList": [
                    {"authorId": 173620, "title": "标题", "onlineCount": 80,
                     "coverUrls": ["https://cdn/c.jpg"], "user": {"name": "主播"}},
                    {"authorId": 0, "title": "无作者", "user": {"name": "x"}}
                ]
            }
        });
        let (items, next) = parse_directory(&value).expect("解析目录");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].room_id, "173620");
        assert_eq!(items[0].online, 80);
        assert_eq!(next.as_deref(), Some("abc"));

        let ended = json!({"channelListData": {"pcursor": "no_more", "liveList": []}});
        assert_eq!(parse_directory(&ended).expect("结束页").1, None);
    }

    #[test]
    fn search_reads_ajaxpipe_html_cards() {
        let body = r#"{"html":"<span class=\"total-num\" data-total=\"1\">共1条结果</span><div class=\"search-up\" data-up-exposure-log='{\"up_id\":13456522,\"is_on_live\":\"\"}'><a href=\"/u/13456522\"><img class=\"up__avatar live-avatar\" src=\"https://cdn/a.jpg\"/></a><div class=\"up__main__name\"><a href=\"/u/13456522\">游戏X博士</a></div></div>"}"#;
        let anchors = parse_search(body);
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].room_id, "13456522");
        assert_eq!(anchors[0].user_name, "游戏X博士");
        assert_eq!(anchors[0].avatar, "https://cdn/a.jpg");
        assert!(!anchors[0].live_status, "is_on_live 为空即未开播");
        assert!(parse_search("<html>无结果</html>").is_empty());
    }

    #[test]
    fn search_marks_live_when_is_on_live_holds_an_id() {
        let body = r#"{"html":"<div class=\"search-up\" data-up-exposure-log='{\"up_id\":173620,\"is_on_live\":\"29RchpoKMpA\"}'><a href=\"/u/173620\">主播</a></div>"}"#;
        let anchors = parse_search(body);
        assert_eq!(anchors.len(), 1);
        assert!(anchors[0].live_status);
    }
}
