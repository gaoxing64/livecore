//! Twitch GQL 请求构造：persisted query 与 raw query。
//!
//! Twitch Web 的 GQL 走 **persisted query**：只传 `operationName` + `sha256Hash`，
//! 服务端用哈希取已注册的服务端文档。因此本模块只负责**忠实构造 JSON**，
//! 不做任何签名/加密。
//!
//! 键序与 Dart 的模板串一致（`operationName` -> `extensions` -> `variables`），
//! 便于 golden 断言与抓包比对。
//!
//! 上半部分是旧链路（详情 / 取流，对照 `twitch_site.dart` 的 `buildPersistedRequest`），
//! 下半部分是 PureLive `live_core` 的列表能力（对照 `twitch_api.dart:135-264`）。

use serde_json::{json, Map, Value};

/// 构造单条 persisted query 的请求体（`buildPersistedRequest` 的 Rust 等价物）。
///
/// `variables` 会被原样嵌入；调用方负责其形状（各 operation 不同，见 [`super::detail`] / [`super::play`]）。
pub fn build_persisted_request(operation_name: &str, sha256_hash: &str, variables: Value) -> Value {
    json!({
        "operationName": operation_name,
        "extensions": {
            "persistedQuery": {
                "version": 1,
                "sha256Hash": sha256_hash,
            }
        },
        "variables": variables,
    })
}

/// 批量请求体：`[q1, q2]`（对齐 `twitch_site.dart:263` 的 `[${queries.join(',')}]`）。
pub fn build_batch(queries: &[Value]) -> Value {
    Value::Array(queries.to_vec())
}

/// `ChannelShell` 的 persisted query 哈希（`twitch_site.dart:249`）。
pub const CHANNEL_SHELL_HASH: &str =
    "fea4573a7bf2644f5b3f2cbbdcbee0d17312e48d2e55f080589d053aad353f11";
/// `StreamMetadata` 的 persisted query 哈希（`twitch_site.dart:256`）。
pub const STREAM_METADATA_HASH: &str =
    "b57f9b910f8cd1a4659d894fe7550ccc81ec9052c01e438b290fd66a040b9b93";
/// `PlaybackAccessToken` 的 persisted query 哈希（`twitch_site.dart:94`）。
pub const PLAYBACK_ACCESS_TOKEN_HASH: &str =
    "ed230aa1e33e07eebb8928504583da78a5173989fadfb1ac94be06a04f3cdbe9";

// ---------------------------------------------------------------- 列表能力

/// `SearchCategoryTags`：`twitch_api.dart:135`。
pub const SEARCH_CATEGORY_TAGS_HASH: &str =
    "b4cb189d8d17aadf29c61e9d7c7e7dcfc932e93b77b3209af5661bffb484195f";
/// `BrowsePage_AllDirectories`：`twitch_api.dart:136`。
pub const BROWSE_PAGE_ALL_DIRECTORIES_HASH: &str =
    "2f67f71ba89f3c0ed26a141ec00da1defecb2303595f5cda4298169549783d9e";
/// `DirectoryPage_Game`：`twitch_api.dart:137`。
pub const DIRECTORY_PAGE_GAME_HASH: &str =
    "76cb069d835b8a02914c08dc42c421d0dafda8af5b113a3f19141824b901402f";
/// `SearchResultsPage_SearchResults`：`twitch_api.dart:138`。
pub const SEARCH_RESULTS_PAGE_HASH: &str =
    "7f3580f6ac6cd8aa1424cff7c974a07143827d6fa36bba1b54318fe7f0b68dc5";

/// 一个分类标签每页的目录数（`twitch_api.dart:83`）。
pub const TAG_DIRECTORY_LIMIT: i64 = 30;
/// 一次 `DirectoryPage_Game` 请求的直播数（`twitch_api.dart:88`）。
pub const DIRECTORY_FETCH_LIMIT: i64 = 100;
/// 一次全站 `streams` 请求的直播数上限（`twitch_api.dart:91`）。
pub const STREAMS_LIMIT: i64 = 30;
/// 一个 GQL 批量请求最多携带的 operation 数（`twitch_api.dart:80`）。
pub const BATCH_LIMIT: usize = 35;
/// 单个分类标签最多翻多少页（`twitch_site.dart:27`）。
pub const TAG_PAGE_LIMIT: usize = 50;

/// 全站最热直播的 raw query（`twitch_api.dart:157-162`）。
///
/// Web 端没有对应的 persisted query，只能带文档原文；`$languages` 必须声明为
/// `[Language!]`，声明成 `[String!]` 会被 2026-10 之后的 schema 直接拒绝。
pub const STREAMS_QUERY: &str = concat!(
    "query($first: Int!, $after: Cursor, $languages: [Language!]) { ",
    "streams(first: $first, after: $after, options: {broadcasterLanguages: $languages}) { ",
    "edges { cursor node { id title type viewersCount createdAt ",
    "previewImageURL(width: 440, height: 248) restriction { type } ",
    "broadcaster { id login displayName profileImageURL(width: 70) } ",
    "game { id name displayName slug } } } ",
    "pageInfo { hasNextPage } } }",
);

/// `SearchCategoryTags`：全部标签（`twitch_api.dart:179-180`）。
pub fn tags_operation() -> Value {
    build_persisted_request(
        "SearchCategoryTags",
        SEARCH_CATEGORY_TAGS_HASH,
        json!({"userQuery": "", "limit": 100}),
    )
}

