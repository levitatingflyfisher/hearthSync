// Package relay is the Go hearthSync relay: the request handler, its bbolt store and the
// HTTP server around it. It implements docs/reference/relay-protocol.md independently of
// the Rust relay and answers vectors/relay_v1.json byte for byte.
//
// The handler is pure apart from its store: time comes in as now (Unix milliseconds),
// so the vectors and the differential harness drive it directly.
package relay

import (
	"crypto/sha256"
	"errors"
	"strings"

	bolt "go.etcd.io/bbolt"

	"hearthsync/go-relay/internal/dcbor"
	"hearthsync/go-relay/internal/edsig"
)

// Response is what the handler answers.
type Response struct {
	Status      int
	Body        []byte
	ContentType string
	Verb        string // for logs: never a channel or a device
}

// fail is an error answer. It also aborts the request's store transaction.
type fail struct {
	code string
	seq  *uint64 // the "seq" error's third item
}

func (f *fail) Error() string { return f.code }

func failed(code string) error { return &fail{code: code} }

var statusOf = map[string]int{
	"bad_request": 400, "channel_mismatch": 400, "bad_envelope": 400,
	"stale": 401, "bad_signature": 401, "replay": 401,
	"bad_auth": 403, "not_enrolled": 403, "forgotten": 403,
	"no_snapshot": 404, "not_found": 404, "method": 405, "seq": 409,
	"too_large": 413, "rate_limited": 429, "quota": 507, "epoch": 409,
}

// Past this many buckets the relay sheds the ones that would be full by now (which
// changes nothing), as the Rust relay does.
const maxBuckets = 100_000

type bucketKind uint8

const (
	bDevice bucketKind = iota
	bChannel
	bEnroll
	bCreate
)

type bucketKey struct {
	kind   bucketKind
	ch     ID
	device ID
}

type bucket struct{ tokens, last uint64 }

// take is the protocol's token bucket: one token per interval, whole tokens only (the
// remainder carries over), and a full bucket's clock restarts at now.
func (b *bucket) take(burst, interval, now uint64) bool {
	interval = max(interval, 1)
	if now > b.last {
		add := (now - b.last) / interval
		b.tokens = min(burst, satAdd(b.tokens, add))
		if b.tokens == burst {
			b.last = now
		} else {
			b.last += add * interval
		}
	}
	if b.tokens == 0 {
		return false
	}
	b.tokens--
	return true
}

// readerKey is one reader in one channel.
type readerKey struct{ ch, reader ID }

type nonceExp struct {
	nonce [16]byte
	exp   uint64
}

// Relay is the handler and its state. It is not safe for concurrent use: the HTTP
// layer serialises requests.
type Relay struct {
	db      *bolt.DB
	cfg     Config
	epoch   uint64
	buckets map[bucketKey]*bucket
	// Live read nonces per reader: at most MaxReaderNonces each, so no reader or
	// household can use up another's (no relay-wide cap). Memory only: the epoch is
	// what makes them safe across a restart.
	nonces map[readerKey][]nonceExp
}

// Open opens (creating if needed) the relay's store in dir. Every open is a start:
// the store's epoch moves on. noSync skips fsync, for tests and the differential
// harness only.
func Open(dir string, cfg Config, noSync bool) (*Relay, error) {
	db, epoch, err := openDB(dir, noSync)
	if err != nil {
		return nil, err
	}
	return &Relay{db: db, cfg: cfg, epoch: epoch, buckets: map[bucketKey]*bucket{}, nonces: map[readerKey][]nonceExp{}}, nil
}

// Epoch is the epoch reads must name.
func (r *Relay) Epoch() uint64 { return r.epoch }

// Close closes the store.
func (r *Relay) Close() error { return r.db.Close() }

// Config is the relay's limits.
func (r *Relay) Config() Config { return r.cfg }

// Digest is SHA-256 of the canonical store dump (conformance).
func (r *Relay) Digest() ([32]byte, error) {
	var d [32]byte
	err := r.db.View(func(tx *bolt.Tx) error {
		b, err := stx{tx}.dump()
		d = sha256.Sum256(b)
		return err
	})
	return d, err
}

// Sweep is the server's sweep as of now: it prunes every channel (writes prune their
// own), expires idle channels and drops expired nonces. It returns the number of
// entries pruned.
func (r *Relay) Sweep(now uint64) (uint64, error) {
	return r.SweepAhead(now, 0)
}

