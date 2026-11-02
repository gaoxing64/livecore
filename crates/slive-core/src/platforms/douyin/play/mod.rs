//! 清晰度列表 / 播放地址（对照 Dart `getPlayQualities` / `getPlayUrls`，
//! `douyin_site.dart:948-1087`）。
//!
//! **纯本地计算，零网络请求**（Dart 亦然）：输入是 `detail.data`（`stream_url` 的
//! JSON String 通道）与 `quality.data`（地址数组的 JSON 串）。
//!
//! # 三条画质分支（顺序即 Dart 的 if/else 链，不可重排）
//! 1. `data` 为空 -> 空列表。房间下播、或 `web/enter` 被降级时 `data` 是 `{}`；
//!    Dart 曾直接下标取 `live_core_sdk_data` 导致连麦房间抛 `NoSuchMethodError`，
//!    整个直播间被判成「加载失败」。
//! 2. 画质列表（`pull_data.options.qualities`）为空 -> 直接用地址表
//!    （`flv_pull_url` + `hls_pull_url_map`）的键当画质名，按键数从高到低排档位。
//! 3. 有画质列表：`stream_data` 不以 `{` 开头 -> 优先按 `sdk_key`/名称匹配地址表，
//!    匹配不到再按 `level` **倒序索引**取值；
//!    否则解析 `stream_data` JSON，按 `sdk_key` 精确取 `main.flv` / `main.hls`。
//!
//! # hlsFirst
//! `set_site_attrs({"hlsFirst": true})` 时 hls 地址排在 flv **前**（App 的用户偏好，
//! 由 `LocalStorageService.kDouyinHlsFirst` 下发）。

mod branches;

use serde_json::Value;

use crate::error::SliveError;
use crate::models::{LivePlayQuality, LivePlayUrl, LiveRoomDetail};

use super::site::DouyinSite;
use super::util::{safe_json_decode, urls_from_json};

use branches::{from_levels, from_stream_data, from_url_tables};

