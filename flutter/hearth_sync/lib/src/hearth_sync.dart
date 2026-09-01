import 'dart:async';
import 'dart:typed_data';

import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show ExternalLibrary;

import 'changes.dart';
import 'persist.dart';
import 'relay.dart';
import 'relay_client.dart';
import 'rust/api/kernel.dart' as k;
import 'rust/frb_generated.dart';
import 'schema.dart';
import 'signer.dart';
import 'sync_loop.dart';
import 'values.dart';

/// The clock the kernel reads (Unix time); inject a fixed one in tests.
typedef Clock = DateTime Function();

/// A kernel call failed. Nothing changed; see [code].
class HearthSyncException implements Exception {
  /// An error.
  const HearthSyncException(this.code, [this.detail, this.latest]);

  /// `bad_argument`, `schema`, `undeclared`, `persist`, `bad_message`, `no_keys`,
  /// `clock_behind`, `rejected`, `bad_snapshot`, `snapshot_unavailable`,
  /// `bad_signature`, `nothing_to_finish`, `awaiting_signature`, or
  /// `wrong_device` (the records belong to another device key).
  final String code;

  /// The kernel's detail (a reject code, a schema error, ...).
  final String? detail;

  /// For `clock_behind`: the newest clock in the log (Unix millis). "Your clock
  /// looks wrong."
  final int? latest;

  static HearthSyncException of(k.ApiError e) => switch (e) {
    k.ApiError_BadArgument(:final field0) => HearthSyncException(
      'bad_argument',
      field0,
    ),
    k.ApiError_Schema(:final field0) => HearthSyncException('schema', field0),
    k.ApiError_Undeclared() => const HearthSyncException('undeclared'),
    k.ApiError_Persist(:final field0) => HearthSyncException('persist', field0),
    k.ApiError_BadMessage() => const HearthSyncException('bad_message'),
    k.ApiError_NoKeys() => const HearthSyncException('no_keys'),
    k.ApiError_ClockBehind(:final latest) => HearthSyncException(
      'clock_behind',
      null,
      latest.toInt(),
    ),
    k.ApiError_Rejected(:final field0) => HearthSyncException(
      'rejected',
      field0,
    ),
    k.ApiError_BadSnapshot(:final field0) => HearthSyncException(
      'bad_snapshot',
      field0,
    ),
    k.ApiError_SnapshotUnavailable() => const HearthSyncException(
      'snapshot_unavailable',
    ),
    k.ApiError_BadSignature() => const HearthSyncException('bad_signature'),
    k.ApiError_NothingToFinish() => const HearthSyncException(
      'nothing_to_finish',
    ),
    k.ApiError_AwaitingSignature() => const HearthSyncException(
      'awaiting_signature',
    ),
  };

  @override
  String toString() =>
      'HearthSyncException($code${detail == null ? '' : ': $detail'})';
}

/// The stored records belong to a device that was forgotten and wiped. It has no
/// keys: hand its ops on with [HearthSync.wipedHandover], then clear the records.
class DeviceWipedException implements Exception {
  @override
  String toString() => 'DeviceWipedException: this device was forgotten';
}

/// A self-Forget through the relay wiped this device, but the relay round that
/// was to hand its ops and Forget record over did not get through. The wipe
/// happened; [changes] are its stored changes.
class RelayHandoverException implements Exception {
  /// A failed handover.
  const RelayHandoverException(this.changes, this.cause);

  /// What the self-Forget stored.
  final Changes changes;

  /// Why the round stopped.
  final Object cause;

  @override
  String toString() => 'RelayHandoverException($cause)';
}

/// Whose records these are, read without keys.
class StoredDevice {
  /// Stored-device info.
  const StoredDevice(this.device, this.wiped);

  /// The device's public key, if the records name one.
  final Uint8List? device;

  /// It was forgotten and wiped.
  final bool wiped;
}

