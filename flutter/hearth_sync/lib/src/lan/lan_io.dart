// Same-Wi-Fi sync over plain HTTP on the local network (ADR 0014). Native only:
// a web page cannot listen, and the web build gets lan_stub.dart instead.
//
// The protocol, in full:
//
//   POST /oh-lan/v1/open     body: client nonce (16)   x-oh-mac: HMAC(Kt, "oh-lan/v1 open", cn)
//     200 body: server nonce (16) ‖ session id (16)    x-oh-mac: HMAC(Kt, "oh-lan/v1 opened", cn, sn, sid)
//   Ks = HMAC(Kt, "oh-lan/v1 session", cn, sn, sid)
//   POST /oh-lan/v1/<method> body: the kernel's sealed message
//     x-oh-session: hex(sid)   x-oh-seq: n (1, 2, 3, ... exactly)
//     x-oh-mac: HMAC(Ks, "oh-lan/v1 c2s", sid, n, method, body)
//     200/422 body: answer (or the kernel's error code)   x-oh-flags: "s" needs snapshot, "w" wiped
//     x-oh-mac: HMAC(Ks, "oh-lan/v1 s2c", sid, n, method, status, flags, body)
//
// Kt is LanKeys.token: only a phone holding the household's words and this
// code's token computes it. Every refusal ends the session, so a replayed,
// reordered or altered message gets nothing further from the listener.
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';

import 'package:flutter/foundation.dart' show visibleForTesting;

import '../hearth_sync.dart';
import '../values.dart';
import 'lan_code.dart';

const _prefix = '/oh-lan/v1/';
const _methods = {'hello', 'request', 'offer', 'handover', 'accept', 'wiped', 'close'};
final _ascii = ascii.encoder;

/// [s] as bytes if it is even-length hex, else null.
Uint8List? unhexOrNull(String? s) {
  if (s == null || s.length.isOdd || !RegExp(r'^[0-9a-fA-F]*$').hasMatch(s)) {
    return null;
  }
  return unhex(s);
}

List<int> _seqBytes(int n) =>
    [for (var i = 7; i >= 0; i--) (n >> (8 * i)) & 0xff];

Uint8List _bytes(Random r, int n) =>
    Uint8List.fromList(List.generate(n, (_) => r.nextInt(256)));

/// Read a body of at most [limit] bytes, counting as it reads (a chunked body
/// has no length to check up front).
Future<Uint8List?> _readLimited(Stream<List<int>> s, int limit, Duration timeout) async {
  final b = BytesBuilder(copy: false);
  var over = false;
  await for (final chunk in s.timeout(timeout)) {
    if (b.length + chunk.length > limit) {
      over = true;
      break;
    }
    b.add(chunk);
  }
  return over ? null : b.toBytes();
}

/// This phone listens on the local network for one other phone of the
/// household, which connects with [code] and drives a full two-way sync.
class LanListener {
  LanListener._(this._hs, this._keys, this._server, this.code, this.limits, this._random);

  final HearthSync _hs;
  final LanKeys _keys;
  final HttpServer _server;
  final Random _random;

  /// What to show the other phone.
  final LanCode code;

  /// The limits it keeps.
  final LanLimits limits;

  final _done = Completer<LanSyncResult>();
  Timer? _lifetime, _idle;
  bool _opened = false;
  int _failures = 0;
  Uint8List? _sid, _key;
  int _seq = 0;
  String? _peer;
  bool _busy = false;

  /// Start listening for [hs]'s household (whose 64-byte [seed] the words
  /// give). [advertise] is the address the code carries; by default the
  /// device's private Wi-Fi or Ethernet address ([pickLanAddress]). Throws
  /// [LanException] `unreachable` if this device is on no local network.
  static Future<LanListener> start(
    HearthSync hs,
    Uint8List seed, {
    String? advertise,
    int port = 0,
    LanLimits limits = const LanLimits(),
    Random? random,
  }) async {
    final host = advertise ?? await localLanAddress();
    if (host == null) {
      throw const LanException('unreachable', 'no Wi-Fi or Ethernet address');
    }
    final rnd = random ?? Random.secure();
    final token = _bytes(rnd, LanCode.tokenBytes);
    final keys = await LanKeys.derive(seed, hs.app, token);
    final server = await HttpServer.bind(InternetAddress.anyIPv4, port);
    server.idleTimeout = const Duration(seconds: 10);
    final l = LanListener._(hs, keys, server, LanCode(host, server.port, token), limits, rnd);
    // A listener nobody awaits must not surface its ending as an uncaught error.
    unawaited(l._done.future.then((_) {}, onError: (Object _) {}));
    l._lifetime = Timer(limits.lifetime, () {
      if (!l._opened) l._finish(const LanException('expired'));
    });
    server.listen(l._handle, onError: (Object _) {});
    return l;
  }

