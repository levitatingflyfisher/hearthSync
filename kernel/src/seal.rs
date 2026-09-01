//! Sealing (ADR 0008): every op, snapshot and sync message leaves the kernel, for the
//! app's database, the relay or the LAN, inside an XChaCha20-Poly1305 envelope under
//! a key only the household holds. The relay never sees parents, clocks or bodies.
//!
//! - **Keys.** `HKDF-SHA256(seed, info = "openhearth.<app>.hearthsync.seal.v1")` is the
//!   AEAD key and `…hearthsync.nonce.v1` the nonce key: the same construction and app
//!   domain rule as the enroll key, in their own domains.
//! - **Envelope.** dCBOR `[1, kind, ref, nonce, ciphertext]`. `kind` is 0 op,
//!   1 snapshot, 2 message. `ref` is the op id (op), the checkpoint id (snapshot) or
//!   null (message); it travels in clear so the receiver can rebuild the AAD, and an
//!   opened op must hash to it.
//! - **AAD.** dCBOR `["oh-seal/v1", kind name, app, household, ref]`, where the
//!   household is the enroll public key. An envelope cannot be replayed into another
//!   household, another app, another op id, or as another kind.
//! - **Nonce.** The kernel draws no randomness (ADR 0001), so the 24-byte nonce is
//!   synthetic: the first 24 bytes of `HMAC-SHA256(nonce key, AAD ‖ plaintext)`.
//!   Equal inputs give equal envelopes (which leaks only that two sealed ops are the
//!   same op, already visible from `ref`), and distinct inputs collide on a nonce
//!   with probability about 2^-96 per pair, far inside XChaCha's margin.
//!
//! `vectors/seal_v1.json`, built by PyNaCl and pyca, pins every byte.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use dcbor::{CBORCase, CBOR};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::keys::HouseholdRoot;
use crate::{sha256, Id};

/// Envelope format version.
pub const SEAL_V: u64 = 1;

/// What an envelope holds. Bound into the AAD, so one kind never opens as another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SealKind {
    Op = 0,
    Snapshot = 1,
    Msg = 2,
}

impl SealKind {
    fn name(self) -> &'static str {
        match self {
            SealKind::Op => "op",
            SealKind::Snapshot => "snapshot",
            SealKind::Msg => "msg",
        }
    }
}

/// Why an envelope did not open. The api reports all of these as `bad_seal`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SealError {
    /// Not a canonical envelope of the right shape and version.
    Malformed,
    /// An envelope of another kind (an op offered as a snapshot, say).
    WrongKind,
    /// The AEAD refused it: tampered, or sealed for another household, app or ref.
    Unauthentic,
    /// An op whose plaintext does not hash to the id it was sealed under.
    IdMismatch,
}

/// The household's sealing keys for one app. Zeroised on drop.
#[derive(Clone)]
pub struct SealKeys {
    app: String,
    household: [u8; 32],
    aead: Zeroizing<[u8; 32]>,
    nonce: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for SealKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealKeys").field("app", &self.app).finish_non_exhaustive()
    }
}

impl SealKeys {
    /// The sealing keys every holder of the household seed derives for `app`.
    pub fn derive(root: &HouseholdRoot, app: &str) -> Self {
        SealKeys {
            app: app.to_string(),
            household: root.enroll_public(app),
            aead: root.derive(app, "hearthsync.seal.v1"),
            nonce: root.derive(app, "hearthsync.nonce.v1"),
        }
    }

    /// From raw parts (vectors, and a relay-side test harness).
    pub fn from_parts(app: &str, household: [u8; 32], aead: [u8; 32], nonce: [u8; 32]) -> Self {
        SealKeys { app: app.to_string(), household, aead: Zeroizing::new(aead), nonce: Zeroizing::new(nonce) }
    }

    /// The household's identity in the log (the enroll public key).
    pub fn household(&self) -> [u8; 32] {
        self.household
    }

    fn aad(&self, kind: SealKind, reference: Option<&Id>) -> Vec<u8> {
        CBOR::from(vec![
            CBOR::from("oh-seal/v1"),
            CBOR::from(kind.name()),
            CBOR::from(self.app.as_str()),
            CBOR::to_byte_string(self.household),
            reference.map(CBOR::to_byte_string).unwrap_or_else(CBOR::null),
        ])
        .to_cbor_data()
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new_from_slice(self.aead.as_slice()).expect("32-byte key")
    }

