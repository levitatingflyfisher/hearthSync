// Package dcbor is the small deterministic-CBOR codec the relay needs: a strict decoder
// that accepts only canonical dCBOR (RFC 8949 §4.2 with the dCBOR profile) and only the
// item kinds a relay request or a sealed envelope can hold, and an encoder for answers,
// signed messages and the store dump.
//
// It is hand-written rather than a library because the relay must accept exactly what
// the Rust relay's dcbor 0.25 accepts (a body is refused if it does not decode, or if
// it re-encodes to other bytes), and must refuse deep nesting before it recurses. The
// decoder recurses at most MaxDepth levels and never allocates more items than the
// input has bytes.
//
// Every item kind outside unsigned integers, byte strings, text strings, arrays and
// null (negative integers, maps, tags, floats, booleans, other simple values) is
// refused outright. No request or envelope field holds one, so a body carrying one is
// refused by the Rust relay too: by its shape check if not by its decoder. The answer
// is the same either way.
package dcbor

import (
	"bytes"
	"errors"
	"unicode/utf8"

	"golang.org/x/text/unicode/norm"
)

// MaxDepth is the deepest nesting of non-empty containers a decoded item may have,
// counting the root level as 1 (the Rust relay's wire::MAX_DEPTH). A request nests at
// most four deep.
const MaxDepth = 8

// Kind is what an Item holds.
type Kind uint8

const (
	Uint Kind = iota + 1
	Bytes
	Text
	Array
	Null
)

// Item is one decoded value. Bytes and Text alias the input.
type Item struct {
	Kind  Kind
	U     uint64
	B     []byte // Bytes, or the UTF-8 of Text
	Items []Item // Array
}

// ErrNotCanonical is every decoding failure: malformed, non-canonical, too deep, or a
// kind the relay never accepts.
var ErrNotCanonical = errors.New("dcbor: not a canonical item the relay accepts")

// Decode decodes exactly one item filling data.
func Decode(data []byte) (Item, error) {
	it, n, err := decode(data, 1)
	if err != nil {
		return Item{}, err
	}
	if n != len(data) {
		return Item{}, ErrNotCanonical
	}
	return it, nil
}

// head reads a canonical head: the major type and argument, and the bytes it used.
func head(data []byte) (major byte, arg uint64, n int, err error) {
	if len(data) == 0 {
		return 0, 0, 0, ErrNotCanonical
	}
	h := data[0]
	major, ai := h>>5, h&31
	switch {
	case ai < 24:
		return major, uint64(ai), 1, nil
	case ai <= 27:
		size := 1 << (ai - 24)
		if len(data) < 1+size {
			return 0, 0, 0, ErrNotCanonical
		}
		for _, b := range data[1 : 1+size] {
			arg = arg<<8 | uint64(b)
		}
		// Shortest form: each width must be needed.
		min := [4]uint64{24, 1 << 8, 1 << 16, 1 << 32}[ai-24]
		if arg < min {
			return 0, 0, 0, ErrNotCanonical
		}
		return major, arg, 1 + size, nil
	default:
		return 0, 0, 0, ErrNotCanonical // reserved, or indefinite length
	}
}

func decode(data []byte, depth int) (Item, int, error) {
	major, arg, n, err := head(data)
	if err != nil {
		return Item{}, 0, err
	}
	switch major {
	case 0:
		return Item{Kind: Uint, U: arg}, n, nil
	case 2, 3:
		if arg > uint64(len(data)-n) {
			return Item{}, 0, ErrNotCanonical
		}
		b := data[n : n+int(arg)]
		if major == 3 {
			if !utf8.Valid(b) || !norm.NFC.IsNormal(b) {
				return Item{}, 0, ErrNotCanonical
			}
			return Item{Kind: Text, B: b}, n + int(arg), nil
		}
		return Item{Kind: Bytes, B: b}, n + int(arg), nil
	case 4:
		// Each element needs at least one byte, which bounds the allocation.
		if arg > uint64(len(data)-n) {
			return Item{}, 0, ErrNotCanonical
		}
		if arg == 0 {
			return Item{Kind: Array, Items: []Item{}}, n, nil
		}
		if depth >= MaxDepth {
			return Item{}, 0, ErrNotCanonical
		}
		items := make([]Item, 0, arg)
		pos := n
		for i := uint64(0); i < arg; i++ {
			it, m, err := decode(data[pos:], depth+1)
			if err != nil {
				return Item{}, 0, err
			}
			items = append(items, it)
			pos += m
		}
		return Item{Kind: Array, Items: items}, pos, nil
	case 7:
		if data[0] == 0xf6 {
			return Item{Kind: Null}, 1, nil
		}
	}
	return Item{}, 0, ErrNotCanonical
}

