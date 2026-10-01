#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["cryptography>=42"]
# ///
"""Relay conformance vectors: request bytes, the exact response bytes, and the store digest.

Every request is built by hand (no CBOR library) and signed by pyca/cryptography. The
expected answers come from the small reference model below, written from
docs/reference/relay-protocol.md alone, never from the Rust or Go relay. Each step also
names the answer it is meant to provoke, and the script refuses to write a vector whose
model answer differs, so a case always tests what its name says.

Run: uv run vectors/make_relay_vectors.py > vectors/relay_v1.json
"""
import copy, hashlib, json
from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey
from cryptography.hazmat.primitives.kdf.hkdf import HKDF

# ---- minimal deterministic CBOR (as make_vectors.py) ----
def head(major, n):
    if n < 24: return bytes([major << 5 | n])
    for ai, size in ((24, 1), (25, 2), (26, 4), (27, 8)):
        if n < 1 << (8 * size): return bytes([major << 5 | ai]) + n.to_bytes(size, "big")

def uint(n): return head(0, n)
def bstr(b): return head(2, len(b)) + b
def tstr(s): return head(3, len(s.encode())) + s.encode()
def arr(items): return head(4, len(items)) + b"".join(items)
NULL, FALSE, TRUE = b"\xf6", b"\xf4", b"\xf5"
def opt(x, f): return NULL if x is None else f(x)
def sha(b): return hashlib.sha256(b).digest()
def pairs(ps): return arr([arr([bstr(d), uint(n)]) for d, n in sorted(ps)])
def ids(xs): return arr([bstr(x) for x in sorted(xs)])

# ---- keys ----
APP = "lullaby"
T0 = 1_727_000_000_000
W = 300_000
DAY = 86_400_000
P25519 = 2**255 - 19

def enroll_key(seed64, app):
    okm = HKDF(algorithm=hashes.SHA256(), length=32, salt=None, info=f"openhearth.{app}.enroll.v1".encode()).derive(seed64)
    return Ed25519PrivateKey.from_private_bytes(okm)

