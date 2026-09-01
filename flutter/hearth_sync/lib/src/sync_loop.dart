// One relay sync round, and a scheduler that runs rounds (ADR 0012, "The relay
// round"). The round is what relay/tests/e2e.rs does for its devices, through
// the Dart API: enrol if the relay does not know this device, pull every log
// from the kernel's cursors (adopting a snapshot where the relay pruned past
// them), ingest, upload the outbox, post the Forget records the kernel hands
// out, and upload this device's snapshot when its base changed.
import 'dart:async';
import 'dart:math';
import 'dart:typed_data';

import 'changes.dart';
import 'hearth_sync.dart';
import 'relay.dart';
import 'relay_client.dart';
import 'signer.dart';
import 'values.dart';

/// What one round did.
class RelayRoundResult {
  /// Envelopes pulled and handed to the kernel.
  int pulled = 0;

  /// Ops uploaded.
  int uploaded = 0;

  /// Forget records posted.
  int forgetsPosted = 0;

  /// This device (re-)enrolled at the relay.
  bool enrolled = false;

  /// The relay named a new channel generation, and the positions started over.
  bool newGeneration = false;

  /// A snapshot was adopted, because the relay had pruned past a cursor.
  bool adopted = false;

  /// This device's snapshot was uploaded.
  bool snapshotUploaded = false;

  /// This device was forgotten: it wiped during the round.
  bool wiped = false;

  @override
  String toString() =>
      'RelayRoundResult(pulled $pulled, uploaded $uploaded, forgets '
      '$forgetsPosted${enrolled ? ', enrolled' : ''}'
      '${newGeneration ? ', new generation' : ''}${adopted ? ', adopted' : ''}'
      '${snapshotUploaded ? ', snapshot' : ''}${wiped ? ', wiped' : ''})';
}

/// The relay listed pruned entries past a cursor, and no snapshot it holds could
/// be adopted.
class SnapshotNeededException implements Exception {
  @override
  String toString() => 'SnapshotNeededException';
}

/// Run one round for [hs] against [relay], signing with [signer] (the device
/// key [hs] was opened with) on [channel]. With [pull] false it only uploads
/// (after a local write: uploads use no read budget), unless the relay's answers
/// say the positions are stale, when it pulls first anyway.
///
/// Ingest runs in batches of [ingestBatch] envelopes with a yield between them,
/// so a long pull never holds the UI thread for one long call (on the web every
/// bridge call is synchronous).
Future<RelayRoundResult> runRelayRound(
  HearthSync hs,
  RelayClient relay,
  Signer signer,
  Uint8List channel, {
  bool pull = true,
  int ingestBatch = 32,
}) => _Round(hs, relay, signer, channel, ingestBatch).run(pull);

class _Round {
  _Round(this.hs, this.relay, this.signer, this.channel, this.ingestBatch);

  final HearthSync hs;
  final RelayClient relay;
  final Signer signer;
  final Uint8List channel;
  final int ingestBatch;
  final result = RelayRoundResult();
  RelayPullAnswer? _answer;
  late final Uint8List _me;

  Future<RelayRoundResult> run(bool pull) async {
    _me = await signer.publicKey();
    final wiped = hs.isWiped;
    if (!wiped) {
      if (hs.relayState().generation == 0) {
        pull = await _enrol() || pull;
      }
      if (pull) await _enrolled(_pull);
      if (hs.isWiped) {
        // Forgotten by another device: the ingest wiped this one and its key is
        // gone. Nothing more to send.
        result.wiped = true;
        return result;
      }
    }
    // A wiped device (holding its key for this one round) only hands over.
    await _enrolled(() => _push(wiped: wiped));
    if (!wiped && _answer != null) await _enrolled(_snapshot);
    return result;
  }

  /// Enrol this device; true if the relay named a new generation (the caller
  /// must pull before it uploads).
  Future<bool> _enrol() async {
    final g = await relay.enroll(hs.app, hs.relayEnrollment(_me));
    result.enrolled = true;
    final before = hs.relayState().generation;
    if (g == before) return false;
    await hs.relayGeneration(g);
    if (before != 0) result.newGeneration = true;
    return true;
  }

  /// Run [step]; if the relay no longer knows this device (its channel
  /// expired), enrol again once, pull, and run [step] again.
  Future<void> _enrolled(Future<void> Function() step) async {
    try {
      await step();
    } on RelayException catch (e) {
      if (e.code != 'not_enrolled' || result.enrolled || hs.isWiped) rethrow;
      await _enrol();
      await _pull();
      if (step != _pull) await step();
    }
  }

