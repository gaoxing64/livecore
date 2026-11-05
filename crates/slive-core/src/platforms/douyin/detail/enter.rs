//! `web/enter/` 链路（webRid -> 房间；首页 / 推荐 / 关注列表的主路径）。
//!
//! 对照 Dart `_getRoomDataByApi` + `_getRoomDetailByWebRidApi`
//! （`douyin_site.dart:646-724`、`909-928`）。
//!
//! ## 「降级骨架」补救（本文件存在的核心理由）
//! `web/enter` 对**连麦 / 多人连线**房间会返回「降级房间」：`room` 数据在，
//! 但 `status != 2`、也没有 `stream_url`。首页 / 推荐 / 关注列表下发的 `roomId` 全是
//! webRid、都走这条链路，不做补救则这些入口进连麦房间会被判成「未开机播」。
//!
//! 补救方式：用 enter 返回的 `roomId` 再查一次 `reflow/info`，**整体采信**其结果
//! （那里能拿到 status / stream_url / 画质列表）。
//!
//! ⚠️ 不能改调 `getRoomDetailByRoomId` 来做补救：它在 `status == 4` 时会回调
//! `getRoomDetailByWebRid`，而调用方**正是** `getRoomDetailByWebRid` —— 会无限递归。

use crate::error::SliveError;
use crate::models::LiveRoomDetail;

use super::super::signing::build_signed_url;
use super::super::site::DouyinSite;
use super::super::util::as_object;
use super::builder::build_detail_from_enter;
use super::reflow::fetch_reflow_room;

/// `web/enter/` 接口（webRid -> 房间）。
pub const ENTER_URL: &str = "https://live.douyin.com/webcast/room/web/enter/";

/// 通过 `web/enter` 获取详情；降级骨架时整体采信 `reflow/info` 的结果。
pub(crate) async fn fetch_by_web_rid_api(
    site: &DouyinSite,
    web_rid: &str,
) -> Result<LiveRoomDetail, SliveError> {
    super::super::header::ensure_ttwid(site).await?;
    let url = build_signed_url(ENTER_URL, &enter_params(web_rid))?;
    let value = site
        .get_json(&url, Vec::new(), super::super::header::build_header(site))
        .await?;

    let room = crate::util::array_at(&value, "/data/data")
        .first()
        .and_then(|v| as_object(Some(v)))
        .ok_or_else(|| SliveError::decode("抖音进房接口未返回房间数据".to_string()))?;

    let room_id = room
        .get("id_str")
        .map(crate::util::value_to_string)
        .unwrap_or_default();
    let stream_empty = as_object(room.get("stream_url"))
        .map(|s| s.is_empty())
        .unwrap_or(true);

    if stream_empty && !room_id.is_empty() {
        if let Some(reflow_room) = fetch_reflow_room(site, &room_id).await {
            let has_stream = as_object(reflow_room.get("stream_url"))
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            if has_stream {
                return Ok(super::builder::build_detail_from_reflow_room(
                    site,
                    &reflow_room,
                    &room_id,
                    web_rid,
                ));
            }
        }
    }

    let detail = build_detail_from_enter(
        site,
        room,
        as_object(value.pointer("/data/user")),
        web_rid,
        &room_id,
    );
    Ok(super::ensure_origin_quality(site, detail, web_rid, &room_id).await)
}

