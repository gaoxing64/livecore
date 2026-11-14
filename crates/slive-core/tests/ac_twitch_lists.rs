//! Twitch 列表能力（分类 / 首页推荐 / 搜索）的离线契约测试。
//!
//! 行为基准：PureLive `live_core/src/sites/twitch/{twitch_api,twitch_site}.dart`。
//! 夹具 `fixtures/twitch/lists/responses.json` 的字段形状取自 PureLive 的
//! `fixtures/twitch/S01*/S02*/S03*/S04*` 抓包样本，取值合成为便于断言的小样本。

mod twitch_common;

use std::sync::OnceLock;

use serde_json::Value;
use slive_core::error::SliveError;
use slive_core::models::LiveSubCategory;
use twitch_common::{harness, Harness, GQL_KEY};

fn responses() -> &'static Value {
    static RESPONSES: OnceLock<Value> = OnceLock::new();
    RESPONSES.get_or_init(|| {
        serde_json::from_str(include_str!("fixtures/twitch/lists/responses.json"))
            .expect("解析 Twitch 列表夹具")
    })
}

fn response(key: &str) -> String {
    serde_json::to_string(&responses()[key]).expect("序列化夹具")
}

/// 分类链路：中文 tags -> 英文补名 -> 两轮目录批量。
fn stub_catalog(h: &Harness) {
    let zh = response("tags_zh");
    let en = response("tags_en");
    h.gql
        .stub_sequence(GQL_KEY, "SearchCategoryTags", &[&zh, &en]);
    let p1 = response("directories_p1");
    let p2 = response("directories_p2");
    h.gql
        .stub_sequence(GQL_KEY, "BrowsePage_AllDirectories", &[&p1, &p2]);
}

fn category() -> LiveSubCategory {
    LiveSubCategory {
        name: "谈天说地".to_string(),
        pic: None,
        id: "just-chatting".to_string(),
        parent_id: "tag-1".to_string(),
    }
}

fn gql_bodies(h: &Harness) -> Vec<Value> {
    h.gql
        .requests()
        .iter()
        .map(|request| serde_json::from_str(&request.body).expect("请求体是 JSON"))
        .collect()
}

#[tokio::test]
async fn categories_build_a_tag_tree_with_slug_ids_and_chinese_names() {
    let h = harness();
    stub_catalog(&h);

    let categories = h.site.get_categories().await.expect("分类树");
    assert_eq!(categories.len(), 3);
    assert_eq!(categories[0].id, "tag-1");
    assert_eq!(categories[0].name, "冒险游戏");
    // Twitch 没有中文名的标签用英文请求补名。
    assert_eq!(categories[2].id, "tag-3");
    assert_eq!(categories[2].name, "Gambling");

    // 首轮 30 个目录（满页）触发 cursor 翻页，第二轮再补 1 个。
    assert_eq!(categories[0].children.len(), 31);
    let first = &categories[0].children[0];
    assert_eq!(first.id, "game-1", "DirectoryPage_Game 用 slug 取流");
    assert_eq!(first.name, "游戏1");
    assert_eq!(first.parent_id, "tag-1");
    assert!(first.pic.as_deref().unwrap_or("").contains("ttv-boxart"));
    assert_eq!(categories[0].children[30].id, "game-200");
    assert_eq!(categories[1].children.len(), 1);
    assert!(categories[2].children.is_empty());

    let requests = h.gql.requests();
    assert_eq!(requests.len(), 4, "tags(中) + tags(英) + 两轮目录");
    let bodies = gql_bodies(&h);
    assert_eq!(bodies[0]["operationName"], "SearchCategoryTags");
    assert_eq!(bodies[2].as_array().expect("目录批量").len(), 3);
    assert_eq!(bodies[3].as_array().expect("第二轮批量").len(), 1);
    assert_eq!(
        bodies[3][0]["variables"]["cursor"], "dir-cur-30",
        "第二轮按首轮最后一个 cursor 翻页"
    );
    assert_eq!(
        requests[0].headers.iter().find(|(k, _)| k == "accept-language").map(|(_, v)| v.as_str()),
        Some("zh-CN,zh;q=0.9,en;q=0.8")
    );
    assert_eq!(
        requests[1].headers.iter().find(|(k, _)| k == "accept-language").map(|(_, v)| v.as_str()),
        Some("en-US,en;q=0.9")
    );
}

