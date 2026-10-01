# Deploying hearth-relay-go

The Go relay deploys like the Rust one. Read [`relay/deploy/README.md`](../../relay/deploy/README.md)
for the threat model, the TLS proxy (Caddy or nginx), per-address limits and the CPU
alert: all of it applies unchanged, with `hearth-relay-go` for `hearth-relay`. Run one
relay or the other on a data directory, not both; their stores differ (bbolt here,
SQLite there) and neither reads the other's.

For a household running its own relay on a spare computer or Raspberry Pi (a user
unit, a compose file, static arm64 builds and HTTPS options), see
[`home/README.md`](home/README.md).

Nothing here has been deployed, and the image has not been built yet.
The unit passes `systemd-analyze verify` (which only notes the binary is not installed);
its sandbox has not been run.

## Container

`Containerfile` builds a static binary (`CGO_ENABLED=0`, pure Go) into
`gcr.io/distroless/static-debian12:nonroot`: no shell, no libc, non-root, one writable
volume.

```sh
podman build -f go-relay/deploy/Containerfile --ignorefile go-relay/deploy/Containerfile.dockerignore -t hearth-relay-go go-relay
podman network create --internal hearth-internal
podman volume create hearth-data
podman run -d --name hearth-relay-go --network hearth-internal \
  --read-only --cap-drop=ALL --security-opt no-new-privileges \
  --memory 512m --cpus 0.5 --pids-limit 64 -e GOMEMLIMIT=400MiB \
  -v hearth-data:/data hearth-relay-go
```

The internal network is the egress denial: the relay can answer a proxy on that network
and cannot dial out. Pin both base images by digest before a real build.

## systemd

```sh
CGO_ENABLED=0 go build -trimpath -o hearth-relay-go ./cmd/hearth-relay-go   # from go-relay/
install -m 0755 hearth-relay-go /usr/local/bin/
cp deploy/hearth-relay-go.service /etc/systemd/system/
systemctl enable --now hearth-relay-go
```

The unit is the Rust relay's (throwaway user, read-only system, empty capability set,
syscall filter, MemoryMax/CPUQuota, `IPAddressDeny=any` with `IPAddressAllow=localhost`,
so the proxy must run on the same host) plus `GOMEMLIMIT`, which keeps the Go collector
under `MemoryMax`.

## What differs from the Rust relay

- The store is one bbolt file, `relay.bolt`, in `--data`. It is memory-mapped; mapped
  pages are page cache and reclaimable, so `MemoryMax` does not need to cover it. Back
  it up by copying the file while the relay is stopped.
- The binary is static; the image has no libc.
