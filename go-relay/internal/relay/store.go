package relay

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"errors"
	"path/filepath"
	"time"

	bolt "go.etcd.io/bbolt"

	"hearthsync/go-relay/internal/dcbor"
)

// The store is one bbolt file (a B+tree with ACID transactions, pure Go). Each request
// runs in one transaction, so a request that fails changes nothing in it.
//
//	relay/total                  relay-wide stored envelope bytes
//	relay/count                  channels
//	relay/epoch                  the epoch: one more at every open
//	relay/generation             the last channel generation given out
//	channels/<id>/info           next_ord(8) bytes(8) last_write(8) generation(8) household(32) app
//	channels/<id>/dev/<device>   dCBOR [label|null, auth|null, cut_seq|null, freeze_ord|null]
//	channels/<id>/log/<uploader> last(8)
//	channels/<id>/ent/<up><seq8> ord(8) stored_at(8) envelope
//	channels/<id>/snap/<device>  stored_at(8) envelope
//	channels/<id>/cov/<device>   dCBOR covers
//
// Integers are big-endian, so keys sort by seq. Every uint64 is stored whole.

var (
	kRelay    = []byte("relay")
	kChannels = []byte("channels")
	kTotal    = []byte("total")
	kCount    = []byte("count")
	kEpoch    = []byte("epoch")
	kGen      = []byte("generation")
	kInfo     = []byte("info")
	kDev      = []byte("dev")
	kLog      = []byte("log")
	kEnt      = []byte("ent")
	kSnap     = []byte("snap")
	kCov      = []byte("cov")
)

// DBFile is the store's file name inside the data directory.
const DBFile = "relay.bolt"

func openDB(dir string, noSync bool) (*bolt.DB, uint64, error) {
	db, err := bolt.Open(filepath.Join(dir, DBFile), 0o600, &bolt.Options{
		Timeout:      time.Second, // another process holds the file
		NoSync:       noSync,
		NoGrowSync:   noSync,
		FreelistType: bolt.FreelistMapType,
	})
	if err != nil {
		return nil, 0, err
	}
	var epoch uint64
	err = db.Update(func(tx *bolt.Tx) error {
		rb, err := tx.CreateBucketIfNotExists(kRelay)
		if err != nil {
			return err
		}
		if _, err := tx.CreateBucketIfNotExists(kChannels); err != nil {
			return err
		}
		// Every start moves the epoch on (a new store starts at 1), so a read signed
		// before this start never passes after it.
		epoch = u64(rb.Get(kEpoch)) + 1
		return rb.Put(kEpoch, be(epoch))
	})
	if err != nil {
		db.Close()
		return nil, 0, err
	}
	return db, epoch, nil
}

func u64(b []byte) uint64 {
	if len(b) < 8 {
		return 0
	}
	return binary.BigEndian.Uint64(b)
}

func be(n uint64) []byte { return binary.BigEndian.AppendUint64(nil, n) }

type channelRow struct {
	app       string
	household ID
	nextOrd   uint64
	bytes     uint64
	lastWrite uint64
	gen       uint64 // changes whenever the channel's logs are wiped
}

type deviceRow struct {
	label     *string
	auth      []byte
	cutSeq    *uint64
	freezeOrd *uint64
}

func (d *deviceRow) enrolled() bool  { return d.auth != nil }
func (d *deviceRow) forgotten() bool { return d.cutSeq != nil }

type entry struct {
	seq uint64
	env []byte
}

// stx is one store transaction.
type stx struct{ tx *bolt.Tx }

var errCorrupt = errors.New("store: corrupt record")

func (s stx) chb(ch *ID) *bolt.Bucket { return s.tx.Bucket(kChannels).Bucket(ch[:]) }

func (s stx) channel(ch *ID) (*channelRow, error) {
	b := s.chb(ch)
	if b == nil {
		return nil, nil
	}
	v := b.Get(kInfo)
	if len(v) < 64 {
		return nil, errCorrupt
	}
	r := &channelRow{nextOrd: u64(v[0:8]), bytes: u64(v[8:16]), lastWrite: u64(v[16:24]), gen: u64(v[24:32]),
		app: string(v[64:])}
	copy(r.household[:], v[32:64])
	return r, nil
}

