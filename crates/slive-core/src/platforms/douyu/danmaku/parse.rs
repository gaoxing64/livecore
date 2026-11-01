//! 斗鱼弹幕消息解析（对照 `douyu_danmaku.dart:67-150` / `:230-247`）。
//!
//! 只处理 Dart 明确处理的 4 类：
//! - `chatmsg`（**仅在显式 `if == 0` 时丢弃**，见下）；
//! - `comm_chatmsg` -> 醒目留言（SC）；
//! - `voice_trlt` -> 高能弹幕 2（SC）；
//! - `uenter` -> 显式忽略（Dart `else if (type != "uenter") {}` 空分支）。
//!
//! 纪律（对齐 B站 AC-28）：脏帧 / 缺字段只记日志、不 panic、不打断会话。

use chrono::{DateTime, Local};
use serde_json::Value;

use crate::models::{
    LiveMessage, LiveMessageColor, LiveMessageData, LiveMessageType, LiveSuperChatMessage,
};

use super::super::emoticon::{
    extract_emoticons, extract_popular_danmu, static_emoticons, DouyuEmoticonTable,
};
use super::protocol::{deserialize, stt_to_value_with};

/// SC 占位消息的固定文本（App 侧靠它识别 SC 卡片，对齐 Dart）。
const SC_PLACEHOLDER: &str = "SUPER_CHAT_MESSAGE";
/// `comm_chatmsg` 的配色（斗鱼不下发颜色，取用 Dart 的紫色系方案）。
const COMM_CHAT_BG: &str = "#c1c1ff";
const COMM_CHAT_BG_BOTTOM: &str = "#292a60";
/// 高能弹幕的配色。
const VOICE_TRLT_BG: &str = "#ffffff";
const VOICE_TRLT_BG_BOTTOM: &str = "#246488";

/// 解码一帧为若干条 [`LiveMessage`]（损坏帧返回空集）。
pub fn decode_frame(data: &[u8]) -> Vec<LiveMessage> {
    decode_frame_with_emoticons(data, static_emoticons())
}

/// 解码一帧，并用房间表情表把 `[占位符]` 还原为图片。
pub fn decode_frame_with_emoticons(
    data: &[u8],
    emoticons: &DouyuEmoticonTable,
) -> Vec<LiveMessage> {
    let Some(text) = deserialize(data) else {
        log::warn!("斗鱼弹幕帧解包失败（{} 字节）", data.len());
        return Vec::new();
    };
    let value = parse_stt(&text);
    parse_message_with_emoticons(&value, emoticons)
        .into_iter()
        .collect()
}

/// 需要「其后字段全部归属该容器」的 STT 键。
///
/// Dart 的 `sttToJObject` 是**扁平**解析（`k@v` 只取第二段），因此斗鱼 SC 的嵌套字段
/// （`comm_chatmsg.chatmsg.*` / `voice_trlt.list[0].*` / `uat[0]`）在 Dart 里其实取不到 ——
/// Dart 是靠 `try/catch + ?? ''` 兜底成空值。真实报文里这些字段是存在的，故此处按容器语义解析，
/// **取值口径与 Dart 一致**（拿不到就用空/0），仅在「能取到」时比 Dart 更完整。
const NESTED_CONTAINERS: &[&str] = &["chatmsg", "list", "uat"];

/// 嵌套取值：`nested -> key` 优先，其次回落顶层 `key`（兼容两种服务端写法）。
fn nested_text(value: &Value, container: &str, key: &str) -> String {
    value
        .get(container)
        .and_then(|c| c.get(key))
        .map(crate::util::value_to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| text(value, key))
}

/// 容器第 0 项的字段值（数组容器 `list@=0@=…`）。
fn first_item_text(value: &Value, container: &str, key: &str) -> String {
    value
        .get(container)
        .and_then(|c| c.get(0))
        .and_then(|item| item.get(key))
        .map(crate::util::value_to_string)
        .unwrap_or_default()
}

/// 容器第 `index` 项的**字符串本身**（`uat@=1@=host` 这种「数组元素即字符串」的写法）。
fn item_value(value: &Value, container: &str, index: usize) -> String {
    value
        .get(container)
        .and_then(|c| c.get(index))
        .map(crate::util::value_to_string)
        .unwrap_or_default()
}

