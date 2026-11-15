# livecore

LiveCore 核心库。**协议实现 100% 在 Rust**（`crates/slive-core`），Dart 侧只保留跨平台模型与
FRB 绑定层 —— 不存在并行的 Dart 协议实现。

## 目录

| 路径 | 内容 |
|------|------|
| `Cargo.toml` / `Cargo.lock` | Rust workspace 根（成员：`slive-core`、`slive-cli`） |
| `crates/slive-core/` | **纯核心 crate**（零 FRB 依赖）：`platforms/{bilibili,douyu,huya,twitch,douyin,kuaishou,acfun,weibo,xiaohongshu,youtube}`、`tars/`、`ws/`、`http/`、`time/`、`models/` |
| `crates/slive-cli/` | 命令行工具 `slive`：只依赖 `slive-core` + `slive-host`，**不经过 FRB** |
| `crates/slive-host/` | **宿主层**：真实 WS/TCP 连接 + 弹幕会话装配（B站/斗鱼/虎牙/抖音/Twitch）（CLI / FRB 桥 / C ABI 共用） |
| `crates/slive-ffi/` | **C ABI**（`cdylib`/`staticlib`）：`include/slive.h` + `slive.hpp`(C++) + `csharp/Slive.cs`(WinUI3) |
| `src/` | FRB 桥层（`api/slive/**`；含既有 `danmaku_mask` 模块）+ `frb_generated.rs` |
| `crates/slive-core/tests/` | 集成测试 + 离线夹具（`tests/fixtures/{platform}/`） |
| `lib/src/rust/` | FRB **Dart 生成物**（已入库） |
| `lib/src/bridge/` | **Dart 绑定层**（`rust_*_site.dart`：FRB 模型 ↔ Dart 模型 1:1 翻译，零协议逻辑） |
| `lib/src/{model,interface}/` | App/TV 消费的跨平台模型与 `LiveSite`/`LiveDanmaku` 契约 |
| `rust_builder/` | cargokit 构建插件（包名 `rust_lib_livecore`，5 平台） |
| `flutter_rust_bridge.yaml` | codegen 配置（`rust_root: .` / `dart_output: lib/src/rust`） |

## 跑测试（无需 Flutter 工具链）

```bash
cd livecore
cargo test                       # 全量（990 用例）
cargo test -p slive-core         # 仅纯核心
cargo test -p slive-cli          # 仅 CLI（URL 解析）
cargo clippy --all-targets       # 静态检查
```

## 命令行

```bash
cd livecore
cargo build -p slive-cli --release
./target/release/slive info    https://live.bilibili.com/7734200
./target/release/slive danmaku https://live.bilibili.com/6
./target/release/slive rooms   douyin --limit 20
./target/release/slive rooms   kuaishou --limit 20
./target/release/slive info    https://live.acfun.cn/live/173620
./target/release/slive info    https://live.douyin.com/80126552431 --cookie-file /path/to/douyin.cookie
./target/release/slive help
```

`info` 输出房间详情、全部清晰度与直链（含请求头）；`danmaku` 持续打印弹幕，Ctrl-C 退出；
`rooms` 输出推荐房间。`--cookie-file` 目前用于抖音，文件内容只作为 Cookie 请求头注入，
不会写入日志。也可通过环境变量 `SLIVE_COOKIE_FILE` 传入路径，避免 Cookie 出现在进程参数中。

## 平台状态（2026-11）

- **B站 / 斗鱼 / 虎牙 / 抖音 / Twitch**：详情、取流、弹幕完整；Twitch 的分类 / 推荐 / 搜索见下节。
- **快手**：8 个固定分类、首页推荐、搜索、房间详情、画质与取流已接入（房间页 `__INITIAL_STATE__`，匿名会话兜底）；区域列表（gameboard / non-gameboard）待接入。
- **ACFUN**：房间详情、visitor 会话（5 分钟缓存）、`startPlay` 画质与取流已接入；分类 / 推荐 / 搜索待接入。
- **微博**：推荐快照、昵称 / 场次搜索、单场详情、直播 FLV 与回放 HLS 已接入（搜索只筛官方推荐快照，非全站）。
- **小红书**：分享页详情、受限判定、多 CDN 取流已接入；目录 / 关键词搜索需要浏览器访客会话（本 core 不提供）。
- **YouTube**：列表 / 搜索 / 详情已接入；匿名取流被平台要求登录或 PO token，质量接口会如实报错。
- **弹幕**：仍只覆盖 B站 / 斗鱼 / 虎牙 / 抖音 / Twitch。

真机验证（mpv 解码）：Twitch 1080p60 H.264、ACFUN 1080p59 4Mbps、快手 1080p30 8Mbps、微博 720×1280 H.264、小红书 1440×2560 60fps，均为 H.264 + AAC。

## Twitch 列表能力

Twitch 的分类、首页推荐与搜索按 PureLive `live_core` 的 Twitch 实现重写（`crates/slive-core/src/platforms/twitch/{catalog,rooms,search}.rs`）；详情、取流、弹幕仍沿用旧 Dart core 的迁移结果。

