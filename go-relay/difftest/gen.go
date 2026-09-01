package difftest

import (
	"bytes"
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"fmt"
	"math/rand/v2"
	"sort"

	"hearthsync/go-relay/internal/dcbor"
	"hearthsync/go-relay/internal/relay"
)

// The generator writes request sequences from the protocol's grammar: mostly valid,
// signed requests from a small cast (so they reach the deep checks and the store), with
// deliberate faults mixed in: wrong keys, other channels, stale and extreme times, u64
// extremes, non-NFC and edge-length labels, reserved app domains, envelope variants,
// unsorted lists, deep nesting, byte-level damage, odd paths and methods.

type ID = [32]byte

// key is a signer. A forger has a small-order public key and signs with R = identity,
// s = 0, which crypto/ed25519 accepts for every message and a strict verifier refuses.
type key struct {
	priv ed25519.PrivateKey
	pub  ID
}

func realKey(seed byte) key {
	k := ed25519.NewKeyFromSeed(bytes.Repeat([]byte{seed}, 32))
	var pub ID
	copy(pub[:], k.Public().(ed25519.PublicKey))
	return key{priv: k, pub: pub}
}

// forger returns a small-order key: the identity, canonical or as y = p + 1.
func forger(nonCanonical bool) key {
	var pub ID
	if nonCanonical {
		// p + 1 = 2^255 - 18, little-endian.
		pub = ID{0xee}
		for i := 1; i < 31; i++ {
			pub[i] = 0xff
		}
		pub[31] = 0x7f
	} else {
		pub[0] = 1
	}
	return key{pub: pub}
}

func (k key) sign(msg []byte) []byte {
	if k.priv == nil {
		sig := make([]byte, 64)
		sig[0] = 1 // R = the identity, s = 0
		return sig
	}
	return ed25519.Sign(k.priv, msg)
}

// ctSizes are the ciphertext lengths of generated envelopes: few, so byte budgets can
// land exactly on them.
var ctSizes = []int{0, 16, 40}

// envLen is the length of a valid generated envelope with ct bytes of ciphertext.
func envLen(ct int) int {
	return len(dcbor.Enc{}.Array(5).Uint(1).Uint(0).Bytes(make([]byte, 32)).Bytes(make([]byte, 24)).Bytes(make([]byte, ct)))
}

var u64Edges = []uint64{0, 1, 2, 3, 1<<63 - 1, 1 << 63, 1<<64 - 1}

// Gen is one run's generator.
type Gen struct {
	rng     *rand.Rand
	Config  map[string]uint64
	now     uint64
	window  uint64
	hh      []key
	devs    []key
	apps    []string
	refs    []ID
	last    map[[2]ID]uint64 // (channel, uploader) -> last seq seen in an answer
	nonces  [][16]byte
	envSeq  int
	skew    bool // include labels that probe the Unicode-version skew
	lastReq Step
	warm    int
	epoch   uint64 // the relay's, as last learned (a fresh relay starts at 1)
}

var configRanges = []struct {
	name   string
	lo, hi uint64
}{
	{"window_ms", 1000, 300_000},
	{"max_envelope", 60, 400},
	{"max_batch", 1, 4},
	{"max_snapshot", 60, 400},
	{"channel_quota", 150, 2000},
	{"max_total_bytes", 300, 3000},
	{"max_devices", 1, 7},
	{"max_channels", 1, 4},
	{"max_pull_entries", 1, 6},
	{"max_pull_bytes", 50, 600},
	{"retain_ms", 0, 200_000},
	{"device_burst", 1, 12},
	{"device_interval_ms", 1, 3000},
	{"enroll_burst", 1, 6},
	{"enroll_interval_ms", 1, 60_000},
	{"create_burst", 1, 4},
	{"create_interval_ms", 1, 60_000},
	{"channel_burst", 1, 30},
	{"channel_interval_ms", 1, 3000},
	{"idle_ms", 2_000_000, 40_000_000}, // long enough that the cast mostly stays enrolled
	{"max_reader_nonces", 1, 6},
}

