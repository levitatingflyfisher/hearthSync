# hearth_sync

The OpenHearth sync kernel for Flutter apps. It binds the Rust kernel
(`../../kernel`, crate `hearth_sync_kernel`, its `api` module) through
flutter_rust_bridge 2.13.0 and wraps it in a small Dart API. The device key
stays on the Dart side, in the platform's secure storage. The kernel's records
live in the app's own database: Drift on native, IndexedDB on the web.

Why the boundary sits where it does is in `../../docs/adr/0012-dart-boundary.md`.
The kernel's own contract is in ADRs 0008–0011.

## Use

```dart
import 'package:hearth_sync/hearth_sync.dart';

await HearthSync.init(); // once; loads the .so / .wasm

const schema = SyncSchema([
  SyncTable('rooms', [SyncField('name', SyncType.text)]),
  SyncTable('items', [SyncField('name', SyncType.text), SyncField('room', SyncType.text)],
      containerField: 'room', containerTable: 'rooms'),
  SyncSet('groceries', SyncType.text),
  SyncStream('votes', SyncType.any),
], horizon: Duration(days: 90));

final persist = DriftPersist(appDb, applyTables: (batch) async {
  // Runs inside the same transaction as the records: apply batch.changes
  // (rows, sets, streams; replace everything if batch.changes.replaceView).
});
// On the web: IdbPersist(idbFactoryBrowser, applyTables: ...)

// A wiped device has no words and no key: clear its records (a LAN peer can
// first take HearthSync.wipedHandover's ops into its own ingest).
final stored = await HearthSync.storedDevice(persist);
if (stored?.wiped ?? false) await persist.clear();

final hs = await HearthSync.open(
  app: 'lullaby',
  schema: schema,
  persist: persist,
  signer: SecureStorageSigner(),
  seed: householdSeed,          // 64 bytes from the 12 words
  label: 'Kitchen tablet',
);
paint(hs.view());               // first paint; hs.opened holds what open stored
hs.changes.listen(relayPostOutgoing);

await hs.put('rooms', 'den', {'name': 'Den'});
await hs.setAdd('groceries', 'milk');
await hs.append('votes', 3);
await hs.syncWith(peer);        // another device over a SyncPeer transport
```

## Adopt in an app

1. Declare the schema once per app version (above): add names, never change or
   remove one (ADR 0009).
2. `HearthSync.init()`, then `HearthSync.open(...)` with a `DriftPersist` (or
   `IdbPersist` on the web) whose `applyTables` writes your tables from each
   call's `Changes`, in the same transaction. First paint from `hs.view()`.
3. Write only through `put`, `delete`, `restore`, `setAdd`, `setRemove` and
   `append`.
4. Sync: `final loop = SyncLoop(hs, RelayClient(Uri.parse('https://relay.example/')))..start();`
   It pulls every 5 minutes, uploads 2 s after a write, and backs off with
   jitter; watch `loop.status`, call `loop.syncNow()` on foreground and
   `await loop.stop()` before `hs.close()`. Android needs `INTERNET`.
5. Forget: `hs.forgetDevice(pk)` for another device; `loop.forgetSelf()` for
   this one (it uploads what it owes, then destroys the key). Delete the words
   when `Changes.wiped` is set.
6. Show `hs.review()` (edits a merge overrode, with Undo; `dismissReview`
   when done) and `hs.status()` on a sync-health screen.

## The API

