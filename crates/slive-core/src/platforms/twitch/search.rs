//! F-04 搜索房间 / F-05 搜索主播（对照 PureLive `twitch_site.dart:427-449`）。
//!
//! 同一个 `SearchResultsPage_SearchResults` 返回频道（在播或未开播）：房间搜索
//! 只给在播频道（列表项没有「未开播」字段），主播搜索给全部频道并带 `live_status`。
//! 翻页沿 `options.targets` 里的 CHANNEL cursor，存放在句柄级状态里。

use serde_json::Value;

use crate::error::SliveError;
use crate::models::{LiveAnchorItem, LiveRoomItem, LiveSearchAnchorResult, LiveSearchRoomResult};

use super::gql;
use super::gql_response::{self, GqlFailure};
use super::header::TWITCH_ACCEPT_LANGUAGE_ZH;
use super::site::TwitchSite;

/// 搜索结果里的一个频道。
struct Channel {
    login: String,
    display_name: String,
    avatar: String,
    title: String,
    cover: String,
    online: i32,
    live: bool,
}

struct SearchPage {
    channels: Vec<Channel>,
    has_more: bool,
}

impl SearchPage {
    fn empty() -> Self {
        Self {
            channels: Vec::new(),
            has_more: false,
        }
    }
}

/// F-04 搜索房间：只返回在播频道。
pub(crate) async fn fetch_rooms(
    site: &TwitchSite,
    keyword: &str,
    page: i32,
) -> Result<LiveSearchRoomResult, SliveError> {
    let page = search_page(site, keyword, page).await?;
    Ok(LiveSearchRoomResult {
        has_more: page.has_more,
        items: page
            .channels
            .into_iter()
            .filter(|channel| channel.live)
            .map(|channel| LiveRoomItem {
                room_id: channel.login,
                title: channel.title,
                cover: channel.cover,
                user_name: channel.display_name,
                online: channel.online,
            })
            .collect(),
    })
}

/// F-05 搜索主播：包含未开播频道。
pub(crate) async fn fetch_anchors(
    site: &TwitchSite,
    keyword: &str,
    page: i32,
) -> Result<LiveSearchAnchorResult, SliveError> {
    let page = search_page(site, keyword, page).await?;
    Ok(LiveSearchAnchorResult {
        has_more: page.has_more,
        items: page
            .channels
            .into_iter()
            .map(|channel| LiveAnchorItem {
                room_id: channel.login,
                avatar: channel.avatar,
                user_name: channel.display_name,
                live_status: channel.live,
            })
            .collect(),
    })
}

async fn search_page(
    site: &TwitchSite,
    keyword: &str,
    page: i32,
) -> Result<SearchPage, SliveError> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(SearchPage::empty());
    }
    let number = page.max(1);
    let key = format!("search|{keyword}|{number}");
    let cursor = if number == 1 {
        None
    } else {
        site.cursor_for(&key)
    };
    // 没有上一页记住的 cursor 就没有这一页（对齐 PureLive 的空页收尾）。
    if number > 1 && cursor.is_none() {
        return Ok(SearchPage::empty());
    }

    let decoded = site
        .post_gql_with_language(
            &gql::search_operation(keyword, cursor.as_deref()),
            TWITCH_ACCEPT_LANGUAGE_ZH,
        )
        .await?;
    let data = match gql_response::data(&decoded, "SearchResultsPage_SearchResults") {
        Ok(data) => data,
        // 后续页被要求 integrity token 时安静结束，第一页则照实报错。
        Err(GqlFailure::Integrity) if cursor.is_some() => return Ok(SearchPage::empty()),
        Err(failure) => return Err(failure.into()),
    };

    let channels = data.pointer("/searchFor/channels");
    let items = parse_channels(channels, site.now_secs());
    let next = channels
        .and_then(|value| value.get("cursor"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|_| !items.is_empty());
    let has_more = next.is_some();
    site.remember_cursor(
        &format!("search|{keyword}|{}", number + 1),
        next.as_deref(),
    );
    Ok(SearchPage {
        channels: items,
        has_more,
    })
}

/// `searchFor.channels.edges[].item` 转频道（对照 `TwitchApi.searchPage`）。
fn parse_channels(channels: Option<&Value>, now: u64) -> Vec<Channel> {
    let Some(channels) = channels.filter(|value| value.is_object()) else {
        return Vec::new();
    };
    gql_response::edges(Some(channels))
        .iter()
        .filter_map(|edge| {
            let item = edge.get("item")?;
            let login = item.get("login")?.as_str()?.to_string();
            let stream = item.get("stream").filter(|value| value.is_object());
            let preview = gql_response::text(stream.and_then(|value| value.get("previewImageURL")));
            Some(Channel {
                login,
                display_name: gql_response::text(item.get("displayName")),
                avatar: gql_response::text(item.get("profileImageURL")),
                title: gql_response::text(
                    item.pointer("/broadcastSettings/title"),
                ),
                cover: if preview.trim().is_empty() {
                    String::new()
                } else {
                    format!("{preview}?&t={now}")
                },
                online: gql_response::count(stream.and_then(|value| value.get("viewersCount"))),
                live: stream.is_some(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn search_channels_keep_offline_ones_but_mark_them() {
        let channels = json!({
            "cursor": "MTA=",
            "edges": [
                {"item": {
                    "login": "feinberg", "displayName": "Feinberg",
                    "profileImageURL": "https://cdn/a.png",
                    "broadcastSettings": {"title": "live now"},
                    "stream": {"viewersCount": 3189, "previewImageURL": "https://cdn/p.jpg"}
                }},
                {"item": {
                    "login": "offline", "displayName": "Offline",
                    "broadcastSettings": {"title": "last title"},
                    "stream": null
                }},
                {"item": {"displayName": "no login"}}
            ]
        });
        let items = parse_channels(Some(&channels), 42);
        assert_eq!(items.len(), 2);
        assert!(items[0].live);
        assert_eq!(items[0].online, 3189);
        assert_eq!(items[0].cover, "https://cdn/p.jpg?&t=42");
        assert!(!items[1].live);
        assert_eq!(items[1].online, 0);
        assert!(items[1].cover.is_empty());
    }

    #[test]
    fn missing_channels_is_an_empty_page() {
        assert!(parse_channels(None, 0).is_empty());
    }
}
