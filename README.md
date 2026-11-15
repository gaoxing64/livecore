# LiveCore

[![Core CI](https://github.com/gaoxing64/livecore/actions/workflows/ci.yml/badge.svg?branch=future)](https://github.com/gaoxing64/livecore/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/gaoxing64/livecore)](https://github.com/gaoxing64/livecore/releases)

LiveCore 是一个面向直播应用、命令行工具和非 Dart 宿主的聚合直播核心库。直播平台的
协议、数据处理和会话管理均在 Rust 中实现；Dart 层仅提供跨平台模型、接口定义和
Flutter Rust Bridge（FRB）类型翻译。

项目同时提供 Rust、Dart/Flutter 和 C ABI 三种接入方式，可用于 Flutter、桌面应用、
命令行程序、C/C++ 项目及 C#/WinUI 3 应用。

## 设计目标

- **单一协议实现**：平台协议只存在于 Rust，不在 Dart 侧维护并行实现。
- **清晰分层**：核心协议、设备连接、语言绑定和宿主适配相互独立。
- **多语言复用**：同一套核心能力通过 Rust、Dart/FRB 与 C ABI 对外提供。
- **可验证行为**：集成测试、离线夹具、头文件契约和 codegen 漂移检查均纳入 CI。
- **匿名优先**：优先支持匿名访问；平台强制要求登录或凭据时，返回明确错误。

## 架构

```text
                 ┌─────────────────────┐
                 │      slive-core     │
                 │  platform protocols │
                 └──────────┬──────────┘
                            │
                 ┌──────────▼──────────┐
                 │      slive-host     │
                 │ HTTP/WS/TCP sessions│
                 └───────┬───────┬─────┘
                         │       │
             ┌───────────▼─┐ ┌───▼──────────┐
             │ slive-cli   │ │  slive-ffi   │
             └─────────────┘ └──────────────┘
                         │
             ┌───────────▼──────────────┐
             │ FRB bridge / Dart package│
             └──────────────────────────┘
```

## 仓库结构

| 路径 | 说明 |
|------|------|
| `crates/slive-core/` | 纯 Rust 核心，包含平台协议、数据模型、HTTP/TARS/WebSocket 辅助能力，不依赖 FRB。 |
| `crates/slive-host/` | 宿主层，负责真实连接、弹幕会话装配和运行时生命周期。 |
| `crates/slive-ffi/` | 稳定 C ABI，提供动态库、静态库、C/C++ 头文件和 C# P/Invoke 包装器。 |
| `crates/slive-cli/` | `slive` 命令行工具，直接调用 Rust 核心。 |
| `src/` | FRB 桥接层与生成到 Rust 的绑定代码。 |
| `lib/src/rust/` | FRB 生成的 Dart 绑定。 |
| `lib/src/bridge/` | FRB 模型到 Dart 公共模型的类型翻译层，不包含协议逻辑。 |
| `lib/src/{model,interface}/` | Flutter 应用使用的跨平台模型与 `LiveSite`/`LiveDanmaku` 接口。 |
| `rust_builder/` | Flutter 插件构建入口，包名为 `rust_lib_livecore`。 |
| `flutter_rust_bridge.yaml` | FRB codegen 配置。 |
| `.github/workflows/` | Rust CI、C ABI 跨平台构建与版本发布流程。 |
| `scripts/` | 本地完整验证脚本。 |

## 公开接口

| 宿主 | 入口 |
|------|------|
| Rust | `slive_core::platforms::{create_site, Platform}`、`slive_host` |
| Flutter / Dart | `package:livecore/livecore.dart`，初始化入口为 `RustLib.init()` |
| C | `crates/slive-ffi/include/slive.h` |
| C++ / Qt / MyGUI | `crates/slive-ffi/include/slive.hpp` |
| C# / WinUI 3 | `crates/slive-ffi/csharp/Slive.cs` |

## 平台能力

下表记录 2026-11-15 对全部已注册平台的联网实测结果。浏览和搜索使用 `slive` CLI；
播放列使用 `mpv v0.42.0` 解码至少两帧；弹幕列使用 15–30 秒实时会话并观察消息。

播放成功表示当前直链、请求头和本机网络能够完成解码，不代表长时间播放稳定性。

| 平台 | 浏览/推荐 | 搜索 | 播放 | 弹幕 |
|------|-----------|------|------|------|
| B 站 | ✅ | ✅ | ✅ | ✅ |
| 斗鱼 | ✅ | ✅ | ✅ | ✅ |
| 虎牙 | ✅ | ✅ | ✅ | ✅ |
| 抖音 | ✅ | ⛔ 主播搜索 | ✅ | ✅ |
| Twitch | ✅ | ✅ | ✅ | ✅ |
| 快手 | ✅ | ✅ | ✅ | ⛔ `websocketinfo` 匿名返回验证码 `result=400002` |
| AcFun | ✅ | ✅ | ✅ | ✅ 实机收到弹幕 |
| 微博 | ✅ | ✅ 推荐快照内搜索 | ✅ | ⛔ `wesync` 返回 `21301`（需登录） |
| 小红书 | — 仅分享页直达 | — | ✅ | ⛔ 需浏览器访客会话 |
| YouTube | ✅ | ✅ | ⛔ 匿名取流被拒（HTTP 400，需登录/PO token） | ✅ 实机收到弹幕 |

状态说明：

- `✅`：实测返回数据、成功解码或收到实时消息。
- `⛔`：当前平台策略阻止匿名访问，或缺少接入所需的会话/凭据。
- `—`：当前不提供该能力。

具体限制：

- 抖音 CLI 搜索只提供主播搜索入口，接口明确返回“暂不支持搜索主播”；直播间详情和弹幕正常。
- 快手搜索可返回快手账号，但精确关键词可能得到空结果；弹幕票据要求通过平台验证码。
- 微博搜索只在官方推荐快照内过滤，不等同于全站搜索；弹幕需要登录 Cookie。
- 小红书没有公开目录和关键词搜索接口；当前直播分享页可以解析详情并经 MPV 播放，弹幕需要浏览器访客会话。
- YouTube 列表、搜索、详情和弹幕可用；匿名 `player` 请求要求登录或 PO token，因此当前无法取流。

## 环境要求

- Rust stable 工具链。
- Dart/Flutter：仅在生成或使用 FRB 绑定时需要。
- Linux 编译 C ABI：OpenSSL 开发文件、`pkg-config` 及兼容的 OpenSSL 运行库。
- Windows 编译 C ABI：Visual Studio Build Tools 与 MSVC x64 工具链。

## Rust 构建与测试

在仓库根目录执行：

```bash
cargo build --locked -p slive-cli --release
cargo test --workspace
cargo clippy -p slive-core -p slive-host -p slive-ffi --all-targets -- -D warnings
```

仅验证核心库：

```bash
cargo test -p slive-core
```

完整本地验证：

```powershell
.\scripts\test-all.ps1
```

该脚本覆盖 workspace 测试、核心 crate clippy、FRB 生成物漂移和 C 头文件常量一致性。

## 命令行工具

构建 `slive`：

```bash
cargo build -p slive-cli --release
```

常用命令：

```bash
./target/release/slive info https://live.bilibili.com/7734200
./target/release/slive danmaku https://live.bilibili.com/6
./target/release/slive rooms douyin --limit 20
./target/release/slive rooms kuaishou --limit 20
./target/release/slive info https://live.acfun.cn/live/173620
./target/release/slive info https://live.douyin.com/80126552431 --cookie-file /path/to/douyin.cookie
./target/release/slive help
```

- `info`：输出房间详情、清晰度和播放直链。
- `danmaku`：持续输出弹幕，按 `Ctrl-C` 退出。
- `rooms`：输出推荐或分类房间。
- `--cookie-file`：从文件读取 Cookie；也可通过 `SLIVE_COOKIE_FILE` 传入路径，避免凭据出现在进程参数中。

## Flutter / Dart 接入

Flutter 应用通过 Git 依赖获取 Dart 包和 Rust 构建插件：

```yaml
dependencies:
  livecore:
    git:
      url: https://github.com/gaoxing64/livecore.git
      ref: future
  rust_lib_livecore:
    git:
      url: https://github.com/gaoxing64/livecore.git
      path: rust_builder
      ref: future
```

应用应提交 `pubspec.lock`，以锁定实际解析到的 LiveCore commit。

Dart 侧统一导入：

```dart
import 'package:livecore/livecore.dart';
```

FRB 桥接层只负责模型翻译、句柄管理和调用转发，平台协议不得进入 Dart 层。

## FRB 代码生成

修改 `src/api/**` 后，在仓库根目录执行：

```bash
flutter_rust_bridge_codegen generate
```

要求：

- `pubspec.yaml` 与 `Cargo.toml` 中的 Flutter Rust Bridge 版本必须一致。
- 当前版本为 `2.13.0`。
- 生成物 `src/frb_generated.rs` 与 `lib/src/rust/` 必须提交。

## C ABI

### 本机构建

```bash
cargo build --locked --release -p slive-ffi
```

指定 Rust target：

```bash
cargo build --locked --release -p slive-ffi --target <rust-target>
```

Windows 可使用构建脚本：

```powershell
.\crates\slive-ffi\build.ps1 -Release
.\crates\slive-ffi\build.ps1 -Release -StaticCrt
```

未指定 target 时产物位于 `target/release/`，指定后位于
`target/<rust-target>/release/`。

### GitHub Actions 构建与发布

`LiveCore C ABI` workflow 覆盖：

- Linux x86_64
- Linux aarch64
- macOS x86_64
- macOS aarch64
- Windows x86_64

每次构建上传 Actions artifacts；推送 `v*` 标签时，构建包会附加到 GitHub Release。
发布包包含动态库、静态库、C/C++ 头文件、C# 包装器和 SHA256 校验文件。

其他仓库可以调用可复用 workflow：

```yaml
jobs:
  livecore-c-abi:
    uses: gaoxing64/livecore/.github/workflows/build-c-abi.yml@future
    with:
      source_ref: future
```

`source_ref` 可指定 LiveCore 的分支、标签或 commit。

### 调用约定

```c
SliveConfig cfg = {
    .struct_size = sizeof cfg,
    .abi_version = SLIVE_ABI_VERSION,
    .on_complete = my_completion,
    .on_event = my_event,
    .on_log = my_log,
    .user_data = ctx,
};
slive_init(&cfg);

SliveHandle site;
slive_site_create(SLIVE_PLATFORM_BILIBILI, &site);
slive_site_call(
    site,
    SLIVE_STR(SLIVE_METHOD_GET_ROOM_DETAIL),
    SLIVE_STR("{\"room_id\":\"6\"}"),
    slive_request_id_new()
);
```

- 回调在库内单一专用线程上串行触发。
- WinUI 3 应通过 `DispatcherQueue.TryEnqueue` 切回 UI 线程。
- Qt 应使用 `QMetaObject::invokeMethod(..., Qt::QueuedConnection)` 切回 UI 线程。
- `0` 表示请求已受理；`1` 表示业务错误；负数表示 ABI 级错误。

完整返回码和结构体定义见 `crates/slive-ffi/include/slive.h`。

## 持续集成

| 工作流 | 触发条件 | 内容 |
|--------|----------|------|
| `ci.yml` | push、pull request | workspace 测试、clippy、头文件契约、codegen 漂移检查 |
| `build-c-abi.yml` | push、pull request、手动触发、外部调用、版本标签 | 五目标 C ABI 构建、artifact 上传、Release 发布 |

## 贡献

提交前应完成相关测试与静态检查。涉及平台接口时，应优先补充离线夹具或契约测试，
并明确记录匿名访问、登录要求、验证码和动态票据等平台限制。

仓库地址：[github.com/gaoxing64/livecore](https://github.com/gaoxing64/livecore)
