//! Twitch GQL 响应的公共解码（对照 PureLive `twitch_api.dart:390-458, 898-928`）。
//!
//! 列表能力的每个响应都要过一遍 [`check`]：匿名客户端常被要求提供浏览器
//! integrity token，此时 `extensions.challenge` 或 errors 里会出现特征串。
//! 这种失败与「网络挂了 / 接口变了」不同——列表分页遇到它应当**安静收尾**，
//! 所以单独用 [`GqlFailure::Integrity`] 表达，不混进 `SliveError`。

use serde_json::Value;

use crate::error::SliveError;
use crate::util::{as_i32, value_to_string};

/// GQL 层失败。
#[derive(Debug)]
pub(crate) enum GqlFailure {
    /// Twitch 要求浏览器 integrity token（列表后续页的正常终点）。
    Integrity,
    /// 其它失败，透传给调用方。
    Fatal(SliveError),
}

impl From<SliveError> for GqlFailure {
    fn from(error: SliveError) -> Self {
        GqlFailure::Fatal(error)
    }
}

impl From<GqlFailure> for SliveError {
    fn from(failure: GqlFailure) -> Self {
        match failure {
            GqlFailure::Integrity => SliveError::Api {
                code: -1,
                message: "twitch: integrity check required".to_string(),
            },
            GqlFailure::Fatal(error) => error,
        }
    }
}

fn api(what: impl Into<String>) -> GqlFailure {
    GqlFailure::Fatal(SliveError::Api {
        code: -1,
        message: what.into(),
    })
}

/// 一个响应可能是单 envelope 或批量数组（对照 `TwitchApi._envelope`）。
pub(crate) fn envelopes(decoded: &Value) -> Vec<&Value> {
    match decoded {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    }
}

/// 任一 envelope 是否带 integrity challenge（对照 `isIntegrityRejection`）。
pub(crate) fn is_integrity(decoded: &Value) -> bool {
    fn one(envelope: &Value) -> bool {
        if envelope.pointer("/extensions/challenge").is_some() {
            return true;
        }
        envelope
            .get("errors")
            .and_then(Value::as_array)
            .map(|errors| {
                errors.iter().any(|error| {
                    let message = error
                        .get("message")
                        .map(value_to_string)
                        .unwrap_or_default()
                        .to_lowercase();
                    message.contains("failed integrity check") || message.contains("integrity token")
                })
            })
            .unwrap_or(false)
    }

    envelopes(decoded).iter().any(|envelope| one(envelope))
}

/// 拒绝 integrity challenge；其余情形继续。
pub(crate) fn check(decoded: &Value) -> Result<(), GqlFailure> {
    if is_integrity(decoded) {
        Err(GqlFailure::Integrity)
    } else {
        Ok(())
    }
}

/// 取第一个 envelope 的 `data`（对照 `TwitchApi._data` / `_envelope`）。
///
/// 有 partial `service error` 但 `data` 仍在时照常返回；只有在没有 data 时
/// 才把错误信息拼进异常。服务端不再认识 persisted query 时单独报错，
/// 因为那意味着所有用同一 hash 的请求都会失败。
pub(crate) fn data<'a>(decoded: &'a Value, what: &str) -> Result<&'a Value, GqlFailure> {
    check(decoded)?;
    let envelope = envelopes(decoded)
        .into_iter()
        .next()
        .filter(|value| value.is_object())
        .ok_or_else(|| api(format!("{what}: no envelope")))?;

    let errors: Vec<String> = envelope
        .get("errors")
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .map(|error| error.get("message").map(value_to_string).unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    if errors.iter().any(|message| message.contains("PersistedQueryNotFound")) {
        return Err(api(format!("{what}: persisted query no longer known")));
    }

    match envelope.get("data") {
        Some(value) if value.is_object() => Ok(value),
        _ => {
            let detail = if errors.is_empty() {
                String::new()
            } else {
                format!(" ({})", errors.join("; "))
            };
            Err(api(format!("{what}: no data{detail}")))
        }
    }
}