// NewGen makes run seed's generator: a small random config (or the defaults, one run
// in eight) and the cast.
func NewGen(seed uint64, skew bool) *Gen {
	g := &Gen{rng: rand.New(rand.NewPCG(seed, seed^0x9e3779b97f4a7c15)), Config: map[string]uint64{},
		now: 1_727_000_000_000, last: map[[2]ID]uint64{}, skew: skew, epoch: 1}
	if g.rng.IntN(8) != 0 {
		for _, c := range configRanges {
			if g.rng.IntN(4) != 0 {
				g.Config[c.name] = c.lo + g.rng.Uint64N(c.hi-c.lo+1)
			}
		}
	}
	cfg := relay.DefaultConfig()
	for k, v := range g.Config {
		cfg.Set(k, v)
	}
	// Page byte budgets that land exactly on envelope boundaries, half the time.
	if _, set := g.Config["max_pull_bytes"]; set && g.rng.IntN(2) == 0 {
		g.Config["max_pull_bytes"] = uint64(1+g.rng.IntN(4)) * uint64(envLen(ctSizes[g.rng.IntN(len(ctSizes))]))
	}
	cfg = relay.DefaultConfig()
	for k, v := range g.Config {
		cfg.Set(k, v)
	}
	g.window = cfg.WindowMS
	g.hh = []key{realKey(1), realKey(2), forger(false), forger(true)}
	g.devs = []key{realKey(11), realKey(12), realKey(13), realKey(14), forger(false), forger(true)}
	g.apps = []string{"lullaby", "peckish"}
	for i := 0; i < 6; i++ {
		g.refs = append(g.refs, sha256.Sum256([]byte{byte(i)}))
	}
	for i := 0; i < 4; i++ {
		g.nonces = append(g.nonces, [16]byte{byte(i + 1)})
	}
	return g
}

func (g *Gen) chance(n int) bool { return g.rng.IntN(100) < n }

func (g *Gen) pickDev() key {
	if g.chance(90) {
		return g.devs[g.rng.IntN(4)]
	}
	return g.devs[4+g.rng.IntN(2)]
}

func (g *Gen) pickHH() key {
	switch r := g.rng.IntN(100); {
	case r < 70:
		return g.hh[0]
	case r < 92:
		return g.hh[1]
	}
	return g.hh[2+g.rng.IntN(2)]
}

func (g *Gen) pickApp() string {
	if g.chance(93) {
		return g.apps[g.rng.IntN(len(g.apps))]
	}
	return []string{"Lullaby", "sync", "channel", "", "abcdefghijklmnopqrstuvwxyz0123456", "lull-aby"}[g.rng.IntN(6)]
}

// channel is a (household, app) channel, mostly one of the cast's.
type channel struct {
	id  ID
	app string
	hh  key
}

func (g *Gen) pickChannel() channel {
	app := g.apps[0]
	if g.chance(20) {
		app = g.apps[1]
	}
	hh := g.pickHH()
	c := channel{id: relay.ChannelID(app, &hh.pub), app: app, hh: hh}
	if g.chance(3) {
		c.id = sha256.Sum256([]byte(fmt.Sprint("unknown", g.rng.IntN(3))))
	}
	return c
}

func (g *Gen) ts() uint64 {
	w := g.window
	switch r := g.rng.IntN(100); {
	case r < 55:
		return g.now
	case r < 80:
		d := g.rng.Uint64N(w + 1)
		if g.chance(50) && g.now >= d {
			return g.now - d
		}
		return g.now + d
	case r < 95:
		return []uint64{g.now - w, g.now + w, g.now - w - 1, g.now + w + 1}[g.rng.IntN(4)]
	default:
		return u64Edges[g.rng.IntN(len(u64Edges))]
	}
}

func (g *Gen) seqish(base uint64) uint64 {
	switch r := g.rng.IntN(100); {
	case r < 50:
		return base
	case r < 80:
		return base + g.rng.Uint64N(3)
	case r < 90 && base > 0:
		return base - 1
	default:
		return u64Edges[g.rng.IntN(len(u64Edges))]
	}
}

func (g *Gen) label() string {
	opts := []string{"A", "Kitchen tablet", "Dad's phone", "café", "가", strings128(), strings128() + "x",
		"", "café", "가", "á̖", "á̖"}
	if g.skew {
		// U+0897 (Unicode 16, ccc 230) then U+0316 (ccc 220): out of canonical order under
		// Unicode 16+, but U+0897 is unassigned (ccc 0) in Unicode 15.
		opts = append(opts, "a̖ࢗ")
	}
	if g.chance(80) {
		return opts[g.rng.IntN(5)]
	}
	return opts[g.rng.IntN(len(opts))]
}

