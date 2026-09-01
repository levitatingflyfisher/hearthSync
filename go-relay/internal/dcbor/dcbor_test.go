package dcbor

import (
	"bytes"
	"encoding/hex"
	"strings"
	"testing"
)

func h(s string) []byte {
	b, err := hex.DecodeString(strings.ReplaceAll(s, " ", ""))
	if err != nil {
		panic(err)
	}
	return b
}

func TestAcceptsCanonicalItemsAndReencodesThemExactly(t *testing.T) {
	for _, s := range []string{
		"00", "17", "1818", "18ff", "190100", "1a00010000", "1b0000000100000000", "1bffffffffffffffff",
		"40", "4401020304", "60", "6161", "80", "83010203", "f6",
		"8263657272 6b6261645f72657175657374",
		"8181818181818101", // seven nested arrays: the deepest allowed
		"62c3a9",           // "é", precomposed (NFC)
	} {
		b := h(s)
		it, err := Decode(b)
		if err != nil {
			t.Errorf("%s: %v", s, err)
			continue
		}
		if got := reencode(it); !bytes.Equal(got, b) {
			t.Errorf("%s re-encodes as %x", s, got)
		}
	}
}

func reencode(it Item) Enc {
	var e Enc
	switch it.Kind {
	case Uint:
		return e.Uint(it.U)
	case Bytes:
		return e.Bytes(it.B)
	case Text:
		return e.Text(string(it.B))
	case Null:
		return e.Null()
	}
	e = e.Array(len(it.Items))
	for _, x := range it.Items {
		e = e.Raw(reencode(x))
	}
	return e
}

func TestRefusesNonCanonicalMalformedDeepAndForeignItems(t *testing.T) {
	for name, s := range map[string]string{
		"empty":             "",
		"trailing byte":     "0000",
		"uint not shortest": "1817",
		"u16 not shortest":  "1900ff",
		"u32 not shortest":  "1a0000ffff",
		"u64 not shortest":  "1b00000000ffffffff",
		"reserved ai":       "1c",
		"indefinite bytes":  "5f40ff",
		"indefinite array":  "9fff",
		"truncated bytes":   "4301",
		"truncated array":   "8201",
		"truncated head":    "19ff",
		"negative int":      "20",
		"map":               "a0",
		"tag":               "c000",
		"float":             "f93c00",
		"false":             "f4",
		"true":              "f5",
		"undefined":         "f7",
		"simple 24+":        "f820",
		"invalid utf-8":     "61ff",
		"surrogate":         "63eda080",
		"not NFC":           "6365cc81", // "e" + combining acute
		"eight nested":      "818181818181818101",
		"huge array claim":  "9bffffffffffffffff00",
		"huge bytes claim":  "5bffffffffffffffff00",
	} {
		if _, err := Decode(h(s)); err == nil {
			t.Errorf("%s (%s) decoded", name, s)
		}
	}
	if _, err := Decode(bytes.Repeat([]byte{0x81}, 100_000)); err == nil {
		t.Error("100,000 nested arrays decoded")
	}
}

func TestShapeHelpersRefuseUnsortedAndDuplicateLists(t *testing.T) {
	a, b := bytes.Repeat([]byte{1}, 32), bytes.Repeat([]byte{2}, 32)
	list := func(ids ...[]byte) Item {
		e := Enc{}.Array(len(ids))
		for _, id := range ids {
			e = e.Bytes(id)
		}
		it, err := Decode(e)
		if err != nil {
			t.Fatal(err)
		}
		return it
	}
	if _, ok := list(a, b).IDList(); !ok {
		t.Error("sorted ids refused")
	}
	if _, ok := list(b, a).IDList(); ok {
		t.Error("unsorted ids accepted")
	}
	if _, ok := list(a, a).IDList(); ok {
		t.Error("duplicate ids accepted")
	}
	pairs := func(ids ...[]byte) Item {
		e := Enc{}.Array(len(ids))
		for _, id := range ids {
			e = e.Array(2).Bytes(id).Uint(1)
		}
		it, _ := Decode(e)
		return it
	}
	if _, ok := pairs(a, b).PairList(); !ok {
		t.Error("sorted pairs refused")
	}
	if _, ok := pairs(b, a).PairList(); ok {
		t.Error("unsorted pairs accepted")
	}
}
