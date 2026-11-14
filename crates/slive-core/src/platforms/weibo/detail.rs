//! 微博单场直播解析（对照 PureLive `weibo_api.dart:detail`）。

use serde_json::Value;

use crate::error::SliveError;
use crate::models::LiveRoomDetail;

use super::api;

/// 观看限制。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// `play_switch != 0` 且 `watch_limit == 0`。
    Public,
    /// `watch_limit` 非 0。
    Restricted,
    /// `play_switch == 0`。
    Disabled,
}

/// 一场直播的详情与其播放信息。
#[derive(Debug, Clone)]
pub struct WeiboBroadcast {
    /// 解析出的房间详情。
    pub room: LiveRoomDetail,
    /// `status`：1 直播、3 回放、5 已结束。
    pub status: i64,
    /// `watch_limit`。
    pub watch_limit: i64,
    /// 是否公开可播。
    pub access: Access,
    /// 直播媒体地址（FLV 优先，HLS 通常与 FLV 相同）。
    pub media_urls: Vec<String>,
    /// 回放地址（HLS VOD，http 升 https）。
    pub replay_url: Option<String>,
    /// 受限时平台给的说明。
    pub tip: Option<String>,
}

/// 解析 `show_pc_live` 应答。
pub fn parse_detail(json: &Value, requested_id: &str) -> Result<WeiboBroadcast, SliveError> {
    let data = api::envelope(json, "show_pc_live")?;

    let live_id = data
        .get("liveId")
        .and_then(Value::as_str)
        .ok_or_else(|| SliveError::Api {
            code: -1,
            message: "show_pc_live: 缺少 liveId".to_string(),
        })?;
    if live_id != requested_id {
        return Err(SliveError::Api {
            code: -1,
            message: format!("show_pc_live: 请求 {requested_id}，回答 {live_id}"),
        });
    }

    let user = data.get("user").ok_or_else(|| SliveError::Api {
        code: -1,
        message: "show_pc_live: 缺少 user".to_string(),
    })?;
    let uid = user
        .get("uid")
        .map(crate::util::value_to_string)
        .filter(|value| !value.is_empty() && value != "null")
        .ok_or_else(|| SliveError::Api {
            code: -1,
            message: "show_pc_live: 缺少 user.uid".to_string(),
        })?;

    let status = data.get("status").and_then(Value::as_i64).ok_or_else(|| {
        SliveError::Api {
            code: -1,
            message: "show_pc_live: 缺少 status".to_string(),
        }
    })?;
    let watch_limit = data
        .get("watch_limit")
        .and_then(Value::as_i64)
        .ok_or_else(|| SliveError::Api {
            code: -1,
            message: "show_pc_live: 缺少 watch_limit".to_string(),
        })?;
    let play_switch = data
        .get("play_switch")
        .and_then(Value::as_i64)
        .ok_or_else(|| SliveError::Api {
            code: -1,
            message: "show_pc_live: 缺少 play_switch".to_string(),
        })?;
    let access = if play_switch == 0 {
        Access::Disabled
    } else if watch_limit != 0 {
        Access::Restricted
    } else {
        Access::Public
    };

    let mut media_urls: Vec<String> = Vec::new();
    if access == Access::Public && status == api::STATUS_LIVE {
        for key in ["live_origin_flv_url", "live_origin_hls_url"] {
            if let Some(url) = media_url(data.get(key)) {
                if !media_urls.contains(&url) {
                    media_urls.push(url);
                }
            }
        }
    }
    let replay_url = if access == Access::Public && status == api::STATUS_REPLAY {
        replay_url(data.get("replay_origin_url"))
    } else {
        None
    };
    let tip = if access == Access::Restricted {
        data.pointer("/pay_dialog_info/buy_tip")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    } else {
        None
    };

    let title = data
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let screen_name = user
        .get("screenName")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let avatar = api::cover_url(user.get("avatar"));
    let avatar = if avatar.is_empty() {
        api::cover_url(user.get("profileImageUrl"))
    } else {
        avatar
    };

    let notice = match access {
        Access::Restricted => format!("{}\n{}", api::RESTRICTED_NOTICE, api::ROOM_SCOPE_NOTICE),
        Access::Disabled => format!("{}\n{}", api::DISABLED_NOTICE, api::ROOM_SCOPE_NOTICE),
        Access::Public => api::ROOM_SCOPE_NOTICE.to_string(),
    };

    let room = LiveRoomDetail {
        room_id: requested_id.to_string(),
        title,
        cover: api::cover_url(data.get("cover")),
        user_name: screen_name,
        user_avatar: avatar,
        online: 0,
        introduction: None,
        notice: Some(notice),
        status: status == api::STATUS_LIVE,
        data: Some(
            serde_json::json!({
                "owner_id": uid,
                "status": status,
                "watch_limit": watch_limit,
            })
            .to_string(),
        ),
        danmaku_data: None,
        is_record: status == api::STATUS_REPLAY,
        url: api::room_url(requested_id),
    };

    Ok(WeiboBroadcast {
        room,
        status,
        watch_limit,
        access,
        media_urls,
        replay_url,
        tip,
    })
}