/// 校验批量响应的 envelope 数量与请求顺序一致（对照 `TwitchApi.batch`）。
pub(crate) fn batch_envelopes<'a>(
    decoded: &'a Value,
    expected: usize,
    what: &str,
) -> Result<Vec<&'a Value>, GqlFailure> {
    check(decoded)?;
    let items = match decoded {
        Value::Array(items) => items.iter().collect::<Vec<_>>(),
        other => vec![other],
    };
    if items.len() != expected {
        return Err(api(format!(
            "{what}: {} answers for {expected} operations",
            items.len()
        )));
    }
    Ok(items)
}

/// connection 的 edges（缺失/类型不符 -> 空）。
pub(crate) fn edges(connection: Option<&Value>) -> &[Value] {
    connection
        .and_then(|value| value.get("edges"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// `pageInfo.hasNextPage` 为真时取最后一条 edge 的 cursor（对照 `_nextCursor`）。
pub(crate) fn next_cursor(connection: Option<&Value>) -> Option<String> {
    let connection = connection?;
    if connection.pointer("/pageInfo/hasNextPage") != Some(&Value::Bool(true)) {
        return None;
    }
    let edges = edges(Some(connection));
    edges
        .last()
        .and_then(|edge| edge.get("cursor"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// 对象字段转文本（对齐 Dart `_text`：null -> 空串，数字/布尔转字符串）。
pub(crate) fn text(value: Option<&Value>) -> String {
    value.map(value_to_string).unwrap_or_default()
}

/// 字段转 i32（对齐 Dart `jsonString` 之外的数值口径）。
pub(crate) fn count(value: Option<&Value>) -> i32 {
    as_i32(value).unwrap_or(0)
}

/// 目录/游戏在请求语言下的名字：`displayName` 优先，回落 `name`。
pub(crate) fn game_name(game: &Value) -> String {
    let display = text(game.get("displayName"));
    if display.is_empty() {
        text(game.get("name"))
    } else {
        display
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn integrity_challenge_is_recognized_in_any_form() {
        assert!(is_integrity(&json!({"extensions": {"challenge": {"type": "integrity"}}})));
        assert!(is_integrity(&json!([{"data": {}}, {"errors": [{"message": "failed integrity check"}]}])));
        assert!(is_integrity(&json!({"errors": [{"message": "missing integrity token"}]})));
        assert!(!is_integrity(&json!({"data": {"user": null}, "errors": [{"message": "service error"}]})));
    }

    #[test]
    fn data_accepts_partial_errors_but_rejects_missing_data() {
        let partial = json!({"data": {"game": {"id": "1"}}, "errors": [{"message": "service error"}]});
        assert_eq!(data(&partial, "x").expect("partial data")["game"]["id"], "1");

        let empty = json!({"data": null, "errors": [{"message": "service error"}]});
        let err = data(&empty, "x").expect_err("无 data 必须报错");
        assert!(matches!(err, GqlFailure::Fatal(SliveError::Api { .. })));

        let stale = json!({"errors": [{"message": "PersistedQueryNotFound"}]});
        assert!(matches!(data(&stale, "x"), Err(GqlFailure::Fatal(_))));
    }

    #[test]
    fn next_cursor_requires_has_next_page_and_edges() {
        let connection = json!({
            "pageInfo": {"hasNextPage": true},
            "edges": [{"cursor": "A"}, {"cursor": "B"}]
        });
        assert_eq!(next_cursor(Some(&connection)).as_deref(), Some("B"));
        let last = json!({"pageInfo": {"hasNextPage": false}, "edges": [{"cursor": "B"}]});
        assert_eq!(next_cursor(Some(&last)), None);
        assert_eq!(next_cursor(None), None);
    }

    #[test]
    fn batch_envelopes_keep_request_order() {
        let decoded = json!([{"data": {"a": 1}}, {"data": {"a": 2}}]);
        let items = batch_envelopes(&decoded, 2, "batch").expect("数量一致");
        assert_eq!(items[0]["data"]["a"], 1);
        assert_eq!(items[1]["data"]["a"], 2);
        assert!(matches!(
            batch_envelopes(&decoded, 3, "batch"),
            Err(GqlFailure::Fatal(_))
        ));
    }
}
