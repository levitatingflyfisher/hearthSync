// The pairing code, the address chooser and the LAN keys (ADR 0014). Pure Dart,
// so the web build can parse a code it cannot use.
import 'dart:convert';
import 'dart:typed_data';

import 'package:cryptography/cryptography.dart';

/// What the listening phone shows: where it listens and a one-time token.
///
/// The text is 11 bytes in Crockford base32, `XXXXXX-XXXXXX-XXXXXX`: the IPv4
/// address (4), the port (2), the token (4) and a CRC-8 (1), so a typo fails
/// when the code is read, not when the connection times out. The token is not
/// what makes the session safe (the household key is); it binds the session to
/// this one showing of the code.
class LanCode {
  /// A code.
  LanCode(this.host, this.port, this.token) {
    if (_ipv4(host) == null) throw ArgumentError.value(host, 'host', 'not IPv4');
    if (port <= 0 || port > 0xffff) throw ArgumentError.value(port, 'port');
    if (token.length != tokenBytes) throw ArgumentError.value(token, 'token');
  }

  /// Bytes of token in a code.
  static const tokenBytes = 4;

  /// The listener's IPv4 address, dotted.
  final String host;

  /// The listener's port.
  final int port;

  /// The one-time token.
  final Uint8List token;

  /// The same code at another address (an `adb forward`, a test's loopback).
  LanCode withHost(String host, [int? port]) =>
      LanCode(host, port ?? this.port, token);

  static const _alphabet = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';

  /// The code as people read it.
  String get text {
    final b = BytesBuilder()
      ..add(_ipv4(host)!)
      ..add([port >> 8, port & 0xff])
      ..add(token);
    final body = b.toBytes();
    final all = [...body, _crc8(body)];
    // 88 bits in 18 characters: the first carries 3 bits (two zero pad bits).
    var n = BigInt.zero;
    for (final x in all) {
      n = (n << 8) | BigInt.from(x);
    }
    final out = StringBuffer();
    for (var i = 17; i >= 0; i--) {
      out.write(_alphabet[((n >> (5 * i)) & BigInt.from(31)).toInt()]);
    }
    final s = out.toString();
    return '${s.substring(0, 6)}-${s.substring(6, 12)}-${s.substring(12)}';
  }

  /// Read a typed or scanned code (or a URI whose `code` parameter is one);
  /// null if it is not a whole, correct code.
  static LanCode? tryParse(String input) {
    var t = input.trim();
    final uri = Uri.tryParse(t);
    if (uri != null && uri.hasScheme && uri.queryParameters['code'] != null) {
      t = uri.queryParameters['code']!;
    }
    t = t
        .toUpperCase()
        .replaceAll(RegExp(r'[\s-]'), '')
        .replaceAll('O', '0')
        .replaceAll(RegExp('[IL]'), '1');
    if (t.length != 18) return null;
    var n = BigInt.zero;
    for (final c in t.split('')) {
      final v = _alphabet.indexOf(c);
      if (v < 0) return null;
      n = (n << 5) | BigInt.from(v);
    }
    if (n >> 88 != BigInt.zero) return null;
    final bytes = Uint8List(11);
    for (var i = 10; i >= 0; i--) {
      bytes[i] = (n & BigInt.from(0xff)).toInt();
      n >>= 8;
    }
    if (_crc8(bytes.sublist(0, 10)) != bytes[10]) return null;
    final port = (bytes[4] << 8) | bytes[5];
    if (port == 0) return null;
    return LanCode(bytes.sublist(0, 4).join('.'), port, bytes.sublist(6, 10));
  }

  @override
  String toString() => 'LanCode($host:$port)';
}

List<int>? _ipv4(String host) {
  final parts = host.split('.');
  if (parts.length != 4) return null;
  final out = <int>[];
  for (final p in parts) {
    final v = int.tryParse(p);
    if (v == null || v < 0 || v > 255) return null;
    out.add(v);
  }
  return out;
}

/// CRC-8 (polynomial 0x07): catches any single-character typo, since a
/// character spans at most 5 bits.
int _crc8(List<int> data) {
  var crc = 0;
  for (final b in data) {
    crc ^= b;
    for (var i = 0; i < 8; i++) {
      crc = (crc & 0x80) != 0 ? ((crc << 1) ^ 0x07) & 0xff : (crc << 1) & 0xff;
    }
  }
  return crc;
}

