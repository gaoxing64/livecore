//! B站的 Rust Core 绑定层：`RustBilibiliSite implements LiveSite`。
//!
//! 纪律（AC-39 / 裁决 J-6 / R-13）：本层**只做类型翻译与转发**，零协议逻辑 ——
//! 不得出现 URL 拼接 / 签名 / 解析 / 重试；一切协议能力在 `slive-core`（Rust）。
//! 回退窗口已结束：`sites.dart` 直连本绑定层，无运行时开关；
//! Dart core 的 B站实现待 console 迁移后删除（console 暂留 Dart 路径）。

import 'package:simple_live_core/src/rust/api/slive/danmaku.dart' as rust_danmaku;
import 'package:simple_live_core/src/rust/api/slive/models/danmu.dart' as rust_danmu;
import 'package:simple_live_core/src/rust/api/slive/models/message.dart' as rust_msg;
import 'package:simple_live_core/src/rust/api/slive/site.dart' as rust_site;
import 'package:simple_live_core/simple_live_core.dart';

import 'rust_danmaku_base.dart';
import 'rust_site_handle.dart';
import 'rust_site_models.dart';

/// B站的 Rust Core 站点实现（D2：Dart 直连 FRB API，无 Dart 协议实现）。
class RustBilibiliSite extends LiveSite
    with RustSiteHandle<rust_site.LiveBilibiliSite> {
  RustBilibiliSite() {
    id = "bilibili";
    name = "哔哩哔哩直播";
  }

  @override
  Future<rust_site.LiveBilibiliSite> createSite() =>
      rust_site.LiveBilibiliSite.default_();

  @override
  void applyAttrs(rust_site.LiveBilibiliSite site, String attrsJson) =>
      site.setSiteAttrs(data: attrsJson);

  @override
  LiveDanmaku getDanmaku() => RustLiveDanmaku();

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
  Future<QRCode> getQRCode() async =>
      RustSiteModels.toCoreQrCode(await (await siteHandle()).getQrCode());

  @override
  Future<QRPollResult> getQRPollResult({required String qrcodeKey}) async =>
      RustSiteModels.toCoreQrPollResult(
        await (await siteHandle()).getQrPollResult(qrcodeKey: qrcodeKey),
      );

  @override
  Future<AccountInfo> getAccountInfo({required String credential}) async =>
      RustSiteModels.toCoreAccountInfo(
        await (await siteHandle()).getAccountInfo(credential: credential),
      );
}

/// 弹幕适配：core 的回调式 `LiveDanmaku` -> Rust 的 Stream。
///
/// `start(args)` 的 `args` 为 `detail.danmakuData` 原样透传（JSON 串，
/// Rust 侧 `from_json` 解析失败抛 `CoreError` —— J-10）。
class RustLiveDanmaku extends RustDanmakuBase<rust_danmaku.DanmakuConnection> {
  @override
  String get adapterName => 'RustLiveDanmaku';

  @override
  rust_danmaku.DanmakuConnection createConnection() =>
      rust_danmaku.DanmakuConnection();

  @override
  Stream<rust_danmu.LiveDanmuItem> openStream(
          rust_danmaku.DanmakuConnection conn, String args) =>
      conn.start(argsJson: args);

  @override
  void closeConnection(rust_danmaku.DanmakuConnection conn) => conn.stop();

  @override
  LiveMessage toCoreMessage(rust_msg.LiveMessage item) =>
      RustSiteModels.toCoreMessage(item);
}
