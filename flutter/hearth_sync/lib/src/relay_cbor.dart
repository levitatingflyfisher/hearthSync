// The few dCBOR item kinds the relay protocol uses (docs/reference/relay-protocol.md):
// unsigned integers, byte and text strings, arrays, and the simple values false,
// true and null. Requests are built canonically (shortest heads, definite
// lengths); answers come from an untrusted relay, so the decoder is strict and
// depth-limited, and refuses anything else.
import 'dart:convert';
import 'dart:typed_data';

/// The largest integer every platform holds exactly (the web's ints are
/// doubles). Relay seqs, epochs, generations and times stay far below it.
const maxSafeInt = 9007199254740991;

/// Deepest nesting an answer may have: answers nest at most six deep, and the
/// relay's own guard for requests is eight.
const maxAnswerDepth = 8;

/// Encode [v]: a non-negative [int], [Uint8List], [String], [List], [bool] or
/// `null`.
Uint8List cborEncode(Object? v) {
  final out = BytesBuilder(copy: false);
  _enc(out, v);
  return out.takeBytes();
}

void _head(BytesBuilder out, int major, int n) {
  if (n < 0 || n > maxSafeInt) {
    throw ArgumentError.value(
      n,
      'n',
      'not an unsigned integer this client sends',
    );
  }
  final m = major << 5;
  if (n < 24) {
    out.addByte(m | n);
  } else if (n < 0x100) {
    out
      ..addByte(m | 24)
      ..addByte(n);
  } else if (n < 0x10000) {
    out
      ..addByte(m | 25)
      ..addByte(n >> 8)
      ..addByte(n & 0xff);
  } else if (n < 0x100000000) {
    out.addByte(m | 26);
    _be(out, n, 4);
  } else {
    // No 64-bit shifts: on the web bit operations are 32-bit.
    out.addByte(m | 27);
    _be(out, n ~/ 0x100000000, 4);
    _be(out, n % 0x100000000, 4);
  }
}

void _be(BytesBuilder out, int n, int width) {
  for (var i = width - 1; i >= 0; i--) {
    out.addByte((n ~/ _pow256[i]) % 256);
  }
}

const _pow256 = [1, 0x100, 0x10000, 0x1000000];

void _enc(BytesBuilder out, Object? v) {
  switch (v) {
    case null:
      out.addByte(0xf6);
    case false:
      out.addByte(0xf4);
    case true:
      out.addByte(0xf5);
    case int n:
      _head(out, 0, n);
    case Uint8List b:
      _head(out, 2, b.length);
      out.add(b);
    case String s:
      final b = utf8.encode(s);
      _head(out, 3, b.length);
      out.add(b);
    case List<Object?> l:
      _head(out, 4, l.length);
      for (final x in l) {
        _enc(out, x);
      }
    default:
      throw ArgumentError.value(v, 'v', 'not a relay protocol item');
  }
}

/// An answer that is not the protocol's CBOR.
class BadAnswer implements Exception {
  /// A bad answer.
  const BadAnswer(this.why);

  /// What was wrong.
  final String why;

  @override
  String toString() => 'BadAnswer($why)';
}

/// Decode one item filling all of [data], nested at most [maxDepth] deep. Ints
/// come back as [int], strings as [Uint8List] or [String], arrays as
/// `List<Object?>`.
Object? cborDecode(Uint8List data, {int maxDepth = maxAnswerDepth}) {
  final d = _Dec(data, maxDepth);
  final v = d.item(0);
  if (d.i != data.length) throw const BadAnswer('trailing bytes');
  return v;
}

class _Dec {
  _Dec(this.b, this.maxDepth);
  final Uint8List b;
  final int maxDepth;
  int i = 0;

  int _byte() {
    if (i >= b.length) throw const BadAnswer('truncated');
    return b[i++];
  }

  int _arg(int info) {
    if (info < 24) return info;
    final width = switch (info) {
      24 => 1,
      25 => 2,
      26 => 4,
      27 => 8,
      _ => throw const BadAnswer('indefinite or reserved length'),
    };
    var n = 0;
    for (var k = 0; k < width; k++) {
      n = n * 256 + _byte();
      if (n > maxSafeInt) throw const BadAnswer('integer too large');
    }
    // Shortest form only, as dCBOR requires.
    final min = switch (width) {
      1 => 24,
      2 => 0x100,
      4 => 0x10000,
      _ => 0x100000000,
    };
    if (n < min) throw const BadAnswer('not the shortest form');
    return n;
  }

  Uint8List _take(int n) {
    if (n > b.length - i) throw const BadAnswer('truncated');
    final out = Uint8List.sublistView(b, i, i + n);
    i += n;
    return out;
  }

  Object? item(int depth) {
    final h = _byte();
    final major = h >> 5, info = h & 0x1f;
    switch (major) {
      case 0:
        return _arg(info);
      case 2:
        return Uint8List.fromList(_take(_arg(info)));
      case 3:
        try {
          return utf8.decode(_take(_arg(info)));
        } on FormatException {
          throw const BadAnswer('bad UTF-8');
        }
      case 4:
        final n = _arg(info);
        if (n > 0 && depth + 1 >= maxDepth) throw const BadAnswer('too deep');
        // Every item takes at least a byte: no huge allocation from a lying length.
        if (n > b.length - i) throw const BadAnswer('truncated');
        return [for (var k = 0; k < n; k++) item(depth + 1)];
      case 7:
        return switch (info) {
          20 => false,
          21 => true,
          22 => null,
          _ => throw const BadAnswer('unsupported simple value'),
        };
      default:
        throw const BadAnswer('unsupported item');
    }
  }
}
