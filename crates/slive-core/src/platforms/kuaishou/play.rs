//! 快手取流与画质解析（对照 PureLive `kuaishou_api.dart:665-760`）。
//!
//! `playUrls` 可能是描述符数组（房间页与首页卡片）或 `{h264, hevc}` 对象；
//! 每个描述符的实际档位在 `adaptationSet.representation[]`：
//! - 每个描述符取 `h264`/`avc`/`hevc`/`h265` 中第一个有档位的键，否则用描述符本身；
//! - 跳过 `hidden` 与非 http(s) 地址；
//! - 档位名 `name` → `shortName` → `qualityType` → `清晰度 {sort}`，`sort` 取 `level` → `bitrate`；
//! - 同名同 sort 合并，URL 去重；H.264 集里缺席的 H.265 档位加 “ · H.265” 后缀，
//!   `prefer_h264` 时排在所有 H.264 档位之后。

use std::cmp::Ordering;

use serde_json::Value;

use crate::error::SliveError;
use crate::models::*;

/// H.265 专属档位的名称后缀（PureLive `_hevcSuffix`）。
const HEVC_SUFFIX: &str = " · H.265";

struct Tier {
    name: String,
    sort: i32,
    urls: Vec<String>,
    hevc_only: bool,
    order: usize,
}

/// 描述符里的档位列表。
fn representations_of(value: Option<&Value>) -> Vec<&Value> {
    let Some(map) = value.and_then(Value::as_object) else {
        return Vec::new();
    };
    map.get("adaptationSet")
        .and_then(Value::as_object)
        .and_then(|set| set.get("representation"))
        .or_else(|| map.get("representation"))
        .and_then(Value::as_array)
        .map(|list| list.iter().collect())
        .unwrap_or_default()
}

fn add_tiers(tiers: &mut Vec<Tier>, representations: &[&Value], hevc_only: bool) {
    for item in representations {
        let Some(map) = item.as_object() else {
            continue;
        };
        if map.get("hidden").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        let url = map
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            continue;
        }
        let sort = map
            .get("level")
            .and_then(Value::as_i64)
            .or_else(|| map.get("bitrate").and_then(Value::as_i64))
            .unwrap_or(0) as i32;
        let label = |key: &str| {
            map.get(key)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        let name = label("name")
            .or_else(|| label("shortName"))
            .or_else(|| label("qualityType"))
            .unwrap_or_else(|| format!("清晰度 {sort}"));
        let name = if hevc_only {
            format!("{name}{HEVC_SUFFIX}")
        } else {
            name
        };
        if let Some(tier) = tiers
            .iter_mut()
            .find(|tier| tier.name == name && tier.sort == sort)
        {
            if !tier.urls.contains(&url) {
                tier.urls.push(url);
            }
        } else {
            let order = tiers.len();
            tiers.push(Tier {
                name,
                sort,
                urls: vec![url],
                hevc_only,
                order,
            });
        }
    }
}