func (s stx) putInfo(ch *ID, r *channelRow) error {
	v := append(append(append(append(append(be(r.nextOrd), be(r.bytes)...), be(r.lastWrite)...), be(r.gen)...),
		r.household[:]...), r.app...)
	return s.chb(ch).Put(kInfo, v)
}

// nextGen hands out the next channel generation from the store's counter.
func (s stx) nextGen() (uint64, error) {
	g := u64(s.tx.Bucket(kRelay).Get(kGen)) + 1
	return g, s.tx.Bucket(kRelay).Put(kGen, be(g))
}

// touch records a write to the channel at now (its last_write, for idle expiry).
func (s stx) touch(ch *ID, now uint64) error {
	r, err := s.channel(ch)
	if err != nil || r == nil {
		return errCorrupt
	}
	r.lastWrite = now
	return s.putInfo(ch, r)
}

// expireIdle expires every channel with no write for idle ms (last_write + idle <= now):
// its entries, snapshots, covers, the records of devices not forgotten and their logs
// go, and the channel itself if no record is left. Forgotten records stay, so a
// forgotten device cannot enrol again, and so do their logs' last, so it cannot store
// entries at used seqs again. It returns how many channels it expired.
func (s stx) expireIdle(now, idle uint64) (uint64, error) {
	if now < idle {
		return 0, nil
	}
	cutoff := now - idle
	chans := s.tx.Bucket(kChannels)
	var ids []ID
	err := chans.ForEachBucket(func(k []byte) error {
		var id ID
		copy(id[:], k)
		r, err := s.channel(&id)
		if err != nil {
			return err
		}
		if r.lastWrite <= cutoff {
			ids = append(ids, id)
		}
		return nil
	})
	if err != nil {
		return 0, err
	}
	for i := range ids {
		ch := &ids[i]
		r, err := s.channel(ch)
		if err != nil {
			return 0, err
		}
		b := s.chb(ch)
		for _, k := range [][]byte{kEnt, kSnap, kCov} {
			if err := b.DeleteBucket(k); err != nil {
				return 0, err
			}
			if _, err := b.CreateBucket(k); err != nil {
				return 0, err
			}
		}
		dev := b.Bucket(kDev)
		var drop [][]byte
		err = dev.ForEach(func(k, _ []byte) error {
			var d ID
			copy(d[:], k)
			row, err := s.device(ch, &d)
			if err != nil {
				return err
			}
			if !row.forgotten() {
				drop = append(drop, bytes.Clone(k))
			}
			return nil
		})
		if err != nil {
			return 0, err
		}
		for _, k := range drop {
			if err := dev.Delete(k); err != nil {
				return 0, err
			}
			if err := b.Bucket(kLog).Delete(k); err != nil {
				return 0, err
			}
		}
		if err := s.addBytes(ch, -int64(r.bytes)); err != nil {
			return 0, err
		}
		// Not Stats().KeyN: it reads the pages as committed, not this transaction's deletes.
		if k, _ := dev.Cursor().First(); k == nil {
			if err := chans.DeleteBucket(ch[:]); err != nil {
				return 0, err
			}
			if err := s.tx.Bucket(kRelay).Put(kCount, be(s.channelCount()-1)); err != nil {
				return 0, err
			}
		} else {
			// Kept for its tombstones, with its logs wiped: clients' positions are void.
			kept, err := s.channel(ch)
			if err != nil {
				return 0, err
			}
			if kept.gen, err = s.nextGen(); err != nil {
				return 0, err
			}
			if err := s.putInfo(ch, kept); err != nil {
				return 0, err
			}
		}
	}
	return uint64(len(ids)), nil
}

func (s stx) channelCount() uint64 { return u64(s.tx.Bucket(kRelay).Get(kCount)) }

func (s stx) totalBytes() uint64 { return u64(s.tx.Bucket(kRelay).Get(kTotal)) }

