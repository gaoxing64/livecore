//! 五个站点共享的「FRB 句柄 + attrs 暂存」机制。

import 'package:simple_live_core/simple_live_core.dart';

/// 站点句柄骨架：FRB 句柄类型 [H] 各平台不同（`LiveBilibiliSite` 等），
/// 懒初始化与 attrs 暂存/下发逻辑全平台一致。
///
/// `setSiteAttrs` 是同步接口而 FRB 句柄初始化是异步 —— 暂存到队列，
/// 句柄可用后**按序下发**（Rust `set_site_attrs` 按 key 独立生效，
/// 多条 JSON 天然合并，消除「谁后谁赢」覆盖凭证的竞态）。
mixin RustSiteHandle<H extends Object> on LiveSite {
  Future<H>? _siteFuture;
  final List<String> _pendingAttrs = [];

  /// 创建 FRB 站点句柄（`XxxSite.default_()`）。
  Future<H> createSite();

  /// 对句柄下发一条 attrs JSON（`site.setSiteAttrs(data: ...)`）。
  void applyAttrs(H site, String attrsJson);

  /// 暂存 attrs；句柄可用后按序下发。
  void queueAttrs(String attrsJson) => _pendingAttrs.add(attrsJson);

  /// 懒初始化句柄；首次返回前把暂存的 attrs 按序下发。
  Future<H> siteHandle() async {
    final site = await (_siteFuture ??= createSite());
    if (_pendingAttrs.isNotEmpty) {
      final pending = List<String>.from(_pendingAttrs);
      _pendingAttrs.clear();
      for (final attrs in pending) {
        applyAttrs(site, attrs);
      }
    }
    return site;
  }

  @override
  void setSiteAttrs(String data) => queueAttrs(data);
}