| Call | What it does |
|---|---|
| `HearthSync.init({library})` | Loads the bridge. Tests pass the host `.so`. |
| `HearthSync.open(app:, schema:, persist:, signer:, seed:, label:, clock:)` | Loads the stored records, or starts a new replica, and enrols a new device. Throws `DeviceWipedException` for a wiped device's records, and `wrong_device` if the records belong to another key. |
| `HearthSync.storedDevice(persist)` | Whose records they are and whether that device was wiped. Needs no keys. |
| `HearthSync.wipedHandover(persist:, schema:)` | After a restart, a wiped device's own sealed ops and its Forgets' past, read from the records without keys, byte for byte as first sent. Not for the relay: it takes only requests this device signs, and the key is gone. A peer can `ingest` them. |
| `put` / `delete` / `restore` | Row writes. Last writer wins per field, an edit beats a concurrent delete, and Undo restores. |
| `setAdd` / `setRemove` / `append` | Add-wins sets and append-only streams. |
| `enrollDevice(pk, label)` / `forgetDevice(pk)` / `forgetSelf({relay})` | Pairing and "Forget this device". A forgotten device's key is destroyed through `Signer.destroy`; `forgetSelf(relay:)` keeps it for one upload round first, so the relay gets the handover and the Forget record. |
| `checkpoint()` / `compact()` | Checkpoint, and prune behind the horizon. |
| `ingest(sealed)` / `snapshot()` / `adoptSnapshot(snap, ops)` | The relay path. |
| `relayState()` / `relayPulled(generation, cursors)` / `relayOutbox({max})` / `relayUploaded(ids, firstSeq)` | The relay client's positions, kept in the records: the own log's next seq, the pull cursors, and every op the relay is not known to hold (including ones learned over the LAN), to upload in order, a page of `max` at a time. `relayPulled` refuses cursors from a pull of another generation (`stale_generation`). |
| `relayVerify(ops)` | Checks the signatures of sealed ops ahead of the `adoptSnapshot` that takes them, in small batches, so the adoption skips those checks. |
| `relayGeneration(g)` | The relay's channel generation, from every enroll and pull answer; a new one (the channel expired and was made again) resets the positions and queues everything held for upload again. Pull again afterwards. |
| `relaySnapshot()` / `relayEnrollment(pk)` / `relayForgets()` / `relayForgetPosted(id)` | The base with the covers recorded at its checkpoint; the `enroll` fields; Forget records, each handed out only once its Forget op is on the relay (ADR 0011, "The relay client"). |
| `syncWithRelay(relay, {pull})` | One relay round: enrol if needed, pull (adopting a snapshot past a pruned cursor), ingest in small batches, upload the outbox, post Forget records, upload the snapshot when the base changed. `pull: false` only uploads. Throws `RelayException` or `SnapshotNeededException`. |
| `SyncLoop(hs, relay)` | Schedules rounds: `start`, `syncNow`, `forgetSelf`, `stop`, `status`. One round at a time. |
| `RelayClient(base)` | The relay's HTTP verbs (POST, dCBOR), with the epoch learned and signed on reads. |
| `hello` / `request` / `offer` / `handover` / `accept` | The five sealed LAN messages. `HearthSync` implements `SyncPeer` with them. |
| `syncWith(peer)` | A full two-way sync with any `SyncPeer`, in the kernel's order. |
| `LanListener.start(hs, seed)` / `syncOverLan(hs, seed, code)` | Same-Wi-Fi sync with no relay (ADR 0014): one phone listens and shows a `LanCode`; the other connects with it and drives `syncWith`. The session is authenticated by the household's words plus the code's one-time token; every message is MAC'd with a strict sequence. Native only (`lanSupported`); the web stub throws `LanException('unsupported')`. |
| `view()` / `review()` / `dismissReview(key)` / `devices()` / `status()` | Reading. |
| `changes` / `opened` | Every stored call's `Changes`: rows, sets and streams for the app's tables, `outgoing` sealed ops for the relay, `wiped`, rejections, review entries. `opened` holds what `open` itself stored. |

Every call is serialised. A call's signatures are collected first. Its records
are stored next, through `Persist.apply`, in one transaction and in the kernel's
order. Only then does the call return anything it produced (ADR 0010). Errors
come back as `HearthSyncException` with the kernel's stable `code`, and nothing
changes. If `Persist.apply` itself throws, the kernel is ahead of the store:
drop the `HearthSync` and open it again.

**Values** are `null`, `bool`, `int`, `String` or `Uint8List`.

**Signers.**

- `Signer` is the interface: `publicKey`, `sign`, `destroy`.
- `SecureStorageSigner` keeps a random Ed25519 seed in flutter_secure_storage.
- `SoftwareSigner.fromSeed` holds its seed in memory (its base, and for tests).
- Both sign with the fastest `SigningBackend` available: WebCrypto Ed25519 on
  the web, the Rust bridge (native on Android and Linux, about 0.1 ms)
  elsewhere, pure Dart (about
  4 ms) only before the bridge is loaded. Pass `backend:` to force one. The
  seed never enters the kernel (ADR 0012, "Signing").

**Stores.**

- `Persist` is the interface: `readAll`, `apply`, `clear`.
- `DriftPersist` uses one `hearth_records(key BLOB PRIMARY KEY, value BLOB)`
  table in any `GeneratedDatabase`, with no codegen.
- `IdbPersist` uses one IndexedDB object store with hex keys, through idb_shim.
- `MemoryPersist` is for tests.