/// `web/enter` 的业务参数（固定 5 个键，插入序参与 a_bogus 签名）。
///
/// 抽成独立函数是为了**参数级防回归**：合成夹具只校验响应解析，校验不到请求参数
/// （2026-10-16 真机缺陷：feed/partition 漏 `aid=6383` 直接 400，夹具全绿）。
/// 故每个键都由 `enter_params_carry_required_keys` 钉住。
pub fn enter_params(web_rid: &str) -> super::super::util::QueryParams {
    let mut params = super::super::util::QueryParams::new();
    params
        .put("app_name", "douyin_web")
        .put("enter_from", "web_live")
        .put("live_id", "1")
        .put("web_rid", web_rid)
        .put("is_need_double_stream", "false");
    params
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use crate::testing::MockTransport;
    use crate::time::SystemClock;

    use super::*;

    /// 5 档 options；`stream_data` 由用例决定（空串 = 旧地址表分支）。
    fn enter_body(stream_data: &str) -> String {
        json!({
            "data": {
                "data": [{
                    "id_str": "7704356715309222696",
                    "status": 2,
                    "title": "测试直播间",
                    "owner": {"nickname": "主播", "avatar_thumb": {"url_list": []}},
                    "room_view_stats": {"display_value": 1},
                    "stream_url": {
                        "flv_pull_url": {
                            "FULL_HD1": "https://x_uhd.flv",
                            "HD1": "https://x_hd.flv",
                            "SD1": "https://x_ld.flv",
                            "SD2": "https://x_sd.flv"
                        },
                        "live_core_sdk_data": {"pull_data": {
                            "options": {"qualities": [
                                {"name": "标清", "level": 1, "sdk_key": "ld"},
                                {"name": "高清", "level": 2, "sdk_key": "sd"},
                                {"name": "超清", "level": 3, "sdk_key": "hd"},
                                {"name": "蓝光", "level": 4, "sdk_key": "uhd"},
                                {"name": "原画", "level": 5, "sdk_key": "origin"}
                            ]},
                            "stream_data": stream_data
                        }}
                    }
                }],
                "user": {"nickname": "主播"}
            }
        })
        .to_string()
    }

    /// reflow/info 的完整阶梯（含 origin 地址）。
    fn reflow_body() -> String {
        let stream_data = json!({
            "data": {
                "origin": {"main": {"flv": "https://origin.flv"}},
                "uhd": {"main": {"flv": "https://uhd.flv"}},
                "hd": {"main": {"flv": "https://hd.flv"}},
                "sd": {"main": {"flv": "https://sd.flv"}},
                "ld": {"main": {"flv": "https://ld.flv"}}
            }
        })
        .to_string();
        let mut body: serde_json::Value = serde_json::from_str(&enter_body(&stream_data)).expect("enter");
        let room = &mut body["data"]["data"][0];
        room["owner"]["web_rid"] = json!("416144012050");
        json!({"data": {"room": room.clone(), "user": {}}}).to_string()
    }

    fn site_with(mock: &Arc<MockTransport>) -> DouyinSite {
        DouyinSite::with_deps(mock.clone(), Arc::new(SystemClock))
    }

    fn stub_homepage(mock: &MockTransport) {
        mock.stub_with_cookie(
            "https://live.douyin.com",
            "{}",
            vec!["ttwid=1%2Cabc; Path=/".to_string()],
        );
    }

    #[tokio::test]
    async fn enter_without_origin_falls_back_to_reflow() {
        let mock = Arc::new(MockTransport::new());
        mock.stub("webcast/room/web/enter", &enter_body(""));
        mock.stub("reflow/info", &reflow_body());
        stub_homepage(&mock);
        let site = site_with(&mock);

        let detail = fetch_by_web_rid_api(&site, "416144012050").await.expect("详情");
        let qualities = crate::platforms::douyin::play::fetch_qualities(&site, &detail).expect("画质");
        assert!(
            qualities.iter().any(|item| item.quality == "原画"),
            "缺少原画的 enter 结果应由 reflow 补全：{qualities:?}"
        );
        assert_eq!(qualities[0].quality, "原画");
        assert_eq!(qualities[0].data, r#"["https://origin.flv"]"#);
        assert_eq!(mock.request_count("reflow/info"), 1);
    }

    /// 真正只有 4 档的房间（有合法 stream_data、没有 origin）不应被“补全”打扰。
    #[tokio::test]
    async fn genuine_four_tier_room_does_not_hit_reflow() {
        let stream_data = json!({
            "data": {
                "uhd": {"main": {"flv": "https://uhd.flv"}},
                "hd": {"main": {"flv": "https://hd.flv"}},
                "sd": {"main": {"flv": "https://sd.flv"}},
                "ld": {"main": {"flv": "https://ld.flv"}}
            }
        })
        .to_string();
        let mut body: serde_json::Value =
            serde_json::from_str(&enter_body(&stream_data)).expect("enter");
        body["data"]["data"][0]["stream_url"]["live_core_sdk_data"]["pull_data"]["options"]
            ["qualities"]
            .as_array_mut()
            .expect("qualities")
            .pop();
        let mock = Arc::new(MockTransport::new());
        mock.stub("webcast/room/web/enter", &body.to_string());
        mock.stub("reflow/info", &reflow_body());
        stub_homepage(&mock);
        let site = site_with(&mock);

        let detail = fetch_by_web_rid_api(&site, "416144012050").await.expect("详情");
        let qualities = crate::platforms::douyin::play::fetch_qualities(&site, &detail).expect("画质");
        assert_eq!(qualities.len(), 4);
        assert!(!qualities.iter().any(|item| item.quality == "原画"));
        assert_eq!(
            mock.request_count("reflow/info"),
            0,
            "房间本来就没有原画时不应多打请求"
        );
    }

    #[tokio::test]
    async fn complete_enter_result_does_not_hit_reflow() {
        let stream_data = json!({
            "data": {
                "origin": {"main": {"flv": "https://origin.flv"}},
                "uhd": {"main": {"flv": "https://uhd.flv"}},
                "hd": {"main": {"flv": "https://hd.flv"}},
                "sd": {"main": {"flv": "https://sd.flv"}},
                "ld": {"main": {"flv": "https://ld.flv"}}
            }
        })
        .to_string();
        let mock = Arc::new(MockTransport::new());
        mock.stub("webcast/room/web/enter", &enter_body(&stream_data));
        mock.stub("reflow/info", &reflow_body());
        stub_homepage(&mock);
        let site = site_with(&mock);

        let detail = fetch_by_web_rid_api(&site, "416144012050").await.expect("详情");
        let qualities = crate::platforms::douyin::play::fetch_qualities(&site, &detail).expect("画质");
        assert_eq!(qualities[0].quality, "原画");
        assert_eq!(
            mock.request_count("reflow/info"),
            0,
            "完整响应不应额外请求 reflow"
        );
    }

    #[test]
    fn enter_params_carry_required_keys() {
        let params = enter_params("416144012050");
        let keys: Vec<&str> = params.iter().map(|(k, _)| k).collect();
        assert_eq!(
            keys,
            vec!["app_name", "enter_from", "live_id", "web_rid", "is_need_double_stream"],
            "顺序参与 a_bogus 签名，不可重排"
        );
        let map: std::collections::HashMap<_, _> = params.iter().collect();
        assert_eq!(map["app_name"], "douyin_web");
        assert_eq!(map["enter_from"], "web_live");
        assert_eq!(map["live_id"], "1");
        assert_eq!(map["web_rid"], "416144012050");
        assert_eq!(map["is_need_double_stream"], "false");
    }

    #[test]
    fn enter_url_gets_common_params_and_a_bogus_appended() {
        // `build_signed_url` 必须补上 Dart `buildRequestUrl` 的 7 个公共键 + msToken + a_bogus。
        // 缺任何一个都会让签名或服务端校验失败（aid 缺失 -> 真机 400）。
        let url = crate::platforms::douyin::signing::build_signed_url(
            ENTER_URL,
            &enter_params("416144012050"),
        )
        .expect("应能构造");
        for key in [
            "app_name=douyin_web",
            "enter_from=web_live",
            "web_rid=416144012050",
            "aid=6383",
            "compress=gzip",
            "device_platform=web",
            "browser_name=Edge",
            "browser_version=125.0.0.0",
            "msToken=",
            "a_bogus=",
        ] {
            assert!(url.contains(key), "URL 缺少 {key}：{url}");
        }
    }

    #[test]
    fn empty_data_list_is_decode_error_not_panic() {
        // 该错误是「刻意」的：上层据此回退 HTML 链路而非再打重复请求
        let err = SliveError::decode("抖音进房接口未返回房间数据".to_string());
        assert!(matches!(err, SliveError::Decode(_)));
    }

    #[test]
    fn stream_url_emptiness_detects_degraded_skeleton() {
        let skeleton = json!({"status": 4});
        assert!(as_object(skeleton.get("stream_url")).map(|s| s.is_empty()).unwrap_or(true));
        let full = json!({"stream_url": {"flv_pull_url": {}}});
        assert!(!as_object(full.get("stream_url")).map(|s| s.is_empty()).unwrap_or(true));
    }
}
