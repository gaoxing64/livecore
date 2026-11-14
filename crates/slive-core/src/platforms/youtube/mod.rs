//! YouTube 平台适配器（IP-11）。
//!
//! 行为基准：PureLive `live_core/src/sites/youtube/*.dart`。
//! - InnerTube GraphQL-like JSON API (`POST /youtubei/v1/...`)
//! - 取流用 ANDROID client 的 player；匿名被风控（LOGIN_REQUIRED/PO token）时如实报错
//! - 分页：continuation token

pub mod api;
pub mod play;
pub mod site;

pub use site::Site as YouTubeSite;
