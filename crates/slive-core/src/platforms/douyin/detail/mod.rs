//! 房间详情（对照 Dart `getRoomDetail` 全家桶，`douyin_site.dart:494-946`）。
//!
//! # 两种房间号
//!
//! - `webRid`：固定，11~12 位（如 `416144012050`），首页/推荐下发的就是它；
//! - `roomId`：一次性，19 位（如 `7376429659866598196`），主播每次重开播都会换。
//!
//! Dart 用**长度 ≤ 16** 判定是哪个（`douyin_site.dart:502`），此处照抄。
//!
//! # 三段降级（少一段就有房间进不去）
//! ```text
//! roomId(19) ──> reflow/info ──┐
//!                              ├─> 房间数据 ──> LiveRoomDetail
//! webRid(≤16) ─> web/enter ────┤
//!                └(失败)─> 页面 HTML ─┘
//! ```
//! 关键修补点（照抄 Dart 的实测结论，勿「简化」）：
//! - 19 位 `roomId` **必须先走 `reflow/info`**，不能先转 webRid 再走 `web/enter`：
//!   连麦 / 多人连线房间下 `web/enter` 会返回「降级房间」（只剩 22 个字段的骨架，
//!   `status != 2` 且无 `stream_url`），后续取画质会直接崩，直播间根本进不去；
//! - `web/enter` 返回的 `stream_url` 为空时，用它给的 `roomId` **整体采信**
//!   `reflow/info` 的结果（首页 / 推荐 / 关注列表下发的都是 webRid，全走这条链路，
//!   不做这个补救则连麦房间会被判成「未开播」）。

mod builder;
mod enter;
mod html;
mod reflow;

pub use enter::enter_params;
pub use reflow::reflow_params;

use crate::error::SliveError;
use crate::models::LiveRoomDetail;

use super::site::DouyinSite;

/// 直播间页面 URL 前缀。
pub const ROOM_PAGE_URL: &str = "https://live.douyin.com/";
/// `roomId.length <= 16` 判定为 webRid（Dart `douyin_site.dart:502`）。
pub const WEB_RID_MAX_LEN: usize = 16;

/// 房间详情入口：按房间号长度分派。
pub(crate) async fn fetch(site: &DouyinSite, room_id: &str) -> Result<LiveRoomDetail, SliveError> {
    if room_id.chars().count() <= WEB_RID_MAX_LEN {
        return fetch_by_web_rid(site, room_id).await;
    }
    // 19 位 roomId 优先 reflow/info。
    // 只把「网络抖动 / 反爬拦截」这类意外兜底到 webRid 链路：刻意抛出的业务错误
    // （房间数据异常）直接上抛，避免多打 2~3 次重复请求。
    match reflow::fetch_by_room_id(site, room_id).await {
        Ok(detail) => Ok(detail),
        Err(e) if is_deliberate(&e) => Err(e),
        Err(e) => {
            log::warn!("抖音 reflow/info 链路失败，回退 webRid 链路: {e}");
            let web_rid = reflow::resolve_web_rid(site, room_id).await;
            fetch_by_web_rid(site, &web_rid).await
        }
    }
}

/// 原画档在服务端的显示名（`options.qualities` 分支）。
const ORIGIN_QUALITY: &str = "原画";

/// 是否 origin 档：显示名或旧地址表键名都可能出现。
fn is_origin_quality(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    name == ORIGIN_QUALITY || matches!(lower.as_str(), "origin" | "origion" | "or4")
}

/// 详情里的清晰度列表是否含 origin 档。
fn has_origin_quality(site: &DouyinSite, detail: &LiveRoomDetail) -> bool {
    super::play::fetch_qualities(site, detail)
        .map(|list| list.iter().any(|item| is_origin_quality(&item.quality)))
        .unwrap_or(false)
}

