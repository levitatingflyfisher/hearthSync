import 'dart:typed_data';

/// A position in a relay log: an uploader and a seq in its log.
class RelayCursor {
  /// A cursor.
  const RelayCursor(this.device, this.seq);

  /// The uploader's public key.
  final Uint8List device;

  /// The last seq of its log taken into account.
  final int seq;
}

/// Where this device stands with the relay.
class RelayState {
  /// A relay state.
  const RelayState(this.nextSeq, this.cursors, {this.generation = 0});

  /// The relay channel's generation these positions belong to (0 until the
  /// relay has named one). A different one in an enroll or pull answer means
  /// the channel was made again: pass it to `HearthSync.relayGeneration`.
  final int generation;

  /// The seq the next upload to this device's own log starts at.
  final int nextSeq;

  /// How far each log was pulled and ingested.
  final List<RelayCursor> cursors;
}

/// The fields of the relay's `enroll` verb for one device.
class RelayEnrollment {
  /// An enrolment.
  const RelayEnrollment(this.household, this.device, this.label, this.auth);

  /// The household's enroll public key.
  final Uint8List household;

  /// The enrolled device.
  final Uint8List device;

  /// Its label (the relay sees it: keep labels unrevealing).
  final String label;

  /// The household's signature over the enrolment.
  final Uint8List auth;
}

/// A Forget record the relay's `forget` verb takes, handed out only once its
/// Forget op is on the relay.
class RelayForget {
  /// A Forget record.
  const RelayForget(this.forget, this.target, this.cut, this.auth, this.cutSeq);

  /// The Forget op's id: pass it to `HearthSync.relayForgetPosted`.
  final Uint8List forget;

  /// The forgotten device.
  final Uint8List target;

  /// The target's op ids the Forget keeps.
  final List<Uint8List> cut;

  /// The household's signature over the Forget.
  final Uint8List auth;

  /// The last seq of the target's relay log that the cut covers.
  final int cutSeq;
}

/// This device's base as a sealed snapshot, with what it covers on the relay.
class RelaySnapshot {
  /// A snapshot.
  const RelaySnapshot(this.sealed, this.covers);

  /// The sealed snapshot.
  final Uint8List sealed;

  /// Log entries behind the snapshot's checkpoint (empty: prune nothing).
  final List<RelayCursor> covers;
}
