// Timing, not a check: skipped unless HEARTH_BENCH=1. Measures, over the host
// build of the bridge (HEARTH_SYNC_LIB picks which build), what the kernel costs
// per op on the two hot paths, and what Ed25519 signing costs on each backend the
// host reaches (WebCrypto only runs in a browser).
//   HEARTH_BENCH=1 flutter test --concurrency=1 test/bench_test.dart
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:hearth_sync/hearth_sync.dart';

import 'support.dart';

void main() {
  final n = int.tryParse(Platform.environment['HEARTH_BENCH_N'] ?? '') ?? 300;
  setUpAll(initBridge);

  test('per-op costs', () async {
    final clock = TestClock();
    final a = await device(1, clock);
    final b = await device(2, clock);

    final msg = Uint8List(200);
    final signUs = <SigningBackend, double>{};
    for (final backend in [SigningBackend.rust, SigningBackend.dart]) {
      final signer = SoftwareSigner.fromSeed(
        List.filled(32, 7),
        backend: backend,
      );
      await signer.sign(msg);
      final sw = Stopwatch()..start();
      for (var i = 0; i < n; i++) {
        await signer.sign(msg);
      }
      signUs[backend] = sw.elapsedMicroseconds / n;
    }
    Stopwatch sw;

    sw = Stopwatch()..start();
    for (var i = 0; i < n; i++) {
      clock.now += 1;
      await a.hs.put('items', 'i$i', {'name': 'item $i', 'value': i});
    }
    final writeUs = sw.elapsedMicroseconds / n;

    final sealed = [for (final s in a.sent) s.sealed];
    sw = Stopwatch()..start();
    await b.hs.ingest(sealed);
    final ingestUs = sw.elapsedMicroseconds / sealed.length;
    expect(b.hs.view().rows.length, n);

    // ignore: avoid_print
    print(
      'BENCH n=$n lib=${Platform.environment['HEARTH_SYNC_LIB'] ?? 'default'}\n'
      '  ed25519 sign, rust bridge: ${signUs[SigningBackend.rust]!.toStringAsFixed(1)} us/op\n'
      '  ed25519 sign, pure dart: ${signUs[SigningBackend.dart]!.toStringAsFixed(0)} us/op\n'
      '  write (kernel prepare + sign + kernel finish + store): ${writeUs.toStringAsFixed(0)} us/op\n'
      '  ingest of ${sealed.length} sealed ops (unseal + verify + fold): ${ingestUs.toStringAsFixed(0)} us/op',
    );
  }, skip: Platform.environment['HEARTH_BENCH'] != '1');
}
