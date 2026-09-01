import 'package:drift/drift.dart';
import 'package:drift/native.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hearth_sync/hearth_sync.dart';
import 'package:idb_shim/idb_client_memory.dart';

import 'support.dart';

/// An app database with no tables of its own: DriftPersist needs no codegen.
class AppDb extends GeneratedDatabase {
  AppDb(super.e);
  @override
  Iterable<TableInfo<Table, dynamic>> get allTables => const [];
  @override
  int get schemaVersion => 1;
}

Map<String, String> dump(List<StoredRecord> rs) => {
  for (final r in rs) hex(r.key): hex(r.value!),
};

/// Drive one device through writes, a sync and a checkpoint, then reopen it from
/// [persist] alone and check it is the same replica.
Future<void> restartRoundTrip(Persist Function() persistFor) async {
  final clock = TestClock();
  final persist = persistFor();
  final a = await device(1, clock, persist: persist);
  final b = await device(2, clock);
  clock.now += 1;
  await a.hs.put('rooms', 'den', {'name': 'Den'});
  await a.hs.put('items', 'i1', {'name': 'Lamp', 'value': 40, 'box': null});
  await a.hs.setAdd('tags', 'fragile');
  await b.hs.append('log', Uint8List.fromList([1, 2, 3]));
  clock.now += 1;
  await a.hs.syncWith(b.hs);
  await a.hs.checkpoint();
  a.check();
  final before = a.mirror.snapshot;
  final heads = a.hs.devices().length;
  await a.hs.close();

  // Restart: a new store over the same database, a new HearthSync, the same key.
  clock.now += 1;
  final reopened = persistFor();
  final stored = await reopened.readAll();
  expect(dump(stored), dump(await persist.readAll()));
  expect(stored.length, greaterThan(5));
  final again = Device(1, reopened, clock);
  await again.open();
  expect(
    again.hs.opened.single.replaceView,
    isFalse,
    reason: 'same schema, and already enrolled',
  );
  expect(Mirror.of(again.hs).snapshot, before);
  expect(again.hs.devices().length, heads);
  expect(
    dump(await reopened.readAll()),
    dump(stored),
    reason: 'a clean reopen repairs nothing',
  );

  // It carries on: writes and syncs as before the restart.
  again.mirror.apply(
    Changes(
      rows: again.hs.view().rows,
      sets: again.hs.view().sets,
      streams: again.hs.view().streams,
    ),
  );
  again.watch();
  await again.hs.put('rooms', 'den', {'name': 'Den (renamed)'});
  clock.now += 1;
  await b.hs.syncWith(again.hs);
  expect(b.mirror.rows['rooms/den'], {'name': 'Den (renamed)'});
  again.check();
  expect(again.mirror.snapshot, b.mirror.snapshot);
}

void main() {
  setUpAll(initBridge);
  // Each test opens its own in-memory database on purpose.
  driftRuntimeOptions.dontWarnAboutMultipleDatabases = true;

  test('a device reopens from its Drift records and carries on', () async {
    final db = AppDb(NativeDatabase.memory());
    await restartRoundTrip(() => DriftPersist(db));
    await db.close();
  });

  test('a device reopens from its IndexedDB records and carries on', () async {
    final factory = newIdbFactoryMemory();
    await restartRoundTrip(() => IdbPersist(factory));
  });

  for (final (name, make) in <(String, Future<Persist> Function())>[
    ('Drift', () async => DriftPersist(AppDb(NativeDatabase.memory()))),
    ('IndexedDB', () async => IdbPersist(newIdbFactoryMemory())),
    ('memory', () async => MemoryPersist()),
  ]) {
    test(
      '$name applies a batch in order, deletes, resets and clears',
      () async {
        final p = await make();
        Uint8List b(List<int> x) => Uint8List.fromList(x);
        const none = Changes();
        await p.apply(
          RecordBatch(
            reset: false,
            changes: none,
            records: [
              StoredRecord(b([0x10, 1]), b([1])),
              StoredRecord(b([0x10, 2]), b([2])),
              StoredRecord(
                b([0x10, 1]),
                b([3]),
              ), // later wins: written in order
              StoredRecord(b([0x01]), b([0, 255])),
            ],
          ),
        );
        expect(dump(await p.readAll()), {
          '1001': '03',
          '1002': '02',
          '01': '00ff',
        });
        await p.apply(
          RecordBatch(
            reset: false,
            changes: none,
            records: [
              StoredRecord(b([0x10, 2]), null),
            ],
          ),
        );
        expect(dump(await p.readAll()), {'1001': '03', '01': '00ff'});
        await p.apply(
          RecordBatch(
            reset: true,
            changes: none,
            records: [
              StoredRecord(b([0x20]), b([9])),
            ],
          ),
        );
        expect(dump(await p.readAll()), {
          '20': '09',
        }, reason: 'a reset replaces everything');
        await p.clear();
        expect(await p.readAll(), isEmpty);
      },
    );
  }

  test(
    'Drift writes the app tables in the same transaction as the records',
    () async {
      final db = AppDb(NativeDatabase.memory());
      await db.customStatement(
        'CREATE TABLE rooms (id TEXT PRIMARY KEY, name TEXT)',
      );
      final p = DriftPersist(
        db,
        applyTables: (batch) async {
          for (final r in batch.changes.rows.where((r) => r.table == 'rooms')) {
            if (r.fields['name'] == 'boom') {
              throw StateError('app table write failed');
            }
            await db.customStatement(
              'INSERT OR REPLACE INTO rooms VALUES (?, ?)',
              [r.row, r.fields['name']],
            );
          }
        },
      );
      final clock = TestClock();
      final a = await device(1, clock, persist: p);
      await a.hs.put('rooms', 'den', {'name': 'Den'});
      final n = (await p.readAll()).length;
      await expectLater(
        a.hs.put('rooms', 'den', {'name': 'boom'}),
        throwsStateError,
      );
      expect(
        (await p.readAll()).length,
        n,
        reason: 'the records rolled back with the app table',
      );
      final rows = await db.customSelect('SELECT name FROM rooms').get();
      expect(rows.single.read<String>('name'), 'Den');
      await db.close();
    },
  );
}
