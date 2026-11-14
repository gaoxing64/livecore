//! ACFUN 站点句柄（对照 PureLive `acfun_site.dart`）。
//!
//! 流程：visitor/login → live/info → startPlay → 取流

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::error::SliveError;
use crate::http::{HttpRequest, HttpTransport, HttpTransportExt, ReqwestTransport};
use crate::models::*;
use crate::time::{SystemClock, TimeSource};
use serde_json::Value;

use super::visitor::AcfunVisitor;

/// ACFUN 站点实例。
///
/// `visitor` 用 tokio 的异步锁：登录请求要跨 await 持锁，让并发首调只有一个
/// 真正登录（std 锁的 guard 不是 Send，不能跨 await 持有）。
pub struct AcfunSite {
    http: Arc<dyn HttpTransport>,
    time: Arc<dyn TimeSource>,
    visitor: Mutex<Option<AcfunVisitor>>,
}

impl Default for AcfunSite {
    fn default() -> Self {
        Self::new()
    }
}

impl AcfunSite {
    pub fn new() -> Self {
        Self::with_deps(
            Arc::new(ReqwestTransport::new()),
            Arc::new(SystemClock),
        )
    }

    pub fn with_deps(http: Arc<dyn HttpTransport>, time: Arc<dyn TimeSource>) -> Self {
        Self {
            http,
            time,
            visitor: Mutex::new(None),
        }
    }

    pub fn http(&self) -> Arc<dyn HttpTransport> {
        Arc::clone(&self.http)
    }

    pub fn now_secs(&self) -> u64 {
        self.time.unix_secs()
    }

    /// 获取或刷新 visitor 会话（5 分钟有效期）。
    ///
    /// 跨登录请求持锁：并发首调只有一次真正发 `visitor/login`，其余等待后复用结果。
    async fn ensure_visitor(&self) -> Result<AcfunVisitor, SliveError> {
        let now = self.now_secs();
        let mut cached = self.visitor.lock().await;
        if let Some(visitor) = cached.as_ref() {
            if !visitor.is_expired(now) {
                return Ok(visitor.clone());
            }
        }

        let device_id = super::visitor::generate_device_id();
        let url = "https://id.app.acfun.cn/rest/app/visitor/login";
        let body = "sid=acfun.api.visitor".to_string();
        let response = self
            .http
            .post_form_with_cookie(url, &body, &format!("_did={device_id};"))
            .await?;
        let visitor = AcfunVisitor::from_response(&response, &device_id, now)?;
        *cached = Some(visitor.clone());
        Ok(visitor)
    }

    /// 调用 startPlay 获取播放数据。
    ///
    /// 结束码（129004 关播 / 380205 付费无权限）在这里统一分类，
    /// `play::parse_qualities` 只处理成功应答的档位解析。
    async fn start_play(&self, author_id: &str) -> Result<serde_json::Value, SliveError> {
        let visitor = self.ensure_visitor().await?;

        // visitor token 是 base64：必须百分号编码后再拼进 query，
        // 否则 `+` / `/` / `=` 会把参数截断（服务端回 -401）。authorId 来自调用方，
        // 同样编码后再进 form body。
        let token = crate::util::encode_query_component(&visitor.token);
        let url = format!(
            "https://api.kuaishouzt.com/rest/zt/live/web/startPlay?subBiz=mainApp&kpn=ACFUN_APP&kpf=PC_WEB&userId={}&did={}&acfun.api.visitor_st={}",
            visitor.user_id, visitor.device_id, token
        );
        let body = format!(
            "authorId={}&pullStreamType=FLV",
            crate::util::encode_query_component(author_id)
        );

        let response = self
            .http
            .post_form_with_headers(
                &url,
                &body,
                &[
                    (
                        "user-agent",
                        crate::http::DEFAULT_USER_AGENT.to_string(),
                    ),
                    ("referer", "https://www.acfun.cn/".to_string()),
                ],
            )
            .await?;

        let json: serde_json::Value = serde_json::from_str(&response)
            .map_err(|e| SliveError::decode(format!("解析 startPlay 失败：{e}")))?;

        let result = json.get("result").and_then(|v| v.as_i64()).unwrap_or(-1);
        if result == 129004 {
            return Err(SliveError::Api {
                code: 129004,
                message: "直播已关播".to_string(),
            });
        }
        if result == 380205 {
            return Err(SliveError::Api {
                code: 380205,
                message: "付费直播，无观看权限".to_string(),
            });
        }
        if result != 1 {
            return Err(SliveError::Api {
                code: result as i32,
                message: format!("startPlay: result {result}"),
            });
        }

        Ok(json)
    }

    /// 调用 channel/list（`count=1` 只为拿区域过滤器）。
    async fn list_json(&self, count: i32, cursor: &str, filters: Option<&str>) -> Result<Value, SliveError> {
        let url = super::api::channel_list(count, cursor, filters);
        self.http.get_json(&url).await
    }

