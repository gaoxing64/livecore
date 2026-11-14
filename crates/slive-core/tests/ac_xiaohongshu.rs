//! 小红书离线契约测试。

use std::sync::Arc;

use serde_json::json;
use slive_core::error::SliveError;
use slive_core::platforms::xiaohongshu::XiaohongshuSite;
use slive_core::platforms::LiveSite;
use slive_core::testing::{FakeTime, MockTransport};

const ROOM: &str = "570541076495019830";

fn harness() -> (XiaohongshuSite, Arc<MockTransport>) {
    let http = Arc::new(MockTransport::new());
    let site = XiaohongshuSite::with_deps(http.clone(), Arc::new(FakeTime::new(1_700_000_000)));
    (site, http)
}

fn page(info: serde_json::Value, live_status: &str) -> String {
    let state = json!({
        "liveStream": {
            "pageStatus": "success",
            "liveStatus": live_status,
            "roomData": {
                "roomInfo": info,
                "hostInfo": {"nickName": "冯子涵", "avatar": "https://cdn/a.jpg"}
            }
        }
    });
    format!("<html><script>window.__INITIAL_STATE__={state};</script></html>")
}

fn live_info() -> serde_json::Value {
    json!({
        "roomId": ROOM,
        "status": 2,
        "monetizeType": 0,
        "joinLimitTypes": [0],
        "roomTitle": "地铁站捞人",
        "roomCover": "https://cdn/c.jpg",
        "displayViewerCount": "1千+",
        "pullConfig": json!({
            "h264": [
                {"master_url": format!("http://live-source-play.xhscdn.com/live/{ROOM}.m3u8"), "quality_type": "HD", "quality_type_name": "原画"},
                {"master_url": format!("http://live-source-play.xhscdn.com/live/{ROOM}.flv"), "quality_type": "HD", "quality_type_name": "原画"}
            ],
            "h265": [
                {"master_url": format!("https://live-source-play.xhscdn.com/live/{ROOM}.flv"), "quality_type": "HD", "quality_type_name": "原画"}
            ]
        })
        .to_string()
    })
}

#[tokio::test]
async fn share_page_builds_detail_and_merged_qualities() {
    let (site, http) = harness();
    http.stub("xiaohongshu.com/livestream/", &page(live_info(), "success"));

    let detail = site.get_room_detail(ROOM.to_string()).await.expect("详情");
    assert_eq!(detail.title, "地铁站捞人");
    assert_eq!(detail.user_name, "冯子涵");
    assert!(detail.status);
    assert!(detail.notice.as_deref().unwrap_or("").contains("1千+"));

    let qualities = site
        .get_play_qualities(detail.clone())
        .await
        .expect("画质");
    assert_eq!(qualities.len(), 2, "同档同编码合并；H.265 单独一档");
    assert_eq!(qualities[0].quality, "原画");
    assert!(
        qualities[0].data.ends_with(".m3u8"),
        "H.264 保留第一条地址：{}",
        qualities[0].data
    );
    assert_eq!(qualities[1].quality, "原画 · H.265");
    assert_eq!(http.requests().len(), 1, "详情已带解析结果，不再读页");

    let play = site
        .get_play_urls(detail, qualities[0].clone())
        .await
        .expect("直链");
    assert_eq!(play.urls.len(), 1);
    let headers = play.headers.expect("媒体请求头");
    assert!(headers.keys().any(|key| key.eq_ignore_ascii_case("referer")));
}

#[tokio::test]
async fn ended_page_is_offline_and_unplayable() {
    let (site, http) = harness();
    let info = json!({"roomId": ROOM, "status": 3, "monetizeType": 0, "joinLimitTypes": [0]});
    http.stub("xiaohongshu.com/livestream/", &page(info, "end"));

    let detail = site.get_room_detail(ROOM.to_string()).await.expect("详情");
    assert!(!detail.status);
    assert!(!site.get_live_status(ROOM.to_string()).await.expect("状态"));
    assert!(site.get_play_qualities(detail).await.is_err());
}

#[tokio::test]
async fn restricted_page_reports_why() {
    let (site, http) = harness();
    let info = json!({"roomId": ROOM, "status": 2, "monetizeType": 1, "joinLimitTypes": [0]});
    http.stub("xiaohongshu.com/livestream/", &page(info, "success"));

    let detail = site.get_room_detail(ROOM.to_string()).await.expect("详情");
    let err = site.get_play_qualities(detail).await.expect_err("付费不可播");
    let message = format!("{err:?}");
    assert!(message.contains("付费"), "实际 {message}");
    assert!(message.contains("观看条件"), "实际 {message}");
}

#[tokio::test]
async fn invalid_or_unknown_room_is_rejected() {
    let (site, http) = harness();
    let err = site.get_room_detail("not-a-room".to_string()).await.expect_err("非法 id");
    assert!(matches!(err, SliveError::NotFound(_)));
    assert_eq!(http.requests().len(), 0);

    http.stub(
        "xiaohongshu.com/livestream/",
        r#"<script>window.__INITIAL_STATE__={"liveStream":{"pageStatus":"error","errorMessage":"未找到直播间"}};</script>"#,
    );
    let err = site.get_room_detail(ROOM.to_string()).await.expect_err("页面错误");
    assert!(matches!(err, SliveError::NotFound(_)), "实际 {err:?}");
}

#[tokio::test]
async fn directory_and_search_are_empty_without_a_browser_session() {
    let (site, http) = harness();
    assert!(site.get_categories().await.expect("分类").is_empty());
    assert!(site
        .get_recommend_rooms(1, None)
        .await
        .expect("推荐")
        .items
        .is_empty());
    assert!(site
        .search_rooms("x".to_string(), 1)
        .await
        .expect("搜索")
        .items
        .is_empty());
    assert_eq!(http.requests().len(), 0, "这些能力不发请求");
}
