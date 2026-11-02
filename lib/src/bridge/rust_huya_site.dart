//! 虎牙的 Rust Core 绑定层：`RustHuyaSite implements LiveSite`。
//!
//! 纪律（AC-39 / 裁决 J-6 / R-13）：本层**只做类型翻译与转发**，零协议逻辑 ——
//! 不得出现 URL 拼接 / 签名 / TARS 解析 / 正则 / 重试；一切协议能力在
//! `slive-core`（Rust）。回退窗口已结束：`sites.dart` 直连本绑定层。
//!
//! 与 B站绑定的差异：虎牙 `detail.danmakuData` / `quality.data` 在 Rust 侧是
//! **JSON 串**（`schema_version` 契约），本层**原样透传**，不解析 ——
//! App 侧 `liveDanmaku.start(detail.danmakuData)` 与
//! `getPlayUrls(detail:, quality:)` 会把它带回 Rust（J-10：
//! 任何 Dart 消费者解析失败必须抛 `FormatException`，不得静默兜底）。

import 'package:simple_live_core/src/rust/api/slive/huya.dart' as rust_huya_danmaku;
import 'package:simple_live_core/src/rust/api/slive/huya_site.dart'
    as rust_huya;
import 'package:simple_live_core/src/rust/api/slive/models/danmu.dart' as rust_danmu;
import 'package:simple_live_core/src/rust/api/slive/models/message.dart' as rust_msg;
import 'package:simple_live_core/simple_live_core.dart';

import 'rust_danmaku_base.dart';
import 'rust_site_handle.dart';
import 'rust_site_models.dart';

/// 虎牙的 Rust Core 站点实现（D2：Dart 直连 FRB API，无 Dart 协议实现）。
class RustHuyaSite extends LiveSite
    with RustSiteHandle<rust_huya.LiveHuyaSite> {
  RustHuyaSite() {
    id = "huya";
    name = "虎牙直播";
  }

  @override
  Future<rust_huya.LiveHuyaSite> createSite() =>
      rust_huya.LiveHuyaSite.default_();

  @override
  void applyAttrs(rust_huya.LiveHuyaSite site, String attrsJson) =>
      site.setSiteAttrs(data: attrsJson);

  @override
  void setSiteAttrs(String data) {
    // 空串 = 未登录账号的 credential（`accountLogin` 既有路径）。空串不是合法
    // JSON，core 层虽已容错，这里仍显式跳过 —— 暂存语义只接受有效 JSON。
    if (data.isEmpty) return;
    super.setSiteAttrs(data);
  }

  @override
  LiveDanmaku getDanmaku() => RustHuyaDanmaku();

  @override
  Future<List<LiveCategory>> getCategories() async {
    final list = await (await siteHandle()).getCategories();
    return list.map(RustSiteModels.toCoreCategory).toList();
  }

  @override
  Future<LiveSearchRoomResult> searchRooms(String keyword, {int page = 1}) async =>
      RustSiteModels.toCoreSearchRoom(
        await (await siteHandle()).searchRooms(keyword: keyword, page: page),
      );

  @override
  Future<LiveSearchAnchorResult> searchAnchors(String keyword, {int page = 1}) async =>
      RustSiteModels.toCoreSearchAnchor(
        await (await siteHandle()).searchAnchors(keyword: keyword, page: page),
      );

  @override
  Future<LiveCategoryResult> getCategoryRooms(
    LiveSubCategory category, {
    int page = 1,
    int? pageSize,
  }) async =>
      RustSiteModels.toCoreCategoryResult(
        await (await siteHandle()).getCategoryRooms(
          category: RustSiteModels.toRustSubCategory(category),
          page: page,
          pageSize: pageSize,
        ),
      );

  @override
  Future<LiveCategoryResult> getRecommendRooms({int page = 1, int? pageSize}) async =>
      RustSiteModels.toCoreCategoryResult(
        await (await siteHandle())
            .getRecommendRooms(page: page, pageSize: pageSize),
      );

  @override
  Future<LiveRoomDetail> getRoomDetail({required String roomId}) async =>
      RustSiteModels.toCoreRoomDetail(
        await (await siteHandle()).getRoomDetail(roomId: roomId),
      );

  @override
  Future<List<LivePlayQuality>> getPlayQualities(
          {required LiveRoomDetail detail}) async =>
      (await (await siteHandle()).getPlayQualities(
        detail: RustSiteModels.toRustRoomDetail(detail),
      ))
          .map(RustSiteModels.toCorePlayQuality)
          .toList();

  @override
  Future<LivePlayUrl> getPlayUrls(
          {required LiveRoomDetail detail, required LivePlayQuality quality}) async =>
      RustSiteModels.toCorePlayUrl(
        await (await siteHandle()).getPlayUrls(
          detail: RustSiteModels.toRustRoomDetail(detail),
          quality: RustSiteModels.toRustPlayQuality(quality),
        ),
      );

  @override
  Future<bool> getLiveStatus({required String roomId}) async =>
      (await siteHandle()).getLiveStatus(roomId: roomId);

  @override
  Future<List<LiveSuperChatMessage>> getSuperChatMessage(
          {required String roomId}) async =>
      (await (await siteHandle()).getSuperChatMessages(roomId: roomId))
          .map(RustSiteModels.toCoreSuperChat)
          .toList();

  @override
  Future<AccountInfo> getAccountInfo({required String credential}) async =>
      RustSiteModels.toCoreAccountInfo(
        await (await siteHandle()).getAccountInfo(credential: credential),
      );

  @override
  Future<QRCode> getQRCode() async =>
      RustSiteModels.toCoreQrCode(await (await siteHandle()).getQrCode());

  @override
  Future<QRPollResult> getQRPollResult({required String qrcodeKey}) async =>
      RustSiteModels.toCoreQrPollResult(
        await (await siteHandle()).getQrPollResult(qrcodeKey: qrcodeKey),
      );
}

/// 弹幕适配：core 的回调式 `LiveDanmaku` -> Rust 的 Stream。
///
/// `start(args)` 的 `args` 为 `detail.danmakuData` 原样透传（JSON 串，
/// Rust 侧 `from_json` 解析失败抛 `CoreError` —— J-10）。
/// 心跳由 Rust 会话内部管理（60s），Dart 侧 `heartbeat()` / `heartbeatTime` 不参与。
class RustHuyaDanmaku
    extends RustDanmakuBase<rust_huya_danmaku.HuyaDanmakuConnection> {
  @override
  String get adapterName => 'RustHuyaDanmaku';

  /// 会话异常退出（如线程 panic）时上报「弹幕连接已断开」，
  /// 不接这个回调的话 UI 看起来就是"弹幕卡住"，没有任何提示。
  @override
  bool get notifyOnStreamDone => true;

  @override
  String normalizeArgs(dynamic args) {
    if (args is! String) {
      throw ArgumentError.value(
        args,
        'args',
        '虎牙弹幕参数应为 Rust 契约的 JSON 串（detail.danmakuData）',
      );
    }
    return args;
  }

  @override
  rust_huya_danmaku.HuyaDanmakuConnection createConnection() =>
      rust_huya_danmaku.HuyaDanmakuConnection();

  @override
  Stream<rust_danmu.LiveDanmuItem> openStream(
          rust_huya_danmaku.HuyaDanmakuConnection conn, String args) =>
      conn.start(argsJson: args);

  @override
  void closeConnection(rust_huya_danmaku.HuyaDanmakuConnection conn) =>
      conn.stop();

  @override
  LiveMessage toCoreMessage(rust_msg.LiveMessage item) =>
      RustSiteModels.toCoreMessage(item);
}
