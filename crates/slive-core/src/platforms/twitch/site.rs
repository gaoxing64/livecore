//! Twitch 站点句柄：句柄级状态 + 16 方法编排（`site.rs` 仅编排，能力下沉到各模块）。
//!
//! 状态：
//! - `device_id`：Dart 每次请求用 `Random()` 重新生成（`getDeviceId()`）；Rust 落为
//!   **句柄级**（同一会话稳定），值由注入的 [`TimeSource`] 派生 —— 请求头集合与 Dart 一致，
//!   属行为不变（`13-spec.md §10` 已有同类裁决：wbi keys 静态 -> 句柄级）；
//! - 列表快照 / 搜索 cursor：PureLive 的列表接口用 cursor 翻页，而 core 契约是 `page`，
//!   故把快照与 cursor 落在句柄级（对照 `twitch_site.dart` 的 `_snapshots` / `_cursors`）；
//! - 注入的 [`HttpTransport`]（usher m3u8 是 GET）、[`TwitchPost`]（GQL 是 POST JSON）与
//!   [`TimeSource`] 使离线夹具与确定性断言可行。
//!
//! 依赖方向：本模块只向下依赖 `super::{catalog,detail,play,rooms,search,unsupported,transport,header}`
//! 与 `crate::{error,http,models,time}`。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::error::SliveError;
use crate::http::{HttpRequest, HttpTransport, ReqwestTransport};
use crate::models::{
    LiveCategory, LiveCategoryResult, LivePlayQuality, LivePlayUrl, LiveRoomDetail, LiveSearchAnchorResult,
    LiveSearchRoomResult, LiveSubCategory, LiveSuperChatMessage,
};
use crate::time::{SystemClock, TimeSource};

use super::header::{
    request_header, request_header_with_language, DEVICE_ID_BASE, DEVICE_ID_SPAN,
    TWITCH_ACCEPT_LANGUAGE, TWITCH_GQL_URL,
};
use super::rooms::{ListSnapshot, SNAPSHOT_LIMIT};
use super::transport::{PostRequest, ReqwestTwitchPost, TwitchPost};

/// 句柄级可变状态。
#[derive(Default)]
struct SiteState {
    device_id: String,
    snapshots: HashMap<String, ListSnapshot>,
    cursors: HashMap<String, String>,
}

/// Twitch 站点实例。
pub struct TwitchSite {
    http: Arc<dyn HttpTransport>,
    gql: Arc<dyn TwitchPost>,
    time: Arc<dyn TimeSource>,
    state: Mutex<SiteState>,
}

impl Default for TwitchSite {
    fn default() -> Self {
        Self::new()
    }
}

impl TwitchSite {
    /// 生产构造：真实传输 + 系统时钟。
    pub fn new() -> Self {
        Self::with_deps(
            Arc::new(ReqwestTransport::new()),
            Arc::new(ReqwestTwitchPost::new()),
            Arc::new(SystemClock),
        )
    }

    /// 依赖注入构造（供测试注入 `MockTransport` / `MockTwitchPost` / `FakeTime`）。
    pub fn with_deps(http: Arc<dyn HttpTransport>, gql: Arc<dyn TwitchPost>, time: Arc<dyn TimeSource>) -> Self {
        Self {
            http,
            gql,
            time,
            state: Mutex::new(SiteState::default()),
        }
    }

    /// 站点 ID。
    pub fn id(&self) -> String {
        "twitch".to_string()
    }

    /// 站点名。
    pub fn name(&self) -> String {
        "Twitch".to_string()
    }

    /// 注入的 HTTP 传输（GET 通道）。
    pub fn http(&self) -> Arc<dyn HttpTransport> {
        Arc::clone(&self.http)
    }

    /// 注入的 GQL POST 传输。
    pub fn gql(&self) -> Arc<dyn TwitchPost> {
        Arc::clone(&self.gql)
    }

