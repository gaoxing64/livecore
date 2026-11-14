//! YouTube 离线契约测试。

use std::sync::Arc;

use serde_json::json;
use slive_core::error::SliveError;
use slive_core::models::LiveSubCategory;
use slive_core::platforms::youtube::YouTubeSite;
use slive_core::platforms::LiveSite;
use slive_core::testing::{FakeTime, MockTransport};

const FIXED_WTS: u64 = 1_700_000_000;

fn harness() -> (YouTubeSite, Arc<MockTransport>) {
    let http = Arc::new(MockTransport::new());
    let site = YouTubeSite::with_deps(http.clone(), Arc::new(FakeTime::new(FIXED_WTS)));
    (site, http)
}

fn live_row(video_id: &str, channel_id: &str, owner: &str, title: &str, watching: &str) -> serde_json::Value {
    json!({
        "videoRenderer": {
            "videoId": video_id,
            "title": {"runs": [{"text": title}]},
            "ownerText": {"runs": [{
                "text": owner,
                "navigationEndpoint": {"browseEndpoint": {"browseId": channel_id}}
            }]},
            "thumbnail": {"thumbnails": [{"url": "https://cdn/cover.jpg"}]},
            "channelThumbnailSupportedRenderers": {"channelThumbnailWithLinkRenderer": {
                "thumbnail": {"thumbnails": [{"url": "https://cdn/avatar.jpg"}]}
            }},
            "viewCountText": {"runs": [{"text": watching}]},
            "thumbnailOverlays": [{"thumbnailOverlayTimeStatusRenderer": {"style": "LIVE"}}]
        }
    })
}

#[tokio::test]
async fn recommend_rooms_read_the_live_destination() {
    let (site, http) = harness();
    let body = json!({
        "contents": [live_row("DxgDeW-gIcU", "UC1234567890123456789012", "HotSpanish", "LIVE NOW", "13,049 watching")]
    })
    .to_string();
    http.stub("youtubei/v1/browse", &body);

    let result = site.get_recommend_rooms(1, Some(10)).await.expect("推荐");
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].room_id, "UC1234567890123456789012", "房间是频道 id");
    assert_eq!(result.items[0].user_name, "HotSpanish");
    assert_eq!(result.items[0].online, 13049);
    assert!(!result.has_more, "Live 目的地只有一页");

    assert!(site
        .get_recommend_rooms(2, None)
        .await
        .expect("第二页")
        .items
        .is_empty());
}

#[tokio::test]
async fn search_pages_follow_the_continuation() {
    let (site, http) = harness();
    let first = json!({
        "contents": [live_row("AAAAAAAAAAA", "UC1234567890123456789012", "A", "one", "10 watching")],
        "continuationItemRenderer": {
            "continuationEndpoint": {"continuationCommand": {"token": "TOKEN1"}}
        }
    })
    .to_string();
    http.stub("youtubei/v1/search", &first);

    let page1 = site.search_rooms("lofi".to_string(), 1).await.expect("第一页");
    assert_eq!(page1.items.len(), 1);
    assert_eq!(page1.items[0].room_id, "UC1234567890123456789012");
    assert!(page1.has_more, "有 continuation 即有下一页");

    // 第二页会带 continuation 请求同一端点；mock 仍返回第一页，故仍有下一页。
    let page2 = site.search_rooms("lofi".to_string(), 2).await.expect("第二页");
    assert_eq!(page2.items.len(), 1, "第二页确实沿 cursor 发出了请求");

    // 没有记录的 cursor（例如跳过第 2 页直接取第 3 页）时返回空页且不发请求。
    let page5 = site.search_rooms("lofi".to_string(), 5).await.expect("第五页");
    assert!(page5.items.is_empty(), "没有记录的 cursor 即空页");
}

#[tokio::test]
async fn blank_keyword_and_unsupported_surfaces() {
    let (site, http) = harness();
    assert!(site
        .search_rooms("   ".to_string(), 1)
        .await
        .expect("空关键词")
        .items
        .is_empty());
    assert_eq!(http.requests().len(), 0, "空关键词不发请求");

    assert!(site.get_categories().await.expect("无分区").is_empty());
    assert!(matches!(
        site.get_category_rooms(
            LiveSubCategory {
                id: "x".to_string(),
                name: "x".to_string(),
                pic: None,
                parent_id: String::new(),
            },
            1,
            None
        )
        .await,
        Err(SliveError::InvalidInput(_))
    ));
    assert!(matches!(
        site.set_site_attrs("{}").err(),
        Some(SliveError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn room_detail_rejects_non_youtube_ids() {
    let (site, _http) = harness();
    let err = site
        .get_room_detail("channel".to_string())
        .await
        .expect_err("不是频道/视频 id");
    assert!(matches!(err, SliveError::NotFound(_)), "实际 {err:?}");
}

#[tokio::test]
async fn room_detail_follows_resolve_to_the_channel_live_tab() {
    let (site, http) = harness();
    let resolved = json!({"endpoint": {"browseEndpoint": {
        "browseId": "UC1234567890123456789012",
        "params": "EgRsaXZl"
    }}})
    .to_string();
    let page = json!({
        "contents": [live_row("CCCCCCCCCCC", "UC1234567890123456789012", "Chan", "live title", "777 watching")]
    })
    .to_string();
    http.stub("navigation/resolve_url", &resolved);
    http.stub("youtubei/v1/browse", &page);

    let detail = site
        .get_room_detail("UC1234567890123456789012".to_string())
        .await
        .expect("频道详情");
    assert_eq!(detail.room_id, "UC1234567890123456789012");
    assert_eq!(detail.title, "live title");
    assert_eq!(detail.online, 777);
    assert!(detail.status);
    assert_eq!(
        detail.danmaku_data.as_deref(),
        Some("CCCCCCCCCCC"),
        "弹幕/取流用当前直播视频 id"
    );
    assert!(detail.url.contains("watch?v=CCCCCCCCCCC"));
    assert!(detail
        .data
        .as_deref()
        .unwrap_or("")
        .contains("\"video_id\":\"CCCCCCCCCCC\""));
}

#[tokio::test]
async fn player_login_required_is_reported_honestly() {
    let (site, http) = harness();
    let resolved = json!({"endpoint": {"browseEndpoint": {"browseId": "UC1234567890123456789012"}}})
        .to_string();
    let page = json!({
        "contents": [live_row("CCCCCCCCCCC", "UC1234567890123456789012", "Chan", "live", "7 watching")]
    })
    .to_string();
    let watch = json!({"INNERTUBE_API_KEY": "AIzaTESTKEY"}).to_string();
    let player = json!({"playabilityStatus": {"status": "LOGIN_REQUIRED", "reason": "Sign in to confirm you’re not a bot"}})
        .to_string();

    http.stub("navigation/resolve_url", &resolved);
    http.stub("youtubei/v1/browse", &page);
    http.stub("youtube.com/watch", &watch);
    http.stub("youtubei/v1/player", &player);

    let detail = site
        .get_room_detail("UC1234567890123456789012".to_string())
        .await
        .expect("详情可用");
    let err = site
        .get_play_qualities(detail)
        .await
        .expect_err("匿名取流应报错");
    let message = format!("{err:?}");
    assert!(message.contains("LOGIN_REQUIRED"), "实际 {message}");
}
