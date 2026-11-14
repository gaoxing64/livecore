//! ACFUN 房间详情解析（对照 PureLive `acfun_api.dart:344-389`）。

use serde_json::Value;
use crate::error::SliveError;
use crate::models::*;

/// 从 live/info API 响应解析房间详情。
/// JSON 结构示例：{"result":0, "user":{"name":"xxx","headUrl":"...",id:"123"}, "title":"直播标题", "coverUrls":[...], "liveId":"","onlineCount":0, ...}
pub fn parse_room_info(json: &Value, room_id: &str) -> Result<LiveRoomDetail, SliveError> {
    let result = json.get("result")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| SliveError::Api { 
            code: -1, 
            message: "live/info: 无 result".to_string() 
        })?;
    
    if result != 0 {
        return Err(SliveError::Api { 
            code: result as i32, 
            message: format!("live/info: result {}", result) 
        });
    }
    
    // user 对象
    let user = json.get("user")
        .ok_or_else(|| SliveError::Api { 
            code: -1, 
            message: "live/info: 无 user".to_string() 
        })?;
    
    let user_name = user.get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    
    if user_name.is_empty() {
        return Err(SliveError::Api { code: -1, message: "live/info: 主播不存在".to_string() });
    }
    
    let user_avatar = user.get("headUrl")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    
    // 直播状态：liveId 非空表示在播
    let live_id = json.get("liveId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let is_live = !live_id.is_empty();
    
    // 标题
    let title = json.get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    
    // 封面：coverUrls 数组的第一个
    let cover = json.get("coverUrls")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    
    // 在线人数
    let online = json.get("onlineCount")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;
    
    // 简介 (signature)
    let introduction = user.get("signature")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    
    Ok(LiveRoomDetail {
        room_id: room_id.to_string(),
        title,
        cover,
        user_name,
        user_avatar,
        online,
        introduction,
        notice: None,
        status: is_live,
        data: None,
        danmaku_data: Some(room_id.to_string()),
        is_record: false,
        url: format!("https://live.acfun.cn/live/{}", room_id),
    })
}