// ---------------------------------------------------------------- encoding

// Enc appends canonical dCBOR.
type Enc []byte

func (e Enc) head(major byte, n uint64) Enc {
	m := major << 5
	switch {
	case n < 24:
		return append(e, m|byte(n))
	case n < 1<<8:
		return append(e, m|24, byte(n))
	case n < 1<<16:
		return append(e, m|25, byte(n>>8), byte(n))
	case n < 1<<32:
		return append(e, m|26, byte(n>>24), byte(n>>16), byte(n>>8), byte(n))
	default:
		return append(e, m|27, byte(n>>56), byte(n>>48), byte(n>>40), byte(n>>32),
			byte(n>>24), byte(n>>16), byte(n>>8), byte(n))
	}
}

func (e Enc) Uint(n uint64) Enc  { return e.head(0, n) }
func (e Enc) Bytes(b []byte) Enc { return append(e.head(2, uint64(len(b))), b...) }
func (e Enc) Text(s string) Enc  { return append(e.head(3, uint64(len(s))), s...) }
func (e Enc) Array(n int) Enc    { return e.head(4, uint64(n)) }
func (e Enc) Null() Enc          { return append(e, 0xf6) }
func (e Enc) Raw(b []byte) Enc   { return append(e, b...) }
func (e Enc) Bool(v bool) Enc {
	if v {
		return append(e, 0xf5)
	}
	return append(e, 0xf4)
}

// Pair is a (device, n) pair: a cursor or a cover.
type Pair struct {
	ID [32]byte
	N  uint64
}

// Pairs appends a list of pairs (the caller keeps them sorted).
func (e Enc) Pairs(ps []Pair) Enc {
	e = e.Array(len(ps))
	for _, p := range ps {
		e = e.Array(2).Bytes(p.ID[:]).Uint(p.N)
	}
	return e
}

// IDs appends a list of 32-byte ids.
func (e Enc) IDs(ids [][32]byte) Enc {
	e = e.Array(len(ids))
	for _, id := range ids {
		e = e.Bytes(id[:])
	}
	return e
}

// ---------------------------------------------------------------- shape helpers

// Fixed returns a byte string of exactly n bytes.
func (it Item) Fixed(n int) ([]byte, bool) {
	if it.Kind != Bytes || len(it.B) != n {
		return nil, false
	}
	return it.B, true
}

// Fixed32 returns a 32-byte byte string as an array.
func (it Item) Fixed32() ([32]byte, bool) {
	var a [32]byte
	b, ok := it.Fixed(32)
	if ok {
		copy(a[:], b)
	}
	return a, ok
}

// Fixed64 returns a 64-byte byte string as an array.
func (it Item) Fixed64() ([64]byte, bool) {
	var a [64]byte
	b, ok := it.Fixed(64)
	if ok {
		copy(a[:], b)
	}
	return a, ok
}

// Tuple returns an array's items if it has exactly n.
func (it Item) Tuple(n int) ([]Item, bool) {
	if it.Kind != Array || len(it.Items) != n {
		return nil, false
	}
	return it.Items, true
}

// IDList parses a list of 32-byte ids, strictly ascending.
func (it Item) IDList() ([][32]byte, bool) {
	if it.Kind != Array {
		return nil, false
	}
	out := make([][32]byte, 0, len(it.Items))
	for i, x := range it.Items {
		id, ok := x.Fixed32()
		if !ok || (i > 0 && bytes.Compare(out[i-1][:], id[:]) >= 0) {
			return nil, false
		}
		out = append(out, id)
	}
	return out, true
}

// PairList parses a list of [bstr32, uint] pairs, strictly ascending by id.
func (it Item) PairList() ([]Pair, bool) {
	if it.Kind != Array {
		return nil, false
	}
	out := make([]Pair, 0, len(it.Items))
	for i, x := range it.Items {
		t, ok := x.Tuple(2)
		if !ok || t[1].Kind != Uint {
			return nil, false
		}
		id, ok := t[0].Fixed32()
		if !ok || (i > 0 && bytes.Compare(out[i-1].ID[:], id[:]) >= 0) {
			return nil, false
		}
		out = append(out, Pair{ID: id, N: t[1].U})
	}
	return out, true
}
