# ADR 0013: The relays (Rust and Go) and their differential harness

Status: accepted (kernel v1 stage 3, 2026-09-30; the Go relay and the harness in stage 4;
epochs, per-reader nonces, channel rate limits and idle expiry in stage 5)

## Context

D3 found that the old TypeScript relay authorised every request by the channel id
alone: anyone who learned it could overwrite or erase every device's data. Decision 4
wants a household or community relay (and an optional project-run one) in Rust and in
Go, held to one conformance suite, with D3 fixed before anything is deployed and the
host hardened against being taken over. Design §7 sketches the contract. The kernel
already hands the app sealed ops for the relay (`Outcome::outgoing`), takes them back
(`Kernel::ingest`), and seals snapshots (`Kernel::snapshot`, `adopt_snapshot`).

The wire contract is `docs/reference/relay-protocol.md`; the vectors are
`vectors/relay_v1.json`. This ADR records why the contract is shaped as it is.

## Decision

### What the relay checks, and with what

The relay cannot open an envelope, so it checks only signatures and framing, always
with the kernel's own code:

- **Enrolment**: the household enroll key's signature over the kernel's own
  `keys::enroll_auth_msg(app, device, label)`, verified with `op::verify_strict`. The
  relay is sent the Enroll's `(device, label, auth)`, never the Enroll op: the op would
  show it parents and a clock (design §7.1: no DAG, no HLCs). The label is therefore
  visible to the relay, since the household signature covers it.
- **Forget**: likewise with `keys::forget_auth_msg(app, target, cut)`. The cut's op ids
  are opaque hashes to the relay.
- **Every other request** is signed by a device key over a dCBOR array that starts with
  a tag naming the request and the channel. Ops are dCBOR maps, so a request signature
  can never pass as an op signature or as another request's, and never in another
  channel.
- **Envelopes**: `seal::envelope_ref` must find a ref. The kind byte is not checked:
  the kernel keeps its parser private, and a second parser would drift. It is also
  harmless: the AAD binds the kind, so an op filed as a snapshot never opens.

### Channels are bound to their household

`channel = SHA-256(dCBOR ["oh-relay-channel/v1", app, household])`. Design §7.1 had
trust on first use: the first write set the household key. With a derived id, the
relay checks the id on the first enrolment, so nobody can squat a channel for a
household whose enroll key they lack, and there is no first-use window. The id stays
public; knowing it grants nothing.

### One POST shape, dCBOR everywhere

Every request is `POST /v1/{channel}/{verb}` with a dCBOR body; reads are POSTs too,
so their signature and nonce travel in the body like any other request's. Both relays
must answer byte for byte, and dCBOR gives each value one encoding. §7.2's REST verbs
(PUT, GET) would have put auth in headers, which proxies rewrite.

### Logs are numbered by their uploader, and hold what it uploaded

Each device appends to its own log with seqs it chooses (`first_seq`, from 1); the
relay refuses gaps and rewrites and ignores exact retries. The relay cannot tell who
authored a sealed op, so a log holds what its device *uploaded*. Normally that is what
it wrote, but a client may upload another device's op that the relay lacks (an op it
only learned over the LAN, which its own later ops build on); nothing else can deliver
such an op to relay-only peers. The api does not yet list such ops for forwarding.

### Forget at the relay

- A forgotten device's appends above its `cut_seq` are refused. The poster supplies
  `cut_seq` (the relay cannot order op ids); the lowest of several Forgets wins, as the
  kernel excludes an op that any Forget cuts out.
- Up to `cut_seq` a forgotten device may still append, and it may post Forgets. That is
  the WipedPush handover (ADR 0006): a device that forgets itself sets `cut_seq` to
  cover its queued ops, so the handover works whether the record or the ops arrive
  first.
- A forgotten device reads the channel **frozen** at its Forget: only entries stored
  before the record (`ord <= freeze_ord`), no snapshots. The project's decision says the
  remote device wipes when it next receives a message, and the kernel wipes only on a
  delivered Forget op, whose parents it needs, so the relay must let it read that far.
  It must not let it read further: until MLS the forgotten device still holds the sync
  key, and the freeze keeps whatever the household writes afterwards from it. The
  client therefore posts the Forget record *after* appending the Forget op.
