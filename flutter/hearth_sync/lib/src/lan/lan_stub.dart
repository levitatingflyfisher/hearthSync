// The web build's same-Wi-Fi sync: a browser page cannot listen for another
// phone, so PWAs sync through the relay (ADR 0014).
import 'dart:math';
import 'dart:typed_data';

import '../hearth_sync.dart';
import 'lan_code.dart';

/// Same-Wi-Fi sync can run here.
const lanSupported = false;

/// Not on the web: always throws [LanException] `unsupported`.
class LanListener {
  LanListener._();

  /// Throws.
  static Future<LanListener> start(
    HearthSync hs,
    Uint8List seed, {
    String? advertise,
    int port = 0,
    LanLimits limits = const LanLimits(),
    Random? random,
  }) async => throw const LanException('unsupported');

  /// Never shown.
  LanCode get code => throw const LanException('unsupported');

  /// Never completes normally.
  Future<LanSyncResult> get done => Future.error(const LanException('unsupported'));

  /// Nothing to stop.
  Future<void> stop() async {}
}

/// Not on the web: always throws [LanException] `unsupported`.
Future<void> syncOverLan(
  HearthSync hs,
  Uint8List seed,
  LanCode code, {
  LanLimits limits = const LanLimits(),
}) async => throw const LanException('unsupported');

/// None on the web.
Future<String?> localLanAddress() async => null;
