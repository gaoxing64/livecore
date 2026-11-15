//! YouTube 弹幕入参：直播视频 id（裸字符串或 JSON 契约）。
//!
//! 房间详情的 `danmaku_data` 已经是 `video_id` 裸串；为兼容调用方，
//! 也接受 `{"video_id":"..."}` 形式。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::SliveError;

/// 弹幕会话入参。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct YouTubeDanmakuArgs {
    /// 直播视频 id（watch?v=）。
    pub video_id: String,
    /// 可选 Cookie（登录态解锁观看页 / live_chat 的风控）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie: Option<String>,
}

impl YouTubeDanmakuArgs {
    /// 裸视频 id。
    pub fn new(video_id: &str) -> Result<Self, SliveError> {
        let video_id = video_id.trim();
        if video_id.is_empty() {
            return Err(SliveError::invalid_input("YouTube 弹幕缺少视频 id"));
        }
        if !crate::platforms::youtube::api::is_video_id(video_id) {
            return Err(SliveError::invalid_input(format!(
                "YouTube 视频 id 非法：{video_id}（需要 11 位视频 id，不是频道 id）"
            )));
        }
        Ok(Self {
            video_id: video_id.to_string(),
            cookie: None,
        })
    }

    /// 附加 Cookie（空串视为未注入）。
    pub fn with_cookie(mut self, cookie: Option<&str>) -> Self {
        self.cookie = cookie
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string);
        self
    }

    /// 兼容裸 id 与 JSON 契约。
    pub fn parse(input: &str) -> Result<Self, SliveError> {
        let trimmed = input.trim();
        if trimmed.starts_with('{') {
            let value: Value = serde_json::from_str(trimmed)
                .map_err(|e| SliveError::decode(format!("解析 YouTube 弹幕参数失败: {e}")))?;
            let video_id = value
                .get("video_id")
                .and_then(Value::as_str)
                .ok_or_else(|| SliveError::invalid_input("YouTube 弹幕参数缺少 video_id"))?;
            let cookie = value.get("cookie").and_then(Value::as_str);
            return Ok(Self::new(video_id)?.with_cookie(cookie));
        }
        Self::new(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_video_id_is_accepted() {
        assert_eq!(
            YouTubeDanmakuArgs::parse("dQw4w9WgXcQ").unwrap(),
            YouTubeDanmakuArgs::new("dQw4w9WgXcQ").unwrap()
        );
    }

    #[test]
    fn json_contract_is_accepted() {
        let args = YouTubeDanmakuArgs::parse(r#"{"video_id":"dQw4w9WgXcQ"}"#).expect("解析");
        assert_eq!(args.video_id, "dQw4w9WgXcQ");
    }

    #[test]
    fn empty_id_is_rejected() {
        assert!(YouTubeDanmakuArgs::parse("   ").is_err());
    }

    #[test]
    fn cookie_is_kept_from_json_and_builder() {
        let args = YouTubeDanmakuArgs::parse(r#"{"video_id":"dQw4w9WgXcQ","cookie":"SID=1"}"#)
            .expect("解析");
        assert_eq!(args.cookie.as_deref(), Some("SID=1"));

        let args = YouTubeDanmakuArgs::new("dQw4w9WgXcQ")
            .expect("构造")
            .with_cookie(Some("  SID=2  "));
        assert_eq!(args.cookie.as_deref(), Some("SID=2"));
        assert_eq!(
            YouTubeDanmakuArgs::new("dQw4w9WgXcQ")
                .expect("构造")
                .with_cookie(Some("  "))
                .cookie,
            None
        );
    }

    #[test]
    fn channel_id_is_rejected() {
        assert!(YouTubeDanmakuArgs::parse("UCSJ4gkVC6NrvII8umztf0Ow").is_err());
    }
}