// SweepAhead is the sweep with pruning and idle expiry run as of now + aheadMS
// (nonces still expire as of now). For tests only: the HTTP layer's --test-hooks
// hook, so an expiry test needs neither a restart nor a fake clock.
func (r *Relay) SweepAhead(now, aheadMS uint64) (uint64, error) {
	later := now + aheadMS
	if later < now {
		later = ^uint64(0)
	}
	var n uint64
	err := r.db.Update(func(tx *bolt.Tx) error {
		s := stx{tx}
		var ids []ID
		tx.Bucket(kChannels).ForEachBucket(func(k []byte) error {
			var id ID
			copy(id[:], k)
			ids = append(ids, id)
			return nil
		})
		for i := range ids {
			m, err := s.prune(&ids[i], later, r.cfg.RetainMS)
			if err != nil {
				return err
			}
			n += m
		}
		_, err := s.expireIdle(later, r.cfg.IdleMS)
		return err
	})
	for k, live := range r.nonces {
		if live = liveNonces(live, now); len(live) == 0 {
			delete(r.nonces, k)
		} else {
			r.nonces[k] = live
		}
	}
	return n, err
}

// Handle answers one request.
func (r *Relay) Handle(method, path string, body []byte, now uint64) Response {
	if path == "/healthz" && method == "GET" {
		return Response{Status: 200, Body: []byte("ok"), ContentType: "text/plain", Verb: "healthz"}
	}
	ch, verb, ok := route(path)
	if !ok {
		return errResponse("-", "not_found")
	}
	if method != "POST" {
		return errResponse(verb, "method")
	}
	var out []byte
	var err error
	switch verb {
	case "enroll":
		out, err = r.enroll(&ch, body, now)
	case "forget":
		out, err = r.forget(&ch, body, now)
	case "append":
		out, err = r.append(&ch, body, now)
	case "snapshot":
		out, err = r.snapshot(&ch, body, now)
	case "pull":
		out, err = r.pull(&ch, body, now)
	case "fetch_snapshot":
		out, err = r.fetchSnapshot(&ch, body, now)
	}
	var f *fail
	switch {
	case err == nil:
		return Response{Status: 200, Body: out, ContentType: "application/cbor", Verb: verb}
	case errors.As(err, &f) && f.seq != nil:
		return Response{Status: 409, Body: dcbor.Enc{}.Array(3).Text("err").Text(f.code).Uint(*f.seq),
			ContentType: "application/cbor", Verb: verb}
	case errors.As(err, &f):
		return errResponse(verb, f.code)
	default:
		logEvent("error", "event", "store", "verb", verb, "error", err.Error())
		return Response{Status: 500, Body: errorBody("internal"), ContentType: "application/cbor", Verb: verb}
	}
}

func errResponse(verb, code string) Response {
	return Response{Status: statusOf[code], Body: errorBody(code), ContentType: "application/cbor", Verb: verb}
}

var verbs = map[string]bool{"enroll": true, "forget": true, "append": true, "snapshot": true, "pull": true, "fetch_snapshot": true}

// route parses /v1/{channel}/{verb}: 64 lowercase hex digits and a known verb.
func route(path string) (ch ID, verb string, ok bool) {
	rest, ok := strings.CutPrefix(path, "/v1/")
	if !ok {
		return ch, "", false
	}
	hex, verb, ok := strings.Cut(rest, "/")
	if !ok || !verbs[verb] || len(hex) != 64 {
		return ch, "", false
	}
	for i := 0; i < 32; i++ {
		hi, ok1 := nibble(hex[2*i])
		lo, ok2 := nibble(hex[2*i+1])
		if !ok1 || !ok2 {
			return ch, "", false
		}
		ch[i] = hi<<4 | lo
	}
	return ch, verb, true
}

func nibble(c byte) (byte, bool) {
	switch {
	case c >= '0' && c <= '9':
		return c - '0', true
	case c >= 'a' && c <= 'f':
		return c - 'a' + 10, true
	}
	return 0, false
}

// ------------------------------------------------------------ shared checks

func (r *Relay) take(key bucketKey, burst, interval, now uint64) error {
	b := r.buckets[key]
	if b == nil {
		if len(r.buckets) >= maxBuckets {
			r.shedBuckets(now)
		}
		b = &bucket{tokens: burst, last: now}
		r.buckets[key] = b
	}
	if !b.take(burst, interval, now) {
		return failed("rate_limited")
	}
	return nil
}

