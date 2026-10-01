# Relay protocol v1

The wire contract between devices and a hearthSync relay. `relay/` (Rust) and `go-relay/` (Go)
implement it, and `vectors/relay_v1.json` pins it byte for byte. When this page
and the vectors disagree, the vectors win and this page has a bug. ADR 0013 explains the
choices; this page only states them.

The two relays were written independently. A differential harness
(`go-relay/difftest/`) drives both with the same request sequences, and every place
where they, or they and the vectors' model, disagreed is settled on this page with a
vector.

A relay stores sealed ops (ADR 0008) for a household's app. It cannot open them. It checks
only what it can check without the household's keys: household signatures on enrolments
and Forgets, and device signatures on every other request.

## Conventions

- **dCBOR everywhere.** Every request and response body is one deterministic CBOR item
  (RFC 8949 §4.2 with the dCBOR profile, as ops are). A body that does not decode, or
  that re-encodes to different bytes, is `bad_request`. Text must be NFC (dCBOR's
  rule), so a label in decomposed form is `bad_request`.
- **Nesting.** A body, or an envelope, with non-empty arrays (or maps or tags) nested
  more than seven deep is refused before it is decoded (`bad_request`, or
  `bad_envelope` for an envelope): decoders recurse per level. No valid request nests
  more than four deep.
- **Unicode version.** NFC depends on the Unicode version. The Rust relay's dcbor uses
  Unicode 17 (unicode-normalization 0.1.25); the Go relay's x/text uses Unicode 15.0.
  They disagree on text that is out of canonical order only under code points assigned
  after 15.0 (for example `"a\u0897\u0316"`: Rust refuses it, Go accepts it). Clients
  never send such text: dcbor normalises every string to NFC when it encodes, so a
  label from the kernel or `wire::client` arrives in Unicode 17 order, and normalisation
  is stable, so text in NFC under 17 is in NFC under 15.0 too. Only a hand-built body
  can reach the skew. Clients should still keep labels to code points assigned by
  Unicode 15.0.
