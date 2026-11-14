//! 快手 REST API 端点与响应构造（对照 PureLive `kuaishou_api.dart`）。
//!
//! 端点均以 `/live_api/` 前缀：
//! - `home/list`：`data.list[].gameLiveInfo[].liveInfo[]` 三层分组，卡片即直播；
//! - `search/author`：作者列表（`data.list[]`，结果在 `data.result`）；
//! - 房间详情走房间页 `window.__INITIAL_STATE__`（见 [`super::detail`]）。
//!
//! 卡片的 `playUrls` 是描述符数组（`adaptationSet.representation[]`），
//! 画质解析见 [`super::play`]。

use serde::Deserialize;
use serde_json::Value;

use crate::error::SliveError;
use crate::models::{LiveCategory, LiveRoomItem};

/// 快手 API 根 URL。
const KUAISHOU_API: &str = "https://live.kuaishou.com";

/// `result` 字段的成功值。
pub const RESULT_OK: i64 = 2;

/// 8 个固定一级分类（PureLive `topCategories`）。
pub const TOP_CATEGORIES: &[(&str, &str)] = &[
    ("1", "热门"),
    ("2", "网游"),
    ("3", "单机"),
    ("4", "手游"),
    ("5", "棋牌"),
    ("6", "娱乐"),
    ("7", "综合"),
    ("8", "文化"),
];

/// 固定分类（子分类区域暂未拉取，children 为空）。
pub fn fixed_categories() -> Vec<LiveCategory> {
    TOP_CATEGORIES
        .iter()
        .map(|(id, name)| LiveCategory {
            id: (*id).to_string(),
            name: (*name).to_string(),
            children: Vec::new(),
        })
        .collect()
}

// ============================================================================
// 端点
// ============================================================================

/// 首页推荐列表。
pub fn home_list() -> String {
    format!("{KUAISHOU_API}/live_api/home/list")
}

/// 分区数据（分类下的区域列表）。
pub fn category_data(page: i32, size: i32, category_id: &str) -> String {
    format!("{KUAISHOU_API}/live_api/category/data?type={category_id}&page={page}&size={size}")
}

/// 游戏板列表（短 id 区域）。
pub fn gameboard_list(game_id: &str, page: i32, cursor: Option<&str>) -> String {
    let mut url = format!(
        "{KUAISHOU_API}/live_api/gameboard/list?gameId={game_id}&page={page}&pageSize=20"
    );
    if let Some(cursor) = cursor {
        url.push_str(&format!("&cursor={cursor}"));
    }
    url
}

/// 非游戏板列表（长 id 区域，后续页带 cursor）。
pub fn non_gameboard_list(area_id: &str, page: i32, cursor: Option<&str>) -> String {
    let mut url = format!(
        "{KUAISHOU_API}/live_api/non-gameboard/list?type={area_id}&page={page}&pageSize=20"
    );
    if let Some(cursor) = cursor {
        url.push_str(&format!("&cursor={cursor}"));
    }
    url
}

/// 搜索作者（房间搜索 / 主播搜索同源）。
pub fn search_author(keyword: &str, page: i32, lssid: &str) -> String {
    format!("{KUAISHOU_API}/live_api/search/author?keyword={keyword}&page={page}&lssid={lssid}")
}

/// 房间页（详情与取流都解析页内 `__INITIAL_STATE__`）。
pub fn room_page(room_id: &str) -> String {
    format!("{KUAISHOU_API}/u/{room_id}")
}

/// 站点首页（匿名会话的 cookie 来源页）。
pub fn home_page() -> String {
    format!("{KUAISHOU_API}/")
}

/// 分类下的区域列表（50 条/页，服务端忽略 size）。
pub fn area_list(category_id: &str, page: i32) -> String {
    format!(
        "{KUAISHOU_API}/live_api/category/data?type={category_id}&page={page}&size=30"
    )
}

