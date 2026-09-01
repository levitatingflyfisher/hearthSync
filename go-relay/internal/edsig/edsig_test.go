package edsig

import (
	"crypto/ed25519"
	"encoding/hex"
	"math/big"
	"testing"
)

func h(s string) []byte {
	b, err := hex.DecodeString(s)
	if err != nil {
		panic(err)
	}
	return b
}

// RFC 8032 §7.1, TEST 1.
var (
	rfcPK  = h("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
	rfcSig = h("e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b")
)

func TestVerifiesAnOrdinarySignature(t *testing.T) {
	if !VerifyStrict(rfcPK, nil, rfcSig) {
		t.Fatal("RFC 8032 test 1 refused")
	}
	if VerifyStrict(rfcPK, []byte("x"), rfcSig) {
		t.Fatal("wrong message accepted")
	}
}

// smallOrderEncodings is every encoding of a small-order point: each y (and y + p
// where it fits in 255 bits) with either sign bit.
func smallOrderEncodings() [][]byte {
	var out [][]byte
	for _, y := range smallOrder {
		for _, v := range []*big.Int{y, new(big.Int).Add(y, p)} {
			if v.BitLen() > 255 {
				continue
			}
			for _, sign := range []byte{0, 0x80} {
				be := v.FillBytes(make([]byte, 32))
				le := make([]byte, 32)
				for i := range le {
					le[i] = be[31-i]
				}
				le[31] |= sign
				out = append(out, le)
			}
		}
	}
	return out
}

func TestSmallOrderPointsAreRecognisedInEveryEncoding(t *testing.T) {
	encs := smallOrderEncodings()
	if len(encs) != 14 {
		t.Fatalf("%d encodings, want 14", len(encs))
	}
	for _, e := range encs {
		if !SmallOrder(e) {
			t.Errorf("%x not small order", e)
		}
	}
	if SmallOrder(rfcPK) {
		t.Error("an ordinary key is small order")
	}
}

func TestSmallOrderKeysAndRsAreRefusedEvenWhenStdlibAccepts(t *testing.T) {
	// With A = the identity and R = the identity, s = 0 satisfies [s]B = R + [k]A for
	// every message: crypto/ed25519 accepts it; a strict verifier must not.
	identity := h("0100000000000000000000000000000000000000000000000000000000000000")
	sig := append(append([]byte{}, identity...), make([]byte, 32)...)
	if !ed25519.Verify(identity, []byte("anything"), sig) {
		t.Fatal("expected crypto/ed25519 to accept the identity forgery (the reason for this package)")
	}
	if VerifyStrict(identity, []byte("anything"), sig) {
		t.Fatal("identity forgery accepted")
	}
	for _, e := range smallOrderEncodings() {
		if VerifyStrict(e, nil, rfcSig) {
			t.Errorf("small-order key %x accepted", e)
		}
		s := append(append([]byte{}, e...), rfcSig[32:]...)
		if VerifyStrict(rfcPK, nil, s) {
			t.Errorf("small-order R %x accepted", e)
		}
	}
}

func TestNonCanonicalSIsRefused(t *testing.T) {
	l, _ := new(big.Int).SetString("7237005577332262213973186563042994240857116359379907606001950938285454250989", 10)
	le := rfcSig[32:]
	be := make([]byte, 32)
	for i := range be {
		be[i] = le[31-i]
	}
	s := new(big.Int).Add(new(big.Int).SetBytes(be), l)
	if s.BitLen() > 256 {
		t.Skip("s + L does not fit")
	}
	sb := s.FillBytes(make([]byte, 32))
	sig := append([]byte{}, rfcSig[:32]...)
	for i := 31; i >= 0; i-- {
		sig = append(sig, sb[i])
	}
	if VerifyStrict(rfcPK, nil, sig) {
		t.Fatal("s + L accepted")
	}
}
