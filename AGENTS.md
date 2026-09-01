# AGENTS.md: hearthSync

The OpenHearth sync kernel (Rust), its Flutter package and its Rust relay. Read `README.md`
first, then the ADRs in `docs/adr/`. The design this implements lives in the
workshop's research notes (`2026-09-27-sync-kernel-design.md`) with the
operator's rulings on its questions.

## Map

| Path | What |
|---|---|
| `kernel/src/api.rs` | The bridge-facing surface: `Kernel`, `Step` (sign / done), `Outcome`, sealed sync messages, the replayed flows, the incremental view diff, review entries (ADR 0011) |
| `kernel/src/cbor.rs` | The nesting guard every dCBOR decode goes through (`cbor::decode`; the relay re-exports `nesting_ok`, ADR 0003) |
| `kernel/src/op.rs` | Op format, dCBOR encode/decode, id, signature, `Reject` codes |
| `kernel/src/keys.rs` | Enroll key (HKDF), `DeviceSigner`, enroll/forget authorisations |
| `kernel/src/store.rs` | `OpStore` trait, `MemStore`, index entries |
| `kernel/src/state.rs` | The fold (LWW rows, observed-remove deletes, OR-sets, streams) and canonical state bytes |
| `kernel/src/persist.rs` | Records the app stores for the kernel: keys, versioned values, the journal behind `take_changes`, the write order, `load` and its repairs (ADR 0010) |
| `kernel/src/replica.rs` | Ingest and validity (V1–V6, V8; V4 from the interned `EnrollCache`, V5 and the cut by the clock-bounded `all_in_past` walk), V6 with the shield (`Exclusion`), the one `fold`, the backing rule (`split_at`) and `checkpoint_hash`, authoring, Forget and self-wipe, checkpoint, compaction with kept bodies |
| `kernel/src/schema.rs` | App schemas: collections and merges, V7 (reject a misused declared name, hold an undeclared one), the container view (ADR 0009) |
| `kernel/src/seal.rs` | The XChaCha20-Poly1305 envelope (keys, AAD, synthetic nonce; ADR 0008) |
| `kernel/src/sync.rs` | Hello/Request/Offer, AgePull, snapshots and their verification, adoption (rebase, BaseRebase, Reforget, ForeignReview), WipedPush handover, `reconcile` |
| `kernel/tests/` | Vectors, validity, D1 and merge scenarios, Forget, horizon, seal, schema, persistence, property tests (incl. the horizon and persistence properties); `perf.rs` is ignored by default |
| `kernel/tests/model_cx.rs` | One regression test per Alloy counterexample (ADR 0006), `cx1_…` to `cx8_…` |
| `vectors/` | Independent op vectors (Python, pyca), seal vectors (PyNaCl), and relay conformance vectors from a reference model (`make_relay_vectors.py`) |
| `relay/src/wire.rs` | Relay request parsing, the signed messages, the nesting guard (re-exported from the kernel), client builders |
| `relay/src/relay.rs` | The relay handler: routing, checks in protocol order, rate limits, nonces; takes `now`, so the vectors drive it |
| `relay/src/store.rs`, `http.rs`, `main.rs` | SQLite store and canonical dump; the hyper server; the CLI |
| `relay/tests/` | Conformance (vectors), e2e (kernel replicas through the relay), HTTP and CLI, proptest fuzzing |
| `relay/deploy/` | Containerfile, systemd unit, deploy notes (ADR 0013) |
| `go-relay/` | The Go relay, written from the protocol page alone: strict dCBOR (`internal/dcbor`), strict Ed25519 (`internal/edsig`), handler, bbolt store and HTTP server (`internal/relay`), CLI, deploy kit |
| `go-relay/difftest/` | The differential harness: generated request sequences through both relays, answers and store digests compared every step; `rustdriver/` is the Rust relay behind a pipe (its own cargo workspace) |
| `docs/reference/op-format.md` | The wire format |
| `docs/reference/relay-protocol.md` | The relay protocol, which both relays implement |
| `model/forget.als` | Alloy 6 model of Forget, checkpoints and the horizon (ADRs 0006, 0007: the v0.2 checks end in `…2`); run with the Alloy jar in the git-ignored `.tools/` |
| `flutter/hearth_sync/` | The Flutter package (ADR 0012): `rust/` is the bridge crate (its own workspace and lockfile; `#[frb(mirror)]` of every `api` type, one opaque `HearthKernel`), `rust/src/api/signing.rs` native Ed25519 for the signers (the seed passed per call, never kept), `lib/src/` the Dart API, signers (WebCrypto / Rust / pure-Dart backends), stores, and the relay client (`relay_cbor.dart`, `relay_client.dart`: pure-Dart dCBOR over package:http; `sync_loop.dart`: the round and `SyncLoop`), `lib/src/rust/` the committed generated bindings, `test/` host tests over the Linux `.so` (`relay_e2e_test.dart` starts both relay binaries on localhost), `example/` the demo app and `tool/build_web.sh` |
| `spike/` | The throwaway flutter_rust_bridge spike (not in the workspace) |

