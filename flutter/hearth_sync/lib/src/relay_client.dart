// The relay client's transport: `POST /v1/{channel}/{verb}` with dCBOR bodies, as
// docs/reference/relay-protocol.md states them. It signs with the device key,
// learns and signs the relay's epoch on reads, and turns every error answer into
// a RelayException. What to send and what the answers mean for the kernel is the
// sync round's business (sync_loop.dart).
import 'dart:async';
import 'dart:math';
import 'dart:typed_data';

import 'package:cryptography/dart.dart' show DartSha256;
import 'package:http/http.dart' as http;

import 'relay.dart';
import 'relay_cbor.dart';
import 'signer.dart';
import 'values.dart';

Uint8List _sha256(List<int> b) =>
    Uint8List.fromList(const DartSha256().hashSync(b).bytes);

/// A relay refused a request, or could not be reached.
class RelayException implements Exception {
  /// An error.
  const RelayException(this.status, this.code, {this.last, this.epoch});

  /// The HTTP status (0: no answer at all).
  final int status;

  /// The protocol's error code (`seq`, `epoch`, `not_enrolled`, `forgotten`,
  /// `rate_limited`, `quota`, ...), `network` when there was no answer, or
  /// `bad_answer` when the answer was not the protocol's.
  final String code;

  /// For `seq`: the uploader's last seq on the relay.
  final int? last;

  /// For `epoch`: the relay's current epoch.
  final int? epoch;

  /// Worth trying again later: rate limits, time-outs, server errors, no
  /// answer. Everything else needs something to change first.
  bool get retryable =>
      status == 0 || status == 408 || status == 429 || status >= 500;

  @override
  String toString() =>
      'RelayException($status $code${last == null ? '' : ' last=$last'}'
      '${epoch == null ? '' : ' epoch=$epoch'})';
}

/// One uploader's page of a pull answer.
class RelayLogPage {
  /// A page.
  const RelayLogPage(this.uploader, this.first, this.entries);

  /// The uploader.
  final Uint8List uploader;

  /// The lowest seq the relay still holds (`last + 1` if none): a cursor below
  /// `first - 1` means entries were pruned and a snapshot is needed.
  final int first;

  /// `(seq, envelope)`, ascending.
  final List<(int, Uint8List)> entries;
}

/// A snapshot the relay lists in a pull answer.
class RelaySnapshotInfo {
  /// A listing.
  const RelaySnapshotInfo(this.device, this.ref, this.covers);

  /// Who stored it.
  final Uint8List device;

  /// Its checkpoint's id (the envelope's ref).
  final Uint8List ref;

  /// What it covers.
  final List<RelayCursor> covers;
}

/// A pull answer.
class RelayPullAnswer {
  /// An answer.
  const RelayPullAnswer(this.generation, this.logs, this.snapshots, this.more);

  /// The channel's generation.
  final int generation;

  /// One page per uploader that ever appended, sorted by uploader.
  final List<RelayLogPage> logs;

  /// Every stored snapshot.
  final List<RelaySnapshotInfo> snapshots;

  /// The page was full: pull again from the new cursors.
  final bool more;
}

/// The wire format, free of I/O (tested against vectors/relay_v1.json).
abstract final class RelayWire {
  /// `SHA-256(dCBOR ["oh-relay-channel/v1", app, household])`.
  static Uint8List channel(String app, Uint8List household) =>
      _sha256(cborEncode(['oh-relay-channel/v1', app, household]));

  static List<Object?> _pairs(List<(Uint8List, int)> p) => [
    for (final (d, n) in _sorted(p)) [d, n],
  ];

  static List<(Uint8List, int)> _sorted(List<(Uint8List, int)> p) =>
      [...p]..sort((a, b) => _cmp(a.$1, b.$1));

  static int _cmp(List<int> a, List<int> b) {
    for (var i = 0; i < a.length && i < b.length; i++) {
      if (a[i] != b[i]) return a[i] - b[i];
    }
    return a.length - b.length;
  }

