import 'dart:typed_data';

import 'package:idb_shim/idb.dart';

import 'persist.dart';
import 'values.dart';

/// The kernel's records in one IndexedDB object store, for the web (ADR 0010).
/// Keys are stored as lowercase hex, which sorts like the bytes. Pass
/// `idbFactoryBrowser` (from `package:idb_shim/idb_browser.dart`) in the app and
/// `idbFactoryMemory` in tests.
///
/// Every write of a batch is queued in one readwrite transaction and only the
/// transaction's completion is awaited, so the browser cannot commit it early.
/// [applyTables] may queue writes to [tableStores] in the same transaction; it
/// must not await anything but those requests.
class IdbPersist implements Persist {
  /// Records in [store] of database [dbName] from [factory].
  IdbPersist(
    this.factory, {
    this.dbName = 'hearth_sync',
    this.store = 'records',
    this.tableStores = const [],
    this.version = 1,
    this.onUpgrade,
    this.applyTables,
  });

  /// Where databases come from.
  final IdbFactory factory;

  /// The database name.
  final String dbName;

  /// The object store for records.
  final String store;

  /// The app's own object stores, written in the same transaction.
  final List<String> tableStores;

  /// The database version (raise it when the app adds stores).
  final int version;

  /// Creates the app's own stores on upgrade.
  final void Function(VersionChangeEvent e)? onUpgrade;

  /// Queues the call's changes to the app's stores inside the transaction.
  final void Function(Transaction txn, RecordBatch batch)? applyTables;

  Database? _db;

  Future<Database> _open() async => _db ??= await factory.open(
    dbName,
    version: version,
    onUpgradeNeeded: (e) {
      final db = e.database;
      if (!db.objectStoreNames.contains(store)) db.createObjectStore(store);
      onUpgrade?.call(e);
    },
  );

  @override
  Future<List<StoredRecord>> readAll() async {
    final db = await _open();
    final txn = db.transaction(store, idbModeReadOnly);
    final out = <StoredRecord>[];
    final cursor = txn.objectStore(store).openCursor(autoAdvance: true);
    await cursor.forEach((c) {
      out.add(StoredRecord(unhex(c.key as String), _bytes(c.value)));
    });
    await txn.completed;
    return out;
  }

  static Uint8List _bytes(Object? v) => switch (v) {
    Uint8List b => b,
    List<Object?> l => Uint8List.fromList(l.cast<int>()),
    _ => throw StateError('record value is not bytes'),
  };

  @override
  Future<void> apply(RecordBatch batch) async {
    final db = await _open();
    final txn = db.transactionList([store, ...tableStores], idbModeReadWrite);
    final os = txn.objectStore(store);
    if (batch.reset) os.clear();
    for (final r in batch.records) {
      if (r.value case final v?) {
        os.put(v, hex(r.key));
      } else {
        os.delete(hex(r.key));
      }
    }
    applyTables?.call(txn, batch);
    await txn.completed;
  }

  @override
  Future<void> clear() async {
    final db = await _open();
    final txn = db.transaction(store, idbModeReadWrite);
    txn.objectStore(store).clear();
    await txn.completed;
  }

  /// Close the database.
  void close() {
    _db?.close();
    _db = null;
  }
}