/// 区域房间页：短 id（< 7 位）走 gameboard，其余 non-gameboard。
pub fn area_rooms(area_id: &str, page: i32, cursor: &str) -> String {
    let board = if area_id.len() < 7 {
        "gameboard"
    } else {
        "non-gameboard"
    };
    let mut url = format!(
        "{KUAISHOU_API}/live_api/{board}/list?filterType=0&pageSize=20&gameId={area_id}&page={page}"
    );
    if !cursor.is_empty() {
        url.push_str(&format!("&cursor={cursor}"));
    }
    url
}

// ============================================================================
// 列表响应
// ============================================================================

/// 推荐卡片（`home/list` 的 `liveInfo[]`）。
#[derive(Debug, Deserialize, Clone, Default)]
pub struct StreamCard {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub caption: String,
    #[serde(default)]
    pub author: StreamAuthor,
    #[serde(default)]
    pub poster: Option<String>,
    #[serde(default, alias = "watchingCount")]
    pub watching_count: Value,
}

/// 主播信息。
#[derive(Debug, Deserialize, Clone, Default)]
pub struct StreamAuthor {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// 搜索结果里的作者（`search/author` 的 `data.list[]`）。
#[derive(Debug, Deserialize, Clone, Default)]
pub struct SearchAuthor {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub living: bool,
}

fn api_error(json: &Value, what: &str) -> Option<SliveError> {
    let result = json
        .get("result")
        .and_then(Value::as_i64)
        .or_else(|| json.pointer("/data/result").and_then(Value::as_i64));
    match result {
        Some(code) if code != RESULT_OK && code != 1 => Some(SliveError::Api {
            code: code as i32,
            message: format!("{what}失败：result={code}"),
        }),
        _ => None,
    }
}

/// 推荐列表：三层分组扁平化成卡片（保持服务端顺序）。
pub fn parse_home_list(json: &Value) -> Result<Vec<StreamCard>, SliveError> {
    if let Some(error) = api_error(json, "快手首页") {
        return Err(error);
    }
    let groups = json
        .pointer("/data/list")
        .and_then(Value::as_array)
        .ok_or_else(|| SliveError::Api {
            code: -1,
            message: "快手首页缺少 data.list".to_string(),
        })?;

    let mut cards = Vec::new();
    let mut flat = Vec::new();
    let mut grouped = false;
    for group in groups {
        if let Some(labels) = group.get("gameLiveInfo").and_then(Value::as_array) {
            grouped = true;
            for label in labels {
                for card in label
                    .get("liveInfo")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    flat.push(card);
                }
            }
        } else {
            flat.push(group);
        }
    }
    let _ = grouped;
    for card in flat {
        if let Ok(parsed) = serde_json::from_value::<StreamCard>(card.clone()) {
            cards.push(parsed);
        }
    }
    Ok(cards)
}

/// 搜索作者：`data.list[]`（`data.result=1` 且无 list 表示没有结果）。
pub fn parse_search_authors(json: &Value) -> Result<Vec<SearchAuthor>, SliveError> {
    if let Some(error) = api_error(json, "快手搜索") {
        return Err(error);
    }
    let Some(list) = json.pointer("/data/list").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    Ok(list
        .iter()
        .filter_map(|item| serde_json::from_value::<SearchAuthor>(item.clone()).ok())
        .filter(|author| !author.id.is_empty())
        .collect())
}