func (s stx) createChannel(ch *ID, app string, household *ID, now uint64) error {
	b, err := s.tx.Bucket(kChannels).CreateBucket(ch[:])
	if err != nil {
		return err
	}
	for _, k := range [][]byte{kDev, kLog, kEnt, kSnap, kCov} {
		if _, err := b.CreateBucket(k); err != nil {
			return err
		}
	}
	gen, err := s.nextGen()
	if err != nil {
		return err
	}
	if err := s.putInfo(ch, &channelRow{app: app, household: *household, nextOrd: 1, lastWrite: now, gen: gen}); err != nil {
		return err
	}
	return s.tx.Bucket(kRelay).Put(kCount, be(s.channelCount()+1))
}

// addBytes moves the channel's and the relay's byte counts by delta.
func (s stx) addBytes(ch *ID, delta int64) error {
	r, err := s.channel(ch)
	if err != nil || r == nil {
		return errCorrupt
	}
	r.bytes = uint64(int64(r.bytes) + delta)
	if err := s.putInfo(ch, r); err != nil {
		return err
	}
	return s.tx.Bucket(kRelay).Put(kTotal, be(uint64(int64(s.totalBytes())+delta)))
}

func optText(it dcbor.Item) (*string, bool) {
	switch it.Kind {
	case dcbor.Null:
		return nil, true
	case dcbor.Text:
		s := string(it.B)
		return &s, true
	}
	return nil, false
}

func optUint(it dcbor.Item) (*uint64, bool) {
	switch it.Kind {
	case dcbor.Null:
		return nil, true
	case dcbor.Uint:
		n := it.U
		return &n, true
	}
	return nil, false
}

func (s stx) device(ch *ID, d *ID) (*deviceRow, error) {
	b := s.chb(ch)
	if b == nil {
		return nil, nil
	}
	v := b.Bucket(kDev).Get(d[:])
	if v == nil {
		return nil, nil
	}
	c, err := dcbor.Decode(v)
	if err != nil {
		return nil, errCorrupt
	}
	a, ok := c.Tuple(4)
	if !ok {
		return nil, errCorrupt
	}
	var r deviceRow
	var ok1, ok3, ok4 bool
	r.label, ok1 = optText(a[0])
	switch a[1].Kind {
	case dcbor.Null:
	case dcbor.Bytes:
		r.auth = bytes.Clone(a[1].B)
	default:
		return nil, errCorrupt
	}
	r.cutSeq, ok3 = optUint(a[2])
	r.freezeOrd, ok4 = optUint(a[3])
	if !ok1 || !ok3 || !ok4 {
		return nil, errCorrupt
	}
	return &r, nil
}

func (d *deviceRow) encode() []byte {
	e := dcbor.Enc{}.Array(4)
	if d.label != nil {
		e = e.Text(*d.label)
	} else {
		e = e.Null()
	}
	if d.auth != nil {
		e = e.Bytes(d.auth)
	} else {
		e = e.Null()
	}
	for _, n := range []*uint64{d.cutSeq, d.freezeOrd} {
		if n != nil {
			e = e.Uint(*n)
		} else {
			e = e.Null()
		}
	}
	return e
}

func (s stx) putDevice(ch *ID, d *ID, r *deviceRow) error {
	return s.chb(ch).Bucket(kDev).Put(d[:], r.encode())
}

func (s stx) deviceCount(ch *ID) uint64 {
	return uint64(s.chb(ch).Bucket(kDev).Stats().KeyN)
}

// last is the uploader's last seq (0 before its first append).
func (s stx) last(ch *ID, up *ID) uint64 {
	b := s.chb(ch)
	if b == nil {
		return 0
	}
	return u64(b.Bucket(kLog).Get(up[:]))
}

func entKey(up *ID, seq uint64) []byte { return append(append([]byte{}, up[:]...), be(seq)...) }

// entry returns the envelope held at (uploader, seq), or nil.
func (s stx) entry(ch *ID, up *ID, seq uint64) []byte {
	v := s.chb(ch).Bucket(kEnt).Get(entKey(up, seq))
	if len(v) < 16 {
		return nil
	}
	return v[16:]
}