- Relay refusals are resource protection, not correctness: the kernel excludes a
  forgotten device's late ops everywhere anyway (V6).

### Pruning follows the kernel's checkpoints, through covers

The relay cannot see which ops a checkpoint covers. A snapshot upload therefore carries
`covers`, per uploader the seq up to which the snapshot makes entries unnecessary, and
the relay drops an entry once some snapshot covers it and it has been held for the
horizon plus 30 days' grace. §7.2's rule (drop old entries once a newer snapshot
exists) would drop ops a snapshot never saw, for example a late upload the
checkpointing device had not pulled. A log's `last` survives pruning, so a replayed
append can never store a pruned entry again, and every signed write carries a `ts`
checked against a five-minute window. A client computes covers from the cursors of its
last clean pull before its own checkpoint. Snapshots of another device's checkpoint go
up with empty covers (they prune nothing). There is no client DELETE.

### Limits

Per device, channel and relay, all configurable, all in the spec: envelope and batch
size, snapshot size, bytes per channel and per relay, devices per channel (which also
caps the uploaders a snapshot's covers or a pull's cursors may list: covers are stored
outside the byte quota), channels per relay (anyone can mint a household with a fresh seed, so the relay-wide caps are what
bound its disk), page size, and token buckets. Buckets and read nonces are memory only.
Per-address limits are the TLS proxy's job; the relay never sees or logs addresses.

### Replay across a restart: an epoch, not stored nonces

Reads carry a single-use nonce, remembered until the request's `ts` leaves the window.
The nonces live in memory, so before stage 5 a read captured shortly before a restart
could be replayed just after it. Three fixes were weighed:

- **Store the nonces.** Every read becomes a write transaction and an fsync (SQLite
  runs `synchronous = FULL`). It collides with the rule that a failed request changes
  nothing in the store, while a nonce must be used up even when a later check fails.
  The nonce rows would also have to be bounded on disk, and bbolt never shrinks its
  file, so the churn would only grow the Go relay's.
- **A time floor after start** (refuse reads with `ts` below the start time plus the
  window). It costs a five-minute read outage after every restart, and it is only as
  sound as the host clock: a clock stepped back across a restart reopens the hole.
- **An epoch** (chosen). The store holds one counter that every start increments (a
  new store starts at 1). Reads sign it; a read naming another epoch is
  `["err", "epoch", current]`, so a client learns the epoch with one extra round trip
  (it starts with 0, which is never valid) and signs again. Nonces stay in memory,
  scoped to the epoch: a read captured before a restart names an old epoch and fails.
  It costs one write per start, depends on no clock, and changes no write verb.

The epoch is checked after the signature (only an enrolled device learns it, not that
it is secret) and before the nonce, so a wrong-epoch read uses no nonce. It is not in
the store digest; the `epoch` answers in the vectors pin it instead. Writes need no
epoch: each carries a `ts` inside the window and is idempotent.

### Nonces are capped per reader

Stage 3 kept one relay-wide table capped at 200,000 live nonces, answering
`rate_limited` past it. Anyone can mint a household, so one household could fill the
table with its own reads and lock every other household out of reading for five
minutes; the per-read sweep over the table was also O(n) under the relay's lock.
Now each reader (a device key in a channel) holds at most `max_reader_nonces` live
nonces (16 by default); a fresh nonce past that is `rate_limited` and not recorded,
and only that reader waits. There is no relay-wide cap, since any relay-wide cap
brings the cross-household denial back. Expiry happens per reader on each read, and
across the table at the hourly sweep.