- **Types.** `bstr32` is a 32-byte byte string, `bstr64` a 64-byte one, `uint` an
  unsigned integer, `text` a text string. Times are Unix milliseconds as `uint`. Every
  `uint` is a whole u64: a relay stores and echoes it exactly (a `cut_seq` or cover of
  2^64 - 1 is kept as sent, not clamped to a storage type's range), and arithmetic on
  it is exact (a batch whose last seq would pass 2^64 - 1 is above any cut).
- **Signatures** are Ed25519, checked strictly (`op::verify_strict`: no small-order
  keys, no malleable signatures). A signature whose R, or whose public key, has small
  order never verifies, for household keys as for device keys, even though the plain
  Ed25519 equation holds for it (R = identity, s = 0 under the identity key verifies
  every message in OpenSSL and in Go's crypto/ed25519). Both encodings of such a point
  count, canonical or with y >= p, with either sign bit. A device signs with its own device key, the one that
  signs its ops. Every signed message is a dCBOR array whose first item is a tag naming
  the request, and whose second item is the channel, so a signature is never valid for
  another request, another channel, or as an op (ops are maps).
- **Freshness.** A signed request carries `ts`. It is `stale` unless
  `now - W <= ts <= now + W`, with `W = 300000` (five minutes), computed without
  wrapping (for `now < W` the lower bound is 0).
- **Epoch.** The relay keeps one `uint`, its epoch, in its store, and adds 1 to it every
  time it starts (a new store starts at 1). Reads (`pull`, `fetch_snapshot`) carry and
  sign `epoch`; one whose `epoch` is not the relay's is `["err", "epoch", current]`.
  A client that does not know the epoch sends 0 (never valid) and retries with the
  one the answer names. Nonces live in memory only, so without the epoch a read
  captured before a restart could be replayed after it; with it, that read names an
  old epoch and is refused. The epoch is not part of the store digest.
- **Nonces.** Reads carry a 16-byte `nonce`. The relay remembers `(channel, reader,
  nonce)` until `ts + W`, for every verb that carries one; a second request with the
  same triple is `replay`. A nonce is live while `ts + W >= now` (at `now = ts + W` a
  reuse is still `replay`). A reader holds at most `max_reader_nonces` live nonces in
  a channel: a fresh nonce beyond that is `rate_limited`, and is not recorded. A nonce
  is used once it passes these checks, even if a later check fails. There is no
  relay-wide cap, so one reader, or one household, can never use up another's reads;
  the table is bounded by `max_channels` x `max_devices` x `max_reader_nonces`.
- **Restarts.** Nonces and rate-limit buckets are memory only. After a restart the
  nonce table is empty and every bucket is full, and the epoch has moved on, so no
  read from before the restart can be replayed. Signed writes are not protected by
  nonces: each carries `ts` inside the window and is idempotent (a retried append
  stores no entry, a repeated Forget changes no record; either moves `last_write`,
  as any fresh signed write does), before a restart or after it.
- **Sorted lists.** A list of ids, or of `[device, n]` pairs, is sorted ascending by the
  id (bytewise) with no duplicates, or the body is `bad_request`.

## Channels

A channel is one household's log for one app. Its id is

```
channel = SHA-256( dCBOR ["oh-relay-channel/v1", app, household] )
```

where `household` is the enroll public key (ADR 0002) and `app` the app domain. In a path
it is 64 lowercase hex digits. The relay checks the id when the first enrolment creates
the channel, so no one can claim a channel for a household whose enroll key they do not
hold. The id is not a secret, and knowing it grants nothing.

## Requests

Every request is `POST /v1/{channel}/{verb}`. `OPTIONS` on any path answers `204` with the
CORS headers (below). `GET /healthz` answers `200` with the text `ok` (for probes; it
touches no channel); any other method on `/healthz` is `404 not_found`, since only that
exact pair is special. The path is checked first: an unknown path, or a channel that is
not 64 lowercase hex digits, is `404 not_found`; then a method other than POST is
`405 method`. The path is the request target's path exactly as sent: not percent-decoded,
not cleaned (`//`, `..`, a trailing `/` all make it unknown), and without its query.

A success is `200` with a dCBOR array whose first item is `"ok"`. An error is
`["err", code]` (two errors, `seq` and `epoch`, add a third item) with the status below.

| Status | Code | Meaning |
|---|---|---|
| 400 | `bad_request` | Not canonical dCBOR, or not the shape below |
| 400 | `channel_mismatch` | Enrol: the path's channel is not the id of this app and household |
| 400 | `bad_envelope` | An envelope that is not a canonical sealed envelope with a ref |
| 401 | `stale` | `ts` outside the window |
| 401 | `bad_signature` | The device signature does not verify |
| 401 | `replay` | A read's nonce was already used |
| 403 | `bad_auth` | The household signature on an enrolment or Forget does not verify |
| 403 | `not_enrolled` | The signer is not enrolled in this channel (also: no such channel) |
| 403 | `forgotten` | The device was forgotten and may not do this |
| 404 | `no_snapshot` | That device has stored no snapshot |
| 409 | `seq` | An append does not continue the log; the third item is the log's last seq |
| 409 | `epoch` | A read names another epoch; the third item is the relay's epoch |
| 413 | `too_large` | A body, envelope, snapshot or batch over its limit |
| 429 | `rate_limited` | Too many requests from this device, channel or relay |
| 507 | `quota` | The channel's or relay's storage, device or channel cap is reached |
| 404 | `not_found` | Unknown path |
| 405 | `method` | Not POST (or OPTIONS, or GET /healthz) |

The HTTP layer adds two answers of its own, outside the vectors: `408` `timeout` (the
body did not arrive in time) and `500` `internal` (a store failure or a panic).

"No such channel" is `not_enrolled`, so a request never learns whether a channel exists.
The checks run in the order listed for each verb; the first failure is the answer.

**A request that fails changes nothing in the store**: not even the prune an append or
snapshot ran before its quota check (each request is one transaction). What a failing
request does keep is in memory: the rate-limit tokens it took and the nonce it used.

"Rate" in a verb's checks below means: the signer's device bucket, then the channel's
bucket (see "Rate limits"). A request the channel bucket refuses keeps the device token
it took.

A channel's `last_write` is the `now` of the last request that wrote to it: every
`forget`, `append` or `snapshot` answered `200` (a retried append that stores nothing
counts: it is a signed, fresh request from a live device), and an `enroll` answered
`200` that stored a record (a repeat of an enrolment does not count: it carries no
`ts`, so anyone who saw it once could replay it forever). It drives idle expiry
(below).

### enroll

Admit a device. The household enroll key signed the admission, as it does inside the
Enroll op; the relay checks that signature with the kernel's own `keys::enroll_auth_msg`.

```
[app: text, household: bstr32, device: bstr32, label: text, auth: bstr64]
```

1. Shape: `app` is a valid app domain (`keys::app_domain_ok`), `label` is 1 to 128 bytes.
   → `bad_request`
2. `channel` equals the channel id of `app` and `household` → `channel_mismatch`
3. `auth` verifies under `household` over `["oh-enroll/v1", app, device, label]`
   → `bad_auth`
4. The device was forgotten in this channel → `forgotten`
5. The channel is new and the relay holds `max_channels` channels → `quota`
6. Rate: the relay's channel-creation bucket (only if the channel is new), then the
   channel's enrol bucket → `rate_limited`
7. The device is already enrolled → `["ok", generation]` (the first record is kept)
8. The channel holds `max_devices` devices → `quota`
9. Store the channel (if new, with a new generation) and the record, and set the
   channel's `last_write` → `["ok", generation]`

`generation` is the channel's generation (see "Generations").

The label is visible to the relay: the household signature covers it.

### forget

Record a Forget. `target`, `cut` and `auth` are the Forget op's own fields; `auth` is
checked with `keys::forget_auth_msg`. The relay cannot read op ids' order, so the poster
adds `cut_seq`: the last seq of the target's relay log that the cut covers.

```
[target: bstr32, cut: [bstr32...], auth: bstr64, cut_seq: uint, poster: bstr32, ts: uint, sig: bstr64]
```

Signed by `poster` over
`["oh-relay-forget/v1", channel, poster, ts, target, cut, auth, cut_seq]`.

1. Shape (`cut` sorted, at most 64 ids) → `bad_request`
2. `poster` is enrolled in the channel (a forgotten poster may still post: a wiped device
   hands over its Forgets) → `not_enrolled`
3. `ts` → `stale`
4. `sig` → `bad_signature`
5. `auth` verifies under the channel's household over `["oh-forget/v1", app, target, cut]`
   → `bad_auth`
6. Rate → `rate_limited`
7. The target has no record in the channel and the channel holds `max_devices` devices
   → `quota`
8. Record it and set the channel's `last_write` → `["ok"]`. The first Forget of a target sets its `cut_seq` and its
   `freeze_ord` (the channel's last assigned `ord`, see below); a later one lowers
   `cut_seq` to the smaller of the two, as the kernel excludes an op any Forget cuts out.

Post the Forget record **after** appending the sealed Forget op itself: a forgotten
device reads the channel as it stood when its Forget was recorded (see pull), and must
find the Forget op there to wipe.

### append

Add sealed ops to the uploader's log. A log is numbered from 1 by the uploader. It holds
what that device uploaded, which is normally what it wrote, but the relay cannot tell
authorship (the op is sealed) and does not try.

```
[uploader: bstr32, first_seq: uint, envelopes: [bstr...], ts: uint, sig: bstr64]
```

Signed by `uploader` over
`["oh-relay-append/v1", channel, uploader, ts, first_seq, [SHA-256(envelope)...]]`.
Envelope `i` gets seq `first_seq + i`.

1. Shape: `first_seq >= 1`, at least one envelope → `bad_request`
2. More than `max_batch` envelopes, or one longer than `max_envelope` bytes → `too_large`
3. `uploader` is enrolled → `not_enrolled`
4. `ts` → `stale`
5. `sig` → `bad_signature`
6. Rate → `rate_limited`
7. Every envelope parses as a sealed envelope (ADR 0008) with a ref
   (`seal::envelope_ref`) → `bad_envelope`
8. The uploader is forgotten and the batch's last seq is above its `cut_seq`
   → `forgotten`
9. `first_seq > last + 1` → `["err", "seq", last]`. Where the batch overlaps seqs the log
   already has: a held entry with a different envelope → `["err", "seq", last]`; a seq
   whose entry was pruned is skipped, never stored again.
10. Prune the channel (below).
11. The new entries would take the channel over `channel_quota` bytes, or the relay over
    `max_total_bytes` → `quota`
12. Store each new entry with the next `ord` (a counter per channel, from 1) and
    `stored_at = now`, and set the channel's `last_write` → `["ok", last]`

A retried append (same entries) answers `["ok", last]` and stores nothing.

### snapshot

Store the uploader's sealed snapshot (the kernel's `Kernel::snapshot`), replacing its
previous one. `covers` says which log entries the snapshot makes unnecessary: for each
listed uploader, every entry up to that seq is behind the snapshot's checkpoint.

```
[device: bstr32, envelope: bstr, covers: [[bstr32, uint]...], ts: uint, sig: bstr64]
```

Signed by `device` over
`["oh-relay-snapshot/v1", channel, device, ts, SHA-256(envelope), covers]`.

1. Shape → `bad_request`
2. The envelope is longer than `max_snapshot` bytes, or `covers` lists more than
   `max_devices` uploaders (covers are stored, and not counted as envelope bytes)
   → `too_large`
3. `device` is enrolled → `not_enrolled`
4. `ts` → `stale`
5. `sig` → `bad_signature`
6. `device` is forgotten → `forgotten`
7. Rate → `rate_limited`
8. The envelope has a ref → `bad_envelope`
9. Prune the channel (below).
10. The channel (less the replaced snapshot) would exceed `channel_quota`, or the relay
    `max_total_bytes` → `quota`
11. Store it (`stored_at = now`) and set the channel's `last_write` → `["ok"]`

### pull

Read the logs from the reader's cursors.

```
[reader: bstr32, ts: uint, epoch: uint, nonce: bstr16, cursors: [[bstr32, uint]...], sig: bstr64]
```

Signed by `reader` over `["oh-relay-pull/v1", channel, reader, ts, epoch, nonce, cursors]`.

1. Shape → `bad_request`
2. `cursors` lists more than `max_devices` uploaders → `too_large`
3. `reader` is enrolled, or forgotten → `not_enrolled`. A forgotten reader pulls
   frozen (below), even one that was never enrolled in the channel: a Forget can be
   recorded for a target that has no record yet, as when the household returns after
   its channel expired and forgets a device before that device enrols again. The
   target must still be able to read its Forget op, since `enroll` answers it
   `forgotten`.
4. `ts` → `stale`
5. `sig` → `bad_signature`
6. `epoch` is the relay's → `["err", "epoch", current]`
7. `nonce` is not live → `replay`; the reader already holds `max_reader_nonces` live
   nonces in this channel → `rate_limited`
8. Rate → `rate_limited`

Answer: `["ok", generation, logs, snapshots, more]`, `generation` as for enroll.

- `logs`: one `[uploader, first, entries]` per uploader that has ever appended, sorted by
  uploader. `first` is the lowest seq the relay still holds (`last + 1` if none): a
  cursor below `first - 1` means entries were pruned and the reader needs a snapshot.
  `entries` are `[seq, envelope]` with `seq > cursor` (0 for an uploader not in
  `cursors`), ascending.
- The page is bounded: uploaders are taken in order and entries within each in order; an
  entry is left out if the page already holds `max_pull_entries` entries, or if its
  envelope would take the page's envelope bytes over `max_pull_bytes` (unless the page is
  still empty). Once one entry is left out, every later one is. `more` is `true` if any
  was.
- `snapshots`: `[device, ref, covers]` for each stored snapshot, sorted by device (fetch
  one with `fetch_snapshot`).
- A **forgotten** reader sees the channel frozen at its Forget: only entries whose `ord`
  is at most its `freeze_ord`, and no snapshots. An entry it cannot see neither counts
  toward the page's budgets nor sets `more`; `first` is still the lowest seq held, seen
  or not. That is enough to deliver the Forget op
  and wipe, and nothing written after it.

### fetch_snapshot

```
[reader: bstr32, ts: uint, epoch: uint, nonce: bstr16, device: bstr32, sig: bstr64]
```

Signed by `reader` over
`["oh-relay-fetch-snapshot/v1", channel, reader, ts, epoch, nonce, device]`.

1. Shape → `bad_request`
2. `reader` is enrolled → `not_enrolled`
3. `ts` → `stale`
4. `sig` → `bad_signature`
5. `epoch` is the relay's → `["err", "epoch", current]`
6. `nonce` is not live → `replay`; the reader holds `max_reader_nonces` → `rate_limited`
7. `reader` is forgotten → `forgotten`
8. Rate → `rate_limited`
9. `device` has no snapshot → `no_snapshot`

Answer: `["ok", envelope, covers]`.

## Pruning

There is no delete endpoint. Before storing, an append or snapshot prunes its channel as
of `now` (and the server prunes every channel when it starts and hourly): a log entry `(uploader, seq)` is dropped when some stored snapshot's `covers`
gives that uploader a seq at least `seq`, and `stored_at + retain <= now`, where `retain`
is the horizon plus a grace period (90 + 30 days by default). A log's `last` survives
pruning, so a replayed old append can never store a pruned entry again.

A snapshot is what the kernel hands out as its base, which exists only once the device
has pruned behind a checkpoint older than the horizon. The client computes `covers`: the
cursors of its last pull before that checkpoint, provided that pull left nothing pending,
quarantined or held.

## Idle channels

A channel with no write for `idle` ms expires. The server's sweep (at start-up and
hourly) expires every channel whose `last_write + idle <= now`: it deletes the channel's
log entries, snapshots and covers, the record of every device that is not forgotten,
and those devices' logs. If no device record is left, the channel itself is deleted,
and its id no longer counts toward `max_channels`. Otherwise the channel stays, holding
only its forgotten devices' records and their logs' `last` (with `next_ord` and
`last_write` unchanged), and takes a new generation. So a forgotten device can never enrol again (its old enrolment
`auth` carries no time and would otherwise re-admit it to a channel the household later
reuses), and can never store entries again at seqs it already used, which it could
otherwise fill up to its `cut_seq` for the returning household to pull. Requests between two
sweeps see an idle channel as it is; only the sweep expires it.

