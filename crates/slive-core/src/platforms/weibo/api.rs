//! 微博直播端点与列表解析（对照 PureLive `weibo_api.dart`）。
//!
//! 匿名接口都在 `weibo.com/l/!/2/wblive/` 下：
//! - `pc_recommend/list.json`：官方推荐快照（无分页、无观众数，只有在播场次）；
//! - `room/show_pc_live.json`：单场直播（`{code:100000,error_code:0,data}`）。
//!
//! 微博的「房间」是**一场直播**（`1022:…`），不是主播。

use serde_json::Value;

use crate::error::SliveError;
use crate::models::{LiveCategory, LiveRoomItem, LiveSubCategory};

/// 站点根。
pub const ORIGIN: &str = "https://weibo.com";
/// 推荐快照路径。
pub const RECOMMEND_PATH: &str = "/l/!/2/wblive/pc_recommend/list.json";
/// 单场直播详情路径。
pub const DETAIL_PATH: &str = "/l/!/2/wblive/room/show_pc_live.json";
/// 快照请求条数（站点实际回答约 51 条）。
pub const RECOMMEND_COUNT: i32 = 100;
/// 站点名（分类名同）。
pub const SITE_NAME: &str = "微博直播";
/// 唯一分类 id。
pub const CATEGORY_ID: &str = "weibo";
/// 唯一区域的父类型 / id / 名称。
pub const AREA_TYPE: &str = "recommendation";
pub const AREA_ID: &str = "live";
pub const AREA_NAME: &str = "公开推荐";
/// 直播 / 回放 / 已结束的 `status`。
pub const STATUS_LIVE: i64 = 1;
pub const STATUS_REPLAY: i64 = 3;
pub const STATUS_ENDED: i64 = 5;
/// 详情里不存在场次的错误码。
pub const MISSING_CODE: i64 = 27401;
/// 观看限制的已知值。
pub const LIMIT_APP_ONLY: i64 = 8;
pub const LIMIT_FRIENDS_ONLY: i64 = 10;
pub const LIMIT_SELF_ONLY: i64 = 11;
pub const LIMIT_PAID: i64 = 12;

/// 每场直播都会带的提示：关注的是这一场，不是主播。
pub const ROOM_SCOPE_NOTICE: &str =
    "微博每场直播都有单独的链接，这里关注的是这一场。主播下次开播时，请重新导入新的直播链接。";
/// 受限场次的提示。
pub const RESTRICTED_NOTICE: &str =
    "这场直播设置了观看限制（仅好友、仅主播本人、付费或仅限微博 App），这里无法播放。";
/// 关闭播放的提示。
pub const DISABLED_NOTICE: &str = "这场直播已关闭播放，这里无法播放。";
/// 3.x 的 User-Agent。
pub const USER_AGENT: &str = "Mozilla/5.0";

/// 3.x 的请求头（列表、详情共用）。
pub fn headers() -> Vec<(String, String)> {
    vec![
        ("referer".to_string(), format!("{ORIGIN}/l/wblive/")),
        ("user-agent".to_string(), USER_AGENT.to_string()),
    ]
}

/// 推荐快照请求。
pub fn recommend_url() -> String {
    format!("{ORIGIN}{RECOMMEND_PATH}?count={RECOMMEND_COUNT}&uid=")
}

/// 单场直播详情请求（`live_id` 百分号编码）。
pub fn detail_url(live_id: &str) -> String {
    format!(
        "{ORIGIN}{DETAIL_PATH}?live_id={}",
        crate::util::encode_query_component(live_id)
    )
}

/// 房间分享页。
pub fn room_url(live_id: &str) -> String {
    format!("{ORIGIN}/l/wblive/p/show/{live_id}")
}