The table is bounded by `max_channels` x `max_devices` x `max_reader_nonces`. Stage 5
first shipped 10,000 channels x 64 devices, a worst case of 640,000 readers, and put its
cost at about 415 MB against the systemd unit's `MemoryMax=512M`: too close. The
defaults are now 1,000 channels x 32 devices x 16 nonces, so 32,000 readers and 512,000
nonces. `go-relay/internal/relay/memory_test.go` fills the Go relay's nonce and bucket
tables to exactly that bound and measures **23.7 MiB of live heap (776 bytes per
reader)**. The old defaults would have been about 474 MiB of live heap by the same
measure, before any GC headroom. The Rust relay's tables are smaller per entry (a
vector of 16 nonces and a hashbrown slot, about 500 bytes per reader by count, not
measured), so about 16 MB. Either way the worst case is a few percent of the unit's
memory.

To reach it, an attacker has to mint all 1,000 channels (the creation bucket allows 32 a
minute) and enrol 32 devices in each. An operator who wants a bigger relay raises
`--max-channels` and `--max-devices`, and can tune `--max-reader-nonces`,
`--channel-burst` and `--channel-interval-ms`; the product of the first three sets the
table's size. 32 devices per household is generous: the kernel has no smaller limit, but
no household is near it.

The cap of 16 still lets an honest client page through about 8,000 entries per window;
a client that needs more is told `rate_limited` and waits. The cap, not the device
bucket, is therefore the binding limit on reads: about one every 19 seconds per device,
where the bucket would allow two a second. That is the price of the memory bound, and
the protocol page tells clients to poll in minutes.

### Channel generations: a returning household starts over by itself

Expiry wipes a channel's logs, but every device of the household still holds positions
in them: its own log's seq, its cursors, its checkpoint covers. Without a signal, a
returning device's upload would be answered `seq` with last 0, forever. So the relay
gives each channel a generation from a counter in its store (not in the digest),
whenever it creates the channel or expires it and keeps it, and names it in every
`enroll` and `pull` answer; the channel's generation is in the digest. A counter rather
than, say, the creation time, because a channel deleted and made again in the same
millisecond must still differ, and a restart must not change it.

The client side is the kernel's `relay_generation` (ADR 0011): the first generation is
recorded; a different one resets the positions and queues every op still held with a
body. A returning device's first read is answered `not_enrolled` (its record expired
with the channel), so it enrols again, and that answer names the new generation. It
then pulls from the start before uploading, so ops another device already re-uploaded
drop out of its outbox. Ops behind a device's base have no body to re-upload; a device
that needs them adopts a snapshot, uploaded with empty covers. The relay's e2e tests
drive exactly this across a sweep with a fake clock, against both relays.

### Rate limits per device and per channel

The device bucket alone let one household's 32 devices together send 64 requests a
second. A channel bucket (600 burst, one token per 100 ms) is now taken by every
device-signed verb, after the device bucket, so one device cannot drain its
household's tokens with requests its own bucket would have refused. A request the
channel bucket refuses keeps the device token it took. The enrol and creation buckets
are unchanged. Buckets are memory only and start full after a restart. Limits reset
on a restart, which a restart-looping attacker could exploit only by restarting the
relay; that needs the host. Replay stays impossible because of the epoch.

### Idle channels expire, and forgotten devices stay

A channel with no write for `idle` (400 days by default, `--idle-days`) is expired by
the sweep, at start-up and hourly. Its logs, entries, snapshots, covers and the records
of devices not forgotten are deleted, and the channel row too if no record is left, so
its slot under `max_channels` is freed. 400 days is well past the kernel's 90-day
horizon, so every device of such a household would need a snapshot anyway, and the data
lives on the devices.

A write is a `forget`, `append` or `snapshot` answered 200, including a retried append
that stores nothing (it is signed and fresh, so a live device sent it), or an `enroll`
that stored a record. A repeated enrolment does not count: it carries no `ts`, so
anyone who once saw one could keep an abandoned channel alive forever by replaying it.
Reads do not count; the dispatch asked for "no writes", and a household that only reads
for 400 days has no device writing. `last_write` is in the store digest, so the two
relays cannot disagree about when it moves without the vectors or the harness seeing it.

