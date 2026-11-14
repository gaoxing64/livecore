//! YouTube 站点句柄（对照 PureLive `youtube_site.dart`）。
//!
//! - 列表/搜索：Web 客户端 InnerTube（`browse` 的 "Live" 页 / `search` 的 Live 过滤）；
//! - 房间详情：频道房间沿 `resolve_url` 的 live 标签页取当前直播；
//! - 取流：`player`（ANDROID 上下文）——当前网络环境匿名请求会被要求登录/PO token，
//!   此时返回明确的错误而不是伪造地址。

use async_trait::async_trait;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::error::SliveError;
use crate::http::{HttpRequest, HttpTransport, HttpTransportExt, ReqwestTransport};
use crate::models::*;
use crate::time::{SystemClock, TimeSource};

use super::api;

/// continuation 缓存上限（按关键词 × 页）。
const CURSOR_CACHE_LIMIT: usize = 128;

#[derive(Default)]
struct SiteState {
    /// `keyword|generation|page` -> 该页的 continuation token。
    cursors: std::collections::HashMap<String, String>,
    /// 每个关键词当前会话代数（首页刷新后旧 cursor 失效）。
    generations: std::collections::HashMap<String, u64>,
}

pub struct Site {
    http: Arc<dyn HttpTransport>,
    time: Arc<dyn TimeSource>,
    state: Mutex<SiteState>,
}

impl Default for Site {
    fn default() -> Self {
        Self::new()
    }
}

impl Site {
    pub fn new() -> Self {
        Self::with_deps(Arc::new(ReqwestTransport::new()), Arc::new(SystemClock))
    }

    pub fn with_deps(http: Arc<dyn HttpTransport>, time: Arc<dyn TimeSource>) -> Self {
        Self {
            http,
            time,
            state: Mutex::new(SiteState::default()),
        }
    }

    pub fn http(&self) -> Arc<dyn HttpTransport> {
        Arc::clone(&self.http)
    }

