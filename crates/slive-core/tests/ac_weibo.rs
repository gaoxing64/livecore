//! 微博直播离线契约测试。

use std::sync::Arc;

use serde_json::json;
use slive_core::error::SliveError;
use slive_core::models::LiveSubCategory;
use slive_core::platforms::weibo::WeiboSite;
use slive_core::platforms::LiveSite;
use slive_core::testing::{FakeTime, MockTransport};

const ID: &str = "1022:2321325347771573207158";

fn harness() -> (WeiboSite, Arc<MockTransport>) {
    let http = Arc::new(MockTransport::new());
    let site = WeiboSite::with_deps(http.clone(), Arc::new(FakeTime::new(1_700_000_000)));
    (site, http)
}

fn snapshot() -> String {
    json!({
        "code": 100000,
        "error_code": 0,
        "data": {"data": [
            {"liveid": ID, "nickname": "十二青苏", "cover": "http://cdn/a.jpg"},
            {"liveid": "1022:2321325347904448757771", "nickname": "另一个人", "cover": "https://cdn/b.jpg"}
        ]}
    })
    .to_string()
}

fn detail(extra: serde_json::Value) -> String {
    let mut data = json!({
        "liveId": ID,
        "status": 1,
        "watch_limit": 0,
        "play_switch": 1,
        "title": "标题",
        "cover": "https://cdn/cover.jpg",
        "user": {"uid": 42, "screenName": "主播", "avatar": "https://cdn/a.jpg"},
        "live_origin_flv_url": "https://cdn/live.flv",
        "live_origin_hls_url": "https://cdn/live.flv"
    });
    if let Some(extra) = extra.as_object() {
        for (key, value) in extra {
            data[key] = value.clone();
        }
    }
    json!({"code": 100000, "error_code": 0, "data": data}).to_string()
}

#[tokio::test]
async fn categories_are_one_area_without_a_request() {
    let (site, http) = harness();
    let categories = site.get_categories().await.expect("分类");
    assert_eq!(categories.len(), 1);
    assert_eq!(categories[0].name, "微博直播");
    assert_eq!(categories[0].children.len(), 1);
    assert_eq!(categories[0].children[0].id, "live");
    assert_eq!(http.requests().len(), 0);
}

#[tokio::test]
async fn recommend_and_search_use_the_snapshot() {
    let (site, http) = harness();
    http.stub("pc_recommend", &snapshot());

    let rooms = site.get_recommend_rooms(1, None).await.expect("推荐");
    assert_eq!(rooms.items.len(), 2);
    assert_eq!(rooms.items[0].room_id, ID);
    assert_eq!(rooms.items[0].cover, "https://cdn/a.jpg");
    assert_eq!(rooms.items[0].online, 0, "微博不提供观众数");

    let found = site.search_rooms("青苏".to_string(), 1).await.expect("搜索");
    assert_eq!(found.items.len(), 1);
    assert_eq!(found.items[0].user_name, "十二青苏");

    let by_id = site.search_rooms(ID.to_string(), 1).await.expect("按 id");
    assert_eq!(by_id.items.len(), 1);

    assert!(site
        .get_recommend_rooms(2, None)
        .await
        .expect("第二页")
        .items
        .is_empty());

    assert!(site
        .search_anchors("另一个人".to_string(), 1)
        .await
        .expect("主播搜索")
        .items
        .iter()
        .all(|anchor| anchor.live_status));

    let before = http.requests().len();
    assert!(site
        .search_rooms("   ".to_string(), 1)
        .await
        .expect("空关键词")
        .items
        .is_empty());
    assert_eq!(http.requests().len(), before, "空关键词不发请求");
}

#[tokio::test]
async fn area_rooms_reject_other_areas() {
    let (site, _http) = harness();
    let err = site
        .get_category_rooms(
            LiveSubCategory {
                name: "x".to_string(),
                pic: None,
                id: "other".to_string(),
                parent_id: "recommendation".to_string(),
            },
            1,
            None,
        )
        .await
        .expect_err("别的区域");
    assert!(matches!(err, SliveError::InvalidInput(_)));
}

#[tokio::test]
async fn live_detail_and_quality_refetch_the_broadcast() {
    let (site, http) = harness();
    http.stub("show_pc_live", &detail(json!({})));

    let detail = site.get_room_detail(ID.to_string()).await.expect("详情");
    assert_eq!(detail.title, "标题");
    assert_eq!(detail.user_name, "主播");
    assert!(detail.status);
    assert!(detail.notice.as_deref().unwrap_or("").contains("微博每场直播"));

    let qualities = site
        .get_play_qualities(detail.clone())
        .await
        .expect("画质");
    assert_eq!(qualities.len(), 1);
    assert_eq!(qualities[0].quality, "原始流");
    assert_eq!(qualities[0].data, "https://cdn/live.flv");

    let play = site
        .get_play_urls(detail, qualities[0].clone())
        .await
        .expect("直链");
    assert_eq!(play.urls, vec!["https://cdn/live.flv"]);
    assert!(play.headers.is_none(), "微博媒体 CDN 不需要请求头");
    assert_eq!(http.requests().len(), 2, "详情 + 播放各读一次");
}

#[tokio::test]
async fn restricted_broadcast_has_no_quality() {
    let (site, http) = harness();
    http.stub(
        "show_pc_live",
        &detail(json!({"watch_limit": 10, "pay_dialog_info": {"buy_tip": "仅好友"}})),
    );
    let detail = site.get_room_detail(ID.to_string()).await.expect("详情");
    let err = site
        .get_play_qualities(detail)
        .await
        .expect_err("受限不可播");
    let message = format!("{err:?}");
    assert!(message.contains("受限"), "实际 {message}");
}

#[tokio::test]
async fn missing_broadcast_is_not_found() {
    let (site, http) = harness();
    http.stub(
        "show_pc_live",
        &json!({"code": 100000, "error_code": 27401, "data": null}).to_string(),
    );
    let err = site.get_room_detail(ID.to_string()).await.expect_err("不存在");
    assert!(matches!(err, SliveError::NotFound(_)), "实际 {err:?}");
}

#[tokio::test]
async fn invalid_id_is_rejected_without_a_request() {
    let (site, http) = harness();
    let err = site.get_room_detail("not-an-id".to_string()).await.expect_err("非法");
    assert!(matches!(err, SliveError::NotFound(_)));
    assert_eq!(http.requests().len(), 0);
}
