//! 直播间 URL → (平台, 房间标识) 解析。
//!
//! 只认主播页地址，不做短链跳转（与已被删除的 Dart `simple_live_console` 同一口径）。
//! 接受带 scheme 或裸域名两种写法：`live.bilibili.com/7734200` / `https://live.bilibili.com/7734200`。

/// 支持的平台（直接复用 core 的枚举，消除重复定义）。
pub use slive_core::platforms::Platform;

/// 解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// 平台。
    pub platform: Platform,
    /// 房间标识：B站为房间号、斗鱼/虎牙为房间号、抖音为 web_rid、Twitch 为频道名。
    pub room: String,
}

/// 解析直播间地址。
///
/// 顺序敏感：`live.douyin.com` 含 `douyin.com` 子串，故先匹配更具体的域名。
pub fn parse(raw: &str) -> Result<Target, String> {
    let url = raw.trim();
    if url.is_empty() {
        return Err("URL 不能为空".to_string());
    }

    // (域名特征, 平台, 期望的分享页形态，用于报错提示)
    let table: &[(&str, Platform, &str)] = &[
        (
            "live.bilibili.com",
            Platform::Bilibili,
            "https://live.bilibili.com/<房间号>",
        ),
        (
            "bilibili.com",
            Platform::Bilibili,
            "https://live.bilibili.com/<房间号>",
        ),
        ("huya.com", Platform::Huya, "https://www.huya.com/<房间号>"),
        (
            "douyu.com",
            Platform::Douyu,
            "https://www.douyu.com/<房间号>",
        ),
        (
            "live.douyin.com",
            Platform::Douyin,
            "https://live.douyin.com/<web_rid>",
        ),
        (
            "douyin.com",
            Platform::Douyin,
            "https://live.douyin.com/<web_rid>",
        ),
        (
            "twitch.tv",
            Platform::Twitch,
            "https://www.twitch.tv/<频道名>",
        ),
        (
            "live.kuaishou.com",
            Platform::Kuaishou,
            "https://live.kuaishou.com/u/<主播id>",
        ),
        (
            "kuaishou.com",
            Platform::Kuaishou,
            "https://live.kuaishou.com/u/<主播id>",
        ),
        (
            "live.acfun.cn",
            Platform::Acfun,
            "https://live.acfun.cn/live/<authorId>",
        ),
        (
            "acfun.cn",
            Platform::Acfun,
            "https://live.acfun.cn/live/<authorId>",
        ),
        (
            "weibo.com",
            Platform::Weibo,
            "https://weibo.com/l/wblive/p/show/<场次id>",
        ),
        (
            "xiaohongshu.com",
            Platform::Xiaohongshu,
            "https://www.xiaohongshu.com/livestream/<房间号>",
        ),
        (
            "youtube.com",
            Platform::YouTube,
            "https://www.youtube.com/watch?v=<视频id>",
        ),
        ("youtu.be", Platform::YouTube, "https://youtu.be/<视频id>"),
    ];

    for (needle, platform, example) in table {
        if url.contains(needle) {
            let segments = segments_after(url, needle);
            let room = match platform {
                // ACFUN：/live/<authorId>
                Platform::Acfun if segments.first().map(String::as_str) == Some("live") => {
                    segments.get(1).cloned()
                }
                // 快手：/u/<authorId>
                Platform::Kuaishou if segments.first().map(String::as_str) == Some("u") => {
                    segments.get(1).cloned()
                }
                // YouTube：watch?v=、/live|embed|v/<id>、/channel/<id>、/@handle、youtu.be/<id>
                Platform::YouTube if segments.first().map(String::as_str) == Some("watch") => {
                    query_param(url, "v")
                }
                Platform::YouTube
                    if matches!(
                        segments.first().map(String::as_str),
                        Some("live" | "embed" | "v" | "channel")
                    ) =>
                {
                    segments.get(1).cloned()
                }
                Platform::YouTube
                    if segments
                        .first()
                        .map(|segment| segment.starts_with('@'))
                        .unwrap_or(false) =>
                {
                    segments.first().cloned()
                }
                Platform::YouTube
                    if matches!(
                        segments.first().map(String::as_str),
                        Some("playlist" | "user" | "c" | "shorts" | "results" | "feed" | "hashtag")
                    ) =>
                {
                    None
                }
                // 微博分享页：/l/wblive/p/show/<场次 id>（场次 id 含冒号）。
                Platform::Weibo => {
                    let show = segments.iter().position(|segment| segment == "show");
                    match show {
                        Some(index)
                            if index >= 1
                                && segments[..index].iter().any(|segment| segment == "wblive") =>
                        {
                            segments.get(index + 1).cloned()
                        }
                        _ => None,
                    }
                }
                // 小红书分享页：/livestream/<房间号>。
                Platform::Xiaohongshu
                    if segments.first().map(String::as_str) == Some("livestream") =>
                {
                    segments.get(1).cloned()
                }
                _ => segments.first().cloned(),
            }
            .filter(|room| !room.is_empty());

            return match room {
                Some(room) => Ok(Target {
                    platform: *platform,
                    room,
                }),
                None => Err(format!(
                    "无法从地址中取到房间标识：{url}（应形如 {example}）"
                )),
            };
        }
    }

    Err(format!(
        "暂不支持该站点：{url}（支持 B站/斗鱼/虎牙/抖音/Twitch/快手/ACFUN/微博/小红书/YouTube）"
    ))
}

