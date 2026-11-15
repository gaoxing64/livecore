//! YouTube 直播弹幕（InnerTube `live_chat/get_live_chat` 轮询）。
//!
//! 与其余平台的 WS 会话不同，YouTube 聊天是 HTTP 轮询：
//! - 先取直播观看页 HTML，解出 `liveChatRenderer` 的 initial continuation；
//! - POST `youtubei/v1/live_chat/get_live_chat`，解析 `liveChatContinuation.actions`；
//! - 用响应里的 `timedContinuationData.timeoutMs` 决定下一次轮询间隔。
//!
//! 匿名可读（view-only）；发送弹幕需要登录，不在本模块范围。

pub mod args;
pub mod parse;
pub mod session;

pub use args::YouTubeDanmakuArgs;