func strings128() string { return string(bytes.Repeat([]byte("x"), 128)) }

// envelope returns a sealed-envelope-shaped byte string, mostly valid.
func (g *Gen) envelope(kind uint64) []byte {
	g.envSeq++
	ref := sha256.Sum256([]byte(fmt.Sprint("env", g.envSeq)))
	size := ctSizes[g.rng.IntN(len(ctSizes))]
	ct := make([]byte, size)
	for i := range ct {
		ct[i] = byte(g.envSeq + i)
	}
	nonce := make([]byte, 24)
	valid := func(e dcbor.Enc) []byte { return e }
	if g.chance(85) {
		return valid(dcbor.Enc{}.Array(5).Uint(1).Uint(kind).Bytes(ref[:]).Bytes(nonce).Bytes(ct))
	}
	switch g.rng.IntN(12) {
	case 0:
		return dcbor.Enc{}.Array(5).Uint(1).Uint(kind).Null().Bytes(nonce).Bytes(ct)
	case 1:
		return dcbor.Enc{}.Array(5).Uint(1).Uint(3).Bytes(ref[:]).Bytes(nonce).Bytes(ct)
	case 2:
		return dcbor.Enc{}.Array(5).Uint(2).Uint(kind).Bytes(ref[:]).Bytes(nonce).Bytes(ct)
	case 3:
		return append(dcbor.Enc{}.Array(5).Uint(1).Uint(kind).Bytes(ref[:]).Bytes(nonce).Bytes(ct), 0)
	case 4:
		return dcbor.Enc{}.Array(5).Uint(1).Uint(kind).Bytes(ref[:]).Bytes(nonce[:23]).Bytes(ct)
	case 5:
		return dcbor.Enc{}.Array(5).Uint(1).Uint(kind).Bytes(ref[:31]).Bytes(nonce).Bytes(ct)
	case 6:
		// A non-shortest length for the ciphertext.
		e := dcbor.Enc{}.Array(5).Uint(1).Uint(kind).Bytes(ref[:]).Bytes(nonce)
		return append(e, 0x58, 3, 1, 2, 3)
	case 7:
		return bytes.Repeat([]byte{0x81}, 50+g.rng.IntN(100))
	case 8:
		return nil
	case 9:
		return dcbor.Enc{}.Array(4).Uint(1).Uint(kind).Bytes(ref[:]).Bytes(nonce)
	case 10:
		return dcbor.Enc{}.Array(5).Uint(1).Uint(kind).Bytes(ref[:]).Bytes(nonce).Text("ct")
	default:
		big := make([]byte, 300)
		return dcbor.Enc{}.Array(5).Uint(1).Uint(kind).Bytes(ref[:]).Bytes(nonce).Bytes(big)
	}
}

func (g *Gen) pairs(ch ID) []dcbor.Pair {
	var ps []dcbor.Pair
	for _, d := range g.devs {
		if g.chance(22) {
			ps = append(ps, dcbor.Pair{ID: d.pub, N: g.seqish(g.last[[2]ID{ch, d.pub}])})
		}
	}
	sort.Slice(ps, func(i, j int) bool { return bytes.Compare(ps[i].ID[:], ps[j].ID[:]) < 0 })
	return ps
}

// encPairs encodes pairs, sometimes out of order or duplicated.
func (g *Gen) encPairs(e dcbor.Enc, ps []dcbor.Pair) dcbor.Enc {
	if len(ps) >= 2 && g.chance(4) {
		ps = append([]dcbor.Pair{}, ps...)
		ps[0], ps[1] = ps[1], ps[0]
	} else if len(ps) >= 1 && g.chance(3) {
		ps = append([]dcbor.Pair{ps[0]}, ps...)
	}
	return e.Pairs(ps)
}

func (g *Gen) signer(right key) key {
	if g.chance(5) {
		return g.devs[g.rng.IntN(len(g.devs))]
	}
	return right
}

// signCh is the channel a request signs over: sometimes another.
func (g *Gen) signCh(ch ID) ID {
	if g.chance(3) {
		return sha256.Sum256(ch[:])
	}
	return ch
}

