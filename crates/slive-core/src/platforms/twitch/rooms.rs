//! F-02 分类下房间 / F-03 首页推荐（对照 PureLive `twitch_site.dart:346-425`）。
//!
//! Twitch 的列表接口用 cursor 翻页，而 core 的契约是 `page` + `page_size`。
//! 因此按 PureLive 的做法：一次抓一大块（分类 100 条 / 推荐 30 条）存成句柄级
//! 快照，之后按页从快照里切片；只有当某页会越过快照末尾时才用 cursor 抓下一块。
//! 第一页总是重新抓，后续页在快照未满 30 秒时复用它。
//!
//! 匿名客户端常在后续块被要求 integrity token，此时**安静结束列表**而不是报错
//! （对照 `_page` 的 `on RiskControl { snapshot.cursor = null; }`）。

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::error::SliveError;
use crate::models::{LiveCategoryResult, LiveRoomItem, LiveSubCategory};

use super::gql;
use super::gql_response::{self, GqlFailure};
use super::header::TWITCH_ACCEPT_LANGUAGE_ZH;
use super::site::TwitchSite;

/// 快照有效期（秒），对照 PureLive `_snapshotLifetime`。
pub(crate) const SNAPSHOT_LIFETIME: u64 = 30;
/// 句柄级快照数量上限，对照 PureLive `_snapshotLimit`。
pub(crate) const SNAPSHOT_LIMIT: usize = 16;
const DEFAULT_PAGE_SIZE: i32 = 30;

/// 一次抓取的结果：房间 + 下一块 cursor。
pub(crate) type Chunk = (Vec<LiveRoomItem>, Option<String>);

/// 一个列表已抓到的房间，按服务端顺序去重存着（对照 PureLive `_Snapshot`）。
#[derive(Clone, Default)]
pub(crate) struct ListSnapshot {
    pub(crate) fetched_at: u64,
    rooms: Vec<LiveRoomItem>,
    seen: HashSet<String>,
    cursor: Option<String>,
    starts: HashMap<i32, usize>,
}

impl ListSnapshot {
    pub(crate) fn new(now: u64, chunk: Chunk) -> Self {
        let mut snapshot = Self {
            fetched_at: now,
            ..Self::default()
        };
        snapshot.add(chunk);
        snapshot
    }

    /// 追加一块；没带来新房间就结束列表（cursor 原地打转时不能一直跟）。
    pub(crate) fn add(&mut self, chunk: Chunk) -> bool {
        let (rooms, cursor) = chunk;
        let mut added = false;
        for room in rooms {
            if self.seen.insert(room.room_id.clone()) {
                self.rooms.push(room);
                added = true;
            }
        }
        self.cursor = if added { cursor } else { None };
        added
    }

    pub(crate) fn len(&self) -> usize {
        self.rooms.len()
    }

    pub(crate) fn has_cursor(&self) -> bool {
        self.cursor.is_some()
    }

    pub(crate) fn cursor(&self) -> Option<String> {
        self.cursor.clone()
    }

    pub(crate) fn end_list(&mut self) {
        self.cursor = None;
    }

    pub(crate) fn is_stale(&self, now: u64) -> bool {
        now.saturating_sub(self.fetched_at) >= SNAPSHOT_LIFETIME
    }

    /// 本页起点：优先沿用上一页记录的结束位置，否则按页宽推算。
    pub(crate) fn start_for(&self, page: i32, size: usize) -> usize {
        if page <= 1 {
            0
        } else {
            self.starts
                .get(&page)
                .copied()
                .unwrap_or((page as usize - 1) * size)
        }
    }

    pub(crate) fn remember_start(&mut self, next_page: i32, end: usize) {
        self.starts.insert(next_page, end);
    }

    pub(crate) fn slice(&self, start: usize, size: usize) -> Vec<LiveRoomItem> {
        if start >= self.rooms.len() {
            return Vec::new();
        }
        let end = (start + size).min(self.rooms.len());
        self.rooms[start..end].to_vec()
    }
}

#[derive(Clone, Copy)]
enum ListKind<'a> {
    Category(&'a str),
    Recommend,
}

/// F-02 某个目录下的直播。空 slug 不发请求（对齐 PureLive）。
pub(crate) async fn fetch_category_rooms(
    site: &TwitchSite,
    category: &LiveSubCategory,
    page: i32,
    page_size: Option<i32>,
) -> Result<LiveCategoryResult, SliveError> {
    let slug = category.id.trim();
    if slug.is_empty() {
        return Ok(LiveCategoryResult {
            has_more: false,
            items: Vec::new(),
        });
    }
    page_list(
        site,
        &format!("area|{slug}"),
        page,
        page_size,
        ListKind::Category(slug),
    )
    .await
}

/// F-03 全站最热直播。
pub(crate) async fn fetch_recommend_rooms(
    site: &TwitchSite,
    page: i32,
    page_size: Option<i32>,
) -> Result<LiveCategoryResult, SliveError> {
    page_list(site, "recommend", page, page_size, ListKind::Recommend).await
}

