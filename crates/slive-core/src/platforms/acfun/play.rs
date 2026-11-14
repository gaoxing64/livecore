//! ACFUN 取流与画质解析（对照 PureLive `acfun_api.dart:487-560`）。
//!
//! 结束码（129004 关播 / 380205 付费无权限）由 [`super::site::AcfunSite::start_play`]
//! 统一分类；本模块只解析成功应答（`result=1`），不再重复判定。

use serde_json::Value;
use crate::error::SliveError;
use crate::models::*;

/// 从 startPlay API 响应解析画质列表。
/// startPlay 成功码是 result=1，videoPlayRes 在 data 对象内（可能是 JSON 字符串）。
pub fn parse_qualities(json: &Value) -> Result<Vec<LivePlayQuality>, SliveError> {
    let result = json
        .get("result")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| SliveError::Api {
            code: -1,
            message: "startPlay: 无 result".to_string(),
        })?;
    if result != 1 {
        return Err(SliveError::Api {
            code: result as i32,
            message: format!("startPlay: result {result}"),
        });
    }
    
    // data 对象
    let data = json.get("data")
        .ok_or_else(|| SliveError::Api { 
            code: -1, 
            message: "startPlay: 无 data".to_string() 
        })?;
    
    // videoPlayRes 可能是 JSON 字符串或对象
    let video_play_res = data.get("videoPlayRes")
        .ok_or_else(|| SliveError::Api { 
            code: -1, 
            message: "startPlay: 无 videoPlayRes".to_string() 
        })?;
    
    let play_res: Value = if video_play_res.is_string() {
        serde_json::from_str(video_play_res.as_str().unwrap())
            .map_err(|e| SliveError::decode(format!("解析 videoPlayRes 失败：{}", e)))?
    } else {
        video_play_res.clone()
    };
    
    // liveAdaptiveManifest 数组
    let manifests = play_res.get("liveAdaptiveManifest")
        .and_then(|v| v.as_array())
        .ok_or_else(|| SliveError::Api { 
            code: -1, 
            message: "startPlay: 无 liveAdaptiveManifest".to_string() 
        })?;
    
    let mut qualities = Vec::new();
    
    for manifest in manifests {
        let representations = manifest.pointer("/adaptationSet/representation")
            .and_then(|v| v.as_array());
        
        if let Some(reps) = representations {
            for rep in reps {
                // 跳过隐藏的
                if rep.get("hidden").and_then(|v| v.as_bool()).unwrap_or(false) {
                    continue;
                }
                
                let url = rep.get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                
                // 只接受 http/https URL
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    continue;
                }
                
                let id = rep
                    .get("id")
                    .map(crate::util::value_to_string)
                    .filter(|text| !text.is_empty() && text != "null")
                    .unwrap_or_default();

                if id.is_empty() {
                    continue;
                }
                
                let label = rep
                    .get("name")
                    .and_then(|v| v.as_str())
                    .filter(|text| !text.is_empty())
                    .or_else(|| {
                        rep.get("shortName")
                            .and_then(|v| v.as_str())
                            .filter(|text| !text.is_empty())
                    })
                    .or_else(|| {
                        rep.get("qualityType")
                            .and_then(|v| v.as_str())
                            .filter(|text| !text.is_empty())
                    })
                    .map(str::to_string);

                let sort = rep
                    .get("level")
                    .and_then(|v| v.as_i64())
                    .or_else(|| rep.get("bitrate").and_then(|v| v.as_i64()))
                    .or_else(|| rep.get("rank").and_then(|v| v.as_i64()))
                    .unwrap_or(0) as i32;

                let label = label.unwrap_or_else(|| format!("清晰度 {sort}"));

                // 去重：同一 URL 只保留一次
                if qualities.iter().any(|q: &LivePlayQuality| q.data == url) {
                    continue;
                }

                qualities.push(LivePlayQuality {
                    quality: label,
                    sort,
                    data: url.to_string(),
                });
            }
        }
    }
    
    if qualities.is_empty() {
        return Err(SliveError::Api { 
            code: -1, 
            message: "startPlay: 无可用画质".to_string() 
        });
    }
    
    // 按 rank 降序排序
    qualities.sort_by_key(|q| std::cmp::Reverse(q.sort));
    
    Ok(qualities)
}

/// 获取播放 URL（带请求头）。
pub fn get_play_url(quality: &LivePlayQuality, room_id: &str) -> LivePlayUrl {
    LivePlayUrl {
        urls: vec![quality.data.clone()],
        headers: Some(std::collections::HashMap::from([
            (
                "User-Agent".to_string(),
                crate::http::DEFAULT_USER_AGENT.to_string(),
            ),
            (
                "Referer".to_string(),
                format!("https://live.acfun.cn/live/{room_id}"),
            ),
            ("Origin".to_string(), "https://live.acfun.cn".to_string()),
        ])),
    }
}
