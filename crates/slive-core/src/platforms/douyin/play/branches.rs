//! 清晰度列表（F-07）的**三条分支**实现，从 `play.rs` 拆出（单文件 ≤300 行门禁）。
//!
//! 三条分支的判定顺序即 Dart 的 if/else 链（`douyin_site.dart:961-1077`），**不可重排**：
//! 1. [`from_url_tables`]：无画质列表 -> 用地址表的键当画质名；
//! 2. [`from_levels`]：`stream_data` 非 JSON -> 优先按 `sdk_key`/名称匹配地址表，
//!    匹配不到再按 `level` 倒序索引取值；
//! 3. [`from_stream_data`]：`stream_data` 是 JSON -> 按 `sdk_key` 精确取 `main.flv` / `main.hls`。
//!
//! 本模块只依赖 [`super::util`] 与 `models`，**不**发起任何网络请求。

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::models::LivePlayQuality;

use super::super::util::{as_object, safe_json_decode, urls_to_json};

/// 组装一条画质项（`urls` 为空的不产出 —— 与 Dart 的 `if (urls.isNotEmpty)` 一致）。
pub(super) fn push_quality(
    out: &mut Vec<LivePlayQuality>,
    quality: &str,
    sort: i32,
    urls: Vec<String>,
) {
    if urls.is_empty() {
        return;
    }
    out.push(LivePlayQuality {
        quality: quality.to_string(),
        sort,
        data: urls_to_json(&urls),
    });
}

/// 按 flv/hls 顺序拼地址（`hls_first` 决定谁在前）。
pub(super) fn order_urls(flv: &str, hls: &str, hls_first: bool) -> Vec<String> {
    let mut urls = Vec::with_capacity(2);
    if hls_first {
        push_non_empty(&mut urls, hls);
        push_non_empty(&mut urls, flv);
    } else {
        push_non_empty(&mut urls, flv);
        push_non_empty(&mut urls, hls);
    }
    urls
}

fn push_non_empty(out: &mut Vec<String>, url: &str) {
    if !url.is_empty() {
        out.push(url.to_string());
    }
}

/// 分支 1：无画质列表 -> 键名即画质名，`sort` 从键数递减。
pub(super) fn from_url_tables(data: &Value, hls_first: bool) -> Vec<LivePlayQuality> {
    let flv_map = as_object(data.get("flv_pull_url"));
    let hls_map = as_object(data.get("hls_pull_url_map"));

    // 键集合：flv 的全部键 + hls 里 flv 没有的键（保持插入序）
    let mut keys: Vec<String> = Vec::new();
    if let Some(flv) = flv_map {
        keys.extend(flv.keys().cloned());
    }
    if let Some(hls) = hls_map {
        for key in hls.keys() {
            if !flv_map.map(|f| f.contains_key(key)).unwrap_or(false) {
                keys.push(key.clone());
            }
        }
    }

    let mut out = Vec::new();
    let mut level = keys.len() as i32;
    for key in keys {
        let flv = flv_map
            .and_then(|m| m.get(&key))
            .map(crate::util::value_to_string)
            .unwrap_or_default();
        let hls = hls_map
            .and_then(|m| m.get(&key))
            .map(crate::util::value_to_string)
            .unwrap_or_default();
        push_quality(&mut out, &key, level, order_urls(&flv, &hls, hls_first));
        level -= 1;
    }
    out
}

/// 分支 2：`stream_data` 非 JSON -> 优先按 `sdk_key`/名称匹配，兜底按 `level` 倒序索引。
pub(super) fn from_levels(
    data: &Value,
    qualities: &[Value],
    hls_first: bool,
) -> Vec<LivePlayQuality> {
    let flv_map = as_object(data.get("flv_pull_url"));
    let hls_map = as_object(data.get("hls_pull_url_map"));
    let flv_list = map_values(data.get("flv_pull_url"));
    let hls_list = map_values(data.get("hls_pull_url_map"));

    let mut out = Vec::new();
    for quality in qualities {
        let level = crate::util::as_i32(quality.get("level")).unwrap_or(0);
        let sdk_key = quality
            .get("sdk_key")
            .map(crate::util::value_to_string)
            .unwrap_or_default();
        let name = quality
            .get("name")
            .map(crate::util::value_to_string)
            .unwrap_or_default();
        // 精确键优先；旧接口键名不一致时按名称/流名后缀匹配，最后才走倒序索引。
        let flv = pick_url(flv_map, &flv_list, &sdk_key, &name, level);
        let hls = pick_url(hls_map, &hls_list, &sdk_key, &name, level);
        push_quality(&mut out, &name, level, order_urls(&flv, &hls, hls_first));
    }
    out
}