    /// 注入的时间源。
    pub fn time(&self) -> Arc<dyn TimeSource> {
        Arc::clone(&self.time)
    }

    /// 当前 Unix 秒。
    pub fn now_secs(&self) -> u64 {
        self.time.unix_secs()
    }

    /// 句柄级 `device-id`（首次访问时由时间源派生并缓存）。
    pub fn device_id(&self) -> String {
        {
            let state = self.lock();
            if !state.device_id.is_empty() {
                return state.device_id.clone();
            }
        }
        let derived = (DEVICE_ID_BASE + self.time.unix_secs() % DEVICE_ID_SPAN).to_string();
        let mut state = self.lock();
        if state.device_id.is_empty() {
            state.device_id = derived.clone();
        }
        state.device_id.clone()
    }

    /// 覆盖 `device-id`（测试/未来注入用）。
    pub fn set_device_id(&self, device_id: &str) {
        self.lock().device_id = device_id.to_string();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SiteState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 携带标准头 POST 一条 GQL 请求并解析 JSON（旧链路的 en-US）。
    pub(crate) async fn post_gql(&self, body: &Value) -> Result<Value, SliveError> {
        self.post_gql_with_language(body, TWITCH_ACCEPT_LANGUAGE).await
    }

    /// 指定 `Accept-Language` 的 GQL POST（列表能力用 zh-CN 拿中文名称）。
    pub(crate) async fn post_gql_with_language(
        &self,
        body: &Value,
        language: &str,
    ) -> Result<Value, SliveError> {
        let payload =
            serde_json::to_string(body).map_err(|e| SliveError::decode(format!("序列化 GQL 请求失败: {e}")))?;
        let reply = self
            .gql()
            .post_json(&PostRequest {
                url: TWITCH_GQL_URL.to_string(),
                body: payload,
                headers: request_header_with_language(self, language),
            })
            .await?;
        reply.json()
    }

    /// 取列表快照（克隆，避免跨 await 持有锁）。
    pub(crate) fn list_snapshot(&self, key: &str) -> Option<ListSnapshot> {
        self.lock().snapshots.get(key).cloned()
    }

    /// 写回列表快照，超过上限时丢弃最旧的一份（对照 PureLive `_snapshotLimit`）。
    pub(crate) fn save_snapshot(&self, key: &str, snapshot: ListSnapshot) {
        let mut state = self.lock();
        state.snapshots.insert(key.to_string(), snapshot);
        if state.snapshots.len() > SNAPSHOT_LIMIT {
            let oldest = state
                .snapshots
                .iter()
                .min_by_key(|(_, snapshot)| snapshot.fetched_at)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                state.snapshots.remove(&oldest);
            }
        }
    }

    /// 搜索翻页 cursor（对照 PureLive `_cursors`）。
    pub(crate) fn cursor_for(&self, key: &str) -> Option<String> {
        self.lock().cursors.get(key).cloned()
    }

    /// 记住 / 清除下一页 cursor（对照 PureLive `_remember`）。
    pub(crate) fn remember_cursor(&self, key: &str, cursor: Option<&str>) {
        const CURSOR_LIMIT: usize = 256;
        let mut state = self.lock();
        state.cursors.remove(key);
        if let Some(cursor) = cursor {
            state.cursors.insert(key.to_string(), cursor.to_string());
        }
        if state.cursors.len() > CURSOR_LIMIT {
            let oldest = state.cursors.keys().next().cloned();
            if let Some(oldest) = oldest {
                state.cursors.remove(&oldest);
            }
        }
    }

    /// 带标准头发起 GET 并返回响应文本（usher m3u8 用）。
    pub(crate) async fn get_text(&self, url: &str, query: Vec<(String, String)>) -> Result<String, SliveError> {
        let request = HttpRequest {
            url: url.to_string(),
            query,
            headers: request_header(self),
        };
        Ok(self.http().get(&request).await?.text)
    }

