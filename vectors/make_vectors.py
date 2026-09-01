#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["cryptography>=42"]
# ///
"""Shared op vectors for the hearthSync kernel, the relays and any other implementation.

Every byte here is built by hand (no CBOR library) and signed by pyca/cryptography,
so the Rust kernel is checked against bytes it did not produce. The format is the
one in docs/reference/op-format.md; this script is its executable twin.

Run: uv run vectors/make_vectors.py > vectors/ops_v1.json
"""
import hashlib, json
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.kdf.hkdf import HKDF

# ---- minimal deterministic CBOR (RFC 8949 §4.2.1: shortest heads, definite lengths) ----
def head(major, n):
    if n < 24: return bytes([major << 5 | n])
    for ai, size in ((24, 1), (25, 2), (26, 4), (27, 8)):
        if n < 1 << (8 * size): return bytes([major << 5 | ai]) + n.to_bytes(size, "big")

def uint(n): return head(0, n)
def nint(n): return head(1, -1 - n)          # n < 0
def integer(n): return uint(n) if n >= 0 else nint(n)
def bstr(b): return head(2, len(b)) + b
def tstr(s): return head(3, len(s.encode())) + s.encode()
def arr(items): return head(4, len(items)) + b"".join(items)
def cmap(pairs):  # pairs of already-encoded (key, value); keys sorted by encoded bytes
    pairs = sorted(pairs, key=lambda kv: kv[0])
    return head(5, len(pairs)) + b"".join(k + v for k, v in pairs)
NULL, FALSE, TRUE = b"\xf6", b"\xf4", b"\xf5"

def value(v):
    if v is None: return NULL
    if v is True: return TRUE
    if v is False: return FALSE
    if isinstance(v, int): return integer(v)
    if isinstance(v, str): return tstr(v)
    if isinstance(v, bytes): return bstr(v)
    raise TypeError(v)

def ids(xs): return arr([bstr(x) for x in sorted(xs)])

# ---- bodies: [kind, ...] ----
def put(table, row, fields, origin=None):
    # A re-issued edit (rebase) carries its original clock as a 5th item; LWW orders by it.
    items = [uint(0), tstr(table), tstr(row), cmap([(tstr(k), value(v)) for k, v in fields.items()])]
    if origin is not None: items.append(arr([uint(origin[0]), uint(origin[1])]))
    return arr(items)
def delete(table, row, observed): return arr([uint(1), tstr(table), tstr(row), ids(observed)])
def restore(table, row, observed): return arr([uint(2), tstr(table), tstr(row), ids(observed)])
def set_add(s, e): return arr([uint(3), tstr(s), value(e)])
def set_remove(s, e, observed): return arr([uint(4), tstr(s), value(e), ids(observed)])
def append(stream, record): return arr([uint(5), tstr(stream), value(record)])
def enroll(device, label, auth): return arr([uint(6), bstr(device), tstr(label), bstr(auth)])
def forget(device, cut, auth): return arr([uint(7), bstr(device), ids(cut), bstr(auth)])
def checkpoint(state_hash): return arr([uint(8), bstr(state_hash)])

def enroll_auth_msg(app, device, label): return arr([tstr("oh-enroll/v1"), tstr(app), bstr(device), tstr(label)])
def forget_auth_msg(app, device, cut): return arr([tstr("oh-forget/v1"), tstr(app), bstr(device), ids(cut)])

def fields(app, device, parents, millis, counter, body):
    return [(uint(0), uint(1)), (uint(1), tstr(app)), (uint(2), bstr(device)), (uint(3), ids(parents)),
            (uint(4), arr([uint(millis), uint(counter)])), (uint(5), body)]

def sign_op(sk, app, parents, millis, counter, body):
    device = sk.public_key().public_bytes_raw()
    f = fields(app, device, parents, millis, counter, body)
    signable = cmap(f)
    sig = sk.sign(signable)
    signed = cmap(f + [(uint(6), bstr(sig))])
    return {"signable": signable, "sig": sig, "signed": signed, "id": hashlib.sha256(signed).digest(), "device": device}