/// A device enrolled in the household.
class HouseholdDevice {
  /// A device.
  const HouseholdDevice(
    this.device,
    this.label, {
    required this.forgotten,
    required this.me,
  });

  /// Its public key.
  final Uint8List device;

  /// Its label.
  final String label;

  /// Someone forgot it.
  final bool forgotten;

  /// It is this device.
  final bool me;
}

/// What [SyncPeer.request] returns.
class SyncRequest {
  /// A request.
  const SyncRequest(this.message, {required this.needsSnapshot});

  /// Send this to the provider.
  final Uint8List message;

  /// The provider holds an old checkpoint this device lacks: take its offer (a
  /// snapshot) before sending anything the other way.
  final bool needsSnapshot;
}

/// The other side of a sync: another [HearthSync] in-process, or a transport's
/// stand-in for a device on the LAN. Every message is sealed (ADR 0008).
abstract interface class SyncPeer {
  /// Its opening message.
  Future<Uint8List> hello();

  /// Its answer to our hello.
  Future<SyncRequest> request(Uint8List hello);

  /// What it has that our request lacks.
  Future<Uint8List> offer(Uint8List request);

  /// WipedPush: what it hands over if it is wiped.
  Future<Uint8List> handover(Uint8List request);

  /// Take our offer.
  Future<void> accept(Uint8List offer);

  /// Whether it was forgotten and wiped.
  Future<bool> wiped();
}

/// One app's replicated state on this device: the kernel, its records and the
/// device key. Every call is serialised: a call's signatures are gathered and its
/// records stored before the next call starts, and before its results (sealed
/// ops, messages) are returned (ADR 0010).
class HearthSync implements SyncPeer {
  HearthSync._(
    this._k,
    this.app,
    this.schema,
    this._persist,
    this._signer,
    this._clock,
  );

  final k.HearthKernel _k;

  /// The app domain.
  final String app;

  /// The schema this app version registers.
  final SyncSchema schema;
  final Persist _persist;
  final Signer _signer;
  final Clock _clock;
  Future<void> _tail = Future.value();

  /// A self-Forget through the relay keeps the key for its one round.
  bool _holdKey = false;
  Uint8List? _channel;
  final _changes = StreamController<Changes>.broadcast(sync: true);

  /// Load the bridge. Call once before anything else; tests pass the host build.
  static Future<void> init({ExternalLibrary? library}) async {
    if (RustLib.instance.initialized) return;
    await RustLib.init(externalLibrary: library);
  }

  static List<k.Record> _toKernel(List<StoredRecord> rs) => [
    for (final r in rs) k.Record(key: r.key, value: r.value),
  ];

  /// Whose records [persist] holds and whether that device was wiped, without
  /// keys: call before fetching the words.
  static Future<StoredDevice?> storedDevice(Persist persist) async {
    final info = k.storedInfo(records: _toKernel(await persist.readAll()));
    return info == null ? null : StoredDevice(info.device, info.wiped);
  }

  /// WipedPush after a restart: the sealed ops a wiped device still owes the
  /// household (its own ops and its Forgets' past), read from its records
  /// without keys, byte for byte as first sent. Post them to the relay, then
  /// [Persist.clear].
  static Future<List<SealedOp>> wipedHandover({
    required Persist persist,
    required SyncSchema schema,
    Clock clock = DateTime.now,
  }) async {
    try {
      final out = k.sealedHandover(
        records: _toKernel(await persist.readAll()),
        schema: schema.toKernel(),
        now: u64(clock().millisecondsSinceEpoch),
      );
      return [for (final s in out) SealedOp(s.id, s.sealed)];
    } on k.ApiError catch (e) {
      throw HearthSyncException.of(e);
    }
  }

