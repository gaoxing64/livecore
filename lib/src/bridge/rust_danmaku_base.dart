//! 五个平台共享的弹幕适配骨架：core 的回调式 `LiveDanmaku` -> Rust 的 Stream。

import 'dart:async';

import 'package:simple_live_core/src/rust/api/slive/models/danmu.dart' as rust_danmu;
import 'package:simple_live_core/src/rust/api/slive/models/message.dart' as rust_msg;
import 'package:simple_live_core/simple_live_core.dart';

/// 弹幕适配骨架：连接句柄类型 [C] 各平台不同（`DanmakuConnection` /
/// `DouyinDanmakuConnection` 等），会话装配与事件分发全平台一致。
///
/// `start(args)` 的 `args` 为 `detail.danmakuData`，入参契约各平台不同
///（JSON 串 / 房间号 / 频道名，见 [`normalizeArgs`]）。
///
/// 多开播放（2026-10-18 修复）：每个实例自持一个连接句柄 —— 旧实现是
/// 全局单例，第二路 start 会把第一路 stop 掉，表现为「只有一路有弹幕，
/// 另一路聊天区显示已手动停止」。
abstract class RustDanmakuBase<C extends Object> extends LiveDanmaku {
  StreamSubscription<rust_danmu.LiveDanmuItem>? _subscription;
  C? _conn;
  bool _stopped = true;

  // 心跳由 Rust 会话内部管理，Dart 侧 heartbeat()/heartbeatTime 不参与。

  /// 创建各平台的弹幕连接句柄。
  C createConnection();

  /// 以归一化后的参数启动弹幕流。
  Stream<rust_danmu.LiveDanmuItem> openStream(C conn, String args);

  /// 关闭连接句柄。
  void closeConnection(C conn);

  /// Rust 消息 -> core 消息（各平台模型翻译器不同）。
  LiveMessage toCoreMessage(rust_msg.LiveMessage item);

  /// 报错文案用的适配器名。
  String get adapterName;

  /// Rust 会话异常退出（如线程 panic）时 sink 被 drop，流会静默 onDone；
  /// 置 `true` 的适配器在未手动停止时上报「弹幕连接已断开」，
  /// 避免 UI 看起来只是「弹幕卡住」没有任何提示。
  bool get notifyOnStreamDone => false;

  /// 把 `start(args)` 入参归一为交给 Rust 的字符串（各平台契约不同）。
  /// 默认只接受 JSON 串（Rust 契约）。
  String normalizeArgs(dynamic args) => args is String
      ? args
      : throw ArgumentError(
          '$adapterName.start 需要 danmakuData JSON 串，收到 ${args.runtimeType}',
        );

  @override
  Future<void> start(dynamic args) async {
    final argsString = normalizeArgs(args);
    // 新会话 = 新句柄：先停掉旧会话（换台 / 刷新路径），再多开时各停各的。
    await stop();
    _stopped = false;
    final conn = createConnection();
    _conn = conn;
    _subscription = openStream(conn, argsString).listen(
      (item) {
        switch (item) {
          case rust_danmu.LiveDanmuItem_Message(:final field0):
            onMessage?.call(toCoreMessage(field0));
          case rust_danmu.LiveDanmuItem_Control(:final field0):
            switch (field0) {
              case rust_danmu.LiveDanmuControlEvent_Ready():
                onReady?.call();
              case rust_danmu.LiveDanmuControlEvent_Reconnecting(:final attempt):
                onClose?.call('与服务器断开连接，正在尝试重连($attempt)');
              case rust_danmu.LiveDanmuControlEvent_Closed(:final field0):
                onClose?.call(field0);
            }
        }
      },
      onError: (Object e) {
        // 流级错误（解码失败等）不中断弹幕，记录即可
        // ignore: avoid_print
        CoreLog.error(e);
      },
      onDone: () {
        if (!_stopped && notifyOnStreamDone) {
          onClose?.call('弹幕连接已断开');
        }
      },
      cancelOnError: false,
    );
  }

  @override
  Future<void> stop() async {
    _stopped = true;
    final conn = _conn;
    if (conn != null) {
      closeConnection(conn);
    }
    await _subscription?.cancel();
    _subscription = null;
    _conn = null;
  }
}
