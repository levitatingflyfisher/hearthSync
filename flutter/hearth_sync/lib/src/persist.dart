import 'dart:typed_data';

import 'changes.dart';
import 'values.dart';

/// One record the kernel asks the app to store (ADR 0010): an opaque key and
/// either a value to write or `null` to delete it.
class StoredRecord {
  /// A record.
  const StoredRecord(this.key, this.value);

  /// The key: a tag byte, then an op id where the record belongs to one.
  final Uint8List key;

  /// The value to store, or `null` to delete the record.
  final Uint8List? value;
}

/// What one kernel call asks the app to store.
class RecordBatch {
  /// A batch.
  const RecordBatch({
    required this.reset,
    required this.records,
    required this.changes,
  });

  /// Delete every stored record before writing [records] (a snapshot replaced the
  /// whole log); the whole batch must then be atomic.
  final bool reset;

  /// Records to write or delete, in this order.
  final List<StoredRecord> records;

  /// The same call's changes to the app's own tables, for a store that writes
  /// them in the same transaction.
  final Changes changes;
}

/// Where the kernel's records live. The kernel never touches storage (ADR 0001);
/// after each call the app writes the batch, in one transaction, in the order
/// given, before anything the call produced is sent (ADR 0010).
abstract interface class Persist {
  /// Every stored record (values only), for [HearthSync.open].
  Future<List<StoredRecord>> readAll();

  /// Apply one call's batch atomically: if [RecordBatch.reset], delete every
  /// record first; then write or delete each record in order.
  Future<void> apply(RecordBatch batch);

  /// Delete every record (after a wiped device has handed its ops on).
  Future<void> clear();
}

/// Records in memory: for tests, and for an app that keeps no history.
class MemoryPersist implements Persist {
  final Map<String, Uint8List> _records = {};

  /// Called inside [apply], after the records, as a store would in its
  /// transaction.
  void Function(RecordBatch batch)? onApply;

  /// How many records are stored.
  int get length => _records.length;

  @override
  Future<List<StoredRecord>> readAll() async => [
    for (final e in _records.entries) StoredRecord(unhex(e.key), e.value),
  ];

  @override
  Future<void> apply(RecordBatch batch) async {
    if (batch.reset) _records.clear();
    for (final r in batch.records) {
      final k = hex(r.key);
      if (r.value case final v?) {
        _records[k] = v;
      } else {
        _records.remove(k);
      }
    }
    onApply?.call(batch);
  }

  @override
  Future<void> clear() async => _records.clear();
}