  /// Open [app]'s replica from [persist] (empty on first launch), with the
  /// 64-byte household [seed] (the 12 words) and the device key in [signer]. A
  /// device not yet enrolled enrols itself as [label]. What that stores is in
  /// [opened]; paint first from [view].
  ///
  /// Throws [DeviceWipedException] if the records belong to a wiped device.
  static Future<HearthSync> open({
    required String app,
    required SyncSchema schema,
    required Persist persist,
    required Signer signer,
    required Uint8List seed,
    String label = 'this device',
    Clock clock = DateTime.now,
  }) async {
    final records = _toKernel(await persist.readAll());
    final info = k.storedInfo(records: records);
    if (info != null && info.wiped) {
      // The records say this key was forgotten. If it is still here (the app
      // stopped during the one relay round a self-Forget holds it for), it goes.
      await signer.destroy();
      throw DeviceWipedException();
    }
    final device = await signer.publicKey();
    if (info?.device case final d? when !sameBytes(d, device)) {
      throw const HearthSyncException(
        'wrong_device',
        'the records belong to another device key',
      );
    }
    final k.HearthKernel kernel;
    try {
      kernel = k.HearthKernel.open(
        args: k.OpenArgs(
          app: app,
          seed: seed,
          device: device,
          schema: schema.toKernel(),
          records: records,
          now: u64(clock().millisecondsSinceEpoch),
        ),
      );
    } on k.ApiError catch (e) {
      throw HearthSyncException.of(e);
    }
    final hs = HearthSync._(kernel, app, schema, persist, signer, clock);
    hs.opened.add(await hs._serial(() => hs._store(kernel.flush())));
    if (!kernel.status().enrolled && !kernel.status().wiped) {
      hs.opened.add(
        await hs._serial(
          () => hs._drive(() => kernel.enrollSelf(label: label, now: hs._now)),
        ),
      );
    }
    return hs;
  }

  /// Every completed call's changes, after its records are stored (also returned
  /// by the call itself).
  Stream<Changes> get changes => _changes.stream;

  /// What [open] itself stored, before anyone could listen to [changes]: load's
  /// repairs (with `replaceView` after a schema change) and, on a new device, its
  /// enrolment, whose sealed op the relay needs.
  final List<Changes> opened = [];

  BigInt get _now => u64(_clock().millisecondsSinceEpoch);

  Future<T> _serial<T>(Future<T> Function() f) {
    final run = _tail.then((_) => f());
    _tail = run.then((_) {}, onError: (_) {});
    return run;
  }

  /// Run a kernel call that may need signatures, then store its outcome.
  Future<Changes> _drive(k.Step Function() start) async {
    k.Step step = _call(start);
    while (true) {
      switch (step) {
        case k.Step_Done(:final field0):
          return _store(field0);
        case k.Step_Sign(:final signable):
          // A signer that throws, or a signature the kernel refuses, ends the
          // call: drop the kernel's waiting flow so the next call can start.
          try {
            final sig = await _signer.sign(signable);
            step = _call<k.Step>(() => _k.finish(signature: sig));
          } catch (_) {
            _k.abandon();
            rethrow;
          }
      }
    }
  }

  T _call<T>(T Function() f) {
    try {
      return f();
    } on k.ApiError catch (e) {
      throw HearthSyncException.of(e);
    }
  }

  Future<Changes> _store(k.Outcome o) async {
    final changes = Changes.of(o);
    await _persist.apply(
      RecordBatch(
        reset: o.resetRecords,
        records: [for (final r in o.records) StoredRecord(r.key, r.value)],
        changes: changes,
      ),
    );
    if (o.wiped && !_holdKey) await _signer.destroy();
    _changes.add(changes);
    return changes;
  }

  // ---------------------------------------------------------------- writing

  /// Set [fields] of [row] in [table] (creating it if new).
  Future<Changes> put(String table, String row, Map<String, Object?> fields) =>
      _write(
        k.Write.put(
          table: table,
          row: row,
          fields: [
            for (final e in fields.entries)
              k.Field(name: e.key, value: toKernel(e.value)),
          ],
        ),
      );