func signedHead(tag string, ch ID, signer ID, ts uint64, rest int) dcbor.Enc {
	return dcbor.Enc{}.Array(4 + rest).Text(tag).Bytes(ch[:]).Bytes(signer[:]).Uint(ts)
}

func (g *Gen) path(ch ID, verb string) string { return "/v1/" + hex.EncodeToString(ch[:]) + "/" + verb }

func (g *Gen) enroll() Step {
	app := g.pickApp()
	hh := g.pickHH()
	dev := g.pickDev()
	label := g.label()
	signBy := hh
	if g.chance(5) {
		signBy = g.hh[g.rng.IntN(len(g.hh))]
	}
	auth := signBy.sign(dcbor.Enc{}.Array(4).Text("oh-enroll/v1").Text(app).Bytes(dev.pub[:]).Text(label))
	chID := relay.ChannelID(app, &hh.pub)
	if g.chance(5) {
		chID = g.pickChannel().id
	}
	body := dcbor.Enc{}.Array(5).Text(app).Bytes(hh.pub[:]).Bytes(dev.pub[:]).Text(label).Bytes(auth)
	return Step{Method: "POST", Path: g.path(chID, "enroll"), Body: hex.EncodeToString(body), Note: "enroll"}
}

func (g *Gen) forget() Step {
	c := g.pickChannel()
	poster, target := g.pickDev(), g.pickDev()
	if g.chance(50) {
		target = g.devs[4+g.rng.IntN(2)] // a device never enrolled: leaves the cast writing
	}
	var cut []ID
	n := g.rng.IntN(3)
	if g.chance(3) {
		n = 65
	}
	for i := 0; i < n; i++ {
		cut = append(cut, sha256.Sum256([]byte(fmt.Sprint("cut", g.rng.IntN(1000)))))
	}
	sort.Slice(cut, func(i, j int) bool { return bytes.Compare(cut[i][:], cut[j][:]) < 0 })
	cut = dedup(cut)
	cutSeq := g.seqish(g.last[[2]ID{c.id, target.pub}])
	ids := func(e dcbor.Enc) dcbor.Enc {
		e = e.Array(len(cut))
		for _, id := range cut {
			e = e.Bytes(id[:])
		}
		return e
	}
	authBy := c.hh
	if g.chance(5) {
		authBy = g.hh[g.rng.IntN(len(g.hh))]
	}
	auth := authBy.sign(ids(dcbor.Enc{}.Array(4).Text("oh-forget/v1").Text(c.app).Bytes(target.pub[:])))
	ts := g.ts()
	msg := ids(signedHead("oh-relay-forget/v1", g.signCh(c.id), poster.pub, ts, 4).Bytes(target.pub[:])).Bytes(auth).Uint(cutSeq)
	sig := g.signer(poster).sign(msg)
	body := ids(dcbor.Enc{}.Array(7).Bytes(target.pub[:])).Bytes(auth).Uint(cutSeq).Bytes(poster.pub[:]).Uint(ts).Bytes(sig)
	return Step{Method: "POST", Path: g.path(c.id, "forget"), Body: hex.EncodeToString(body), Note: "forget"}
}

func dedup(ids []ID) []ID {
	var out []ID
	for i, id := range ids {
		if i == 0 || id != ids[i-1] {
			out = append(out, id)
		}
	}
	return out
}

func (g *Gen) appendReq() Step {
	c := g.pickChannel()
	up := g.pickDev()
	first := g.seqish(g.last[[2]ID{c.id, up.pub}] + 1)
	if first == 0 && g.chance(70) {
		first = 1
	}
	n := 1 + g.rng.IntN(int(min(g.Config2("max_batch", 64), 4)))
	if g.chance(5) {
		n++
	}
	if g.chance(3) {
		n = 0
	}
	var envs [][]byte
	for i := 0; i < n; i++ {
		envs = append(envs, g.envelope(0))
	}
	ts := g.ts()
	hashes := dcbor.Enc{}.Array(len(envs))
	for _, e := range envs {
		h := sha256.Sum256(e)
		hashes = hashes.Bytes(h[:])
	}
	msg := signedHead("oh-relay-append/v1", g.signCh(c.id), up.pub, ts, 2).Uint(first).Raw(hashes)
	sig := g.signer(up).sign(msg)
	body := dcbor.Enc{}.Array(5).Bytes(up.pub[:]).Uint(first).Array(len(envs))
	for _, e := range envs {
		body = body.Bytes(e)
	}
	body = body.Uint(ts).Bytes(sig)
	return Step{Method: "POST", Path: g.path(c.id, "append"), Body: hex.EncodeToString(body), Note: "append"}
}