/// 该响应是否**证明**原画档存在但取不到地址（而不是房间本来就没有 origin）。
///
/// 判据只有两条：
/// 1. `options.qualities` 里明确有 origin 档（`sdk_key`/`name`），解析却没产出；
/// 2. 有档位清单却没有 `stream_data`（非 JSON/空串），只能退回不含 origin 的旧地址表。
///
/// 真正的 4 档房间（有合法 `stream_data` 但只有 ld/sd/hd/uhd）不满足任一条件，
/// 因此不会为了补一档不存在的画质而多打请求。
fn ladder_is_incomplete(_site: &DouyinSite, detail: &LiveRoomDetail) -> bool {
    let Some(raw) = detail.data.as_deref() else {
        return false;
    };
    let Some(data) = super::util::safe_json_decode(raw) else {
        return false;
    };
    let Some(pull) = data.pointer("/live_core_sdk_data/pull_data") else {
        return false;
    };
    let qualities = pull
        .pointer("/options/qualities")
        .and_then(serde_json::Value::as_array)
        .filter(|list| !list.is_empty());
    let Some(qualities) = qualities else {
        return false;
    };
    let declares_origin = qualities.iter().any(|item| {
        item.get("sdk_key")
            .map(crate::util::value_to_string)
            .is_some_and(|key| is_origin_quality(key.as_str()))
            || item
                .get("name")
                .map(crate::util::value_to_string)
                .is_some_and(|name| is_origin_quality(name.as_str()))
    });
    if declares_origin {
        return true;
    }
    let stream_data = pull
        .get("stream_data")
        .map(crate::util::value_to_string)
        .unwrap_or_default();
    !stream_data.trim_start().starts_with('{')
}

/// `web/enter` 缺原画且响应确有残缺时，用 `reflow/info` 补一次完整阶梯。
///
/// 常见失败形态：`options.qualities` 仍有 origin，但本次没带 `stream_data`，
/// 旧地址表又没有 origin 地址，解析出来只剩 4 档；网页刷新后仍显示原画。
/// 只在满足 [`ladder_is_incomplete`] 时才多打这一次请求，正常房间零额外开销。
pub(crate) async fn ensure_origin_quality(
    site: &DouyinSite,
    detail: LiveRoomDetail,
    web_rid: &str,
    room_id: &str,
) -> LiveRoomDetail {
    if !detail.status
        || room_id.is_empty()
        || has_origin_quality(site, &detail)
        || !ladder_is_incomplete(site, &detail)
    {
        return detail;
    }
    log::info!("抖音详情缺少原画档（room_id={room_id}），尝试 reflow/info 补全");
    let Some(reflow_room) = reflow::fetch_reflow_room(site, room_id).await else {
        return detail;
    };
    let candidate = builder::build_detail_from_reflow_room(site, &reflow_room, room_id, web_rid);
    if candidate.status && has_origin_quality(site, &candidate) {
        log::info!("抖音 reflow/info 补全原画档成功（room_id={room_id}）");
        candidate
    } else {
        log::warn!("抖音 reflow/info 同样缺少原画档（room_id={room_id}），保留原结果");
        detail
    }
}

/// 该错误是否是「刻意抛出的业务错误」（值得直接上抛，不该再打重复请求）。
///
/// Dart 的判据是 `if (e is CoreError) rethrow` —— 即凡是被显式包装成 `CoreError` 的
/// （房间数据异常 / 未返回房间数据）都不兜底。这里对应 `Decode` / `NotFound` /
/// `InvalidInput`；`Network` / `Http` / `Api` 属「意外」，走兜底链路。
fn is_deliberate(err: &SliveError) -> bool {
    matches!(
        err,
        SliveError::Decode(_) | SliveError::NotFound(_) | SliveError::InvalidInput(_)
    )
}

/// 通过 webRid 获取详情：先 API，失败再走页面 HTML。
pub(crate) async fn fetch_by_web_rid(
    site: &DouyinSite,
    web_rid: &str,
) -> Result<LiveRoomDetail, SliveError> {
    match enter::fetch_by_web_rid_api(site, web_rid).await {
        Ok(detail) => Ok(detail),
        Err(e) => {
            log::warn!("抖音 web/enter 链路失败，回退页面 HTML: {e}");
            html::fetch_by_web_rid_html(site, web_rid).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_deliberate_separates_business_errors_from_transport() {
        assert!(is_deliberate(&SliveError::decode("x")));
        assert!(is_deliberate(&SliveError::not_found("x")));
        assert!(is_deliberate(&SliveError::invalid_input("x")));
        assert!(!is_deliberate(&SliveError::network("x")));
        assert!(!is_deliberate(&SliveError::Http { status: 403, body: String::new() }));
        assert!(!is_deliberate(&SliveError::Api { code: -352, message: String::new() }));
    }

    #[test]
    fn web_rid_length_threshold_matches_dart() {
        // Dart: roomId.length <= 16 视为 webRid
        assert!("416144012050".len() <= WEB_RID_MAX_LEN);
        assert!("7376429659866598196".len() > WEB_RID_MAX_LEN);
    }
}