  /// Delete [row] (a concurrent edit elsewhere keeps it).
  Future<Changes> delete(String table, String row) =>
      _write(k.Write.delete(table: table, row: row));

  /// Undo every delete of [row].
  Future<Changes> restore(String table, String row) =>
      _write(k.Write.restore(table: table, row: row));

  /// Add [element] to [set].
  Future<Changes> setAdd(String set, Object? element) =>
      _write(k.Write.setAdd(set_: set, element: toKernel(element)));

  /// Remove [element] from [set] (a concurrent add elsewhere keeps it).
  Future<Changes> setRemove(String set, Object? element) =>
      _write(k.Write.setRemove(set_: set, element: toKernel(element)));

  /// Append [record] to [stream].
  Future<Changes> append(String stream, Object? record) =>
      _write(k.Write.append(stream: stream, record: toKernel(record)));

  Future<Changes> _write(k.Write w) =>
      _serial(() => _drive(() => _k.write(write: w, now: _now)));

  /// Enrol another device (pairing).
  Future<Changes> enrollDevice(Uint8List device, String label) => _serial(
    () =>
        _drive(() => _k.enrollDevice(device: device, label: label, now: _now)),
  );

  /// "Forget this device": its later ops stop counting everywhere.
  Future<Changes> forgetDevice(Uint8List device) =>
      _serial(() => _drive(() => _k.forgetDevice(device: device, now: _now)));

  /// Forget this device itself: it wipes at once, and its key is destroyed.
  ///
  /// With a [relay], the key is kept for exactly one upload round first: the
  /// relay only takes the Forget op, the ops this device still owes, and the
  /// Forget record (which freezes this device on the relay) signed by this
  /// device. The key is destroyed when that round ends, whether or not it got
  /// through; if it did not, [RelayHandoverException] carries the changes, and
  /// a LAN peer's outbox can still forward the ops.
  Future<Changes> forgetSelf({RelayClient? relay}) async {
    if (relay == null) {
      return _serial(() => _drive(() => _k.forgetSelf(now: _now)));
    }
    final channel = _relayChannel();
    _holdKey = true;
    final Changes changes;
    try {
      changes = await _serial(() => _drive(() => _k.forgetSelf(now: _now)));
    } catch (_) {
      _holdKey = false;
      rethrow;
    }
    try {
      await runRelayRound(this, relay, _signer, channel, pull: false);
    } catch (e) {
      throw RelayHandoverException(changes, e);
    } finally {
      _holdKey = false;
      await _signer.destroy();
    }
    return changes;
  }

  /// Write a checkpoint.
  Future<Changes> checkpoint() =>
      _serial(() => _drive(() => _k.checkpoint(now: _now)));

  /// Prune op bodies behind the newest checkpoint older than the horizon.
  Future<Changes> compact() =>
      _serial(() => _store(_call(() => _k.compact(now: _now))));

  // ---------------------------------------------------------------- relay path

  /// Take sealed ops from the relay, in any order, with duplicates.
  Future<Changes> ingest(List<Uint8List> sealed) =>
      _serial(() => _store(_call(() => _k.ingest(sealed: sealed, now: _now))));

  /// This device's base as a sealed snapshot for the relay, if it has pruned.
  Future<Uint8List?> snapshot() => _serial(() async => _k.snapshot());

  /// Adopt a snapshot from the relay with the sealed ops above it (may need
  /// several signatures: the rebase).
  Future<Changes> adoptSnapshot(Uint8List snapshot, List<Uint8List> ops) =>
      _serial(
        () => _drive(
          () => _k.adoptSnapshot(snapshot: snapshot, ops: ops, now: _now),
        ),
      );

