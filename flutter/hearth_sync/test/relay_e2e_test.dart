// End to end over real HTTP: HearthSync instances syncing only through the real
// relay binaries, the Rust one and the Go one, started here on localhost. Each
// scenario runs against both. Build them first (see the package README):
//   cargo build --release -p hearth_sync_relay        -> target/release/hearth-relay
//   (cd go-relay && go build -o .cache/hearth-relay-go ./cmd/hearth-relay-go)
// or point HEARTH_RUST_RELAY / HEARTH_GO_RELAY at them. A missing binary fails
// the test rather than skipping it.
//
// The relays run on the wall clock (they have no fake one), so these devices do
// too. A channel's expiry is reached by restarting a relay with --idle-days 0:
// its start-up sweep then expires every channel.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:hearth_sync/hearth_sync.dart';
import 'package:hearth_sync/src/relay_client.dart' show RelayWire;
import 'package:http/http.dart' as http;

import 'support.dart';

/// The kernel's clock is the wall clock: the relay checks every `ts` against
/// its own.
class WallClock extends TestClock {
  @override
  int get now => DateTime.now().millisecondsSinceEpoch;

  @override
  set now(int _) {}
}

final repo = Directory.current.parent.parent.path;
final relays = {
  'rust':
      Platform.environment['HEARTH_RUST_RELAY'] ??
      '$repo/target/release/hearth-relay',
  'go':
      Platform.environment['HEARTH_GO_RELAY'] ??
      '$repo/go-relay/.cache/hearth-relay-go',
};

/// A relay binary running on localhost, over a data directory that survives
/// its restarts.
class RelayProcess {
  RelayProcess(this.binary, this.dir);

  final String binary;
  final Directory dir;
  late Process _p;
  int port = 0;
  final lines = <String>[];
  Completer<void>? _sweep;

  Uri get uri => Uri.parse('http://127.0.0.1:$port/');

  Future<void> start({int? idleDays}) async {
    _sweep = Completer();
    final started = Completer<void>();
    _p = await Process.start(binary, [
      '--data',
      dir.path,
      '--listen',
      '127.0.0.1:$port',
      // The tests read far more often than a real client may.
      '--max-reader-nonces',
      '100000',
      if (idleDays != null) ...['--idle-days', '$idleDays'],
    ]);
    // Drain both pipes all the time, or the relay blocks on a full one.
    _p.stdout.drain<void>();
    _p.stderr.transform(utf8.decoder).transform(const LineSplitter()).listen((
      l,
    ) {
      lines.add(l);
      final Object? j;
      try {
        j = jsonDecode(l);
      } on FormatException {
        return;
      }
      if (j is! Map) return;
      if (j['event'] == 'start' && !started.isCompleted) {
        port = int.parse((j['listen'] as String).split(':').last);
        started.complete();
      }
      if (j['event'] == 'sweep' && !_sweep!.isCompleted) _sweep!.complete();
    });
    await started.future.timeout(
      const Duration(seconds: 20),
      onTimeout: () => throw StateError('$binary did not start: $lines'),
    );
  }

  /// Wait for the start-up sweep (it runs beside the first requests).
  Future<void> swept() => _sweep!.future.timeout(const Duration(seconds: 20));

  /// Stop it and start it again over the same store, on the same port: its
  /// memory (nonces, buckets) goes and its epoch moves on.
  Future<void> restart({int? idleDays}) async {
    await stop();
    await start(idleDays: idleDays);
  }

  Future<void> stop() async {
    _p.kill(ProcessSignal.sigterm);
    await _p.exitCode.timeout(
      const Duration(seconds: 20),
      onTimeout: () {
        _p.kill(ProcessSignal.sigkill);
        return _p.exitCode;
      },
    );
  }
}

