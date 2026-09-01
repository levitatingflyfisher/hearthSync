#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["cryptography>=42", "pynacl>=1.5"]
# ///
"""Shared sealing vectors for the hearthSync kernel and the relays (ADR 0008).

Built by hand (no CBOR library): HKDF and HMAC from pyca/cryptography and the
standard library, XChaCha20-Poly1305 from libsodium through PyNaCl, so the Rust
kernel is checked against bytes it did not produce. Reads the ops from
ops_v1.json, so run make_vectors.py first.

Run: uv run vectors/make_seal_vectors.py > vectors/seal_v1.json
"""
import hashlib, hmac, json, pathlib
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from nacl.bindings import crypto_aead_xchacha20poly1305_ietf_encrypt as xchacha_seal

def head(major, n):
    if n < 24: return bytes([major << 5 | n])
    for ai, size in ((24, 1), (25, 2), (26, 4), (27, 8)):
        if n < 1 << (8 * size): return bytes([major << 5 | ai]) + n.to_bytes(size, "big")
def uint(n): return head(0, n)
def bstr(b): return head(2, len(b)) + b
def tstr(s): return head(3, len(s.encode())) + s.encode()
def arr(items): return head(4, len(items)) + b"".join(items)
NULL = b"\xf6"

ops = json.loads((pathlib.Path(__file__).parent / "ops_v1.json").read_text())
APP = ops["app"]
SEED = bytes.fromhex(ops["household_seed"])
HOUSEHOLD = bytes.fromhex(ops["enroll_public"])  # the household's identity in the log

def hkdf(info):
    return HKDF(algorithm=hashes.SHA256(), length=32, salt=None, info=info.encode()).derive(SEED)

SEAL_INFO = f"openhearth.{APP}.hearthsync.seal.v1"
NONCE_INFO = f"openhearth.{APP}.hearthsync.nonce.v1"
SEAL_KEY, NONCE_KEY = hkdf(SEAL_INFO), hkdf(NONCE_INFO)
KINDS = {"op": 0, "snapshot": 1, "msg": 2}

def seal(kind, ref, plaintext):
    ref_c = bstr(ref) if ref is not None else NULL
    aad = arr([tstr("oh-seal/v1"), tstr(kind), tstr(APP), bstr(HOUSEHOLD), ref_c])
    # Synthetic nonce: the kernel draws no randomness (ADR 0001), so the nonce is a
    # PRF of the AAD and the plaintext under its own key.
    nonce = hmac.new(NONCE_KEY, aad + plaintext, hashlib.sha256).digest()[:24]
    ct = xchacha_seal(plaintext, aad, nonce, SEAL_KEY)
    envelope = arr([uint(1), uint(KINDS[kind]), ref_c, bstr(nonce), bstr(ct)])
    return {"kind": kind, "ref": ref.hex() if ref else None, "plaintext": plaintext.hex(),
            "aad": aad.hex(), "nonce": nonce.hex(), "envelope": envelope.hex()}

a_put = next(o for o in ops["valid_in_order"] if o["name"] == "a_put")
a_cp = next(o for o in ops["valid_in_order"] if o["name"] == "a_checkpoint")
cases = [
    dict(seal("op", bytes.fromhex(a_put["id"]), bytes.fromhex(a_put["signed"])), name="op_a_put",
         note="an op sealed for storage or the relay; ref is its id, which must equal SHA-256(plaintext)"),
    dict(seal("snapshot", bytes.fromhex(a_cp["id"]), b"stand-in snapshot bytes: the seal does not parse them"),
         name="snapshot", note="a snapshot sealed under its checkpoint id"),
    dict(seal("msg", None, b"\x82\x00\x80"), name="msg", note="a sync message; no ref"),
]
print(json.dumps({
    "format": "hearthSync seal vectors v1",
    "app": APP,
    "household": HOUSEHOLD.hex(),
    "seal_hkdf_info": SEAL_INFO,
    "seal_key": SEAL_KEY.hex(),
    "nonce_hkdf_info": NONCE_INFO,
    "nonce_key": NONCE_KEY.hex(),
    "cases": cases,
}, indent=1))
