// The web build is single-threaded WASM with non-shared memory (example/tool/
// build_web.sh, ADR 0012), so frb's worker pool cannot start there: its first use
// panics with "#<Memory> could not be cloned" (seen in headless Chromium, browser
// test report). Only an async (`executeNormal`) bridge call builds that pool, so
// every call the generated bindings make, initializers included, must be sync.
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

void main() {
  test('every generated bridge call is sync, so the web never builds the '
      'worker pool', () {
    final src = File('lib/src/rust/frb_generated.dart').readAsStringSync();
    final asyncCalls = RegExp(
      r'Future<[^>]*> (\w+)\([^)]*\) \{\s*return handler\.executeNormal\(',
    ).allMatches(src).map((m) => m.group(1)).toList();
    expect(asyncCalls, isEmpty);
    expect(src.contains('executeNormal('), isFalse);
  });
}