**Forgotten records survive expiry.** A forgotten device keeps its old enrolment, whose
household signature carries no time. If expiry deleted its record, the device could
re-enrol in the channel the moment the household came back, and read whatever the
household wrote next. Until MLS the relay's freeze is the only barrier between a
forgotten device, which still holds the sync key, and new writes. So an expired channel
that has forgotten devices keeps its row, their records and their logs' `last`
(`next_ord` and `last_write` unchanged), and nothing else. The kept `last` matters: with
it reset to 0, a stolen forgotten device could fill seqs 1 to its `cut_seq` with fresh
envelopes for the returning household to pull (the kernel would exclude them, but they
would cost storage and a pull); with it kept, those seqs are "pruned" and skipped. Such a channel keeps holding a slot, and an attacker could
pin slots that way. They can already pin one by writing once every 400 days, so
`max_channels` stays the bound.

**A forgotten device reads frozen even if it never enrolled in the channel.** A
`forget` may create a record for a target the channel does not know. That happens in
practice after an expiry: the household returns, and a device forgets another before
that one has enrolled again. The target's pull used to answer `not_enrolled` (it had
no enrolment) and its enrolment `forgotten`, so it could never read its Forget op
through the relay. A pull now accepts a forgotten reader with or without an
enrolment, and shows it the channel frozen at its record, which holds the Forget op
(the author re-uploads it before posting the record). `fetch_snapshot` is unchanged.
A device forgotten *before* the expiry keeps its record, but the entries it could see
expire with the rest, so it learns its Forget only over the LAN (ADR 0014).

### Two relays, and the harness that holds them together

The Go relay (`go-relay/`, stage 4) is the protocol implemented a second time from the
protocol page alone: its own strict dCBOR decoder (depth-limited before it recurses,
NFC text), its own strict Ed25519, a bbolt store and a net/http server, with the same
CLI and deploy kit. It answers `vectors/relay_v1.json` byte for byte. Two
implementations exist so that a sentence of the page that can be read two ways shows up
as a disagreement rather than as a silent fork.

`go-relay/difftest/` drives both with the same generated request sequences and compares
every answer and the store digest after every step. It runs the Go relay in process and
the Rust relay behind `rustdriver/` (a line protocol over a pipe), both over stores on
disk. The sequences are mostly valid and signed requests from a small cast, with faults
mixed in, small random limits, and a clock that jumps past the retention and idle
periods. They include sweeps, and restarts in mid-sequence: memory is dropped, the store
reopened, the epoch moved on. The generator learns the epoch from `epoch` answers, as a
client does. Each divergence it finds is settled on the protocol page with a vector.
The vector format carries `sweep` and `restart` steps, so a rule about either can be
pinned. The harness does not reach the HTTP layer; each relay tests that itself.

### Implementation

- **The handler is pure apart from its store.** Time comes in as `now`, so the vectors
  drive it directly and the e2e test runs replicas through it in-process; the HTTP
  layer reads the clock and calls it.
- **A nesting guard before every dCBOR decode.** dcbor 0.25 decodes recursively, one
  stack frame per level with no limit; 65,000 nested arrays overflow a 2 MiB thread
  stack and abort the process. `wire::nesting_ok` walks the heads without recursing and
  refuses anything nested more than 8 deep, both whole bodies and each envelope before
  `envelope_ref`. Since kernel v1 stage 3b the one implementation lives in the kernel
  (`cbor::nesting_ok`, which guards every kernel decode too, ADR 0003); `wire`
  re-exports it, and the relay keeps its own tighter limit of 8.
- **The relay build profile** is `opt-level = 3` (a signature check per request) with
  `panic = "unwind"`, so a panic in one request's task becomes a 500 instead of taking
  every household's relay down. The workspace release profile stays tuned for the
  kernel's WASM size.
- **Persistence: SQLite** through rusqlite, one transaction per request. Files would
  need their own crash-safety; SQLite gives atomic requests, bounded-storage accounting
  and a consistent online backup. The store digest used by the conformance suite is
  defined over logical content, so the Go relay may store however it likes.
