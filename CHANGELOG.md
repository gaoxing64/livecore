## 2.0.0

- 独立仓库更名为 `livecore`，Dart 包名同步改为 `livecore`。
- Flutter Rust Bridge 构建插件更名为 `rust_lib_livecore`。
- 新增可供外部仓库调用的 C ABI GitHub Actions workflow，覆盖 Linux、macOS 与 Windows，
  并在版本 tag 上自动附加 C、C++、C# 所需求的头文件和原生库。
- 新增 AcFun 弹幕：link IM（`platforms/ztlive`，AES-CBC 载荷、注册 / 进房 / 心跳 /
  保活 / 推送确认、断线重连）。
- 新增 YouTube 弹幕：InnerTube `live_chat/get_live_chat` HTTP 轮询，支持聊天与
  Super Chat。
- `LiveSite.set_site_attrs` 的 Cookie 注入扩展到快手 / 微博 / YouTube（共享
  `credential` 解析，接受原始 Cookie 串或 `{"cookie":"..."}`）；CLI `--cookie-file`
  同步放开。
- CLI `danmaku` 支持 AcFun / YouTube；快手、微博、小红书 按平台限制
  返回明确原因。

## 1.1.0

- `LiveSite.getRecommendRooms` / `getCategoryRooms` 新增可选具名参数 `pageSize`：
  供调用方按窗口宽度调整单页条数，当前仅 bilibili 推荐流实际使用
  （`page_size` 参数化，默认 30），其余平台接受并忽略。
- ⚠️ **对实现 `LiveSite` 的下游是破坏性变更**：实现类必须同步补上
  `int? pageSize` 参数才能编译通过。仅调用（不实现）该接口的用法不受影响。
- `DouyuSite.getRecommendRooms` 不再依赖已失效的服务端 `pgcnt` 字段
  （该接口实测恒为 0），改为「本页非空即还有更多」。
  `DouyuSite.getCategoryRooms` 走另一接口，其 `pgcnt` 仍有效，判定保持不变。

## 1.0.0

- Initial version.