func (r *Relay) limits(k bucketKind) (burst, interval uint64) {
	switch k {
	case bDevice:
		return r.cfg.DeviceBurst, r.cfg.DeviceIntervalMS
	case bChannel:
		return r.cfg.ChannelBurst, r.cfg.ChannelIntervalMS
	case bEnroll:
		return r.cfg.EnrollBurst, r.cfg.EnrollIntervalMS
	case bCreate:
		return r.cfg.CreateBurst, r.cfg.CreateIntervalMS
	}
	panic("relay: unknown bucket kind")
}

// shedBuckets drops the buckets that would be full by now: a fresh one is the same.
func (r *Relay) shedBuckets(now uint64) {
	for k, b := range r.buckets {
		burst, interval := r.limits(k.kind)
		var gained uint64
		if now > b.last {
			gained = (now - b.last) / max(interval, 1)
		}
		if satAdd(b.tokens, gained) >= burst {
			delete(r.buckets, k)
		}
	}
}

// deviceRate is "Rate" in the protocol: the signer's device bucket, then the channel's
// (a channel refusal keeps the device token).
func (r *Relay) deviceRate(ch, d *ID, now uint64) error {
	burst, interval := r.limits(bDevice)
	if err := r.take(bucketKey{kind: bDevice, ch: *ch, device: *d}, burst, interval, now); err != nil {
		return err
	}
	burst, interval = r.limits(bChannel)
	return r.take(bucketKey{kind: bChannel, ch: *ch}, burst, interval, now)
}

// epochOK: a read must name this start's epoch.
func (r *Relay) epochOK(epoch uint64) error {
	if epoch != r.epoch {
		cur := r.epoch
		return &fail{code: "epoch", seq: &cur}
	}
	return nil
}

// fresh: now - W <= ts <= now + W, exactly (no wrap-around at either end).
func (r *Relay) fresh(ts, now uint64) error {
	w := r.cfg.WindowMS
	lo := uint64(0)
	if now > w {
		lo = now - w
	}
	if ts < lo || ts > satAdd(now, w) {
		return failed("stale")
	}
	return nil
}

// liveNonces keeps the nonces whose expiry is at least now, in place.
func liveNonces(ns []nonceExp, now uint64) []nonceExp {
	out := ns[:0]
	for _, n := range ns {
		if n.exp >= now {
			out = append(out, n)
		}
	}
	return out
}

// useNonce records (channel, reader, nonce) until ts + W; a live one is a replay. A
// nonce stays live while its expiry is at least now. A reader holding MaxReaderNonces
// live nonces is refused (and the nonce not recorded): only that reader waits.
func (r *Relay) useNonce(ch, reader *ID, nonce [16]byte, ts, now uint64) error {
	k := readerKey{*ch, *reader}
	live := liveNonces(r.nonces[k], now)
	defer func() {
		if len(live) == 0 {
			delete(r.nonces, k)
		} else {
			r.nonces[k] = live
		}
	}()
	for _, n := range live {
		if n.nonce == nonce {
			return failed("replay")
		}
	}
	if uint64(len(live)) >= r.cfg.MaxReaderNonces {
		return failed("rate_limited")
	}
	live = append(live, nonceExp{nonce, satAdd(ts, r.cfg.WindowMS)})
	return nil
}

func signed(signer *ID, msg []byte, sig [64]byte) error {
	if !edsig.VerifyStrict(signer[:], msg, sig[:]) {
		return failed("bad_signature")
	}
	return nil
}

// enrolled returns the signer's channel and record if the signer is enrolled
// (forgotten or not).
func enrolled(s stx, ch, d *ID) (*channelRow, *deviceRow, error) {
	c, err := s.channel(ch)
	if err != nil {
		return nil, nil, err
	}
	if c == nil {
		return nil, nil, failed("not_enrolled")
	}
	dev, err := s.device(ch, d)
	if err != nil {
		return nil, nil, err
	}
	if dev == nil || !dev.enrolled() {
		return nil, nil, failed("not_enrolled")
	}
	return c, dev, nil
}

// reader returns a pull's channel and record if the reader is enrolled or
// forgotten. A Forget can be recorded for a target that never enrolled in the
// channel (the household came back after an expiry and forgot it first), and that
// target must still read its Forget op.
func reader(s stx, ch, d *ID) (*channelRow, *deviceRow, error) {
	c, err := s.channel(ch)
	if err != nil {
		return nil, nil, err
	}
	if c == nil {
		return nil, nil, failed("not_enrolled")
	}
	dev, err := s.device(ch, d)
	if err != nil {
		return nil, nil, err
	}
	if dev == nil || !dev.enrolled() && !dev.forgotten() {
		return nil, nil, failed("not_enrolled")
	}
	return c, dev, nil
}

