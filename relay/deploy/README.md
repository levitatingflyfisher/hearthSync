# Deploying hearth-relay

Nothing here has been deployed yet, and the image has not been built
yet. The unit passes `systemd-analyze verify`; its sandbox has not been run.

The threat this kit answers is the **machine** being taken over (to mine, say). The
data is safe regardless: the relay only ever holds sealed envelopes. So the aim is a
process that can do little if hijacked: no shell, no privileges, no writable root, a
CPU and memory cap a miner would hit, and no way to reach a mining pool.

## Pick one: container or systemd

**Container** (`Containerfile`: distroless, no shell, non-root, one writable volume):

```sh
podman build -f relay/deploy/Containerfile --ignorefile relay/deploy/Containerfile.dockerignore -t hearth-relay .
# An internal network has no route out: the relay can answer the proxy, never dial out.
podman network create --internal hearth-internal
podman volume create hearth-data
podman run -d --name hearth-relay --network hearth-internal \
  --read-only --cap-drop=ALL --security-opt no-new-privileges \
  --memory 512m --cpus 0.5 --pids-limit 64 \
  -v hearth-data:/data hearth-relay
```

Pin the two base images by digest before you build for real (`FROM ...@sha256:...`), and
rebuild when either publishes a security update. Egress denial for a container is a
runtime setting (the internal network above, or a host firewall rule); an image cannot
carry it. The default seccomp profile of podman or Docker applies.

**systemd** (`hearth-relay.service`): install the binary
(`cargo build --profile relay -p hearth_sync_relay`, then copy
`target/relay/hearth-relay` to `/usr/local/bin/`), copy the unit to
`/etc/systemd/system/`, and `systemctl enable --now hearth-relay`. The unit denies all
IP traffic except localhost (`IPAddressDeny=any`, `IPAddressAllow=localhost`). That
also blocks inbound traffic from elsewhere, so the TLS proxy must run on the same host.

Either way, turn on the distribution's automatic security updates (`dnf-automatic`,
`unattended-upgrades`).

## HTTPS through a standard proxy

The relay speaks plain HTTP/1.1 and must never face the internet directly. Put a TLS
proxy in front and forward only the relay's paths. With Caddy (automatic certificates):

```
relay.example.org {
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

With nginx, which can also limit requests per client address:

```nginx
limit_req_zone $binary_remote_addr zone=relay:10m rate=10r/s;
server {
    listen 443 ssl;
    server_name relay.example.org;
    # ssl_certificate ... (certbot)
    client_max_body_size 9m;
    client_body_timeout 60s;
    location ~ ^/(v1/|healthz$) {
        limit_req zone=relay burst=40 nodelay;
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
    }
    location / { return 404; }
}
```

In the container setup, run the proxy on both the internal network and the public one,
and proxy to `hearth-relay:8080`.

The relay limits requests per device and per channel itself; per-address limits
belong to the proxy, since the relay never sees or logs client addresses. Keep the
proxy's own access log off, or short-lived, if you want the relay's promise ("logs
carry no channel or device ids") to hold for the whole host: the channel id is in
every request path.

## A CPU alert

A hijacked relay shows up as sustained CPU. The cap limits the damage; the alert tells
you. The relay itself is idle almost all the time: a household sends a few requests an
hour.

With systemd and no monitoring stack, a timer that samples the unit's CPU time:

```sh
# /usr/local/bin/hearth-relay-cpu-alert: alert if the relay used more than 20% of a CPU
# over the last 5 minutes.
set -eu
state=/var/lib/hearth-relay-cpu-alert
now=$(systemctl show -P CPUUsageNSec hearth-relay)
prev=$(cat "$state" 2>/dev/null || echo "$now")
echo "$now" > "$state"
if [ $(( (now - prev) / 1000000000 )) -gt 60 ]; then
  logger -p daemon.crit "hearth-relay: CPU above 20% for 5 minutes"
  # and mail or push it, e.g.: echo "check the relay" | mail -s "hearth-relay CPU" you@example.org
fi
```

Run it from a `.timer` with `OnUnitActiveSec=5min`. With Prometheus and node_exporter
or cAdvisor, alert on the same thing:
`rate(container_cpu_usage_seconds_total{name="hearth-relay"}[5m]) > 0.2 for 15m`.

A probe on `GET /healthz` (through the proxy) tells you it is up.

## What to back up

The data directory holds one SQLite database, `relay.sqlite3` (WAL mode). It is
ciphertext and households can rebuild it by syncing, so a backup only saves them a
re-upload. `sqlite3 relay.sqlite3 ".backup copy.sqlite3"` takes a consistent copy while
the relay runs.
