//! 快手站点句柄（对照 PureLive `kuaishou_site.dart`）。
//!
//! 列表走 `/live_api/*` JSON；房间详情与取流解析房间页的
//! `window.__INITIAL_STATE__`。匿名访问偶发验证页，此时先用首页换一份
//! 会话 cookie 再取一次（PureLive 的 `_ensureSession` 简化版）。

use async_trait::async_trait;
use std::sync::{Arc, Mutex};

use crate::error::SliveError;
use crate::http::{HttpRequest, HttpTransport, ReqwestTransport};
use crate::models::*;
use crate::time::{SystemClock, TimeSource};

use super::api;

/// 桌面浏览器 UA（对齐 PureLive 的 web 头）。
const WEB_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

/// 快手站点实例。
pub struct KuaishouSite {
    http: Arc<dyn HttpTransport>,
    time: Arc<dyn TimeSource>,
    /// 首页换来的匿名会话 cookie（`name=value; ...`）。
    session: Mutex<Option<String>>,
    /// 搜索用的 `lssid`（进程内稳定）。
    lssid: String,
    /// 用户注入的 Cookie（优先于匿名会话）。
    user_cookie: Mutex<String>,
}

impl Default for KuaishouSite {
    fn default() -> Self {
        Self::new()
    }
}

impl KuaishouSite {
    pub fn new() -> Self {
        Self::with_deps(Arc::new(ReqwestTransport::new()), Arc::new(SystemClock))
    }

    pub fn with_deps(http: Arc<dyn HttpTransport>, time: Arc<dyn TimeSource>) -> Self {
        let lssid = format!("slive-{}", time.unix_secs());
        Self {
            http,
            time,
            session: Mutex::new(None),
            lssid,
            user_cookie: Mutex::new(String::new()),
        }
    }

    pub fn http(&self) -> Arc<dyn HttpTransport> {
        Arc::clone(&self.http)
    }

    pub fn now_secs(&self) -> u64 {
        self.time.unix_secs()
    }

    fn user_cookie(&self) -> Option<String> {
        let cookie = self
            .user_cookie
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if cookie.is_empty() {
            None
        } else {
            Some(cookie)
        }
    }

    fn session_cookie(&self) -> Option<String> {
        self.session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn set_session_cookie(&self, cookie: String) {
        if !cookie.is_empty() {
            *self.session.lock().unwrap_or_else(|e| e.into_inner()) = Some(cookie);
        }
    }

    /// 取房间页 HTML；遇到验证页时先访问首页拿 cookie 再重试一次。
    async fn fetch_room_html(&self, room_id: &str) -> Result<String, SliveError> {
        let url = api::room_page(room_id);
        let html = self.get_page(&url, false).await?;
        if html.contains("__INITIAL_STATE__") {
            return Ok(html);
        }

        // 无状态：换一份匿名会话 cookie 后重试。
        let home = self.get_page(&api::home_page(), true).await?;
        let _ = home;
        let retried = self.get_page(&url, false).await?;
        if !retried.contains("__INITIAL_STATE__") {
            return Err(SliveError::Api {
                code: -1,
                message: "快手房间页需要验证（未找到 __INITIAL_STATE__）".to_string(),
            });
        }
        Ok(retried)
    }

    /// GET 页面；`capture_cookie` 时把 `Set-Cookie` 存成会话 cookie。
    async fn get_page(&self, url: &str, capture_cookie: bool) -> Result<String, SliveError> {
        let mut headers = vec![
            ("user-agent".to_string(), WEB_USER_AGENT.to_string()),
            (
                "accept".to_string(),
                "text/html,application/xhtml+xml".to_string(),
            ),
            ("accept-language".to_string(), "zh-CN,zh;q=0.9".to_string()),
        ];
        if let Some(cookie) = self.user_cookie().or_else(|| self.session_cookie()) {
            headers.push(("cookie".to_string(), cookie));
        }
        let reply = self
            .http
            .get(&HttpRequest {
                url: url.to_string(),
                query: Vec::new(),
                headers,
            })
            .await?;
        if capture_cookie {
            let cookies: Vec<String> = reply
                .set_cookie
                .iter()
                .filter_map(|raw| raw.split(';').next())
                .map(str::trim)
                .filter(|pair| pair.contains('='))
                .map(str::to_string)
                .collect();
            if !cookies.is_empty() {
                self.set_session_cookie(cookies.join("; "));
            }
        }
        Ok(reply.text)
    }

    /// GET `/live_api/*` JSON：与页面请求共用同一份 Cookie（用户注入优先）。
    async fn get_api_json(&self, url: &str) -> Result<serde_json::Value, SliveError> {
        let mut headers = vec![
            (
                "user-agent".to_string(),
                crate::http::DEFAULT_USER_AGENT.to_string(),
            ),
            (
                "accept-language".to_string(),
                "zh-CN,zh;q=0.9,en;q=0.8".to_string(),
            ),
            (
                "referer".to_string(),
                "https://live.kuaishou.com/".to_string(),
            ),
        ];
        if let Some(cookie) = self.user_cookie().or_else(|| self.session_cookie()) {
            headers.push(("cookie".to_string(), cookie));
        }
        self.http
            .get(&HttpRequest {
                url: url.to_string(),
                query: Vec::new(),
                headers,
            })
            .await?
            .json()
    }
}

#[async_trait]
impl crate::platforms::live_site::LiveSite for KuaishouSite {
    fn id(&self) -> String {
        "kuaishou".to_string()
    }

