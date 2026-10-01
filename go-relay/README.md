# hearth-relay-go

The Go relay for hearthSync: the same protocol as the Rust relay in `relay/`, written
independently from [`docs/reference/relay-protocol.md`](../docs/reference/relay-protocol.md)
so that the two can check each other. It stores sealed envelopes it cannot open, checks
household signatures on enrolments and Forgets and device signatures on everything else,
and answers `vectors/relay_v1.json` byte for byte (answers and store digests).

- Why the protocol is shaped this way: [ADR 0013](../docs/adr/0013-relay.md)
- Deploying it: [`deploy/README.md`](deploy/README.md)

## Running

```sh
go build -trimpath -o hearth-relay-go ./cmd/hearth-relay-go
mkdir -p /srv/hearth-relay
./hearth-relay-go --data /srv/hearth-relay --listen 127.0.0.1:8080
```

The flags, output rules and exit codes are the Rust relay's (`--help` lists them). It
prints nothing on stdout while serving; stderr carries one JSON object per line, never a
channel id, device id or client address. SIGTERM or SIGINT stops it.

## Layout

| Path | What |
|---|---|
| `internal/dcbor` | Strict dCBOR decoder (canonical only, depth-limited before it recurses, NFC text) and encoder |
| `internal/edsig` | Strict Ed25519: crypto/ed25519 plus the small-order check dalek's `verify_strict` makes |
| `internal/relay/wire.go` | Request parsing, the signed messages, channel ids, envelope refs |
| `internal/relay/relay.go` | The handler: routing, checks in protocol order, buckets, nonces |
| `internal/relay/store.go` | The bbolt store (one transaction per request) and the canonical dump |
| `internal/relay/http.go` | The net/http server: raw-path routing, body limits, connection cap and lifetime, CORS, the hourly sweep |
| `cmd/hearth-relay-go` | The CLI |
| `difftest/` | The differential harness against the Rust relay, and `rustdriver/`, the Rust relay behind a pipe |
| `deploy/` | Containerfile, systemd unit, deploy notes |

## Dependencies

| Module | Why |
|---|---|
| `go.etcd.io/bbolt` | Persistence: one file, ACID transactions, pure Go (no cgo), needs only `x/sys` |
| `golang.org/x/text` | NFC checking, which dCBOR requires of every text string |

No CBOR library: the relay must accept exactly what the Rust relay's dcbor 0.25 accepts
and refuse deep nesting before it recurses, and the item kinds it needs are few. No
HTTP framework, no CLI or logging library.

## Tests

Keep `GOCACHE`, `GOTMPDIR` and `TMPDIR` on real disk (not `/tmp`, which may be RAM-backed).

```sh
go vet ./... && go test ./...        # vectors, codec, signatures, HTTP layer, CLI
```

The differential harness needs the Rust relay behind its driver. Build that from
`difftest/rustdriver` (its own cargo workspace, with the repo's lockfile), then point
the tests at it:

```sh
(cd difftest/rustdriver && CARGO_TARGET_DIR=../../../target cargo build --offline)
HEARTH_RUST_DRIVER=$PWD/../target/debug/hearth_relay_diffdriver \
  DIFF_RUNS=300 DIFF_STEPS=300 DIFF_OUT=$PWD/.cache/divergences go test -run Differential -v ./difftest/
```

`TestVectorsAgreeOnBothSides` replays the vectors through both relays. `TestDifferential`
generates `DIFF_RUNS` request sequences (seeds `DIFF_SEED`...) of `DIFF_STEPS` steps from
the protocol's grammar, mostly valid and signed, with faults mixed in, under small
random limits and a clock that jumps past the retention and idle periods, with sweeps
and relay restarts (memory dropped, store reopened, epoch moved on) in mid-sequence; it
learns the epoch from `epoch` answers as a client does, compares every answer and the
store digest after every step, and writes each run's first divergence (config and
all steps) to `DIFF_OUT` as JSON, ready to become a vector. `DIFF_SKEW=1` adds labels that
probe the Unicode-version difference the protocol page describes.

The other direction exists too: `difftest/godriver` is the Go relay behind the same
kind of line protocol, so the Rust relay's end-to-end tests (kernel replicas syncing
through a relay, `relay/tests/e2e.rs`) run against the Go relay as well:

```sh
go build -o .cache/godriver ./difftest/godriver
(cd .. && HEARTH_GO_DRIVER=$PWD/go-relay/.cache/godriver CARGO_TARGET_DIR=target cargo test -p hearth_sync_relay --test e2e)
```

Without `HEARTH_GO_DRIVER` those tests run against the Rust relay only.

The driver is built in the dev profile, so an arithmetic overflow in the Rust relay
panics and shows up as a divergence rather than wrapping.