/// 分类区域：`data.list[]` 的 `{id, name, poster|iconUrl}` 与 `hasMore`。
pub fn parse_areas(
    json: &Value,
    category_id: &str,
    category_name: &str,
) -> Result<(Vec<crate::models::LiveSubCategory>, bool), SliveError> {
    if let Some(error) = api_error(json, "快手分类区域") {
        return Err(error);
    }
    let list = json
        .pointer("/data/list")
        .and_then(Value::as_array)
        .ok_or_else(|| SliveError::Api {
            code: -1,
            message: "快手分类区域缺少 data.list".to_string(),
        })?;
    let mut seen = std::collections::HashSet::new();
    let areas = list
        .iter()
        .filter_map(|item| {
            let id = item.get("id").map(crate::util::value_to_string)?;
            if id.is_empty() || !seen.insert(id.clone()) {
                return None;
            }
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let picture = item
                .get("poster")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .or_else(|| item.get("iconUrl").and_then(Value::as_str))
                .map(str::to_string);
            Some(crate::models::LiveSubCategory {
                name,
                pic: picture,
                id,
                parent_id: category_id.to_string(),
            })
        })
        .collect::<Vec<_>>();
    let _ = category_name;
    let has_more = json
        .pointer("/data/hasMore")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok((areas, has_more))
}

/// 区域房间：卡片列表、`hasMore` 与下一页 `cursor`。
pub fn parse_area_rooms(
    json: &Value,
) -> Result<(Vec<StreamCard>, bool, Option<String>), SliveError> {
    if let Some(error) = api_error(json, "快手区域房间") {
        return Err(error);
    }
    let list = json
        .pointer("/data/list")
        .and_then(Value::as_array)
        .ok_or_else(|| SliveError::Api {
            code: -1,
            message: "快手区域房间缺少 data.list".to_string(),
        })?;
    let cards = list
        .iter()
        .filter_map(|item| serde_json::from_value::<StreamCard>(item.clone()).ok())
        .collect();
    let has_more = json
        .pointer("/data/hasMore")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let cursor = json
        .pointer("/data/cursor")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    Ok((cards, has_more, cursor))
}

/// 观看数：`"1 万+"`、`"1.2万"`、`5000` 都归一为整数。
pub fn parse_watching_count(value: &Value) -> i32 {
    match value {
        Value::Number(number) => number.as_i64().unwrap_or(0) as i32,
        Value::String(text) => {
            let cleaned = text.trim().replace(['+', ','], "");
            let multiplier = if cleaned.contains('万') { 10_000.0 } else { 1.0 };
            let number: String = cleaned
                .chars()
                .filter(|ch| ch.is_ascii_digit() || *ch == '.')
                .collect();
            (number.parse::<f64>().unwrap_or(0.0) * multiplier) as i32
        }
        _ => 0,
    }
}

/// 封面：无扩展名的截图地址补 `.jpg`，带 query 的原样返回。
fn normalize_cover(value: Option<&str>) -> String {
    let Some(url) = value else {
        return String::new();
    };
    let url = url.trim();
    if url.is_empty() || url.contains('?') || url.contains('#') {
        return url.to_string();
    }
    match url.rsplit('/').next().and_then(|name| name.rfind('.')) {
        Some(dot) => {
            let extension = url.rsplit('/').next().unwrap_or("")[dot + 1..].to_ascii_lowercase();
            if matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif") {
                url.to_string()
            } else {
                format!("{url}.jpg")
            }
        }
        None => format!("{url}.jpg"),
    }
}

/// 推荐卡片转房间项：`room_id` 是**主播 id**（房间页 `/u/{id}`）。
pub fn card_to_room_item(card: &StreamCard) -> LiveRoomItem {
    let title = if card.caption.trim().is_empty() {
        card.author
            .description
            .as_deref()
            .unwrap_or("")
            .replace('\n', " ")
    } else {
        card.caption.clone()
    };
    LiveRoomItem {
        room_id: card.author.id.clone(),
        title,
        cover: normalize_cover(card.poster.as_deref()),
        user_name: card.author.name.clone(),
        online: parse_watching_count(&card.watching_count),
    }
}

/// 搜索作者转房间项（标题即主播名，封面即头像；无观众数）。
pub fn author_to_room_item(author: &SearchAuthor) -> LiveRoomItem {
    LiveRoomItem {
        room_id: author.id.clone(),
        title: author.name.clone(),
        cover: author.avatar.clone().unwrap_or_default(),
        user_name: author.name.clone(),
        online: 0,
    }
}