  /// Completes when the other phone finished its sync; fails with
  /// [LanException] if the code expired, was refused too often, or the
  /// session broke off.
  Future<LanSyncResult> get done => _done.future;

  /// Stop listening (the code stops working).
  Future<void> stop() => _finish(const LanException('used', 'stopped'));

  Future<void> _finish([Object? error]) async {
    _lifetime?.cancel();
    _idle?.cancel();
    if (!_done.isCompleted) {
      if (error == null) {
        _done.complete(LanSyncResult(peer: _peer, at: DateTime.now()));
      } else {
        _done.completeError(error);
      }
    }
    await _server.close(force: true);
  }

  void _touch() {
    _idle?.cancel();
    _idle = Timer(limits.idleTimeout, () => _finish(const LanException('timeout', 'the other phone went quiet')));
  }

  Future<void> _handle(HttpRequest req) async {
    try {
      await _route(req);
    } on Object catch (e) {
      try {
        req.response.statusCode = HttpStatus.badRequest;
        await req.response.close();
      } on Object catch (_) {}
      if (e is TimeoutException) await _finish(const LanException('timeout'));
    }
  }

  Future<void> _reply(HttpRequest req, int status, [List<int> body = const [], Map<String, String> headers = const {}]) async {
    final r = req.response..statusCode = status;
    headers.forEach(r.headers.set);
    r.contentLength = body.length;
    r.add(body);
    await r.close();
  }

  Future<void> _route(HttpRequest req) async {
    final path = req.uri.path;
    if (req.method != 'POST' || !path.startsWith(_prefix)) {
      return _reply(req, HttpStatus.notFound);
    }
    final method = path.substring(_prefix.length);
    if (method != 'open' && !_methods.contains(method)) {
      return _reply(req, HttpStatus.notFound);
    }
    if (req.contentLength > limits.maxMessageBytes) {
      return _reply(req, HttpStatus.requestEntityTooLarge);
    }
    final body = await _readLimited(req, limits.maxMessageBytes, limits.callTimeout);
    if (body == null) return _reply(req, HttpStatus.requestEntityTooLarge);
    final mac = unhexOrNull(req.headers.value('x-oh-mac'));
    if (method == 'open') return _open(req, body, mac);
    return _call(req, method, body, mac);
  }

  Future<void> _refuse(HttpRequest req, String why) async {
    await _reply(req, HttpStatus.forbidden);
    if (_opened) {
      // A session that sees one bad message is over.
      await _finish(LanException('refused', why));
    } else if (++_failures >= limits.maxAuthFailures) {
      await _finish(LanException('refused', 'too many refused phones'));
    }
  }

  Future<void> _open(HttpRequest req, Uint8List cn, Uint8List? mac) async {
    if (_opened || _done.isCompleted) return _reply(req, HttpStatus.gone);
    if (cn.length != 16 || mac == null) return _refuse(req, 'bad opening');
    final want = await LanKeys.mac(_keys.token, [_ascii.convert('oh-lan/v1 open'), cn]);
    if (!constantTimeEquals(want, mac)) return _refuse(req, 'not this household');
    _opened = true;
    _lifetime?.cancel();
    final sn = _bytes(_random, 16), sid = _bytes(_random, 16);
    _sid = sid;
    _key = await LanKeys.mac(_keys.token, [_ascii.convert('oh-lan/v1 session'), cn, sn, sid]);
    _peer = req.connectionInfo?.remoteAddress.address;
    final answer = await LanKeys.mac(_keys.token, [_ascii.convert('oh-lan/v1 opened'), cn, sn, sid]);
    _touch();
    return _reply(req, HttpStatus.ok, [...sn, ...sid], {'x-oh-mac': hex(answer)});
  }

