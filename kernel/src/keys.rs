//! Keys: the household enroll key, device signers, and the enroll / forget
//! authorisations. See ADR 0002.
//!
//! - The **household root** is the 64-byte BIP39 seed. Per the operator's Q1 ruling
//!   every device stores the 12 words, so every device can derive the enroll key.
//! - The **enroll key** is `HKDF-SHA256(seed, salt = empty, info =
//!   "openhearth.<app>.enroll.v1")`, the same construction as `key_derivation.dart`,
//!   used as an Ed25519 secret. Its public half is the household's identity in the log.
//! - A **device key** is a random Ed25519 key made on first launch (the caller passes
//!   the 32 random bytes; the kernel takes no randomness). Deriving it from the seed
//!   would let every device forge every other device's writes.

use dcbor::CBOR;
use ed25519_dalek::{Signer, SigningKey};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::op::ids_cbor;
use crate::{DeviceId, Id};

/// Segments of the frozen legacy info strings in `key_derivation.dart`; an app
/// domain equal to one of them could collide with a legacy derivation.
const RESERVED_APP_DOMAINS: [&str; 5] = ["encryption", "sync", "auth", "recovery", "channel"];

/// Same rule as `KeyDerivation._validateAppDomain`: `^[a-z0-9]+$`, not reserved.
pub fn app_domain_ok(app: &str) -> bool {
    !app.is_empty()
        && app.len() <= 32
        && app.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && !RESERVED_APP_DOMAINS.contains(&app)
}

/// The 64-byte household seed. Zeroised on drop.
#[derive(Clone)]
pub struct HouseholdRoot(Zeroizing<[u8; 64]>);

impl HouseholdRoot {
    pub fn from_seed(seed: [u8; 64]) -> Self {
        HouseholdRoot(Zeroizing::new(seed))
    }

    /// `HKDF-SHA256(seed, salt = empty, info = "openhearth.<app>.<purpose>")`, the
    /// construction `key_derivation.dart` uses. Each purpose is its own domain: the
    /// enroll key (`enroll.v1`) and the sealing keys (`hearthsync.seal.v1`,
    /// `hearthsync.nonce.v1`, ADR 0008). Panics on an invalid app domain.
    pub fn derive(&self, app: &str, purpose: &str) -> Zeroizing<[u8; 32]> {
        assert!(app_domain_ok(app), "invalid app domain {app:?}");
        let hk = Hkdf::<Sha256>::new(None, self.0.as_slice());
        let mut okm = Zeroizing::new([0u8; 32]);
        hk.expand(format!("openhearth.{app}.{purpose}").as_bytes(), okm.as_mut_slice())
            .expect("32 bytes is a valid HKDF-SHA256 length");
        okm
    }

    /// The household enroll signing key for `app`. Panics on an invalid app domain.
    pub fn enroll_key(&self, app: &str) -> SigningKey {
        SigningKey::from_bytes(&self.derive(app, "enroll.v1"))
    }

    pub fn enroll_public(&self, app: &str) -> [u8; 32] {
        self.enroll_key(app).verifying_key().to_bytes()
    }
}

/// Signs ops as one device. In v1 the platform implements this (Android Keystore,
/// Keychain, WebCrypto) and the private key never enters the kernel; v0 ships the
/// software signer below for tests and for platforms without a hardware option.
pub trait DeviceSigner: Send + Sync {
    fn device(&self) -> DeviceId;
    fn sign(&self, msg: &[u8]) -> [u8; 64];
}

/// A software Ed25519 device key; its secret is zeroised on drop.
pub struct SoftSigner(SigningKey);

impl SoftSigner {
    /// `secret` must be 32 bytes from a CSPRNG (or a test vector).
    pub fn from_secret(secret: [u8; 32]) -> Self {
        let secret = Zeroizing::new(secret);
        SoftSigner(SigningKey::from_bytes(&secret))
    }
}

impl DeviceSigner for SoftSigner {
    fn device(&self) -> DeviceId {
        self.0.verifying_key().to_bytes()
    }
    fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.0.sign(msg).to_bytes()
    }
}

/// Bytes the enroll key signs to admit `device`.
pub fn enroll_auth_msg(app: &str, device: &DeviceId, label: &str) -> Vec<u8> {
    CBOR::from(vec![CBOR::from("oh-enroll/v1"), CBOR::from(app), CBOR::to_byte_string(device), CBOR::from(label)])
        .to_cbor_data()
}

/// Bytes the enroll key signs to forget `device` at `cut`.
pub fn forget_auth_msg(app: &str, device: &DeviceId, cut: &[Id]) -> Vec<u8> {
    CBOR::from(vec![CBOR::from("oh-forget/v1"), CBOR::from(app), CBOR::to_byte_string(device), ids_cbor(cut)])
        .to_cbor_data()
}

pub fn sign_enroll(root: &HouseholdRoot, app: &str, device: &DeviceId, label: &str) -> [u8; 64] {
    root.enroll_key(app).sign(&enroll_auth_msg(app, device, label)).to_bytes()
}

pub fn sign_forget(root: &HouseholdRoot, app: &str, device: &DeviceId, cut: &[Id]) -> [u8; 64] {
    root.enroll_key(app).sign(&forget_auth_msg(app, device, cut)).to_bytes()
}
