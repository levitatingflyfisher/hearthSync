import 'dart:typed_data';

import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show PlatformInt64, PlatformInt64Util;

import 'rust/api/kernel.dart';

/// A Dart value as the kernel stores it: `null`, [bool], [int], [String] or
/// [Uint8List] (any other [List<int>] is taken as bytes).
KernelValue toKernel(Object? v) => switch (v) {
  null => const KernelValue.null_(),
  bool b => KernelValue.bool(b),
  int n => KernelValue.int(PlatformInt64Util.from(n)),
  String s => KernelValue.text(s),
  Uint8List b => KernelValue.bytes(b),
  List<int> b => KernelValue.bytes(Uint8List.fromList(b)),
  _ => throw ArgumentError.value(
    v,
    'value',
    'not null, bool, int, String or bytes',
  ),
};

/// The kernel's value as plain Dart.
Object? fromKernel(KernelValue v) => switch (v) {
  KernelValue_Null() => null,
  KernelValue_Bool(:final field0) => field0,
  KernelValue_Int(:final field0) => i64(field0),
  KernelValue_Text(:final field0) => field0,
  KernelValue_Bytes(:final field0) => field0,
};

/// A 64-bit integer from the bridge as a Dart int (an int on native, a BigInt on
/// the web).
int i64(PlatformInt64 v) {
  final Object o = v;
  return o is BigInt ? o.toInt() : o as int;
}

/// Unix millis to the bridge's u64.
BigInt u64(int millis) => BigInt.from(millis);

/// Byte equality.
bool sameBytes(List<int>? a, List<int>? b) {
  if (identical(a, b)) return true;
  if (a == null || b == null || a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i] != b[i]) return false;
  }
  return true;
}

/// Lowercase hex, for keys and display.
String hex(List<int> b) =>
    b.map((x) => x.toRadixString(16).padLeft(2, '0')).join();

/// The inverse of [hex].
Uint8List unhex(String s) => Uint8List.fromList([
  for (var i = 0; i < s.length; i += 2)
    int.parse(s.substring(i, i + 2), radix: 16),
]);