pub = lambda sk: sk.public_key().public_bytes_raw()
EK = enroll_key(bytes(range(64)), APP)          # the household of ops_v1.json
FK = enroll_key(bytes([0xFF]) * 64, APP)        # another household
A = Ed25519PrivateKey.from_private_bytes(bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"))
B = Ed25519PrivateKey.from_private_bytes(bytes.fromhex("4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb"))
C = Ed25519PrivateKey.from_private_bytes(bytes.fromhex("c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7"))
HH, FH = pub(EK), pub(FK)

class Forger:
    """A small-order key (the identity, canonical or as y = p + 1). Its "signature",
    R = identity and s = 0, satisfies the plain Ed25519 equation for every message;
    a strict verifier refuses it."""
    def __init__(self, enc): self.enc = enc
    def public_key(self): return self
    def public_bytes_raw(self): return self.enc
    def sign(self, msg): return bytes([1]) + bytes(63)
IDENTITY = Forger(bytes([1]) + bytes(31))
IDENTITY_NC = Forger((P25519 + 1).to_bytes(32, "little"))    # the identity, non-canonically

def channel_id(app, household): return sha(arr([tstr("oh-relay-channel/v1"), tstr(app), bstr(household)]))
CH = channel_id(APP, HH)
FCH = channel_id(APP, FH)

def enroll_auth_msg(app, device, label): return arr([tstr("oh-enroll/v1"), tstr(app), bstr(device), tstr(label)])
def forget_auth_msg(app, device, cut): return arr([tstr("oh-forget/v1"), tstr(app), bstr(device), ids(cut)])

# Strict verification (op::verify_strict): a small-order public key or R never verifies.
# OpenSSL (under pyca) accepts them, so the model refuses them itself: a point has small
# order exactly when y mod p is one of these (any sign bit, canonical or not).
Y8 = 2707385501144840649318225287225658788936804267575313519463743609750303402022
SMALL_ORDER_Y = {0, 1, P25519 - 1, Y8, P25519 - Y8}
def small_order(enc):
    return (int.from_bytes(enc, "little") & ((1 << 255) - 1)) % P25519 in SMALL_ORDER_Y

def verify(pk, msg, sig):
    if small_order(pk) or small_order(sig[:32]):
        return False
    try:
        Ed25519PublicKey.from_public_bytes(pk).verify(sig, msg)
        return True
    except (InvalidSignature, ValueError):
        return False

# ---- envelopes: the relay only parses them, so the ciphertext here is filler ----
def envelope(ref, size=40, kind=0, tag=0):
    ct = bytes([(tag + i) % 251 for i in range(size)])
    return arr([uint(1), uint(kind), opt(ref, bstr), bstr(bytes(24)), bstr(ct)])

def env_ref(env):
    """seal::envelope_ref for the envelopes this script builds (it never builds a
    non-canonical one; a malformed one is plain junk)."""
    if len(env) < 3 or env[0] != 0x85 or env[1] != 0x01 or env[2] > 0x02: return None
    if env[3] == 0xf6: return None
    if env[3] != 0x58 or env[4] != 32: return None
    return env[5:37]

REF = lambda n: sha(b"op %d" % n)

# ---- requests: (body bytes, structured form for the model) ----
def enroll_req(app, household_sk, device, label, auth_sk=None):
    auth = (auth_sk or household_sk).sign(enroll_auth_msg(app, device, label))
    household = pub(household_sk)
    return arr([tstr(app), bstr(household), bstr(device), tstr(label), bstr(auth)]), \
        dict(verb="enroll", app=app, household=household, device=device, label=label, auth=auth)

def forget_req(ch, target, cut, cut_seq, poster_sk, ts, auth_sk=EK, sign_sk=None):
    auth = auth_sk.sign(forget_auth_msg(APP, target, cut))
    poster = pub(poster_sk)
    signable = arr([tstr("oh-relay-forget/v1"), bstr(ch), bstr(poster), uint(ts), bstr(target), ids(cut), bstr(auth), uint(cut_seq)])
    sig = (sign_sk or poster_sk).sign(signable)
    return arr([bstr(target), ids(cut), bstr(auth), uint(cut_seq), bstr(poster), uint(ts), bstr(sig)]), \
        dict(verb="forget", target=target, cut=cut, auth=auth, cut_seq=cut_seq, signer=poster, ts=ts, signable=signable, sig=sig)

def append_req(ch, sk, first, envs, ts, sign_sk=None, signable=None):
    up = pub(sk)
    signable = signable or arr([tstr("oh-relay-append/v1"), bstr(ch), bstr(up), uint(ts), uint(first), arr([bstr(sha(e)) for e in envs])])
    sig = (sign_sk or sk).sign(signable)
    return arr([bstr(up), uint(first), arr([bstr(e) for e in envs]), uint(ts), bstr(sig)]), \
        dict(verb="append", signer=up, first=first, envs=envs, ts=ts, signable=signable, sig=sig)

def snapshot_req(ch, sk, env, covers, ts):
    d = pub(sk)
    signable = arr([tstr("oh-relay-snapshot/v1"), bstr(ch), bstr(d), uint(ts), bstr(sha(env)), pairs(covers)])
    sig = sk.sign(signable)
    return arr([bstr(d), bstr(env), pairs(covers), uint(ts), bstr(sig)]), \
        dict(verb="snapshot", signer=d, env=env, covers=sorted(covers), ts=ts, signable=signable, sig=sig)

def pull_req(ch, sk, ts, nonce, cursors, sign_sk=None, epoch=1):
    r = pub(sk)
    signable = arr([tstr("oh-relay-pull/v1"), bstr(ch), bstr(r), uint(ts), uint(epoch), bstr(nonce), pairs(cursors)])
    sig = (sign_sk or sk).sign(signable)
    return arr([bstr(r), uint(ts), uint(epoch), bstr(nonce), pairs(cursors), bstr(sig)]), \
        dict(verb="pull", signer=r, ts=ts, epoch=epoch, nonce=nonce, cursors=dict(cursors), signable=signable, sig=sig)

def fetch_req(ch, sk, ts, nonce, device, epoch=1):
    r = pub(sk)
    signable = arr([tstr("oh-relay-fetch-snapshot/v1"), bstr(ch), bstr(r), uint(ts), uint(epoch), bstr(nonce), bstr(device)])
    sig = sk.sign(signable)
    return arr([bstr(r), uint(ts), uint(epoch), bstr(nonce), bstr(device), bstr(sig)]), \
        dict(verb="fetch_snapshot", signer=r, ts=ts, epoch=epoch, nonce=nonce, device=device, signable=signable, sig=sig)

VERB_PATH = {"enroll": "enroll", "forget": "forget", "append": "append", "snapshot": "snapshot",
             "pull": "pull", "fetch_snapshot": "fetch_snapshot"}

# ---- the reference model ----
DEFAULT = dict(window_ms=W, max_envelope=66_560, max_batch=64, max_snapshot=8 << 20, channel_quota=64 << 20,
               max_total_bytes=8 << 30, max_devices=32, max_channels=1_000, max_pull_entries=512,
               max_pull_bytes=4 << 20, retain_ms=120 * DAY, device_burst=120, device_interval_ms=500,
               enroll_burst=16, enroll_interval_ms=60_000, create_burst=32, create_interval_ms=60_000,
               channel_burst=600, channel_interval_ms=100, idle_ms=400 * DAY, max_reader_nonces=16)

OK = arr([tstr("ok")])
def err(code, *extra): return arr([tstr("err"), tstr(code)] + list(extra))
STATUS = {"bad_request": 400, "channel_mismatch": 400, "bad_envelope": 400, "stale": 401, "bad_signature": 401,
          "replay": 401, "bad_auth": 403, "not_enrolled": 403, "forgotten": 403, "no_snapshot": 404,
          "seq": 409, "epoch": 409, "too_large": 413, "rate_limited": 429, "quota": 507, "not_found": 404, "method": 405}

class Fail(Exception):
    def __init__(self, code, *extra): self.code, self.extra = code, extra

class Bucket:
    def __init__(self, burst, interval, now): self.burst, self.interval, self.tokens, self.last = burst, interval, burst, now
    def take(self, now):
        if now > self.last:
            add = (now - self.last) // self.interval
            self.tokens = min(self.burst, self.tokens + add)
            self.last = now if self.tokens == self.burst else self.last + add * self.interval
        if self.tokens == 0: return False
        self.tokens -= 1
        return True

class Relay:
    def __init__(self, cfg):
        self.cfg = cfg
        self.channels = {}   # id -> dict(app, household, next_ord, last_write, devices{d: dict}, logs{u: dict(last, entries{seq: (ord, at, env)})}, snaps{d: (at, env, covers)})
        self.buckets, self.nonces = {}, {}   # memory only: nonces is (channel, reader) -> {nonce: expiry}
        self.epoch = 1                       # in the store; a new store starts at 1
        self.gen = 0                         # in the store: the last generation given out

    def restart(self):
        """Stop and start over the same store: memory is lost, the epoch moves on."""
        self.buckets, self.nonces = {}, {}
        self.epoch += 1

    def sweep(self, now):
        """The server's sweep: prune every channel, drop expired nonces, expire idle channels."""
        for c in self.channels.values(): self.prune(c, now)
        self.nonces = {k: {n: e for n, e in v.items() if e >= now} for k, v in self.nonces.items()}
        for ch in list(self.channels):
            c = self.channels[ch]
            if c["last_write"] + self.cfg["idle_ms"] > now: continue
            c["snaps"] = {}
            c["devices"] = {d: v for d, v in c["devices"].items() if v["cut_seq"] is not None}
            # A kept (forgotten) device keeps its log's last, emptied, so it can never
            # store entries again at seqs it already used.
            c["logs"] = {u: dict(last=l["last"], entries={}) for u, l in c["logs"].items() if u in c["devices"]}
            if not c["devices"]: del self.channels[ch]
            else: c["generation"] = self.next_gen()

    def next_gen(self):
        self.gen += 1
        return self.gen

    def bucket(self, key, burst, interval, now):
        if key not in self.buckets: self.buckets[key] = Bucket(burst, interval, now)
        if not self.buckets[key].take(now): raise Fail("rate_limited")

    def total(self): return sum(self.used(c) for c in self.channels.values())
    @staticmethod
    def used(c): return sum(len(e[2]) for l in c["logs"].values() for e in l["entries"].values()) + sum(len(s[1]) for s in c["snaps"].values())

    def enrolled(self, ch, d):
        c = self.channels.get(ch)
        if c is None or d not in c["devices"] or c["devices"][d]["auth"] is None: raise Fail("not_enrolled")
        return c

    def reader(self, ch, d):
        """A pull's reader: enrolled, or forgotten (a forgotten target may never have
        enrolled in this channel, and must still read its Forget op)."""
        c = self.channels.get(ch)
        if c is None or d not in c["devices"] or (c["devices"][d]["auth"] is None and c["devices"][d]["cut_seq"] is None):
            raise Fail("not_enrolled")
        return c

    def fresh(self, ts, now):
        if not (now - self.cfg["window_ms"] <= ts <= now + self.cfg["window_ms"]): raise Fail("stale")

    def signed(self, ch, r):
        """Rebuild the signed message from the request's own fields (never trust r["signable"],
        which is only what the client happened to sign)."""
        v, d, ts = r["verb"], r["signer"], r["ts"]
        head_ = [bstr(ch), bstr(d), uint(ts)]
        msg = {
            "forget": lambda: arr([tstr("oh-relay-forget/v1")] + head_ + [bstr(r["target"]), ids(r["cut"]), bstr(r["auth"]), uint(r["cut_seq"])]),
            "append": lambda: arr([tstr("oh-relay-append/v1")] + head_ + [uint(r["first"]), arr([bstr(sha(e)) for e in r["envs"]])]),
            "snapshot": lambda: arr([tstr("oh-relay-snapshot/v1")] + head_ + [bstr(sha(r["env"])), pairs(r["covers"])]),
            "pull": lambda: arr([tstr("oh-relay-pull/v1")] + head_ + [uint(r["epoch"]), bstr(r["nonce"]), pairs(r["cursors"].items())]),
            "fetch_snapshot": lambda: arr([tstr("oh-relay-fetch-snapshot/v1")] + head_ + [uint(r["epoch"]), bstr(r["nonce"]), bstr(r["device"])]),
        }[v]()
        if not verify(d, msg, r["sig"]): raise Fail("bad_signature")

    def epoch_ok(self, r):
        if r["epoch"] != self.epoch: raise Fail("epoch", uint(self.epoch))

    def nonce(self, ch, r, now):
        live = {n: e for n, e in self.nonces.get((ch, r["signer"]), {}).items() if e >= now}
        if r["nonce"] in live: raise Fail("replay")
        if len(live) >= self.cfg["max_reader_nonces"]: raise Fail("rate_limited")
        live[r["nonce"]] = r["ts"] + self.cfg["window_ms"]
        self.nonces[(ch, r["signer"])] = live

    def device_rate(self, ch, d, now):
        """"Rate": the signer's device bucket, then the channel's."""
        self.bucket(("dev", ch, d), self.cfg["device_burst"], self.cfg["device_interval_ms"], now)
        self.bucket(("chan", ch), self.cfg["channel_burst"], self.cfg["channel_interval_ms"], now)

    def forgotten(self, c, d): return d in c["devices"] and c["devices"][d]["cut_seq"] is not None

    def prune(self, c, now):
        for u, l in c["logs"].items():
            cover = max([dict(s[2]).get(u, 0) for s in c["snaps"].values()], default=0)
            for seq in [s for s, e in l["entries"].items() if s <= cover and e[1] + self.cfg["retain_ms"] <= now]:
                del l["entries"][seq]

    def handle(self, ch, r, now):
        try:
            return 200, getattr(self, r["verb"])(ch, r, now)
        except Fail as f:
            return STATUS[f.code], err(f.code, *f.extra)

    def enroll(self, ch, r, now):
        cfg = self.cfg
        if channel_id(r["app"], r["household"]) != ch: raise Fail("channel_mismatch")
        if not verify(r["household"], enroll_auth_msg(r["app"], r["device"], r["label"]), r["auth"]): raise Fail("bad_auth")
        c = self.channels.get(ch)
        if c is not None and self.forgotten(c, r["device"]): raise Fail("forgotten")
        if c is None and len(self.channels) >= cfg["max_channels"]: raise Fail("quota")
        if c is None: self.bucket(("create",), cfg["create_burst"], cfg["create_interval_ms"], now)
        self.bucket(("enroll", ch), cfg["enroll_burst"], cfg["enroll_interval_ms"], now)
        if c is not None and r["device"] in c["devices"] and c["devices"][r["device"]]["auth"] is not None:
            return arr([tstr("ok"), uint(c["generation"])])
        if c is not None and len(c["devices"]) >= cfg["max_devices"]: raise Fail("quota")
        if c is None:
            c = self.channels[ch] = dict(app=r["app"], household=r["household"], generation=self.next_gen(), next_ord=1, last_write=now, devices={}, logs={}, snaps={})
        c["devices"][r["device"]] = dict(label=r["label"], auth=r["auth"], cut_seq=None, freeze_ord=None)
        c["last_write"] = now
        return arr([tstr("ok"), uint(c["generation"])])

    def forget(self, ch, r, now):
        c = self.enrolled(ch, r["signer"])
        self.fresh(r["ts"], now); self.signed(ch, r)
        if not verify(c["household"], forget_auth_msg(c["app"], r["target"], r["cut"]), r["auth"]): raise Fail("bad_auth")
        self.device_rate(ch, r["signer"], now)
        t = r["target"]
        if t not in c["devices"] and len(c["devices"]) >= self.cfg["max_devices"]: raise Fail("quota")
        d = c["devices"].setdefault(t, dict(label=None, auth=None, cut_seq=None, freeze_ord=None))
        if d["cut_seq"] is None:
            d["cut_seq"], d["freeze_ord"] = r["cut_seq"], c["next_ord"] - 1
        else:
            d["cut_seq"] = min(d["cut_seq"], r["cut_seq"])
        c["last_write"] = now
        return OK

    def append(self, ch, r, now):
        cfg = self.cfg
        if len(r["envs"]) > cfg["max_batch"] or any(len(e) > cfg["max_envelope"] for e in r["envs"]): raise Fail("too_large")
        c = self.enrolled(ch, r["signer"])
        self.fresh(r["ts"], now); self.signed(ch, r); self.device_rate(ch, r["signer"], now)
        if any(env_ref(e) is None for e in r["envs"]): raise Fail("bad_envelope")
        dev = c["devices"][r["signer"]]
        first, envs = r["first"], r["envs"]
        if dev["cut_seq"] is not None and first + len(envs) - 1 > dev["cut_seq"]: raise Fail("forgotten")
        log = c["logs"].get(r["signer"], dict(last=0, entries={}))
        last = log["last"]
        if first > last + 1: raise Fail("seq", uint(last))
        for i, e in enumerate(envs):
            s = first + i
            if s <= last and s in log["entries"] and log["entries"][s][2] != e: raise Fail("seq", uint(last))
        kept = copy.deepcopy(c["logs"])   # a failed request changes nothing: not even its prune
        self.prune(c, now)
        new = [(first + i, e) for i, e in enumerate(envs) if first + i > last]
        add = sum(len(e) for _, e in new)
        if self.used(c) + add > cfg["channel_quota"] or self.total() + add > cfg["max_total_bytes"]:
            c["logs"] = kept
            raise Fail("quota")
        for s, e in new:
            log["entries"][s] = (c["next_ord"], now, e)
            c["next_ord"] += 1
            log["last"] = s
        if new: c["logs"][r["signer"]] = log
        c["last_write"] = now
        return arr([tstr("ok"), uint(log["last"])])

    def snapshot(self, ch, r, now):
        cfg = self.cfg
        if len(r["env"]) > cfg["max_snapshot"] or len(r["covers"]) > cfg["max_devices"]: raise Fail("too_large")
        c = self.enrolled(ch, r["signer"])
        self.fresh(r["ts"], now); self.signed(ch, r)
        if self.forgotten(c, r["signer"]): raise Fail("forgotten")
        self.device_rate(ch, r["signer"], now)
        if env_ref(r["env"]) is None: raise Fail("bad_envelope")
        kept = copy.deepcopy(c["logs"])
        self.prune(c, now)
        old = len(c["snaps"][r["signer"]][1]) if r["signer"] in c["snaps"] else 0
        add = len(r["env"]) - old
        if self.used(c) + add > cfg["channel_quota"] or self.total() + add > cfg["max_total_bytes"]:
            c["logs"] = kept
            raise Fail("quota")
        c["snaps"][r["signer"]] = (now, r["env"], r["covers"])
        c["last_write"] = now
        return OK

    def pull(self, ch, r, now):
        cfg = self.cfg
        if len(r["cursors"]) > cfg["max_devices"]: raise Fail("too_large")
        c = self.reader(ch, r["signer"])
        self.fresh(r["ts"], now); self.signed(ch, r); self.epoch_ok(r); self.nonce(ch, r, now); self.device_rate(ch, r["signer"], now)
        me = c["devices"][r["signer"]]
        frozen = me["cut_seq"] is not None
        logs, n, size, more = [], 0, 0, False
        for u in sorted(c["logs"]):
            l = c["logs"][u]
            first = min(l["entries"]) if l["entries"] else l["last"] + 1
            got = []
            for s in sorted(l["entries"]):
                o, _, e = l["entries"][s]
                if s <= r["cursors"].get(u, 0) or (frozen and o > me["freeze_ord"]): continue
                if more or n >= cfg["max_pull_entries"] or (n > 0 and size + len(e) > cfg["max_pull_bytes"]):
                    more = True
                    continue
                got.append(arr([uint(s), bstr(e)])); n += 1; size += len(e)
            logs.append(arr([bstr(u), uint(first), arr(got)]))
        snaps = [] if frozen else [arr([bstr(d), bstr(env_ref(s[1])), pairs(s[2])]) for d, s in sorted(c["snaps"].items())]
        return arr([tstr("ok"), uint(c["generation"]), arr(logs), arr(snaps), TRUE if more else FALSE])

    def fetch_snapshot(self, ch, r, now):
        c = self.enrolled(ch, r["signer"])
        self.fresh(r["ts"], now); self.signed(ch, r); self.epoch_ok(r); self.nonce(ch, r, now)
        if self.forgotten(c, r["signer"]): raise Fail("forgotten")
        self.device_rate(ch, r["signer"], now)
        if r["device"] not in c["snaps"]: raise Fail("no_snapshot")
        _, env, covers = c["snaps"][r["device"]]
        return arr([tstr("ok"), bstr(env), pairs(covers)])

    def dump(self):
        out = []
        for ch in sorted(self.channels):
            c = self.channels[ch]
            devs = [arr([bstr(d), opt(v["label"], tstr), opt(v["auth"], bstr), opt(v["cut_seq"], uint), opt(v["freeze_ord"], uint)])
                    for d, v in sorted(c["devices"].items())]
            logs = [arr([bstr(u), uint(l["last"]), arr([arr([uint(s), uint(o), uint(at), bstr(sha(e))]) for s, (o, at, e) in sorted(l["entries"].items())])])
                    for u, l in sorted(c["logs"].items())]
            snaps = [arr([bstr(d), uint(at), bstr(sha(e)), pairs(cv)]) for d, (at, e, cv) in sorted(c["snaps"].items())]
            out.append(arr([bstr(ch), tstr(c["app"]), bstr(c["household"]), uint(c["generation"]), uint(c["next_ord"]), uint(c["last_write"]), arr(devs), arr(logs), arr(snaps)]))
        return arr(out)

# ---- cases ----
cases = []

class Case:
    def __init__(self, name, note, **config):
        self.name, self.note, self.config = name, note, config
        self.relay = Relay({**DEFAULT, **config})
        self.steps = []

    def req(self, now, req, expect, ch=CH):
        body, r = req
        status, resp = self.relay.handle(ch, r, now)
        self._step(now, "POST", f"/v1/{ch.hex()}/{VERB_PATH[r['verb']]}", body, status, resp, expect)
        return resp

    def sweep(self, now):
        self.relay.sweep(now)
        self.steps.append(dict(now=now, sweep=True))

    def restart(self, now):
        self.relay.restart()
        self.steps.append(dict(now=now, restart=True))

    def raw(self, now, method, path, body, expect):
        """A request the model does not parse: malformed bodies, paths and methods.
        The expected answer is stated, not modelled."""
        if expect == "healthz":
            status, resp = 200, b"ok"
        else:
            status, resp = STATUS[expect], err(expect)
        self._step(now, method, path, body, status, resp, expect)

    def _step(self, now, method, path, body, status, resp, expect):
        got = "ok" if status == 200 else resp[6:6 + resp[5] - 0x60].decode()
        if expect != "healthz" and got != expect:
            raise SystemExit(f"{self.name} step {len(self.steps)}: model answered {got}, the case expects {expect}")
        self.steps.append(dict(now=now, method=method, path=path, body=body.hex(), status=status, response=resp.hex(), expect=expect))

    def done(self):
        cases.append(dict(name=self.name, note=self.note, config=self.config, steps=self.steps,
                          store_digest=sha(self.relay.dump()).hex()))

P = lambda ch, verb: f"/v1/{ch.hex()}/{verb}"
NONCE = lambda n: bytes([n]) * 16

# 1. The happy path, retries and sequence errors.
c = Case("append_and_pull", "two devices enrol; A appends; B pulls from cursors; retries are idempotent; gaps and rewrites are seq errors")
t = T0
c.req(t, enroll_req(APP, EK, pub(A), "Kitchen tablet"), "ok")
c.req(t, enroll_req(APP, EK, pub(B), "Dad's phone"), "ok")
c.req(t, enroll_req(APP, EK, pub(B), "Dad's phone"), "ok")  # idempotent
e1, e2, e3 = envelope(REF(1), tag=1), envelope(REF(2), tag=2), envelope(REF(3), tag=3)
c.req(t + 1, append_req(CH, A, 1, [e1, e2], t + 1), "ok")
c.req(t + 2, append_req(CH, A, 1, [e1, e2], t + 2), "ok")             # retry
c.req(t + 3, append_req(CH, A, 2, [e2, e3], t + 3), "ok")             # overlap then new
c.req(t + 4, append_req(CH, A, 5, [e1], t + 4), "seq")                # gap
c.req(t + 5, append_req(CH, A, 3, [e1], t + 5), "seq")                # rewrite of a held entry
c.req(t + 6, pull_req(CH, B, t + 6, NONCE(1), []), "ok")
c.req(t + 7, pull_req(CH, B, t + 7, NONCE(2), [(pub(A), 2)]), "ok")
c.req(t + 8, append_req(CH, B, 1, [envelope(REF(9), tag=9)], t + 8), "ok")
c.req(t + 9, pull_req(CH, A, t + 9, NONCE(3), [(pub(A), 3), (pub(B), 0)]), "ok")
c.done()

# 2. Enrolment.
c = Case("enroll_checks", "channel binding, household signature, shape, device cap, forgotten devices", max_devices=2)
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
c.req(t, enroll_req(APP, FK, pub(B), "B"), "channel_mismatch")                 # another household's channel id
c.req(t, enroll_req(APP, EK, pub(B), "B", auth_sk=FK), "bad_auth")             # signed by another household
c.req(t, enroll_req("peckish", EK, pub(B), "B"), "channel_mismatch")           # another app
c.req(t, enroll_req(APP, EK, pub(B), "B"), "ok")
c.req(t, enroll_req(APP, EK, pub(C), "C"), "quota")                            # max_devices
c.req(t, enroll_req(APP, FK, pub(C), "C"), "ok", ch=FCH)                       # the other household's own channel
body, _ = enroll_req(APP, EK, pub(C), "C")
c.raw(t, "POST", P(CH, "enroll"), arr([tstr(APP), bstr(HH), bstr(pub(C)), tstr(""), bstr(bytes(64))]), "bad_request")  # empty label
c.raw(t, "POST", P(CH, "enroll"), arr([tstr("Lullaby"), bstr(HH), bstr(pub(C)), tstr("C"), bstr(bytes(64))]), "bad_request")  # bad app domain
c.raw(t, "POST", P(CH, "enroll"), body[:-1], "bad_request")                    # truncated
c.raw(t, "POST", P(CH, "enroll"), body + b"\x00", "bad_request")               # trailing byte
c.done()

# 3. Signed-request checks.
c = Case("request_auth", "unenrolled, unknown channel, stale both ways, bad and cross-verb signatures, junk envelopes, batch and size limits", max_batch=2, max_envelope=200)
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
c.req(t + 1, append_req(CH, C, 1, [e1], t + 1), "not_enrolled")
c.req(t + 1, append_req(FCH, A, 1, [e1], t + 1), "not_enrolled", ch=FCH)       # no such channel
c.req(t + 1, append_req(CH, A, 1, [e1], t + 1 - W - 1), "stale")
c.req(t + 1, append_req(CH, A, 1, [e1], t + 1 + W + 1), "stale")
c.req(t + 1, append_req(CH, A, 1, [e1], t + 1 - W), "ok")                     # the window's edge
c.req(t + 2, append_req(CH, A, 2, [e2], t + 2, sign_sk=B), "bad_signature")
pull_signable = arr([tstr("oh-relay-pull/v1"), bstr(CH), bstr(pub(A)), uint(t + 2), bstr(NONCE(1)), pairs([])])
c.req(t + 2, append_req(CH, A, 2, [e2], t + 2, signable=pull_signable), "bad_signature")   # a pull signature on an append
other_ch = arr([tstr("oh-relay-append/v1"), bstr(FCH), bstr(pub(A)), uint(t + 2), uint(2), arr([bstr(sha(e2))])])
c.req(t + 2, append_req(CH, A, 2, [e2], t + 2, signable=other_ch), "bad_signature")        # signed for another channel
c.req(t + 3, append_req(CH, A, 2, [b"\x01\x02\x03"], t + 3), "bad_envelope")
c.req(t + 3, append_req(CH, A, 2, [envelope(None)], t + 3), "bad_envelope")               # no ref
c.req(t + 3, append_req(CH, A, 2, [e2, e3, e1], t + 3), "too_large")                       # max_batch
c.req(t + 3, append_req(CH, A, 2, [envelope(REF(4), size=200)], t + 3), "too_large")        # max_envelope
c.req(t + 3, append_req(CH, A, 2, [envelope(REF(5), size=120)], t + 3), "ok")
_, r = append_req(CH, A, 3, [e3], t + 4)
c.raw(t + 4, "POST", P(CH, "append"), arr([bstr(pub(A)), uint(0), arr([bstr(e3)]), uint(t + 4), bstr(r["sig"])]), "bad_request")  # seq 0
c.raw(t + 4, "POST", P(CH, "append"), arr([bstr(pub(A)), uint(3), arr([]), uint(t + 4), bstr(r["sig"])]), "bad_request")          # empty batch
c.raw(t + 4, "POST", P(CH, "append"), arr([bstr(pub(A)), b"\x18\x03", arr([bstr(e3)]), uint(t + 4), bstr(r["sig"])]), "bad_request")  # non-shortest uint
c.done()

# 4. Forget another device.
fe = envelope(REF(100), tag=100)   # A's sealed Forget op
c = Case("forget_other", "A appends its Forget op, then records the Forget of B; B's later writes are refused; B reads the channel frozen at the Forget, which holds the Forget op")
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
c.req(t, enroll_req(APP, EK, pub(B), "B"), "ok")
b1 = envelope(REF(20), tag=20)
c.req(t + 1, append_req(CH, B, 1, [b1], t + 1), "ok")
c.req(t + 2, append_req(CH, A, 1, [e1, fe], t + 2), "ok")
c.req(t + 3, forget_req(CH, pub(B), [REF(20)], 1, C, t + 3), "not_enrolled")                   # poster not enrolled
c.req(t + 3, forget_req(CH, pub(B), [REF(20)], 1, A, t + 3, auth_sk=FK), "bad_auth")
c.req(t + 3, forget_req(CH, pub(B), [REF(20)], 1, A, t + 3, sign_sk=B), "bad_signature")
c.req(t + 3, forget_req(CH, pub(B), [REF(20)], 1, A, t + 3 + W + 1), "stale")
c.req(t + 3, forget_req(CH, pub(B), [REF(20)], 1, A, t + 3), "ok")
c.req(t + 4, append_req(CH, B, 2, [envelope(REF(21), tag=21)], t + 4), "forgotten")
c.req(t + 4, append_req(CH, B, 1, [b1], t + 4), "ok")                                           # a retry within the cut
c.req(t + 5, append_req(CH, A, 3, [e3], t + 5), "ok")                                           # after the Forget
r_ = c.req(t + 6, pull_req(CH, B, t + 6, NONCE(1), []), "ok")                                   # frozen: no e3
assert fe in r_ and e1 in r_ and b1 in r_ and e3 not in r_
c.req(t + 6, snapshot_req(CH, A, envelope(REF(30), kind=1, tag=30), [(pub(A), 2)], t + 6), "ok")
r_ = c.req(t + 7, pull_req(CH, B, t + 7, NONCE(2), []), "ok")                                   # frozen: no snapshots
assert r_.endswith(arr([]) + FALSE)
c.req(t + 7, fetch_req(CH, B, t + 7, NONCE(3), pub(A)), "forgotten")
c.req(t + 7, snapshot_req(CH, B, envelope(REF(31), kind=1), [], t + 7), "forgotten")
c.req(t + 7, enroll_req(APP, EK, pub(B), "B again"), "forgotten")
c.req(t + 8, forget_req(CH, pub(B), [], 0, A, t + 8), "ok")                                     # a second Forget lowers cut_seq
c.req(t + 9, append_req(CH, B, 1, [b1], t + 9), "forgotten")
c.req(t + 9, forget_req(CH, pub(C), [], 0, A, t + 9), "ok")                                     # a device never enrolled
c.req(t + 9, enroll_req(APP, EK, pub(C), "C"), "forgotten")
c.req(t + 10, pull_req(CH, A, t + 10, NONCE(4), [(pub(A), 1)]), "ok")
c.req(t + 10, append_req(CH, A, 4, [envelope(REF(101), tag=101)], t + 10), "ok")         # after C's Forget
r_ = c.req(t + 11, pull_req(CH, C, t + 11, NONCE(1), []), "ok")                          # never enrolled, forgotten: reads frozen
assert fe in r_ and e3 in r_ and envelope(REF(101), tag=101) not in r_
c.req(t + 11, fetch_req(CH, C, t + 11, NONCE(2), pub(A)), "not_enrolled")
c.done()

# 5. A device forgets itself and hands over.
c = Case("forget_self_handover", "B records its own Forget before its last ops reach the relay; the ops up to cut_seq still go up, nothing after")
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
c.req(t, enroll_req(APP, EK, pub(B), "B"), "ok")
c.req(t + 1, append_req(CH, B, 1, [envelope(REF(40), tag=40)], t + 1), "ok")
c.req(t + 2, forget_req(CH, pub(B), [REF(42)], 3, B, t + 2), "ok")
c.req(t + 3, append_req(CH, B, 2, [envelope(REF(41), tag=41), envelope(REF(42), tag=42)], t + 3), "ok")
c.req(t + 4, append_req(CH, B, 4, [envelope(REF(43), tag=43)], t + 4), "forgotten")
c.req(t + 5, forget_req(CH, pub(B), [REF(42)], 3, B, t + 5), "ok")                              # a forgotten device may still post its Forget
c.req(t + 6, pull_req(CH, A, t + 6, NONCE(1), []), "ok")
c.done()

# 6. Snapshots, pruning and replay.
R = 10 * DAY
c = Case("snapshot_prune", "a snapshot covering A's first two entries lets them go once retained long enough; a replayed old append stores nothing", retain_ms=R)
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
c.req(t, enroll_req(APP, EK, pub(B), "B"), "ok")
old = append_req(CH, A, 1, [e1, e2, e3], t + 1)
c.req(t + 1, old, "ok")
c.req(t + 2, fetch_req(CH, A, t + 2, NONCE(1), pub(B)), "no_snapshot")
snap = envelope(REF(50), kind=1, size=64, tag=50)
c.req(t + 3, snapshot_req(CH, B, envelope(REF(50), kind=1, tag=49), [(pub(A), 2)], t + 3), "ok")
c.req(t + 4, snapshot_req(CH, B, snap, [(pub(A), 2)], t + 4), "ok")                             # replaces
c.req(t + R, append_req(CH, B, 1, [envelope(REF(51))], t + R), "ok")                             # prunes: not old enough yet
c.req(t + 1 + R, append_req(CH, B, 2, [envelope(REF(52))], t + 1 + R), "ok")                     # prunes A's seq 1 and 2
r_ = c.req(t + 1 + R, pull_req(CH, A, t + 1 + R, NONCE(2), []), "ok")                           # first = 3
assert arr([bstr(pub(A)), uint(3), arr([arr([uint(3), bstr(e3)])])]) in r_
c.raw(t + 2 + R, "POST", P(CH, "append"), old[0], "stale")                                       # the captured request, replayed
c.req(t + 2 + R, append_req(CH, A, 1, [e1, e2], t + 2 + R), "ok")                               # re-signed: pruned seqs are skipped
c.req(t + 3 + R, fetch_req(CH, A, t + 3 + R, NONCE(3), pub(B)), "ok")
c.req(t + 3 + R, snapshot_req(CH, A, envelope(None, kind=1), [], t + 3 + R), "bad_envelope")
c.done()

# 7. Storage caps.
c = Case("quotas", "per-channel bytes, relay-wide bytes, relay-wide channel count", channel_quota=400, max_total_bytes=500, max_channels=2, max_snapshot=120)
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
c.req(t + 1, append_req(CH, A, 1, [envelope(REF(60), size=100), envelope(REF(61), size=100)], t + 1), "ok")   # 2 x 165 bytes
c.req(t + 2, append_req(CH, A, 3, [envelope(REF(62), size=10)], t + 2), "quota")
c.req(t + 2, snapshot_req(CH, A, envelope(REF(63), kind=1, size=80), [], t + 2), "too_large")
c.req(t, enroll_req(APP, FK, pub(B), "B"), "ok", ch=FCH)
c.req(t + 3, append_req(FCH, B, 1, [envelope(REF(64), size=100)], t + 3), "ok", ch=FCH)
c.req(t + 3, append_req(FCH, B, 2, [envelope(REF(65), size=100)], t + 3), "quota", ch=FCH)                   # relay-wide bytes
third = Ed25519PrivateKey.from_private_bytes(bytes([7]) * 32)
TCH = channel_id(APP, pub(third))
c.req(t, enroll_req(APP, third, pub(C), "C"), "quota", ch=TCH)                                              # max_channels
c.done()

# 8. Rate limits.
c = Case("rate_limits", "the device bucket refills one token per interval; the enrol and creation buckets", device_burst=2, device_interval_ms=1000, enroll_burst=2, create_burst=1, create_interval_ms=60_000)
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
c.req(t + 1, pull_req(CH, A, t + 1, NONCE(1), []), "ok")
c.req(t + 2, pull_req(CH, A, t + 2, NONCE(2), []), "ok")
c.req(t + 3, pull_req(CH, A, t + 3, NONCE(3), []), "rate_limited")
c.req(t + 1000, pull_req(CH, A, t + 1000, NONCE(4), []), "rate_limited")      # 999 ms after the bucket's clock
c.req(t + 1001, pull_req(CH, A, t + 1001, NONCE(5), []), "ok")
c.req(t + 1002, enroll_req(APP, EK, pub(B), "B"), "ok")
c.req(t + 1003, enroll_req(APP, EK, pub(C), "C"), "rate_limited")             # enrol burst 2 (A, B)
c.req(t + 1004, enroll_req(APP, FK, pub(C), "C"), "rate_limited", ch=FCH)     # creation burst 1
c.req(t + 61_000, enroll_req(APP, FK, pub(C), "C"), "ok", ch=FCH)
c.done()

# 9. Nonces.
c = Case("nonce_replay", "a read's nonce is used once within the window; the same nonce with a new ts is a new request")
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
p = pull_req(CH, A, t + 1, NONCE(9), [])
c.req(t + 1, p, "ok")
c.req(t + 2, p, "replay")
c.req(t + 3, fetch_req(CH, A, t + 3, NONCE(9), pub(A)), "replay")              # another verb, same (channel, reader, nonce)
f = fetch_req(CH, A, t + 4, NONCE(8), pub(A))
c.req(t + 4, f, "no_snapshot")
c.req(t + 5, f, "replay")                                                     # the nonce was used even though the answer was an error
c.req(t + 2 + W, p, "stale")
c.done()

# 10. Paging. Uploaders come in id order: B (3d40...) before A (d75a...).
c = Case("pull_paging", "a byte budget the first entry may exceed alone, then the entry budget; more is set until the last page", max_pull_entries=3, max_pull_bytes=400)
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
c.req(t, enroll_req(APP, EK, pub(B), "B"), "ok")
big = envelope(REF(70), size=500)
c.req(t + 1, append_req(CH, B, 1, [big], t + 1), "ok")
smalls = [envelope(REF(71 + i), size=50) for i in range(4)]
c.req(t + 2, append_req(CH, A, 1, smalls, t + 2), "ok")
r_ = c.req(t + 3, pull_req(CH, A, t + 3, NONCE(1), []), "ok")                    # B's big entry alone
assert big in r_ and smalls[0] not in r_ and r_.endswith(TRUE)
r_ = c.req(t + 4, pull_req(CH, A, t + 4, NONCE(2), [(pub(B), 1)]), "ok")         # three of A's (entry budget)
assert smalls[2] in r_ and smalls[3] not in r_ and r_.endswith(TRUE)
r_ = c.req(t + 5, pull_req(CH, A, t + 5, NONCE(3), [(pub(A), 3), (pub(B), 1)]), "ok")
assert smalls[3] in r_ and smalls[2] not in r_ and r_.endswith(FALSE)
c.done()

# 12. List caps.
c = Case("list_caps", "covers and cursors may list at most max_devices uploaders: covers are stored outside the byte quota", max_devices=2)
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
three = [(pub(A), 1), (pub(B), 1), (pub(C), 1)]
c.req(t + 1, snapshot_req(CH, A, envelope(REF(80), kind=1), three, t + 1), "too_large")
c.req(t + 1, snapshot_req(CH, A, envelope(REF(80), kind=1), three[:2], t + 1), "ok")
c.req(t + 2, pull_req(CH, A, t + 2, NONCE(1), three), "too_large")
c.req(t + 2, pull_req(CH, A, t + 2, NONCE(2), three[:2]), "ok")
c.done()

# 11. Malformed requests and paths.
c = Case("malformed", "paths, methods and bodies the handler refuses before any check")
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
good, r = pull_req(CH, A, t + 1, NONCE(1), [])
c.raw(t + 1, "GET", P(CH, "pull"), b"", "method")
c.raw(t + 1, "PUT", P(CH, "append"), good, "method")
c.raw(t + 1, "POST", f"/v1/{CH.hex().upper()}/pull", good, "not_found")
c.raw(t + 1, "POST", f"/v1/{CH.hex()[:-2]}/pull", good, "not_found")
c.raw(t + 1, "POST", P(CH, "delete"), good, "not_found")
c.raw(t + 1, "POST", "/v2/" + CH.hex() + "/pull", good, "not_found")
c.raw(t + 1, "POST", P(CH, "pull") + "/", good, "not_found")
c.raw(t + 1, "GET", "/healthz", b"", "healthz")
c.raw(t + 1, "POST", P(CH, "pull"), b"", "bad_request")
c.raw(t + 1, "POST", P(CH, "pull"), b"\xa0", "bad_request")                      # a map, not an array
c.raw(t + 1, "POST", P(CH, "pull"), arr([bstr(pub(A)), uint(t + 1), uint(1), bstr(bytes(15)), pairs([]), bstr(r["sig"])]), "bad_request")  # 15-byte nonce
hi, lo = max(pub(A), pub(B)), min(pub(A), pub(B))
unsorted = arr([arr([bstr(hi), uint(0)]), arr([bstr(lo), uint(0)])])
c.raw(t + 1, "POST", P(CH, "pull"), arr([bstr(pub(A)), uint(t + 1), uint(1), bstr(NONCE(1)), unsorted, bstr(r["sig"])]), "bad_request")
c.raw(t + 1, "POST", P(CH, "pull"), b"\x81" * 10_000, "bad_request")             # deep nesting
c.raw(t + 1, "POST", P(CH, "forget"), arr([bstr(pub(B)), arr([bstr(max(REF(1), REF(2))), bstr(min(REF(1), REF(2)))]), bstr(bytes(64)), uint(0), bstr(pub(A)), uint(t), bstr(bytes(64))]), "bad_request")  # unsorted cut
c.req(t + 2, pull_req(CH, A, t + 2, NONCE(2), []), "ok")
c.done()

# 13. Whole u64 values: covers and cut_seq above 2^63 - 1 are stored and echoed whole, and
# a batch whose last seq passes 2^64 - 1 is above any cut.
U64 = (1 << 64) - 1
c = Case("u64_edges", "covers and cut_seq above 2^63 - 1 are kept whole and echoed; a cover of 2^64 - 1 covers everything; a batch past 2^64 - 1 is above any cut", retain_ms=R)
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
c.req(t, enroll_req(APP, EK, pub(B), "B"), "ok")
c.req(t + 1, append_req(CH, A, 1, [e1, e2], t + 1), "ok")
c.req(t + 2, snapshot_req(CH, A, envelope(REF(90), kind=1, tag=90), [(pub(A), U64), (pub(B), 1 << 63)], t + 2), "ok")
r_ = c.req(t + 3, pull_req(CH, B, t + 3, NONCE(1), [(pub(A), U64)]), "ok")
assert uint(U64) in r_ and uint(1 << 63) in r_
c.req(t + 3, fetch_req(CH, B, t + 3, NONCE(2), pub(A)), "ok")
c.req(t + 4, forget_req(CH, pub(B), [], U64, A, t + 4), "ok")
c.req(t + 5, append_req(CH, B, U64, [envelope(REF(91)), envelope(REF(92))], t + 5), "forgotten")
c.req(t + 1 + R, append_req(CH, A, 3, [e3], t + 1 + R), "ok")                           # prunes A's 1 and 2
r_ = c.req(t + 2 + R, pull_req(CH, A, t + 2 + R, NONCE(3), []), "ok")
assert arr([bstr(pub(A)), uint(3), arr([arr([uint(3), bstr(e3)])])]) in r_
c.done()

# 14. A request that fails changes nothing in the store, not even the prune it ran.
c = Case("failed_write_changes_nothing", "an append refused for quota after pruning leaves the pruned entries in place; the next append that fits prunes them", channel_quota=320, retain_ms=R)
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
c.req(t + 1, append_req(CH, A, 1, [envelope(REF(93), size=40), envelope(REF(94), size=40)], t + 1), "ok")   # 2 x 105 bytes
c.req(t + 2, snapshot_req(CH, A, envelope(REF(95), kind=1, size=0), [(pub(A), 2)], t + 2), "ok")          # 64 bytes
c.req(t + 2 + R, append_req(CH, A, 3, [envelope(REF(96), size=200)], t + 2 + R), "quota")                 # 64 + 265 > 320 even pruned
r_ = c.req(t + 3 + R, pull_req(CH, A, t + 3 + R, NONCE(1), []), "ok")                                     # 1 and 2 still held
assert arr([uint(1), bstr(envelope(REF(93), size=40))]) in r_
c.req(t + 4 + R, append_req(CH, A, 3, [envelope(REF(97), size=0)], t + 4 + R), "ok")                      # fits; prunes
r_ = c.req(t + 5 + R, pull_req(CH, A, t + 5 + R, NONCE(2), []), "ok")
assert arr([uint(1), bstr(envelope(REF(93), size=40))]) not in r_
c.done()

# 15. Strict signatures: small-order keys never verify, though plain Ed25519 accepts them.
c = Case("strict_signatures", "a small-order household key (canonical or not) cannot enrol with a forged auth; an enrolled small-order device key cannot sign")
c.req(t, enroll_req(APP, IDENTITY, pub(A), "A"), "bad_auth", ch=channel_id(APP, IDENTITY.enc))
c.req(t, enroll_req(APP, IDENTITY_NC, pub(A), "A"), "bad_auth", ch=channel_id(APP, IDENTITY_NC.enc))
c.req(t, enroll_req(APP, EK, IDENTITY.enc, "odd key"), "ok")                        # the household may enrol any key
c.req(t + 1, append_req(CH, IDENTITY, 1, [e1], t + 1), "bad_signature")
c.req(t + 1, pull_req(CH, IDENTITY, t + 1, NONCE(1), []), "bad_signature")
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
c.req(t + 2, append_req(CH, A, 1, [e1], t + 2, sign_sk=IDENTITY), "bad_signature")   # R = identity under a good key
c.done()

# 16. Labels are canonical text: NFC and at most 128 bytes.
c = Case("labels", "a label must be NFC (dCBOR text) and 1 to 128 bytes")
c.req(t, enroll_req(APP, EK, pub(A), "caf\u00e9"), "ok")
c.req(t, enroll_req(APP, EK, pub(B), "x" * 128), "ok")
auth = EK.sign(enroll_auth_msg(APP, pub(C), "cafe\u0301"))
c.raw(t, "POST", P(CH, "enroll"), arr([tstr(APP), bstr(HH), bstr(pub(C)), tstr("cafe\u0301"), bstr(auth)]), "bad_request")   # decomposed
auth = EK.sign(enroll_auth_msg(APP, pub(C), "x" * 129))
c.raw(t, "POST", P(CH, "enroll"), arr([tstr(APP), bstr(HH), bstr(pub(C)), tstr("x" * 129), bstr(auth)]), "bad_request")
c.done()

# 17. Routing and nonce expiry edges.
c = Case("edges", "only GET /healthz is special; a nonce is live while ts + W >= now")
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
c.raw(t, "POST", "/healthz", b"", "not_found")
c.raw(t, "HEAD", "/healthz", b"", "not_found")
c.req(t + 1, pull_req(CH, A, t, NONCE(7), []), "ok")                                # nonce 7 live until t + W
c.req(t + W, pull_req(CH, A, t + W, NONCE(7), []), "replay")                        # now = t + W: still live
c.req(t + W + 1, pull_req(CH, A, t + W + 1, NONCE(7), []), "ok")                    # expired: a new request
c.done()

# 18. The epoch: reads name it, a restart moves it on, so no read survives a restart.
c = Case("epoch_restart", "reads carry the relay's epoch; a restart keeps the store, empties the nonces, refills the buckets and moves the epoch on, so a read captured before it is refused", device_burst=2, device_interval_ms=60_000)
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
c.req(t + 1, append_req(CH, A, 1, [e1], t + 1), "ok")
c.req(t + 2, pull_req(CH, A, t + 2, NONCE(1), [], epoch=0), "epoch")               # a client that does not know it yet
c.req(t + 2, pull_req(CH, A, t + 2, NONCE(1), [], epoch=2), "epoch")
captured = pull_req(CH, A, t + 2, NONCE(1), [])
c.req(t + 2, captured, "ok")                                                       # the wrong-epoch tries used no nonce
c.req(t + 3, pull_req(CH, A, t + 3, NONCE(2), []), "rate_limited")                 # burst 2: the append and the pull
c.restart(t + 4)
c.req(t + 4, captured, "epoch")                                                    # the captured read, replayed after the restart
c.req(t + 4, fetch_req(CH, A, t + 4, NONCE(3), pub(A)), "epoch")
r_ = c.req(t + 4, pull_req(CH, A, t + 4, NONCE(1), [], epoch=2), "ok")             # a full bucket, an empty nonce table, the store kept
assert e1 in r_
c.req(t + 5, pull_req(CH, A, t + 4, NONCE(1), [], epoch=2), "replay")
c.req(t + 6, append_req(CH, A, 2, [e2], t + 6), "ok")                             # the refused reads above took no token
c.req(t + 6, pull_req(CH, A, t + 6, NONCE(5), [], epoch=2), "rate_limited")
c.restart(t + 7)
c.restart(t + 8)
c.req(t + 9, fetch_req(CH, A, t + 9, NONCE(4), pub(A), epoch=4), "no_snapshot")
c.done()

# 19. The per-reader nonce cap.
c = Case("reader_nonce_cap", "a reader holds at most max_reader_nonces live nonces; a nonce refused for the cap is not recorded; other readers are unaffected", max_reader_nonces=2)
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
c.req(t, enroll_req(APP, EK, pub(B), "B"), "ok")
c.req(t, enroll_req(APP, FK, pub(C), "C"), "ok", ch=FCH)
c.req(t + 1, pull_req(CH, A, t + 1, NONCE(1), []), "ok")
c.req(t + 1000, pull_req(CH, A, t + 1000, NONCE(2), []), "ok")
n3 = pull_req(CH, A, t + 2000, NONCE(3), [])
c.req(t + 2000, n3, "rate_limited")
c.req(t + 2000, fetch_req(CH, A, t + 2000, NONCE(4), pub(A)), "rate_limited")      # the cap is per reader, over both read verbs
c.req(t + 2000, pull_req(CH, B, t + 2000, NONCE(3), []), "ok")                    # another reader in the channel
c.req(t + 2000, pull_req(FCH, C, t + 2000, NONCE(3), []), "ok", ch=FCH)           # another household
c.req(t + 2 + W, n3, "ok")                                                        # nonce 1 expired; nonce 3 was never recorded
c.req(t + 2 + W, pull_req(CH, A, t + 2 + W, NONCE(5), []), "rate_limited")         # nonces 2 and 3 live
c.done()

# 20. The channel bucket.
c = Case("channel_rate", "the channel bucket is shared by the channel's devices and taken after the device bucket; a channel refusal keeps the device token", channel_burst=3, channel_interval_ms=1000, device_burst=2, device_interval_ms=60_000)
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
c.req(t, enroll_req(APP, EK, pub(B), "B"), "ok")
c.req(t, enroll_req(APP, FK, pub(C), "C"), "ok", ch=FCH)
c.req(t + 1, pull_req(CH, A, t + 1, NONCE(1), []), "ok")
c.req(t + 1, pull_req(CH, A, t + 1, NONCE(2), []), "ok")
c.req(t + 1, pull_req(CH, B, t + 1, NONCE(3), []), "ok")
c.req(t + 1, append_req(CH, B, 1, [e1], t + 1), "rate_limited")                    # the channel's three are gone
c.req(t + 1, pull_req(FCH, C, t + 1, NONCE(1), []), "ok", ch=FCH)                 # another channel's bucket
c.req(t + 1001, pull_req(CH, B, t + 1001, NONCE(4), []), "rate_limited")           # B's device bucket: the refused append took its token
c.req(t + 1001, pull_req(CH, A, t + 1001, NONCE(5), []), "rate_limited")           # A's device bucket is empty too
c.req(t + 1001, forget_req(CH, pub(C), [], 0, B, t + 1001), "rate_limited")
c.req(t + 60_001, append_req(CH, A, 1, [e1], t + 60_001), "ok")                   # both refilled
c.done()

# 21. Idle channels expire at the sweep; forgotten devices stay as tombstones.
I = 10 * DAY
c = Case("idle_expiry", "a channel with no write for idle_ms is expired by the sweep; forgotten records survive it, so a forgotten device cannot enrol again; a channel with none is deleted and frees its slot", idle_ms=I, max_channels=2)
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")
c.req(t, enroll_req(APP, EK, pub(B), "B"), "ok")
c.req(t + 1, append_req(CH, A, 1, [e1, fe], t + 1), "ok")
c.req(t + 1, append_req(CH, B, 1, [b1], t + 1), "ok")
c.req(t + 2, snapshot_req(CH, A, envelope(REF(110), kind=1), [], t + 2), "ok")
c.req(t + 3, forget_req(CH, pub(B), [REF(20)], 1, A, t + 3), "ok")                # last write in CH: t + 3
c.req(t + 4, enroll_req(APP, FK, pub(C), "C"), "ok", ch=FCH)                      # FCH: t + 4
third = Ed25519PrivateKey.from_private_bytes(bytes([7]) * 32)
TCH = channel_id(APP, pub(third))
c.req(t + 5, enroll_req(APP, third, pub(C), "C"), "quota", ch=TCH)                # max_channels
c.sweep(t + 2 + I)                                                                # neither is idle yet
c.req(t + 2 + I, pull_req(CH, A, t + 2 + I, NONCE(1), []), "ok")                  # a read is not a write
c.sweep(t + 3 + I)                                                                # CH expires; FCH (t + 4) not yet
c.req(t + 3 + I, pull_req(CH, A, t + 3 + I, NONCE(2), []), "not_enrolled")
c.req(t + 3 + I, enroll_req(APP, EK, pub(B), "B"), "forgotten")                    # the tombstone holds
c.req(t + 3 + I, append_req(CH, B, 1, [envelope(REF(21), tag=21)], t + 3 + I), "ok")  # B's log kept its last: seq 1 is skipped, not stored
c.req(t + 3 + I, enroll_req(APP, third, pub(C), "C"), "quota", ch=TCH)            # CH still holds its slot
c.sweep(t + 4 + I)                                                                # FCH had no forgotten device: deleted
c.req(t + 4 + I, pull_req(FCH, C, t + 4 + I, NONCE(1), []), "not_enrolled", ch=FCH)
c.req(t + 4 + I, enroll_req(APP, third, pub(C), "C"), "ok", ch=TCH)               # its slot is free
c.req(t + 5 + I, enroll_req(APP, EK, pub(A), "A"), "ok")                          # the household returns
c.req(t + 5 + I, append_req(CH, A, 3, [e3], t + 5 + I), "seq")                     # its log starts over
c.req(t + 5 + I, append_req(CH, A, 1, [e3], t + 5 + I), "ok")
# Back, A forgets C, which has not enrolled again: the Forget op goes up first, then
# the record (cut_seq 0: nothing of C's new log). C's pull, still naming no
# generation, gets the new one and the channel frozen at its Forget, Forget op
# included, so it can wipe; enrolling again is refused.
fe2 = envelope(REF(111), tag=111)
c.req(t + 6 + I, append_req(CH, A, 2, [fe2], t + 6 + I), "ok")
c.req(t + 6 + I, forget_req(CH, pub(C), [], 0, A, t + 6 + I), "ok")
c.req(t + 6 + I, append_req(CH, A, 3, [envelope(REF(112), tag=112)], t + 6 + I), "ok")
r_ = c.req(t + 7 + I, pull_req(CH, C, t + 7 + I, NONCE(3), []), "ok")
assert fe2 in r_ and envelope(REF(112), tag=112) not in r_
c.req(t + 7 + I, enroll_req(APP, EK, pub(C), "C"), "forgotten")
c.done()

# 22. What refreshes last_write.
c = Case("idle_refresh", "a repeated enrolment (no ts, replayable) does not keep a channel alive; a retried append (signed, fresh) does; a failed write does not", idle_ms=I)
c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok")                                   # CH: t
c.req(t, enroll_req(APP, FK, pub(C), "C"), "ok", ch=FCH)
c.req(t + 1, append_req(FCH, C, 1, [e1], t + 1), "ok", ch=FCH)                    # FCH: t + 1
c.req(t + 5 * DAY, enroll_req(APP, EK, pub(A), "A"), "ok")                        # a repeat: stores nothing, refreshes nothing
c.req(t + 5 * DAY, append_req(CH, A, 1, [e1], t + 5 * DAY - W - 1), "stale")      # a failed write refreshes nothing
c.req(t + 5 * DAY, append_req(FCH, C, 1, [e1], t + 5 * DAY), "ok", ch=FCH)        # a retry: stores nothing, refreshes (FCH: t + 5 d)
c.sweep(t + 1 + I)                                                                # CH is idle; FCH would be, but for the retry
c.req(t + 1 + I, pull_req(CH, A, t + 1 + I, NONCE(1), []), "not_enrolled")
c.req(t + 1 + I, pull_req(FCH, C, t + 1 + I, NONCE(1), []), "ok", ch=FCH)
c.sweep(t + 5 * DAY + I)
c.req(t + 5 * DAY + I, pull_req(FCH, C, t + 5 * DAY + I, NONCE(2), []), "not_enrolled", ch=FCH)
c.done()

# 23. Generations: a channel's changes exactly when its logs are wiped.
c = Case("generations", "enroll and pull name the channel's generation; a restart keeps it; an expired channel kept for its tombstones takes a new one; a deleted channel made again never repeats one", idle_ms=I)
G = lambda n: arr([tstr("ok"), uint(n)])
assert c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok") == G(1)
assert c.req(t, enroll_req(APP, FK, pub(C), "C"), "ok", ch=FCH) == G(2)
assert c.req(t, enroll_req(APP, EK, pub(A), "A"), "ok") == G(1)                    # a repeat names it too
assert c.req(t + 1, pull_req(CH, A, t + 1, NONCE(1), []), "ok").startswith(b"\x85" + tstr("ok") + uint(1))
c.req(t + 2, forget_req(CH, pub(B), [], 0, A, t + 2), "ok")                       # a tombstone to keep
c.restart(t + 3)
assert c.req(t + 3, pull_req(CH, A, t + 3, NONCE(2), [], epoch=2), "ok").startswith(b"\x85" + tstr("ok") + uint(1))
c.sweep(t + 2 + I)                                                                # CH kept (gen 3), FCH deleted
c.req(t + 2 + I, pull_req(CH, A, t + 2 + I, NONCE(3), [], epoch=2), "not_enrolled")
assert c.req(t + 2 + I, enroll_req(APP, EK, pub(A), "A"), "ok") == G(3)
assert c.req(t + 2 + I, enroll_req(APP, FK, pub(C), "C"), "ok", ch=FCH) == G(4)
assert c.req(t + 3 + I, pull_req(CH, A, t + 3 + I, NONCE(4), [], epoch=2), "ok").startswith(b"\x85" + tstr("ok") + uint(3))
c.done()

print(json.dumps({
    "format": "hearthSync relay conformance v1",
    "app": APP,
    "household": HH.hex(),
    "channel": CH.hex(),
    "devices": {"A": pub(A).hex(), "B": pub(B).hex(), "C": pub(C).hex()},
    "config": DEFAULT,
    "cases": cases,
}, indent=1))