Both real stores can write the app's own tables in the same transaction.

## Same-Wi-Fi sync

```dart
// The phone that shows the code:
final listener = await LanListener.start(hs, seed); // throws LanException('unreachable') off Wi-Fi
show(listener.code.text);                          // XXXXXX-XXXXXX-XXXXXX, or a QR of an app URI with ?code=
await listener.done;                               // the other phone synced (or LanException: expired, refused)

// The phone that types it:
final code = LanCode.tryParse(typed);              // null on a typo (the code carries a CRC)
if (code != null) await syncOverLan(hs, seed, code);
```

One code serves one session; it expires after 10 minutes and after three
refused phones. A phone of another household is refused before its kernel does
any work. Android needs only `INTERNET`. A PWA cannot listen, so on the web it
syncs through the relay. Why plain HTTP, why no mDNS and no in-app scanner:
`../../docs/adr/0014-lan-transport.md`.

## Build and test

Run the commands below one at a time on a small machine; each build peaks around 2 GiB.

```sh
# Host library for the Dart tests
(cd rust && CARGO_TARGET_DIR=$PWD/target cargo build --release)
flutter test --concurrency=1 test/sync_test.dart   # and persist, forget, review, signer, relay,
                                                    # relay_wire, relay_client, lan_code, lan

# The end-to-end tests start both real relays on localhost; build them first
# (from the repo root; a missing binary fails the test):
CARGO_TARGET_DIR=$PWD/target cargo build --release -p hearth_sync_relay
(cd go-relay && GOCACHE=$PWD/.cache/gocache go build -o .cache/hearth-relay-go ./cmd/hearth-relay-go)
flutter test --concurrency=1 test/relay_e2e_test.dart

# After changing rust/src/api/kernel.rs
../../.tools/bin/flutter_rust_bridge_codegen generate
dart run build_runner build --force-jit   # freezed; sqlite3's build hooks stop AOT mode

# Web bundle (single-threaded WASM, no COOP/COEP) and APK, from example/
tool/build_web.sh
flutter build apk --release
# With the device panel (Put / Sync / Forget per relay; emulator reaches the host at 10.0.2.2)
flutter build apk --release --target-platform android-x64 --split-per-abi \
  --dart-define=HEARTH_RELAYS=rust=http://10.0.2.2:18081/,go=http://10.0.2.2:18082/
```

The generated files are committed: `lib/src/rust/**` (with `*.freezed.dart`)
and `rust/src/frb_generated.rs`. Apps depend on this package by path and never
run the codegen.

## Not yet

- The LAN transport has run over loopback in host tests and between the
  Android emulator and a host device (Lullaby); not between two real phones.
  No mDNS discovery: the code is typed or scanned.
- The web build has run in headless Chromium only (not Firefox or Safari),
  served with no COOP/COEP: the demo passes, WebCrypto signs, IndexedDB and the
  key survive a reload, and two browsers sync through both relays over CORS. A
  Content-Security-Policy must allow `'unsafe-eval'` (frb's loader runs
  `new Function`). The demo makes no request off its own origin: it builds
  with `--no-web-resources-cdn`, its `web/flutter_bootstrap.js` points
  CanvasKit and fallback fonts at the app, and it bundles a family named
  Roboto so the engine never fetches one. An app on this package should do
  the same, or its PWA fetches CanvasKit and Roboto from Google's gstatic.
- Android has run on an emulator only (API 34, x86_64), not a phone. The release
  APK draws its first frame; signing goes through the native bridge (about
  0.06–0.1 ms a signature, pure Dart 3.5 ms); records (Drift) and the key
  survive an app kill; it syncs with a host device through both relays; a
  self-Forget destroys the key and a relaunch opens as wiped. dart:io sent
  plain `http://` to the relay with no network-security config, so Android's
  cleartext policy does not guard the relay URL. `RelayClient` guards it
  instead: it throws `ArgumentError` for any URL that is not `https://`,
  except plain `http://` to `127.0.0.1`, `::1`, `localhost` or the emulator
  host `10.0.2.2`.
- `ingest` runs on the UI thread, on the web as well. The relay round feeds it
  32 envelopes at a time and yields between batches, uploads a page of the
  outbox at a time, and verifies an adoption's ops in batches first; the
  adoption itself (validity, fold, rebase) is still one call, about 90 µs per
  op natively after verifying ahead, several times that in WASM.
