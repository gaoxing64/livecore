//! 小红书分享页解析（对照 PureLive `xiaohongshu_api.dart:room`）。

use serde_json::Value;

use crate::error::SliveError;
use crate::models::LiveRoomDetail;

use super::api::{self, PullStream};

/// 观看条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restriction {
    /// `monetizeType != 0`。
    Paid,
    /// `joinLimitTypes` 含群聊 / 家人（1、2）。
    Private,
    /// `joinLimitTypes` 含 IP 围栏（4）。
    RegionBlocked,
    /// 其它非 0 限制。
    Other,
}

/// 谁能看。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Public,
    Restricted(Restriction),
    Unknown,
}

/// 分享页解析结果。
#[derive(Debug, Clone)]
pub struct XhsPage {
    pub room: LiveRoomDetail,
    /// `status` 2/3 对应的直播中/已结束；其它状态为 `None`。
    pub live: Option<bool>,
    pub access: Access,
    /// 公开直播的拉流地址。
    pub streams: Vec<PullStream>,
    /// 平台展示的累计观看文本（如「1千+」）。
    pub display_viewers: Option<String>,
}

/// 解析分享页 HTML。
pub fn parse_page(html: &str, requested_id: &str) -> Result<XhsPage, SliveError> {
    if !api::is_room_id(requested_id) {
        return Err(SliveError::NotFound(format!(
            "不是小红书房间 id：{requested_id}"
        )));
    }
    let state = api::extract_live_stream(html)?;
    let page_status = state
        .get("pageStatus")
        .and_then(Value::as_str)
        .unwrap_or("");
    if page_status == "error" {
        let message = state
            .get("errorMessage")
            .and_then(Value::as_str)
            .unwrap_or("pageStatus error");
        return Err(SliveError::NotFound(format!("分享页 {requested_id}: {message}")));
    }
    if page_status != "success" {
        return Err(SliveError::Api {
            code: -1,
            message: format!("分享页 pageStatus {page_status:?}"),
        });
    }

    let room_data = object(state.get("roomData"), "roomData")?;
    let info = object(room_data.get("roomInfo"), "roomInfo")?;
    let host = object(room_data.get("hostInfo"), "hostInfo")?;

    let response_id = info.get("roomId").and_then(Value::as_str);
    if let Some(response_id) = response_id {
        if response_id != requested_id {
            return Err(SliveError::Api {
                code: -1,
                message: format!("分享页请求 {requested_id}，回答 {response_id}"),
            });
        }
    }

    let code = integer(info.get("status")).ok_or_else(|| SliveError::Api {
        code: -1,
        message: "分享页缺少 status".to_string(),
    })?;
    let live_status = state.get("liveStatus").and_then(Value::as_str);
    let live = match code {
        2 => Some(true),
        3 => Some(false),
        _ => None,
    };
    if (live == Some(true) && live_status != Some("success"))
        || (live == Some(false) && live_status != Some("end"))
    {
        return Err(SliveError::Api {
            code: -1,
            message: format!("分享页 status {code} 与 liveStatus {live_status:?} 不一致"),
        });
    }
    if live == Some(true) && response_id.is_none() {
        return Err(SliveError::Api {
            code: -1,
            message: "分享页直播中没有 roomId".to_string(),
        });
    }

    let access = parse_access(info.get("monetizeType"), info.get("joinLimitTypes"))?;
    let pull_config = if live == Some(true) && access == Access::Public {
        info.get("pullConfig")
    } else {
        None
    };
    let streams = api::parse_pull_config(pull_config, requested_id)?;

    let title = text(info.get("roomTitle"));
    let nick = text(host.get("nickName"));
    let cover = text(info.get("roomCover"));
    let avatar = text(host.get("avatar"));
    let display_viewers = info
        .get("displayViewerCount")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|text| !text.is_empty());

    let mut notices = vec![api::ROOM_SCOPE_NOTICE.to_string()];
    match access {
        Access::Restricted(_) => notices.push(api::RESTRICTED_NOTICE.to_string()),
        Access::Unknown => notices.push(api::UNKNOWN_ACCESS_NOTICE.to_string()),
        Access::Public => {}
    }
    if let Some(viewers) = display_viewers.as_deref() {
        notices.push(format!(
            "小红书显示 {viewers} 人看过（累计的约数，不是在线人数）"
        ));
    }

    let room = LiveRoomDetail {
        room_id: requested_id.to_string(),
        title,
        cover,
        user_name: nick,
        user_avatar: avatar,
        online: 0,
        introduction: None,
        notice: Some(notices.join("\n")),
        status: live == Some(true),
        data: Some(
            serde_json::json!({
                "live": live,
                "access": match access {
                    Access::Public => "public",
                    Access::Unknown => "unknown",
                    Access::Restricted(_) => "restricted",
                },
                "restriction": match access {
                    Access::Restricted(Restriction::Paid) => Some("paid"),
                    Access::Restricted(Restriction::Private) => Some("private"),
                    Access::Restricted(Restriction::RegionBlocked) => Some("region"),
                    Access::Restricted(Restriction::Other) => Some("other"),
                    _ => None,
                },
                "pull_config": pull_config.map(|value| value.to_string()),
                "streams": streams
                    .iter()
                    .map(|stream| serde_json::json!({
                        "url": stream.url,
                        "codec": stream.codec,
                        "quality": stream.quality,
                        "label": stream.label,
                    }))
                    .collect::<Vec<_>>(),
            })
            .to_string(),
        ),
        danmaku_data: None,
        is_record: false,
        url: api::room_url(requested_id),
    };

    Ok(XhsPage {
        room,
        live,
        access,
        streams,
        display_viewers,
    })
}