  Future<void> _pull() async {
    final cursors = <String, int>{
      for (final c in hs.relayState().cursors) hex(c.device): c.seq,
    };
    final envs = <Uint8List>[];
    var gap = false;
    RelayPullAnswer a;
    while (true) {
      a = await relay.pull(channel, signer, [
        for (final e in cursors.entries) RelayCursor(unhex(e.key), e.value),
      ]);
      if (a.generation != hs.relayState().generation) {
        // A new channel: every position was in logs that are gone. Start over
        // from the kernel's reset positions and pull from the beginning.
        if (hs.relayState().generation != 0) result.newGeneration = true;
        await hs.relayGeneration(a.generation);
        cursors.clear();
        envs.clear();
        gap = false;
        continue;
      }
      for (final page in a.logs) {
        final u = hex(page.uploader);
        var cur = cursors[u] ?? 0;
        if (page.first > cur + 1) {
          // Pruned past the cursor: a snapshot stands in for what is gone.
          gap = true;
          cur = page.first - 1;
        }
        for (final (seq, env) in page.entries) {
          cur = seq;
          envs.add(env);
        }
        cursors[u] = cur;
      }
      if (!a.more) break;
    }
    _answer = a;
    if (gap) {
      await _adopt(a, envs);
    } else {
      for (var i = 0; i < envs.length; i += ingestBatch) {
        await hs.ingest(envs.sublist(i, min(i + ingestBatch, envs.length)));
        if (hs.isWiped) break;
        // Let the event loop (and the UI) run between batches.
        await Future<void>.delayed(Duration.zero);
      }
    }
    result.pulled += envs.length;
    if (hs.isWiped) return;
    await hs.relayPulled([
      for (final e in cursors.entries) RelayCursor(unhex(e.key), e.value),
    ]);
  }

  Future<void> _adopt(RelayPullAnswer a, List<Uint8List> envs) async {
    // Other devices' snapshots first: this device's own is behind what it has.
    final order = [
      ...a.snapshots.where((s) => !sameBytes(s.device, _me)),
      ...a.snapshots.where((s) => sameBytes(s.device, _me)),
    ];
    for (final s in order) {
      final Uint8List snap;
      try {
        (snap, _) = await relay.fetchSnapshot(channel, signer, s.device);
      } on RelayException catch (e) {
        if (e.code == 'no_snapshot') continue;
        rethrow;
      }
      try {
        await hs.adoptSnapshot(snap, envs);
      } on HearthSyncException {
        continue;
      }
      result.adopted = true;
      return;
    }
    throw SnapshotNeededException();
  }

  Future<void> _push({required bool wiped}) async {
    var out = await hs.relayOutbox();
    while (out.isNotEmpty) {
      final batch = out.take(relay.maxBatch).toList();
      final first = hs.relayState().nextSeq;
      await relay.append(channel, signer, first, [
        for (final o in batch) o.sealed,
      ]);
      await hs.relayUploaded([for (final o in batch) o.id], first);
      result.uploaded += batch.length;
      out = out.skip(batch.length).toList();
    }
    // The kernel hands a record out only once its Forget op is on the relay.
    for (final f in hs.relayForgets()) {
      await relay.forget(channel, signer, f);
      await hs.relayForgetPosted(f.forget);
      result.forgetsPosted++;
    }
  }

  Future<void> _snapshot() async {
    final s = await hs.relaySnapshot();
    if (s == null) return;
    final ref = RelayWire.envelopeRef(s.sealed);
    final listed = _answer!.snapshots.where((l) => sameBytes(l.device, _me));
    if (listed.isNotEmpty &&
        sameBytes(listed.first.ref, ref) &&
        _sameCovers(listed.first.covers, s.covers)) {
      return;
    }
    await relay.snapshot(channel, signer, s);
    result.snapshotUploaded = true;
  }

  static bool _sameCovers(List<RelayCursor> a, List<RelayCursor> b) {
    String key(List<RelayCursor> l) =>
        ([for (final c in l) '${hex(c.device)}:${c.seq}']..sort()).join(',');
    return key(a) == key(b);
  }
}

/// The delay before retry number [failures] (from 1): full jitter over an
/// exponential ceiling, `random(0, min(max, base * 2^(failures-1)))`, never
/// under a second, so a household's devices that failed together (a relay
/// restart, a rate limit) do not come back together.
Duration backoffDelay(
  int failures,
  Random random, {
  Duration base = const Duration(seconds: 30),
  Duration max = const Duration(minutes: 30),
}) {
  final exp = failures <= 1 ? 1 : 1 << min(failures - 1, 20);
  final ceiling = min(max.inMilliseconds, base.inMilliseconds * exp);
  final ms = (random.nextDouble() * ceiling).round();
  return Duration(milliseconds: ms < 1000 ? 1000 : ms);
}

/// How the last round went.
class SyncStatus {
  /// A status.
  const SyncStatus(this.at, {this.result, this.error, this.retryIn});

  /// When the round ended.
  final DateTime at;