/// 解析一条斗鱼 STT 报文（带嵌套容器语义）。
pub fn parse_stt(text_body: &str) -> Value {
    stt_to_value_with(text_body, NESTED_CONTAINERS)
}

/// 解析一条 STT 消息为 [`LiveMessage`]（不关心的类型返回 `None`）。
pub fn parse_message(value: &Value) -> Option<LiveMessage> {
    parse_message_with_emoticons(value, static_emoticons())
}

/// 同 [`parse_message`]，但为聊天消息注入房间表情表。
pub fn parse_message_with_emoticons(
    value: &Value,
    emoticons: &DouyuEmoticonTable,
) -> Option<LiveMessage> {
    let message_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    match message_type {
        // `if` 是「粉丝团发言」标记，**不是**弹幕有效性的开关：实测 8 个热门房间
        // 2 分钟内 92 条 chatmsg 中 65 条（71%）根本没有 `if` 字段，且 `if@=0`
        // 一次都没出现。Dart 原实现把缺字段当成 `'0'` 直接丢弃，会吞掉大部分
        // 正常弹幕（连带其中的表情）。这里只丢弃**显式** `if@=0` 的报文，
        // 保留 Dart 规避「阴间弹幕」的意图，同时不再误伤普通观众。
        "chatmsg" if !is_explicit_non_fan(value) => Some(chat_message(value, emoticons)),
        "comm_chatmsg" => super_chat_from_comm(value),
        "voice_trlt" => super_chat_from_voice(value),
        // uenter / 其他类型一律忽略（Dart 显式空分支）
        _ => None,
    }
}

/// 是否被服务端显式标记为「非粉丝团发言」（`if@=0`）。缺字段不算。
fn is_explicit_non_fan(value: &Value) -> bool {
    value
        .get("if")
        .map(crate::util::value_to_string)
        .map(|raw| raw.trim().to_string())
        .as_deref()
        == Some("0")
}

fn chat_message(value: &Value, emoticons: &DouyuEmoticonTable) -> LiveMessage {
    let color_id = value
        .get("col")
        .map(crate::util::value_to_string)
        .and_then(|raw| raw.trim().parse::<i32>().ok())
        .unwrap_or(0);
    let original = text(value, "txt");
    let gag = text(value, "gag");
    let popular = extract_popular_danmu(emoticons, &gag);
    let message = if popular.is_some() {
        // 梗弹幕的正文只是发送端占位文本，展示以 gag 对应的整张横图为准。
        "[梗弹幕]".to_string()
    } else {
        original
    };
    LiveMessage {
        message_type: LiveMessageType::Chat,
        user_name: text(value, "nn"),
        user_id: None,
        emoticons: popular.map(|emoticon| vec![emoticon]).or_else(|| {
            if message.is_empty() {
                None
            } else {
                extract_emoticons(&message, emoticons)
            }
        }),
        message,
        data: None,
        color: color_of(color_id),
    }
}

/// `comm_chatmsg`：斗鱼醒目留言（服务端下发起止时间与价格）。
fn super_chat_from_comm(value: &Value) -> Option<LiveMessage> {
    let start_ms = value
        .get("now")
        .map(crate::util::value_to_string)
        .and_then(|raw| raw.trim().parse::<i64>().ok())?;
    let duration_secs = value
        .get("cet")
        .map(crate::util::value_to_string)
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .unwrap_or(0);
    let start = local_time(start_ms)?;
    // Dart: face = jsonData["chatmsg"]["ic"]，取不到时用空串（CoreLog 记录后继续）
    let face_id = nested_text(value, "chatmsg", "ic");
    let super_chat = LiveSuperChatMessage {
        user_name: nested_text(value, "chatmsg", "nn"),
        face: format!("https://apic.douyucdn.cn/upload/{face_id}_small.jpg"),
        message: nested_text(value, "chatmsg", "txt"),
        price: fen_to_yuan(value.get("cprice")),
        start_time: start,
        end_time: start + chrono::Duration::seconds(duration_secs),
        background_color: COMM_CHAT_BG.to_string(),
        background_bottom_color: COMM_CHAT_BG_BOTTOM.to_string(),
    };
    Some(sc_message(super_chat))
}