A forgotten device whose Forget was recorded before the expiry keeps its record, but
the entries it could see are gone with the rest, so it can no longer learn its Forget
through this relay (it still can over the LAN, ADR 0014). One forgotten after the
household returned reads the new log frozen at its new record, which holds the Forget
op that was uploaded again first.

A household that returns after its channel expired finds its devices `not_enrolled`
and enrols them again. The `enroll` answer names a new generation, and so does every
later `pull`: the client's positions (its own log's seq, its cursors, its covers)
belong to logs that are gone. It starts over (the kernel's `relay_generation`): its
own log from seq 1, empty cursors, every op it holds queued for upload again, then a
fresh pull before it uploads. The data itself lives on the devices; the relay only
carried it.

## Generations

The relay keeps a counter in its store (not in the digest), and gives a channel the
next value, its generation, whenever it creates the channel or expires it and keeps it.
So a channel's generation changes exactly when its logs were wiped, never on a restart,
and a channel made again after being deleted never repeats an old generation.

## Rate limits

Token buckets, all in memory. A bucket holds up to `burst` tokens and gains one every
`interval` ms (whole tokens only: the remainder carries over, and a full bucket's clock
restarts at `now`). A request takes one token or is `rate_limited`. Buckets start full,
including after a restart (they are not stored: limits reset, and replay stays
impossible because of the epoch).

| Bucket | Key | Taken by | Default burst, interval |
|---|---|---|---|
| device | channel, signer | forget, append, snapshot, pull, fetch_snapshot | 120, 500 ms |
| channel | channel | the same, after the device bucket | 600, 100 ms |
| enrol | channel | enroll | 16, 60 s |
| creation | the relay | enroll of a new channel | 32, 60 s |

An `interval` of 0 counts as 1 ms. The device bucket stops one device flooding; the
channel bucket stops one household's devices together flooding (32 devices each at the
device rate would be 64 requests a second).

