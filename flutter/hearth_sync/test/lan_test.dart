// Same-Wi-Fi sync (ADR 0014): two in-process devices over a real HTTP socket on
// loopback. One listens and shows a code; the other connects with it.
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:hearth_sync/hearth_sync.dart';
import 'package:hearth_sync/src/lan/lan_io.dart' show LanSession;

import 'support.dart';

/// Another household's seed.
final strangerSeed = Uint8List.fromList(List.generate(64, (i) => 255 - i));

Future<LanListener> listen(Device d, {LanLimits limits = const LanLimits()}) =>
    LanListener.start(d.hs, seed, advertise: '127.0.0.1', limits: limits);

void main() {
  setUpAll(initBridge);
  setUp(() => HttpOverrides.global = null);

  test('two devices sync both ways over a code shown by the listener', () async {
    final clock = TestClock();
    final a = await device(1, clock);
    final b = await device(2, clock);
    clock.now += 1;
    await a.hs.put('rooms', 'den', {'name': 'Den'});
    await b.hs.setAdd('tags', 'fragile');
    clock.now += 1;

    final listener = await listen(a);
    expect(listener.code.host, '127.0.0.1');
    await syncOverLan(b.hs, seed, LanCode.tryParse(listener.code.text)!);
    final result = await listener.done;
    expect(result.peer, isNotNull);

    for (final d in [a, b]) {
      d.check();
    }
    expect(b.mirror.rows['rooms/den'], {'name': 'Den'});
    expect(a.mirror.sets, {'tags/fragile'});
    expect(a.mirror.snapshot, b.mirror.snapshot);
  });

  test('a phone outside the household is refused, and nothing changes', () async {
    final clock = TestClock();
    final a = await device(1, clock);
    await a.hs.put('rooms', 'den', {'name': 'Den'});
    final before = a.mirror.snapshot.toString();
    final stranger = await HearthSync.open(
      app: app,
      schema: support_schema,
      persist: MemoryPersist(),
      signer: SoftwareSigner.fromSeed(List.filled(32, 3)),
      seed: strangerSeed,
      label: 'stranger',
      clock: clock.call,
    );
    final listener = await listen(a);
    final done = listener.done.then<Object>((r) => r, onError: (Object e) => e);

    await expectLater(
      syncOverLan(stranger, strangerSeed, listener.code),
      throwsA(isA<LanException>().having((e) => e.code, 'code', 'refused')),
    );
    expect(stranger.view().rows, isEmpty);
    expect(a.mirror.snapshot.toString(), before);
    await listener.stop();
    expect(await done, isA<LanException>());
  });

  test('a replayed message is refused and ends the session', () async {
    final clock = TestClock();
    final a = await device(1, clock);
    final b = await device(2, clock);
    final listener = await listen(a);
    final session = await LanSession.open(b.hs, seed, listener.code);
    await session.hello();
    // Send seq 1 again, with the MAC it legitimately had.
    final replay = session.lastSent!;
    final status = await session.raw(replay.method, replay.body, seq: replay.seq, mac: replay.mac);
    expect(status, HttpStatus.forbidden);
    // The session is over: even a correct next message is refused.
    await expectLater(session.hello(), throwsA(isA<LanException>()));
    await listener.stop();
  });

  test('a tampered message is refused', () async {
    final clock = TestClock();
    final a = await device(1, clock);
    final b = await device(2, clock);
    final listener = await listen(a);
    final session = await LanSession.open(b.hs, seed, listener.code);
    final hello = await b.hs.hello();
    final mac = await session.macFor('request', hello, seq: 1);
    final flipped = Uint8List.fromList(hello)..[hello.length ~/ 2] ^= 1;
    expect(await session.raw('request', flipped, seq: 1, mac: mac), HttpStatus.forbidden);
    await listener.stop();
  });

  test('a code works once', () async {
    final clock = TestClock();
    final a = await device(1, clock);
    final b = await device(2, clock);
    final listener = await listen(a);
    await syncOverLan(b.hs, seed, listener.code);
    await listener.done;
    await expectLater(
      syncOverLan(b.hs, seed, listener.code),
      throwsA(isA<LanException>()),
    );
  });

  test('a message over the size limit is refused', () async {
    final clock = TestClock();
    final a = await device(1, clock);
    final b = await device(2, clock);
    final listener = await listen(a, limits: const LanLimits(maxMessageBytes: 1024));
    final session = await LanSession.open(b.hs, seed, listener.code);
    final big = Uint8List(4096);
    final mac = await session.macFor('request', big, seq: 1);
    expect(
      await session.raw('request', big, seq: 1, mac: mac),
      HttpStatus.requestEntityTooLarge,
    );
    await listener.stop();
  });

  test('an unanswered code expires', () async {
    final clock = TestClock();
    final a = await device(1, clock);
    final listener = await listen(
      a,
      limits: const LanLimits(lifetime: Duration(milliseconds: 200)),
    );
    await expectLater(
      listener.done,
      throwsA(isA<LanException>().having((e) => e.code, 'code', 'expired')),
    );
  });
}
