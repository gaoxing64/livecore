//! Twitch 仍未实现的能力（逐条对照旧 Dart `twitch_site.dart`）。
//!
//! 分类 / 分类房间 / 推荐 / 搜索已按 PureLive 的 `live_core` Twitch 实现重写，
//! 分别见 [`super::catalog`] / [`super::rooms`] / [`super::search`]。本模块只保留
//! 账号相关能力：Twitch 的读链路是匿名的，没有扫码登录、凭证注入或醒目留言。
//!
//! 错误口径：`UnimplementedError` -> [`SliveError::InvalidInput`]（含「Dart core 亦未实现」后缀），
//! 不静默兜底。

use crate::error::SliveError;
use crate::models::LiveSuperChatMessage;

/// F-10 SC 列表：恒空（对齐 Dart `return Future.value([])`）。
pub(crate) fn super_chat(_room_id: &str) -> Result<Vec<LiveSuperChatMessage>, SliveError> {
    Ok(Vec::new())
}

/// F-17 注入凭证：Dart `throw UnimplementedError()`（Twitch 匿名链路无 token/cookie）。
pub(crate) fn set_site_attrs(_data: &str) -> Result<(), SliveError> {
    Err(unsupported("Twitch 不支持注入凭证（匿名只读链路）"))
}

/// F-18 二维码：Dart `throw UnimplementedError()`。
pub(crate) fn qr_code() -> Result<crate::models::QrCode, SliveError> {
    Err(unsupported("Twitch 不支持二维码登录"))
}

/// F-19 扫码轮询：Dart `throw UnimplementedError()`。
pub(crate) fn qr_poll_result(_qrcode_key: &str) -> Result<crate::models::QrPollResult, SliveError> {
    Err(unsupported("Twitch 不支持扫码轮询"))
}

/// F-20 账号信息：Dart `throw UnimplementedError()`。
pub(crate) fn account_info(_credential: &str) -> Result<crate::models::AccountInfo, SliveError> {
    Err(unsupported("Twitch 不支持账号信息查询"))
}

/// 构造「平台未实现」错误（口径对齐 Dart `UnimplementedError`，映射为 `InvalidInput`）。
fn unsupported(what: &str) -> SliveError {
    SliveError::invalid_input(format!("{what}（Dart core 亦未实现）"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn super_chat_is_still_empty() {
        assert!(super_chat("xqc").expect("SC 恒空").is_empty());
    }

    #[test]
    fn unimplemented_methods_are_invalid_input() {
        for err in [
            set_site_attrs("{}").err(),
            qr_code().err(),
            qr_poll_result("k").err(),
            account_info("c").err(),
        ] {
            let err = err.expect("未实现必须报错");
            assert!(
                matches!(err, SliveError::InvalidInput(_)),
                "未实现应为 InvalidInput，实际 {err:?}"
            );
        }
    }
}