  static Uint8List _signed(
    String tag,
    Uint8List ch,
    Uint8List signer,
    int ts,
    List<Object?> rest,
  ) => cborEncode([tag, ch, signer, ts, ...rest]);

  /// What `forget`'s poster signs.
  static Uint8List forgetSignable(
    Uint8List ch,
    Uint8List poster,
    int ts,
    Uint8List target,
    List<Uint8List> cut,
    Uint8List auth,
    int cutSeq,
  ) => _signed('oh-relay-forget/v1', ch, poster, ts, [
    target,
    _sortedIds(cut),
    auth,
    cutSeq,
  ]);

  static List<Uint8List> _sortedIds(List<Uint8List> ids) {
    final out = [...ids]..sort(_cmp);
    return [
      for (var i = 0; i < out.length; i++)
        if (i == 0 || !sameBytes(out[i - 1], out[i])) out[i],
    ];
  }

  /// What `append`'s uploader signs.
  static Uint8List appendSignable(
    Uint8List ch,
    Uint8List uploader,
    int ts,
    int firstSeq,
    List<Uint8List> envelopes,
  ) => _signed('oh-relay-append/v1', ch, uploader, ts, [
    firstSeq,
    [for (final e in envelopes) _sha256(e)],
  ]);

  /// What `snapshot`'s device signs.
  static Uint8List snapshotSignable(
    Uint8List ch,
    Uint8List device,
    int ts,
    Uint8List envelope,
    List<(Uint8List, int)> covers,
  ) => _signed('oh-relay-snapshot/v1', ch, device, ts, [
    _sha256(envelope),
    _pairs(covers),
  ]);

  /// What `pull`'s reader signs.
  static Uint8List pullSignable(
    Uint8List ch,
    Uint8List reader,
    int ts,
    int epoch,
    Uint8List nonce,
    List<(Uint8List, int)> cursors,
  ) => _signed('oh-relay-pull/v1', ch, reader, ts, [
    epoch,
    nonce,
    _pairs(cursors),
  ]);

  /// What `fetch_snapshot`'s reader signs.
  static Uint8List fetchSignable(
    Uint8List ch,
    Uint8List reader,
    int ts,
    int epoch,
    Uint8List nonce,
    Uint8List device,
  ) => _signed('oh-relay-fetch-snapshot/v1', ch, reader, ts, [
    epoch,
    nonce,
    device,
  ]);

  static List<Object?> _ok(Uint8List body, int n) {
    final a = cborDecode(body);
    if (a is! List || a.length != n || a[0] != 'ok') {
      throw const BadAnswer('not the expected ok answer');
    }
    return a;
  }

  static T _as<T>(Object? v) => v is T ? v : throw BadAnswer('expected $T');

  static Uint8List _fixed(Object? v, int n) {
    final b = _as<Uint8List>(v);
    return b.length == n ? b : throw const BadAnswer('wrong length');
  }

  static List<RelayCursor> _cursors(Object? v) => [
    for (final p in _as<List<Object?>>(v))
      if (_as<List<Object?>>(p) case [final d, final n])
        RelayCursor(_fixed(d, 32), _as<int>(n))
      else
        throw const BadAnswer('bad pair'),
  ];

  /// `["ok", generation]`.
  static int parseEnroll(Uint8List body) => _as<int>(_ok(body, 2)[1]);

  /// `["ok", last]`.
  static int parseAppend(Uint8List body) => _as<int>(_ok(body, 2)[1]);

  /// `["ok", generation, logs, snapshots, more]`.
  static RelayPullAnswer parsePull(Uint8List body) {
    final a = _ok(body, 5);
    final logs = [
      for (final l in _as<List<Object?>>(a[2]))
        if (_as<List<Object?>>(l) case [final u, final first, final es])
          RelayLogPage(_fixed(u, 32), _as<int>(first), [
            for (final e in _as<List<Object?>>(es))
              if (_as<List<Object?>>(e) case [final seq, final env])
                (_as<int>(seq), _as<Uint8List>(env))
              else
                throw const BadAnswer('bad entry'),
          ])
        else
          throw const BadAnswer('bad log'),
    ];
    final snaps = [
      for (final s in _as<List<Object?>>(a[3]))
        if (_as<List<Object?>>(s) case [final d, final r, final covers])
          RelaySnapshotInfo(_fixed(d, 32), _fixed(r, 32), _cursors(covers))
        else
          throw const BadAnswer('bad snapshot listing'),
    ];
    return RelayPullAnswer(_as<int>(a[1]), logs, snaps, _as<bool>(a[4]));
  }

