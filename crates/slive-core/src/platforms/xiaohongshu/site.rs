//! 小红书站点句柄（对照 PureLive `xiaohongshu_site.dart`）。
//!
//! 只有分享页链路：房间详情与取流都来自
//! `www.xiaohongshu.com/livestream/<roomId>` 的 hydration state。
//! 目录 / 关键词搜索需要浏览器访客会话（本 core 无 WebView），这里返回空。

use async_trait::async_trait;
use std::sync::Arc;

use crate::error::SliveError;
use crate::http::{HttpRequest, HttpTransport, ReqwestTransport};
use crate::models::*;
use crate::time::{SystemClock, TimeSource};

use super::api;
use super::detail::{self, XhsPage};
use super::play;

pub struct Site {
    http: Arc<dyn HttpTransport>,
    time: Arc<dyn TimeSource>,
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
        Self { http, time }
    }

    pub fn http(&self) -> Arc<dyn HttpTransport> {
        Arc::clone(&self.http)
    }

    pub fn now_secs(&self) -> u64 {
        self.time.unix_secs()
    }

    /// GET 分享页 HTML（带 3.x 的 Referer / UA，不跟随重定向由传输层决定）。
    async fn fetch_page(&self, room_id: &str) -> Result<XhsPage, SliveError> {
        if !api::is_room_id(room_id) {
            return Err(SliveError::NotFound(format!(
                "不是小红书房间 id：{room_id}"
            )));
        }
        let request = HttpRequest {
            url: api::room_url(room_id),
            query: Vec::new(),
            headers: api::headers(),
        };
        let reply = self.http.get(&request).await?;
        detail::parse_page(&reply.text, room_id)
    }
}

#[async_trait]
impl crate::platforms::live_site::LiveSite for Site {
    fn id(&self) -> String {
        "xiaohongshu".to_string()
    }

    fn name(&self) -> String {
        "小红书直播".to_string()
    }

    fn set_site_attrs(&self, _data: &str) -> Result<(), SliveError> {
        Err(SliveError::invalid_input("小红书不支持注入凭证"))
    }

    async fn get_categories(&self) -> Result<Vec<LiveCategory>, SliveError> {
        // 平台分类需要浏览器访客会话；无 WebView 的宿主只有分享页链路。
        Ok(Vec::new())
    }

    async fn get_category_rooms(
        &self,
        _category: LiveSubCategory,
        _page: i32,
        _page_size: Option<i32>,
    ) -> Result<LiveCategoryResult, SliveError> {
        Err(SliveError::invalid_input(
            "小红书没有可用的分区列表（需要浏览器会话）",
        ))
    }

    async fn get_recommend_rooms(
        &self,
        _page: i32,
        _page_size: Option<i32>,
    ) -> Result<LiveCategoryResult, SliveError> {
        // 公开推荐页需要访客会话；这里如实返回空而不是猜数据。
        Ok(LiveCategoryResult {
            has_more: false,
            items: Vec::new(),
        })
    }

    async fn search_rooms(&self, _keyword: String, _page: i32) -> Result<LiveSearchRoomResult, SliveError> {
        Ok(LiveSearchRoomResult {
            has_more: false,
            items: Vec::new(),
        })
    }

    async fn search_anchors(
        &self,
        _keyword: String,
        _page: i32,
    ) -> Result<LiveSearchAnchorResult, SliveError> {
        Ok(LiveSearchAnchorResult {
            has_more: false,
            items: Vec::new(),
        })
    }

    async fn get_room_detail(&self, room_id: String) -> Result<LiveRoomDetail, SliveError> {
        Ok(self.fetch_page(room_id.trim()).await?.room)
    }

    async fn get_play_qualities(
        &self,
        detail: LiveRoomDetail,
    ) -> Result<Vec<LivePlayQuality>, SliveError> {
        // 详情来自 `get_room_detail` 时已带解析结果；缺数据（外部构造）才重新读页。
        if detail.data.is_some() {
            return play::qualities(&detail);
        }
        let page = self.fetch_page(detail.room_id.trim()).await?;
        play::qualities(&page.room)
    }

    async fn get_play_urls(
        &self,
        _detail: LiveRoomDetail,
        quality: LivePlayQuality,
    ) -> Result<LivePlayUrl, SliveError> {
        Ok(play::get_play_url(&quality))
    }

    async fn get_live_status(&self, room_id: String) -> Result<bool, SliveError> {
        Ok(self.fetch_page(room_id.trim()).await?.live == Some(true))
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
