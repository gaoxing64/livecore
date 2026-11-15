//! 快手 ZtLive link IM 协议（AcFun 直播弹幕共用）。
//!
//! AcFun 直播页内嵌的 `kwai-acfun-live-link` 与快手同源：
//! - WebSocket 二进制帧 = 12 字节头 + PacketHeader + 载荷；
//! - 载荷是 `UpstreamPayload` / `DownstreamPayload`，用 AES-CBC（PKCS#7）
//!   以服务 token 或注册下发的会话密钥加密；
//! - 注册（`Basic.Register`）→ 进房（`Global.ZtLiveInteractive.CsCmd`）→
//!   心跳（`ZtLiveCsHeartbeat`，间隔由进房应答下发）/ 保活（`Basic.KeepAlive`）；
//! - 推送（`Push.ZtLiveInteractive.Message`）解出 `ZtLiveSc*Signal`，
//!   其中 `CommonActionSignalComment` 是聊天、`CommonStateSignalDisplayInfo` 是在线人数。
//!
//! 快手站点的票据接口（`live_api/liveroom/websocketinfo`）当前对匿名请求返回
//! 验证码（`result=400002`），因此本模块先只服务 AcFun；协议本身不依赖站点。

pub mod args;
pub mod crypto;
pub mod frame;
pub mod messages;
pub mod proto;
pub mod session;

pub use args::ZtLiveDanmakuArgs;
