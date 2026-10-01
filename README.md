# hearthSync

The OpenHearth **sync kernel**: one pure Rust crate that every syncing app will
call. It keeps a signed, hash-linked log of operations per household per app and
folds it into app state. Every replica holding the same ops ends up with the same
state, whatever order they arrived in. A delete is always an explicit op, never
inferred from a missing row. The two relays, Rust (`relay/`) and Go (`go-relay/`),
live here too, held to one set of conformance vectors and checked against each other
by a differential harness.

## What v1 (stage 1) is

`kernel/` is the crate `hearth_sync_kernel`.

- **Ops.** Deterministic CBOR, id = SHA-256 of the signed bytes, and a strict
  Ed25519 signature by the device's own key. The household enroll key admits
  devices; it is derived from the 12-word seed that every device stores.
- **Validity.** Bad ops are rejected: forged, unenrolled, non-canonical,
  oversize, clock not after their parents, deleting something they never saw.
  Ops from a clock far in the future are held back until real time catches up.
- **Data types.** Rows with last-writer-wins per field; deletes and Undo, where
  an edit beats a concurrent delete; add-wins sets (grocery lists); append-only
  streams (votes, forecasts).
- **App schemas.** An app registers its tables (typed fields, optionally a
  container field), sets and streams, and its horizon. An op that misuses a
  declared name is rejected; one that uses an undeclared name is held until an
  app version declares it (ADR 0009). The app's view hides everything under a
  deleted container, and Undo brings it back.
- **Persistence without files.** After each call the kernel hands the app a
  changeset of opaque, versioned records (sealed ops, index entries, pending,
  quarantined, held and rejected sets, the base, the wipe flag) to store in its
  own database, in a defined write order, and rebuilds itself from them on the
  next launch (ADR 0010).
- **An app-facing api.** `api::Kernel`: synchronous calls over plain types, the
  device key kept in the platform key store (a write returns the bytes to sign,
  and `finish` takes the signature; adoption replays until it has every
  signature it needs), outcomes carrying the records to store and the row, set
  and stream changes to apply, sealed sync messages, the review list, the wipe
  signal, and NFC at the boundary (ADR 0011).
- **Sync.** Two replicas swap heads and exchange what the other lacks. A replica
  that is new, or that lacks any checkpoint older than the 90-day horizon
  (configurable, or keep full history) which its peer holds, gets a verified
  snapshot first, whether or not the peer has pruned. It then re-applies its
  offline edits to fields nobody else touched, ordered by their original clock so
  they never beat a newer write, and lists the rest for review. Nothing pruned
  vanishes on the way: it is re-issued or listed. A snapshot's checkpoint commits
  to exactly what the snapshot carries, so the peer providing it cannot bend it.
- **Forget this device.** Ops the forgotten device writes after the cut stop
  counting everywhere, except behind a checkpoint that lacked the Forget: a Forget
  cannot reach behind a checkpoint. The forgotten device wipes its keys when the
  Forget reaches it, and at once if it forgets itself; a wiped device hands its
  own ops and Forgets over instead of adopting a snapshot, so a Forget is never
  lost.

It is tested against byte vectors built independently in Python (`vectors/`),
against hostile ops, against the D1 stale-peer scenario ported from
`sanctuary_auth_core` (red there, green here), against the eight counterexamples
the Alloy model of Forget and the horizon found in v0 (`model/`, ADR 0006), and
with property tests over random three-device histories, including one that ages
checkpoints, prunes and returns past the horizon, and one that persists, reloads
and cuts changesets short. Sealing is checked against PyNaCl-built vectors, and
two devices are driven through `api` alone, with the app's mirrored tables
checked against a fresh view after every call. `kernel/tests/perf.rs` (ignored by
default, `--profile perf`) measures 10⁴ and 10⁵ ops; the numbers are in the kernel
v1 stage 1 report.

## What v1 (stage 1) is not

- The kernel has no network code. The `seal` module seals ops, snapshots and
  messages (XChaCha20-Poly1305 under household keys, ADR 0008). Stage 3 adds the
  Rust relay (`relay/`, ADR 0013): data-blind store-and-forward of those sealed ops
  per household channel, checking household and device signatures with the kernel's
  own functions (the D3 fix), with a shared conformance suite, a hardening kit, and
  an end-to-end test of replicas syncing only through it. It has not been deployed,
  nor its image built. Stage 6 adds the Dart client (`RelayClient`, `SyncLoop` in
  the Flutter package), tested over localhost HTTP against both relays; no app
  uses it yet.
- Stage 2 adds the Flutter package `flutter/hearth_sync`. It binds `api`
  through flutter_rust_bridge and keeps the device key in secure storage. It
  stores records through Drift or IndexedDB (see its README and ADR 0012). Its
  web build has run in headless Chromium from a plain static server (no
  COOP/COEP): WebCrypto signing, IndexedDB across a reload, and two browsers
  syncing through both relays. Its release APK has run on an Android 14
  emulator (x86_64): native Rust signing, records in Drift and the key in
  secure storage across an app kill, sync with a host device through both
  relays, and a self-Forget that destroys the key.
- It is not used by any app yet. Each app keeps its current sync until it moves
  onto the kernel. Lullaby moves first.
- The known gaps are listed in `docs/adr/0005-horizon-snapshots-rebase.md` and,
  for Forget and the horizon, in ADR 0006's "Where the implementation departs
  from the model", and in ADR 0007. A sync no longer stalls on an old
  checkpoint the peer cannot build a snapshot at (the peer falls back to its own
  base), and a device's ops are pruned only once another device's checkpoint
  covers them, so a one-device household never prunes (ADR 0007 explains why
  the shield ruling forbids more).

## Running

```sh
# From the repo root.
CARGO_TARGET_DIR=target cargo test -p hearth_sync_kernel
# Performance (ignored by default; HS_PERF_N sets the op count, 10^4 by default)
CARGO_TARGET_DIR=target cargo test --profile perf -p hearth_sync_kernel --test perf -- --ignored --nocapture
# Regenerate the shared vectors (needs uv); ops first, the seal vectors read them
uv run vectors/make_vectors.py > vectors/ops_v1.json
uv run vectors/make_seal_vectors.py > vectors/seal_v1.json
# The relay: conformance, end to end, HTTP and CLI, fuzzing (see relay/README.md)
CARGO_TARGET_DIR=target cargo test -p hearth_sync_relay
uv run vectors/make_relay_vectors.py > vectors/relay_v1.json
```

## Layout

| Path | What |
|---|---|
| `kernel/` | The kernel crate and its tests |
| `relay/` | The Rust relay (`hearth-relay`), its tests and its deploy kit |
| `vectors/` | Shared op, seal and relay conformance vectors and the Python scripts that build them |
| `docs/adr/` | Decisions: pure kernel, keys and Forget, format and fold, validity and clocks, horizon, the Forget and horizon model, the committed split and fallback pull, sealing, app schemas, persistence, the app api, the Dart boundary, the relay |
| `model/` | The Alloy 6 model of Forget, checkpoints and the horizon (ADR 0006) |
| `docs/reference/op-format.md` | The wire format |
| `docs/reference/relay-protocol.md` | The relay protocol |
| `flutter/hearth_sync/` | The Flutter package: the frb bridge crate (`rust/`), the Dart API, signers and record stores, host tests, and an example app |
| `spike/` | The earlier flutter_rust_bridge spike (throwaway; kept for its measurements) |

The design decisions are recorded in `docs/adr/`.

## License

MIT. See [LICENSE](LICENSE).
