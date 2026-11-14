//! ACFUN Visitor 会话管理。

use serde_json::Value;
use crate::error::SliveError;

/// ACFUN 匿名访客会话。
#[derive(Debug, Clone)]
pub struct AcfunVisitor {
    pub user_id: String,
    pub device_id: String,
    pub token: String,
    pub expires_at: u64, // Unix timestamp
}

impl AcfunVisitor {
    /// 从 visitor/login 响应解析。
    pub fn from_response(body: &str, device_id: &str, now_secs: u64) -> Result<Self, SliveError> {
        let json: Value = serde_json::from_str(body)
            .map_err(|e| SliveError::decode(format!("解析 visitor/login 失败: {}", e)))?;
        
        let result = json.get("result")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| SliveError::Api { 
                code: -1, 
                message: "visitor/login: 无 result".to_string() 
            })?;
        
        if result != 0 {
            return Err(SliveError::Api { 
                code: result as i32, 
                message: format!("visitor/login: result {}", result) 
            });
        }
        
        let token = json.get("acfun.api.visitor_st")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SliveError::Api { 
                code: -1, 
                message: "visitor/login: 无 token".to_string() 
            })?
            .to_string();
        
        // `userId` 是 JSON 数字（如 1000000284766701），也接受字符串写法。
        let user_id = json
            .get("userId")
            .map(crate::util::value_to_string)
            .filter(|text| !text.is_empty() && text != "null")
            .ok_or_else(|| SliveError::Api {
                code: -1,
                message: "visitor/login: 无 userId".to_string(),
            })?;
        
        // 5 分钟有效期
        let expires_at = now_secs + 300;
        
        Ok(Self {
            user_id,
            device_id: device_id.to_string(),
            token,
            expires_at,
        })
    }
    
    /// 检查是否过期。
    pub fn is_expired(&self, now_secs: u64) -> bool {
        now_secs >= self.expires_at
    }
}

/// 生成设备 ID（`web_` + 16 位十六进制）。
///
/// 用「纳秒 + 进程 id + 进程内计数器」混合，避免同一毫秒启动的两个进程
/// 生成相同的 `did`（它同时是 `_did` cookie 与 `did` 参数的身份）。
pub fn generate_device_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let mixed = nanos
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .rotate_left(17)
        .wrapping_add(u64::from(std::process::id()))
        .wrapping_add(COUNTER.fetch_add(1, Ordering::Relaxed));
    format!("web_{mixed:016x}")
}
