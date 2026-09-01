#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["cryptography>=42"]
# ///
"""Independent spike vector: build the signed op by hand (no CBOR library) and
sign it with pyca/cryptography, so the Rust and Dart sides are checked against
bytes neither of them produced."""
import hashlib, json
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

SEED = bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")  # RFC 8032 test 1
APP, MILLIS, COUNTER, BODY = "lullaby", 1_727_000_000_000, 0, b"hello"

def head(major, n):
    if n < 24: return bytes([major << 5 | n])
    for ai, size in ((24, 1), (25, 2), (26, 4), (27, 8)):
        if n < 1 << (8 * size): return bytes([major << 5 | ai]) + n.to_bytes(size, "big")

uint = lambda n: head(0, n)
bstr = lambda b: head(2, len(b)) + b
tstr = lambda s: head(3, len(s.encode())) + s.encode()
arr = lambda items: head(4, len(items)) + b"".join(items)

sk = Ed25519PrivateKey.from_private_bytes(SEED)
device = sk.public_key().public_bytes_raw()
fields = [uint(0), uint(1), uint(1), tstr(APP), uint(2), bstr(device), uint(3), arr([]),
          uint(4), arr([uint(MILLIS), uint(COUNTER)]), uint(5), bstr(BODY)]
signable = head(5, 6) + b"".join(fields)
sig = sk.sign(signable)
signed = head(5, 7) + b"".join(fields) + uint(6) + bstr(sig)
print(json.dumps({
    "seed": SEED.hex(), "app": APP, "hlc_millis": MILLIS, "hlc_counter": COUNTER,
    "body": BODY.hex(), "device": device.hex(), "signable": signable.hex(),
    "sig": sig.hex(), "signed": signed.hex(), "id": hashlib.sha256(signed).hexdigest(),
}, indent=2))