def enroll_key(seed64, app):
    okm = HKDF(algorithm=hashes.SHA256(), length=32, salt=None, info=f"openhearth.{app}.enroll.v1".encode()).derive(seed64)
    return Ed25519PrivateKey.from_private_bytes(okm), okm

APP = "lullaby"
T0 = 1_727_000_000_000
HOUSEHOLD_SEED = bytes(range(64))
FOREIGN_SEED = bytes([0xFF]) * 64
# RFC 8032 §7.1 TEST 1, 2, 3 secret keys as device keys.
A = Ed25519PrivateKey.from_private_bytes(bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"))
B = Ed25519PrivateKey.from_private_bytes(bytes.fromhex("4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb"))
C = Ed25519PrivateKey.from_private_bytes(bytes.fromhex("c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7"))
pub = lambda sk: sk.public_key().public_bytes_raw()

EK, ek_okm = enroll_key(HOUSEHOLD_SEED, APP)
FK, _ = enroll_key(FOREIGN_SEED, APP)

ops = []
def emit(name, sk, parents, millis, counter, body, note):
    op = sign_op(sk, APP, parents, millis, counter, body)
    op["name"], op["note"], op["parents"], op["millis"], op["counter"] = name, note, parents, millis, counter
    ops.append(op)
    return op["id"]

a_enroll = emit("a_enroll", A, [], T0, 0, enroll(pub(A), "Kitchen tablet", EK.sign(enroll_auth_msg(APP, pub(A), "Kitchen tablet"))), "A enrolls itself; first op of a fresh household")
a_put = emit("a_put", A, [a_enroll], T0 + 1000, 0, put("feeds", "r1", {"ml": 120, "delta": -5, "note": "left side", "done": True, "extra": None, "blob": b"\x01\x02"}), "every value type, fields sorted by encoded key")
b_enroll = emit("b_enroll", B, [a_put], T0 + 2000, 0, enroll(pub(B), "Dad's phone", EK.sign(enroll_auth_msg(APP, pub(B), "Dad's phone"))), "B enrolls itself after seeing A's work")
b_put = emit("b_put", B, [b_enroll], T0 + 3000, 0, put("feeds", "r1", {"ml": 150}), "B overwrites one field; LWW by (hlc, id)")
a_put2 = emit("a_put2", A, [b_enroll], T0 + 3000, 1, put("feeds", "r2", {"ml": 90}), "concurrent with b_put")
a_delete = emit("a_delete", A, [a_put2, b_put], T0 + 4000, 0, delete("feeds", "r1", [a_put, b_put]), "two parents; observes both puts to r1")
a_restore = emit("a_restore", A, [a_delete], T0 + 5000, 0, restore("feeds", "r1", [a_delete]), "undo the delete")
a_set_add = emit("a_set_add", A, [a_restore], T0 + 6000, 0, set_add("groceries", "milk"), "OR-set add; the tag is this op's id")
b_set_remove = emit("b_set_remove", B, [a_set_add], T0 + 7000, 0, set_remove("groceries", "milk", [a_set_add]), "removes the observed tag")
b_append = emit("b_append", B, [b_set_remove], T0 + 8000, 0, append("votes", "alice>bob"), "append-only stream record")
a_forget = emit("a_forget", A, [b_append], T0 + 9000, 0, forget(pub(B), [b_append], EK.sign(forget_auth_msg(APP, pub(B), [b_append]))), "forget B; B's ops up to b_append still count")
a_checkpoint = emit("a_checkpoint", A, [a_forget], T0 + 10000, 0, checkpoint(hashlib.sha256(b"vector placeholder, not a real state").digest()), "structurally valid; its hash is only checked at compaction")
a_reissue = emit("a_reissue", A, [a_checkpoint], T0 + 11000, 0, put("feeds", "r2", {"ml": 95}, origin=(T0 + 2500, 0)), "a re-issued edit: newer by its own clock, but LWW orders it by its origin, so a_put2 still wins r2")
valid = [o["name"] for o in ops]

hostile = []
def bad(name, raw, reason, note):
    hostile.append({"name": name, "signed": raw.hex(), "id": hashlib.sha256(raw).hexdigest(), "reject": reason, "note": note})

good = ops[1]["signed"]  # a_put
flipped = bytearray(good); flipped[-1] ^= 1
bad("bad_signature", bytes(flipped), "bad_signature", "a_put with the last signature byte flipped")
bad("not_canonical", good[:1] + b"\x00\x18\x01" + good[3:], "not_canonical", "a_put with version 1 spelled 0x1801 (non-shortest)")
c_put = sign_op(C, APP, [a_put], T0 + 1500, 0, put("feeds", "r1", {"ml": 1}))
bad("not_enrolled", c_put["signed"], "not_enrolled", "valid signature by C, who was never enrolled")
c_enroll = sign_op(C, APP, [a_put], T0 + 1500, 0, enroll(pub(C), "Stranger", FK.sign(enroll_auth_msg(APP, pub(C), "Stranger"))))
bad("foreign_enroll", c_enroll["signed"], "bad_enroll_auth", "C enrolls with another household's enroll key")
early_delete = sign_op(A, APP, [a_put], T0 + 1500, 0, delete("feeds", "r1", [b_put]))
bad("observed_not_in_past", early_delete["signed"], "observed_not_in_past", "delete observes b_put, which is not in before(u)")
same_clock = sign_op(A, APP, [a_put], T0 + 1000, 0, put("feeds", "r1", {"ml": 2}))
bad("clock_not_after_parents", same_clock["signed"], "clock_not_after_parents", "hlc equal to its parent's")
wrong_app = sign_op(A, "peckish", [a_put], T0 + 1500, 0, put("feeds", "r1", {"ml": 3}))
bad("wrong_app", wrong_app["signed"], "wrong_app", "op for another app's log")
f = fields(APP, pub(A), [a_put], T0 + 1500, 0, put("feeds", "r1", {"ml": 4}))
extra = cmap(f + [(uint(7), uint(0))])
bad("unknown_key", cmap(f + [(uint(6), bstr(A.sign(extra))), (uint(7), uint(0))]), "schema", "closed schema: key 7 is not part of an op")
late_origin = sign_op(A, APP, [a_put], T0 + 1500, 0, put("feeds", "r1", {"ml": 7}, origin=(T0 + 1500, 0)))
bad("origin_not_before_clock", late_origin["signed"], "schema", "a re-issue's origin must be strictly older than its own clock")
child = sign_op(A, APP, [bytes.fromhex(hostile[0]["id"])], T0 + 2500, 0, put("feeds", "r1", {"ml": 6}))
bad("child_of_rejected", child["signed"], "parent_rejected", "built on bad_signature; ingest bad_signature first")

hexify = lambda o: {k: (v.hex() if isinstance(v, bytes) else [x.hex() for x in v] if k == "parents" else v) for k, v in o.items()}
print(json.dumps({
    "format": "hearthSync op vectors v1",
    "app": APP,
    "household_seed": HOUSEHOLD_SEED.hex(),
    "enroll_hkdf_info": f"openhearth.{APP}.enroll.v1",
    "enroll_secret": ek_okm.hex(),
    "enroll_public": pub(EK).hex(),
    "devices": {"A": {"seed": A.private_bytes_raw().hex(), "public": pub(A).hex()},
                "B": {"seed": B.private_bytes_raw().hex(), "public": pub(B).hex()},
                "C": {"seed": C.private_bytes_raw().hex(), "public": pub(C).hex()}},
    "ingest_now_millis": T0 + 60_000,
    "valid_in_order": [hexify(o) for o in ops],
    "hostile": hostile,
    "expected_view_after_valid": {
        "feeds/r1": {"visible": True, "fields": {"ml": 150, "delta": -5, "note": "left side", "done": True, "extra": None, "blob": "0102"}},
        "feeds/r2": {"visible": True, "fields": {"ml": 90}},
        "groceries": [],
        "votes": ["alice>bob"],
        "enrolled": sorted([pub(A).hex(), pub(B).hex()]),
        "forgotten": [pub(B).hex()],
    },
}, indent=1))