/// `BrowsePage_AllDirectories`：一个标签的一页目录（`twitch_api.dart:183-193`）。
///
/// Dart 的 `'cursor': ?cursor` 在 cursor 为 null 时**省略整个键**，这里保持一致。
pub fn directories_operation(tag_id: &str, cursor: Option<&str>) -> Value {
    let mut request = build_persisted_request(
        "BrowsePage_AllDirectories",
        BROWSE_PAGE_ALL_DIRECTORIES_HASH,
        json!({
            "limit": TAG_DIRECTORY_LIMIT,
            "options": {
                "recommendationsContext": {"platform": "web"},
                "requestID": "JIRA-VXP-2397",
                "sort": "VIEWER_COUNT",
                "tags": [tag_id],
            },
        }),
    );
    if let Some(cursor) = cursor {
        request["variables"]["cursor"] = json!(cursor);
    }
    request
}

/// `DirectoryPage_Game`：一个目录的一批直播（`twitch_api.dart:198-219`）。
///
/// 语言筛选在 PureLive 里来自设置；core 没有设置通道，默认空数组即「不过滤」，
/// 与 PureLive 的默认行为一致。
pub fn game_operation(slug: &str, cursor: Option<&str>) -> Value {
    let mut request = build_persisted_request(
        "DirectoryPage_Game",
        DIRECTORY_PAGE_GAME_HASH,
        json!({
            "imageWidth": 50,
            "slug": slug,
            "options": {
                "sort": "VIEWER_COUNT",
                "recommendationsContext": {"platform": "web"},
                "requestID": "JIRA-VXP-2397",
                "freeformTags": null,
                "tags": [],
                "broadcasterLanguages": [],
                "systemFilters": [],
            },
            "sortTypeIsRecency": false,
            "limit": DIRECTORY_FETCH_LIMIT,
            "includeCostreaming": true,
        }),
    );
    if let Some(cursor) = cursor {
        request["variables"]["cursor"] = json!(cursor);
    }
    request
}

/// 全站最热直播的 raw query 请求（`twitch_api.dart:223-234`）。
pub fn streams_operation(cursor: Option<&str>) -> Value {
    let mut variables = Map::new();
    variables.insert("first".to_string(), json!(STREAMS_LIMIT));
    if let Some(cursor) = cursor {
        variables.insert("after".to_string(), json!(cursor));
    }
    json!({"query": STREAMS_QUERY, "variables": Value::Object(variables)})
}

/// `SearchResultsPage_SearchResults`（`twitch_api.dart:250-264`）。
///
/// 后续页的 cursor 必须放在 `options.targets` 的 CHANNEL 索引里；顶层 cursor
/// 会被服务端忽略。`targets` 在首页是显式的 `null`，不是省略。
pub fn search_operation(keyword: &str, cursor: Option<&str>) -> Value {
    let targets = match cursor {
        Some(cursor) => json!([{"index": "CHANNEL", "cursor": cursor}]),
        None => Value::Null,
    };
    build_persisted_request(
        "SearchResultsPage_SearchResults",
        SEARCH_RESULTS_PAGE_HASH,
        json!({
            "platform": "web",
            "query": keyword,
            "options": {
                "targets": targets,
                "shouldSkipDiscoveryControl": false,
            },
            "requestID": "808c9f2e-f52e-431c-8dc7-d2e3c1831d77",
            "includeIsDJ": true,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_request_matches_dart_shape() {
        let got = build_persisted_request("ChannelShell", CHANNEL_SHELL_HASH, json!({"login": "x"}));
        assert_eq!(
            serde_json::to_string(&got).expect("序列化"),
            r#"{"operationName":"ChannelShell","extensions":{"persistedQuery":{"version":1,"sha256Hash":"fea4573a7bf2644f5b3f2cbbdcbee0d17312e48d2e55f080589d053aad353f11"}},"variables":{"login":"x"}}"#
        );
    }

    #[test]
    fn batch_wraps_queries_in_array() {
        let batch = build_batch(&[
            build_persisted_request("A", "ha", json!({})),
            build_persisted_request("B", "hb", json!({"k": 1})),
        ]);
        let arr = batch.as_array().expect("应为数组");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["operationName"], "A");
        assert_eq!(arr[1]["variables"]["k"], 1);
    }

    #[test]
    fn first_page_operations_omit_cursor_key() {
        let dirs = directories_operation("tag-1", None);
        assert!(dirs["variables"].get("cursor").is_none());
        assert_eq!(dirs["variables"]["options"]["tags"][0], "tag-1");
        assert_eq!(dirs["variables"]["limit"], TAG_DIRECTORY_LIMIT);

        let game = game_operation("just-chatting", None);
        assert!(game["variables"].get("cursor").is_none());
        assert_eq!(game["variables"]["limit"], DIRECTORY_FETCH_LIMIT);
        assert_eq!(game["variables"]["options"]["broadcasterLanguages"], json!([]));

        let streams = streams_operation(None);
        assert!(streams["variables"].get("after").is_none());
        assert_eq!(streams["variables"]["first"], STREAMS_LIMIT);
        assert!(streams["query"].as_str().expect("raw query").contains("[Language!]"));
    }

    #[test]
    fn later_pages_carry_cursor_in_the_right_place() {
        let dirs = directories_operation("tag-1", Some("CUR"));
        assert_eq!(dirs["variables"]["cursor"], "CUR");

        let game = game_operation("just-chatting", Some("CUR"));
        assert_eq!(game["variables"]["cursor"], "CUR");

        let streams = streams_operation(Some("CUR"));
        assert_eq!(streams["variables"]["after"], "CUR");

        let search = search_operation("minecraft", None);
        assert!(search["variables"]["options"]["targets"].is_null());
        let next = search_operation("minecraft", Some("CUR"));
        assert_eq!(next["variables"]["options"]["targets"][0]["index"], "CHANNEL");
        assert_eq!(next["variables"]["options"]["targets"][0]["cursor"], "CUR");
        assert!(next["variables"].get("cursor").is_none());
    }
}