  // ---------------------------------------------------------------- relay client
  //
  // The kernel keeps the relay client's positions in its records (ADR 0011, "The
  // relay client"). A round: pull every log from [relayState]'s cursors,
  // [ingest], [relayPulled]; upload [relayOutbox] in order from
  // [RelayState.nextSeq] and [relayUploaded] each acknowledged batch; then post
  // [relayForgets] and [relayForgetPosted] each. Every enroll and pull answer
  // names the channel's generation: when it differs from
  // [RelayState.generation], call [relayGeneration] and pull again from the
  // start before uploading. A read answered not_enrolled means the channel
  // expired: enrol again with [relayEnrollment] (its answer names the new
  // generation) and carry on.

  static List<RelayCursor> _cursors(List<k.RelayCursor> cs) => [
    for (final c in cs) RelayCursor(c.device, c.seq.toInt()),
  ];

  /// The own log's next seq and the cursors to pull from.
  RelayState relayState() {
    final s = _k.relayState();
    return RelayState(
      s.nextSeq.toInt(),
      _cursors(s.cursors),
      generation: s.generation.toInt(),
    );
  }

  /// The relay named its channel's generation. The first is recorded; a new
  /// one resets this device's relay positions and queues everything it holds
  /// for upload again (the channel was expired and made again).
  Future<Changes> relayGeneration(int generation) => _serial(
    () => _store(_call(() => _k.relayGeneration(generation: u64(generation)))),
  );

  /// Every op this device holds that the relay is not known to hold, sealed,
  /// parents first: its own writes and what it learned over the LAN.
  Future<List<SealedOp>> relayOutbox() => _serial(
    () async => [for (final s in _k.relayOutbox()) SealedOp(s.id, s.sealed)],
  );

  /// The relay acknowledged an append of these ops (in the order sent) from
  /// [firstSeq].
  Future<Changes> relayUploaded(List<Uint8List> ids, int firstSeq) => _serial(
    () => _store(
      _call(() => _k.relayUploaded(ids: ids, firstSeq: u64(firstSeq))),
    ),
  );

  /// Every log was pulled to [cursors] and ingested.
  Future<Changes> relayPulled(List<RelayCursor> cursors) => _serial(
    () => _store(
      _call(
        () => _k.relayPulled(
          cursors: [
            for (final c in cursors)
              k.RelayCursor(device: c.device, seq: u64(c.seq)),
          ],
        ),
      ),
    ),
  );

  /// This device's base as a sealed snapshot, with its covers, if it has pruned.
  Future<RelaySnapshot?> relaySnapshot() => _serial(() async {
    final s = _k.relaySnapshot();
    return s == null ? null : RelaySnapshot(s.sealed, _cursors(s.covers));
  });

  /// The relay's `enroll` fields for an enrolled device.
  RelayEnrollment relayEnrollment(Uint8List device) {
    final e = _call(() => _k.relayEnrollment(device: device));
    return RelayEnrollment(e.household, e.device, e.label, e.auth);
  }

  /// The Forget records owed to the relay, each only once its Forget op is on
  /// the relay.
  List<RelayForget> relayForgets() => [
    for (final f in _k.relayForgets())
      RelayForget(f.forget, f.target, f.cut, f.auth, f.cutSeq.toInt()),
  ];

  /// The relay recorded this Forget.
  Future<Changes> relayForgetPosted(Uint8List forget) =>
      _serial(() => _store(_call(() => _k.relayForgetPosted(forget: forget))));

  Uint8List _relayChannel() => _channel ??= RelayWire.channel(
    app,
    relayEnrollment(devices().firstWhere((d) => d.me).device).household,
  );

  /// One relay round (pull, ingest, upload, Forget records, snapshot); with
  /// [pull] false, upload only. [SyncLoop] schedules these. A wiped device has
  /// no key left, so it has nothing to do. Throws [RelayException] (see
  /// [RelayException.retryable]) or [SnapshotNeededException]; whatever the
  /// round stored before it stopped stays stored.
  Future<RelayRoundResult> syncWithRelay(
    RelayClient relay, {
    bool pull = true,
  }) async {
    if (isWiped) return RelayRoundResult()..wiped = true;
    return runRelayRound(this, relay, _signer, _relayChannel(), pull: pull);
  }

