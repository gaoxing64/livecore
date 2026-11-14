//! F-01 分类：标签 + 标签下的目录（对照 PureLive `twitch_site.dart:273-344`）。
//!
//! 一级分类是 `SearchCategoryTags` 的标签，二级分类是
//! `BrowsePage_AllDirectories` 的目录（游戏）。目录的 `slug` 落在
//! [`LiveSubCategory::id`] 上，因为 `DirectoryPage_Game` 用 slug 取该目录下的直播。
//!
//! 名称默认用中文（`Accept-Language: zh-CN`）；Twitch 对个别标签没有中文名，
//! 再用一次英文 tags 请求补上。首轮目录请求失败会让整个分类失败，后续轮
//! （cursor 翻页）失败只结束这些标签——匿名客户端常在翻页时被要求 integrity token。

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::error::SliveError;
use crate::models::{LiveCategory, LiveSubCategory};

use super::gql::{self, BATCH_LIMIT, TAG_DIRECTORY_LIMIT, TAG_PAGE_LIMIT};
use super::gql_response::{self, GqlFailure};
use super::header::{TWITCH_ACCEPT_LANGUAGE_EN, TWITCH_ACCEPT_LANGUAGE_ZH};
use super::site::TwitchSite;

/// 一个分类标签。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Tag {
    id: String,
    name: String,
}

/// 拉取分类树。
pub(crate) async fn fetch(site: &TwitchSite) -> Result<Vec<LiveCategory>, SliveError> {
    let mut tags = fetch_tags(site, TWITCH_ACCEPT_LANGUAGE_ZH).await?;
    if tags.iter().any(|tag| tag.name.is_empty()) {
        // 名称只是展示用，英文回退失败时保留空名，不让整个分类失败。
        if let Ok(english) = fetch_tags(site, TWITCH_ACCEPT_LANGUAGE_EN).await {
            let names: HashMap<&str, &str> = english
                .iter()
                .map(|tag| (tag.id.as_str(), tag.name.as_str()))
                .collect();
            for tag in &mut tags {
                if tag.name.is_empty() {
                    tag.name = names.get(tag.id.as_str()).copied().unwrap_or("").to_string();
                }
            }
        }
    }

    let mut children: HashMap<String, Vec<LiveSubCategory>> =
        tags.iter().map(|tag| (tag.id.clone(), Vec::new())).collect();
    let mut seen_cursors = HashSet::new();
    let mut pending: Vec<(usize, Option<String>)> =
        (0..tags.len()).map(|index| (index, None)).collect();

    for round in 0..TAG_PAGE_LIMIT {
        if pending.is_empty() {
            break;
        }
        let answers = directories_round(site, &tags, &pending, round > 0).await?;
        let mut next = Vec::new();
        for (tag_index, areas, cursor) in answers {
            let full_page = areas.len() >= TAG_DIRECTORY_LIMIT as usize;
            let tag_id = tags[tag_index].id.clone();
            children.entry(tag_id.clone()).or_default().extend(areas);
            if full_page {
                if let Some(cursor) = cursor {
                    if seen_cursors.insert(format!("{tag_id}|{cursor}")) {
                        next.push((tag_index, Some(cursor)));
                    }
                }
            }
        }
        pending = next;
    }

    Ok(tags
        .into_iter()
        .map(|tag| LiveCategory {
            id: tag.id.clone(),
            name: tag.name,
            children: children.remove(&tag.id).unwrap_or_default(),
        })
        .collect())
}

/// `SearchCategoryTags`：`tagName` 优先，`localizedName` 兜底（对照 `TwitchApi.tags`）。
async fn fetch_tags(site: &TwitchSite, language: &str) -> Result<Vec<Tag>, SliveError> {
    let decoded = site
        .post_gql_with_language(&gql::tags_operation(), language)
        .await?;
    let data = gql_response::data(&decoded, "SearchCategoryTags").map_err(SliveError::from)?;
    parse_tags(data)
}

fn parse_tags(data: &Value) -> Result<Vec<Tag>, SliveError> {
    let list = data
        .get("searchCategoryTags")
        .and_then(Value::as_array)
        .ok_or_else(|| SliveError::Api {
            code: -1,
            message: "SearchCategoryTags: no list".to_string(),
        })?;
    Ok(list
        .iter()
        .filter_map(|tag| {
            let id = tag.get("id")?.as_str()?.to_string();
            let primary = gql_response::text(tag.get("tagName"));
            let name = if primary.is_empty() {
                gql_response::text(tag.get("localizedName"))
            } else {
                primary
            };
            Some(Tag { id, name })
        })
        .collect())
}

