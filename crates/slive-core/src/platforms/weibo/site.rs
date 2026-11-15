//! 微博直播站点句柄（对照 PureLive `weibo_site.dart`）。
//!
//! 匿名链路：推荐快照 `pc_recommend`、单场直播 `show_pc_live`；
//! 「房间」是一场直播（`1022:…`），不是主播。没有弹幕。

use async_trait::async_trait;
use std::sync::Arc;

use serde_json::Value;

use crate::error::SliveError;
use crate::http::{HttpRequest, HttpTransport, ReqwestTransport};
use crate::models::*;
use crate::time::{SystemClock, TimeSource};

use super::api;
use super::detail::{self, WeiboBroadcast};
use super::play;

pub struct Site {
    http: Arc<dyn HttpTransport>,
    time: Arc<dyn TimeSource>,
    /// 用户注入的 Cookie（可选，匿名也能用推荐与详情）。
    cookie: std::sync::Mutex<String>,
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
            cookie: std::sync::Mutex::new(String::new()),
        }
    }

    pub fn http(&self) -> Arc<dyn HttpTransport> {
        Arc::clone(&self.http)
    }

    pub fn now_secs(&self) -> u64 {
        self.time.unix_secs()
    }

    /// GET 一个微博 JSON 接口（带 3.x 的 referer / UA）。
    async fn get_json(&self, url: &str) -> Result<Value, SliveError> {
        let mut headers = api::headers();
        let cookie = self
            .cookie
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if !cookie.is_empty() {
            headers.push(("cookie".to_string(), cookie));
        }
        let request = HttpRequest {
            url: url.to_string(),
            query: Vec::new(),
            headers,
        };
        let reply = self.http.get(&request).await?;
        reply.json()
    }

    /// 推荐快照（一次请求，只有第一页）。
    async fn snapshot(&self) -> Result<Vec<LiveRoomItem>, SliveError> {
        let json = self.get_json(&api::recommend_url()).await?;
        api::parse_recommendations(&json)
    }

    /// 取单场直播（详情与每次取流都会重新请求，微博地址没有可复用签名）。
    async fn broadcast(&self, live_id: &str) -> Result<WeiboBroadcast, SliveError> {
        let id = live_id.trim();
        if !api::is_live_id(id) {
            return Err(SliveError::NotFound(format!("不是微博场次 id：{id}")));
        }
        let json = self.get_json(&api::detail_url(id)).await?;
        detail::parse_detail(&json, id)
    }

    /// 昵称筛选（另有场次 id / 分享链接的精确匹配），对齐 3.x 的快照内搜索。
    fn filter_snapshot(items: Vec<LiveRoomItem>, keyword: &str) -> Vec<LiveRoomItem> {
        let query = keyword.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        items
            .into_iter()
            .filter(|item| {
                item.user_name.to_lowercase().contains(&query)
                    || item.room_id.to_lowercase() == query
                    || item.room_id.to_lowercase().ends_with(&query)
            })
            .collect()
    }
}

#[async_trait]
impl crate::platforms::live_site::LiveSite for Site {
    fn id(&self) -> String {
        "weibo".to_string()
    }

    fn name(&self) -> String {
        api::SITE_NAME.to_string()
    }

    fn set_site_attrs(&self, data: &str) -> Result<(), SliveError> {
        let Some(cookie) = crate::platforms::credential::parse_cookie(data)? else {
            return Ok(());
        };
        *self.cookie.lock().unwrap_or_else(|e| e.into_inner()) = cookie;
        Ok(())
    }

    async fn get_categories(&self) -> Result<Vec<LiveCategory>, SliveError> {
        Ok(api::categories())
    }

    async fn get_category_rooms(
        &self,
        category: LiveSubCategory,
        page: i32,
        _page_size: Option<i32>,
    ) -> Result<LiveCategoryResult, SliveError> {
        if !api::is_area(&category) {
            return Err(SliveError::invalid_input("不是微博的公开推荐区域"));
        }
        if page > 1 {
            return Ok(LiveCategoryResult {
                has_more: false,
                items: Vec::new(),
            });
        }
        Ok(LiveCategoryResult {
            has_more: false,
            items: self.snapshot().await?,
        })
    }

    async fn get_recommend_rooms(
        &self,
        page: i32,
        _page_size: Option<i32>,
    ) -> Result<LiveCategoryResult, SliveError> {
        // 快照只有一页；后续页不发请求（对齐 PureLive）。
        if page > 1 {
            return Ok(LiveCategoryResult {
                has_more: false,
                items: Vec::new(),
            });
        }
        Ok(LiveCategoryResult {
            has_more: false,
            items: self.snapshot().await?,
        })
    }

    async fn search_rooms(
        &self,
        keyword: String,
        page: i32,
    ) -> Result<LiveSearchRoomResult, SliveError> {
        if page > 1 || keyword.trim().is_empty() {
            return Ok(LiveSearchRoomResult {
                has_more: false,
                items: Vec::new(),
            });
        }
        let items = Self::filter_snapshot(self.snapshot().await?, &keyword);
        Ok(LiveSearchRoomResult {
            has_more: false,
            items,
        })
    }

    async fn search_anchors(
        &self,
        keyword: String,
        page: i32,
    ) -> Result<LiveSearchAnchorResult, SliveError> {
        if page > 1 || keyword.trim().is_empty() {
            return Ok(LiveSearchAnchorResult {
                has_more: false,
                items: Vec::new(),
            });
        }
        let items = Self::filter_snapshot(self.snapshot().await?, &keyword)
            .into_iter()
            .map(|item| LiveAnchorItem {
                room_id: item.room_id,
                avatar: item.cover,
                user_name: item.user_name,
                live_status: true,
            })
            .collect();
        Ok(LiveSearchAnchorResult {
            has_more: false,
            items,
        })
    }

    async fn get_room_detail(&self, room_id: String) -> Result<LiveRoomDetail, SliveError> {
        Ok(self.broadcast(&room_id).await?.room)
    }

    async fn get_play_qualities(
        &self,
        detail: LiveRoomDetail,
    ) -> Result<Vec<LivePlayQuality>, SliveError> {
        // 每次播放都重新读详情（微博地址没有可复用的签名）。
        let broadcast = self.broadcast(&detail.room_id).await?;
        Ok(vec![play::quality(&broadcast)?])
    }

    async fn get_play_urls(
        &self,
        _detail: LiveRoomDetail,
        quality: LivePlayQuality,
    ) -> Result<LivePlayUrl, SliveError> {
        Ok(play::get_play_url(&quality))
    }

    async fn get_live_status(&self, room_id: String) -> Result<bool, SliveError> {
        Ok(self.broadcast(&room_id).await?.room.status)
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
