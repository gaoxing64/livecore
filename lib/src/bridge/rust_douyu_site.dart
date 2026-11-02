//! 斗鱼的 Rust Core 绑定层：`RustDouyuSite implements LiveSite`。
//!
//! 纪律（AC-39 / 裁决 J-6 / R-13）：本层**只做类型翻译与转发**，零协议逻辑 ——
//! 不得出现 URL 拼接 / 签名 / STT 解析 / 重试；一切协议能力在 `slive-core`（Rust）。
//! 回退窗口已结束：`sites.dart` 直连本绑定层，无运行时开关。

import 'package:simple_live_core/src/rust/api/slive/douyu.dart' as rust_douyu;
import 'package:simple_live_core/src/rust/api/slive/models/danmu.dart' as rust_danmu;
import 'package:simple_live_core/src/rust/api/slive/models/message.dart' as rust_msg;
import 'package:simple_live_core/simple_live_core.dart';

import 'rust_danmaku_base.dart';
import 'rust_douyu_models.dart';
import 'rust_site_handle.dart';

/// 斗鱼的 Rust Core 站点实现（D2：Dart 直连 FRB API，无 Dart 协议实现）。
class RustDouyuSite extends LiveSite
    with RustSiteHandle<rust_douyu.LiveDouyuSite> {
  RustDouyuSite() {
    id = "douyu";
    name = "斗鱼直播";
  }

  @override
  Future<rust_douyu.LiveDouyuSite> createSite() =>
      rust_douyu.LiveDouyuSite.default_();

  @override
  void applyAttrs(rust_douyu.LiveDouyuSite site, String attrsJson) =>
      site.setSiteAttrs(data: attrsJson);

  @override
  void setSiteAttrs(String data) {
    // 空串 = 未登录账号的 credential（`accountLogin` 既有路径）。空串不是合法
    // JSON，core 层虽已容错，这里仍显式跳过 —— 暂存语义只接受有效 JSON。
    if (data.isEmpty) return;
    super.setSiteAttrs(data);
  }

  @override
  LiveDanmaku getDanmaku() => RustDouyuDanmaku();

  @override
  Future<List<LiveCategory>> getCategories() async {
    final list = await (await siteHandle()).getCategories();
    return list.map(RustDouyuModels.toCoreCategory).toList();
  }

  @override
  Future<LiveSearchRoomResult> searchRooms(String keyword, {int page = 1}) async =>
      RustDouyuModels.toCoreSearchRoom(
        await (await siteHandle()).searchRooms(keyword: keyword, page: page),
      );

  @override
  Future<LiveSearchAnchorResult> searchAnchors(String keyword, {int page = 1}) async =>
      RustDouyuModels.toCoreSearchAnchor(
        await (await siteHandle()).searchAnchors(keyword: keyword, page: page),
      );

  @override
  Future<LiveCategoryResult> getCategoryRooms(
    LiveSubCategory category, {
    int page = 1,
    int? pageSize,
  }) async =>
      RustDouyuModels.toCoreCategoryResult(
        await (await siteHandle()).getCategoryRooms(
          category: RustDouyuModels.toRustSubCategory(category),
          page: page,
          pageSize: pageSize,
        ),
      );

  @override
  Future<LiveCategoryResult> getRecommendRooms({int page = 1, int? pageSize}) async =>
      RustDouyuModels.toCoreCategoryResult(
        await (await siteHandle())
            .getRecommendRooms(page: page, pageSize: pageSize),
      );

  @override
  Future<LiveRoomDetail> getRoomDetail({required String roomId}) async =>
      RustDouyuModels.toCoreRoomDetail(
        await (await siteHandle()).getRoomDetail(roomId: roomId),
      );

  @override
  Future<List<LivePlayQuality>> getPlayQualities(
      {required LiveRoomDetail detail}) async =>
      (await (await siteHandle()).getPlayQualities(
        detail: RustDouyuModels.toRustRoomDetail(detail),
      ))
          .map(RustDouyuModels.toCorePlayQuality)
          .toList();

  @override
  Future<LivePlayUrl> getPlayUrls(
      {required LiveRoomDetail detail, required LivePlayQuality quality}) async =>
      RustDouyuModels.toCorePlayUrl(
        await (await siteHandle()).getPlayUrls(
          detail: RustDouyuModels.toRustRoomDetail(detail),
          quality: RustDouyuModels.toRustPlayQuality(quality),
        ),
      );

  @override
  Future<bool> getLiveStatus({required String roomId}) async =>
      (await siteHandle()).getLiveStatus(roomId: roomId);

  @override
  Future<List<LiveSuperChatMessage>> getSuperChatMessage(
          {required String roomId}) async =>
      (await (await siteHandle()).getSuperChatMessages(roomId: roomId))
          .map(RustDouyuModels.toCoreSuperChat)
          .toList();

  @override
  Future<QRCode> getQRCode() async =>
      RustDouyuModels.toCoreQrCode(await (await siteHandle()).getQrCode());

  @override
  Future<QRPollResult> getQRPollResult({required String qrcodeKey}) async =>
      RustDouyuModels.toCoreQrPollResult(
        await (await siteHandle()).getQrPollResult(qrcodeKey: qrcodeKey),
      );

  @override
  Future<AccountInfo> getAccountInfo({required String credential}) async =>
      RustDouyuModels.toCoreAccountInfo(
        await (await siteHandle()).getAccountInfo(credential: credential),
      );
}

/// 弹幕适配：core 的回调式 `LiveDanmaku` -> Rust 的 Stream。
///
/// `start(args)` 的 `args` 为 `detail.danmakuData`，斗鱼即**房间号字符串**
///（Rust 侧空值/非法值抛 `CoreError` —— J-10）。
///
/// 心跳由 Rust 会话内部管理（45s，与 Dart `DouyuDanmaku.heartbeatTime` 一致），
/// Dart 侧 heartbeat()/heartbeatTime 不参与。
class RustDouyuDanmaku extends RustDanmakuBase<rust_douyu.DouyuDanmakuConnection> {
  @override
  String get adapterName => 'RustDouyuDanmaku';

  @override
  String normalizeArgs(dynamic args) => args is String ? args : (args?.toString() ?? '');

  @override
  rust_douyu.DouyuDanmakuConnection createConnection() =>
      rust_douyu.DouyuDanmakuConnection();

  @override
  Stream<rust_danmu.LiveDanmuItem> openStream(
          rust_douyu.DouyuDanmakuConnection conn, String args) =>
      conn.start(roomId: args);

  @override
  void closeConnection(rust_douyu.DouyuDanmakuConnection conn) => conn.stop();

  @override
  LiveMessage toCoreMessage(rust_msg.LiveMessage item) =>
      RustDouyuModels.toCoreMessage(item);
}