## Limits (defaults)

| Name | Default |
|---|---|
| `max_envelope` | 66,560 bytes (an op's 64 KiB plus the envelope) |
| `max_batch` | 64 envelopes |
| `max_snapshot` | 8 MiB |
| `max_body` (HTTP) | the larger of `max_snapshot` and `max_batch` x (`max_envelope` + 8), plus 64 KiB |
| `channel_quota` | 64 MiB |
| `max_total_bytes` | 8 GiB |
| `max_devices` | 32 per channel |
| `max_channels` | 1,000 |
| `max_pull_entries` | 512 |
| `max_pull_bytes` | 4 MiB |
| `retain` | 120 days |
| `idle` | 400 days |
| `max_reader_nonces` | 16 live nonces per reader and channel |

Storage is counted as the sum of the stored envelopes' lengths (log entries and
snapshots).

## Clients

A client treats the relay as untrusted: it depth-limits every answer before decoding it
(answers nest at most six deep), opens every envelope with the household keys, and
lets the kernel validate every op.

A device's read budget is `max_reader_nonces` reads per window (16 per five minutes by
default, one every 19 seconds on average), whatever its device bucket would allow: each
read's nonce stays live until `ts + W`. A client pulls when it has reason to (a write, a
foreground, a timer of minutes, not seconds), and treats `rate_limited` as "back off".

