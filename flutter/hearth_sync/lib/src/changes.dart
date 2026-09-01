import 'dart:typed_data';

import 'rust/api/kernel.dart' as k;
import 'values.dart';

/// A row as the app should now show it. When [visible] is false, hide it
/// (deleted, or under a deleted container) and [fields] is empty.
class RowUpdate {
  /// A row update.
  const RowUpdate(this.table, this.row, this.visible, this.fields);

  /// The table.
  final String table;

  /// The row id.
  final String row;

  /// Show (with [fields]) or hide.
  final bool visible;

  /// Every declared field the row has.
  final Map<String, Object?> fields;

  @override
  String toString() => 'RowUpdate($table/$row, ${visible ? fields : 'hidden'})';
}

/// A set element that appeared or went.
class SetUpdate {
  /// A set update.
  const SetUpdate(this.set, this.element, this.present);

  /// The set.
  final String set;

  /// The element.
  final Object? element;

  /// Whether it is now in the set.
  final bool present;
}

/// A stream record that appeared, or left the fold (its author was forgotten).
class StreamUpdate {
  /// A stream update.
  const StreamUpdate({
    required this.stream,
    required this.id,
    required this.millis,
    required this.counter,
    required this.device,
    required this.record,
    required this.present,
  });

  /// The stream.
  final String stream;

  /// The record's op id; streams are ordered by (millis, counter, id).
  final Uint8List id;

  /// The record's clock, Unix millis.
  final int millis;

  /// The counter within the millisecond.
  final int counter;

  /// The device that appended it.
  final Uint8List device;

  /// The record.
  final Object? record;

  /// Whether it is in the stream.
  final bool present;
}

/// An op this device wrote, sealed for the relay.
class SealedOp {
  /// A sealed op.
  const SealedOp(this.id, this.sealed);

  /// The op id (32 bytes), also in clear on the envelope.
  final Uint8List id;

  /// The envelope (ADR 0008).
  final Uint8List sealed;
}

/// An op refused for good, with a stable code (`bad_seal` for an envelope that
/// did not open).
class Rejected {
  /// A rejection.
  const Rejected(this.id, this.code);

  /// The op id.
  final Uint8List id;

  /// Why.
  final String code;
}

/// Something a rebase did not re-apply on its own, for a "review these edits"
/// list. Which fields are set depends on [kind]: `field`, `row_deleted`, `op`,
/// `foreign`, `lost` or `pruned` (see the kernel's `ReviewEntry`).
class ReviewItem {
  /// A review item.
  const ReviewItem({
    required this.key,
    required this.kind,
    required this.op,
    required this.opKind,
    this.device,
    this.table,
    this.row,
    this.field,
    this.mine,
    this.current,
    this.set,
    this.stream,
    this.enrolled,
  });

  /// Pass to [HearthSync.dismissReview].
  final Uint8List key;

  /// What kind of entry.
  final String kind;

  /// The op it is about.
  final Uint8List op;

  /// That op's kind (`put`, `delete`, ...).
  final String opKind;

  /// The op's author, when it is another device.
  final Uint8List? device;

  /// The table, for a row op.
  final String? table;

  /// The row, for a row op.
  final String? row;

  /// The field, for a field entry.
  final String? field;

  /// The value the op wrote.
  final Object? mine;

  /// What the field holds now, for a `field` entry.
  final Object? current;

  /// The set, for a set op.
  final String? set;

  /// The stream, for an append.
  final String? stream;

  /// The enrolled device, for a pruned Enroll.
  final Uint8List? enrolled;

  /// From the bridge.
  factory ReviewItem.of(k.ReviewEntry e) => ReviewItem(
    key: e.key,
    kind: e.kind,
    op: e.op,
    opKind: e.opKind,
    device: e.device,
    table: e.table,
    row: e.row,
    field: e.field,
    mine: e.mine == null ? null : fromKernel(e.mine!),
    current: e.current == null ? null : fromKernel(e.current!),
    set: e.set_,
    stream: e.stream,
    enrolled: e.enrolled,
  );
}

/// What one completed call did, for the app's tables and for the relay. The
/// records are already stored when the app sees this.
class Changes {
  /// Changes.
  const Changes({
    this.rows = const [],
    this.sets = const [],
    this.streams = const [],
    this.replaceView = false,
    this.outgoing = const [],
    this.delivered = const [],
    this.pending = const [],
    this.held = const [],
    this.quarantined = const [],
    this.excluded = const [],
    this.rejected = const [],
    this.wiped = false,
    this.reviewAdded = const [],
    this.reissued = 0,
  });

  /// Rows to upsert or hide.
  final List<RowUpdate> rows;

  /// Set elements that appeared or went.
  final List<SetUpdate> sets;

  /// Stream records that appeared or went.
  final List<StreamUpdate> streams;

  /// [rows], [sets] and [streams] are the whole view: replace the app's tables.
  final bool replaceView;

  /// Ops this device wrote, sealed, for the relay.
  final List<SealedOp> outgoing;

  /// Ops delivered from elsewhere.
  final List<Uint8List> delivered;

  /// Ops that had to wait for a parent at some point in the call, including any
  /// admitted later in the same batch. For what still waits, read
  /// `HearthSync.status().pending`.
  final List<Uint8List> pending;

  /// Using names this app version does not declare (retried after an upgrade).
  final List<Uint8List> held;

  /// Too far in the future (retried as time passes).
  final List<Uint8List> quarantined;

  /// Delivered but left out of the fold (their author was forgotten).
  final List<Uint8List> excluded;

  /// Refused for good.
  final List<Rejected> rejected;

  /// This device was forgotten: its key is destroyed. Delete the words; keep the
  /// records until [HearthSync.wipedHandover] has reached the relay.
  final bool wiped;

  /// Entries this call added to the review list.
  final List<ReviewItem> reviewAdded;

  /// How many ops a rebase re-issued.
  final int reissued;

  /// From the bridge.
  factory Changes.of(k.Outcome o) => Changes(
    rows: [for (final r in o.rows) rowUpdate(r)],
    sets: [
      for (final s in o.sets)
        SetUpdate(s.set_, fromKernel(s.element), s.present),
    ],
    streams: [for (final s in o.streams) streamUpdate(s)],
    replaceView: o.replaceView,
    outgoing: [for (final s in o.outgoing) SealedOp(s.id, s.sealed)],
    delivered: o.delivered,
    pending: o.pending,
    held: o.held,
    quarantined: o.quarantined,
    excluded: o.excluded,
    rejected: [for (final r in o.rejected) Rejected(r.id, r.code)],
    wiped: o.wiped,
    reviewAdded: [for (final e in o.reviewAdded) ReviewItem.of(e)],
    reissued: o.reissued.length,
  );
}

/// A row change from the bridge.
RowUpdate rowUpdate(k.RowChange r) => RowUpdate(r.table, r.row, r.visible, {
  for (final f in r.fields) f.name: fromKernel(f.value),
});

/// A stream change from the bridge.
StreamUpdate streamUpdate(k.StreamChange s) => StreamUpdate(
  stream: s.stream,
  id: s.id,
  millis: s.millis.toInt(),
  counter: s.counter,
  device: s.device,
  record: fromKernel(s.record),
  present: s.present,
);