// Config2 is a config value, or def if the run uses the default.
func (g *Gen) Config2(name string, def uint64) uint64 {
	if v, ok := g.Config[name]; ok {
		return v
	}
	return def
}

func (g *Gen) snapshot() Step {
	c := g.pickChannel()
	d := g.pickDev()
	env := g.envelope(1)
	covers := g.pairs(c.id)
	ts := g.ts()
	h := sha256.Sum256(env)
	msg := g.encPairs(signedHead("oh-relay-snapshot/v1", g.signCh(c.id), d.pub, ts, 2).Bytes(h[:]), covers)
	sig := g.signer(d).sign(msg)
	body := dcbor.Enc{}.Array(5).Bytes(d.pub[:]).Bytes(env)
	body = g.encPairs(body, covers).Uint(ts).Bytes(sig)
	return Step{Method: "POST", Path: g.path(c.id, "snapshot"), Body: hex.EncodeToString(body), Note: "snapshot"}
}

func (g *Gen) nonce() [16]byte {
	if g.chance(60) {
		var n [16]byte
		binary.BigEndian.PutUint64(n[:], g.rng.Uint64())
		return n
	}
	return g.nonces[g.rng.IntN(len(g.nonces))]
}

// readEpoch is the epoch a read names: mostly the relay's, sometimes a stale, future
// or extreme one.
func (g *Gen) readEpoch() uint64 {
	if g.chance(92) {
		return g.epoch
	}
	return []uint64{0, g.epoch - 1, g.epoch + 1, 1<<64 - 1}[g.rng.IntN(4)]
}

func (g *Gen) pull() Step {
	c := g.pickChannel()
	r := g.pickDev()
	ts, nonce, epoch := g.ts(), g.nonce(), g.readEpoch()
	cursors := g.pairs(c.id)
	msg := signedHead("oh-relay-pull/v1", g.signCh(c.id), r.pub, ts, 3).Uint(epoch).Bytes(nonce[:]).Pairs(cursors)
	sig := g.signer(r).sign(msg)
	body := dcbor.Enc{}.Array(6).Bytes(r.pub[:]).Uint(ts).Uint(epoch).Bytes(nonce[:])
	body = g.encPairs(body, cursors).Bytes(sig)
	return Step{Method: "POST", Path: g.path(c.id, "pull"), Body: hex.EncodeToString(body), Note: "pull"}
}

func (g *Gen) fetch() Step {
	c := g.pickChannel()
	r, d := g.pickDev(), g.pickDev()
	ts, nonce, epoch := g.ts(), g.nonce(), g.readEpoch()
	msg := signedHead("oh-relay-fetch-snapshot/v1", g.signCh(c.id), r.pub, ts, 3).Uint(epoch).Bytes(nonce[:]).Bytes(d.pub[:])
	sig := g.signer(r).sign(msg)
	body := dcbor.Enc{}.Array(6).Bytes(r.pub[:]).Uint(ts).Uint(epoch).Bytes(nonce[:]).Bytes(d.pub[:]).Bytes(sig)
	return Step{Method: "POST", Path: g.path(c.id, "fetch_snapshot"), Body: hex.EncodeToString(body), Note: "fetch_snapshot"}
}