/// The address a listener should show, from each interface's IPv4 addresses:
/// a private (RFC 1918) address, Wi-Fi first, then Ethernet, then any other
/// interface that is not cellular or a VPN. Never loopback, link-local or
/// carrier-grade NAT (100.64/10, which is also where Tailscale lives). Null if
/// this device is on no local network.
String? pickLanAddress(Map<String, List<String>> interfaces) {
  bool private(String a) {
    final p = _ipv4(a);
    if (p == null) return false;
    return p[0] == 10 ||
        (p[0] == 172 && p[1] >= 16 && p[1] <= 31) ||
        (p[0] == 192 && p[1] == 168);
  }

  int rank(String name) {
    final n = name.toLowerCase();
    if (n.startsWith('wlan') || n.startsWith('wl') || n.startsWith('swlan')) {
      return 0;
    }
    if (n.startsWith('eth') || n.startsWith('en')) return 1;
    const never = ['rmnet', 'ccmni', 'pdp', 'tun', 'tap', 'ppp', 'tailscale', 'wg', 'ipsec', 'lo', 'dummy'];
    if (never.any(n.startsWith)) return -1;
    return 2;
  }

  String? best;
  var bestRank = 99;
  for (final e in interfaces.entries) {
    final r = rank(e.key);
    if (r < 0 || r >= bestRank) continue;
    for (final a in e.value) {
      if (private(a)) {
        best = a;
        bestRank = r;
        break;
      }
    }
  }
  return best;
}

/// Framed input for a MAC: each part as a 4-byte big-endian length, then its
/// bytes, so no two different part lists frame the same.
Uint8List frame(List<List<int>> parts) {
  final b = BytesBuilder(copy: false);
  for (final p in parts) {
    b.add([p.length >> 24 & 0xff, p.length >> 16 & 0xff, p.length >> 8 & 0xff, p.length & 0xff]);
    b.add(p);
  }
  return b.toBytes();
}

/// The keys a LAN session is authenticated with, from the 64-byte household
/// seed (the 12 words) and the code's token. Only a phone holding the words can
/// compute them.
class LanKeys {
  LanKeys._(this.household, this.token);

  /// `HKDF-SHA256(seed, salt = empty, info = "openhearth.<app>.hearthsync.lan.v1")`:
  /// the same construction as the kernel's keys (ADR 0002), in its own domain.
  final Uint8List household;

  /// `HMAC-SHA256(household, frame("oh-lan/v1 token", token))`: this code's key.
  final Uint8List token;

  static final _hmac = Hmac.sha256();

  /// Derive both.
  static Future<LanKeys> derive(Uint8List seed, String app, Uint8List token) async {
    final k = await Hkdf(hmac: _hmac, outputLength: 32).deriveKey(
      secretKey: SecretKey(seed),
      nonce: const [],
      info: utf8.encode('openhearth.$app.hearthsync.lan.v1'),
    );
    final household = Uint8List.fromList(await k.extractBytes());
    final t = await mac(household, [utf8.encode('oh-lan/v1 token'), token]);
    return LanKeys._(household, t);
  }

  /// `HMAC-SHA256(key, frame(parts))`.
  static Future<Uint8List> mac(List<int> key, List<List<int>> parts) async {
    final m = await _hmac.calculateMac(frame(parts), secretKey: SecretKey(key));
    return Uint8List.fromList(m.bytes);
  }
}

/// A comparison that takes the same time wherever the first difference is.
bool constantTimeEquals(List<int> a, List<int> b) {
  if (a.length != b.length) return false;
  var d = 0;
  for (var i = 0; i < a.length; i++) {
    d |= a[i] ^ b[i];
  }
  return d == 0;
}

/// Limits a LAN session keeps to.
class LanLimits {
  /// Limits.
  const LanLimits({
    this.maxMessageBytes = 32 * 1024 * 1024,
    this.connectTimeout = const Duration(seconds: 5),
    this.callTimeout = const Duration(seconds: 60),
    this.idleTimeout = const Duration(seconds: 60),
    this.lifetime = const Duration(minutes: 10),
    this.maxAuthFailures = 3,
  });

  /// The largest message either side takes (an offer of a long history).
  final int maxMessageBytes;

  /// How long connecting to the listener may take.
  final Duration connectTimeout;

  /// How long one call (sending, the kernel's work, the answer) may take.
  final Duration callTimeout;

  /// How long an open session may sit between calls.
  final Duration idleTimeout;

  /// How long a shown code waits for a phone before it expires.
  final Duration lifetime;

  /// Refused openings before the listener gives up on this code.
  final int maxAuthFailures;
}

/// A LAN sync did not happen. Nothing was taken from the peer, except what an
/// earlier, complete step of the sync already took (which is safe: the kernel
/// takes ops one by one).
class LanException implements Exception {
  /// An error.
  const LanException(this.code, [this.detail]);

  /// `unreachable` (no answer at that address), `timeout`, `refused` (not the
  /// same household, or the code was not this listener's), `used` (the code was
  /// used or has expired), `expired` (nobody came while the code was shown),
  /// `too_large`, `tampered` (an answer failed its check), `protocol`, or
  /// `unsupported` (the web cannot listen or connect).
  final String code;

  /// More about it, for logs.
  final String? detail;

  @override
  String toString() => 'LanException($code${detail == null ? '' : ': $detail'})';
}

/// How a listener's sync ended.
class LanSyncResult {
  /// A result.
  const LanSyncResult({required this.peer, required this.at});

  /// The other phone's address.
  final String? peer;

  /// When it finished.
  final DateTime at;
}
