//! The spike's pure core: the primitives the sync kernel design (§2.1, §9) commits to.
//! No I/O, no clock, no randomness: every input arrives as an argument.

use anyhow::{bail, Result};


/// A signed op as the spike encodes it (a cut-down §2.1 op).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedOp {
    /// dCBOR bytes of the op without `sig` (what the device signs).
    pub signable: Vec<u8>,
    /// dCBOR bytes of the op with `sig` (what travels and is hashed).
    pub signed: Vec<u8>,
    /// SHA-256 of `signed`.
    pub id: [u8; 32],
    pub device: [u8; 32],
    pub sig: [u8; 64],
}

use dcbor::{Map, CBOR};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

const FORMAT_V: u64 = 1;
// Integer keys in field order; dCBOR sorts map keys by encoded bytes, so this is also wire order.
const K_V: u64 = 0;
const K_APP: u64 = 1;
const K_DEVICE: u64 = 2;
const K_PARENTS: u64 = 3;
const K_HLC: u64 = 4;
const K_BODY: u64 = 5;
const K_SIG: u64 = 6;

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// `id = SHA-256(bytes)`, but only for bytes that are already canonical dCBOR.
/// Decoding with dcbor rejects unsorted keys, non-shortest ints, indefinite
/// lengths and trailing bytes, so two implementations cannot hash two spellings
/// of one op to two ids.
pub fn op_id(op: &[u8]) -> Result<[u8; 32]> {
    let decoded = CBOR::try_from_data(op).map_err(|e| anyhow::anyhow!("not dCBOR: {e}"))?;
    // Belt and braces: re-encoding must reproduce the input exactly.
    if decoded.to_cbor_data() != op {
        bail!("not canonical dCBOR");
    }
    Ok(sha256(op))
}

pub fn verify_ed25519(pk: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    let (Ok(pk), Ok(sig)) = (<[u8; 32]>::try_from(pk), <[u8; 64]>::try_from(sig)) else {
        return false;
    };
    let Ok(vk) = VerifyingKey::from_bytes(&pk) else {
        return false;
    };
    // Strict: rejects small-order keys and non-canonical S, the malleability a
    // Byzantine peer would use to mint two ids for one op.
    vk.verify_strict(msg, &Signature::from_bytes(&sig)).is_ok()
}

fn signable_map(
    app: &str,
    device: &[u8; 32],
    parents: &[[u8; 32]],
    hlc_millis: u64,
    hlc_counter: u32,
    body: &[u8],
) -> Map {
    let mut sorted: Vec<[u8; 32]> = parents.to_vec();
    sorted.sort();
    let parents: Vec<CBOR> = sorted.iter().map(CBOR::to_byte_string).collect();
    let mut m = Map::new();
    m.insert(K_V, FORMAT_V);
    m.insert(K_APP, app);
    m.insert(K_DEVICE, CBOR::to_byte_string(device));
    m.insert(K_PARENTS, parents);
    m.insert(K_HLC, vec![CBOR::from(hlc_millis), CBOR::from(hlc_counter as u64)]);
    m.insert(K_BODY, CBOR::to_byte_string(body));
    m
}

pub fn encode_signable(
    app: &str,
    device: &[u8; 32],
    parents: &[[u8; 32]],
    hlc_millis: u64,
    hlc_counter: u32,
    body: &[u8],
) -> Vec<u8> {
    signable_map(app, device, parents, hlc_millis, hlc_counter, body).cbor_data()
}

/// Test/demo signer: the real kernel never holds the device secret (§6: Dart signs).
pub fn sign_op(
    seed: &[u8; 32],
    app: &str,
    parents: &[[u8; 32]],
    hlc_millis: u64,
    hlc_counter: u32,
    body: &[u8],
) -> SignedOp {
    let sk = SigningKey::from_bytes(seed);
    let device = sk.verifying_key().to_bytes();
    let mut m = signable_map(app, &device, parents, hlc_millis, hlc_counter, body);
    let signable = m.cbor_data();
    let sig = sk.sign(&signable).to_bytes();
    m.insert(K_SIG, CBOR::to_byte_string(sig));
    let signed = m.cbor_data();
    let id = sha256(&signed);
    SignedOp { signable, signed, id, device, sig }
}