Future<RelayProcess> launch(String side, String name) async {
  final bin = relays[side]!;
  expect(
    File(bin).existsSync(),
    isTrue,
    reason: 'build the $side relay first: $bin',
  );
  final dir = Directory('$repo/target/relay-dart-e2e/$side/$name');
  if (dir.existsSync()) dir.deleteSync(recursive: true);
  dir.createSync(recursive: true);
  final r = RelayProcess(bin, dir);
  await r.start();
  await r.swept();
  // Real HTTP, not flutter_test's fake: the first thing checked.
  final health = await http.get(r.uri.resolve('healthz'));
  expect((health.statusCode, health.body), (200, 'ok'));
  return r;
}

/// A device with its loop and its own HTTP client to [relay].
class Peer {
  Peer(this.d, RelayProcess relay) : client = RelayClient(relay.uri) {
    loop = SyncLoop(d.hs, client);
  }

  final Device d;
  final RelayClient client;
  late final SyncLoop loop;

  Future<RelayRoundResult> sync() async {
    final s = await loop.syncNow();
    expect(s.ok, isTrue, reason: 'device ${d.n}: ${s.error}');
    return s.result!;
  }

  Set<String> get rows => {for (final r in d.hs.view().rows) r.row};
}

Future<Peer> join(int n, RelayProcess relay) async {
  final p = Peer(await device(n, WallClock()), relay);
  await p.sync();
  return p;
}

String viewOf(Peer p) => p.d.hs
    .view()
    .rows
    .map((r) => '${r.table}/${r.row}=${r.fields}')
    .toList()
    .join(';');