/// 解析画质列表。
///
/// 一个档位可能有多条 CDN 线路，`LivePlayQuality.data` 只承载第一条
/// （core 契约是单串）；其余线路在当前版本丢弃。
pub fn parse_qualities(
    play_urls: &Value,
    prefer_h264: bool,
) -> Result<Vec<LivePlayQuality>, SliveError> {
    let descriptors: Vec<&Value> = match play_urls {
        Value::Array(list) => list.iter().collect(),
        other => vec![other],
    };

    let mut merged: Vec<Tier> = Vec::new();
    let mut hevc_representations: Vec<&Value> = Vec::new();

    for descriptor in &descriptors {
        let mut chosen: Option<(&str, &str)> = None;
        for (key, codec) in [
            ("h264", "avc"),
            ("avc", "avc"),
            ("hevc", "hevc"),
            ("h265", "hevc"),
        ] {
            if !representations_of(descriptor.get(key)).is_empty() {
                chosen = Some((key, codec));
                break;
            }
        }
        let (representations, codec) = match chosen {
            Some((key, codec)) => (representations_of(descriptor.get(key)), Some(codec)),
            None => (representations_of(Some(*descriptor)), None),
        };
        add_tiers(&mut merged, &representations, false);
        if codec == Some("avc") {
            let hevc = if !representations_of(descriptor.get("hevc")).is_empty() {
                descriptor.get("hevc")
            } else {
                descriptor.get("h265")
            };
            hevc_representations.extend(representations_of(hevc));
        }
    }

    // H.265 集里 H.264 集没有的档位（4K / 蓝光 质臻 等）。
    let mut extra: Vec<Tier> = Vec::new();
    add_tiers(&mut extra, &hevc_representations, true);
    extra.retain(|tier| {
        !merged
            .iter()
            .any(|existing| existing.name == tier.name && existing.sort == tier.sort)
    });

    let mut all: Vec<Tier> = merged.into_iter().chain(extra).collect();
    all.sort_by(|left, right| {
        if prefer_h264 && left.hevc_only != right.hevc_only {
            return if left.hevc_only {
                Ordering::Greater
            } else {
                Ordering::Less
            };
        }
        right.sort.cmp(&left.sort).then(left.order.cmp(&right.order))
    });

    if all.is_empty() {
        return Err(SliveError::Api {
            code: -1,
            message: "playUrls 里没有可播放的档位".to_string(),
        });
    }

    Ok(all
        .into_iter()
        .map(|tier| LivePlayQuality {
            quality: tier.name,
            sort: tier.sort,
            data: tier.urls.into_iter().next().unwrap_or_default(),
        })
        .collect())
}

/// 获取播放 URL（带请求头；媒体 CDN 只认 UA / Referer）。
pub fn get_play_url(quality: &LivePlayQuality, room_id: &str) -> LivePlayUrl {
    LivePlayUrl {
        urls: vec![quality.data.clone()],
        headers: Some(std::collections::HashMap::from([
            (
                "User-Agent".to_string(),
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36".to_string(),
            ),
            (
                "Referer".to_string(),
                format!("https://live.kuaishou.com/u/{room_id}"),
            ),
        ])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn descriptor_list_becomes_qualities_sorted_by_level() {
        let play_urls = json!([{
            "adaptationSet": {
                "representation": [
                    {"id": 0, "level": 30, "url": "https://cdn/sd.flv", "name": "高清", "hidden": false},
                    {"id": 1, "level": 70, "url": "https://cdn/fhd.flv", "name": "蓝光 4M", "hidden": false},
                    {"id": 2, "level": 130, "url": "https://cdn/uhd.flv", "name": "蓝光 质臻", "hidden": true},
                    {"id": 3, "level": 50, "url": "file:///local.flv", "name": "非法地址"}
                ]
            }
        }]);
        let qualities = parse_qualities(&play_urls, true).expect("解析档位");
        assert_eq!(qualities.len(), 2, "hidden 与非 http(s) 地址被过滤");
        assert_eq!(qualities[0].quality, "蓝光 4M");
        assert_eq!(qualities[0].sort, 70);
        assert_eq!(qualities[1].quality, "高清");
        assert_eq!(qualities[1].data, "https://cdn/sd.flv");
    }

    #[test]
    fn hevc_only_tiers_follow_h264_with_suffix() {
        let play_urls = json!([{
            "h264": {
                "adaptationSet": {
                    "representation": [
                        {"id": 0, "level": 30, "url": "https://cdn/avc.flv", "name": "高清"}
                    ]
                }
            },
            "hevc": {
                "adaptationSet": {
                    "representation": [
                        {"id": 9, "level": 140, "url": "https://cdn/hevc.flv", "name": "蓝光 质臻"}
                    ]
                }
            }
        }]);
        let qualities = parse_qualities(&play_urls, true).expect("解析档位");
        assert_eq!(qualities.len(), 2);
        assert_eq!(qualities[0].quality, "高清", "优先 H.264 时 H.265 排在后面");
        assert_eq!(qualities[1].quality, "蓝光 质臻 · H.265");
        assert_eq!(qualities[1].sort, 140);
    }

    #[test]
    fn empty_play_urls_is_an_api_error() {
        let err = parse_qualities(&json!([]), true).expect_err("空档位必须报错");
        assert!(matches!(err, SliveError::Api { code: -1, .. }));
    }
}