    /// Seal `plaintext` as `kind` under `reference`.
    pub fn seal(&self, kind: SealKind, reference: Option<&Id>, plaintext: &[u8]) -> Vec<u8> {
        let aad = self.aad(kind, reference);
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(self.nonce.as_slice()).expect("any key length");
        mac.update(&aad);
        mac.update(plaintext);
        let tag = mac.finalize().into_bytes();
        let mut nonce = [0u8; 24];
        nonce.copy_from_slice(&tag[..24]);
        let ct = self
            .cipher()
            .encrypt(&XNonce::from(nonce), Payload { msg: plaintext, aad: &aad })
            .expect("XChaCha20-Poly1305 seals any message the kernel makes");
        CBOR::from(vec![
            CBOR::from(SEAL_V),
            CBOR::from(kind as u64),
            reference.map(CBOR::to_byte_string).unwrap_or_else(CBOR::null),
            CBOR::to_byte_string(nonce),
            CBOR::to_byte_string(ct),
        ])
        .to_cbor_data()
    }

    /// Open an envelope that must be of `kind`. Returns its ref and plaintext.
    pub fn open(&self, kind: SealKind, envelope: &[u8]) -> Result<(Option<Id>, Vec<u8>), SealError> {
        let (k, reference, nonce, ct) = parse(envelope)?;
        if k != kind as u64 {
            return Err(SealError::WrongKind);
        }
        let aad = self.aad(kind, reference.as_ref());
        let pt = self
            .cipher()
            .decrypt(&XNonce::from(nonce), Payload { msg: &ct, aad: &aad })
            .map_err(|_| SealError::Unauthentic)?;
        Ok((reference, pt))
    }

    /// Seal an op's signed bytes under its id.
    pub fn seal_op(&self, signed: &[u8]) -> Vec<u8> {
        self.seal(SealKind::Op, Some(&sha256(signed)), signed)
    }

    /// Open a sealed op: its id and signed bytes, the id checked against the bytes.
    pub fn open_op(&self, envelope: &[u8]) -> Result<(Id, Vec<u8>), SealError> {
        let (reference, pt) = self.open(SealKind::Op, envelope)?;
        let id = reference.ok_or(SealError::Malformed)?;
        if sha256(&pt) != id {
            return Err(SealError::IdMismatch);
        }
        Ok((id, pt))
    }
}

/// The ref an envelope claims, without opening it (a relay can dedupe on it; the
/// api names a rejected envelope by it).
pub fn envelope_ref(envelope: &[u8]) -> Option<Id> {
    parse(envelope).ok().and_then(|(_, r, _, _)| r)
}

type Parsed = (u64, Option<Id>, [u8; 24], Vec<u8>);

fn parse(envelope: &[u8]) -> Result<Parsed, SealError> {
    let bad = SealError::Malformed;
    let c = crate::cbor::decode(envelope).ok_or(bad)?;
    if c.to_cbor_data() != envelope {
        return Err(bad);
    }
    let CBORCase::Array(a) = c.as_case() else { return Err(bad) };
    if a.len() != 5 {
        return Err(bad);
    }
    let uint = |c: &CBOR| match c.as_case() {
        CBORCase::Unsigned(n) => Ok(*n),
        _ => Err(bad),
    };
    let bytes = |c: &CBOR| match c.as_case() {
        CBORCase::ByteString(b) => Ok(b.data().to_vec()),
        _ => Err(bad),
    };
    if uint(&a[0])? != SEAL_V {
        return Err(bad);
    }
    let kind = uint(&a[1])?;
    if kind > SealKind::Msg as u64 {
        return Err(bad);
    }
    let reference =
        if a[2].is_null() { None } else { Some(<[u8; 32]>::try_from(bytes(&a[2])?.as_slice()).map_err(|_| bad)?) };
    let nonce = <[u8; 24]>::try_from(bytes(&a[3])?.as_slice()).map_err(|_| bad)?;
    Ok((kind, reference, nonce, bytes(&a[4])?))
}
