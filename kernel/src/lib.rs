//! # hearth_sync_kernel
//!
//! The OpenHearth sync kernel, v1 (stage 1): a signed, hash-linked operation log that every
//! syncing app folds into the same state, whatever order the ops arrive in.
//!
//! - [`api`]: the bridge-facing surface: synchronous, plain types, the device key
//!   kept platform-side (ADR 0011).
//! - [`op`]: the op format (dCBOR, SHA-256 id, Ed25519 signature) and the
//!   deterministic checks on a single op (V1, V2).
//! - [`cbor`]: the nesting guard every dCBOR decode runs first (hostile input cannot
//!   exhaust the stack).
//! - [`keys`]: the household enroll key (HKDF from the 64-byte seed) and device signers.
//! - [`store`]: the [`store::OpStore`] trait and its in-memory implementation.
//! - [`state`]: the fold: per-field LWW rows with observed-remove deletes, add-wins
//!   OR-sets, append streams.
//! - [`persist`]: the opaque, versioned records the app stores for the kernel, their
//!   write order, and loading them back (ADR 0010).
//! - [`replica`]: ingest with the validity rules (V3–V6, V8), authoring, Forget and
//!   self-wipe, checkpoints and the horizon.
//! - [`schema`]: app schemas (V7: reject a declared name misused, hold an undeclared
//!   one) and the app's view, where a deleted container hides its children.
//! - [`seal`]: the XChaCha20-Poly1305 envelope every op, snapshot and message leaves
//!   the kernel in (ADR 0008).
//! - [`sync`]: head exchange and reconciliation between two replicas, including the
//!   past-horizon snapshot path and its rebase.
//!
//! The kernel is pure: no I/O, no clock reads, no randomness. Time comes in as
//! `now` (Unix millis), secrets come in as bytes. See `docs/adr/` for why.

pub mod api;
pub mod cbor;
pub mod keys;
pub mod op;
pub mod persist;
pub mod replica;
pub mod schema;
pub mod seal;
pub mod state;
pub mod store;
pub mod sync;

/// SHA-256 op id: the hash of the op's canonical signed bytes.
pub type Id = [u8; 32];
/// A device id is its Ed25519 public key.
pub type DeviceId = [u8; 32];

/// V1: an op's canonical bytes may not exceed this (design §3.1).
pub const MAX_OP_BYTES: usize = 64 * 1024;
/// V1: at most this many parents (heads) per op. Heads are "typically" 1–3.
pub const MAX_PARENTS: usize = 64;
/// V1: table, row, field, set, stream and label names are 1..=this many UTF-8 bytes.
pub const MAX_NAME_BYTES: usize = 128;
/// V8: an op stamped further than this into the local future is quarantined, not
/// rejected, until local time catches up (design §3.2, ADR 0004).
pub const MAX_FUTURE_SKEW_MS: u64 = 10 * 60 * 1000;
/// Default horizon: op bodies behind a checkpoint older than this may be pruned.
pub const DEFAULT_HORIZON_MS: u64 = 90 * 24 * 60 * 60 * 1000;

pub fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).into()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