// coverOf is, per uploader, the highest seq any stored snapshot covers.
func (s stx) coverOf(ch *ID) (map[ID]uint64, error) {
	out := map[ID]uint64{}
	err := s.chb(ch).Bucket(kCov).ForEach(func(_, v []byte) error {
		c, err := dcbor.Decode(v)
		if err != nil {
			return errCorrupt
		}
		ps, ok := c.PairList()
		if !ok {
			return errCorrupt
		}
		for _, p := range ps {
			out[p.ID] = max(out[p.ID], p.N)
		}
		return nil
	})
	return out, err
}

// prune drops the channel's entries that a snapshot covers and that have been held for
// retain (stored_at + retain <= now). It returns how many it dropped.
func (s stx) prune(ch *ID, now, retain uint64) (uint64, error) {
	if now < retain {
		return 0, nil
	}
	cutoff := now - retain
	cover, err := s.coverOf(ch)
	if err != nil {
		return 0, err
	}
	ents := s.chb(ch).Bucket(kEnt)
	var n uint64
	var freed int64
	for up, top := range cover {
		c := ents.Cursor()
		prefix := up[:]
		var drop [][]byte
		for k, v := c.Seek(prefix); k != nil && bytes.HasPrefix(k, prefix); k, v = c.Next() {
			if len(k) != 40 || len(v) < 16 {
				return 0, errCorrupt
			}
			if u64(k[32:]) > top {
				break
			}
			if u64(v[8:16]) <= cutoff {
				drop = append(drop, bytes.Clone(k))
				freed += int64(len(v) - 16)
			}
		}
		for _, k := range drop {
			if err := ents.Delete(k); err != nil {
				return 0, err
			}
			n++
		}
	}
	if freed > 0 {
		if err := s.addBytes(ch, -freed); err != nil {
			return 0, err
		}
	}
	return n, nil
}

// appendEntries stores new entries (seqs ascending, each above the log's last) with the
// channel's next ords, and returns the log's new last.
func (s stx) appendEntries(ch *ID, up *ID, news []entry, now uint64) (uint64, error) {
	r, err := s.channel(ch)
	if err != nil || r == nil {
		return 0, errCorrupt
	}
	b := s.chb(ch)
	var add int64
	var last uint64
	for _, e := range news {
		v := append(append(be(r.nextOrd), be(now)...), e.env...)
		if err := b.Bucket(kEnt).Put(entKey(up, e.seq), v); err != nil {
			return 0, err
		}
		r.nextOrd++
		add += int64(len(e.env))
		last = e.seq
	}
	if err := b.Bucket(kLog).Put(up[:], be(last)); err != nil {
		return 0, err
	}
	if err := s.putInfo(ch, r); err != nil {
		return 0, err
	}
	return last, s.addBytes(ch, add)
}

// snapshot returns the device's stored snapshot envelope and covers.
func (s stx) snapshot(ch *ID, d *ID) (env []byte, covers []dcbor.Pair, storedAt uint64, found bool, err error) {
	b := s.chb(ch)
	v := b.Bucket(kSnap).Get(d[:])
	if v == nil {
		return nil, nil, 0, false, nil
	}
	c, err := dcbor.Decode(b.Bucket(kCov).Get(d[:]))
	if err != nil || len(v) < 8 {
		return nil, nil, 0, false, errCorrupt
	}
	covers, ok := c.PairList()
	if !ok {
		return nil, nil, 0, false, errCorrupt
	}
	return v[8:], covers, u64(v[:8]), true, nil
}

func (s stx) putSnapshot(ch *ID, d *ID, env []byte, covers []dcbor.Pair, now uint64) error {
	old, _, _, found, err := s.snapshot(ch, d)
	if err != nil {
		return err
	}
	delta := int64(len(env))
	if found {
		delta -= int64(len(old))
	}
	b := s.chb(ch)
	if err := b.Bucket(kSnap).Put(d[:], append(be(now), env...)); err != nil {
		return err
	}
	if err := b.Bucket(kCov).Put(d[:], dcbor.Enc{}.Pairs(covers)); err != nil {
		return err
	}
	return s.addBytes(ch, delta)
}

type logRow struct {
	up   ID
	last uint64
}