// damage mangles a request: its bytes, its path or its method.
func (g *Gen) damage(s Step) Step {
	body, _ := hex.DecodeString(s.Body)
	s.Note += "+damaged"
	switch g.rng.IntN(9) {
	case 0:
		if len(body) > 0 {
			i := g.rng.IntN(len(body))
			body = append([]byte{}, body...)
			body[i] ^= byte(1 << g.rng.IntN(8))
		}
	case 1:
		if len(body) > 0 {
			body = body[:g.rng.IntN(len(body))]
		}
	case 2:
		body = append(append([]byte{}, body...), 0)
	case 3:
		body = append(dcbor.Enc{}.Array(1), body...)
	case 4:
		s.Method = []string{"GET", "PUT", "OPTIONS", "HEAD", "post"}[g.rng.IntN(5)]
	case 5:
		s.Path += "/"
	case 6:
		s.Path = []string{"/healthz", "/healthz/", "/v1/", "/v1//pull", "/v2" + s.Path[3:], "/V1" + s.Path[3:]}[g.rng.IntN(6)]
		if g.chance(50) {
			s.Method = "GET"
		}
	case 7:
		if len(s.Path) > 10 {
			b := []byte(s.Path)
			b[5] = 'A'
			s.Path = string(b)
		}
	default:
		body = bytes.Repeat([]byte{0x81}, g.rng.IntN(20000))
	}
	s.Body = hex.EncodeToString(body)
	return s
}

// Next returns the next step. The first few enrol the cast in the main channel.
func (g *Gen) Next() Step {
	if g.warm < 4 {
		hh := g.hh[0]
		if g.rng.IntN(4) == 0 {
			hh = g.hh[1]
		}
		d := g.devs[g.warm]
		g.warm++
		label := "device"
		auth := hh.sign(dcbor.Enc{}.Array(4).Text("oh-enroll/v1").Text(g.apps[0]).Bytes(d.pub[:]).Text(label))
		body := dcbor.Enc{}.Array(5).Text(g.apps[0]).Bytes(hh.pub[:]).Bytes(d.pub[:]).Text(label).Bytes(auth)
		ch := relay.ChannelID(g.apps[0], &hh.pub)
		return Step{Method: "POST", Path: g.path(ch, "enroll"), Body: hex.EncodeToString(body), Now: g.now, Note: "enroll"}
	}
	switch r := g.rng.IntN(100); {
	case r < 30:
	case r < 60:
		g.now += 1 + g.rng.Uint64N(1000)
	case r < 90:
		g.now += g.rng.Uint64N(g.window + 1)
	case r < 98:
		g.now += g.Config2("retain_ms", 120*relay.DayMS) + g.rng.Uint64N(60_000)
	default:
		g.now += 61_000 + g.rng.Uint64N(120_000)
	}
	var s Step
	switch r := g.rng.IntN(100); {
	case r < 10:
		s = g.enroll()
	case r < 48:
		s = g.appendReq()
	case r < 70:
		s = g.pull()
	case r < 80:
		s = g.snapshot()
	case r < 83:
		s = g.forget()
	case r < 89:
		s = g.fetch()
	case r < 92 && g.lastReq.Method != "":
		s = g.lastReq // a replay
		s.Note = "replay:" + s.Note
	case r < 97:
		s = g.damage([]func() Step{g.enroll, g.appendReq, g.pull, g.snapshot, g.forget, g.fetch}[g.rng.IntN(6)]())
	case r < 99:
		s = Step{Sweep: true, Note: "sweep"}
	default:
		// A restart in mid-sequence: memory goes, the store stays, the epoch moves on.
		// The last request stays around to be replayed across it.
		s = Step{Restart: true, Note: "restart"}
		g.epoch++
	}
	s.Now = g.now
	if !s.Sweep && !s.Restart {
		g.lastReq = s
	}
	return s
}

// Observe learns from an answer (the Go side's): the relay's epoch from an epoch
// answer, and an append's new last seq.
func (g *Gen) Observe(s Step, a Answer) {
	if a.Status == 409 {
		if b, err := hex.DecodeString(a.Body); err == nil {
			if it, err := dcbor.Decode(b); err == nil && len(it.Items) == 3 && string(it.Items[1].B) == "epoch" {
				g.epoch = it.Items[2].U
			}
		}
	}
	if a.Status != 200 || s.Note != "append" {
		return
	}
	body, _ := hex.DecodeString(a.Body)
	it, err := dcbor.Decode(body)
	if err != nil || len(it.Items) != 2 {
		return
	}
	req, _ := hex.DecodeString(s.Body)
	rq, err := dcbor.Decode(req)
	if err != nil {
		return
	}
	var up, ch ID
	copy(up[:], rq.Items[0].B)
	hexCh, _ := hex.DecodeString(s.Path[4:68])
	copy(ch[:], hexCh)
	g.last[[2]ID{ch, up}] = it.Items[1].U
}