  /// What it did, if it finished.
  final RelayRoundResult? result;

  /// Why it stopped, if it did not.
  final Object? error;

  /// When the loop tries again, after an error worth retrying.
  final Duration? retryIn;

  /// It finished.
  bool get ok => error == null;

  @override
  String toString() =>
      ok ? 'SyncStatus(ok, $result)' : 'SyncStatus($error, retry in $retryIn)';
}

/// Runs relay rounds for one [HearthSync]: a full round every [interval] (minutes,
/// not seconds: each pull spends one of the device's reads, 16 per five minutes
/// by default), an upload-only round [debounce] after local writes, and retries
/// with jittered exponential backoff after a failure worth retrying. One round
/// runs at a time; a request during a round runs once after it.
///
/// Call [syncNow] when the app comes to the foreground, and [stop] before
/// [HearthSync.close].
class SyncLoop {
  /// A loop over [hs] and [relay].
  SyncLoop(
    this.hs,
    this.relay, {
    this.interval = const Duration(minutes: 5),
    this.debounce = const Duration(seconds: 2),
    this.backoffBase = const Duration(seconds: 30),
    this.backoffMax = const Duration(minutes: 30),
    Random? random,
    DateTime Function() clock = DateTime.now,
  }) : _random = random ?? Random(),
       _clock = clock;

  /// The replica.
  final HearthSync hs;

  /// The relay.
  final RelayClient relay;

  /// Between full rounds.
  final Duration interval;

  /// From a local write to its upload.
  final Duration debounce;

  /// The backoff's first ceiling.
  final Duration backoffBase;

  /// The backoff's largest ceiling.
  final Duration backoffMax;

  final Random _random;
  final DateTime Function() _clock;
  final _status = StreamController<SyncStatus>.broadcast();
  StreamSubscription<Changes>? _sub;
  Timer? _timer, _debounceTimer;
  Future<SyncStatus>? _running;
  bool _wantPull = false, _wantPush = false, _stopped = true;
  int _failures = 0;

  /// Every round's outcome.
  Stream<SyncStatus> get status => _status.stream;

  /// The last round's outcome.
  SyncStatus? last;

  /// Start: a full round now, then on the timer and after writes.
  void start() {
    if (!_stopped) return;
    _stopped = false;
    _sub = hs.changes.listen((c) {
      if (c.outgoing.isEmpty || _stopped) return;
      _debounceTimer?.cancel();
      _debounceTimer = Timer(debounce, () => syncNow(pull: false));
    });
    _schedule(Duration.zero);
  }

  void _schedule(Duration d) {
    _timer?.cancel();
    if (_stopped) return;
    _timer = Timer(d, () => syncNow());
  }

  /// Run a round now (a full one unless [pull] is false), or once after the
  /// running one. Completes with that round's status; never throws.
  Future<SyncStatus> syncNow({bool pull = true}) {
    if (pull) {
      _wantPull = true;
    } else {
      _wantPush = true;
    }
    return _running ??= _drain().whenComplete(() {
      _running = null;
      // A request that came in as the last round ended.
      if ((_wantPull || _wantPush) && !_stopped) syncNow(pull: _wantPull);
    });
  }

  Future<SyncStatus> _drain() async {
    late SyncStatus s;
    while (_wantPull || _wantPush) {
      final pull = _wantPull;
      _wantPull = _wantPush = false;
      try {
        final r = await hs.syncWithRelay(relay, pull: pull);
        _failures = 0;
        s = SyncStatus(_clock(), result: r);
      } catch (e) {
        final retry = switch (e) {
          RelayException(:final retryable) => retryable,
          SnapshotNeededException() => true,
          _ => false,
        };
        Duration? wait;
        if (retry) {
          _failures++;
          wait = backoffDelay(
            _failures,
            _random,
            base: backoffBase,
            max: backoffMax,
          );
        }
        s = SyncStatus(_clock(), error: e, retryIn: wait);
        last = s;
        _status.add(s);
        _wantPull = _wantPush = false;
        _schedule(wait ?? interval);
        return s;
      }
      last = s;
      _status.add(s);
    }
    if (hs.isWiped) {
      _stopped = true;
      _timer?.cancel();
    } else {
      _schedule(interval);
    }
    return s;
  }

  /// "Forget this device" through the relay: the Forget op and the ops this
  /// device still owes go up, then its record, and then its key is destroyed
  /// (see [HearthSync.forgetSelf]). The loop stops.
  Future<Changes> forgetSelf() async {
    await stop();
    return hs.forgetSelf(relay: relay);
  }

  /// Stop the timers and wait for a running round.
  Future<void> stop() async {
    _stopped = true;
    _timer?.cancel();
    _debounceTimer?.cancel();
    await _sub?.cancel();
    _sub = null;
    await _running;
  }
}