/// F-07 清晰度列表。
pub(crate) fn fetch_qualities(
    site: &DouyinSite,
    detail: &LiveRoomDetail,
) -> Result<Vec<LivePlayQuality>, SliveError> {
    let Some(raw) = detail.data.as_deref().filter(|s| !s.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    // `data` 通道是 JSON 串；非法 JSON 视为「无数据」而非报错（风控可能截断）
    let Some(data) = safe_json_decode(raw).filter(|v| v.as_object().is_some_and(|o| !o.is_empty()))
    else {
        return Ok(Vec::new());
    };

    let hls_first = site.hls_first();
    let pull_data = data.pointer("/live_core_sdk_data/pull_data");
    let qualities = pull_data
        .and_then(|p| p.pointer("/options/qualities"))
        .and_then(Value::as_array)
        .map(|a| a.to_vec())
        .unwrap_or_default();
    let stream_data = pull_data
        .and_then(|p| p.get("stream_data"))
        .map(crate::util::value_to_string)
        .unwrap_or_default();
    let stream_data = stream_data.trim_start();

    let mut out = if qualities.is_empty() {
        from_url_tables(&data, hls_first)
    } else if !stream_data.starts_with('{') {
        from_levels(&data, &qualities, hls_first)
    } else {
        from_stream_data(&qualities, &stream_data, hls_first)
    };

    // 档位从高到低（Dart `sort((a, b) => b.sort.compareTo(a.sort))`）
    out.sort_by_key(|q| std::cmp::Reverse(q.sort));
    Ok(out)
}

/// F-08 播放地址：`urls = quality.data` 的 JSON 数组（Dart 是 `List<String>` 直传）。
pub(crate) fn fetch_urls(
    _site: &DouyinSite,
    quality: &LivePlayQuality,
) -> Result<LivePlayUrl, SliveError> {
    Ok(LivePlayUrl {
        urls: urls_from_json(&quality.data)?,
        headers: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn site_with_hls_first(hls_first: bool) -> DouyinSite {
        let site = DouyinSite::new();
        site.set_site_attrs(&json!({"hlsFirst": hls_first}).to_string())
            .expect("注入");
        site
    }

    fn detail_with(data: Value) -> LiveRoomDetail {
        LiveRoomDetail {
            room_id: "416144012050".into(),
            title: "t".into(),
            cover: String::new(),
            user_name: "u".into(),
            user_avatar: String::new(),
            online: 0,
            introduction: None,
            notice: None,
            status: true,
            data: Some(data.to_string()),
            danmaku_data: None,
            is_record: false,
            url: String::new(),
        }
    }

    #[test]
    fn empty_data_returns_empty_list_instead_of_error() {
        let site = DouyinSite::new();
        let mut detail = detail_with(json!({}));
        detail.data = Some("{}".to_string());
        assert_eq!(
            fetch_qualities(&site, &detail).expect("应返回空"),
            Vec::new()
        );

        detail.data = None;
        assert_eq!(
            fetch_qualities(&site, &detail).expect("应返回空"),
            Vec::new()
        );

        detail.data = Some("   ".to_string());
        assert_eq!(
            fetch_qualities(&site, &detail).expect("应返回空"),
            Vec::new()
        );
    }

    #[test]
    fn truncated_data_json_degrades_to_empty_list() {
        let site = DouyinSite::new();
        let mut detail = detail_with(json!({}));
        detail.data = Some(r#"{"live_core_sdk_data""#.to_string());
        assert_eq!(
            fetch_qualities(&site, &detail).expect("风控截断应降级而非报错"),
            Vec::new()
        );
    }

    #[test]
    fn url_table_branch_uses_keys_as_quality_names_descending() {
        let site = DouyinSite::new();
        let detail = detail_with(json!({
            "flv_pull_url": {"FULL_HD1": "https://flv1", "HD1": "https://flv2"},
            "hls_pull_url_map": {"FULL_HD1": "https://hls1", "HD1": "https://hls2"},
        }));
        let got = fetch_qualities(&site, &detail).expect("应能解析");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].quality, "FULL_HD1", "键序在前的档位更高");
        assert_eq!(got[0].sort, 2);
        assert_eq!(got[1].quality, "HD1");
        assert_eq!(got[1].sort, 1);
        // flv 在前（hlsFirst 默认 false）
        assert_eq!(got[0].data, r#"["https://flv1","https://hls1"]"#);
    }

    #[test]
    fn hls_first_swaps_url_order() {
        let site = site_with_hls_first(true);
        let detail = detail_with(json!({
            "flv_pull_url": {"FULL_HD1": "https://flv1"},
            "hls_pull_url_map": {"FULL_HD1": "https://hls1"},
        }));
        let got = fetch_qualities(&site, &detail).expect("应能解析");
        assert_eq!(got[0].data, r#"["https://hls1","https://flv1"]"#);
    }

    #[test]
    fn level_branch_indexes_url_lists_in_reverse() {
        let site = DouyinSite::new();
        let detail = detail_with(json!({
            "live_core_sdk_data": {"pull_data": {
                "options": {"qualities": [
                    {"name": "原画", "level": 1, "sdk_key": "FULL_HD1"},
                    {"name": "高清", "level": 2, "sdk_key": "HD1"},
                ]},
                "stream_data": "非 JSON 串",
            }},
            "flv_pull_url": {"A": "https://f1", "B": "https://f2"},
            "hls_pull_url_map": {"A": "https://h1", "B": "https://h2"},
        }));
        let got = fetch_qualities(&site, &detail).expect("应能解析");
        assert_eq!(got.len(), 2);
        // level=1 -> 索引 2-1=1 -> B；level=2 -> 索引 0 -> A
        assert_eq!(got[0].quality, "高清", "level 大者排前");
        assert_eq!(got[0].sort, 2);
        assert_eq!(got[0].data, r#"["https://f1","https://h1"]"#);
        assert_eq!(got[1].quality, "原画");
        assert_eq!(got[1].data, r#"["https://f2","https://h2"]"#);
    }

    #[test]
    fn stream_data_branch_reads_main_flv_and_hls_by_sdk_key() {
        let site = DouyinSite::new();
        let stream_data = json!({
            "data": {
                "FULL_HD1": {"main": {"flv": "https://sflv", "hls": "https://shls"}},
                "HD1": {"main": {"flv": "https://hflv"}},
            }
        })
        .to_string();
        let detail = detail_with(json!({
            "live_core_sdk_data": {"pull_data": {
                "options": {"qualities": [
                    {"name": "原画", "level": 1, "sdk_key": "FULL_HD1"},
                    {"name": "高清", "level": 2, "sdk_key": "HD1"},
                    {"name": "不存在的档", "level": 3, "sdk_key": "NOPE"},
                ]},
                "stream_data": stream_data,
            }},
        }));
        let got = fetch_qualities(&site, &detail).expect("应能解析");
        assert_eq!(got.len(), 2, "取不到地址的档位被丢弃（Dart 同口径）");
        assert_eq!(got[0].quality, "高清");
        assert_eq!(got[0].data, r#"["https://hflv"]"#);
        assert_eq!(got[1].quality, "原画");
        assert_eq!(got[1].data, r#"["https://sflv","https://shls"]"#);
    }

    #[test]
    fn stream_data_fills_origin_when_options_omit_it() {
        let site = DouyinSite::new();
        let stream_data = json!({
            "data": {
                "uhd": {"main": {"flv": "https://uhd.flv"}},
                "origin": {"main": {"flv": "https://origin.flv"}},
            }
        })
        .to_string();
        let detail = detail_with(json!({
            "live_core_sdk_data": {"pull_data": {
                "options": {"qualities": [
                    {"name": "蓝光", "level": 4, "sdk_key": "uhd"},
                ]},
                "stream_data": stream_data,
            }},
        }));

        let got = fetch_qualities(&site, &detail).expect("应能解析");
        assert_eq!(got.len(), 2);
        assert_eq!(
            got[0].quality, "原画",
            "接口漏报原画时应从 stream_data 补回"
        );
        assert_eq!(got[0].data, r#"["https://origin.flv"]"#);
        assert_eq!(got[1].quality, "蓝光");
    }

    #[test]
    fn stream_data_fills_origin_when_quality_item_has_no_url() {
        let site = DouyinSite::new();
        let stream_data = json!({
            "data": {
                "origin": {"main": {"flv": "https://origin.flv"}},
                "uhd": {"main": {"flv": "https://uhd.flv"}},
            }
        })
        .to_string();
        let detail = detail_with(json!({
            "live_core_sdk_data": {"pull_data": {
                "options": {"qualities": [
                    {"name": "原画", "level": 5, "sdk_key": "origin"},
                    {"name": "蓝光", "level": 4, "sdk_key": "uhd"},
                ]},
                "stream_data": stream_data,
            }},
        }));

        let got = fetch_qualities(&site, &detail).expect("应能解析");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].quality, "原画", "同名档位地址为空时仍应补回");
        assert_eq!(got[0].data, r#"["https://origin.flv"]"#);
    }

    #[test]
    fn stream_data_with_leading_whitespace_still_uses_json_branch() {
        let site = DouyinSite::new();
        let stream_data = format!(
            " \n{}",
            json!({
                "data": {
                    "origin": {"main": {"flv": "https://origin.flv"}},
                }
            })
        );
        let detail = detail_with(json!({
            "live_core_sdk_data": {"pull_data": {
                "options": {"qualities": [
                    {"name": "原画", "level": 5, "sdk_key": "origin"},
                ]},
                "stream_data": stream_data,
            }},
        }));

        let got = fetch_qualities(&site, &detail).expect("应能解析");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].quality, "原画");
        assert_eq!(got[0].data, r#"["https://origin.flv"]"#);
    }

    #[test]
    fn level_branch_prefers_sdk_key_over_misaligned_index() {
        let site = DouyinSite::new();
        let detail = detail_with(json!({
            "live_core_sdk_data": {"pull_data": {
                "options": {"qualities": [
                    {"name": "原画", "level": 0, "sdk_key": "FULL_HD1"},
                ]},
                "stream_data": "",
            }},
            "flv_pull_url": {"FULL_HD1": "https://origin.flv"},
        }));

        let got = fetch_qualities(&site, &detail).expect("应能解析");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].quality, "原画");
        assert_eq!(got[0].data, r#"["https://origin.flv"]"#);
    }

    #[test]
    fn level_branch_drops_known_key_missing_from_url_table() {
        let site = DouyinSite::new();
        let detail = detail_with(json!({
            "live_core_sdk_data": {"pull_data": {
                "options": {"qualities": [
                    {"name": "原画", "level": 0, "sdk_key": "FULL_HD1"},
                    {"name": "高清", "level": 1, "sdk_key": "HD1"},
                    {"name": "标清", "level": 2, "sdk_key": "SD1"},
                ]},
                "stream_data": "",
            }},
            "flv_pull_url": {
                "FULL_HD1": "https://origin.flv",
                "HD1": "https://hd.flv",
            },
        }));

        let got = fetch_qualities(&site, &detail).expect("应能解析");
        assert_eq!(
            got.iter()
                .map(|item| item.quality.as_str())
                .collect::<Vec<_>>(),
            vec!["高清", "原画"],
            "已知 sdk_key 缺失时不能按 level 错接到别的地址"
        );
    }

    #[test]
    fn stream_data_branch_accepts_case_and_key_aliases() {
        let site = DouyinSite::new();
        let stream_data = json!({
            "data": {
                "ORIGION": {"main": {"flv": "https://origin.flv"}},
                "FULL_HD1": {"main": {"flv": "https://uhd.flv"}},
                "HD1": {"main": {"flv": "https://hd.flv"}},
                "SD2": {"main": {"flv": "https://sd.flv"}},
                "SD1": {"main": {"flv": "https://ld.flv"}},
            }
        })
        .to_string();
        let detail = detail_with(json!({
            "live_core_sdk_data": {"pull_data": {
                "options": {"qualities": [
                    {"name": "标清", "level": 1, "sdk_key": "ld"},
                    {"name": "高清", "level": 2, "sdk_key": "sd"},
                    {"name": "超清", "level": 3, "sdk_key": "hd"},
                    {"name": "蓝光", "level": 4, "sdk_key": "uhd"},
                    {"name": "原画", "level": 5, "sdk_key": "origin"},
                ]},
                "stream_data": stream_data,
            }},
        }));

        let got = fetch_qualities(&site, &detail).expect("应能解析");
        assert_eq!(
            got.iter()
                .map(|item| item.quality.as_str())
                .collect::<Vec<_>>(),
            vec!["原画", "蓝光", "超清", "高清", "标清"]
        );
        assert_eq!(got[0].data, r#"["https://origin.flv"]"#);
        assert_eq!(got[1].data, r#"["https://uhd.flv"]"#);
        assert_eq!(got[2].data, r#"["https://hd.flv"]"#);
        assert_eq!(got[3].data, r#"["https://sd.flv"]"#);
        assert_eq!(got[4].data, r#"["https://ld.flv"]"#);
    }

    #[test]
    fn hls_first_in_stream_data_branch_puts_hls_in_front() {
        let site = site_with_hls_first(true);
        let stream_data =
            json!({"data": {"FULL_HD1": {"main": {"flv": "https://f", "hls": "https://h"}}}})
                .to_string();
        let detail = detail_with(json!({
            "live_core_sdk_data": {"pull_data": {
                "options": {"qualities": [{"name": "原画", "level": 1, "sdk_key": "FULL_HD1"}]},
                "stream_data": stream_data,
            }},
        }));
        let got = fetch_qualities(&site, &detail).expect("应能解析");
        assert_eq!(got[0].data, r#"["https://h","https://f"]"#);
    }

    #[test]
    fn play_urls_decode_json_channel() {
        let site = DouyinSite::new();
        let quality = LivePlayQuality {
            quality: "原画".into(),
            data: r#"["https://a.flv","https://a.m3u8"]"#.into(),
            sort: 1,
        };
        let url = fetch_urls(&site, &quality).expect("应能解析");
        assert_eq!(url.urls, vec!["https://a.flv", "https://a.m3u8"]);
        assert!(url.headers.is_none(), "抖音取流不需要自定义头");
    }

    #[test]
    fn play_urls_reject_malformed_channel() {
        let site = DouyinSite::new();
        let quality = LivePlayQuality {
            quality: "原画".into(),
            data: "not json".into(),
            sort: 1,
        };
        assert!(matches!(
            fetch_urls(&site, &quality),
            Err(SliveError::Decode(_))
        ));
    }

    #[test]
    fn play_urls_tolerate_empty_channel() {
        let site = DouyinSite::new();
        let quality = LivePlayQuality {
            quality: "原画".into(),
            data: String::new(),
            sort: 1,
        };
        assert_eq!(
            fetch_urls(&site, &quality).expect("空串视为空列表").urls,
            Vec::<String>::new()
        );
    }
}
