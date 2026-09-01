import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:hearth_sync/hearth_sync.dart';

import 'support.dart';

void main() {
  setUpAll(initBridge);

  test(
    'two devices sync edits, deletes, containers, sets and streams through the Dart api',
    () async {
      final clock = TestClock();
      final a = await device(1, clock);
      final b = await device(2, clock);
      clock.now += 1;
      await a.hs.put('rooms', 'den', {'name': 'Den'});
      await a.hs.put('boxes', 'b1', {'name': 'Box', 'room': 'den'});
      await a.hs.put('items', 'i1', {'name': 'Lamp', 'box': 'b1', 'value': 40});
      await b.hs.setAdd('tags', 'fragile');
      await b.hs.append('log', 7);
      clock.now += 1;
      await a.hs.syncWith(b.hs);
      for (final d in [a, b]) {
        d.check();
      }
      expect(b.mirror.rows['items/i1'], {
        'name': 'Lamp',
        'box': 'b1',
        'value': 40,
      });
      expect(a.mirror.sets, {'tags/fragile'});
      expect(a.mirror.streams.values, [7]);
      expect(a.mirror.snapshot, b.mirror.snapshot);

      // An edit beats a concurrent delete; a deleted room hides what it contains,
      // and Undo brings it back.
      clock.now += 1;
      await a.hs.delete('rooms', 'den');
      expect(
        a.mirror.rows.keys,
        isEmpty,
        reason: 'the room hides its box and item',
      );
      await b.hs.put('items', 'i1', {'value': 41});
      clock.now += 1;
      await b.hs.syncWith(a.hs);
      for (final d in [a, b]) {
        d.check();
      }
      expect(b.mirror.rows.keys, isEmpty);
      await b.hs.restore('rooms', 'den');
      clock.now += 1;
      await b.hs.syncWith(a.hs);
      expect(a.mirror.rows['items/i1']?['value'], 41);
      expect(a.mirror.snapshot, b.mirror.snapshot);

      // The relay path: sealed ops from one device's changes into another's ingest.
      clock.now += 1;
      final c = await device(3, clock);
      final up = [...a.sent, ...b.sent].map((s) => s.sealed).toList();
      final got = await c.hs.ingest(up);
      expect(got.rejected, isEmpty);
      // An op may wait for a parent later in the same batch; none waits at the end.
      expect(c.hs.status().pending, 0);
      c.check();
      expect(c.mirror.snapshot, a.mirror.snapshot);
      expect(a.hs.devices().map((d) => d.label).toSet(), {
        'device 1',
        'device 2',
      });
    },
  );

  test(
    'bad input surfaces as a HearthSyncException and changes nothing',
    () async {
      final clock = TestClock();
      final a = await device(1, clock);
      await expectLater(
        a.hs.put('nope', 'x', {'name': 'y'}),
        throwsA(
          isA<HearthSyncException>().having(
            (e) => e.code,
            'code',
            'undeclared',
          ),
        ),
      );
      await expectLater(
        a.hs.put('items', 'x', {'value': 'not an int'}),
        throwsA(
          isA<HearthSyncException>().having((e) => e.code, 'code', 'rejected'),
        ),
      );
      // The queue survives a failed call.
      await a.hs.put('rooms', 'den', {'name': 'Den'});
      a.check();
    },
  );

  test(
    'a refused signature fails that call and leaves the device usable',
    () async {
      final clock = TestClock();
      final bad = _FirstSignatureBad(
        SoftwareSigner.fromSeed(List.filled(32, 5)),
      );
      final hs = await HearthSync.open(
        app: app,
        schema: schema(),
        persist: MemoryPersist(),
        signer: bad,
        seed: seed,
        clock: clock.call,
      );
      bad.armed = true;
      await expectLater(
        hs.put('rooms', 'den', {'name': 'Den'}),
        throwsA(
          isA<HearthSyncException>().having(
            (e) => e.code,
            'code',
            'bad_signature',
          ),
        ),
      );
      await hs.put('rooms', 'den', {'name': 'Den'});
      expect(hs.view().rows.single.fields, {'name': 'Den'});
    },
  );
}

/// Returns 64 zero bytes once when armed, then signs correctly.
class _FirstSignatureBad implements Signer {
  _FirstSignatureBad(this.inner);
  final Signer inner;
  bool armed = false;

  @override
  Future<Uint8List> publicKey() => inner.publicKey();

  @override
  Future<Uint8List> sign(Uint8List message) async {
    if (armed) {
      armed = false;
      return Uint8List(64);
    }
    return inner.sign(message);
  }

  @override
  Future<void> destroy() => inner.destroy();
}