// logs lists the channel's logs by uploader.
func (s stx) logs(ch *ID) []logRow {
	var out []logRow
	s.chb(ch).Bucket(kLog).ForEach(func(k, v []byte) error {
		var r logRow
		copy(r.up[:], k)
		r.last = u64(v)
		out = append(out, r)
		return nil
	})
	return out
}

// scan calls f with the uploader's entries above after, ascending, until f is false.
// It returns the lowest seq held (0 if none), found by the same cursor.
func (s stx) firstHeld(ch *ID, up *ID) (uint64, bool) {
	c := s.chb(ch).Bucket(kEnt).Cursor()
	k, _ := c.Seek(up[:])
	if k == nil || !bytes.HasPrefix(k, up[:]) {
		return 0, false
	}
	return u64(k[32:]), true
}

func (s stx) scan(ch *ID, up *ID, after uint64, f func(seq, ord uint64, env []byte) bool) {
	if after == ^uint64(0) {
		return
	}
	c := s.chb(ch).Bucket(kEnt).Cursor()
	for k, v := c.Seek(entKey(up, after+1)); k != nil && bytes.HasPrefix(k, up[:]); k, v = c.Next() {
		if len(v) < 16 || !f(u64(k[32:]), u64(v[:8]), v[16:]) {
			return
		}
	}
}

type snapRow struct {
	device ID
	env    []byte
	covers []dcbor.Pair
}

func (s stx) snapshots(ch *ID) ([]snapRow, error) {
	var out []snapRow
	b := s.chb(ch)
	err := b.Bucket(kSnap).ForEach(func(k, v []byte) error {
		var r snapRow
		copy(r.device[:], k)
		env, covers, _, _, err := s.snapshot(ch, &r.device)
		r.env, r.covers = env, covers
		out = append(out, r)
		return err
	})
	return out, err
}

// dump is the canonical store dump the conformance suite digests
// (docs/reference/relay-protocol.md, "Store digest").
func (s stx) dump() ([]byte, error) {
	chans := s.tx.Bucket(kChannels)
	var ids []ID
	chans.ForEachBucket(func(k []byte) error {
		var id ID
		copy(id[:], k)
		ids = append(ids, id)
		return nil
	})
	e := dcbor.Enc{}.Array(len(ids))
	for i := range ids {
		ch := &ids[i]
		c, err := s.channel(ch)
		if err != nil {
			return nil, err
		}
		b := s.chb(ch)
		e = e.Array(9).Bytes(ch[:]).Text(c.app).Bytes(c.household[:]).Uint(c.gen).Uint(c.nextOrd).Uint(c.lastWrite)
		dev := b.Bucket(kDev)
		e = e.Array(dev.Stats().KeyN)
		err = dev.ForEach(func(k, _ []byte) error {
			var d ID
			copy(d[:], k)
			r, err := s.device(ch, &d)
			if err != nil {
				return err
			}
			e = e.Array(5).Bytes(k).Raw(r.encode()[1:])
			return nil
		})
		if err != nil {
			return nil, err
		}
		logs := s.logs(ch)
		e = e.Array(len(logs))
		for _, l := range logs {
			var rows dcbor.Enc
			n := 0
			s.scan(ch, &l.up, 0, func(seq, ord uint64, env []byte) bool {
				v := b.Bucket(kEnt).Get(entKey(&l.up, seq))
				h := sha256.Sum256(env)
				rows = rows.Array(4).Uint(seq).Uint(ord).Uint(u64(v[8:16])).Bytes(h[:])
				n++
				return true
			})
			e = e.Array(3).Bytes(l.up[:]).Uint(l.last).Array(n).Raw(rows)
		}
		snaps := b.Bucket(kSnap)
		e = e.Array(snaps.Stats().KeyN)
		err = snaps.ForEach(func(k, _ []byte) error {
			var d ID
			copy(d[:], k)
			env, covers, at, _, err := s.snapshot(ch, &d)
			if err != nil {
				return err
			}
			h := sha256.Sum256(env)
			e = e.Array(4).Bytes(k).Uint(at).Bytes(h[:]).Pairs(covers)
			return nil
		})
		if err != nil {
			return nil, err
		}
	}
	return e, nil
}
