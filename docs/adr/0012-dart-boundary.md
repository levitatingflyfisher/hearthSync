# ADR 0012: The Dart boundary

Status: accepted (kernel v1 stage 2, 2026-09-30)

## Context

ADR 0011 gave apps a synchronous, plain-typed `api` built so that
flutter_rust_bridge could take it as it stands. Stage 2 puts it in Flutter's
hands. It needs:

- a Dart package over that api;
- the device key kept on the platform side (ruling Q6: software secure storage
  and pure-Dart Ed25519 are fine for v1; stage 3b signs natively, see
  "Signing");
- record stores for native and web (ADR 0010);
- release profiles that do not make Ed25519 25× slower;
- a way for a wiped device to forward its own ops after a restart.

The spike fixed two things. The web build is a stable, single-threaded WASM
built with wasm-pack, never frb's threaded `build-web`. Every call is
`#[frb(sync)]`.

## Decision

**Layout.** `flutter/hearth_sync/` is a Flutter FFI plugin made from frb's
plugin template, with cargokit building Android. It holds:

- `rust/`, the crate `hearth_sync_bridge`, which depends on the kernel by path.
  It has an empty `[workspace]` of its own, so its `Cargo.lock` and release
  profile belong to it. The root workspace, which the relays also use, is not
  touched.
- the Dart API in `lib/src/`;
- the generated bindings in `lib/src/rust/`, committed with their
  `*.freezed.dart`. Apps take the package by path and never run the codegen.

**The bridge is mirrors plus one handle.** Every plain type in `api` is
declared again as `#[frb(mirror(T))]`, so the generated code uses the kernel's
own types. There is no conversion layer that could drift from them. The one
opaque type, `HearthKernel`, wraps `Kernel` and forwards each method unchanged.
Everything is `#[frb(sync)]`. frb reads a type named `Value` as
`serde_json::Value`, so the kernel's `Value` crosses as `KernelValue`, the same
type under another name. frb emits freezed classes for enums with data, so
freezed and build_runner are dev dependencies. build_runner must run with
`--force-jit`, because sqlite3's build hooks stop its AOT mode.

**The Dart API wraps the generated layer.** The generated layer is not
exported. `HearthSync` takes and returns plain Dart:

- values are `null`, `bool`, `int`, `String` or `Uint8List`;
- times come from an injected `Clock`;
- `SyncSchema` holds `SyncTable`, `SyncSet` and `SyncStream`;
- results are `Changes`, holding `RowUpdate`, `SetUpdate`, `StreamUpdate` and
  `SealedOp`;
- errors are `HearthSyncException` with the kernel's stable codes.

This hides the web/native integer split (`PlatformInt64` is an `int` on native
and a `BigInt` on the web) and frb's naming.

**One queue.** Every call runs through a single Dart future chain. A call:

1. collects its signatures (sign, finish, and repeat);
2. stores its records with `Persist.apply`;
3. destroys the key if the outcome says the device was wiped (a self-Forget
   through the relay holds it for one round; see "The relay round");
4. only then returns, or emits on `changes`.

So no ingest can land while a write waits for its signature (the kernel would
refuse it with `awaiting_signature`). Changesets never interleave, and nothing
a call produced leaves before its records are stored (ADR 0010). A signer that
throws, or a signature the kernel refuses, fails that call and abandons the
kernel's flow, so the next call can start.

**Keys.**

- `Signer` has three methods: `publicKey`, `sign` and `destroy`.
- `SecureStorageSigner` keeps a random 32-byte Ed25519 seed in
  flutter_secure_storage. After `destroy` it refuses to sign rather than
  quietly making a new key. How it signs is below ("Signing").
- The kernel checks every signature against the device key (ADR 0011), so a
  wrong signer shows up as `bad_signature` on that call, never as a junk op,
  and the next call proceeds.

**Signing (kernel v1 stage 3b).** Pure-Dart Ed25519 cost about 4 ms per
signature on the host VM, and every write and every rebase step pays one. The
signers now pick a `SigningBackend`, fastest first:

