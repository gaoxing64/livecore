//! 微博取流：直播一条 FLV、回放一条 HLS（对照 PureLive `weibo_api.dart:qualityOf`）。
//!
//! 媒体 CDN 不校验请求头（PureLive `mediaHeaders` 为空），地址也没有签名或租期。

use crate::error::SliveError;
use crate::models::{LivePlayQuality, LivePlayUrl};

use super::api;
use super::detail::{Access, WeiboBroadcast};

/// 直播的唯一画质名。
pub const QUALITY_LIVE: &str = "原始流";
/// 回放的唯一画质名。
pub const QUALITY_REPLAY: &str = "原画";

/// 这场直播可播的唯一画质；不可播时返回带原因的错误。
pub fn quality(broadcast: &WeiboBroadcast) -> Result<LivePlayQuality, SliveError> {
    if broadcast.status == api::STATUS_ENDED {
        return Err(SliveError::Api {
            code: -1,
            message: "这场直播已结束".to_string(),
        });
    }
    if broadcast.status != api::STATUS_LIVE && broadcast.status != api::STATUS_REPLAY {
        return Err(SliveError::Api {
            code: -1,
            message: format!("未知的直播状态 {}", broadcast.status),
        });
    }
    match broadcast.access {
        Access::Restricted => {
            let kind = restriction_name(broadcast.watch_limit);
            let tip = broadcast
                .tip
                .as_deref()
                .map(|tip| format!("：{tip}"))
                .unwrap_or_default();
            Err(SliveError::Api {
                code: -1,
                message: format!("受限直播（{kind}，watch_limit {}）{tip}", broadcast.watch_limit),
            })
        }
        Access::Disabled => Err(SliveError::Api {
            code: -1,
            message: "这场直播已关闭播放".to_string(),
        }),
        Access::Public => {
            if broadcast.status == api::STATUS_LIVE {
                let url = broadcast.media_urls.first().ok_or_else(|| SliveError::Api {
                    code: -1,
                    message: "直播没有可用的媒体地址".to_string(),
                })?;
                Ok(LivePlayQuality {
                    quality: QUALITY_LIVE.to_string(),
                    sort: 100,
                    data: url.clone(),
                })
            } else {
                let url = broadcast.replay_url.as_ref().ok_or_else(|| SliveError::Api {
                    code: -1,
                    message: "这场回放没有录像地址".to_string(),
                })?;
                Ok(LivePlayQuality {
                    quality: QUALITY_REPLAY.to_string(),
                    sort: 100,
                    data: url.clone(),
                })
            }
        }
    }
}

/// `watch_limit` 的可读名称。
pub fn restriction_name(watch_limit: i64) -> &'static str {
    match watch_limit {
        api::LIMIT_APP_ONLY => "仅限微博 App",
        api::LIMIT_FRIENDS_ONLY | api::LIMIT_SELF_ONLY => "仅部分观众",
        api::LIMIT_PAID => "付费",
        _ => "受限",
    }
}

/// 播放地址：媒体 CDN 不需要请求头。
pub fn get_play_url(quality: &LivePlayQuality) -> LivePlayUrl {
    LivePlayUrl {
        urls: vec![quality.data.clone()],
        headers: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::LiveRoomDetail;

    fn broadcast(status: i64, access: Access, media: &[&str], replay: Option<&str>) -> WeiboBroadcast {
        WeiboBroadcast {
            room: LiveRoomDetail {
                room_id: "1022:2321325347771573207158".to_string(),
                title: String::new(),
                cover: String::new(),
                user_name: String::new(),
                user_avatar: String::new(),
                online: 0,
                introduction: None,
                notice: None,
                status: status == 1,
                data: None,
                danmaku_data: None,
                is_record: status == 3,
                url: String::new(),
            },
            status,
            watch_limit: 0,
            access,
            media_urls: media.iter().map(|url| (*url).to_string()).collect(),
            replay_url: replay.map(str::to_string),
            tip: None,
        }
    }

    #[test]
    fn live_and_replay_pick_their_single_quality() {
        let live = quality(&broadcast(1, Access::Public, &["https://cdn/a.flv"], None)).expect("直播");
        assert_eq!(live.quality, QUALITY_LIVE);
        assert_eq!(live.data, "https://cdn/a.flv");

        let replay = quality(&broadcast(3, Access::Public, &[], Some("https://cdn/r.m3u8"))).expect("回放");
        assert_eq!(replay.quality, QUALITY_REPLAY);
        assert_eq!(replay.data, "https://cdn/r.m3u8");
    }

    #[test]
    fn restricted_and_empty_media_are_errors() {
        for value in [
            broadcast(1, Access::Restricted, &[], None),
            broadcast(1, Access::Disabled, &[], None),
            broadcast(1, Access::Public, &[], None),
            broadcast(5, Access::Public, &["https://cdn/a.flv"], None),
        ] {
            assert!(quality(&value).is_err());
        }
    }
}