/// 场次 id：`3–8 位数字 : 16–48 位字母或数字`。
pub fn is_live_id(id: &str) -> bool {
    let Some((prefix, suffix)) = id.split_once(':') else {
        return false;
    };
    (3..=8).contains(&prefix.len())
        && prefix.bytes().all(|byte| byte.is_ascii_digit())
        && (16..=48).contains(&suffix.len())
        && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// 唯一分类「微博直播 / 公开推荐」。
pub fn categories() -> Vec<LiveCategory> {
    vec![LiveCategory {
        id: CATEGORY_ID.to_string(),
        name: SITE_NAME.to_string(),
        children: vec![LiveSubCategory {
            name: AREA_NAME.to_string(),
            pic: None,
            id: AREA_ID.to_string(),
            parent_id: AREA_TYPE.to_string(),
        }],
    }]
}

/// 是否唯一区域（`parent_id` = 区域类型，`id` = 区域 id）。
pub fn is_area(category: &LiveSubCategory) -> bool {
    category.parent_id.trim() == AREA_TYPE && category.id.trim() == AREA_ID
}

/// 顶层信封校验：`code=100000` 且 `error_code=0`；`27401` 是场次不存在。
pub(crate) fn envelope<'a>(json: &'a Value, what: &str) -> Result<&'a Value, SliveError> {
    let code = json.get("code").and_then(Value::as_i64).unwrap_or(-1);
    let error_code = json.get("error_code").and_then(Value::as_i64).unwrap_or(0);
    if error_code == MISSING_CODE {
        return Err(SliveError::NotFound(format!("{what}: 场次不存在")));
    }
    if code != 100000 || error_code != 0 {
        return Err(SliveError::Api {
            code: error_code as i32,
            message: format!("{what}: code={code} error_code={error_code}"),
        });
    }
    json.get("data").ok_or_else(|| SliveError::Api {
        code: -1,
        message: format!("{what}: 缺少 data"),
    })
}

/// 推荐快照：`data.data[]` 的直播卡片（跳过坏行，不允许整页变空）。
pub fn parse_recommendations(json: &Value) -> Result<Vec<LiveRoomItem>, SliveError> {
    let data = envelope(json, "pc_recommend")?;
    let rows = data
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| SliveError::Api {
            code: -1,
            message: "pc_recommend: data.data 不是数组".to_string(),
        })?;
    if rows.len() > 500 {
        return Err(SliveError::Api {
            code: -1,
            message: format!("pc_recommend: 行数异常 {}", rows.len()),
        });
    }
    let mut seen = std::collections::HashSet::new();
    let mut items = Vec::new();
    for row in rows {
        let Some(id) = row.get("liveid").and_then(Value::as_str) else {
            continue;
        };
        if !is_live_id(id) || !seen.insert(id.to_string()) {
            continue;
        }
        let nick = row
            .get("nickname")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        items.push(LiveRoomItem {
            room_id: id.to_string(),
            title: nick.clone(),
            cover: cover_url(row.get("cover")),
            user_name: nick,
            online: 0,
        });
    }
    if items.is_empty() && !rows.is_empty() {
        return Err(SliveError::Api {
            code: -1,
            message: "pc_recommend: 没有可用的行".to_string(),
        });
    }
    Ok(items)
}

/// 图片地址：去空白、http 升 https。
pub fn cover_url(value: Option<&Value>) -> String {
    let Some(url) = value.and_then(Value::as_str) else {
        return String::new();
    };
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("http://") {
        format!("https://{rest}")
    } else {
        url.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn live_id_shape_matches_the_platform() {
        assert!(is_live_id("1022:2321325347771573207158"));
        assert!(!is_live_id("1022:232132"));
        assert!(!is_live_id("10222321325347771573207158"));
        assert!(!is_live_id("1022:2321-325347771573207158"));
    }

    #[test]
    fn recommendations_skip_bad_rows_and_keep_the_rest() {
        let value = json!({
            "code": 100000,
            "error_code": 0,
            "data": {"data": [
                {"liveid": "1022:2321325347771573207158", "nickname": "A", "cover": "http://cdn/a.jpg"},
                {"liveid": "bad", "nickname": "B"},
                {"liveid": "1022:2321325347771573207158", "nickname": "dup"},
                {"liveid": "1022:2321325347904448757771", "nickname": "C"}
            ]}
        });
        let items = parse_recommendations(&value).expect("快照");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].room_id, "1022:2321325347771573207158");
        assert_eq!(items[0].cover, "https://cdn/a.jpg");
        assert_eq!(items[1].user_name, "C");
    }

    #[test]
    fn missing_broadcast_is_not_found() {
        let value = json!({"code": 100000, "error_code": 27401, "data": null});
        assert!(matches!(
            parse_recommendations(&value),
            Err(SliveError::NotFound(_))
        ));
    }
}
