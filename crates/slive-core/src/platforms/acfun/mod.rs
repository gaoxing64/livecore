//! ACFUN 平台适配器（IP-8）。
//!
//! 行为基准：PureLive `live_core/src/sites/acfun/*.dart`。
//! - REST JSON API (`/api/...`)
//! - 需 visitor login + token
//! - 分页：cursor 翻页

pub mod api;
pub mod detail;
pub mod play;
pub mod site;
pub mod visitor;

pub use site::AcfunSite;