## Working here

- Every cargo command goes through the workshop's `heavy.sh` with
  `CARGO_TARGET_DIR=<repo>/target`, one at a time:
  `heavy.sh cargo test -p hearth_sync_kernel`.
- The kernel stays pure: no I/O, clock, randomness or threads (ADR 0001).
- `api` is what the bridge sees: keep it synchronous, plain-typed (no generics,
  lifetimes, callbacks or handles) and doc-commented. `tests/api.rs` drives
  devices only through it and checks the app's mirrored tables against a fresh
  view after every call.
- A validity rule may read only the op or before(u). Anything else must exclude or
  delay, never reject (ADR 0004). V7 reads the app schema, so it rejects only a
  misused *declared* name and holds an undeclared one (ADR 0009); schemas evolve by
  adding names, never by changing or removing one.
- Changing the format means changing `vectors/make_vectors.py` first, regenerating
  `vectors/ops_v1.json` (`uv run vectors/make_vectors.py > vectors/ops_v1.json`),
  and watching the vector tests fail before the Rust change. The same holds for the
  envelope and `vectors/make_seal_vectors.py` (`seal_v1.json`).
- Forget, checkpoints, compaction, snapshots, rebase and reconcile are covered by
  the Alloy model (ADR 0006). The eight fixes were checked only together; change
  one and re-run the model (Alloy jar in `.tools/`, through `heavy.sh`) as well
  as `model_cx.rs` and the horizon property.
- Every change to what `Replica` keeps in memory needs a matching record (or a
  rebuild in `load`) and a journal mark; the persistence property fails otherwise.
- Changing `api` means changing the mirrors in `flutter/hearth_sync/rust/src/api/kernel.rs`,
  re-running the codegen and freezed (the package README has the commands), and
  committing the generated files.
- Changing the relay protocol means changing `docs/reference/relay-protocol.md` and
  `vectors/make_relay_vectors.py` first, regenerating `vectors/relay_v1.json`, and
  watching `relay/tests/conformance.rs` fail. Neither the kernel nor the relay decodes
  anything with dCBOR except through the nesting guard (`cbor::decode` in the kernel,
  `wire::nesting_ok` in the relay): the decoder recurses per level, so deep input
  aborts the process. Build a deployable
  binary with `--profile relay`.
- Both relays answer the same vectors. A protocol change goes to the page and the
  model first, then to `relay/` and `go-relay/`; run the Go tests and the differential
  harness (`go-relay/README.md`) as well as `relay/tests/conformance.rs`. Go commands go
  through `heavy.sh` too, with `GOCACHE`, `GOTMPDIR` and `TMPDIR` off `/tmp`.
- No users yet: no back-compat, no migrations.
