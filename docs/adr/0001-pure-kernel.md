# ADR 0001: The kernel is a pure crate

Status: accepted (v0, 2026-09-28)

## Context

Decision 3 puts one Rust sync kernel under every syncing app, through
flutter_rust_bridge on Android and as single-threaded WASM in the PWAs (the
spike showed the default threaded web build needs COOP/COEP, which GitHub Pages
cannot send). Dart keeps persistence (Drift, OPFS) and the platform key stores.

## Decision

`hearth_sync_kernel` does no I/O, reads no clock and draws no randomness.

- Time arrives as `now` (Unix millis) on every call that needs it.
- Secrets arrive as bytes: the 64-byte household seed, and the 32 random bytes
  of a new device key. Signing goes through the `DeviceSigner` trait so v1 can
  keep the device key in the platform keystore (`prepare` returns the bytes to
  sign, `finish` takes the signature).
- Storage is the `OpStore` trait. v0 ships `MemStore`; v1 backs it with Drift.
- Messages between replicas are plain structs (`Hello`, `Request`, `Offer`,
  `Snapshot`). Sealing them (XChaCha20-Poly1305) and moving them over the LAN
  or a relay is v1.

## Consequences

- Every test is deterministic: property tests replay from a seed, and the
  same inputs give byte-identical state.
- Every call is synchronous, which fits the spike's rule that web-facing calls
  be `#[frb(sync)]`. A long ingest on the web main thread will block the UI, so
  v1 batches it.
- The bridge API (v1) is a thin layer over `Replica`: bytes in, reports out.