/// 一轮目录请求：把待查页按 `BATCH_LIMIT` 分组，每组一个批量 GQL。
async fn directories_round(
    site: &TwitchSite,
    tags: &[Tag],
    pending: &[(usize, Option<String>)],
    lenient: bool,
) -> Result<Vec<(usize, Vec<LiveSubCategory>, Option<String>)>, SliveError> {
    let mut out = Vec::new();
    for chunk in pending.chunks(BATCH_LIMIT) {
        let body = gql::build_batch(
            &chunk
                .iter()
                .map(|(index, cursor)| {
                    gql::directories_operation(&tags[*index].id, cursor.as_deref())
                })
                .collect::<Vec<_>>(),
        );
        match round_once(site, tags, chunk, &body).await {
            Ok(answer) => out.extend(answer),
            Err(GqlFailure::Integrity) if lenient => continue,
            Err(failure) => return Err(failure.into()),
        }
    }
    Ok(out)
}

/// 一组目录请求的发送与解析（错误保留 [`GqlFailure`] 以区分 integrity）。
async fn round_once(
    site: &TwitchSite,
    tags: &[Tag],
    chunk: &[(usize, Option<String>)],
    body: &Value,
) -> Result<Vec<(usize, Vec<LiveSubCategory>, Option<String>)>, GqlFailure> {
    let decoded = site
        .post_gql_with_language(body, TWITCH_ACCEPT_LANGUAGE_ZH)
        .await
        .map_err(GqlFailure::Fatal)?;
    let envelopes =
        gql_response::batch_envelopes(&decoded, chunk.len(), "BrowsePage_AllDirectories")?;
    let mut out = Vec::with_capacity(chunk.len());
    for (position, (tag_index, _)) in chunk.iter().enumerate() {
        let tag = &tags[*tag_index];
        // 单条 envelope 带错误且无 data 时按「这一页没有目录」处理（对照 `TwitchApi.directories`）。
        let data = gql_response::data(envelopes[position], "BrowsePage_AllDirectories").ok();
        let connection = data.and_then(|value| value.get("directoriesWithTags"));
        let mut areas = Vec::new();
        for edge in gql_response::edges(connection) {
            let Some(node) = edge.get("node").filter(|value| value.is_object()) else {
                continue;
            };
            let Some(slug) = node.get("slug").and_then(Value::as_str) else {
                continue;
            };
            if node.get("id").and_then(Value::as_str).is_none() {
                continue;
            }
            areas.push(LiveSubCategory {
                name: gql_response::game_name(node),
                pic: Some(gql_response::text(node.get("avatarURL"))),
                id: slug.to_string(),
                parent_id: tag.id.clone(),
            });
        }
        out.push((*tag_index, areas, gql_response::next_cursor(connection)));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tag_name_falls_back_to_localized_name_and_needs_an_id() {
        let value = json!({"searchCategoryTags": [
            {"id": "a", "tagName": "中文", "localizedName": "中文"},
            {"id": "b", "tagName": "", "localizedName": "Localized"},
            {"id": "c", "tagName": "", "localizedName": ""},
            {"name": "no id"}
        ]});
        let parsed = parse_tags(&value).expect("解析");
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].name, "中文");
        assert_eq!(parsed[1].name, "Localized");
        assert!(parsed[2].name.is_empty());
    }

    #[test]
    fn missing_tag_list_is_an_api_error() {
        assert!(matches!(
            parse_tags(&json!({})),
            Err(SliveError::Api { code: -1, .. })
        ));
    }

    #[test]
    fn directory_node_becomes_a_sub_category_keyed_by_slug() {
        let node = json!({
            "id": "509658",
            "slug": "just-chatting",
            "name": "Just Chatting",
            "displayName": "谈天说地",
            "avatarURL": "https://cdn/boxart.jpg"
        });
        let sub = LiveSubCategory {
            name: gql_response::game_name(&node),
            pic: Some(gql_response::text(node.get("avatarURL"))),
            id: node["slug"].as_str().unwrap().to_string(),
            parent_id: "tag-1".to_string(),
        };
        assert_eq!(sub.id, "just-chatting");
        assert_eq!(sub.name, "谈天说地");
        assert_eq!(sub.parent_id, "tag-1");
    }
}
