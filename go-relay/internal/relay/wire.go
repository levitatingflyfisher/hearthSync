package relay

import (
	"crypto/sha256"

	"hearthsync/go-relay/internal/dcbor"
)

// ID is a 32-byte id: a channel, a device key, an op id or a hash.
type ID = [32]byte

type enrollReq struct {
	app       string
	household ID
	device    ID
	label     string
	auth      [64]byte
}

type forgetReq struct {
	target ID
	cut    []ID
	auth   [64]byte
	cutSeq uint64
	poster ID
	ts     uint64
	sig    [64]byte
}

type appendReq struct {
	uploader  ID
	firstSeq  uint64
	envelopes [][]byte
	ts        uint64
	sig       [64]byte
}

type snapshotReq struct {
	device   ID
	envelope []byte
	covers   []dcbor.Pair
	ts       uint64
	sig      [64]byte
}

type pullReq struct {
	reader  ID
	ts      uint64
	epoch   uint64
	nonce   [16]byte
	cursors []dcbor.Pair
	sig     [64]byte
}

type fetchReq struct {
	reader ID
	ts     uint64
	epoch  uint64
	nonce  [16]byte
	device ID
	sig    [64]byte
}

const (
	maxNameBytes = 128 // the kernel's MAX_NAME_BYTES: a label's limit
	maxCut       = 64  // the kernel's MAX_PARENTS: a Forget's cut
)

var reservedApps = map[string]bool{"encryption": true, "sync": true, "auth": true, "recovery": true, "channel": true}

// appDomainOK is the kernel's keys::app_domain_ok: 1 to 32 of [a-z0-9], not reserved.
func appDomainOK(app string) bool {
	if len(app) == 0 || len(app) > 32 || reservedApps[app] {
		return false
	}
	for i := 0; i < len(app); i++ {
		b := app[i]
		if !(b >= 'a' && b <= 'z' || b >= '0' && b <= '9') {
			return false
		}
	}
	return true
}

func uintOf(it dcbor.Item) (uint64, bool) { return it.U, it.Kind == dcbor.Uint }

func textOf(it dcbor.Item) (string, bool) { return string(it.B), it.Kind == dcbor.Text }

// all is true if every flag is.
func all(oks ...bool) bool {
	for _, ok := range oks {
		if !ok {
			return false
		}
	}
	return true
}

func parseEnroll(body []byte) (r enrollReq, ok bool) {
	c, err := dcbor.Decode(body)
	if err != nil {
		return r, false
	}
	a, ok := c.Tuple(5)
	if !ok {
		return r, false
	}
	var ok1, ok2, ok3, ok4, ok5 bool
	r.app, ok1 = textOf(a[0])
	r.household, ok2 = a[1].Fixed32()
	r.device, ok3 = a[2].Fixed32()
	r.label, ok4 = textOf(a[3])
	r.auth, ok5 = a[4].Fixed64()
	return r, all(ok1, ok2, ok3, ok4, ok5) && appDomainOK(r.app) && len(r.label) >= 1 && len(r.label) <= maxNameBytes
}

func parseForget(body []byte) (r forgetReq, ok bool) {
	c, err := dcbor.Decode(body)
	if err != nil {
		return r, false
	}
	a, ok := c.Tuple(7)
	if !ok {
		return r, false
	}
	var oks [7]bool
	r.target, oks[0] = a[0].Fixed32()
	r.cut, oks[1] = a[1].IDList()
	r.auth, oks[2] = a[2].Fixed64()
	r.cutSeq, oks[3] = uintOf(a[3])
	r.poster, oks[4] = a[4].Fixed32()
	r.ts, oks[5] = uintOf(a[5])
	r.sig, oks[6] = a[6].Fixed64()
	return r, all(oks[:]...) && len(r.cut) <= maxCut
}

func parseAppend(body []byte) (r appendReq, ok bool) {
	c, err := dcbor.Decode(body)
	if err != nil {
		return r, false
	}
	a, ok := c.Tuple(5)
	if !ok || a[2].Kind != dcbor.Array {
		return r, false
	}
	for _, e := range a[2].Items {
		if e.Kind != dcbor.Bytes {
			return r, false
		}
		r.envelopes = append(r.envelopes, e.B)
	}
	var oks [4]bool
	r.uploader, oks[0] = a[0].Fixed32()
	r.firstSeq, oks[1] = uintOf(a[1])
	r.ts, oks[2] = uintOf(a[3])
	r.sig, oks[3] = a[4].Fixed64()
	return r, all(oks[:]...) && r.firstSeq >= 1 && len(r.envelopes) >= 1
}

func parseSnapshot(body []byte) (r snapshotReq, ok bool) {
	c, err := dcbor.Decode(body)
	if err != nil {
		return r, false
	}
	a, ok := c.Tuple(5)
	if !ok || a[1].Kind != dcbor.Bytes {
		return r, false
	}
	r.envelope = a[1].B
	var oks [4]bool
	r.device, oks[0] = a[0].Fixed32()
	r.covers, oks[1] = a[2].PairList()
	r.ts, oks[2] = uintOf(a[3])
	r.sig, oks[3] = a[4].Fixed64()
	return r, all(oks[:]...)
}

