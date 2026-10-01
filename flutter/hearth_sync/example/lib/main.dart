// The hearth_sync demo: two devices in one process, each with its own kernel,
// key and record store, sync a room and a grocery list through the Dart API.
// The page shows PASS or FAIL and how long the first bridge calls took, as the
// spike's page did, plus which signing backend this platform chose and whether
// it signs RFC 8032's vector byte for byte. Each result line is also printed
// (the browser console on the web), so a browser test can read it. On the web,
// `?device=A` opens the browser probe instead (lib/probe/probe_web.dart).
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:hearth_sync/hearth_sync.dart';

import 'device/panel_stub.dart' if (dart.library.ffi) 'device/panel_io.dart';
import 'probe/probe_stub.dart'
    if (dart.library.js_interop) 'probe/probe_web.dart';
import 'shared.dart';

Future<HearthSync> _open(int n) => HearthSync.open(
  app: 'lullaby',
  schema: demoSchema,
  persist: MemoryPersist(),
  signer: SoftwareSigner.fromSeed(List.filled(32, n)),
  seed: demoSeed,
  label: 'device $n',
);

Uint8List _unhex(String s) => Uint8List.fromList([
  for (var i = 0; i < s.length; i += 2)
    int.parse(s.substring(i, i + 2), radix: 16),
]);

/// RFC 8032 §7.1 TEST 1 through the signer the demo's devices use, on the
/// backend this platform picks (WebCrypto in a browser that has Ed25519).
Future<String> runVector() async {
  final s = SoftwareSigner.fromSeed(
    _unhex('9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60'),
  );
  final backend = await s.backend();
  final sig = hex(await s.sign(Uint8List(0)));
  final pk = hex(await s.publicKey());
  final ok =
      sig ==
          'e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b' &&
      pk == 'd75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a';
  return 'Vector: ${ok ? 'PASS' : 'FAIL'} (RFC 8032 test 1)\n'
      'Signer: ${backend.name}';
}

/// Two devices: A names a room, B adds milk, they sync, and each must see both.
Future<({bool ok, String detail, int coldMicros, int totalMicros})>
runDemo() async {
  final total = Stopwatch()..start();
  final cold = Stopwatch()..start();
  final a = await _open(1);
  cold.stop();
  final b = await _open(2);
  await a.put('rooms', 'den', {'name': 'Den'});
  await b.setAdd('groceries', 'milk');
  await a.syncWith(b);
  final va = a.view(), vb = b.view();
  total.stop();
  final vector = await runVector(); // after the timings, so they stay cold
  final ok = [va, vb].every(
    (v) =>
        v.rows.length == 1 &&
        v.rows.single.fields['name'] == 'Den' &&
        v.sets.length == 1 &&
        v.sets.single.element == 'milk',
  );
  return (
    ok: ok,
    detail:
        'A: ${va.rows.map((r) => r.fields)} ${va.sets.map((s) => s.element)}\n'
        'B: ${vb.rows.map((r) => r.fields)} ${vb.sets.map((s) => s.element)}\n'
        'devices: ${a.devices().map((d) => d.label).join(', ')}\n'
        '$vector',
    coldMicros: cold.elapsedMicroseconds,
    totalMicros: total.elapsedMicroseconds,
  );
}

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final sw = Stopwatch()..start();
  await HearthSync.init();
  _initMicros = sw.elapsedMicroseconds;
  // ignore: avoid_print
  print(_initLine());
  runApp(DemoApp(probe: await startProbe()));
}

/// How long [HearthSync.init] took: loading the library and its first bridge
/// call (the `init_app` initializer).
int _initMicros = 0;
String _initLine() =>
    'init (load library + first bridge call): '
    '${(_initMicros / 1000).toStringAsFixed(2)} ms';

class DemoApp extends StatelessWidget {
  const DemoApp({super.key, this.probe});

  /// The browser probe's status, when the page opened one instead of the demo.
  final String? probe;

  @override
  Widget build(BuildContext context) => MaterialApp(
    title: 'hearthSync',
    home: Scaffold(
      appBar: AppBar(title: const Text('hearthSync kernel demo')),
      body: FutureBuilder(
        future: probe != null ? null : _report(),
        builder: (context, snap) {
          final String text;
          if (probe case final p?) {
            text = p;
          } else if (snap.hasError) {
            text = 'Sync: FAIL\n${snap.error}';
          } else if (!snap.hasData) {
            text = 'Running...';
          } else {
            final r = snap.data!;
            text =
                'Sync: ${r.ok ? 'PASS' : 'FAIL'}\n${r.detail}\n'
                'first open (bridge + kernel + enrol): ${(r.coldMicros / 1000).toStringAsFixed(2)} ms\n'
                'whole demo: ${(r.totalMicros / 1000).toStringAsFixed(2)} ms\n'
                '${_initLine()}';
          }
          return ListView(
            padding: const EdgeInsets.all(16),
            children: [SelectableText(text), ?devicePanel()],
          );
        },
      ),
    ),
  );
}

/// Runs the demo once and prints each line of the result.
Future<({bool ok, String detail, int coldMicros, int totalMicros})>
_report() async {
  try {
    final r = await runDemo();
    // ignore: avoid_print
    print(
      'Sync: ${r.ok ? 'PASS' : 'FAIL'}\n${r.detail}\n'
      'first open (bridge + kernel + enrol): ${(r.coldMicros / 1000).toStringAsFixed(2)} ms\n'
      'whole demo: ${(r.totalMicros / 1000).toStringAsFixed(2)} ms',
    );
    return r;
  } catch (e) {
    // ignore: avoid_print
    print('Sync: FAIL $e');
    rethrow;
  }
}
