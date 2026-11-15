//! ZtLive 弹幕入参契约（JSON，snake_case）。
//!
//! 由宿主在 `startPlay` 成功后组装：`live_id` / `enter_room_attach` /
//! `availableTickets` 来自 startPlay，`uid` / `service_token` / `security`
//! 来自匿名 visitor 登录。

use serde::{Deserialize, Serialize};

use crate::error::SliveError;

/// link IM 会话所需的全部材料。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ZtLiveDanmakuArgs {
    /// 直播 id（startPlay 返回）。
    pub live_id: String,
    /// 匿名 visitor 的用户 id。
    pub uid: u64,
    /// 设备 id（与 visitor 登录时的 `_did` 一致）。
    pub did: String,
    /// visitor token（`acfun.api.visitor_st`）。
    pub service_token: String,
    /// `acSecurity`（base64，AES 密钥）。
    pub security: String,
    /// startPlay 下发的票据列表（至少一项）。
    pub tickets: Vec<String>,
    /// startPlay 下发的进房附属串。
    pub enter_room_attach: String,
}

impl ZtLiveDanmakuArgs {
    /// 从 JSON 契约解析并校验。
    pub fn from_json(raw: &str) -> Result<Self, SliveError> {
        let args: Self = serde_json::from_str(raw)
            .map_err(|e| SliveError::decode(format!("解析 ZtLive 弹幕参数失败: {e}")))?;
        args.validate()?;
        Ok(args)
    }

    /// 序列化为 JSON 契约。
    pub fn to_json(&self) -> Result<String, SliveError> {
        serde_json::to_string(self)
            .map_err(|e| SliveError::decode(format!("序列化 ZtLive 弹幕参数失败: {e}")))
    }

    fn validate(&self) -> Result<(), SliveError> {
        if self.live_id.trim().is_empty() {
            return Err(SliveError::invalid_input("ZtLive 弹幕参数缺少 live_id"));
        }
        if self.tickets.iter().all(|ticket| ticket.trim().is_empty()) {
            return Err(SliveError::invalid_input("ZtLive 弹幕参数缺少票据"));
        }
        if self.service_token.trim().is_empty() || self.security.trim().is_empty() {
            return Err(SliveError::invalid_input("ZtLive 弹幕参数缺少服务 token"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ZtLiveDanmakuArgs {
        ZtLiveDanmakuArgs {
            live_id: "abc".to_string(),
            uid: 1000000284842205,
            did: "web_0123456789abcdef".to_string(),
            service_token: "token".to_string(),
            security: "OVZnZ3cvaUFqdUVUT3RNOUFKUXdEUT09".to_string(),
            tickets: vec!["ticket".to_string()],
            enter_room_attach: "attach".to_string(),
        }
    }

    #[test]
    fn json_roundtrips() {
        let args = sample();
        let json = args.to_json().expect("序列化");
        assert_eq!(ZtLiveDanmakuArgs::from_json(&json).expect("反序列化"), args);
    }

    #[test]
    fn missing_ticket_is_rejected() {
        let mut args = sample();
        args.tickets.clear();
        assert!(args.validate().is_err());
    }

    #[test]
    fn missing_live_id_is_rejected() {
        let mut args = sample();
        args.live_id.clear();
        assert!(args.validate().is_err());
    }
}
