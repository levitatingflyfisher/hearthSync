// The device probe: real devices the way an Android app would wire them (records
// in a Drift database file, the key in flutter_secure_storage), one per relay
// named in `--dart-define=HEARTH_RELAYS=rust=http://10.0.2.2:18081/,go=...`.
// Each device has Put / Sync / Forget buttons and a signer benchmark; every
// result is printed with a `PROBE` prefix so logcat carries the evidence. With
// no HEARTH_RELAYS the panel is not shown.
import 'dart:io';
import 'dart:typed_data';

import 'package:drift/drift.dart' show GeneratedDatabase, TableInfo, Table;
import 'package:drift/native.dart';
import 'package:flutter/material.dart' hide Table;
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:hearth_sync/hearth_sync.dart';
import 'package:path_provider/path_provider.dart';

import '../shared.dart';

const _relays = String.fromEnvironment('HEARTH_RELAYS');

Widget? devicePanel() => _relays.isEmpty ? null : const DevicePanel();

void _log(String s) {
  // ignore: avoid_print
  print('PROBE $s');
}

/// An app database with no tables of its own (DriftPersist needs no codegen).
class _Db extends GeneratedDatabase {
  _Db(super.e);
  @override
  Iterable<TableInfo<Table, dynamic>> get allTables => const [];
  @override
  int get schemaVersion => 1;
}

/// Times every signature the device's own signer makes.
class _TimedSigner implements Signer {
  _TimedSigner(this.inner, this.name);
  final Signer inner;
  final String name;

  @override
  Future<Uint8List> publicKey() => inner.publicKey();

  @override
  Future<Uint8List> sign(Uint8List message) async {
    final sw = Stopwatch()..start();
    final sig = await inner.sign(message);
    _log('sign device=$name us=${sw.elapsedMicroseconds}');
    return sig;
  }

  @override
  Future<void> destroy() async {
    await inner.destroy();
    _log('destroyed device=$name');
  }
}

class _Device {
  _Device(this.name, this.url);
  final String name;
  final Uri url;
  HearthSync? hs;
  late final RelayClient relay = RelayClient(url);
  String state = 'opening';
  int puts = 0;

  String get storageKey => 'hearth_probe.$name';
}

class DevicePanel extends StatefulWidget {
  const DevicePanel({super.key});

  @override
  State<DevicePanel> createState() => _DevicePanelState();
}

class _DevicePanelState extends State<DevicePanel> {
  static const _storage = FlutterSecureStorage();
  final _devices = [
    for (final e in _relays.split(','))
      _Device(e.split('=').first, Uri.parse(e.substring(e.indexOf('=') + 1))),
  ];
  String _bench = '';

  @override
  void initState() {
    super.initState();
    _openAll();
  }

  Future<String> _keyState(_Device d) async =>
      await _storage.read(key: d.storageKey) == null ? 'absent' : 'present';

  Future<void> _openAll() async {
    final dir = await getApplicationSupportDirectory();
    for (final d in _devices) {
      final db = _Db(NativeDatabase(File('${dir.path}/hearth_${d.name}.db')));
      final sw = Stopwatch()..start();
      try {
        d.hs = await HearthSync.open(
          app: 'lullaby',
          schema: demoSchema,
          persist: DriftPersist(db),
          signer: _TimedSigner(
            SecureStorageSigner(key: d.storageKey),
            d.name,
          ),
          seed: demoSeed,
          label: 'android ${d.name}',
        );
        d.state = 'open';
        _log(
          'open device=${d.name} ms=${sw.elapsedMilliseconds} '
          'key=${await _keyState(d)} ${_view(d)}',
        );
      } on DeviceWipedException {
        d.state = 'WIPED: this device was forgotten';
        _log('open device=${d.name} WIPED key=${await _keyState(d)}');
      } catch (e) {
        d.state = 'open failed: $e';
        _log('open device=${d.name} FAILED $e');
      }
    }
    if (mounted) setState(() {});
  }