// quotaOK: the channel (holding used bytes) and the relay may grow by add.
func (r *Relay) quotaOK(s stx, used uint64, add int64) error {
	over := func(have, limit uint64) bool {
		if add <= 0 {
			return have-uint64(-add) > limit && have >= uint64(-add)
		}
		return have+uint64(add) < have || have+uint64(add) > limit
	}
	if over(used, r.cfg.ChannelQuota) || over(s.totalBytes(), r.cfg.MaxTotalBytes) {
		return failed("quota")
	}
	return nil
}

func hasRef(env []byte) bool {
	_, ok := envelopeRef(env)
	return ok
}

// ------------------------------------------------------------ verbs

func (r *Relay) update(f func(s stx) ([]byte, error)) (out []byte, err error) {
	err = r.db.Update(func(tx *bolt.Tx) error {
		out, err = f(stx{tx})
		return err
	})
	return out, err
}

func (r *Relay) view(f func(s stx) ([]byte, error)) (out []byte, err error) {
	err = r.db.View(func(tx *bolt.Tx) error {
		out, err = f(stx{tx})
		return err
	})
	return out, err
}

func (r *Relay) enroll(ch *ID, body []byte, now uint64) ([]byte, error) {
	q, ok := parseEnroll(body)
	if !ok {
		return nil, failed("bad_request")
	}
	if ChannelID(q.app, &q.household) != *ch {
		return nil, failed("channel_mismatch")
	}
	if !edsig.VerifyStrict(q.household[:], enrollAuthMsg(q.app, &q.device, q.label), q.auth[:]) {
		return nil, failed("bad_auth")
	}
	return r.update(func(s stx) ([]byte, error) {
		c, err := s.channel(ch)
		if err != nil {
			return nil, err
		}
		var dev *deviceRow
		if c != nil {
			if dev, err = s.device(ch, &q.device); err != nil {
				return nil, err
			}
		}
		if dev != nil && dev.forgotten() {
			return nil, failed("forgotten")
		}
		if c == nil && s.channelCount() >= r.cfg.MaxChannels {
			return nil, failed("quota")
		}
		if c == nil {
			if err := r.take(bucketKey{kind: bCreate}, r.cfg.CreateBurst, r.cfg.CreateIntervalMS, now); err != nil {
				return nil, err
			}
		}
		if err := r.take(bucketKey{kind: bEnroll, ch: *ch}, r.cfg.EnrollBurst, r.cfg.EnrollIntervalMS, now); err != nil {
			return nil, err
		}
		if dev != nil && dev.enrolled() {
			return dcbor.Enc{}.Array(2).Text("ok").Uint(c.gen), nil
		}
		if c != nil && s.deviceCount(ch) >= r.cfg.MaxDevices {
			return nil, failed("quota")
		}
		if c == nil {
			if err := s.createChannel(ch, q.app, &q.household, now); err != nil {
				return nil, err
			}
		}
		label := q.label
		if err := s.putDevice(ch, &q.device, &deviceRow{label: &label, auth: q.auth[:]}); err != nil {
			return nil, err
		}
		if err := s.touch(ch, now); err != nil {
			return nil, err
		}
		if c, err = s.channel(ch); err != nil {
			return nil, err
		}
		return dcbor.Enc{}.Array(2).Text("ok").Uint(c.gen), nil
	})
}

