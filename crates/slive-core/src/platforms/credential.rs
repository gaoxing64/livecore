//! 平台凭证（Cookie）注入的共享解析。
//!
//! 各平台的 `set_site_attrs` 接受两种写法：
//! - 原始 Cookie 串（`a=1; b=2`）；
//! - JSON `{"cookie":"a=1; b=2"}`（与抖音的既有写法一致）。
//!
//! 空入参 = 未登录账号的空凭证，按抖音口径视为**不修改**（返回 `None`）；
//! JSON 里 `cookie` 为空串则返回 `Some("")`，语义是**清除**已注入的 Cookie。

use crate::error::SliveError;

/// 从 `set_site_attrs` 的入参里取出 Cookie 串；`None` 表示不修改。
pub(crate) fn parse_cookie(data: &str) -> Result<Option<String>, SliveError> {
    let text = data.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let cookie = if text.starts_with('{') {
        let value: serde_json::Value = serde_json::from_str(text)
            .map_err(|error| SliveError::invalid_input(format!("凭证不是合法 JSON：{error}")))?;
        let Some(cookie) = value.get("cookie") else {
            return Err(SliveError::invalid_input("凭证 JSON 缺少 cookie"));
        };
        match cookie {
            serde_json::Value::String(text) => text.clone(),
            serde_json::Value::Null => String::new(),
            other => crate::util::value_to_string(other),
        }
    } else {
        text.to_string()
    };
    let cookie = cookie.replace(['\r', '\n'], "").trim().to_string();
    Ok(Some(cookie))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_raw_and_json_forms() {
        assert_eq!(
            parse_cookie("a=1; b=2").expect("原始串").as_deref(),
            Some("a=1; b=2")
        );
        assert_eq!(
            parse_cookie(r#"{"cookie":"a=1; b=2"}"#)
                .expect("JSON")
                .as_deref(),
            Some("a=1; b=2")
        );
        // 空入参 = 未登录的空凭证：不修改。
        assert_eq!(parse_cookie("  ").expect("空串"), None);
        // JSON 空 cookie = 清除。
        assert_eq!(
            parse_cookie(r#"{"cookie":""}"#).expect("清空").as_deref(),
            Some("")
        );
        assert!(parse_cookie(r#"{"other":"x"}"#).is_err());
    }
}