async fn page_list(
    site: &TwitchSite,
    key: &str,
    page: i32,
    page_size: Option<i32>,
    kind: ListKind<'_>,
) -> Result<LiveCategoryResult, SliveError> {
    let number = page.max(1);
    let size = page_size.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, 100) as usize;
    let now = site.now_secs();

    let mut snapshot = match site.list_snapshot(key) {
        Some(existing) if number != 1 && !existing.is_stale(now) => existing,
        _ => {
            let chunk = fetch_chunk(site, kind, None).await.map_err(SliveError::from)?;
            ListSnapshot::new(now, chunk)
        }
    };

    let start = snapshot.start_for(number, size);
    while start >= snapshot.len() && snapshot.has_cursor() {
        let cursor = snapshot.cursor();
        match fetch_chunk(site, kind, cursor.as_deref()).await {
            Ok(chunk) => {
                if !snapshot.add(chunk) {
                    break;
                }
            }
            Err(GqlFailure::Integrity) => {
                snapshot.end_list();
                break;
            }
            Err(failure) => {
                // 失败时保留原快照，下次调用仍可从旧位置继续。
                site.save_snapshot(key, snapshot);
                return Err(failure.into());
            }
        }
    }

    let items = snapshot.slice(start, size);
    let end = start + items.len();
    let has_more = !items.is_empty() && (end < snapshot.len() || snapshot.has_cursor());
    snapshot.remember_start(number + 1, end);
    site.save_snapshot(key, snapshot);
    Ok(LiveCategoryResult { has_more, items })
}

async fn fetch_chunk(
    site: &TwitchSite,
    kind: ListKind<'_>,
    cursor: Option<&str>,
) -> Result<Chunk, GqlFailure> {
    let connection = match kind {
        ListKind::Category(slug) => {
            let body = gql::build_batch(&[gql::game_operation(slug, cursor)]);
            let decoded = site
                .post_gql_with_language(&body, TWITCH_ACCEPT_LANGUAGE_ZH)
                .await
                .map_err(GqlFailure::Fatal)?;
            let data = gql_response::data(&decoded, "DirectoryPage_Game")?;
            let game = data.get("game").filter(|value| value.is_object()).ok_or_else(|| {
                GqlFailure::Fatal(SliveError::NotFound(format!(
                    "DirectoryPage_Game: no directory {slug}"
                )))
            })?;
            game.get("streams").cloned().unwrap_or(Value::Null)
        }
        ListKind::Recommend => {
            let decoded = site
                .post_gql_with_language(&gql::streams_operation(cursor), TWITCH_ACCEPT_LANGUAGE_ZH)
                .await
                .map_err(GqlFailure::Fatal)?;
            let data = gql_response::data(&decoded, "streams")?;
            data.get("streams").cloned().unwrap_or(Value::Null)
        }
    };
    Ok((
        stream_cards(site.now_secs(), &connection),
        gql_response::next_cursor(Some(&connection)),
    ))
}

/// 一个 stream node 转房间卡片（对照 `TwitchApi._streamCard`）。
fn stream_cards(now: u64, connection: &Value) -> Vec<LiveRoomItem> {
    gql_response::edges(Some(connection))
        .iter()
        .filter_map(|edge| {
            let node = edge.get("node")?;
            let broadcaster = node.get("broadcaster")?;
            let login = broadcaster.get("login")?.as_str()?;
            let display = gql_response::text(broadcaster.get("displayName"));
            let preview = gql_response::text(node.get("previewImageURL"));
            Some(LiveRoomItem {
                room_id: login.to_string(),
                title: gql_response::text(node.get("title")),
                // 3.x 的封面时间戳：刷新时能拿到当前帧。
                cover: if preview.trim().is_empty() {
                    String::new()
                } else {
                    format!("{preview}?&t={now}")
                },
                user_name: if display.is_empty() {
                    login.to_string()
                } else {
                    display
                },
                online: gql_response::count(node.get("viewersCount")),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn room(id: &str) -> LiveRoomItem {
        LiveRoomItem {
            room_id: id.to_string(),
            title: String::new(),
            cover: String::new(),
            user_name: String::new(),
            online: 0,
        }
    }

    #[test]
    fn snapshot_deduplicates_and_stops_on_stalled_cursor() {
        let mut snapshot = ListSnapshot::new(100, (vec![room("a"), room("b")], Some("C1".into())));
        assert_eq!(snapshot.len(), 2);
        assert!(snapshot.add((vec![room("b"), room("c")], Some("C2".into()))));
        assert_eq!(snapshot.len(), 3);
        assert!(!snapshot.add((vec![room("c")], Some("C3".into()))), "无新增须结束");
        assert!(!snapshot.has_cursor());
    }

    #[test]
    fn snapshot_pages_start_where_the_previous_one_ended() {
        let mut snapshot = ListSnapshot::new(100, ((0..5).map(|i| room(&format!("r{i}"))).collect(), None));
        assert_eq!(snapshot.start_for(1, 2), 0);
        assert_eq!(snapshot.slice(0, 2).len(), 2);
        snapshot.remember_start(2, 2);
        assert_eq!(snapshot.start_for(2, 2), 2);
        assert_eq!(snapshot.start_for(3, 2), 4);
        assert!(snapshot.slice(4, 2).len() == 1);
        assert!(!snapshot.is_stale(100 + SNAPSHOT_LIFETIME - 1));
        assert!(snapshot.is_stale(100 + SNAPSHOT_LIFETIME));
    }

    #[test]
    fn stream_card_uses_login_as_room_id_and_stamps_cover() {
        let connection = json!({"edges": [
            {"node": {
                "title": "hello", "viewersCount": 3189,
                "previewImageURL": "https://cdn/p.jpg",
                "broadcaster": {"login": "feinberg", "displayName": "Feinberg"}
            }},
            {"node": {"title": "no broadcaster"}}
        ]});
        let cards = stream_cards(1_700_000_000, &connection);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].room_id, "feinberg");
        assert_eq!(cards[0].user_name, "Feinberg");
        assert_eq!(cards[0].online, 3189);
        assert_eq!(cards[0].cover, "https://cdn/p.jpg?&t=1700000000");
    }
}
