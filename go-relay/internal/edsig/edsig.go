// Package edsig is strict Ed25519 verification, matching ed25519-dalek's verify_strict
// (the kernel's op::verify_strict) on every input.
//
// crypto/ed25519 already refuses a non-canonical s (s >= L), an undecodable public key,
// and compares the recomputed R by its encoding, as dalek does. What it does not do is
// refuse small-order points: dalek's verify_strict rejects a signature whose R, or whose
// public key A, has small order. Without that check a small-order household key would
// verify signatures anyone can make. Both libraries decode the 255-bit y coordinate
// without requiring it to be below p, so a point has small order exactly when y mod p
// is the y of one of the eight small-order points: 0, 1, p-1 and the two order-8 ys.
package edsig

import (
	"crypto/ed25519"
	"math/big"
)

var (
	p          = new(big.Int).Sub(new(big.Int).Lsh(big.NewInt(1), 255), big.NewInt(19))
	smallOrder = func() []*big.Int {
		y8, _ := new(big.Int).SetString("2707385501144840649318225287225658788936804267575313519463743609750303402022", 10)
		return []*big.Int{
			big.NewInt(0),                      // order 4: (±sqrt(-1), 0)
			big.NewInt(1),                      // the identity
			new(big.Int).Sub(p, big.NewInt(1)), // order 2: (0, -1)
			y8,                                 // order 8
			new(big.Int).Sub(p, y8),            // order 8
		}
	}()
)

// SmallOrder reports whether enc, a 32-byte point encoding, decodes to a point of small
// order (whether or not its y is canonical, and whatever its sign bit).
func SmallOrder(enc []byte) bool {
	if len(enc) != 32 {
		return false
	}
	le := make([]byte, 32)
	for i := range le {
		le[i] = enc[31-i]
	}
	le[0] &= 0x7f // the sign bit
	y := new(big.Int).SetBytes(le)
	y.Mod(y, p)
	for _, s := range smallOrder {
		if y.Cmp(s) == 0 {
			return true
		}
	}
	return false
}

// VerifyStrict reports whether sig is a strict Ed25519 signature of msg by pk.
func VerifyStrict(pk []byte, msg []byte, sig []byte) bool {
	if len(pk) != ed25519.PublicKeySize || len(sig) != ed25519.SignatureSize {
		return false
	}
	if SmallOrder(sig[:32]) || SmallOrder(pk) {
		return false
	}
	return ed25519.Verify(ed25519.PublicKey(pk), msg, sig)
}