    fn name(&self) -> String {
        "快手".to_string()
    }

    fn set_site_attrs(&self, data: &str) -> Result<(), SliveError> {
        let Some(cookie) = super::super::credential::parse_cookie(data)? else {
            return Ok(());
        };
        *self.user_cookie.lock().unwrap_or_else(|e| e.into_inner()) = cookie;
        Ok(())
    }

    async fn get_categories(&self) -> Result<Vec<LiveCategory>, SliveError> {
        // 8 个固定分类 + 各自的区域（区域接口 50 条/页，按 hasMore 翻页）。
        let mut categories = api::fixed_categories();
        for category in &mut categories {
            let mut children: Vec<LiveSubCategory> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for page in 1..=50 {
                let json = self
                    .get_api_json(&api::area_list(&category.id, page))
                    .await?;
                let (areas, has_more) = api::parse_areas(&json, &category.id, &category.name)?;
                let mut added = false;
                for area in areas {
                    if seen.insert(area.id.clone()) {
                        children.push(area);
                        added = true;
                    }
                }
                if !has_more || !added {
                    break;
                }
            }
            category.children = children;
        }
        Ok(categories)
    }

    async fn get_category_rooms(
        &self,
        category: LiveSubCategory,
        page: i32,
        _page_size: Option<i32>,
    ) -> Result<LiveCategoryResult, SliveError> {
        let area_id = category.id.trim();
        if area_id.is_empty() {
            return Ok(LiveCategoryResult {
                has_more: false,
                items: Vec::new(),
            });
        }

        // 区域房间固定 20 条/页；非游戏板的后续页需要上一页的 cursor，
        // 故从第 1 页顺序走到目标页（与 PureLive 的 `_areaRooms` 同口径）。
        let target = page.max(1);
        let mut cursor = String::new();
        let mut items = Vec::new();
        let mut has_more = false;
        for current in 1..=target {
            let json = self
                .get_api_json(&api::area_rooms(area_id, current, &cursor))
                .await?;
            let (cards, more, next) = api::parse_area_rooms(&json)?;
            has_more = more;
            if current == target {
                let mut seen = std::collections::HashSet::new();
                items = cards
                    .iter()
                    .filter(|card| !card.author.id.is_empty())
                    .filter(|card| seen.insert(card.author.id.clone()))
                    .map(api::card_to_room_item)
                    .collect();
            } else {
                if !more {
                    has_more = false;
                    break;
                }
                // gameboard 没有 cursor（靠 page 前进），non-gameboard 才带。
                cursor = next.unwrap_or_default();
            }
        }
        Ok(LiveCategoryResult { has_more, items })
    }

    async fn get_recommend_rooms(
        &self,
        _page: i32,
        page_size: Option<i32>,
    ) -> Result<LiveCategoryResult, SliveError> {
        let json = self.get_api_json(&api::home_list()).await?;
        let mut items: Vec<LiveRoomItem> = api::parse_home_list(&json)?
            .iter()
            .filter(|card| !card.author.id.is_empty())
            .map(api::card_to_room_item)
            .collect();
        let limit = page_size.unwrap_or(30).clamp(1, 100) as usize;
        items.truncate(limit);
        Ok(LiveCategoryResult {
            has_more: false,
            items,
        })
    }

    async fn search_rooms(
        &self,
        keyword: String,
        page: i32,
    ) -> Result<LiveSearchRoomResult, SliveError> {
        let json = self
            .get_api_json(&api::search_author(&keyword, page.max(1), &self.lssid))
            .await?;
        let items: Vec<LiveRoomItem> = api::parse_search_authors(&json)?
            .iter()
            .map(api::author_to_room_item)
            .collect();
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
        let json = self
            .get_api_json(&api::search_author(&keyword, page.max(1), &self.lssid))
            .await?;
        let items: Vec<LiveAnchorItem> = api::parse_search_authors(&json)?
            .iter()
            .map(|author| LiveAnchorItem {
                room_id: author.id.clone(),
                avatar: author.avatar.clone().unwrap_or_default(),
                user_name: author.name.clone(),
                live_status: author.living,
            })
            .collect();
        Ok(LiveSearchAnchorResult {
            has_more: false,
            items,
        })
    }

    async fn get_room_detail(&self, room_id: String) -> Result<LiveRoomDetail, SliveError> {
        let html = self.fetch_room_html(&room_id).await?;
        super::detail::parse_room_html(&html, &room_id)
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
        let html = self.fetch_room_html(&detail.room_id).await?;
        let state = super::detail::extract_initial_state(&html)?;
        let play_urls =
            super::detail::extract_play_urls(&state).ok_or_else(|| SliveError::Api {
                code: -1,
                message: "房间页没有 playUrls".to_string(),
            })?;
        super::play::parse_qualities(&play_urls, true)
    }

    async fn get_play_urls(
        &self,
        detail: LiveRoomDetail,
        quality: LivePlayQuality,
    ) -> Result<LivePlayUrl, SliveError> {
        Ok(super::play::get_play_url(&quality, &detail.room_id))
    }

    async fn get_live_status(&self, room_id: String) -> Result<bool, SliveError> {
        let detail = self.get_room_detail(room_id).await?;
        Ok(detail.status)
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
