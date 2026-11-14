//! 快手平台离线契约测试。

mod kuaishou_common;

use kuaishou_common::{harness, sample};
use slive_core::error::SliveError;
use slive_core::models::LiveSubCategory;
use slive_core::platforms::LiveSite;

#[tokio::test]
async fn categories_fetch_their_areas() {
    let h = harness();
    h.http.stub("live.kuaishou.com", &sample("category_areas"));

    let categories = h.site.get_categories().await.expect("分类与区域");
    assert_eq!(categories.len(), 8, "固定 8 个一级分类");
    assert_eq!(categories[0].name, "热门");
    let children = &categories[0].children;
    assert_eq!(children.len(), 2);
    assert_eq!(children[0].id, "22008");
    assert_eq!(children[0].name, "和平精英");
    assert_eq!(children[0].parent_id, "1");
    let picture = children[0].pic.as_deref().unwrap_or("");
    assert!(
        picture.contains("a.webp"),
        "区域封面应优先取 poster，实际 {picture:?}"
    );
    assert_eq!(children[1].id, "22484");
}

#[tokio::test]
async fn category_rooms_map_area_cards() {
    let h = harness();
    h.http.stub("live.kuaishou.com", &sample("area_rooms"));

    let rooms = h
        .site
        .get_category_rooms(
            LiveSubCategory {
                id: "22008".to_string(),
                name: "和平精英".to_string(),
                pic: None,
                parent_id: "1".to_string(),
            },
            1,
            None,
        )
        .await
        .expect("区域房间");
    assert_eq!(rooms.items.len(), 2);
    assert_eq!(rooms.items[0].room_id, "3xarea1");
    assert_eq!(rooms.items[0].title, "区域直播 1");
    assert_eq!(rooms.items[0].online, 20_000);
    assert!(rooms.has_more);

    assert!(matches!(
        h.site.set_site_attrs("{}").err(),
        Some(SliveError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn recommend_rooms_flatten_home_groups() {
    let h = harness();
    h.http.stub("live.kuaishou.com", &sample("home_list"));

    let result = h
        .site
        .get_recommend_rooms(1, Some(10))
        .await
        .expect("首页推荐");
    assert_eq!(result.items.len(), 2, "gameLiveInfo[].liveInfo[] 需扁平化");
    assert_eq!(result.items[0].room_id, "3xauthor1", "房间页用主播 id");
    assert_eq!(result.items[0].title, "测试直播标题 1");
    assert_eq!(result.items[0].user_name, "主播 A");
    assert_eq!(result.items[0].online, 10_000, "\"1 万+\" 归一为 10000");
    assert_eq!(result.items[1].online, 5_000);
    assert_eq!(result.items[1].room_id, "3xauthor2");
}

#[tokio::test]
async fn search_returns_authors_as_rooms_and_anchors() {
    let h = harness();
    h.http.stub("live.kuaishou.com", &sample("search_author"));

    let rooms = h
        .site
        .search_rooms("搜索词".to_string(), 1)
        .await
        .expect("房间搜索");
    assert_eq!(rooms.items.len(), 2, "search/author 返回作者，标题即昵称");
    assert_eq!(rooms.items[0].room_id, "USR001");
    assert_eq!(rooms.items[0].title, "搜索主播");
    assert!(!rooms.has_more);

    let anchors = h
        .site
        .search_anchors("搜索词".to_string(), 1)
        .await
        .expect("主播搜索");
    assert_eq!(anchors.items.len(), 2);
    assert!(anchors.items[0].live_status);
    assert!(!anchors.items[1].live_status);
}

#[tokio::test]
async fn list_failure_surfaces_as_api_error() {
    let h = harness();
    h.http.stub(
        "live.kuaishou.com",
        r#"{"result":10,"errorType":22,"data":{}}"#,
    );
    let err = h
        .site
        .get_recommend_rooms(1, None)
        .await
        .expect_err("result=10 必须报错");
    assert!(
        matches!(err, SliveError::Api { code: 10, .. }),
        "实际 {err:?}"
    );
}