  Future<void> _call(HttpRequest req, String method, Uint8List body, Uint8List? mac) async {
    final sid = _sid, key = _key;
    if (!_opened || sid == null || key == null || _done.isCompleted) {
      return _reply(req, HttpStatus.gone);
    }
    final seq = int.tryParse(req.headers.value('x-oh-seq') ?? '');
    final session = unhexOrNull(req.headers.value('x-oh-session'));
    if (mac == null || seq == null || session == null || !sameBytes(session, sid)) {
      return _refuse(req, 'bad headers');
    }
    final want = await LanKeys.mac(key, [_ascii.convert('oh-lan/v1 c2s'), sid, _seqBytes(seq), _ascii.convert(method), body]);
    if (!constantTimeEquals(want, mac)) return _refuse(req, 'altered message');
    if (seq != _seq + 1 || _busy) return _refuse(req, 'replayed or out of order');
    _seq = seq;
    _busy = true;
    _touch();
    var status = HttpStatus.ok;
    var flags = '';
    List<int> out = const [];
    try {
      switch (method) {
        case 'hello':
          out = await _hs.hello();
        case 'request':
          final r = await _hs.request(body);
          out = r.message;
          if (r.needsSnapshot) flags = 's';
        case 'offer':
          out = await _hs.offer(body);
        case 'handover':
          out = await _hs.handover(body);
        case 'accept':
          await _hs.accept(body);
        case 'wiped':
          if (await _hs.wiped()) flags = 'w';
        case 'close':
          break;
      }
    } on HearthSyncException catch (e) {
      status = 422;
      out = utf8.encode(e.code);
    } finally {
      _busy = false;
    }
    final answer = await LanKeys.mac(key, [
      _ascii.convert('oh-lan/v1 s2c'), sid, _seqBytes(seq), _ascii.convert(method),
      _ascii.convert('$status'), _ascii.convert(flags), out,
    ]);
    await _reply(req, status, out, {'x-oh-mac': hex(answer), 'x-oh-flags': flags});
    if (method == 'close') await _finish();
  }
}

/// One sent message, kept for tests.
@visibleForTesting
class LanSent {
  /// A sent message.
  const LanSent(this.method, this.body, this.seq, this.mac);

  /// What was sent.
  final String method;

  /// Its body.
  final Uint8List body;

  /// Its seq.
  final int seq;

  /// Its MAC.
  final Uint8List mac;
}

/// The listening phone, seen from the connecting one: a [SyncPeer] over an
/// authenticated session.
class LanSession implements SyncPeer {
  LanSession._(this._client, this._base, this._sid, this._key, this.limits);

  final HttpClient _client;
  final Uri _base;
  final Uint8List _sid, _key;

  /// The limits it keeps.
  final LanLimits limits;
  int _seq = 0;

  /// The last message sent (tests replay it).
  @visibleForTesting
  LanSent? lastSent;

  static HttpClient _newClient(LanLimits limits) => HttpClient()
    ..connectionTimeout = limits.connectTimeout
    // Straight to the other phone: never through a configured proxy.
    ..findProxy = ((_) => 'DIRECT');

  /// Open a session with the phone showing [code], for [hs]'s household.
  static Future<LanSession> open(
    HearthSync hs,
    Uint8List seed,
    LanCode code, {
    LanLimits limits = const LanLimits(),
    Random? random,
  }) async {
    final keys = await LanKeys.derive(seed, hs.app, code.token);
    final client = _newClient(limits);
    final base = Uri(scheme: 'http', host: code.host, port: code.port, path: _prefix);
    final cn = _bytes(random ?? Random.secure(), 16);
    final mac = await LanKeys.mac(keys.token, [_ascii.convert('oh-lan/v1 open'), cn]);
    try {
      final (status, body, headers) = await _post(client, base.resolve('open'), cn, {'x-oh-mac': hex(mac)}, limits);
      if (status == HttpStatus.forbidden) throw const LanException('refused');
      if (status == HttpStatus.gone) throw const LanException('used');
      if (status != HttpStatus.ok || body.length != 32) {
        throw LanException('protocol', 'open answered $status');
      }
      final sn = body.sublist(0, 16), sid = body.sublist(16);
      final want = await LanKeys.mac(keys.token, [_ascii.convert('oh-lan/v1 opened'), cn, sn, sid]);
      final got = unhexOrNull(headers.value('x-oh-mac'));
      // The listener proved it holds the words too.
      if (got == null || !constantTimeEquals(want, got)) {
        throw const LanException('refused', 'the listener is not this household');
      }
      final key = await LanKeys.mac(keys.token, [_ascii.convert('oh-lan/v1 session'), cn, sn, sid]);
      return LanSession._(client, base, sid, key, limits);
    } catch (_) {
      client.close(force: true);
      rethrow;
    }
  }

