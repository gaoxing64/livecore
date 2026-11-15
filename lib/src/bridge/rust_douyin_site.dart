//! 抖音的 Rust Core 绑定层：`RustDouyinSite extends LiveSite`。
//!
//! 纪律（AC-39 / 裁决 J-6 / R-13）：本层**只做类型翻译与转发**，零协议逻辑 ——
//! 不得出现 URL 拼接 / a_bogus 签名 / protobuf 编解码 / 重连；一切协议能力在
//! `slive-core`（Rust）。回退窗口已结束：`sites.dart` 直连本绑定层。
//!
//! 与 `RustBilibiliSite` 的三处差异：
//! 1. `getDanmaku()` 返回抖音专属适配器（protobuf over WS 的参数契约键名不同）；
//! 2. `searchAnchors` / `getQRCode` / `getQRPollResult` 在抖音是**不支持**的能力，
//!    Rust 侧恒报错，这里不做「假装成功」的空实现；
//! 3. 清晰度 `data` 通道是地址数组的 JSON 串（见 `RustDouyinModels`）。

import 'package:livecore/src/rust/api/slive/douyin.dart' as rust_douyin;
import 'package:livecore/src/rust/api/slive/models/danmu.dart' as rust_danmu;
import 'package:livecore/src/rust/api/slive/models/message.dart' as rust_msg;
import 'package:livecore/livecore.dart';

import 'rust_danmaku_base.dart';
import 'rust_douyin_models.dart';
import 'rust_site_handle.dart';
import 'rust_site_models.dart';

/// 抖音的 Rust Core 站点实现（类型翻译 + 转发，零协议逻辑）。
class RustDouyinSite extends LiveSite
    with RustSiteHandle<rust_douyin.LiveDouyinSite> {
  RustDouyinSite() {
    id = "douyin";
    name = "抖音直播";
  }

  @override
  Future<rust_douyin.LiveDouyinSite> createSite() =>
      rust_douyin.LiveDouyinSite.default_();

  @override
  void applyAttrs(rust_douyin.LiveDouyinSite site, String attrsJson) =>
      site.setSiteAttrs(data: attrsJson);

  @override
  void setSiteAttrs(String data) {
    // 空串 = 未登录账号的 credential（`accountLogin` 既有路径）。空串不是合法
    // JSON，core 层虽已容错，这里仍显式跳过 —— 暂存语义只接受有效 JSON。
    if (data.isEmpty) return;
    super.setSiteAttrs(data);
  }

  @override
  LiveDanmaku getDanmaku() => RustDouyinDanmaku();

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

  /// 抖音不支持搜索主播 —— Rust 侧恒报错，这里直接转发错误（不假装返回空结果）。
  @override
  Future<LiveSearchAnchorResult> searchAnchors(String keyword,
          {int page = 1}) async =>
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
        await (await siteHandle()).getRecommendRooms(page: page, pageSize: pageSize),
      );

  @override
  Future<LiveRoomDetail> getRoomDetail({required String roomId}) async =>
      RustDouyinModels.toCoreRoomDetail(
        await (await siteHandle()).getRoomDetail(roomId: roomId),
      );

  @override
  Future<List<LivePlayQuality>> getPlayQualities(
          {required LiveRoomDetail detail}) async =>
      (await (await siteHandle()).getPlayQualities(
        detail: RustDouyinModels.toRustRoomDetail(detail),
      ))
          .map(RustDouyinModels.toCorePlayQuality)
          .toList();

  @override
  Future<LivePlayUrl> getPlayUrls(
          {required LiveRoomDetail detail, required LivePlayQuality quality}) async =>
      RustDouyinModels.toCorePlayUrl(
        await (await siteHandle()).getPlayUrls(
          detail: RustDouyinModels.toRustRoomDetail(detail),
          quality: RustDouyinModels.toRustPlayQuality(quality),
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

  /// 抖音不支持扫码登录 —— 转发 Rust 侧错误，不返回伪造的失败对象。
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
      RustDouyinModels.toCoreAccountInfo(
        await (await siteHandle()).getAccountInfo(credential: credential),
      );
}

/// 抖音弹幕适配：core 的回调式 `LiveDanmaku` -> Rust 的 Stream。
///
/// `start(args)` 的 `args` 为 `detail.danmakuData` 原样透传（JSON 串，
/// Rust 侧 `from_json` 解析失败抛 `CoreError` —— J-10）。
/// 心跳由 Rust 会话内部管理（10s），Dart 侧 heartbeat()/heartbeatTime 不参与。
class RustDouyinDanmaku
    extends RustDanmakuBase<rust_douyin.DouyinDanmakuConnection> {
  @override
  String get adapterName => 'RustDouyinDanmaku';

  @override
  rust_douyin.DouyinDanmakuConnection createConnection() =>
      rust_douyin.DouyinDanmakuConnection();

  @override
  Stream<rust_danmu.LiveDanmuItem> openStream(
          rust_douyin.DouyinDanmakuConnection conn, String args) =>
      conn.start(argsJson: args);

  @override
  void closeConnection(rust_douyin.DouyinDanmakuConnection conn) => conn.stop();

  @override
  LiveMessage toCoreMessage(rust_msg.LiveMessage item) =>
      RustDouyinModels.toCoreMessage(item);
}