/// `voice_trlt`：高能弹幕 2（`list[0]` 承载内容，`uat` 是头像 host）。
fn super_chat_from_voice(value: &Value) -> Option<LiveMessage> {
    // Dart: `face = "https://${scData["uat"][1]}"`（scData 即 list[0]）——
    // 优先在容器内取 uat，取不到再回落顶层（数组/标量两种写法都兼容）。
    let in_list = value
        .get("list")
        .and_then(|c| c.get(0))
        .map(|item| {
            item.get("uat")
                .map(crate::util::value_to_string)
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let host = if !in_list.is_empty() {
        in_list
    } else {
        let mut fallback = item_value(value, "uat", 1);
        if fallback.is_empty() {
            fallback = item_value(value, "uat", 0);
        }
        if fallback.is_empty() {
            text(value, "uat")
        } else {
            fallback
        }
    };
    let start = unix_secs_field(&first_item_text(value, "list", "acptime"))?;
    let end = unix_secs_field(&first_item_text(value, "list", "etime"))?;
    let super_chat = LiveSuperChatMessage {
        user_name: first_item_text(value, "list", "un"),
        face: face_url(&host),
        message: first_item_text(value, "list", "content"),
        price: fen_to_yuan_str(&first_item_text(value, "list", "realPrice")),
        start_time: start,
        end_time: end,
        background_color: VOICE_TRLT_BG.to_string(),
        background_bottom_color: VOICE_TRLT_BG_BOTTOM.to_string(),
    };
    Some(sc_message(super_chat))
}

/// 头像 host -> 完整 URL（Dart: `"https://${uat}"`）。
///
/// 容错：STT 的 `//` 是结构分隔符，值里的 `https://` 常被下发成 `https:`
/// （QA 合成夹具即如此）——此时补回斜杠，避免拼出 `https://https:...` 死链。
fn face_url(host: &str) -> String {
    let host = host.trim();
    if host.is_empty() {
        return String::new();
    }
    if host.contains("://") {
        return host.to_string();
    }
    for scheme in ["https:", "http:"] {
        if let Some(rest) = host.strip_prefix(scheme) {
            return format!("{scheme}//{rest}");
        }
    }
    format!("https://{host}")
}

/// Unix **秒** -> 本地时间（高能弹幕的 `acptime`/`etime` 是秒，对齐 Dart `* 1000`）。
fn unix_secs_field(raw: &str) -> Option<DateTime<Local>> {
    let secs = raw.trim().parse::<i64>().ok()?;
    local_time(secs.checked_mul(1000)?)
}

fn sc_message(super_chat: LiveSuperChatMessage) -> LiveMessage {
    LiveMessage {
        message_type: LiveMessageType::SuperChat,
        user_name: SC_PLACEHOLDER.to_string(),
        user_id: None,
        message: SC_PLACEHOLDER.to_string(),
        data: Some(LiveMessageData::SuperChat(super_chat)),
        color: LiveMessageColor::white(),
        emoticons: None,
    }
}

/// 斗鱼价格以**分**下发，除 100 得元（对齐 Dart `~/ 100`）。
fn fen_to_yuan(value: Option<&Value>) -> i32 {
    fen_to_yuan_str(&value.map(crate::util::value_to_string).unwrap_or_default())
}

/// 同 [`fen_to_yuan`]，但入参是已取出的字符串。
fn fen_to_yuan_str(raw: &str) -> i32 {
    raw.trim()
        .parse::<f64>()
        .map(|fen| (fen / 100.0).floor() as i32)
        .unwrap_or(0)
}

/// Unix 毫秒 -> 本地时间（越界返回 `None`）。
fn local_time(ms: i64) -> Option<DateTime<Local>> {
    DateTime::from_timestamp_millis(ms).map(|dt| dt.with_timezone(&Local))
}

fn text(value: &Value, field: &str) -> String {
    value
        .get(field)
        .map(crate::util::value_to_string)
        .unwrap_or_default()
}

/// 斗鱼颜色表（对齐 Dart `getColor`；`0` 与越界 -> 白）。
pub fn color_of(color_id: i32) -> LiveMessageColor {
    match color_id {
        1 => LiveMessageColor::new(255, 0, 0),
        2 => LiveMessageColor::new(30, 135, 240),
        3 => LiveMessageColor::new(122, 200, 75),
        4 => LiveMessageColor::new(255, 127, 0),
        5 => LiveMessageColor::new(155, 57, 244),
        6 => LiveMessageColor::new(255, 105, 180),
        _ => LiveMessageColor::white(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::protocol::stt_to_value;
    use super::*;
    use crate::models::LiveMessageData;

    #[test]
    fn fan_chatmsg_is_parsed_with_color() {
        let value = parse_stt("type@=chatmsg/nn@=观众/txt@=加油/col@=3/if@=1/");
        let msg = parse_message(&value).expect("粉丝发言应产出消息");
        assert_eq!(msg.message_type, LiveMessageType::Chat);
        assert_eq!(msg.user_name, "观众");
        assert_eq!(msg.message, "加油");
        assert!(msg.emoticons.is_none());
        assert_eq!(msg.color, LiveMessageColor::new(122, 200, 75));
    }

    #[test]
    fn chatmsg_without_if_field_is_kept() {
        // 线上主流形态：普通观众弹幕不带 `if` 字段，必须照常产出。
        let value = parse_stt("type@=chatmsg/nn@=普通观众/txt@=你好/col@=1/");
        let msg = parse_message(&value).expect("缺 if 的普通弹幕应产出消息");
        assert_eq!(msg.message_type, LiveMessageType::Chat);
        assert_eq!(msg.user_name, "普通观众");
        assert_eq!(msg.message, "你好");
    }

    #[test]
    fn live_observed_chatmsg_resolves_builtin_emoticon() {
        // 线上真实抓包（房间 1165924）：普通观众弹幕不带 if 字段，正文含标准表情码。
        let value = parse_stt(
            "type@=chatmsg/rid@=1165924/uid@=1/nn@=观众/txt@=哇 6666666666[爱你]/col@=1/cst@=1793496848454/",
        );
        let msg = parse_message(&value).expect("线上报文应产出消息");
        assert_eq!(msg.message, "哇 6666666666[爱你]", "正文保持原样");
        let emoticons = msg.emoticons.expect("内置静态表应命中 [爱你]");
        assert_eq!(emoticons.len(), 1);
        assert_eq!(emoticons[0].name.as_deref(), Some("[爱你]"));
        assert!(emoticons[0].url.contains("aini_"), "应解析到内置图片地址");
        assert!(!emoticons[0].large, "标准表情是行内小表情");
    }

    #[test]
    fn explicit_non_fan_chatmsg_is_dropped() {
        let value = parse_stt("type@=chatmsg/nn@=观众/txt@=阴间弹幕/col@=1/if@=0/");
        assert!(
            parse_message(&value).is_none(),
            "显式 if@=0 仍按 Dart 口径丢弃"
        );
    }

    #[test]
    fn fan_chatmsg_uses_builtin_static_emoticon_by_default() {
        let value = parse_stt("type@=chatmsg/nn@=观众/txt@=你好[财神]/col@=1/if@=1/");
        let msg = parse_message(&value).expect("应产出消息");
        let emoticons = msg.emoticons.expect("内置静态表情应命中");
        assert_eq!(emoticons.len(), 1);
        assert_eq!(emoticons[0].name.as_deref(), Some("[财神]"));
        assert!(emoticons[0].url.contains("caishen_"));
        assert!(!emoticons[0].large);
    }

    #[test]
    fn fan_chatmsg_is_enriched_with_douyu_emoticons() {
        let value = parse_stt("type@=chatmsg/nn@=作茧很孤独/txt@=[梗亲亲]/col@=1/if@=1/");
        let table = DouyuEmoticonTable::from([(
            "[梗亲亲]".to_string(),
            super::super::super::emoticon::DouyuEmoticon {
                url: "https://cdn.example.com/kiss.png".to_string(),
                width: Some(200),
                height: Some(200),
                large: true,
            },
        )]);
        let msg = parse_message_with_emoticons(&value, &table).expect("粉丝发言应产出消息");
        let emoticons = msg.emoticons.expect("应带表情");
        assert_eq!(emoticons.len(), 1);
        assert_eq!(emoticons[0].name.as_deref(), Some("[梗亲亲]"));
        assert_eq!(emoticons[0].url, "https://cdn.example.com/kiss.png");
        assert!(emoticons[0].large);
    }

    #[test]
    fn chatmsg_gag_becomes_popular_danmu_image() {
        let value = parse_stt("type@=chatmsg/nn@=观众/txt@=泰裤辣/gag@=197/col@=1/if@=1/");
        let table = DouyuEmoticonTable::from([(
            "gag:197".to_string(),
            super::super::super::emoticon::DouyuEmoticon {
                url: "https://cdn.example.com/taikula.png".to_string(),
                width: Some(266),
                height: Some(64),
                large: true,
            },
        )]);
        let msg = parse_message_with_emoticons(&value, &table).expect("应产出消息");
        assert_eq!(msg.message, "[梗弹幕]");
        let emoticons = msg.emoticons.expect("应带梗弹幕图片");
        assert_eq!(emoticons.len(), 1);
        assert_eq!(emoticons[0].name.as_deref(), Some("[梗弹幕]"));
        assert_eq!(emoticons[0].url, "https://cdn.example.com/taikula.png");
        assert_eq!(
            (emoticons[0].width, emoticons[0].height),
            (Some(266), Some(64))
        );
    }

    #[test]
    fn non_fan_chatmsg_is_dropped() {
        let value = parse_stt("type@=chatmsg/nn@=路人/txt@=路过/col@=1/if@=0/");
        assert!(
            parse_message(&value).is_none(),
            "显式 if@=0 的报文按 Dart 口径丢弃"
        );
        let missing_if = stt_to_value("type@=chatmsg/nn@=路人/txt@=x/col@=1/");
        assert!(
            parse_message(&missing_if).is_some(),
            "缺 if 是线上主流形态，不得视为非粉丝而丢弃"
        );
    }

    #[test]
    fn uenter_and_unknown_types_are_ignored() {
        assert!(parse_message(&stt_to_value("type@=uenter/uid@=1/")).is_none());
        assert!(parse_message(&stt_to_value("type@=gift/")).is_none());
        assert!(parse_message(&stt_to_value("type@=nonsense/")).is_none());
    }

    #[test]
    fn comm_chatmsg_becomes_super_chat() {
        let value = parse_stt(
            "type@=comm_chatmsg/cprice@=1990/cet@=300/now@=1700000000000/chatmsg@=nn@=土豪/txt@=666/ic@=face1/",
        );
        let msg = parse_message(&value).expect("醒目留言应产出消息");
        assert_eq!(msg.message_type, LiveMessageType::SuperChat);
        assert_eq!(msg.user_name, SC_PLACEHOLDER);
        let Some(LiveMessageData::SuperChat(sc)) = msg.data else {
            panic!("data 必须是 SuperChat");
        };
        assert_eq!(sc.user_name, "土豪");
        assert_eq!(sc.message, "666");
        assert_eq!(sc.price, 19, "1990 分 -> 19 元");
        assert_eq!(
            sc.face, "https://apic.douyucdn.cn/upload/face1_small.jpg",
            "头像地址按 Dart 模板拼接"
        );
        assert_eq!(sc.background_color, COMM_CHAT_BG);
        assert_eq!(
            sc.end_time - sc.start_time,
            chrono::Duration::seconds(300),
            "endTime = start + cet 秒"
        );
    }

    #[test]
    fn comm_chatmsg_without_face_still_produces_message() {
        let value = parse_stt(
            "type@=comm_chatmsg/cprice@=100/cet@=60/now@=1700000000000/chatmsg@=nn@=A/txt@=B/",
        );
        let msg = parse_message(&value).expect("缺 ic 也应产出（Dart 是 try/catch 后继续）");
        let Some(LiveMessageData::SuperChat(sc)) = msg.data else {
            panic!("data 必须是 SuperChat");
        };
        assert_eq!(sc.face, "https://apic.douyucdn.cn/upload/_small.jpg");
    }

    #[test]
    fn comm_chatmsg_falls_back_to_flat_fields() {
        // 服务端把 SC 内容平铺在顶层（无 chatmsg 容器）时也要能取到
        let value = parse_stt(
            "type@=comm_chatmsg/cprice@=100/cet@=60/now@=1700000000000/nn@=平铺/txt@=内容/ic@=face9/",
        );
        let Some(LiveMessageData::SuperChat(sc)) = parse_message(&value).expect("应产出").data
        else {
            panic!("data 必须是 SuperChat");
        };
        assert_eq!(sc.user_name, "平铺");
        assert_eq!(sc.message, "内容");
        assert_eq!(sc.face, "https://apic.douyucdn.cn/upload/face9_small.jpg");
    }

    #[test]
    fn voice_trlt_becomes_super_chat() {
        let value = parse_stt(
            "type@=voice_trlt/list@=0@un@=高能玩家/content@=冲鸭/realPrice@=5000/acptime@=1700000000/etime@=1700000030/uat@=1@=img.h5/avatar.png/",
        );
        let msg = parse_message(&value).expect("高能弹幕应产出消息");
        let Some(LiveMessageData::SuperChat(sc)) = msg.data else {
            panic!("data 必须是 SuperChat");
        };
        assert_eq!(sc.user_name, "高能玩家");
        assert_eq!(sc.message, "冲鸭");
        assert_eq!(sc.price, 50, "5000 分 -> 50 元");
        assert_eq!(
            sc.face, "https://img.h5/avatar.png",
            "uat 是头像 host，值里的裸 '/' 已并回（对齐 Dart 的 https:// + uat 拼接）"
        );
        assert_eq!(sc.background_color, VOICE_TRLT_BG);
        assert_eq!(sc.end_time - sc.start_time, chrono::Duration::seconds(30));
    }

    #[test]
    fn broken_super_chat_payload_is_dropped_not_panic() {
        // 缺 now / list[0] / acptime 等关键字段
        assert!(parse_message(&parse_stt("type@=comm_chatmsg/chatmsg@=nn@=A/")).is_none());
        assert!(parse_message(&parse_stt("type@=voice_trlt/list@=0@un@=A/")).is_none());
    }

    #[test]
    fn face_url_normalizes_scheme_and_empty() {
        assert_eq!(
            face_url("cdn.example.com/a.png"),
            "https://cdn.example.com/a.png"
        );
        assert_eq!(
            face_url("https:cdn.example.com/a.png"),
            "https://cdn.example.com/a.png",
            "STT 里被折叠成 'https:' 的 scheme 要补回斜杠"
        );
        assert_eq!(
            face_url("https://cdn.example.com/a.png"),
            "https://cdn.example.com/a.png",
            "已带 scheme 的原样返回，不能重复拼"
        );
        assert_eq!(face_url(""), "");
    }

    #[test]
    fn decode_frame_end_to_end() {
        let frame =
            super::super::protocol::serialize("type@=chatmsg/nn@=观众/txt@=你好/col@=1/if@=1/");
        let msgs = decode_frame(&frame);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].message, "你好");
        assert!(decode_frame(&[0u8; 4]).is_empty(), "脏帧不 panic");
        assert!(decode_frame(&[]).is_empty());
    }

    #[test]
    fn color_table_covers_dart_mapping() {
        assert_eq!(color_of(1), LiveMessageColor::new(255, 0, 0));
        assert_eq!(color_of(2), LiveMessageColor::new(30, 135, 240));
        assert_eq!(color_of(3), LiveMessageColor::new(122, 200, 75));
        assert_eq!(color_of(4), LiveMessageColor::new(255, 127, 0));
        assert_eq!(color_of(5), LiveMessageColor::new(155, 57, 244));
        assert_eq!(color_of(6), LiveMessageColor::new(255, 105, 180));
        assert_eq!(color_of(0), LiveMessageColor::white());
        assert_eq!(color_of(99), LiveMessageColor::white());
    }
}