    /// 调用 live/info API（ACFUN 对浏览器 UA 返回不同的精简结构，故用 curl UA）。
    async fn get_room_info(&self, author_id: &str) -> Result<Value, SliveError> {
        let request = HttpRequest {
            url: super::api::live_info(author_id),
            query: vec![],
            headers: vec![(
                "user-agent".to_string(),
                super::api::CURL_USER_AGENT.to_string(),
            )],
        };
        let reply = self.http.get(&request).await?;
        serde_json::from_str(&reply.text)
            .map_err(|e| SliveError::decode(format!("解析 live/info 失败：{e}")))
    }

    /// 一页目录（`page_size` 限 1–60）；`page > 1` 需要沿 cursor 顺序走到目标页。
    async fn directory(
        &self,
        page: i32,
        page_size: Option<i32>,
        filters: Option<&str>,
    ) -> Result<LiveCategoryResult, SliveError> {
        let size = page_size.unwrap_or(30).clamp(1, 60);
        let mut cursor = String::new();
        let mut items = Vec::new();
        let mut has_more = false;
        for current in 1..=page {
            let json = self.list_json(size, &cursor, filters).await?;
            let (page_items, next) = super::api::parse_directory(&json)?;
            has_more = next.is_some();
            if current == page {
                let mut seen = std::collections::HashSet::new();
                items = page_items
                    .into_iter()
                    .filter(|item| seen.insert(item.room_id.clone()))
                    .collect();
            } else {
                match next {
                    Some(next) if next != cursor => cursor = next,
                    _ => {
                        has_more = false;
                        break;
                    }
                }
            }
        }
        Ok(LiveCategoryResult { has_more, items })
    }
}

#[async_trait]
impl crate::platforms::live_site::LiveSite for AcfunSite {
    fn id(&self) -> String {
        "acfun".to_string()
    }

    fn name(&self) -> String {
        "ACFUN".to_string()
    }

    fn set_site_attrs(&self, _data: &str) -> Result<(), SliveError> {
        Err(SliveError::invalid_input("ACFUN 不支持注入凭证"))
    }

    async fn get_categories(&self) -> Result<Vec<LiveCategory>, SliveError> {
        // 单个分类，区域来自一次 `count=1` 的列表请求（不动列表 cursor）。
        let json = self.list_json(1, "", None).await?;
        let children = super::api::parse_areas(&json)?;
        Ok(vec![LiveCategory {
            id: "acfun".to_string(),
            name: "AcFun 直播".to_string(),
            children,
        }])
    }

    async fn get_category_rooms(
        &self,
        category: LiveSubCategory,
        page: i32,
        page_size: Option<i32>,
    ) -> Result<LiveCategoryResult, SliveError> {
        self.directory(page.max(1), page_size, super::api::area_filter(&category).as_deref())
            .await
    }

    async fn get_recommend_rooms(&self, page: i32, page_size: Option<i32>) -> Result<LiveCategoryResult, SliveError> {
        self.directory(page.max(1), page_size, None).await
    }

    async fn search_rooms(&self, keyword: String, page: i32) -> Result<LiveSearchRoomResult, SliveError> {
        let items = self.search_anchors(keyword, page).await?.items;
        Ok(LiveSearchRoomResult {
            has_more: false,
            items: items
                .into_iter()
                .filter(|anchor| anchor.live_status)
                .map(|anchor| LiveRoomItem {
                    room_id: anchor.room_id,
                    title: anchor.user_name.clone(),
                    cover: anchor.avatar,
                    user_name: anchor.user_name,
                    online: 0,
                })
                .collect(),
        })
    }

    async fn search_anchors(&self, keyword: String, page: i32) -> Result<LiveSearchAnchorResult, SliveError> {
        let keyword = keyword.trim();
        if keyword.is_empty() {
            return Ok(LiveSearchAnchorResult {
                has_more: false,
                items: Vec::new(),
            });
        }
        let url = super::api::search_user(keyword, page.max(1));
        let body = self.http.get_text(&url).await?;
        Ok(LiveSearchAnchorResult {
            has_more: false,
            items: super::api::parse_search(&body),
        })
    }

    async fn get_room_detail(&self, room_id: String) -> Result<LiveRoomDetail, SliveError> {
        let json = self.get_room_info(&room_id).await?;
        super::detail::parse_room_info(&json, &room_id)
    }

    async fn get_play_qualities(&self, detail: LiveRoomDetail) -> Result<Vec<LivePlayQuality>, SliveError> {
        if !detail.status {
            return Err(SliveError::Api { code: -1, message: "房间未开播".to_string() });
        }
        
        let json = self.start_play(&detail.room_id).await?;
        super::play::parse_qualities(&json)
    }

    async fn get_play_urls(&self, detail: LiveRoomDetail, quality: LivePlayQuality) -> Result<LivePlayUrl, SliveError> {
        Ok(super::play::get_play_url(&quality, &detail.room_id))
    }

    async fn get_live_status(&self, room_id: String) -> Result<bool, SliveError> {
        let detail = self.get_room_detail(room_id).await?;
        Ok(detail.status)
    }

    async fn get_super_chat_messages(&self, _room_id: String) -> Result<Vec<LiveSuperChatMessage>, SliveError> {
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