  /// `["ok", envelope, covers]`.
  static (Uint8List, List<RelayCursor>) parseFetch(Uint8List body) {
    final a = _ok(body, 3);
    return (_as<Uint8List>(a[1]), _cursors(a[2]));
  }

  /// The ref an envelope claims in clear (ADR 0008), or null.
  static Uint8List? envelopeRef(Uint8List envelope) {
    try {
      final a = cborDecode(envelope);
      if (a is List && a.length == 5 && a[2] is Uint8List) {
        return a[2] as Uint8List;
      }
    } on BadAnswer {
      // Not an envelope.
    }
    return null;
  }

  /// The error an answer with [status] carries.
  static RelayException error(int status, List<int> body) {
    try {
      final a = cborDecode(Uint8List.fromList(body));
      if (a is List && a.length >= 2 && a[0] == 'err' && a[1] is String) {
        final code = a[1] as String;
        final third = a.length == 3 && a[2] is int ? a[2] as int : null;
        return RelayException(
          status,
          code,
          last: code == 'seq' ? third : null,
          epoch: code == 'epoch' ? third : null,
        );
      }
    } on BadAnswer {
      // A proxy's page, or garbage.
    }
    return RelayException(status, 'bad_answer');
  }
}

/// A relay over HTTP. It keeps only what the kernel does not: the relay's epoch
/// (learned from the first read's `epoch` answer) and its HTTP client.
class RelayClient {
  /// A client for the relay at [base] (e.g. `https://relay.example/`).
  RelayClient(
    this.base, {
    http.Client? client,
    DateTime Function() clock = DateTime.now,
    Random? random,
    this.timeout = const Duration(seconds: 30),
    this.maxBatch = 64,
  }) : _http = client ?? http.Client(),
       _clock = clock,
       _random = random ?? Random.secure();

  /// The relay's base URL.
  final Uri base;

  /// Per-request time limit.
  final Duration timeout;

  /// Envelopes per append (the relay's `max_batch`).
  final int maxBatch;

  final http.Client _http;
  final DateTime Function() _clock;
  final Random _random;

  /// The relay's epoch as last learned (0: not yet). A relay restart moves it
  /// on; the next read learns the new one and retries once.
  int epoch = 0;

  /// How many times a read was refused for naming an old epoch.
  int epochRetries = 0;

  int get _ts => _clock().millisecondsSinceEpoch;

  Uint8List _nonce() =>
      Uint8List.fromList(List.generate(16, (_) => _random.nextInt(256)));

  /// POST [body] to [verb] on [channel]; the answer body, or a [RelayException].
  Future<Uint8List> post(Uint8List channel, String verb, Uint8List body) async {
    final uri = base.resolve('v1/${hex(channel)}/$verb');
    http.Response? r;
    // One immediate retry when the connection failed: a pooled keep-alive
    // connection to a relay that restarted fails once. Every verb is safe to
    // repeat (writes are idempotent; a read that did land answers `replay`).
    for (var attempt = 0; r == null; attempt++) {
      try {
        r = await _http
            .post(
              uri,
              headers: const {'content-type': 'application/cbor'},
              body: body,
            )
            .timeout(timeout);
      } on TimeoutException {
        throw const RelayException(0, 'network');
      } on http.ClientException {
        if (attempt > 0) throw const RelayException(0, 'network');
      }
    }
    if (r.statusCode != 200) throw RelayWire.error(r.statusCode, r.bodyBytes);
    return r.bodyBytes;
  }

  T _parse<T>(T Function() f) {
    try {
      return f();
    } on BadAnswer {
      throw const RelayException(200, 'bad_answer');
    }
  }