#[tokio::test]
async fn category_rooms_paginate_from_the_cursor_snapshot() {
    let h = harness();
    let p1 = response("game_p1");
    let p2 = response("game_p2");
    h.gql
        .stub_sequence(GQL_KEY, "DirectoryPage_Game", &[&p1, &p2]);

    let page1 = h
        .site
        .get_category_rooms(category(), 1, Some(2))
        .await
        .expect("第一页");
    assert_eq!(page1.items.len(), 2);
    assert!(page1.has_more);
    assert_eq!(page1.items[0].room_id, "alice");
    assert_eq!(page1.items[0].user_name, "ALICE");
    assert_eq!(page1.items[0].online, 59775);
    assert!(page1.items[0].cover.ends_with("?&t=1700000000"));

    let page2 = h
        .site
        .get_category_rooms(category(), 2, Some(2))
        .await
        .expect("第二页");
    assert_eq!(page2.items.len(), 1);
    assert_eq!(page2.items[0].room_id, "carol");
    assert!(page2.has_more);

    let page3 = h
        .site
        .get_category_rooms(category(), 3, Some(2))
        .await
        .expect("第三页");
    assert_eq!(page3.items.len(), 2);
    assert_eq!(page3.items[0].room_id, "dave");
    assert!(!page3.has_more, "cursor 用尽且快照见底");

    assert_eq!(h.gql.requests().len(), 2, "三个页面只抓了两块");
    let bodies = gql_bodies(&h);
    assert!(bodies[0][0]["variables"].get("cursor").is_none());
    assert_eq!(bodies[1][0]["variables"]["cursor"], "stream-cur-carol");
    assert_eq!(bodies[0][0]["variables"]["slug"], "just-chatting");
    assert_eq!(bodies[0][0]["variables"]["limit"], 100);
}

#[tokio::test]
async fn recommend_rooms_use_the_raw_streams_query() {
    let h = harness();
    let p1 = response("streams_p1");
    let p2 = response("streams_p2");
    h.gql
        .stub_sequence(GQL_KEY, "query($first: Int!", &[&p1, &p2]);

    let page1 = h.site.get_recommend_rooms(1, Some(2)).await.expect("首页");
    assert_eq!(page1.items.len(), 2);
    assert_eq!(page1.items[0].room_id, "alice");
    assert!(page1.has_more);

    let page2 = h.site.get_recommend_rooms(2, Some(2)).await.expect("第二页");
    assert_eq!(page2.items.len(), 1);
    assert!(page2.has_more);

    let page3 = h.site.get_recommend_rooms(3, Some(2)).await.expect("第三页");
    assert_eq!(page3.items.len(), 2);
    assert_eq!(page3.items[0].room_id, "dave");
    assert!(!page3.has_more);

    let bodies = gql_bodies(&h);
    assert!(bodies[0].get("operationName").is_none(), "raw query 无 operationName");
    assert!(bodies[0]["query"].as_str().unwrap().contains("[Language!]"));
    assert_eq!(bodies[0]["variables"]["first"], 30);
    assert!(bodies[0]["variables"].get("after").is_none());
    assert_eq!(bodies[1]["variables"]["after"], "stream-cur-carol");
}

#[tokio::test]
async fn search_rooms_return_only_live_channels_and_follow_the_channel_cursor() {
    let h = harness();
    let p1 = response("search_p1");
    let p2 = response("search_p2");
    h.gql
        .stub_sequence(GQL_KEY, "SearchResultsPage_SearchResults", &[&p1, &p2]);

    let page1 = h.site.search_rooms("minecraft".to_string(), 1).await.expect("搜索首页");
    assert_eq!(page1.items.len(), 2, "未开播频道不进房间列表");
    assert_eq!(page1.items[0].room_id, "feinberg");
    assert_eq!(page1.items[0].title, "chamber trials");
    assert_eq!(page1.items[1].room_id, "tubbo");
    assert!(page1.has_more);

    let page2 = h.site.search_rooms("minecraft".to_string(), 2).await.expect("搜索第二页");
    assert_eq!(page2.items.len(), 2);
    assert_eq!(page2.items[0].room_id, "dominoe");
    assert!(!page2.has_more);

    let bodies = gql_bodies(&h);
    assert!(bodies[0]["variables"]["options"]["targets"].is_null());
    assert_eq!(bodies[0]["variables"]["query"], "minecraft");
    assert_eq!(bodies[1]["variables"]["options"]["targets"][0]["index"], "CHANNEL");
    assert_eq!(bodies[1]["variables"]["options"]["targets"][0]["cursor"], "MTA=");
    assert!(bodies[1]["variables"].get("cursor").is_none(), "顶层 cursor 被服务端忽略");
}