## HTTP

- Bodies larger than `max_body` are `413 too_large` without being read.
- Every response carries `Access-Control-Allow-Origin: *` (PWAs call the relay from
  their own origin; no request carries a cookie or credential). `OPTIONS` answers `204`
  with `Access-Control-Allow-Methods: POST`, `Access-Control-Allow-Headers:
  content-type` and `Access-Control-Max-Age: 86400`.
- Responses are `Content-Type: application/cbor`. The relay does not check the
  request's content type.
- The relay speaks plain HTTP/1.1 and expects a TLS proxy in front (see
  `relay/deploy/README.md`).

### Test hook (not part of the protocol)

A relay started with `--test-hooks` (both CLIs) also answers `POST /test/sweep`: the
body is a decimal number of milliseconds `N` (empty means 0), and the relay runs its
sweep (pruning and idle expiry) as of `now + N`, then answers `200` with the text
`ok` (`400 bad_request` for any other body). Nonces still expire as of `now`, and the
relay's clock does not move, so requests signed with the wall clock stay fresh. It
lets an end-to-end test expire a channel without restarting the relay or faking its
clock. Without the flag the path is unknown (`404 not_found`), as any other. It is for
tests only: never start a relay that serves households with it.

The hook lives in each relay's HTTP layer, not in the request handler: the vectors and
the differential harness drive the handler (and run the same sweep as their `sweep`
steps), so neither changes.

