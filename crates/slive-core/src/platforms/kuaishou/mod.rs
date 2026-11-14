//! 快手平台适配器（IP-7）。
//!
//! 行为基准：PureLive `live_core/src/sites/kuaishou/*.dart`。
//! - REST JSON API (`/live_api/...`)
//! - 无需签名，匿名请求即可
//! - 分页：page 编号 + cursor 翻页
//! - 取流：txTime/wsTime/hwTime CDN 签名 URL

pub mod api;
pub mod detail;
pub mod play;
pub mod site;

pub use site::KuaishouSite;