- `get_categories`：`SearchCategoryTags` + `BrowsePage_AllDirectories`，中文名称（缺中文名时再发一次英文请求补），目录 slug 作为 `LiveSubCategory.id` 供 `get_category_rooms` 使用；
- `get_recommend_rooms` / `get_category_rooms`：全站 `streams` raw query / `DirectoryPage_Game`，按 cursor 抓块后存成句柄级快照，再按 `page` 切片；
- `search_rooms` / `search_anchors`：`SearchResultsPage_SearchResults`，沿 CHANNEL cursor 翻页；前者只给在播频道，后者含未开播（`live_status` 标记）。

列表请求用 `Accept-Language: zh-CN`，详情 / 取流链路保持 en-US；匿名客户端在后续页被要求 integrity token 时列表安静收尾。离线契约见 `crates/slive-core/tests/ac_twitch_lists.rs`。

## 重跑 codegen

改了 `src/api/**` 后，在 `livecore/` 下按 `flutter_rust_bridge.yaml` 生成
（`rust_input: crate::api` 自动扫描）；生成物入库。**两侧版本必须都是 2.13.0**。

## 构建入口

主 App `simple_live_app/` 与 TV app `simple_live_tv_app/` 从
`https://github.com/gaoxing64/livecore` 获取 Dart 包和 `rust_builder` 插件，共用同一 crate
与同一份生成物；`pubspec.lock` 锁定实际 commit。Dart 侧统一 `import 'package:livecore/livecore.dart'`（绑定层经库出口暴露，
`RustLib.init()` 见 `frb_generated.dart` 出口）。

设计文档（`docs/rust-core/`、`docs/decisions/` 与 `doc/` 知识库）已归档至备份分支
`backup/future-20261101-234115`。绑定层边界结论：桥层只做类型翻译与转发、零协议逻辑。

## 非 Dart 宿主（C# / C++ / Rust）

核心与传输已从 Dart 完全解耦：

| 目标 | 入口 |
|---|---|
| 纯 Rust | `slive_core::platforms::create_site(Platform::Bilibili)` + `slive_host::danmaku::spawn(...)` |
| C++ / Qt / MyGUI | `include/slive.hpp`（RAII 封装，`Runtime` / `Site` / `Danmaku` / `Mask`） |
| C# / WinUI 3 | `csharp/Slive.cs`（P/Invoke + `TaskCompletionSource`） |
| 裸 C | `include/slive.h` |

### GitHub Actions 构建与下载 C ABI

`LiveCore C ABI` workflow 会为 Linux x64/ARM64、macOS x64/ARM64 和 Windows x64 编译动态库、
静态库、C/C++ 头文件与 C# P/Invoke 包装器。推送 `v*` tag 后，workflow 会将各目标包附加到
GitHub Release；手动运行或从其他仓库调用时，构建包会作为 Actions artifact 提供。

其他仓库可以调用公开的 reusable workflow：

```yaml
jobs:
  livecore-c-abi:
    uses: gaoxing64/livecore/.github/workflows/build-c-abi.yml@future
    with:
      source_ref: future
```

`source_ref` 可指定 LiveCore 的分支、tag 或 commit。调用方 workflow 的运行页面会显示并保存
各目标 artifact。

### 本机编译 C ABI

在 LiveCore 仓库根目录运行以下命令，可按本机 Rust target 编译：

```bash
cargo build --locked --release -p slive-ffi
```

Windows 可用脚本编译 MSVC x64：

```powershell
.\crates\slive-ffi\build.ps1 -Release          # 产出 slive_ffi.dll + slive_ffi.lib
.\crates\slive-ffi\build.ps1 -Release -StaticCrt  # 静态 CRT（宿主必须同口径）
```

指定 Rust target 时可直接运行：

```bash
cargo build --locked --release -p slive-ffi --target <rust-target>
```

产物位于 `target/<rust-target>/release/`；不指定 target 时位于 `target/release/`。
Linux 本机编译需要 OpenSSL 开发文件和 `pkg-config`，运行动态库的系统也需要兼容的 OpenSSL
运行库。C/C++ 头文件已入库，无需 cbindgen。

### 调用模型

```c
SliveConfig cfg = { .struct_size = sizeof cfg, .abi_version = SLIVE_ABI_VERSION,
                    .on_complete = my_completion, .on_event = my_event, .on_log = my_log,
                    .user_data = ctx };
slive_init(&cfg);

SliveHandle site;
slive_site_create(SLIVE_PLATFORM_BILIBILI, &site);
slive_site_call(site, SLIVE_STR(SLIVE_METHOD_GET_ROOM_DETAIL),
                SLIVE_STR("{\"room_id\":\"6\"}"), slive_request_id_new());
/* my_completion 在专用线程串行收到 envelope JSON */
```

**线程约定**：所有回调在库内**单个专用线程**上串行触发。WinUI3 需 `DispatcherQueue.TryEnqueue`，
Qt 需 `QMetaObject::invokeMethod(..., Qt::QueuedConnection)` 切回 UI 线程。

**返回码**：`0` = 已受理（异步方法保证恰好一次 completion）；`1` = 业务错误（解析 envelope）；
负数 = ABI 错误（`-1` 无效句柄 / `-3` 非 UTF-8 / `-6` panic / `-9` 不支持 等，见 `slive.h`）。

## 验证

```powershell
.\scripts\test-all.ps1
```

覆盖：workspace 测试、新 crate clippy `-D warnings`、FRB 生成物漂移检查、C 头文件常量一致性。
