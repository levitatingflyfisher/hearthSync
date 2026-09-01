// Shared by the host-side tests. They load the Linux build of the bridge:
//   (cd rust && CARGO_TARGET_DIR=$PWD/target cargo build --release)
// or set HEARTH_SYNC_LIB to the .so.
import 'dart:io';

import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hearth_sync/hearth_sync.dart';

const app = 'lullaby';
const t0 = 1727000000000;

/// The kernel tests' household seed: bytes 0..63.
final seed = Uint8List.fromList(List.generate(64, (i) => i));

Future<void> initBridge() => HearthSync.init(
  library: ExternalLibrary.open(
    Platform.environment['HEARTH_SYNC_LIB'] ??
        'rust/target/release/libhearth_sync_bridge.so',
  ),
);

SyncSchema schema({Duration horizon = const Duration(seconds: 20)}) =>
    SyncSchema([
      const SyncTable('rooms', [SyncField('name', SyncType.text)]),
      const SyncTable(
        'boxes',
        [SyncField('name', SyncType.text), SyncField('room', SyncType.text)],
        containerField: 'room',
        containerTable: 'rooms',
      ),
      const SyncTable(
        'items',
        [
          SyncField('name', SyncType.text),
          SyncField('box', SyncType.text),
          SyncField('value', SyncType.int),
        ],
        containerField: 'box',
        containerTable: 'boxes',
      ),
      const SyncSet('tags', SyncType.text),
      const SyncStream('log', SyncType.any),
    ], horizon: horizon);

/// A settable clock.
class TestClock {
  int now = t0;
  DateTime call() => DateTime.fromMillisecondsSinceEpoch(now);
}

/// The app's tables, fed only by the changes each call stores.
class Mirror {
  final rows = <String, Map<String, Object?>>{};
  final sets = <String>{};
  final streams = <String, Object?>{};

  void apply(Changes c) {
    if (c.replaceView) {
      rows.clear();
      sets.clear();
      streams.clear();
    }
    for (final r in c.rows) {
      final k = '${r.table}/${r.row}';
      if (r.visible) {
        rows[k] = r.fields;
      } else {
        rows.remove(k);
      }
    }
    for (final s in c.sets) {
      final k = '${s.set}/${s.element}';
      s.present ? sets.add(k) : sets.remove(k);
    }
    for (final s in c.streams) {
      final k = '${s.stream}/${hex(s.id)}';
      if (s.present) {
        streams[k] = s.record;
      } else {
        streams.remove(k);
      }
    }
  }

  /// Built from a fresh view: what the mirror must always equal.
  static Mirror of(HearthSync hs) {
    final v = hs.view();
    return Mirror()
      ..apply(Changes(rows: v.rows, sets: v.sets, streams: v.streams));
  }

  Map<String, Object?> get snapshot => {
    'rows': rows,
    'sets': sets,
    'streams': streams,
  };
}

/// One device: a HearthSync, its record store, its key and the app's tables.
class Device {
  Device(this.n, this.persist, this.clock, {SyncSchema? schema})
    : signer = CountingSigner(SoftwareSigner.fromSeed(List.filled(32, n))),
      schema = schema ?? support_schema;

  final int n;
  final Persist persist;
  final TestClock clock;
  final SyncSchema schema;
  final CountingSigner signer;
  late HearthSync hs;
  final mirror = Mirror();

  /// Every sealed op this device handed out for the relay.
  final sent = <SealedOp>[];

  /// Every stored call's changes, in order.
  final log = <Changes>[];

  Future<Device> open() async {
    hs = await HearthSync.open(
      app: app,
      schema: schema,
      persist: persist,
      signer: signer,
      seed: seed,
      label: 'device $n',
      clock: clock.call,
    );
    return this;
  }

  /// Keep the mirror and the relay list up to date from every stored call.
  void watch() {
    hs.changes.listen((c) {
      log.add(c);
      mirror.apply(c);
      sent.addAll(c.outgoing);
    });
  }

  void check() => expect(
    Mirror.of(hs).snapshot,
    mirror.snapshot,
    reason: 'device $n: the tables the changes built differ from the view',
  );
}

// ignore: non_constant_identifier_names
final support_schema = schema();

/// Two devices, enrolled, watching.
Future<Device> device(
  int n,
  TestClock clock, {
  Persist? persist,
  SyncSchema? schema,
}) async {
  final d = Device(n, persist ?? MemoryPersist(), clock, schema: schema);
  await d.open();
  for (final c in d.hs.opened) {
    d.mirror.apply(c);
    d.sent.addAll(c.outgoing);
  }
  d.watch();
  return d;
}

/// Counts signatures, as the platform key store would be asked for them.
class CountingSigner implements Signer {
  CountingSigner(this.inner);
  final Signer inner;
  int signed = 0;
  bool destroyed = false;

  @override
  Future<Uint8List> publicKey() => inner.publicKey();

  @override
  Future<Uint8List> sign(Uint8List message) {
    signed++;
    return inner.sign(message);
  }

  @override
  Future<void> destroy() {
    destroyed = true;
    return inner.destroy();
  }
}