func parsePull(body []byte) (r pullReq, ok bool) {
	c, err := dcbor.Decode(body)
	if err != nil {
		return r, false
	}
	a, ok := c.Tuple(6)
	if !ok {
		return r, false
	}
	var oks [6]bool
	r.reader, oks[0] = a[0].Fixed32()
	r.ts, oks[1] = uintOf(a[1])
	r.epoch, oks[2] = uintOf(a[2])
	var n []byte
	n, oks[3] = a[3].Fixed(16)
	copy(r.nonce[:], n)
	r.cursors, oks[4] = a[4].PairList()
	r.sig, oks[5] = a[5].Fixed64()
	return r, all(oks[:]...)
}

func parseFetch(body []byte) (r fetchReq, ok bool) {
	c, err := dcbor.Decode(body)
	if err != nil {
		return r, false
	}
	a, ok := c.Tuple(6)
	if !ok {
		return r, false
	}
	var oks [6]bool
	r.reader, oks[0] = a[0].Fixed32()
	r.ts, oks[1] = uintOf(a[1])
	r.epoch, oks[2] = uintOf(a[2])
	var n []byte
	n, oks[3] = a[3].Fixed(16)
	copy(r.nonce[:], n)
	r.device, oks[4] = a[4].Fixed32()
	r.sig, oks[5] = a[5].Fixed64()
	return r, all(oks[:]...)
}

// ---------------------------------------------------------------- signed messages

func signedHead(tag string, ch, signer *ID, ts uint64, rest int) dcbor.Enc {
	return dcbor.Enc{}.Array(4 + rest).Text(tag).Bytes(ch[:]).Bytes(signer[:]).Uint(ts)
}

func (r *forgetReq) signable(ch *ID) []byte {
	return signedHead("oh-relay-forget/v1", ch, &r.poster, r.ts, 4).
		Bytes(r.target[:]).IDs(r.cut).Bytes(r.auth[:]).Uint(r.cutSeq)
}

func (r *appendReq) signable(ch *ID) []byte {
	e := signedHead("oh-relay-append/v1", ch, &r.uploader, r.ts, 2).Uint(r.firstSeq).Array(len(r.envelopes))
	for _, env := range r.envelopes {
		h := sha256.Sum256(env)
		e = e.Bytes(h[:])
	}
	return e
}

func (r *snapshotReq) signable(ch *ID) []byte {
	h := sha256.Sum256(r.envelope)
	return signedHead("oh-relay-snapshot/v1", ch, &r.device, r.ts, 2).Bytes(h[:]).Pairs(r.covers)
}

func (r *pullReq) signable(ch *ID) []byte {
	return signedHead("oh-relay-pull/v1", ch, &r.reader, r.ts, 3).Uint(r.epoch).Bytes(r.nonce[:]).Pairs(r.cursors)
}

func (r *fetchReq) signable(ch *ID) []byte {
	return signedHead("oh-relay-fetch-snapshot/v1", ch, &r.reader, r.ts, 3).Uint(r.epoch).Bytes(r.nonce[:]).Bytes(r.device[:])
}

// enrollAuthMsg is the kernel's keys::enroll_auth_msg.
func enrollAuthMsg(app string, device *ID, label string) []byte {
	return dcbor.Enc{}.Array(4).Text("oh-enroll/v1").Text(app).Bytes(device[:]).Text(label)
}

// forgetAuthMsg is the kernel's keys::forget_auth_msg.
func forgetAuthMsg(app string, device *ID, cut []ID) []byte {
	return dcbor.Enc{}.Array(4).Text("oh-forget/v1").Text(app).Bytes(device[:]).IDs(cut)
}

// ChannelID is SHA-256(dCBOR ["oh-relay-channel/v1", app, household]).
func ChannelID(app string, household *ID) ID {
	return sha256.Sum256(dcbor.Enc{}.Array(3).Text("oh-relay-channel/v1").Text(app).Bytes(household[:]))
}

// envelopeRef is the kernel's seal::envelope_ref: the ref of a canonical sealed
// envelope [1, kind <= 2, ref | null, nonce24, ciphertext], or false if it has none.
func envelopeRef(env []byte) (ID, bool) {
	var ref ID
	c, err := dcbor.Decode(env)
	if err != nil {
		return ref, false
	}
	a, ok := c.Tuple(5)
	if !ok || a[0].Kind != dcbor.Uint || a[0].U != 1 || a[1].Kind != dcbor.Uint || a[1].U > 2 {
		return ref, false
	}
	if _, ok := a[3].Fixed(24); !ok || a[4].Kind != dcbor.Bytes {
		return ref, false
	}
	ref, ok = a[2].Fixed32()
	return ref, ok
}

// errorBody is ["err", code].
func errorBody(code string) []byte {
	return dcbor.Enc{}.Array(2).Text("err").Text(code)
}