/// Decode a signed op, rebuild its signable bytes, check the signature.
pub fn verify_op(signed: &[u8]) -> Result<bool> {
    op_id(signed)?;
    let mut m = CBOR::try_from_data(signed)?.try_into_map()?;
    let get_bytes = |m: &Map, k: u64| -> Result<Vec<u8>> {
        Ok(m.get::<u64, CBOR>(k)
            .ok_or_else(|| anyhow::anyhow!("missing key {k}"))?
            .try_into_byte_string()?)
    };
    let sig = get_bytes(&m, K_SIG)?;
    let device = get_bytes(&m, K_DEVICE)?;
    // Closed schema: exactly the seven known keys.
    for k in m.iter().map(|(k, _)| k.clone()) {
        let k: u64 = k.try_into()?;
        if k > K_SIG {
            bail!("unknown key {k}");
        }
    }
    if m.len() != 7 {
        bail!("wrong field count");
    }
    let mut rebuilt = Map::new();
    for (k, v) in m.iter() {
        if u64::try_from(k.clone())? != K_SIG {
            rebuilt.insert(k.clone(), v.clone());
        }
    }
    m = rebuilt;
    Ok(verify_ed25519(&device, &m.cbor_data(), &sig))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
    fn arr<const N: usize>(s: &str) -> [u8; N] {
        hex(s).try_into().unwrap()
    }

    // FIPS 180-2 Appendix B.1: SHA-256("abc").
    #[test]
    fn sha256_matches_fips_abc_vector() {
        assert_eq!(
            sha256(b"abc").to_vec(),
            hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
    }

    // RFC 8032 §7.1, TEST 1 (empty message).
    const RFC_SK: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
    const RFC_PK: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
    const RFC_SIG: &str = "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b";

    #[test]
    fn verify_accepts_rfc8032_test1() {
        assert!(verify_ed25519(&hex(RFC_PK), b"", &hex(RFC_SIG)));
    }

    #[test]
    fn verify_rejects_flipped_bit_and_bad_lengths() {
        let mut sig = hex(RFC_SIG);
        sig[0] ^= 1;
        assert!(!verify_ed25519(&hex(RFC_PK), b"", &sig));
        assert!(!verify_ed25519(&hex(RFC_PK), b"x", &hex(RFC_SIG)));
        assert!(!verify_ed25519(&hex(RFC_PK)[..31], b"", &hex(RFC_SIG)));
        assert!(!verify_ed25519(&hex(RFC_PK), b"", &hex(RFC_SIG)[..63]));
    }

    #[test]
    fn op_id_hashes_canonical_dcbor() {
        // {1: "a", 10: h'00'}, hand-encoded in canonical key order.
        let canonical = hex("a20161610a4100");
        assert_eq!(op_id(&canonical).unwrap(), sha256(&canonical));
    }

    #[test]
    fn op_id_rejects_non_canonical_cbor() {
        // Same map, keys out of order.
        assert!(op_id(&hex("a20a41000161 61".replace(' ', "").as_str())).is_err());
        // Integer 1 in a non-shortest form.
        assert!(op_id(&hex("1801")).is_err());
        // Indefinite-length byte string.
        assert!(op_id(&hex("5f4100ff")).is_err());
        // Trailing garbage.
        assert!(op_id(&hex("0100")).is_err());
    }

    #[test]
    fn signable_encoding_is_hand_checkable() {
        let device = [0x11u8; 32];
        let parents = [[0x22u8; 32]];
        let got = encode_signable("x", &device, &parents, 1000, 2, &[0xAB]);
        // {0: 1, 1: "x", 2: h'11'*32, 3: [h'22'*32], 4: [1000, 2], 5: h'AB'}
        let mut want = hex("a6");
        want.extend(hex("0001"));
        want.extend(hex("016178"));
        want.extend(hex("025820"));
        want.extend([0x11u8; 32]);
        want.extend(hex("03815820"));
        want.extend([0x22u8; 32]);
        want.extend(hex("04821903e802"));
        want.extend(hex("0541ab"));
        assert_eq!(got, want);
    }

    #[test]
    fn rfc_seed_signs_and_verifies_an_op() {
        let seed: [u8; 32] = arr(RFC_SK);
        let op = sign_op(&seed, "lullaby", &[], 1_727_000_000_000, 0, b"hello");
        assert_eq!(op.device.to_vec(), hex(RFC_PK));
        assert!(verify_ed25519(&op.device, &op.signable, &op.sig));
        assert_eq!(op.id, op_id(&op.signed).unwrap());
        assert!(verify_op(&op.signed).unwrap());
        // Ed25519 is deterministic: same inputs, same op, same id.
        assert_eq!(op, sign_op(&seed, "lullaby", &[], 1_727_000_000_000, 0, b"hello"));
    }

    #[test]
    fn tampered_op_fails_verification() {
        let seed: [u8; 32] = arr(RFC_SK);
        let op = sign_op(&seed, "lullaby", &[], 5, 0, b"hello");
        let mut bad = op.signed.clone();
        let n = bad.len();
        // The body sits before the 64-byte sig; flip its last byte ('o').
        let pos = n - 64 - 3 - 1;
        assert_eq!(bad[pos], b'o');
        bad[pos] = b'O';
        assert!(!verify_op(&bad).unwrap());
    }

    // spike/vectors/op_vector_v1.json: built by hand in Python and signed by
    // pyca/cryptography, so neither dcbor nor dalek produced the expected bytes.
    #[test]
    fn reproduces_independent_python_vector() {
        let seed: [u8; 32] = arr(RFC_SK);
        let op = sign_op(&seed, "lullaby", &[], 1_727_000_000_000, 0, b"hello");
        assert_eq!(op.signed, hex("a7000101676c756c6c616279025820d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a038004821b000001921938b60000054568656c6c6f0658400c09eb39da88c88db1c75826533148a40d706c8b6dfcd324f65e40ab2d4477c273158277ed9622dee665949ee02bc3edccabaf13643e9c54a25d3bbb64f5610b"));
        assert_eq!(op.id.to_vec(), hex("50e9a80b1d54d8bab138f3c5be8215bb389e3c779d5eab31c86ab9a7ab030057"));
    }
}