/// 取 `marker` 之后的全部路径段（剥掉 query / fragment / 空段，并解百分号编码）。
fn segments_after(url: &str, marker: &str) -> Vec<String> {
    let Some(idx) = url.find(marker).map(|idx| idx + marker.len()) else {
        return Vec::new();
    };
    let rest = url[idx..].split(['?', '#']).next().unwrap_or("");
    rest.split('/')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(percent_decode)
        .collect()
}

/// 路径段的百分号解码（`%3A` → `:`；UTF-8 按字节还原）。
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = (bytes[index + 1] as char).to_digit(16);
            let low = (bytes[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 取 query 参数（不做百分号解码，房间标识本身不需要）。
fn query_param(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1.split('#').next().unwrap_or("");
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (key == name && !value.is_empty()).then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_supported_hosts() {
        let cases = [
            (
                "https://live.bilibili.com/7734200",
                Platform::Bilibili,
                "7734200",
            ),
            ("live.bilibili.com/6", Platform::Bilibili, "6"),
            ("https://www.huya.com/lpl", Platform::Huya, "lpl"),
            ("https://www.douyu.com/9999", Platform::Douyu, "9999"),
            (
                "https://live.douyin.com/416144012050",
                Platform::Douyin,
                "416144012050",
            ),
            ("https://www.twitch.tv/shroud", Platform::Twitch, "shroud"),
            (
                "https://live.acfun.cn/live/173620",
                Platform::Acfun,
                "173620",
            ),
            (
                "https://live.kuaishou.com/u/3xdinw882pybqa2",
                Platform::Kuaishou,
                "3xdinw882pybqa2",
            ),
            (
                "https://www.youtube.com/watch?v=jfKfPfyJRdk",
                Platform::YouTube,
                "jfKfPfyJRdk",
            ),
            (
                "https://www.youtube.com/live/jfKfPfyJRdk",
                Platform::YouTube,
                "jfKfPfyJRdk",
            ),
            (
                "https://www.youtube.com/channel/UC4R8DWoMoI7CAwX8_LjQHig/live",
                Platform::YouTube,
                "UC4R8DWoMoI7CAwX8_LjQHig",
            ),
            (
                "https://www.youtube.com/@lofigirl",
                Platform::YouTube,
                "@lofigirl",
            ),
            (
                "https://weibo.com/l/wblive/p/show/1022:2321325347771573207158",
                Platform::Weibo,
                "1022:2321325347771573207158",
            ),
            (
                "https://weibo.com/l/wblive/p/show/1022%3A2321325347771573207158",
                Platform::Weibo,
                "1022:2321325347771573207158",
            ),
            (
                "https://www.xiaohongshu.com/livestream/570541076495019830",
                Platform::Xiaohongshu,
                "570541076495019830",
            ),
            ("https://youtu.be/dQw4w9WgXcQ", Platform::YouTube, "dQw4w9WgXcQ"),
        ];
        for (url, platform, room) in cases {
            let t = parse(url).unwrap();
            assert_eq!(t.platform, platform, "{url}");
            assert_eq!(t.room, room, "{url}");
        }
    }

    #[test]
    fn strips_query_and_fragment() {
        let t = parse("https://live.bilibili.com/7734200?broadcast_type=0#x").unwrap();
        assert_eq!(t.room, "7734200");
    }

    #[test]
    fn rejects_unknown_or_empty() {
        assert!(parse("").is_err());
        assert!(parse("https://example.com/1").is_err());
        assert!(parse("https://live.bilibili.com/").is_err());
        // 微博只认 /l/wblive/p/show/<场次 id>，个人主页与站内搜索不是房间。
        assert!(parse("https://weibo.com/u/1234567890").is_err());
        assert!(parse("https://s.weibo.com/weibo?q=%E7%9B%B4%E6%92%AD").is_err());
        // YouTube 的旧频道 / 列表页不是房间；短链 youtu.be 仍可用（见上一用例）。
        for url in [
            "https://www.youtube.com/user/foo",
            "https://www.youtube.com/c/foo",
            "https://www.youtube.com/playlist?list=PL123",
        ] {
            assert!(parse(url).is_err(), "{url} 不应解析成房间");
        }
    }
}
