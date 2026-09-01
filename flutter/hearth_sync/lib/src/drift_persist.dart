import 'package:drift/drift.dart';

import 'persist.dart';

/// The kernel's records in one table of the app's own Drift database:
/// `hearth_records(key BLOB PRIMARY KEY, value BLOB)` (ADR 0010). It needs no
/// generated code, so it works over any [GeneratedDatabase]. Pass
/// [applyTables] to write the app's own rows in the same transaction.
class DriftPersist implements Persist {
  /// Records in [table] of [db].
  DriftPersist(this.db, {this.table = 'hearth_records', this.applyTables})
    : assert(RegExp(r'^[a-z_][a-z0-9_]*$').hasMatch(table));

  /// The app's database.
  final GeneratedDatabase db;

  /// The table name.
  final String table;

  /// Writes the call's changes to the app's tables, inside the transaction.
  final Future<void> Function(RecordBatch batch)? applyTables;

  bool _created = false;

  Future<void> _ensure() async {
    if (_created) return;
    await db.customStatement(
      'CREATE TABLE IF NOT EXISTS $table '
      '(key BLOB NOT NULL PRIMARY KEY, value BLOB NOT NULL) WITHOUT ROWID',
    );
    _created = true;
  }

  @override
  Future<List<StoredRecord>> readAll() async {
    await _ensure();
    final rows = await db
        .customSelect('SELECT key, value FROM $table ORDER BY key')
        .get();
    return [
      for (final r in rows)
        StoredRecord(r.read<Uint8List>('key'), r.read<Uint8List>('value')),
    ];
  }

  @override
  Future<void> apply(RecordBatch batch) async {
    await _ensure();
    await db.transaction(() async {
      if (batch.reset) await db.customStatement('DELETE FROM $table');
      for (final r in batch.records) {
        if (r.value case final v?) {
          await db.customStatement(
            'INSERT OR REPLACE INTO $table (key, value) VALUES (?, ?)',
            [r.key, v],
          );
        } else {
          await db.customStatement('DELETE FROM $table WHERE key = ?', [r.key]);
        }
      }
      await applyTables?.call(batch);
    });
  }

  @override
  Future<void> clear() async {
    await _ensure();
    await db.customStatement('DELETE FROM $table');
  }
}