#[tokio::test]
async fn search_anchors_include_offline_channels() {
    let h = harness();
    let p1 = response("search_p1");
    h.gql
        .stub_body(GQL_KEY, "SearchResultsPage_SearchResults", &p1);

    let result = h.site.search_anchors("minecraft".to_string(), 1).await.expect("主播搜索");
    assert_eq!(result.items.len(), 3);
    assert_eq!(result.items[0].room_id, "feinberg");
    assert!(result.items[0].live_status);
    assert!(!result.items[1].live_status, "未开播主播仍可被搜到");
    assert_eq!(result.items[1].room_id, "minecraft");
    assert!(result.items[1].avatar.contains("minecraft.png"));
    assert!(result.has_more);
}

#[tokio::test]
async fn blank_keyword_and_empty_slug_short_circuit_without_requests() {
    let h = harness();
    assert!(h.site.search_rooms("   ".to_string(), 1).await.expect("空关键词").items.is_empty());
    let empty = h
        .site
        .get_category_rooms(
            LiveSubCategory {
                name: String::new(),
                pic: None,
                id: "  ".to_string(),
                parent_id: String::new(),
            },
            1,
            None,
        )
        .await
        .expect("空 slug");
    assert!(!empty.has_more && empty.items.is_empty());
    assert_eq!(h.gql.requests().len(), 0);
}

#[tokio::test]
async fn unknown_directory_is_not_found() {
    let h = harness();
    let missing = response("game_missing");
    h.gql.stub_body(GQL_KEY, "DirectoryPage_Game", &missing);
    let err = h
        .site
        .get_category_rooms(category(), 1, None)
        .await
        .expect_err("未知目录");
    assert!(matches!(err, SliveError::NotFound(_)), "实际 {err:?}");
}

#[tokio::test]
async fn integrity_challenge_on_a_later_chunk_ends_the_list_quietly() {
    let h = harness();
    let p1 = response("game_p1");
    let challenge = response("integrity");
    h.gql
        .stub_sequence(GQL_KEY, "DirectoryPage_Game", &[&p1, &challenge]);

    assert!(h
        .site
        .get_category_rooms(category(), 1, Some(2))
        .await
        .expect("首页正常")
        .has_more);
    let page3 = h
        .site
        .get_category_rooms(category(), 3, Some(2))
        .await
        .expect("后续块的 integrity 不是错误");
    assert!(page3.items.is_empty());
    assert!(!page3.has_more);
    assert_eq!(h.gql.requests().len(), 2);
}

#[tokio::test]
async fn integrity_challenge_on_the_first_chunk_is_an_error_for_category_rooms() {
    let h = harness();
    let challenge = response("integrity");
    h.gql.stub_body(GQL_KEY, "DirectoryPage_Game", &challenge);

    let err = h
        .site
        .get_category_rooms(category(), 1, Some(2))
        .await
        .expect_err("首块 integrity 应报错");
    assert!(
        matches!(err, SliveError::Api { code: -1, .. }),
        "实际 {err:?}"
    );
    assert_eq!(h.gql.requests().len(), 1);
}

#[tokio::test]
async fn integrity_challenge_on_the_first_chunk_is_an_error_for_recommend_rooms() {
    let h = harness();
    let challenge = response("integrity");
    h.gql.stub_body(GQL_KEY, "streams", &challenge);

    let err = h
        .site
        .get_recommend_rooms(1, Some(2))
        .await
        .expect_err("首块 integrity 应报错");
    assert!(
        matches!(err, SliveError::Api { code: -1, .. }),
        "实际 {err:?}"
    );
    assert_eq!(h.gql.requests().len(), 1);
}

#[tokio::test]
async fn integrity_challenge_on_the_first_search_page_is_an_error() {
    let h = harness();
    let challenge = response("integrity");
    h.gql
        .stub_body(GQL_KEY, "SearchResultsPage_SearchResults", &challenge);

    let err = h
        .site
        .search_rooms("minecraft".to_string(), 1)
        .await
        .expect_err("搜索首页 integrity 应报错");
    assert!(
        matches!(err, SliveError::Api { code: -1, .. }),
        "实际 {err:?}"
    );
    assert_eq!(h.gql.requests().len(), 1);
}
