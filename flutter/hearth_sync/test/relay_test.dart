// The relay client's api from Dart (ADR 0011, "The relay client"), against a
// stand-in relay with one log per uploader. The kernel's own tests
// (kernel/tests/relay_client.rs) cover the rules in depth.
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:hearth_sync/hearth_sync.dart';

import 'support.dart';

class Relay {
  Relay([this.generation = 1]);

  /// The channel's generation, as a real relay names it in enroll and pull.
  final int generation;
  final logs = <String, List<Uint8List>>{};
}

Future<void> push(HearthSync hs, Relay relay) async {
  final out = await hs.relayOutbox();
  if (out.isEmpty) return;
  final me = hex((hs.devices().firstWhere((d) => d.me)).device);
  final log = relay.logs.putIfAbsent(me, () => []);
  final first = hs.relayState().nextSeq;
  expect(first, log.length + 1);
  log.addAll([for (final o in out) o.sealed]);
  await hs.relayUploaded([for (final o in out) o.id], first);
}

Future<void> pull(HearthSync hs, Relay relay) async {
  if (hs.relayState().generation != relay.generation) {
    // A new channel: start over from the reset positions.
    await hs.relayGeneration(relay.generation);
  }
  final cursors = {
    for (final c in hs.relayState().cursors) hex(c.device): c.seq,
  };
  final envs = <Uint8List>[];
  final next = <RelayCursor>[];
  relay.logs.forEach((uploader, log) {
    envs.addAll(log.skip(cursors[uploader] ?? 0));
    next.add(RelayCursor(unhex(uploader), log.length));
  });
  await hs.ingest(envs);
  await hs.relayPulled(next);
}

void main() {
  setUpAll(initBridge);

  test(
    'a LAN-learned op is forwarded, and a Forget record waits for its op',
    () async {
      final clock = TestClock();
      final relay = Relay();
      final a = await device(1, clock);
      final b = await device(2, clock);
      final c = await device(3, clock);
      await push(a.hs, relay);
      await push(c.hs, relay);
      // B never uses the relay; A learns B's edit over the LAN and forwards it.
      clock.now += 1;
      await b.hs.put('rooms', 'lan', {'name': 'Hall'});
      await a.hs.syncWith(b.hs);
      await push(a.hs, relay);
      expect(await a.hs.relayOutbox(), isEmpty);
      await pull(c.hs, relay);
      expect(c.hs.view().rows.any((r) => r.row == 'lan'), isTrue);

      final e = a.hs.relayEnrollment(
        b.hs.devices().firstWhere((d) => d.me).device,
      );
      expect(e.label, 'device 2');
      expect(e.auth.length, 64);

      await pull(a.hs, relay);
      clock.now += 1;
      await a.hs.forgetDevice(c.hs.devices().firstWhere((d) => d.me).device);
      expect(
        a.hs.relayForgets(),
        isEmpty,
        reason: 'the Forget op is not uploaded',
      );
      await push(a.hs, relay);
      final recs = a.hs.relayForgets();
      expect(recs, hasLength(1));
      expect(
        recs.single.cutSeq,
        1,
        reason: "C's log as far as A had pulled it",
      );
      await a.hs.relayForgetPosted(recs.single.forget);
      expect(a.hs.relayForgets(), isEmpty);
    },
  );

  test('a new channel generation starts the relay client over', () async {
    final clock = TestClock();
    var relay = Relay();
    final a = await device(1, clock);
    await pull(a.hs, relay);
    expect(a.hs.relayState().generation, 1);
    clock.now += 1;
    await a.hs.put('rooms', 'r1', {'name': 'Kitchen'});
    await push(a.hs, relay);
    expect(a.hs.relayState().nextSeq, greaterThan(1));
    expect(await a.hs.relayOutbox(), isEmpty);

    // The channel expired and was made again: A starts over by itself.
    relay = Relay(2);
    await pull(a.hs, relay);
    final state = a.hs.relayState();
    expect(state.generation, 2);
    expect(state.nextSeq, 1);
    expect(state.cursors, isEmpty);
    expect(
      await a.hs.relayOutbox(),
      isNotEmpty,
      reason: 'everything goes up again',
    );
    await push(a.hs, relay);
    final c = await device(3, clock);
    await push(c.hs, relay);
    await pull(c.hs, relay);
    expect(c.hs.view().rows.any((r) => r.row == 'r1'), isTrue);
  });
}