fn pick_url(
    map: Option<&Map<String, Value>>,
    list: &[String],
    sdk_key: &str,
    name: &str,
    level: i32,
) -> String {
    if let Some(value) = map
        .and_then(|m| m.get(sdk_key))
        .filter(|value| !crate::util::value_to_string(value).is_empty())
    {
        return crate::util::value_to_string(value);
    }
    if let Some(value) = pick_by_hint(map, sdk_key, name) {
        return value;
    }
    // 地址表本身使用已知的抖音档位键时，`sdk_key` 缺失就是该档不可用；
    // 不能再按 level 猜一个别的档位（会把标清错接到原画地址）。
    if map.is_some_and(|m| m.keys().any(|key| is_known_quality_key(key)))
        && is_known_quality_key(sdk_key)
    {
        return String::new();
    }
    pick_by_level(list, level)
}

fn is_known_quality_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "origin"
            | "origion"
            | "or4"
            | "uhd"
            | "full_hd1"
            | "hd"
            | "hd1"
            | "sd"
            | "sd2"
            | "ld"
            | "sd1"
            | "md"
    )
}

/// 旧接口的 `sdk_key` 与地址表键名偶尔不一致（例如 `origin` 对应 `ORIGION`，
/// `uhd` 对应 `FULL_HD1`）。先按流名后缀匹配，避免只依赖 `level` 的倒序索引。
fn pick_by_hint(map: Option<&Map<String, Value>>, sdk_key: &str, name: &str) -> Option<String> {
    let hint = match sdk_key.to_ascii_lowercase().as_str() {
        "origin" | "origion" => "_or4",
        "uhd" => "_uhd",
        "hd" => "_hd",
        "sd" => "_sd",
        "ld" => "_ld",
        "md" => "_md",
        _ => match name {
            "原画" => "_or4",
            "蓝光" => "_uhd",
            "超清" => "_hd",
            "高清" => "_sd",
            "标清" => "_ld",
            "流畅" => "_md",
            _ => return None,
        },
    };
    map?.iter().find_map(|(key, value)| {
        let url = crate::util::value_to_string(value);
        (key_matches_hint(key, sdk_key, name) || url.contains(hint))
            .then_some(url)
            .filter(|url| !url.is_empty())
    })
}

fn key_matches_hint(key: &str, sdk_key: &str, name: &str) -> bool {
    let key = key.to_ascii_lowercase();
    match sdk_key.to_ascii_lowercase().as_str() {
        "origin" | "origion" => matches!(key.as_str(), "origin" | "origion" | "or4"),
        "uhd" => matches!(key.as_str(), "uhd" | "full_hd1"),
        "hd" => matches!(key.as_str(), "hd" | "hd1"),
        "sd" => matches!(key.as_str(), "sd" | "sd2"),
        "ld" => matches!(key.as_str(), "ld" | "sd1"),
        "md" => key == "md",
        _ => match name {
            "原画" => matches!(key.as_str(), "origin" | "origion" | "or4"),
            "蓝光" => matches!(key.as_str(), "uhd" | "full_hd1"),
            "超清" => matches!(key.as_str(), "hd" | "hd1"),
            "高清" => matches!(key.as_str(), "sd" | "sd2"),
            "标清" => matches!(key.as_str(), "ld" | "sd1"),
            "流畅" => key == "md",
            _ => false,
        },
    }
}

/// `list[len - level]`；越界（含 level 为负）-> 空串。
fn pick_by_level(list: &[String], level: i32) -> String {
    if level < 0 {
        return String::new();
    }
    list.len()
        .checked_sub(level as usize)
        .and_then(|i| list.get(i))
        .cloned()
        .unwrap_or_default()
}

