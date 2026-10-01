# ADR 0008: Sealing ops, snapshots and messages

Status: accepted (kernel v1 stage 1, 2026-09-29)

## Context

Ruling Q9 picked XChaCha20-Poly1305. Design §2.1 seals each signed op whole under
the app's sync key with a random 24-byte nonce and AAD
`"oh-op/v1" | app | channel | device`, so the relay never sees parents, clocks or
bodies. ADR 0001 keeps the kernel free of randomness. The v1 requirements ask that every op and snapshot be sealed as it leaves the kernel for storage or the
relay, under keys derived from the household root, with the AAD binding household,
app and op id.

## Decision

- **Keys.** From the 64-byte household seed, with the same HKDF construction and app
  domain rule as the enroll key (ADR 0002), each in its own domain:
  - AEAD key: `HKDF-SHA256(seed, salt = empty, info = "openhearth.<app>.hearthsync.seal.v1")`;
  - nonce key: `…info = "openhearth.<app>.hearthsync.nonce.v1"`.

  These are new domains. They do not reuse `openhearth.<app>.sync.encryption.v1`,
  the key of the old sync tier in `key_derivation.dart`, and they cannot collide
  with a legacy info string, because an app domain has no dots.
- **Envelope.** dCBOR `[1, kind, ref, nonce, ciphertext‖tag]`.
  - `kind` is 0 for an op, 1 for a snapshot and 2 for a sync message.
  - `ref` is the op id, the checkpoint id, or null for a message.
  - `ref` travels in clear, so a receiver (and the relay) can name an envelope
    before opening it.
  - An opened op must hash to its `ref` (`IdMismatch` otherwise).
- **AAD.** dCBOR `["oh-seal/v1", kind name, app, household, ref]`. The household is
  the enroll public key, the household's identity in the log. An envelope therefore
  opens only in its own household and app, under its own id, as its own kind.
- **Nonce.** It is synthetic: the first 24 bytes of `HMAC-SHA256(nonce key, AAD ‖
  plaintext)`. The kernel stays pure. Equal inputs give the same envelope, which
  reveals only that two envelopes carry the same op; the op id in `ref` already
  reveals that. Two different inputs share a nonce with probability about 2⁻⁹⁶
  per pair, and XChaCha's 192-bit nonce is chosen so random nonces are safe at
  that rate.
- **What is sealed:**
  - every op body the kernel hands the app to store (persistence, ADR 0010);
  - every op handed to the relay;
  - every snapshot;
  - every sync message on the LAN, as a whole.

  Index entries and other bookkeeping records stay plain in the app's database.
  That database already holds the app's plaintext tables, so sealing them would
  hide nothing.
- A failed open is reported at the api as `bad_seal` with the envelope's claimed
  `ref`. It is not a validity rejection, and nothing is recorded for it: an
  envelope that does not open cannot be attributed to any op.

## Deviations from design §2.1

- **Synthetic, not random, nonce.** This follows from ADR 0001. It changes nothing
  for confidentiality or integrity at the kernel's message rates. Randomness would
  also have made the persisted records differ between two runs of the same
  history, which the round-trip tests rely on.
- **AAD binds the op id, not the device or a channel.** The id already commits to the
  device (it hashes the signed op, which names it). The relay's per-device log carries
  the device on the outside, under its own outer signature (design §7.2). The channel
  id is replaced by the household's enroll public key, which is fixed at creation
  and needs no separate secret.

## Evidence

- `vectors/make_seal_vectors.py` builds the envelopes by hand, with HKDF and HMAC
  from pyca and the standard library, and XChaCha20-Poly1305 from libsodium through
  PyNaCl. `kernel/tests/seal.rs` reproduces them byte for byte: from the seed, and
  from the raw keys.
- A tamper matrix covers every single-bit flip in an envelope, a flipped tag and a
  flipped ciphertext byte, trailing bytes, another household, another app, the wrong
  kind, a relabelled kind byte, and an op sealed under an id that is not its hash.
- **WASM size.** A probe cdylib calls ingest and compaction, built wasm32 with the
  release profile (opt-level z, LTO) and no wasm-opt:

  | Build | Raw | gzip -9 |
  |---|---|---|
  | kernel without seal | 725,224 B | 259,713 B |
  | kernel with seal | 736,203 B | 263,775 B |

  Sealing adds 10,979 B raw and 4,062 B gzipped. The whole kernel is well above
  the spike's primitives-only 167,615 B gzip, mostly from dcbor. It is still under
  the 400 KB bar, but the bridge glue has not been counted yet.

## Consequences

- A forgotten device drops the seed when it wipes, and with it any way to derive
  these keys again. Within the session that learns of the wipe, the api still holds
  the keys, so it can seal the WipedPush handover (ADR 0006). After a restart the
  wiped device cannot open its own records. Because the nonce is synthetic, it can
  still forward them: `api::sealed_handover` reads the plain index entries and
  returns the stored envelopes of its own ops and its Forgets' past, byte for byte
  the ones it first sent, for the relay. The app deletes the records once those
  have been handed on.
- Changing the envelope means changing `make_seal_vectors.py` first, as with ops.
