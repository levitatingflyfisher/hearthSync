# A home relay

How a household runs its own hearthSync relay on a spare computer or a Raspberry Pi,
so its devices sync through a machine it owns. The relay only ever holds sealed
envelopes it cannot open. Losing the machine loses no data: the devices hold all of
it, and they upload it again to a new relay.

The files here:

| File | What |
|---|---|
| `build-release.sh` | Static single-file binaries for linux amd64, arm64 and armv7, with `SHA256SUMS` |
| `hearth-relay.user.service` | A systemd **user** unit (no root needed to run the relay) |
| `compose.yaml` | The same relay in a container, for Podman or Docker |

## Which relay

Use the **Go relay** (`go-relay/`) at home. Both relays speak the same protocol and
answer the same conformance vectors, and a differential harness keeps them in step,
so a device cannot tell them apart. The Go relay is easier to run on a small machine:

- **One static file.** It is pure Go (bbolt for storage, no cgo), so
  `GOARCH=arm64 go build` produces a binary for a Raspberry Pi on any machine, with no
  cross-compiler and no system libraries. The Rust relay bundles SQLite, which is C,
  so cross-building it needs a C toolchain for the target.
- **Small, and measured.** Its in-memory tables at their default bound measure about
  24 MiB (ADR 0013), and an idle relay uses a few MiB. The Rust relay's figure is an
  estimate.

The Rust relay (`relay/`) remains a good choice on a machine that builds it natively.

## Quick start

On the spare machine (a 64-bit Raspberry Pi OS shown; on a PC use the `amd64` file):

```sh
git clone https://github.com/levitatingflyfisher/hearthSync && cd hearthSync/go-relay
deploy/home/build-release.sh   # needs Go 1.25+; or build elsewhere and copy the file over
install -D -m 0755 dist/hearth-relay-go-*-linux-arm64 ~/.local/bin/hearth-relay-go
install -D -m 0644 deploy/home/hearth-relay.user.service ~/.config/systemd/user/hearth-relay.service
systemctl --user daemon-reload && systemctl --user enable --now hearth-relay
loginctl enable-linger "$USER"           # keep it running with nobody logged in
curl -s http://127.0.0.1:8080/healthz    # prints: ok
# HTTPS, one way of several (see below): a mesh VPN that issues certificates.
# Install Tailscale here and on every phone, enable HTTPS certificates for the
# tailnet, then:
sudo tailscale serve --bg 8080
tailscale serve status                   # https://<machine>.<tailnet>.ts.net
# Give that https:// address to the app as its relay.
```

## The binary

`build-release.sh [OUT_DIR]` writes `hearth-relay-go-<version>-linux-amd64`, `-arm64`
(64-bit Raspberry Pi 3, 4, 5 and most single-board computers) and `-armv7` (a Pi on a
32-bit OS) into `go-relay/dist/` by default, plus `SHA256SUMS`. It builds with
`-trimpath` and no build id, so the same source and Go version give the same bytes:
two people can build and compare checksums.

`hearth-relay-go --help` lists the options. The ones that matter at home:

| Option | At home |
|---|---|
| `--data <DIR>` | Where the database lives (one file, `relay.bolt`). The directory must exist |
| `--listen 127.0.0.1:8080` | Keep it on loopback; the HTTPS front reaches it there |
| `--max-channels 16` | A channel is one household's log for one app, so a household needs one per app it syncs. The default (1,000) is sized for a shared relay |
| `--idle-days` | A channel with no write for this long is deleted (400 days by default). Devices upload everything again when they return |

## Running it

**systemd user unit.** `hearth-relay.user.service` runs the relay as you, with its
database in `~/.local/state/hearth-relay`. `loginctl enable-linger` keeps your user's
services running after you log out and starts them at boot. The unit sets
`NoNewPrivileges`, a private umask and memory, CPU and task caps. The caps apply where
the distribution delegates those controllers to user services, which current ones do;
elsewhere systemd ignores them. Logs: `journalctl --user -u hearth-relay`. A
machine that faces the internet directly should use the hardened system unit in
`../hearth-relay-go.service` instead (no egress, a throwaway user, a read-only
system).

**Container.** `compose.yaml` builds the image from `../Containerfile` (distroless, no
shell, non-root) and runs it with a read-only root filesystem, no capabilities and the
same caps, publishing the relay on `127.0.0.1:8080` only:

```sh
podman compose -f go-relay/deploy/home/compose.yaml up -d --build   # or: docker compose
```

The database lives in the `hearth-data` volume.

## HTTPS

Phones must reach the relay over HTTPS with a certificate they already trust:

- The package's `RelayClient` refuses any relay address that is not `https://`.
  Plain `http://` is allowed only to the device itself (`127.0.0.1`, `::1`,
  `localhost`) and to the Android emulator's host alias `10.0.2.2`, for development.
- A self-signed certificate, or one from your own certificate authority, does not
  work. Android apps do not trust certificate authorities that a user installs,
  unless the app opts in, and these apps do not.

So the relay listens on loopback, and something on the same machine terminates TLS
with a publicly trusted certificate. Pick one of these:

| Option | Reachable from | Opens a port | Notes |
|---|---|---|---|
| A mesh VPN that issues certificates (Tailscale's `tailscale serve`, as in the quick start) | Every device on the VPN, at home or away | No | Every phone runs the VPN app. The machine's VPN hostname appears in public Certificate Transparency logs |
| Your own domain, with Caddy and an ACME **DNS-01** challenge | Your home network (point the name at the machine's LAN address) | No | Needs a Caddy build with your DNS provider's plugin and an API token for it |
| Your own domain, a router port forward and Caddy (HTTP-01) | Anywhere | Yes, 443 | The machine faces the internet: use the hardened system unit and the proxy notes in `../../../relay/deploy/README.md` |
| A tunnel service that terminates TLS for you | Anywhere | No | The provider sees request paths (channel ids), sizes and timing, never contents: everything is sealed |

A Caddy site for the DNS-01 option (the `dns` line names your provider's plugin; for
the port-forward option, leave out the `tls` block):

```
relay.home.example.org {
    tls {
        dns <provider> {env.DNS_API_TOKEN}
    }
    @relay path /v1/* /healthz
    handle @relay {
        request_body {
            max_size 9MB
        }
        reverse_proxy 127.0.0.1:8080
    }
    respond 404
}
```

Check from a phone's browser: `https://<the address>/healthz` should show `ok` with no
certificate warning.

## Looking after it

- **Backup.** Optional: the relay holds only ciphertext, and devices re-upload
  everything to an empty relay. To copy it anyway, stop the relay
  (`systemctl --user stop hearth-relay`) and copy `~/.local/state/hearth-relay/relay.bolt`.
- **Updating.** Build the new version, replace `~/.local/bin/hearth-relay-go`, and
  `systemctl --user restart hearth-relay`. A restart moves the relay's epoch on; devices
  notice on their next read and carry on.
- **Removing.** `systemctl --user disable --now hearth-relay`, then delete the binary,
  the unit and `~/.local/state/hearth-relay`.