    pub fn now_secs(&self) -> u64 {
        self.time.unix_secs()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SiteState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// POST 一条 InnerTube JSON 请求（Web 上下文，不带 key）。
    async fn post_api(&self, endpoint: &str, body: Value) -> Result<Value, SliveError> {
        let payload = serde_json::to_string(&body)
            .map_err(|e| SliveError::decode(format!("序列化 InnerTube 请求失败：{e}")))?;
        let request = HttpRequest {
            url: api::api_url(endpoint),
            query: Vec::new(),
            headers: api::api_headers(),
        };
        let reply = self.http.post_json_with(&request, &payload).await?;
        serde_json::from_str(&reply)
            .map_err(|e| SliveError::decode(format!("解析 InnerTube 响应失败：{e}")))
    }

    /// 频道当前（或下一场）直播的卡片。`path` 可以是频道 id，也可以是 `@handle`。
    async fn channel_live_card(&self, path: &str) -> Result<Option<api::VideoCard>, SliveError> {
        let live_url = if api::is_channel_id(path) {
            format!("https://www.youtube.com/channel/{path}/live")
        } else {
            format!("https://www.youtube.com/{path}/live")
        };
        let resolved = self
            .post_api("navigation/resolve_url", api::resolve_body(&live_url))
            .await?;
        // 在播频道直接给出 watchEndpoint.videoId；未在播时给出频道 live 标签页。
        if let Some(video_id) = api::resolve_watch_video_id(&resolved) {
            return self.video_live_card(&video_id).await;
        }
        let Some((browse_id, params)) = api::resolve_browse_endpoint(&resolved) else {
            return Ok(None);
        };
        let page = self
            .post_api("browse", api::browse_body(&browse_id, params.as_deref()))
            .await?;
        Ok(api::listing(&page, false)
            .cards
            .into_iter()
            .find(|card| card.channel_id == browse_id))
    }

    /// 视频 id 对应的直播卡片（用搜索定位）。
    async fn video_live_card(&self, video_id: &str) -> Result<Option<api::VideoCard>, SliveError> {
        let page = self.post_api("search", api::search_body(video_id)).await?;
        Ok(api::listing(&page, true)
            .cards
            .into_iter()
            .find(|card| card.video_id == video_id))
    }

    /// 卡片转房间详情（`data` 里带 videoId / channelId）。
    fn card_to_detail(card: api::VideoCard) -> LiveRoomDetail {
        LiveRoomDetail {
            room_id: card.channel_id.clone(),
            title: card.title.clone(),
            cover: card.cover.clone(),
            user_name: card.owner.clone(),
            user_avatar: card.avatar.clone(),
            online: card.watching,
            introduction: None,
            notice: None,
            status: true,
            data: Some(
                serde_json::json!({
                    "video_id": card.video_id,
                    "channel_id": card.channel_id,
                })
                .to_string(),
            ),
            danmaku_data: Some(card.video_id.clone()),
            is_record: false,
            url: format!("https://www.youtube.com/watch?v={}", card.video_id),
        }
    }

    fn card_to_room_item(card: &api::VideoCard) -> LiveRoomItem {
        LiveRoomItem {
            room_id: card.channel_id.clone(),
            title: card.title.clone(),
            cover: card.cover.clone(),
            user_name: card.owner.clone(),
            online: card.watching,
        }
    }

    /// 详情里的直播视频 id：优先 `data.video_id`，兼容房间 id 本身就是视频 id 的情况。
    fn video_id_of(detail: &LiveRoomDetail) -> Option<String> {
        detail
            .data
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .and_then(|value| {
                value
                    .get("video_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .or_else(|| {
                if api::is_video_id(&detail.room_id) {
                    Some(detail.room_id.clone())
                } else {
                    None
                }
            })
    }

    /// 搜索一页（page 1 新建会话，后续页用 continuation）。
    async fn search_page(&self, keyword: &str, page: i32) -> Result<api::Listing, SliveError> {
        let text = keyword.trim();
        if text.is_empty() {
            return Ok(api::Listing::default());
        }
        let number = page.max(1);
        let (body, generation) = if number == 1 {
            let generation = {
                let mut state = self.lock();
                let entry = state.generations.entry(text.to_string()).or_insert(0);
                *entry += 1;
                *entry
            };
            let prefix = format!("{text}|");
            let keep = format!("{text}|{generation}|");
            self.lock()
                .cursors
                .retain(|key, _| !key.starts_with(&prefix) || key.starts_with(&keep));
            (api::search_body(text), generation)
        } else {
            let generation = self.lock().generations.get(text).copied().unwrap_or(0);
            let key = format!("{text}|{generation}|{number}");
            let Some(token) = self.lock().cursors.get(&key).cloned() else {
                return Ok(api::Listing::default());
            };
            (api::continuation_body(&token), generation)
        };

        let decoded = self.post_api("search", body).await?;
        let listing = api::listing(&decoded, true);
        if let Some(next) = listing.next.clone() {
            let mut state = self.lock();
            state
                .cursors
                .insert(format!("{text}|{generation}|{}", number + 1), next);
            if state.cursors.len() > CURSOR_CACHE_LIMIT {
                // 只淘汰其他关键词的会话，保留当前关键词刚写入的 cursor。
                let prefix = format!("{text}|");
                state.cursors.retain(|key, _| key.starts_with(&prefix));
            }
        }
        Ok(listing)
    }
}

#[async_trait]
impl crate::platforms::live_site::LiveSite for Site {
    fn id(&self) -> String {
        "youtube".to_string()
    }

    fn name(&self) -> String {
        "YouTube Live".to_string()
    }

    fn set_site_attrs(&self, _data: &str) -> Result<(), SliveError> {
        Err(SliveError::invalid_input("YouTube 不支持注入凭证"))
    }

    async fn get_categories(&self) -> Result<Vec<LiveCategory>, SliveError> {
        // YouTube 没有分区。
        Ok(Vec::new())
    }

    async fn get_category_rooms(
        &self,
        _category: LiveSubCategory,
        _page: i32,
        _page_size: Option<i32>,
    ) -> Result<LiveCategoryResult, SliveError> {
        Err(SliveError::invalid_input("YouTube 没有分区"))
    }

    async fn get_recommend_rooms(
        &self,
        page: i32,
        _page_size: Option<i32>,
    ) -> Result<LiveCategoryResult, SliveError> {
        // "Live" 目的地只有第一页。
        if page > 1 {
            return Ok(LiveCategoryResult {
                has_more: false,
                items: Vec::new(),
            });
        }
        let decoded = self.post_api("browse", api::browse_live_body()).await?;
        let listing = api::listing(&decoded, false);
        Ok(LiveCategoryResult {
            has_more: false,
            items: listing.cards.iter().map(Self::card_to_room_item).collect(),
        })
    }

    async fn search_rooms(&self, keyword: String, page: i32) -> Result<LiveSearchRoomResult, SliveError> {
        let listing = self.search_page(&keyword, page).await?;
        Ok(LiveSearchRoomResult {
            has_more: listing.next.is_some(),
            items: listing.cards.iter().map(Self::card_to_room_item).collect(),
        })
    }

    async fn search_anchors(
        &self,
        keyword: String,
        page: i32,
    ) -> Result<LiveSearchAnchorResult, SliveError> {
        let listing = self.search_page(&keyword, page).await?;
        Ok(LiveSearchAnchorResult {
            has_more: listing.next.is_some(),
            items: listing
                .cards
                .iter()
                .map(|card| LiveAnchorItem {
                    room_id: card.channel_id.clone(),
                    avatar: card.avatar.clone(),
                    user_name: card.owner.clone(),
                    live_status: true,
                })
                .collect(),
        })
    }

    async fn get_room_detail(&self, room_id: String) -> Result<LiveRoomDetail, SliveError> {
        let id = room_id.trim();
        if api::is_video_id(id) {
            if let Some(card) = self.video_live_card(id).await? {
                return Ok(Self::card_to_detail(card));
            }
            return Err(SliveError::NotFound(format!(
                "视频 {id} 当前没有在播直播"
            )));
        }
        if api::is_channel_id(id) || id.starts_with('@') {
            if let Some(card) = self.channel_live_card(id).await? {
                return Ok(Self::card_to_detail(card));
            }
            return Err(SliveError::NotFound(format!(
                "频道 {id} 当前没有直播"
            )));
        }
        Err(SliveError::NotFound(format!(
            "不是 YouTube 频道或直播 id：{id}"
        )))
    }

    async fn get_play_qualities(
        &self,
        detail: LiveRoomDetail,
    ) -> Result<Vec<LivePlayQuality>, SliveError> {
        if !detail.status {
            return Err(SliveError::Api {
                code: -1,
                message: "房间未开播".to_string(),
            });
        }
        let video_id = Self::video_id_of(&detail).ok_or_else(|| SliveError::Api {
            code: -1,
            message: "详情里没有 videoId（请重新取详情）".to_string(),
        })?;

        // 播放器需要 API key：优先用观看页里的，找不到用兜底值。
        let watch_html = self
            .http
            .get_with_headers(
                &format!("https://www.youtube.com/watch?v={video_id}"),
                &api::page_headers(&video_id),
            )
            .await?;
        let api_key = api::api_key_of(&watch_html).unwrap_or_else(|| api::FALLBACK_API_KEY.to_string());

        let payload = serde_json::to_string(&api::player_body(&video_id))
            .map_err(|e| SliveError::decode(format!("序列化 player 请求失败：{e}")))?;
        let request = HttpRequest {
            url: api::player_url(&api_key),
            query: Vec::new(),
            headers: api::api_headers(),
        };
        let body = match self.http.post_json_with(&request, &payload).await {
            Ok(body) => body,
            Err(SliveError::Http { status, .. }) if matches!(status, 400 | 401 | 403) => {
                return Err(SliveError::Api {
                    code: -1,
                    message: format!(
                        "YouTube 拒绝匿名取流（HTTP {status}）；需要登录态或 PO token"
                    ),
                })
            }
            Err(error) => return Err(error),
        };
        let decoded: Value = serde_json::from_str(&body)
            .map_err(|e| SliveError::decode(format!("解析 player 响应失败：{e}")))?;

        let status = decoded
            .pointer("/playabilityStatus/status")
            .and_then(Value::as_str)
            .unwrap_or("");
        if status != "OK" {
            let reason = decoded
                .pointer("/playabilityStatus/reason")
                .and_then(Value::as_str)
                .unwrap_or("");
            // 没有 playabilityStatus 时附上响应片段，便于判断是风控还是接口变化。
            let snippet = if status.is_empty() {
                let text = body.trim().replace(['\n', '\r'], " ");
                let head: String = text.chars().take(160).collect();
                format!("；响应片段：{head}")
            } else {
                String::new()
            };
            return Err(SliveError::Api {
                code: -1,
                message: format!(
                    "YouTube 拒绝匿名取流（{}{}{}）{snippet}",
                    if status.is_empty() { "无 playabilityStatus" } else { status },
                    if reason.is_empty() { "" } else { ": " },
                    reason
                ),
            });
        }
        super::play::parse_adaptive_formats(&decoded)
    }

    async fn get_play_urls(
        &self,
        detail: LiveRoomDetail,
        quality: LivePlayQuality,
    ) -> Result<LivePlayUrl, SliveError> {
        // 媒体 Referer 要指向真正的 watch 页：房间 id 是频道 id，不能直接拼。
        let video_id = Self::video_id_of(&detail).unwrap_or_default();
        Ok(super::play::get_play_url(&quality, &video_id))
    }

    async fn get_live_status(&self, room_id: String) -> Result<bool, SliveError> {
        match self.get_room_detail(room_id).await {
            Ok(detail) => Ok(detail.status),
            // 只有「频道当前没有直播」才是否播；网络/接口失败照实抛错，不伪装成离线。
            Err(SliveError::NotFound(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn get_super_chat_messages(
        &self,
        _room_id: String,
    ) -> Result<Vec<LiveSuperChatMessage>, SliveError> {
        Ok(Vec::new())
    }

    async fn get_qr_code(&self) -> Result<QrCode, SliveError> {
        Err(SliveError::invalid_input("该平台不支持二维码登录"))
    }

    async fn get_qr_poll_result(&self, _qrcode_key: String) -> Result<QrPollResult, SliveError> {
        Err(SliveError::invalid_input("该平台不支持二维码登录"))
    }

    async fn get_account_info(&self, _credential: String) -> Result<AccountInfo, SliveError> {
        Err(SliveError::invalid_input("该平台不支持账号信息查询"))
    }
}