  // ---------------------------------------------------------------- LAN sync

  @override
  Future<Uint8List> hello() => _serial(() async => _k.hello(now: _now));

  @override
  Future<SyncRequest> request(Uint8List hello) => _serial(() async {
    final r = _call(() => _k.request(hello: hello));
    return SyncRequest(r.message, needsSnapshot: r.needsSnapshot);
  });

  @override
  Future<Uint8List> offer(Uint8List request) =>
      _serial(() async => _call(() => _k.offer(request: request)));

  @override
  Future<Uint8List> handover(Uint8List request) => _serial(
    () async => _call(() => _k.handover(request: request, now: _now)),
  );

  @override
  Future<void> accept(Uint8List offer) =>
      _serial(() => _drive(() => _k.accept(offer: offer, now: _now)));

  @override
  Future<bool> wiped() async => _k.status().wiped;

  /// A full two-way sync with [peer], in the kernel's order (ADR 0011): whoever
  /// lacks an old checkpoint takes a snapshot first; a wiped side only hands
  /// over; then ops move both ways.
  Future<void> syncWith(SyncPeer peer) async {
    var mine = await request(await peer.hello());
    if (mine.needsSnapshot && !await wiped()) {
      await accept(await peer.offer(mine.message));
    }
    var theirs = await peer.request(await hello());
    if (theirs.needsSnapshot && !await peer.wiped()) {
      await peer.accept(await offer(theirs.message));
    }
    theirs = await peer.request(await hello());
    mine = await request(await peer.hello());
    if (mine.needsSnapshot || theirs.needsSnapshot) {
      // Only a wiped side can still lack a checkpoint: it hands over its own ops.
      if (mine.needsSnapshot) await peer.accept(await handover(theirs.message));
      if (theirs.needsSnapshot) await accept(await peer.handover(mine.message));
      return;
    }
    await peer.accept(await offer(theirs.message));
    mine = await request(await peer.hello());
    await accept(await peer.offer(mine.message));
  }

  // ---------------------------------------------------------------- reading

  /// The whole view, for a first paint.
  ({List<RowUpdate> rows, List<SetUpdate> sets, List<StreamUpdate> streams})
  view() {
    final v = _k.viewAll();
    return (
      rows: [for (final r in v.rows) rowUpdate(r)],
      sets: [
        for (final s in v.sets) SetUpdate(s.set_, fromKernel(s.element), true),
      ],
      streams: [for (final s in v.streams) streamUpdate(s)],
    );
  }

  /// The review list.
  List<ReviewItem> review() => [for (final e in _k.review()) ReviewItem.of(e)];

  /// Remove an entry from the review list.
  Future<Changes> dismissReview(Uint8List key) =>
      _serial(() => _store(_k.dismissReview(key: key)));

  /// Every device ever enrolled.
  List<HouseholdDevice> devices() => [
    for (final d in _k.devices())
      HouseholdDevice(d.device, d.label, forgotten: d.forgotten, me: d.me),
  ];

  /// Forgotten and wiped: no more writes.
  bool get isWiped => _k.status().wiped;

  /// Enrolled in the household.
  bool get isEnrolled => _k.status().enrolled;

  /// Counts for a sync-health screen: pending, quarantined, held, rejected.
  ({int pending, int quarantined, int held, int rejected, bool schemaChanged})
  status() {
    final s = _k.status();
    return (
      pending: s.pending.toInt(),
      quarantined: s.quarantined.toInt(),
      held: s.held.toInt(),
      rejected: s.rejected.toInt(),
      schemaChanged: s.schemaChanged,
    );
  }

  /// Release the kernel. The records stay in [Persist].
  Future<void> close() async {
    await _tail;
    await _changes.close();
    _k.dispose();
  }
}