void main() {
  setUpAll(() async {
    HttpOverrides.global = null;
    await initBridge();
  });

  for (final side in relays.keys) {
    test(
      '$side: two devices converge, across a relay restart and a channel expiry',
      timeout: const Timeout(Duration(minutes: 3)),
      () async {
        final relay = await launch(side, 'converge');
        try {
          final a = await join(1, relay);
          final b = await join(2, relay);
          await a.d.hs.put('rooms', 'r1', {'name': 'Kitchen'});
          await b.d.hs.put('rooms', 'r2', {'name': 'Den'});
          await a.sync();
          await b.sync();
          await a.sync();
          expect(viewOf(a), viewOf(b));
          expect(a.rows, {'r1', 'r2'});
          expect(a.client.epoch, 1, reason: 'learned on the first read');

          // Writes go up by themselves, debounced, without a read.
          final auto = SyncLoop(
            a.d.hs,
            a.client,
            debounce: const Duration(milliseconds: 100),
            interval: const Duration(hours: 1),
          );
          auto.start();
          await auto.status.first; // the start-up round
          await a.d.hs.put('rooms', 'r0', {'name': 'Porch'});
          final pushed = await auto.status.first;
          expect(pushed.ok, isTrue, reason: '${pushed.error}');
          expect(pushed.result!.uploaded, 1);
          expect(pushed.result!.pulled, 0, reason: 'an upload-only round');
          await auto.stop();
          await b.sync();
          expect(b.rows, contains('r0'));

          // An append the relay stored whose answer was lost (no relayUploaded).
          // The next round pulls the device's own entries back (they leave the
          // outbox) and must continue its log after them, not at the old seq.
          await a.d.hs.put('rooms', 'lost', {'name': 'Cellar'});
          final out = await a.d.hs.relayOutbox();
          final channel = RelayWire.channel(
            app,
            a.d.hs.relayEnrollment(await a.d.signer.publicKey()).household,
          );
          await a.client.append(
            channel,
            a.d.signer,
            a.d.hs.relayState().nextSeq,
            [for (final o in out) o.sealed],
          );
          await a.d.hs.put('rooms', 'after', {'name': 'Pantry'});
          await a.sync();
          await b.sync();
          expect(b.rows, containsAll(['lost', 'after']));

          // The relay restarts: its epoch moves on, and both devices, still
          // holding the old one, retry once and carry on.
          await relay.restart();
          await a.d.hs.put('rooms', 'r3', {'name': 'Hall'});
          await a.sync();
          await b.sync();
          expect(a.client.epoch, 2);
          expect(
            b.client.epochRetries,
            2,
            reason: 'learned on the first read, then refused once after the restart',
          );
          expect(b.rows, contains('r3'));

          // The household goes quiet and its channel expires (a restart with
          // --idle-days 0 expires everything at the start-up sweep). B edits
          // offline meanwhile.
          await relay.restart(idleDays: 0);
          await relay.swept();
          final before = a.d.hs.relayState().generation;
          await b.d.hs.put('rooms', 'b-offline', {'name': 'Shed'});
          final ra = await a.sync();
          expect(ra.enrolled, isTrue, reason: 'A found itself unknown');
          expect(ra.newGeneration, isTrue);
          expect(a.d.hs.relayState().generation, greaterThan(before));
          final rb = await b.sync();
          expect(rb.newGeneration, isTrue);
          await a.sync();
          expect(viewOf(a), viewOf(b));
          expect(a.rows, containsAll(['r0', 'r1', 'r2', 'r3', 'b-offline']));
          // A device that never knew the old channel gets everything.
          final c = await join(3, relay);
          await c.sync();
          expect(viewOf(c), viewOf(a));
        } finally {
          await relay.stop();
        }
      },
    );

    test(
      '$side: a device forgotten through the relay wipes; one that forgets itself hands over first',
      timeout: const Timeout(Duration(minutes: 3)),
      () async {
        final relay = await launch(side, 'forget');
        try {
          final a = await join(1, relay);
          final b = await join(2, relay);
          final c = await join(3, relay);
          await b.d.hs.put('rooms', 'b1', {'name': 'Loft'});
          await b.sync();
          await a.sync();
          await c.sync();

          // A forgets B; one round uploads the Forget op, then posts its record.
          final bid = await b.d.signer.publicKey();
          await a.d.hs.forgetDevice(bid);
          expect((await a.sync()).forgetsPosted, 1);
          await a.d.hs.put('rooms', 'secret', {'name': 'Safe'});
          await a.sync();
          final rb = await b.sync();
          expect(rb.wiped, isTrue);
          expect(b.d.hs.isWiped, isTrue);
          expect(b.d.signer.destroyed, isTrue, reason: 'the key is gone');
          expect(b.rows, isNot(contains('secret')));
          // A stolen copy of B's key can no longer write to the relay.
          final stolen = SoftwareSigner.fromSeed(List.filled(32, 2));
          final channel = RelayWire.channel(
            app,
            a.d.hs.relayEnrollment(bid).household,
          );
          await expectLater(
            b.client.append(channel, stolen, b.d.hs.relayState().nextSeq, [
              b.d.sent.last.sealed,
            ]),
            throwsA(
              isA<RelayException>().having((e) => e.code, 'code', 'forgotten'),
            ),
          );

          // C writes, then forgets itself: the edit, the Forget op and the
          // record go up in one round, and then its key is destroyed.
          await c.d.hs.put('rooms', 'c-last', {'name': 'Attic'});
          final wiped = await c.loop.forgetSelf();
          expect(wiped.wiped, isTrue);
          expect(c.d.signer.destroyed, isTrue);
          expect(
            c.d.hs.relayForgets(),
            isEmpty,
            reason: 'the record was posted',
          );
          await a.sync();
          expect(a.rows, contains('c-last'), reason: "C's last edit arrived");
          expect(
            a.d.hs
                .devices()
                .where((d) => d.label == 'device 3')
                .single
                .forgotten,
            isTrue,
          );
          await expectLater(
            c.client.append(
              channel,
              SoftwareSigner.fromSeed(List.filled(32, 3)),
              c.d.hs.relayState().nextSeq,
              [c.d.sent.last.sealed],
            ),
            throwsA(
              isA<RelayException>().having((e) => e.code, 'code', 'forgotten'),
            ),
          );
        } finally {
          await relay.stop();
        }
      },
    );
  }
}
