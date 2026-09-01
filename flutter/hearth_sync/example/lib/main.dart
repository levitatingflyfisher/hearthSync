// The hearth_sync demo: two devices in one process, each with its own kernel,
// key and record store, sync a room and a grocery list through the Dart API.
// The page shows PASS or FAIL and how long the first bridge calls took, as the
// spike's page did.
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:hearth_sync/hearth_sync.dart';

const _schema = SyncSchema([
  SyncTable('rooms', [SyncField('name', SyncType.text)]),
  SyncSet('groceries', SyncType.text),
]);

/// A demo household seed. A real app derives it from the 12 words.
final _seed = Uint8List.fromList(List.generate(64, (i) => i));

Future<HearthSync> _open(int n) => HearthSync.open(
  app: 'lullaby',
  schema: _schema,
  persist: MemoryPersist(),
  signer: SoftwareSigner.fromSeed(List.filled(32, n)),
  seed: _seed,
  label: 'device $n',
);

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
        'devices: ${a.devices().map((d) => d.label).join(', ')}',
    coldMicros: cold.elapsedMicroseconds,
    totalMicros: total.elapsedMicroseconds,
  );
}

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await HearthSync.init();
  runApp(const DemoApp());
}

class DemoApp extends StatelessWidget {
  const DemoApp({super.key});

  @override
  Widget build(BuildContext context) => MaterialApp(
    title: 'hearthSync',
    home: Scaffold(
      appBar: AppBar(title: const Text('hearthSync kernel demo')),
      body: FutureBuilder(
        future: runDemo(),
        builder: (context, snap) {
          final String text;
          if (snap.hasError) {
            text = 'Sync: FAIL\n${snap.error}';
          } else if (!snap.hasData) {
            text = 'Running...';
          } else {
            final r = snap.data!;
            text =
                'Sync: ${r.ok ? 'PASS' : 'FAIL'}\n${r.detail}\n'
                'first open (bridge + kernel + enrol): ${(r.coldMicros / 1000).toStringAsFixed(2)} ms\n'
                'whole demo: ${(r.totalMicros / 1000).toStringAsFixed(2)} ms';
          }
          return Padding(
            padding: const EdgeInsets.all(16),
            child: SelectableText(text),
          );
        },
      ),
    ),
  );
}
