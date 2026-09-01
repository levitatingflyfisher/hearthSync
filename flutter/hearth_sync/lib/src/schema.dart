import 'rust/api/kernel.dart' as k;

/// The type of a field, set element or stream record (ADR 0009).
enum SyncType {
  /// A String.
  text,

  /// An int (64-bit).
  int,

  /// A bool.
  bool,

  /// Bytes.
  bytes,

  /// Any of the above.
  any;

  k.ValueType get _kernel => switch (this) {
    SyncType.text => k.ValueType.text,
    SyncType.int => k.ValueType.int,
    SyncType.bool => k.ValueType.bool,
    SyncType.bytes => k.ValueType.bytes,
    SyncType.any => k.ValueType.any,
  };
}

/// A field of a table.
class SyncField {
  /// A field [name] of [type]; [nullable] lets a write clear it with `null`.
  const SyncField(this.name, this.type, {this.nullable = true});

  /// The field name (1–128 bytes).
  final String name;

  /// Its type.
  final SyncType type;

  /// Whether `null` clears it.
  final bool nullable;
}

/// A collection an app declares. Names are for good: never change a declared
/// name's kind or type, and never remove one (ADR 0009).
sealed class SyncCollection {
  const SyncCollection(this.name);

  /// The collection's name.
  final String name;
}

/// Rows of fields, last writer wins per field; deletes are observed-remove and
/// Undo restores. [containerField] names a text field holding the id of a row in
/// [containerTable]: deleting that row hides this one.
class SyncTable extends SyncCollection {
  /// A table.
  const SyncTable(
    super.name,
    this.fields, {
    this.containerField,
    this.containerTable,
  }) : assert((containerField == null) == (containerTable == null));

  /// Its fields.
  final List<SyncField> fields;

  /// The field holding the container row's id, if any.
  final String? containerField;

  /// The table the container row is in.
  final String? containerTable;
}

/// An add-wins set (a grocery list).
class SyncSet extends SyncCollection {
  /// A set of [element]s.
  const SyncSet(super.name, this.element);

  /// The element type.
  final SyncType element;
}

/// An append-only stream (votes, forecasts, a log).
class SyncStream extends SyncCollection {
  /// A stream of [record]s.
  const SyncStream(super.name, this.record);

  /// The record type.
  final SyncType record;
}

/// An app's schema: its collections and its horizon.
class SyncSchema {
  /// A schema. Op bodies behind a checkpoint older than [horizon] may be pruned,
  /// unless [keepFullHistory].
  const SyncSchema(
    this.collections, {
    this.horizon = const Duration(days: 90),
    this.keepFullHistory = false,
  });

  /// Every collection.
  final List<SyncCollection> collections;

  /// The horizon (every replica of the app must use the same one).
  final Duration horizon;

  /// Never prune (journals, Reckon).
  final bool keepFullHistory;

  /// The kernel's form.
  k.Schema toKernel() => k.Schema(
    collections: [
      for (final c in collections)
        k.Collection(
          name: c.name,
          merge: switch (c) {
            SyncTable t => k.Merge.lww(
              fields: [
                for (final f in t.fields)
                  k.FieldDef(
                    name: f.name,
                    ty: f.type._kernel,
                    nullable: f.nullable,
                  ),
              ],
              container: t.containerField == null
                  ? null
                  : k.ContainerDef(
                      field: t.containerField!,
                      table: t.containerTable!,
                    ),
            ),
            SyncSet s => k.Merge.addWinsSet(element: s.element._kernel),
            SyncStream s => k.Merge.appendOnly(record: s.record._kernel),
          },
        ),
    ],
    horizonMs: BigInt.from(horizon.inMilliseconds),
    keepFullHistory: keepFullHistory,
  );
}