/// 分支 3：`stream_data` 是 JSON -> 按 `sdk_key` 精确取 `main.flv` / `main.hls`。
pub(super) fn from_stream_data(
    qualities: &[Value],
    stream_data: &str,
    hls_first: bool,
) -> Vec<LivePlayQuality> {
    let quality_data = safe_json_decode(stream_data)
        .and_then(|v| v.pointer("/data").and_then(|d| as_object(Some(d))).cloned())
        .unwrap_or_default();

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for quality in qualities {
        let sdk_key = quality
            .get("sdk_key")
            .map(crate::util::value_to_string)
            .unwrap_or_default();
        let name = quality
            .get("name")
            .map(crate::util::value_to_string)
            .unwrap_or_default();
        let (flv, hls) = find_quality_value(&quality_data, &sdk_key, &name)
            .map(main_urls)
            .unwrap_or_default();
        let level = crate::util::as_i32(quality.get("level")).unwrap_or(0);
        if !urls_is_empty(&flv, &hls) {
            seen.insert(sdk_key.to_ascii_lowercase());
            seen.insert(name.clone());
        }
        push_quality(&mut out, &name, level, order_urls(&flv, &hls, hls_first));
    }

    // `options.qualities` 偶尔会少 `原画`，但 `stream_data.data` 仍有 `origin`
    // 的地址。`stream_data` 还包含 `ao`（纯音频）和未对外暴露的 `md`，
    // 因此这里只补原画，不把内部档位擅自塞进 UI。
    if !seen.contains("origin") && !seen.contains("原画") {
        let origin = quality_data
            .iter()
            .find(|(key, _)| {
                matches!(
                    key.to_ascii_lowercase().as_str(),
                    "origin" | "origion" | "or4"
                )
            })
            .map(|(_, value)| main_urls(value));
        if let Some((flv, hls)) = origin.filter(|(flv, hls)| !urls_is_empty(flv, hls)) {
            let sort = out.iter().map(|item| item.sort).max().unwrap_or(0) + 1;
            push_quality(&mut out, "原画", sort, order_urls(&flv, &hls, hls_first));
        }
    }
    out
}

fn find_quality_value<'a>(
    data: &'a Map<String, Value>,
    sdk_key: &str,
    name: &str,
) -> Option<&'a Value> {
    if let Some(value) = data.get(sdk_key) {
        return Some(value);
    }
    let lower = sdk_key.to_ascii_lowercase();
    if let Some((_, value)) = data
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(&lower))
    {
        return Some(value);
    }
    let aliases: &[&str] = match lower.as_str() {
        "origin" | "origion" => &["origin", "origion", "ORIGION"],
        "uhd" => &["uhd", "FULL_HD1"],
        "hd" => &["hd", "HD1"],
        "sd" => &["sd", "SD2"],
        "ld" => &["ld", "SD1"],
        _ => match name {
            "原画" => &["origin", "origion", "ORIGION"],
            "蓝光" => &["uhd", "FULL_HD1"],
            "超清" => &["hd", "HD1"],
            "高清" => &["sd", "SD2"],
            "标清" => &["ld", "SD1"],
            _ => &[],
        },
    };
    aliases.iter().find_map(|alias| {
        data.get(*alias)
            .or_else(|| data.get(&alias.to_ascii_lowercase()))
    })
}

fn main_urls(value: &Value) -> (String, String) {
    let main = value.get("main").and_then(|v| as_object(Some(v)));
    let flv = main
        .and_then(|m| m.get("flv"))
        .map(crate::util::value_to_string)
        .unwrap_or_default();
    let hls = main
        .and_then(|m| m.get("hls"))
        .map(crate::util::value_to_string)
        .unwrap_or_default();
    (flv, hls)
}

fn urls_is_empty(flv: &str, hls: &str) -> bool {
    flv.is_empty() && hls.is_empty()
}

/// 对象的所有值（按插入序），转字符串。
pub(super) fn map_values(value: Option<&Value>) -> Vec<String> {
    as_object(value)
        .map(|m| m.values().map(crate::util::value_to_string).collect())
        .unwrap_or_default()
}