  static Future<(int, Uint8List, HttpHeaders)> _post(
    HttpClient client,
    Uri uri,
    List<int> body,
    Map<String, String> headers,
    LanLimits limits,
  ) async {
    try {
      final req = await client.postUrl(uri).timeout(limits.connectTimeout);
      headers.forEach(req.headers.set);
      req.headers.contentType = ContentType.binary;
      req.contentLength = body.length;
      req.add(body);
      final res = await req.close().timeout(limits.callTimeout);
      if (res.contentLength > limits.maxMessageBytes) {
        throw const LanException('too_large');
      }
      final out = await _readLimited(res, limits.maxMessageBytes, limits.callTimeout);
      if (out == null) throw const LanException('too_large');
      return (res.statusCode, out, res.headers);
    } on LanException {
      rethrow;
    } on TimeoutException {
      throw const LanException('timeout');
    } on SocketException catch (e) {
      throw LanException('unreachable', e.message);
    } on HttpException catch (e) {
      throw LanException('unreachable', e.message);
    }
  }

  /// The MAC a message would carry (tests forge with it).
  @visibleForTesting
  Future<Uint8List> macFor(String method, List<int> body, {required int seq}) =>
      LanKeys.mac(_key, [_ascii.convert('oh-lan/v1 c2s'), _sid, _seqBytes(seq), _ascii.convert(method), body]);

  /// Send a message as given and return the status (tests).
  @visibleForTesting
  Future<int> raw(String method, List<int> body, {required int seq, required List<int> mac}) async {
    final (status, _, _) = await _post(_client, _base.resolve(method), body, {
      'x-oh-session': hex(_sid),
      'x-oh-seq': '$seq',
      'x-oh-mac': hex(mac),
    }, limits);
    return status;
  }

  Future<(Uint8List, String)> _call(String method, [List<int> body = const []]) async {
    final seq = ++_seq;
    final mac = await macFor(method, body, seq: seq);
    lastSent = LanSent(method, Uint8List.fromList(body), seq, mac);
    final (status, out, headers) = await _post(_client, _base.resolve(method), body, {
      'x-oh-session': hex(_sid),
      'x-oh-seq': '$seq',
      'x-oh-mac': hex(mac),
    }, limits);
    if (status == HttpStatus.forbidden) throw const LanException('refused');
    if (status == HttpStatus.gone) throw const LanException('used');
    if (status == HttpStatus.requestEntityTooLarge) throw const LanException('too_large');
    if (status != HttpStatus.ok && status != 422) {
      throw LanException('protocol', '$method answered $status');
    }
    final flags = headers.value('x-oh-flags') ?? '';
    final want = await LanKeys.mac(_key, [
      _ascii.convert('oh-lan/v1 s2c'), _sid, _seqBytes(seq), _ascii.convert(method),
      _ascii.convert('$status'), _ascii.convert(flags), out,
    ]);
    final got = unhexOrNull(headers.value('x-oh-mac'));
    if (got == null || !constantTimeEquals(want, got)) {
      throw const LanException('tampered');
    }
    if (status == 422) throw HearthSyncException(utf8.decode(out, allowMalformed: true));
    return (out, flags);
  }

  @override
  Future<Uint8List> hello() async => (await _call('hello')).$1;

  @override
  Future<SyncRequest> request(Uint8List hello) async {
    final (out, flags) = await _call('request', hello);
    return SyncRequest(out, needsSnapshot: flags.contains('s'));
  }

  @override
  Future<Uint8List> offer(Uint8List request) async => (await _call('offer', request)).$1;

  @override
  Future<Uint8List> handover(Uint8List request) async => (await _call('handover', request)).$1;

  @override
  Future<void> accept(Uint8List offer) async => _call('accept', offer);

  @override
  Future<bool> wiped() async => (await _call('wiped')).$2.contains('w');

  /// End the session; the listener stops.
  Future<void> close() async {
    try {
      await _call('close');
    } finally {
      _client.close(force: true);
    }
  }

  /// Drop the connection without the closing message.
  void abandon() => _client.close(force: true);
}

/// Connect to the phone showing [code] and sync [hs] with it both ways.
/// Throws [LanException] (or [HearthSyncException] from either kernel).
Future<void> syncOverLan(
  HearthSync hs,
  Uint8List seed,
  LanCode code, {
  LanLimits limits = const LanLimits(),
}) async {
  final session = await LanSession.open(hs, seed, code, limits: limits);
  try {
    await hs.syncWith(session);
  } catch (_) {
    session.abandon();
    rethrow;
  }
  await session.close();
}

/// This device's address on its local network ([pickLanAddress]), or null.
Future<String?> localLanAddress() async {
  final list = await NetworkInterface.list(type: InternetAddressType.IPv4);
  return pickLanAddress({
    for (final i in list) i.name: [for (final a in i.addresses) a.address],
  });
}

/// Same-Wi-Fi sync can run here.
const lanSupported = true;