func (r *Relay) forget(ch *ID, body []byte, now uint64) ([]byte, error) {
	q, ok := parseForget(body)
	if !ok {
		return nil, failed("bad_request")
	}
	return r.update(func(s stx) ([]byte, error) {
		c, _, err := enrolled(s, ch, &q.poster)
		if err != nil {
			return nil, err
		}
		if err := r.fresh(q.ts, now); err != nil {
			return nil, err
		}
		if err := signed(&q.poster, q.signable(ch), q.sig); err != nil {
			return nil, err
		}
		if !edsig.VerifyStrict(c.household[:], forgetAuthMsg(c.app, &q.target, q.cut), q.auth[:]) {
			return nil, failed("bad_auth")
		}
		if err := r.deviceRate(ch, &q.poster, now); err != nil {
			return nil, err
		}
		row, err := s.device(ch, &q.target)
		if err != nil {
			return nil, err
		}
		if row == nil {
			if s.deviceCount(ch) >= r.cfg.MaxDevices {
				return nil, failed("quota")
			}
			row = &deviceRow{}
		}
		if row.cutSeq == nil {
			cut, freeze := q.cutSeq, c.nextOrd-1
			row.cutSeq, row.freezeOrd = &cut, &freeze
		} else {
			cut := min(*row.cutSeq, q.cutSeq)
			row.cutSeq = &cut
		}
		if err := s.putDevice(ch, &q.target, row); err != nil {
			return nil, err
		}
		return (dcbor.Enc{}.Array(1).Text("ok")), s.touch(ch, now)
	})
}

func (r *Relay) append(ch *ID, body []byte, now uint64) ([]byte, error) {
	q, ok := parseAppend(body)
	if !ok {
		return nil, failed("bad_request")
	}
	n := uint64(len(q.envelopes))
	if n > r.cfg.MaxBatch {
		return nil, failed("too_large")
	}
	for _, e := range q.envelopes {
		if uint64(len(e)) > r.cfg.MaxEnvelope {
			return nil, failed("too_large")
		}
	}
	return r.update(func(s stx) ([]byte, error) {
		_, dev, err := enrolled(s, ch, &q.uploader)
		if err != nil {
			return nil, err
		}
		if err := r.fresh(q.ts, now); err != nil {
			return nil, err
		}
		if err := signed(&q.uploader, q.signable(ch), q.sig); err != nil {
			return nil, err
		}
		if err := r.deviceRate(ch, &q.uploader, now); err != nil {
			return nil, err
		}
		for _, e := range q.envelopes {
			if !hasRef(e) {
				return nil, failed("bad_envelope")
			}
		}
		// The batch's last seq, first_seq + n - 1, compared exactly: past 2^64 - 1 it
		// is above any cut.
		if dev.cutSeq != nil && (q.firstSeq > *dev.cutSeq || n-1 > *dev.cutSeq-q.firstSeq) {
			return nil, failed("forgotten")
		}
		last := s.last(ch, &q.uploader)
		if q.firstSeq-1 > last {
			return nil, &fail{code: "seq", seq: &last}
		}
		var news []entry
		for k, e := range q.envelopes {
			seq := q.firstSeq + uint64(k)
			if seq <= last {
				if held := s.entry(ch, &q.uploader, seq); held != nil && string(held) != string(e) {
					return nil, &fail{code: "seq", seq: &last}
				}
			} else {
				news = append(news, entry{seq, e})
			}
		}
		if _, err := s.prune(ch, now, r.cfg.RetainMS); err != nil {
			return nil, err
		}
		var add int64
		for _, e := range news {
			add += int64(len(e.env))
		}
		c, err := s.channel(ch)
		if err != nil {
			return nil, err
		}
		if err := r.quotaOK(s, c.bytes, add); err != nil {
			return nil, err
		}
		if len(news) > 0 {
			if last, err = s.appendEntries(ch, &q.uploader, news, now); err != nil {
				return nil, err
			}
		}
		return (dcbor.Enc{}.Array(2).Text("ok").Uint(last)), s.touch(ch, now)
	})
}

func (r *Relay) snapshot(ch *ID, body []byte, now uint64) ([]byte, error) {
	q, ok := parseSnapshot(body)
	if !ok {
		return nil, failed("bad_request")
	}
	if uint64(len(q.envelope)) > r.cfg.MaxSnapshot || uint64(len(q.covers)) > r.cfg.MaxDevices {
		return nil, failed("too_large")
	}
	return r.update(func(s stx) ([]byte, error) {
		_, dev, err := enrolled(s, ch, &q.device)
		if err != nil {
			return nil, err
		}
		if err := r.fresh(q.ts, now); err != nil {
			return nil, err
		}
		if err := signed(&q.device, q.signable(ch), q.sig); err != nil {
			return nil, err
		}
		if dev.forgotten() {
			return nil, failed("forgotten")
		}
		if err := r.deviceRate(ch, &q.device, now); err != nil {
			return nil, err
		}
		if !hasRef(q.envelope) {
			return nil, failed("bad_envelope")
		}
		if _, err := s.prune(ch, now, r.cfg.RetainMS); err != nil {
			return nil, err
		}
		old, _, _, found, err := s.snapshot(ch, &q.device)
		if err != nil {
			return nil, err
		}
		add := int64(len(q.envelope))
		if found {
			add -= int64(len(old))
		}
		c, err := s.channel(ch)
		if err != nil {
			return nil, err
		}
		if err := r.quotaOK(s, c.bytes, add); err != nil {
			return nil, err
		}
		if err := s.putSnapshot(ch, &q.device, q.envelope, q.covers, now); err != nil {
			return nil, err
		}
		return (dcbor.Enc{}.Array(1).Text("ok")), s.touch(ch, now)
	})
}

