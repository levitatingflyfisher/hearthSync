// Host-side bridge test: loads the Linux build of the spike crate through
// flutter_rust_bridge and checks it against the independent Python vector.
// Build first: (cd rust && cargo build --release), with CARGO_TARGET_DIR at the
// repo's target/ (or set HEARTH_SPIKE_LIB to the .so).
import 'dart:convert';
import 'dart:io';

import 'package:app/main.dart' show runVector;
import 'package:app/src/rust/api/spike.dart';
import 'package:app/src/rust/frb_generated.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';

List<int> _hex(String s) => [
  for (var i = 0; i < s.length; i += 2) int.parse(s.substring(i, i + 2), radix: 16),
];
String _toHex(List<int> b) =>
    b.map((x) => x.toRadixString(16).padLeft(2, '0')).join();

void main() {
  late Map<String, dynamic> v;

  setUpAll(() async {
    final lib = Platform.environment['HEARTH_SPIKE_LIB'] ??
        '../../target/release/librust_lib_app.so';
    await RustLib.init(externalLibrary: ExternalLibrary.open(lib));
    v = jsonDecode(File('../vectors/op_vector_v1.json').readAsStringSync())
        as Map<String, dynamic>;
  });

  test('opId over the bridge matches the Python vector id', () {
    expect(_toHex(opId(op: _hex(v['signed'] as String))), v['id']);
  });

  test('opId rejects non-canonical CBOR with an exception', () {
    expect(() => opId(op: _hex('a20a4100016161')), throwsA(anything));
  });

  test('signing in Rust reproduces the Python-signed op byte for byte', () {
    final op = spikeSignOp(
      seed: _hex(v['seed'] as String),
      app: v['app'] as String,
      hlcMillis: BigInt.from(v['hlc_millis'] as int),
      hlcCounter: v['hlc_counter'] as int,
      body: _hex(v['body'] as String),
    );
    expect(_toHex(op.signed), v['signed']);
    expect(_toHex(op.id), v['id']);
    expect(_toHex(op.device), v['device']);
  });

  test('verifyEd25519 and verifyOp accept the vector, reject tampering', () {
    final pk = _hex(v['device'] as String);
    final sig = _hex(v['sig'] as String);
    final signable = _hex(v['signable'] as String);
    expect(verifyEd25519(pk: pk, msg: signable, sig: sig), isTrue);
    expect(verifyEd25519(pk: pk, msg: [...signable, 0], sig: sig), isFalse);
    expect(verifyOp(signed: _hex(v['signed'] as String)), isTrue);
  });

  test('the app runVector path passes (host proxy for the device check)', () {
    final r = runVector();
    expect(r.ok, isTrue);
    expect(r.verified, isTrue);
    // ignore: avoid_print
    print('host first spikeSignOp call: ${r.coldMicros} us');
  });
}