1. `webCrypto` on the web: the seed is imported once as a non-extractable
   PKCS#8 key (RFC 8410 wrapping) and the browser signs (Chrome 137, Firefox 129,
   Safari 17). Feature-detected: if the import rejects, the next one is used.
2. `rust` everywhere else: `api::signing::ed25519_sign(seed, message)` in the
   bridge crate, over the kernel's own `keys::SoftSigner` (no second Ed25519).
   It is native code on the platforms the plugin builds (Android and Linux
   today; iOS and other desktops once the plugin declares them), and WASM on the
   web.
3. `dart` (package:cryptography) only where neither is available, e.g. before
   `HearthSync.init` has loaded the bridge.

The key stays platform-side: it lives in secure storage, and the Dart signer
passes the seed with each message to a free function that keeps nothing (the
seed is zeroised on the way out). `Kernel` still never sees a device key
(ADR 0011).

- **Not `cryptography_flutter`, as the dispatch proposed.** In 2.3.4,
  `FlutterEd25519.isSupportedPlatform` is `FlutterCryptography.isPluginPresent &&
  isCupertino` (`lib/src/flutter/flutter_ed25519.dart`): native Ed25519 on iOS and
  macOS only. On Android, Linux and Windows it falls back to `DartEd25519`, so it
  would have changed nothing on this package's platforms, and Apple's CryptoKit
  signs non-deterministically besides.
- **Measured on the host** (laptop x86_64, `test/bench_test.dart`, 2,000
  signatures of 200 bytes): the Rust bridge 96 µs per signature, pure Dart
  3,674 µs (38x); a write (prepare, sign, finish, store) 697 µs, from 4,525 µs.
- `test/signer_test.dart` checks RFC 8032's vectors byte for byte on every
  backend the host reaches, RFC 8410's PKCS#8 example, and that the kernel
  accepts what each backend signs. WebCrypto has not run anywhere yet: only a
  browser has it, and flutter test runs on the VM. The web bundle builds with
  it (`tool/build_web.sh`); WASM is now 339,589 B gzip (+15 KB, the relay api
  and the signing functions), 346 KB with the glue, under the 400 KB bar.

**Stores.**

- `Persist` has three methods: `readAll`, `apply` (reset, then records in
  order, in one transaction) and `clear`.
- `DriftPersist` uses one raw `hearth_records` table in the app's own
  `GeneratedDatabase`. It uses custom statements, so it needs no codegen and
  imports only `package:drift/drift.dart`, which is web-safe. `applyTables`
  writes the app's rows inside the same transaction.
- `IdbPersist` uses one IndexedDB object store through idb_shim. Keys are hex,
  which sorts like the bytes and avoids binary-key support differences. It
  queues every request and awaits only the transaction's completion, so a
  browser cannot auto-commit it half-way. Its `applyTables` may queue writes to
  the app's own stores in the same transaction.
- Drift's web backend was not used. It needs sqlite3.wasm and a worker to be
  shipped, while an IndexedDB store is what ADR 0010 named, and it tests on the
  host with idb_shim's memory factory.

**Open.** `HearthSync.open` goes in this order:

1. read the records and check `stored_info`;
2. throw `DeviceWipedException` for a wiped device, or `wrong_device` for
   another key;
3. open the kernel and store `flush()`, which carries load's repairs and
   `replace_view` after an upgrade;
4. on a new device, enrol it.

Both outcomes are kept in `opened`, because they happen before anyone can
listen to `changes`, and the enrolment's sealed op must reach the relay. The
first host test caught its absence.

**A wiped device after a restart (kernel addition).**
`api::sealed_handover(records, schema, now)` reads the op records without keys.
The index entry is plain, and the envelope is kept as it was sealed. From them it
returns the device's own ops and the past of its Forgets, in clock order. It
leaves out pruned bodies and checkpoints older than the horizon, as the in-session
handover does. The nonce is synthetic (ADR 0008), so these are byte for byte
the envelopes first sent, and the relay stores each op once. A LAN request
cannot be opened without keys, so this path is for the relay only. Dart exposes
it as `HearthSync.wipedHandover`. The app deletes the records (`Persist.clear`)
once the ops have been handed on.

