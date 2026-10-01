# ADR 0014: Same-Wi-Fi sync, phone to phone, with no relay

Status: accepted (Lullaby night handoff part B, 2026-10-02)

## Context

Two phones of one household on the same Wi-Fi should sync even when no relay is
configured, reachable or running. Local-first's litmus test is that the app keeps
working if every server is shut down; a relay-only design fails it for the very
case (two parents in one house at 3 a.m.) that matters most. The kernel already
has the peer choreography (`hello` / `request` / `offer` / `handover` / `accept`,
ADR 0011) behind `SyncPeer`, and every sync message leaves the kernel sealed under
the household's key (ADR 0008). What was missing is a transport.

The handoff verdict, read through the NAT-traversal and local-first lenses: the
LAN path is first class, and there is no hole punching for sync. Two phones on one
Wi-Fi have no NAT between them; across the internet the relay is the path, and it
is already the side channel any traversal would need.

## Decision

- **Shape.** One phone listens (`dart:io` `HttpServer`, bound to `0.0.0.0` on an
  ephemeral port) and shows a code. The other connects and drives the kernel's
  `syncWith` against a `SyncPeer` whose calls are HTTP POSTs. One session per code;
  the listener stops when the session closes, when the code's lifetime ends (10
  minutes), or after three refused openings.
- **The code** is 11 bytes in Crockford base32 (`XXXXXX-XXXXXX-XXXXXX`): IPv4
  address, port, a 4-byte one-time token and a CRC-8, so a typo fails when the code
  is read, not after a connect timeout. O, I and L read as 0, 1, 1. A URI with a
  `code` parameter is read too, so an app can put the code in a QR code that opens
  it. The advertised address is chosen on purpose (`pickLanAddress`): a private
  RFC 1918 address, Wi-Fi first, then Ethernet; never loopback, link-local, cellular,
  a VPN, or 100.64/10 (carrier-grade NAT, and where Tailscale lives).
- **Authentication is household membership, not the network.** From the 64-byte
  seed, `Kh = HKDF-SHA256(seed, salt = empty, info = "openhearth.<app>.hearthsync.lan.v1")`
  (the kernel's construction, ADR 0002, in a new domain), and per code
  `Kt = HMAC(Kh, "oh-lan/v1 token" ‖ token)`. The opening is mutual: the client
  proves `Kt` over its nonce, the listener over both nonces and a session id, and the
  session key is `Ks = HMAC(Kt, "oh-lan/v1 session" ‖ cn ‖ sn ‖ sid)`. Every MAC input
  is length-framed. A known-answer test pins `Kh` and `Kt` (computed independently
  with Python's `hmac`), so two builds cannot silently disagree.
- **Every message** is the kernel's own sealed message, carried as the body with
  `HMAC(Ks, "oh-lan/v1 c2s" ‖ sid ‖ seq ‖ method ‖ body)`. `seq` must be exactly one
  more than the last. Every answer carries `HMAC(Ks, "oh-lan/v1 s2c" ‖ sid ‖ seq ‖
  method ‖ status ‖ flags ‖ body)`, so the `needs_snapshot` and `wiped` flags and the
  kernel's error codes cannot be flipped in flight. Any refused message ends the
  session: a replayed, reordered or altered message gets nothing further.
- **Confidentiality** comes from the kernel's sealing, not from the transport: the
  bodies are XChaCha20-Poly1305 envelopes (ADR 0008). The transport adds what the
  synthetic-nonce envelopes cannot give on their own: freshness, ordering, and proof
  that the other phone holds the words before the kernel spends any work on it.
- **Limits.** 32 MiB per message, counted while reading (a chunked body has no
  length to check first), on both sides; 5 s to connect; 60 s per call; 60 s idle;
  the client never uses a configured proxy.
- **Native only.** A web page cannot listen for a connection, and a PWA served over
  https cannot fetch plain http from a LAN address. The web build gets
  `lan_stub.dart` (`lanSupported == false`, every call throws `unsupported`); PWAs
  sync through the relay. Apps should hide the Wi-Fi option on the web.
- **Discovery is the code, not mDNS.** `multicast_dns` (the Flutter team's pure-Dart
  package) can only query, not advertise, so the listener could not be found with it.
  `nsd` can advertise but is a native plugin per platform, needs a Wi-Fi multicast
  lock on Android, and raises the nearby-devices permission questions; and a typed
  code is needed anyway for networks that block multicast (guest Wi-Fi, client
  isolation). If the code proves to be friction, mDNS can be added in front of it;
  the code stays.
- **No in-app scanner.** A QR code is optional for apps. The fleet's scanner
  (`mobile_scanner`) is ML Kit, which is not free software and adds megabytes; a QR
  code that carries an app URI lets the phone's own camera open the app instead.

## Consequences

- An op learned over the LAN joins the relay outbox (ADR 0011), so a relay-using
  phone forwards it later; a household with no relay sees that outbox only grow,
  which apps must not present as "waiting".
- Anyone holding the words can sync, exactly as with the relay. The phrase is the
  household; the token only binds a session to one showing of the code.
- No forward secrecy: a recorded session can be opened later by someone who learns
  the words, as with every sealed op on the relay.
- Plain HTTP on the LAN means an observer sees that two phones talked, when, and how
  many bytes; not what.
- Android: `INTERNET` covers listening and connecting today. A future
  local-network runtime permission would have to be requested before showing or
  using a code.

## Evidence

- `flutter/hearth_sync/test/lan_test.dart`: two devices over a real loopback socket
  (both ways); a phone of another household refused with nothing changed; a replayed
  message refused and the session ended; a tampered body refused; a code used once;
  an oversize message refused; an unanswered code expiring. With the seq check and
  the MAC check removed, the replay and tamper tests go red.
- `flutter/hearth_sync/test/lan_code_test.dart`: the code's round trip, typed
  variants, every single-character typo rejected, the address chooser on fake
  interface lists, and the key known-answer test.