  String _view(_Device d) {
    final hs = d.hs;
    if (hs == null) return 'view=-';
    final v = hs.view();
    final rows = {for (final r in v.rows) r.row: r.fields['name']};
    final sets = [for (final s in v.sets) s.element]..sort();
    final devs = [
      for (final x in hs.devices())
        '${x.label}${x.forgotten ? '(forgotten)' : ''}',
    ]..sort();
    return 'view=rows:$rows sets:$sets devices:$devs wiped=${hs.isWiped}';
  }

  Future<void> _run(_Device d, String what, Future<String> Function() f) async {
    final sw = Stopwatch()..start();
    try {
      final r = await f();
      _log('$what device=${d.name} ok ms=${sw.elapsedMilliseconds} $r');
    } catch (e) {
      _log('$what device=${d.name} FAILED ms=${sw.elapsedMilliseconds} $e');
      d.state = '$what failed: $e';
    }
    _log('state device=${d.name} key=${await _keyState(d)} ${_view(d)}');
    if (mounted) setState(() {});
  }

  Future<void> _put(_Device d) => _run(d, 'put', () async {
    d.puts++;
    final c = await d.hs!.put('rooms', 'android-${d.puts}', {
      'name': 'Android ${d.name} ${d.puts}',
    });
    await d.hs!.setAdd('groceries', 'eggs-${d.name}');
    return 'outgoing=${c.outgoing.length}';
  });

  Future<void> _sync(_Device d) => _run(d, 'sync', () async {
    final r = await d.hs!.syncWithRelay(d.relay);
    return 'enrolled=${r.enrolled} pulled=${r.pulled} uploaded=${r.uploaded} '
        'epoch=${d.relay.epoch}';
  });

  Future<void> _forget(_Device d) => _run(d, 'forgetSelf', () async {
    final c = await d.hs!.forgetSelf(relay: d.relay);
    d.state = 'WIPED: this device was forgotten';
    return 'wiped=${c.wiped}';
  });

  /// Mean and median of [n] signatures (after a warm-up) per backend, plus the
  /// cold first signature of a fresh SecureStorageSigner (storage read included).
  Future<void> _benchSigners() async {
    const n = 200;
    final msg = Uint8List.fromList(List.generate(120, (i) => i));
    final out = StringBuffer();
    final chosen = await SoftwareSigner.fromSeed(List.filled(32, 7)).backend();
    out.writeln('default backend: ${chosen.name}');
    for (final b in [null, SigningBackend.rust, SigningBackend.dart]) {
      final s = SecureStorageSigner(key: 'hearth_probe.bench', backend: b);
      final cold = Stopwatch()..start();
      await s.sign(msg);
      cold.stop();
      for (var i = 0; i < 20; i++) {
        await s.sign(msg);
      }
      final us = <int>[];
      for (var i = 0; i < n; i++) {
        final sw = Stopwatch()..start();
        await s.sign(msg);
        us.add(sw.elapsedMicroseconds);
      }
      us.sort();
      final mean = us.reduce((a, b) => a + b) / n;
      final line =
          '${b?.name ?? 'default'}: mean ${mean.toStringAsFixed(1)} us, '
          'p50 ${us[n ~/ 2]} us, cold first ${cold.elapsedMicroseconds} us';
      out.writeln(line);
      _log('bench $line');
    }
    await _storage.delete(key: 'hearth_probe.bench');
    setState(() => _bench = out.toString());
  }

  Widget _button(String label, VoidCallback? onPressed) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 4),
    child: SizedBox(
      width: double.infinity,
      height: 48,
      child: FilledButton(onPressed: onPressed, child: Text(label)),
    ),
  );

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      for (final d in _devices) ...[
        const Divider(),
        Text('Device ${d.name} (${d.url}): ${d.state}'),
        Text(_view(d)),
        if (d.hs != null && !d.hs!.isWiped) ...[
          _button('Put ${d.name}', () => _put(d)),
          _button('Sync ${d.name}', () => _sync(d)),
          _button('Forget ${d.name}', () => _forget(d)),
        ],
      ],
      const Divider(),
      _button('Bench signers', _benchSigners),
      if (_bench.isNotEmpty) Text(_bench),
    ],
  );
}