Correction (stage 6): this path is not for the relay either. The relay takes an
append only when the uploading device signs it, and after a restart the wiped
device's key is gone (`open` destroys any key still present before it throws
`DeviceWipedException`). The ops can reach a peer's `ingest` directly; the relay
path for a self-Forget is the in-session one below.

**Sync.** `SyncPeer` holds the five sealed messages (`hello`, `request`,
`offer`, `handover`, `accept`) and `wiped`. `HearthSync` implements it, and
`syncWith(peer)` runs ADR 0011's choreography. A LAN transport implements
`SyncPeer` on its side of the wire. `syncWith` does not hold the queue across
the whole exchange; each message call is queued on its own.

**The relay round (stage 6).** `RelayClient` speaks the protocol page over
package:http in pure Dart: a small strict dCBOR codec (the few item kinds the
protocol uses, depth-limited, shortest form only, integers capped at 2^53 - 1 so
the web holds them exactly), SHA-256 from `cryptography`'s synchronous Dart
hasher, signatures from the `Signer`. It lands in the app's JavaScript, not the
WASM, so the 400 KB WASM budget is untouched. It keeps only the relay's epoch;
every position is the kernel's. `HearthSync.syncWithRelay` runs one round in
the order relay/tests/e2e.rs uses: enrol if the generation is unknown, pull all
pages (a new generation resets and restarts the pull; a pruned cursor fetches
and adopts a snapshot), ingest in batches of 32 with a yield between them,
`relayPulled`, upload the outbox in batches of 64, post the Forget records the
kernel hands out, and upload the snapshot when the relay's listing differs.
`not_enrolled` enrols once and pulls again. The queue is never held across
HTTP: each kernel call queues on its own. `SyncLoop` runs one round at a time,
a full one every few minutes (the relay allows 16 reads per device per five
minutes), an upload-only one debounced after any change with outgoing ops
(uploads carry no nonce, so they cost no read budget), and retries after
network, 408, 429 and 5xx answers with full-jitter exponential backoff.

A self-Forget could not reach the relay at all: the key was destroyed as the
Forget was stored, and the relay needs this device's signature on the append
of the handover and on the Forget record. `forgetSelf(relay:)` now keeps the
key for exactly one upload-only round and destroys it when that round ends,
successful or not. The window is one round. The kernel's seal keys are gone at
the wipe, so the device key alone can sign relay requests and ops no peer
admits, and the relay refuses its appends above the cut once the record is in.
Plain `forgetSelf()` still destroys at once.

**Profiles.** The bridge's release profile is size-first (`z`, LTO, one codegen
unit, abort, strip). The crypto crates are overridden to opt-level 3:
curve25519-dalek, ed25519-dalek, sha2, chacha20, poly1305 and
chacha20poly1305. Measured on the laptop's host build, ingest (unseal, verify,
fold) costs 137 µs per op with the overrides and 2,596 µs without them, 19×
slower. The overrides survive LTO. They cost 7.8 KB of gzipped WASM (324,370 B
against 316,573 B) and about 1.7 KB of host `.so`. WASM speed has not been
measured, because no browser was run. The stage 2 report has the full numbers.

## Consequences

- Changing `api` means updating the mirrors, running the codegen and freezed,
  and committing the output. A mirror that no longer matches its type fails to
  compile in `frb_generated.rs`, so a drift cannot go unnoticed.
- If `Persist.apply` throws, the kernel in memory is ahead of the store. The
  app drops the `HearthSync` and opens it again from the records.
- `ingest` and adoption run on the UI thread, on the web as well. The relay
  round keeps ingest batches small; an adoption is still one call.
- The generated layer is internal. Apps see only the wrapper, so it can change
  with the frb version.
- Nothing here has run in a browser or on a device yet.