## Store digest

Conformance compares the relay's whole store after each case, as `SHA-256` of this dCBOR
dump:

```
[ channel... ]                               sorted by channel id
channel  = [id, app, household, generation, next_ord, last_write, devices, logs, snapshots]
devices  = [[device, label | null, auth | null, cut_seq | null, freeze_ord | null]...]
logs     = [[uploader, last, [[seq, ord, stored_at, SHA-256(envelope)]...]]...]
snapshots= [[device, stored_at, SHA-256(envelope), covers]...]
```

Each list is sorted by its first item. `next_ord` is the next `ord` to assign (1 in a
new channel). A device has `label` and `auth` once enrolled, `cut_seq` and `freeze_ord`
once forgotten. A log appears once its uploader has appended. `last_write` is as in
"Requests" (the enrolment that created the channel sets it first). The epoch, the
generation counter, nonces and buckets are not part of the digest.

## Vectors

`vectors/relay_v1.json`, built by `vectors/make_relay_vectors.py` (hand-built CBOR, pyca
signatures, no relay code):

```json
{
  "format": "hearthSync relay conformance v1",
  "config": { "window_ms": 300000, "max_batch": 4, "...": "every limit above" },
  "cases": [
    {
      "name": "...", "note": "...",
      "config": { "optional overrides of the top-level config": 0 },
      "steps": [
        { "now": 0, "method": "POST", "path": "/v1/<hex>/enroll",
          "body": "<hex>", "status": 200, "response": "<hex>" },
        { "now": 0, "sweep": true },
        { "now": 0, "restart": true }
      ],
      "store_digest": "<hex>"
    }
  ]
}
```

Each case starts from an empty relay with its config (epoch 1). Run the steps in order;
each request must answer exactly `status` and `response` (the body bytes). A `sweep`
step runs the server's sweep (pruning and idle expiry) as of `now`. A `restart` step
stops the relay and starts it again over the same store: memory (nonces, buckets) is
lost and the epoch goes up by 1. After the last step the store
digest must equal `store_digest`. The vectors drive the relay's request handler, not
HTTP: CORS, body limits and timeouts are HTTP-layer tests in each implementation.