/// `monetizeType` + `joinLimitTypes` → 观看条件。
fn parse_access(monetization: Option<&Value>, limits: Option<&Value>) -> Result<Access, SliveError> {
    let paid = match monetization.filter(|value| !value.is_null()) {
        Some(value) => Some(integer(Some(value)).ok_or_else(|| SliveError::Api {
            code: -1,
            message: format!("分享页 monetizeType 非法：{value}"),
        })?),
        None => None,
    };
    let kinds = match limits.filter(|value| !value.is_null()) {
        Some(Value::String(text)) => {
            let decoded: Value = serde_json::from_str(text).map_err(|error| SliveError::Api {
                code: -1,
                message: format!("分享页 joinLimitTypes 非法：{error}"),
            })?;
            parse_kinds(&decoded)?
        }
        Some(value) => parse_kinds(value)?,
        None => None,
    };
    let (Some(paid), Some(kinds)) = (paid, kinds) else {
        return Ok(Access::Unknown);
    };
    if paid != 0 {
        return Ok(Access::Restricted(Restriction::Paid));
    }
    if kinds.contains(&1) || kinds.contains(&2) {
        return Ok(Access::Restricted(Restriction::Private));
    }
    if kinds.contains(&4) {
        return Ok(Access::Restricted(Restriction::RegionBlocked));
    }
    if kinds.iter().any(|kind| *kind != 0) {
        return Ok(Access::Restricted(Restriction::Other));
    }
    Ok(Access::Public)
}

fn parse_kinds(value: &Value) -> Result<Option<Vec<i64>>, SliveError> {
    let Some(list) = value.as_array() else {
        return Err(SliveError::Api {
            code: -1,
            message: format!("分享页 joinLimitTypes 不是数组：{value}"),
        });
    };
    if list.len() > 16 {
        return Err(SliveError::Api {
            code: -1,
            message: "分享页 joinLimitTypes 过长".to_string(),
        });
    }
    let kinds = list
        .iter()
        .map(|item| {
            integer(Some(item)).ok_or_else(|| SliveError::Api {
                code: -1,
                message: format!("分享页 joinLimitTypes 项非法：{item}"),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(kinds))
}

fn object<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a Value, SliveError> {
    value.filter(|value| value.is_object()).ok_or_else(|| SliveError::Api {
        code: -1,
        message: format!("分享页缺少 {field}"),
    })
}

fn text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// 整型（也接受 `"2"` 这样的字符串）。
fn integer(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page(info: Value, live_status: &str) -> String {
        let state = json!({
            "liveStream": {
                "pageStatus": "success",
                "liveStatus": live_status,
                "roomData": {
                    "roomInfo": info,
                    "hostInfo": {"nickName": "宿主", "avatar": "https://cdn/a.jpg"}
                }
            }
        });
        format!(
            "<script>window.__INITIAL_STATE__={};</script>",
            state
        )
    }

    fn public_info() -> Value {
        json!({
            "roomId": "570541076495019830",
            "status": 2,
            "monetizeType": 0,
            "joinLimitTypes": [0],
            "roomTitle": "标题",
            "roomCover": "https://cdn/c.jpg",
            "displayViewerCount": "1千+",
            "pullConfig": "{\"h264\":[{\"master_url\":\"http://live-source-play.xhscdn.com/live/570541076495019830.m3u8\",\"quality_type\":\"HD\",\"quality_type_name\":\"原画\"}]}"
        })
    }

    #[test]
    fn public_live_reads_streams_and_viewer_text() {
        let html = page(public_info(), "success");
        let page = parse_page(&html, "570541076495019830").expect("解析");
        assert_eq!(page.live, Some(true));
        assert_eq!(page.access, Access::Public);
        assert_eq!(page.streams.len(), 1);
        assert!(page.room.status);
        assert!(page
            .room
            .notice
            .unwrap_or_default()
            .contains("1千+ 人看过"));
    }

    #[test]
    fn ended_page_is_offline() {
        let info = json!({"roomId": "570541076495019830", "status": 3, "monetizeType": 0, "joinLimitTypes": [0]});
        let html = page(info, "end");
        let page = parse_page(&html, "570541076495019830").expect("解析");
        assert_eq!(page.live, Some(false));
        assert!(!page.room.status);
        assert!(page.streams.is_empty());
    }

    #[test]
    fn restrictions_are_classified() {
        let paid = page(
            json!({"roomId": "1", "status": 2, "monetizeType": 1, "joinLimitTypes": [0]}),
            "success",
        );
        assert_eq!(
            parse_page(&paid, "1").expect("解析").access,
            Access::Restricted(Restriction::Paid)
        );

        let private = page(
            json!({"roomId": "1", "status": 2, "monetizeType": 0, "joinLimitTypes": "[0,1]"}),
            "success",
        );
        assert_eq!(
            parse_page(&private, "1").expect("解析").access,
            Access::Restricted(Restriction::Private)
        );

        let unknown = page(json!({"roomId": "1", "status": 2}), "success");
        assert_eq!(parse_page(&unknown, "1").expect("解析").access, Access::Unknown);
    }

    #[test]
    fn mismatched_room_is_an_error() {
        let html = page(public_info(), "success");
        assert!(matches!(
            parse_page(&html, "123"),
            Err(SliveError::Api { .. })
        ));
    }
}
