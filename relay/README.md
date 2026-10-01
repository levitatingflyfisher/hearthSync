# hearth-relay

The Rust relay for hearthSync: data-blind store-and-forward of sealed ops, one channel
per household per app. It stores envelopes it cannot open. It checks household
signatures on enrolments and Forgets and device signatures on every other request,
with the kernel's own functions, so knowing a channel id no longer lets anyone write,
erase or read (D3). No accounts: a device proves membership by signing with the key
the household enrolled.

- Protocol: [`docs/reference/relay-protocol.md`](../docs/reference/relay-protocol.md)
- Why it is shaped this way: [ADR 0013](../docs/adr/0013-relay.md)
- Deploying it (container or systemd, HTTPS, CPU alert): [`deploy/README.md`](deploy/README.md)

## What it does

| Request | What the relay checks |
|---|---|
| `enroll` | The channel id belongs to this app and household; the household enroll key signed the device's admission |
| `forget` | An enrolled poster signed it; the household key signed the Forget |
| `append` | The uploader is enrolled, signed the batch, and continues its log; a forgotten uploader stops at its cut |
| `snapshot` | An enrolled, unforgotten device signed it; it replaces that device's last one |
| `pull` | An enrolled device signed a fresh, unused read under the relay's current epoch; a forgotten one sees the channel frozen at its Forget |
| `fetch_snapshot` | As pull, for one snapshot; not for a forgotten device |

Around those: size limits, rate limits per device and per channel, read nonces capped
per reader, bounded storage per channel and per relay, pruning of log entries that a
snapshot covers once they are old enough, expiry of channels idle for `--idle-days`,
and SQLite persistence under `--data`. The store keeps an epoch that every start
increments, so a read captured before a restart cannot be replayed after it.

## Running

```sh
cargo build --profile relay -p hearth_sync_relay
mkdir -p /srv/hearth-relay
target/relay/hearth-relay --data /srv/hearth-relay --listen 127.0.0.1:8080
```

`hearth-relay --help` lists the options. It prints nothing on stdout while serving;
stderr carries one JSON object per line (request verb, status and timing), never a
channel or device id. SIGTERM or SIGINT stops it. Put a TLS proxy in front.

## Tests

```sh
# From the repo root.
CARGO_TARGET_DIR=target cargo test -p hearth_sync_relay
PROPTEST_CASES=20000 CARGO_TARGET_DIR=target cargo test -p hearth_sync_relay --test fuzz
uv run vectors/make_relay_vectors.py > vectors/relay_v1.json   # after changing the protocol
```

| Test | What |
|---|---|
| `tests/conformance.rs` | Every case in `vectors/relay_v1.json`: byte-equal answers and store digest. The Go relay runs the same file |
| `tests/e2e.rs` | Kernel replicas, driven through the kernel api, syncing only through an in-process relay: convergence, Forget and wipe, self-Forget handover, return past the horizon via a snapshot |
| `tests/http.rs` | CORS, body limits, the handler over real HTTP, and the CLI's house rules |
| `tests/fuzz.rs` | proptest: arbitrary and tampered requests, the nesting guard |

## Layout

| Path | What |
|---|---|
| `src/wire.rs` | Request parsing, the signed messages, the nesting guard, client builders |
| `src/relay.rs` | The handler: routing, checks in protocol order, rate limits, nonces |
| `src/store.rs` | SQLite store and the canonical dump |
| `src/http.rs` | The hyper server: limits, timeouts, CORS, the periodic prune |
| `src/main.rs` | The CLI |
| `deploy/` | Containerfile, systemd unit, deploy notes |

Changing the protocol means changing `vectors/make_relay_vectors.py` and the reference
page first, regenerating the vectors, and watching `tests/conformance.rs` fail.