- **Dependencies**, each for one job, pinned exactly:

  | Crate | Why |
  |---|---|
  | `hearth_sync_kernel` | The checks themselves (reuse, not copies) |
  | `dcbor` | Bodies; already in the tree through the kernel |
  | `rusqlite` (bundled) | Persistence; SQLite compiled in, no system library in the image |
  | `tokio` (rt, net, time, signal, sync) | The async runtime hyper needs; SIGTERM |
  | `hyper` (server, http1) | The HTTP/1.1 server: small, memory-safe, widely deployed |
  | `hyper-util` (tokio) | The glue between hyper and tokio (IO, timer) |
  | `http-body-util` | Body collection with a hard size limit |

  axum was not taken: routing six fixed paths does not need a router, and axum adds
  tower, a router and serde. Logs are hand-written JSON lines, argument parsing is by
  hand, so there is no tracing, serde or clap.
- **Fuzzing uses proptest.** cargo-fuzz needs a nightly toolchain, which the build
  machine does not have and which is a machine-wide change. The proptest targets cover request
  parsing, signature and household-auth checks under tampering, and the nesting guard.

## Deviations from design §7

- Channel ids are derived from the household key, not trusted on first use.
- All requests are POST with dCBOR bodies; reads are signed in the body.
- The outer signature covers `(tag, channel, device, ts, first_seq, envelope hashes)`
  for an append: the task's `(channel, device, op hash, timestamp)` plus the seq that
  numbers the log.
- Logs hold what a device uploaded, which may include others' ops.
- Pruning needs a snapshot's explicit covers, not only a newer snapshot.
- A forgotten device may read, frozen at its Forget, and append up to its cut.
- Snapshots are listed in pull answers and fetched one at a time (a channel's snapshots
  together could exceed any sensible response).

## Consequences

- The client side needed three things the api did not hand out, which the e2e test
  builds from the seed instead: the plain `(device, label, auth)` of an Enroll and
  `(target, cut, auth)` of a Forget; the covers at each own checkpoint (the cursors of
  the last clean pull); and a list of ops the relay lacks that this device holds, for
  forwarding. Kernel v1 stage 3b added them to the api (ADR 0011, "The relay
  client"): `relay_enrollment`, `relay_forgets`, `relay_snapshot`, `relay_outbox`,
  with `relay_state`, `relay_pulled`, `relay_uploaded` and `relay_forget_posted`
  keeping the client's positions in the kernel's records. Since stage 5
  `relay/tests/e2e.rs` drives devices through those calls only (and learns the epoch
  from the relay). The record-first order of a self-Forget is gone from it: the api
  hands a Forget record out only after the Forget op is acknowledged.
- Since stage 6 the Flutter package has the same client in Dart (`RelayClient`,
  `HearthSync.syncWithRelay`, `SyncLoop`; ADR 0012), with its own strict,
  depth-limited dCBOR decoder, polling in minutes and uploading after writes
  without spending reads. Its end-to-end test runs both relay binaries, and reaches
  idle expiry through the relays' test-only `--test-hooks` endpoint (`POST
  /test/sweep`, a sweep as if time had passed; relay-protocol.md, "Test hook"),
  which lives in the HTTP layer so the handler, the vectors and the harness are
  unchanged.
- Labels are visible to the relay. Apps should keep device labels unrevealing, or a
  later protocol version could send a separate, relay-only label signed with the
  enrolment.
- Clients depth-limit relay answers too (`wire::client` does); a hostile relay must
  not be able to crash an app through its decoder.
- Each relay implementation carries the same nesting guard; the Go relay's decoder
  refuses nesting deeper than 8 before it recurses.
- An idle channel expires after `idle`. A household returning after that finds its
  devices unknown, enrols them again, and each client starts over on the channel's new
  generation (`relay_generation`), re-uploading what it holds.
- MLS (design §7.4) will add a `commit` verb with channel-wide ordering; the per-uploader
  logs are unaffected.