  /// Enrol [e]'s device; the channel's generation.
  Future<int> enroll(String app, RelayEnrollment e) async {
    final body = cborEncode([app, e.household, e.device, e.label, e.auth]);
    final r = await post(RelayWire.channel(app, e.household), 'enroll', body);
    return _parse(() => RelayWire.parseEnroll(r));
  }

  /// Append [envelopes] to [signer]'s log at [firstSeq]; the log's last seq.
  Future<int> append(
    Uint8List channel,
    Signer signer,
    int firstSeq,
    List<Uint8List> envelopes,
  ) async {
    final me = await signer.publicKey();
    final ts = _ts;
    final sig = await signer.sign(
      RelayWire.appendSignable(channel, me, ts, firstSeq, envelopes),
    );
    final body = cborEncode([me, firstSeq, envelopes, ts, sig]);
    final r = await post(channel, 'append', body);
    return _parse(() => RelayWire.parseAppend(r));
  }

  /// Post the Forget record [f].
  Future<void> forget(Uint8List channel, Signer signer, RelayForget f) async {
    final me = await signer.publicKey();
    final ts = _ts;
    final cut = RelayWire._sortedIds(f.cut);
    final sig = await signer.sign(
      RelayWire.forgetSignable(
        channel,
        me,
        ts,
        f.target,
        cut,
        f.auth,
        f.cutSeq,
      ),
    );
    await post(
      channel,
      'forget',
      cborEncode([f.target, cut, f.auth, f.cutSeq, me, ts, sig]),
    );
  }

  /// Store [s] as [signer]'s snapshot.
  Future<void> snapshot(
    Uint8List channel,
    Signer signer,
    RelaySnapshot s,
  ) async {
    final me = await signer.publicKey();
    final ts = _ts;
    final covers = [for (final c in s.covers) (c.device, c.seq)];
    final sig = await signer.sign(
      RelayWire.snapshotSignable(channel, me, ts, s.sealed, covers),
    );
    await post(
      channel,
      'snapshot',
      cborEncode([me, s.sealed, RelayWire._pairs(covers), ts, sig]),
    );
  }

  /// A read under the known epoch; on an `epoch` answer, learn it and sign
  /// again, once.
  Future<Uint8List> _read(
    Uint8List channel,
    String verb,
    Future<Uint8List> Function(int epoch, Uint8List nonce, int ts) body,
  ) async {
    for (var attempt = 0; ; attempt++) {
      try {
        return await post(channel, verb, await body(epoch, _nonce(), _ts));
      } on RelayException catch (e) {
        if (e.code != 'epoch' || e.epoch == null || attempt > 0) rethrow;
        epoch = e.epoch!;
        epochRetries++;
      }
    }
  }

  /// One page of every log from [cursors].
  Future<RelayPullAnswer> pull(
    Uint8List channel,
    Signer signer,
    List<RelayCursor> cursors,
  ) async {
    final me = await signer.publicKey();
    final at = RelayWire._sorted([for (final c in cursors) (c.device, c.seq)]);
    final r = await _read(channel, 'pull', (epoch, nonce, ts) async {
      final sig = await signer.sign(
        RelayWire.pullSignable(channel, me, ts, epoch, nonce, at),
      );
      return cborEncode([me, ts, epoch, nonce, RelayWire._pairs(at), sig]);
    });
    return _parse(() => RelayWire.parsePull(r));
  }

  /// [device]'s stored snapshot and its covers.
  Future<(Uint8List, List<RelayCursor>)> fetchSnapshot(
    Uint8List channel,
    Signer signer,
    Uint8List device,
  ) async {
    final me = await signer.publicKey();
    final r = await _read(channel, 'fetch_snapshot', (epoch, nonce, ts) async {
      final sig = await signer.sign(
        RelayWire.fetchSignable(channel, me, ts, epoch, nonce, device),
      );
      return cborEncode([me, ts, epoch, nonce, device, sig]);
    });
    return _parse(() => RelayWire.parseFetch(r));
  }

  /// Release the HTTP client.
  void close() => _http.close();
}