func (r *Relay) pull(ch *ID, body []byte, now uint64) ([]byte, error) {
	q, ok := parsePull(body)
	if !ok {
		return nil, failed("bad_request")
	}
	if uint64(len(q.cursors)) > r.cfg.MaxDevices {
		return nil, failed("too_large")
	}
	return r.view(func(s stx) ([]byte, error) {
		c, me, err := reader(s, ch, &q.reader)
		if err != nil {
			return nil, err
		}
		if err := r.fresh(q.ts, now); err != nil {
			return nil, err
		}
		if err := signed(&q.reader, q.signable(ch), q.sig); err != nil {
			return nil, err
		}
		if err := r.epochOK(q.epoch); err != nil {
			return nil, err
		}
		if err := r.useNonce(ch, &q.reader, q.nonce, q.ts, now); err != nil {
			return nil, err
		}
		if err := r.deviceRate(ch, &q.reader, now); err != nil {
			return nil, err
		}
		frozen := me.forgotten()
		var freeze uint64
		if frozen && me.freezeOrd != nil {
			freeze = *me.freezeOrd
		}
		cursors := map[ID]uint64{}
		for _, p := range q.cursors {
			cursors[p.ID] = p.N
		}
		var n, size uint64
		more := false
		logs := s.logs(ch)
		e := dcbor.Enc{}.Array(5).Text("ok").Uint(c.gen).Array(len(logs))
		for _, l := range logs {
			first := l.last + 1
			if f, held := s.firstHeld(ch, &l.up); held {
				first = f
			}
			var got dcbor.Enc
			count := 0
			if !more {
				s.scan(ch, &l.up, cursors[l.up], func(seq, ord uint64, env []byte) bool {
					if frozen && ord > freeze {
						return true
					}
					if n >= r.cfg.MaxPullEntries || (n > 0 && size+uint64(len(env)) > r.cfg.MaxPullBytes) {
						more = true
						return false
					}
					n++
					size += uint64(len(env))
					got = got.Array(2).Uint(seq).Bytes(env)
					count++
					return true
				})
			}
			e = e.Array(3).Bytes(l.up[:]).Uint(first).Array(count).Raw(got)
		}
		if frozen {
			e = e.Array(0)
		} else {
			snaps, err := s.snapshots(ch)
			if err != nil {
				return nil, err
			}
			e = e.Array(len(snaps))
			for _, sn := range snaps {
				ref, _ := envelopeRef(sn.env)
				e = e.Array(3).Bytes(sn.device[:]).Bytes(ref[:]).Pairs(sn.covers)
			}
		}
		return e.Bool(more), nil
	})
}

func (r *Relay) fetchSnapshot(ch *ID, body []byte, now uint64) ([]byte, error) {
	q, ok := parseFetch(body)
	if !ok {
		return nil, failed("bad_request")
	}
	return r.view(func(s stx) ([]byte, error) {
		_, me, err := enrolled(s, ch, &q.reader)
		if err != nil {
			return nil, err
		}
		if err := r.fresh(q.ts, now); err != nil {
			return nil, err
		}
		if err := signed(&q.reader, q.signable(ch), q.sig); err != nil {
			return nil, err
		}
		if err := r.epochOK(q.epoch); err != nil {
			return nil, err
		}
		if err := r.useNonce(ch, &q.reader, q.nonce, q.ts, now); err != nil {
			return nil, err
		}
		if me.forgotten() {
			return nil, failed("forgotten")
		}
		if err := r.deviceRate(ch, &q.reader, now); err != nil {
			return nil, err
		}
		env, covers, _, found, err := s.snapshot(ch, &q.device)
		if err != nil {
			return nil, err
		}
		if !found {
			return nil, failed("no_snapshot")
		}
		return (dcbor.Enc{}.Array(3).Text("ok").Bytes(env).Pairs(covers)), nil
	})
}