/// 平台直播地址：只接受 http(s)。
fn media_url(value: Option<&Value>) -> Option<String> {
    let url = value?.as_str()?.trim();
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url.to_string())
    } else {
        None
    }
}

/// 回放地址：`replay_origin_url`，http 升 https。
fn replay_url(value: Option<&Value>) -> Option<String> {
    let url = value?.as_str()?.trim();
    if let Some(rest) = url.strip_prefix("http://") {
        Some(format!("https://{rest}"))
    } else if url.starts_with("https://") {
        Some(url.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "1022:2321325347771573207158";

    fn detail(extra: serde_json::Value) -> Value {
        let mut data = json!({
            "liveId": ID,
            "status": 1,
            "watch_limit": 0,
            "play_switch": 1,
            "title": "标题",
            "cover": "https://cdn/cover.jpg",
            "user": {"uid": 42, "screenName": "主播", "avatar": "https://cdn/a.jpg"},
            "live_origin_flv_url": "https://cdn/live.flv",
            "live_origin_hls_url": "https://cdn/live.flv"
        });
        if let Some(extra) = extra.as_object() {
            for (key, value) in extra {
                data[key] = value.clone();
            }
        }
        json!({"code": 100000, "error_code": 0, "data": data})
    }

    #[test]
    fn public_live_reads_one_media_url() {
        let parsed = parse_detail(&detail(json!({})), ID).expect("详情");
        assert_eq!(parsed.status, 1);
        assert_eq!(parsed.access, Access::Public);
        assert_eq!(parsed.media_urls, vec!["https://cdn/live.flv"]);
        assert!(parsed.room.status);
        assert!(parsed.room.notice.unwrap_or_default().contains("微博每场直播"));
    }

    #[test]
    fn restricted_and_disabled_carry_notices() {
        let restricted = parse_detail(&detail(json!({"watch_limit": 10, "pay_dialog_info": {"buy_tip": "仅好友"}})), ID)
            .expect("受限");
        assert_eq!(restricted.access, Access::Restricted);
        assert!(restricted.media_urls.is_empty());
        assert_eq!(restricted.tip.as_deref(), Some("仅好友"));
        assert!(restricted
            .room
            .notice
            .unwrap_or_default()
            .contains("观看限制"));

        let disabled = parse_detail(&detail(json!({"play_switch": 0})), ID).expect("关闭");
        assert_eq!(disabled.access, Access::Disabled);
        assert!(disabled
            .room
            .notice
            .unwrap_or_default()
            .contains("已关闭播放"));
    }

    #[test]
    fn replay_reads_its_recording() {
        let parsed = parse_detail(
            &detail(json!({"status": 3, "replay_origin_url": "http://cdn/replay.m3u8"})),
            ID,
        )
        .expect("回放");
        assert_eq!(parsed.status, 3);
        assert!(!parsed.room.status);
        assert!(parsed.room.is_record);
        assert_eq!(parsed.replay_url.as_deref(), Some("https://cdn/replay.m3u8"));
    }

    #[test]
    fn another_broadcast_in_the_answer_is_an_error() {
        let err = parse_detail(&detail(json!({"liveId": "1022:2321325347904448757771"})), ID)
            .expect_err("id 不符");
        assert!(matches!(err, SliveError::Api { .. }));
    }
}
