//! 小红书取流：按 `quality_type` 分档，H.264 与 H.265 各一条地址
//! （对照 PureLive `xiaohongshu_api.dart:qualities/resolution`）。

use serde_json::Value;

use crate::error::SliveError;
use crate::models::{LivePlayQuality, LivePlayUrl, LiveRoomDetail};

use super::api;

/// 从详情里读出画质列表。
///
/// 详情 `data` 里保存了分享页解析出的 `live / access / streams`；
/// 每行一条地址，`h265` 的行加「 · H.265」后缀，H.264 排在前面。
pub fn qualities(detail: &LiveRoomDetail) -> Result<Vec<LivePlayQuality>, SliveError> {
    let data: Value = detail
        .data
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .ok_or_else(|| SliveError::Api {
            code: -1,
            message: "详情里没有分享页数据（请重新取详情）".to_string(),
        })?;

    if data.get("live").and_then(Value::as_bool) == Some(false) {
        return Err(SliveError::Api {
            code: -1,
            message: "这场直播已结束".to_string(),
        });
    }
    match data.get("access").and_then(Value::as_str).unwrap_or("") {
        "unknown" => {
            return Err(SliveError::Api {
                code: -1,
                message: api::UNKNOWN_ACCESS_NOTICE.to_string(),
            })
        }
        "restricted" => {
            let kind = match data.get("restriction").and_then(Value::as_str).unwrap_or("") {
                "paid" => "付费",
                "private" => "仅限部分观众",
                "region" => "限定地区",
                _ => "受限",
            };
            return Err(SliveError::Api {
                code: -1,
                message: format!("{}{kind}", api::RESTRICTED_NOTICE),
            });
        }
        _ => {}
    }

    let streams = data
        .get("streams")
        .and_then(Value::as_array)
        .ok_or_else(|| SliveError::Api {
            code: -1,
            message: "详情里没有拉流地址".to_string(),
        })?;
    if streams.is_empty() {
        return Err(SliveError::Api {
            code: -1,
            message: "这场直播没有可用的拉流地址".to_string(),
        });
    }

    let mut qualities = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (index, stream) in streams.iter().enumerate() {
        let Some(url) = stream.get("url").and_then(Value::as_str) else {
            continue;
        };
        let codec = stream.get("codec").and_then(Value::as_str).unwrap_or("h264");
        let label = stream
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("原画");
        // 同一档位、同一编码只列一次（保留第一条地址）。
        if !seen.insert((label.to_string(), codec.to_string())) {
            continue;
        }
        let quality = if codec == "h265" {
            format!("{label} · H.265")
        } else {
            label.to_string()
        };
        // H.264 全体排在 H.265 之前（CLI 按 sort 降序展示）。
        let sort = if codec == "h265" {
            5_000 - index as i32
        } else {
            10_000 - index as i32
        };
        qualities.push(LivePlayQuality {
            quality,
            sort,
            data: url.to_string(),
        });
    }
    if qualities.is_empty() {
        return Err(SliveError::Api {
            code: -1,
            message: "拉流地址都不可用".to_string(),
        });
    }
    Ok(qualities)
}

/// 播放地址：媒体 CDN 要求 Referer 与 UA。
pub fn get_play_url(quality: &LivePlayQuality) -> LivePlayUrl {
    LivePlayUrl {
        urls: vec![quality.data.clone()],
        headers: Some(api::headers().into_iter().collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::LiveRoomDetail;
    use serde_json::json;

    fn detail(data: Value) -> LiveRoomDetail {
        LiveRoomDetail {
            room_id: "570541076495019830".to_string(),
            title: String::new(),
            cover: String::new(),
            user_name: String::new(),
            user_avatar: String::new(),
            online: 0,
            introduction: None,
            notice: None,
            status: true,
            data: Some(data.to_string()),
            danmaku_data: None,
            is_record: false,
            url: String::new(),
        }
    }

    #[test]
    fn h264_rows_come_before_h265_rows() {
        let value = json!({
            "live": true,
            "access": "public",
            "streams": [
                {"url": "https://a.xhscdn.com/live/1.m3u8", "codec": "h264", "quality": "HD", "label": "原画"},
                {"url": "https://a.xhscdn.com/live/1.flv", "codec": "h265", "quality": "HD", "label": "原画"}
            ]
        });
        let qualities = qualities(&detail(value)).expect("画质");
        assert_eq!(qualities[0].quality, "原画");
        assert!(qualities[0].sort > qualities[1].sort);
        assert_eq!(qualities[1].quality, "原画 · H.265");
    }

    #[test]
    fn ended_restricted_and_empty_are_errors() {
        for value in [
            json!({"live": false}),
            json!({"live": true, "access": "unknown"}),
            json!({"live": true, "access": "restricted", "restriction": "paid"}),
            json!({"live": true, "access": "public", "streams": []}),
        ] {
            assert!(qualities(&detail(value)).is_err());
        }
    }
}
