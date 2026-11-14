//! YouTube 取流与画质解析（从 InnerTube player 响应）。

use serde_json::Value;
use crate::error::SliveError;
use crate::models::*;

/// 从 player 响应解析自适应流格式。
pub fn parse_adaptive_formats(json: &Value) -> Result<Vec<LivePlayQuality>, SliveError> {
    let streaming_data = json.get("streamingData")
        .ok_or_else(|| SliveError::Api { 
            code: -1, 
            message: "player: 无 streamingData".to_string() 
        })?;
    
    let mut qualities = Vec::new();
    
    // 自适应格式（视频+音频分离）
    if let Some(formats) = streaming_data.get("adaptiveFormats").and_then(|v| v.as_array()) {
        for format in formats {
            // 只处理视频流
            let mime_type = format.get("mimeType")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            
            if !mime_type.starts_with("video/") {
                continue;
            }
            
            // 缺 `url` 的格式很常见（signatureCipher / PO token 门槛），
            // 跳过它继续收集，别让一个格式废掉整页画质。
            let url = format
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if url.is_empty() {
                continue;
            }
            
            let quality_label = format.get("qualityLabel")
                .and_then(|v| v.as_str())
                .unwrap_or("未知");
            
            let bitrate = format
                .get("bitrate")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            
            qualities.push(LivePlayQuality {
                quality: quality_label.to_string(),
                sort: i32::try_from(bitrate).unwrap_or(i32::MAX),
                data: url.to_string(),
            });
        }
    }
    
    // 合并格式（视频+音频）
    if let Some(formats) = streaming_data.get("formats").and_then(|v| v.as_array()) {
        for format in formats {
            let url = format.get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            
            if url.is_empty() {
                continue;
            }
            
            let quality_label = format.get("qualityLabel")
                .and_then(|v| v.as_str())
                .unwrap_or("合并流");
            
            qualities.push(LivePlayQuality {
                quality: format!("{} (合并)", quality_label),
                sort: 0,
                data: url.to_string(),
            });
        }
    }
    
    if qualities.is_empty() {
        return Err(SliveError::Api { 
            code: -1, 
            message: "player: 无可用格式".to_string() 
        });
    }
    
    // 按 bitrate 降序排序
    qualities.sort_by_key(|q| std::cmp::Reverse(q.sort));
    
    Ok(qualities)
}

/// 获取播放 URL（带请求头）。
pub fn get_play_url(quality: &LivePlayQuality, video_id: &str) -> LivePlayUrl {
    LivePlayUrl {
        urls: vec![quality.data.clone()],
        headers: Some(std::collections::HashMap::from([
            ("User-Agent".to_string(), "com.google.android.youtube/21.08.266 (Linux; U; Android 11)".to_string()),
            ("Referer".to_string(), format!("https://www.youtube.com/watch?v={video_id}")),
        ])),
    }
}
