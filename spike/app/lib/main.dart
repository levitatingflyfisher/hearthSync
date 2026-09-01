import 'package:flutter/material.dart';
import 'package:app/src/rust/api/spike.dart';
import 'package:app/src/rust/frb_generated.dart';

/// The RFC 8032 test-1 seed and the op in spike/vectors/op_vector_v1.json.
const _seedHex =
    '9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60';
const expectedIdHex =
    '50e9a80b1d54d8bab138f3c5be8215bb389e3c779d5eab31c86ab9a7ab030057';

List<int> _hex(String s) => [
  for (var i = 0; i < s.length; i += 2)
    int.parse(s.substring(i, i + 2), radix: 16),
];
String _toHex(List<int> b) =>
    b.map((x) => x.toRadixString(16).padLeft(2, '0')).join();

/// Runs the vector through the bridge; the first call is timed cold.
({String id, bool ok, bool verified, int coldMicros}) runVector() {
  final sw = Stopwatch()..start();
  final op = spikeSignOp(
    seed: _hex(_seedHex),
    app: 'lullaby',
    hlcMillis: BigInt.from(1727000000000),
    hlcCounter: 0,
    body: 'hello'.codeUnits,
  );
  sw.stop();
  final id = _toHex(opId(op: op.signed));
  return (
    id: id,
    ok: id == expectedIdHex,
    verified: verifyOp(signed: op.signed),
    coldMicros: sw.elapsedMicroseconds,
  );
}

Future<void> main() async {
  await RustLib.init();
  runApp(const MyApp());
}

class MyApp extends StatelessWidget {
  const MyApp({super.key});

  @override
  Widget build(BuildContext context) {
    final r = runVector();
    return MaterialApp(
      home: Scaffold(
        appBar: AppBar(title: const Text('hearthSync spike')),
        body: Padding(
          padding: const EdgeInsets.all(16),
          child: SelectableText(
            'Vector: ${r.ok ? 'PASS' : 'FAIL'}\n'
            'op id: ${r.id}\n'
            'verifyOp: ${r.verified}\n'
            'cold first call: ${(r.coldMicros / 1000).toStringAsFixed(2)} ms',
          ),
        ),
      ),
    );
  }
}