    /// F-01 分类：标签 + 标签下的目录（PureLive `twitch_site.dart:273-294`）。
    pub async fn get_categories(&self) -> Result<Vec<LiveCategory>, SliveError> {
        super::catalog::fetch(self).await
    }

    /// F-02 分类下房间：`DirectoryPage_Game` + 快照分页。
    pub async fn get_category_rooms(
        &self,
        category: LiveSubCategory,
        page: i32,
        page_size: Option<i32>,
    ) -> Result<LiveCategoryResult, SliveError> {
        super::rooms::fetch_category_rooms(self, &category, page, page_size).await
    }

    /// F-03 首页推荐：全站最热直播 + 快照分页。
    pub async fn get_recommend_rooms(
        &self,
        page: i32,
        page_size: Option<i32>,
    ) -> Result<LiveCategoryResult, SliveError> {
        super::rooms::fetch_recommend_rooms(self, page, page_size).await
    }

    /// F-04 搜索房间：只返回在播频道。
    pub async fn search_rooms(&self, keyword: String, page: i32) -> Result<LiveSearchRoomResult, SliveError> {
        super::search::fetch_rooms(self, &keyword, page).await
    }

    /// F-05 搜索主播：包含未开播频道。
    pub async fn search_anchors(&self, keyword: String, page: i32) -> Result<LiveSearchAnchorResult, SliveError> {
        super::search::fetch_anchors(self, &keyword, page).await
    }

    /// F-06 房间详情（`ChannelShell` + `StreamMetadata` 一次批量）。
    pub async fn get_room_detail(&self, room_id: String) -> Result<LiveRoomDetail, SliveError> {
        super::detail::fetch(self, &room_id).await
    }

    /// F-07 清晰度列表（GQL token + usher m3u8 解析）。
    pub async fn get_play_qualities(&self, detail: LiveRoomDetail) -> Result<Vec<LivePlayQuality>, SliveError> {
        super::play::fetch_qualities(self, &detail).await
    }

    /// F-08 播放地址（`quality.data` 已是线路 URL）。
    pub async fn get_play_urls(
        &self,
        detail: LiveRoomDetail,
        quality: LivePlayQuality,
    ) -> Result<LivePlayUrl, SliveError> {
        super::play::fetch_urls(self, &detail, &quality)
    }

    /// F-09 直播状态（复用详情链路的 `status`，对齐 Dart）。
    pub async fn get_live_status(&self, room_id: String) -> Result<bool, SliveError> {
        Ok(self.get_room_detail(room_id).await?.status)
    }

    /// F-10 SC 列表（恒空）。
    pub async fn get_super_chat_messages(&self, room_id: String) -> Result<Vec<LiveSuperChatMessage>, SliveError> {
        super::unsupported::super_chat(&room_id)
    }

    /// F-17 注入凭证（Twitch 匿名链路不需要 -> 报错）。
    pub fn set_site_attrs(&self, data: &str) -> Result<(), SliveError> {
        super::unsupported::set_site_attrs(data)
    }

    /// F-18 二维码（Dart 未实现 -> 报错）。
    pub async fn get_qr_code(&self) -> Result<crate::models::QrCode, SliveError> {
        super::unsupported::qr_code()
    }

    /// F-19 扫码轮询（Dart 未实现 -> 报错）。
    pub async fn get_qr_poll_result(&self, qrcode_key: String) -> Result<crate::models::QrPollResult, SliveError> {
        super::unsupported::qr_poll_result(&qrcode_key)
    }

    /// F-20 账号信息（Dart 未实现 -> 报错）。
    pub async fn get_account_info(&self, credential: String) -> Result<crate::models::AccountInfo, SliveError> {
        super::unsupported::account_info(&credential)
    }
}

#[cfg(test)]
#[path = "site_tests.rs"]
mod tests;

use crate::platforms::macros::impl_live_site;

impl_live_site!(TwitchSite);
