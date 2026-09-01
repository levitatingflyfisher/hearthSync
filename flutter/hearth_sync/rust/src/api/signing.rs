//! Native Ed25519 for the Dart signers (ADR 0012, "Signing").
//!
//! The device key stays platform-side: it lives in the platform's secure storage,
//! the Dart signer reads it, and passes the 32-byte seed with each message. Nothing
//! here keeps it, and the kernel never sees it (ADR 0011): these are free functions
//! outside `Kernel`, over the kernel's own `keys::SoftSigner`, so there is no second
//! Ed25519 implementation. It is native code on the platforms the plugin builds for
//! (Android and Linux today; iOS would add a `staticlib`, other desktops their plugin
//! entries), and on the web the same code in WASM, behind WebCrypto where the browser
//! has Ed25519.

use flutter_rust_bridge::frb;
use hearth_sync_kernel::api::ApiError;
use hearth_sync_kernel::keys::{DeviceSigner, SoftSigner};
use zeroize::Zeroizing;

fn signer(seed: Vec<u8>) -> Result<SoftSigner, ApiError> {
    let seed = Zeroizing::new(seed);
    let secret: [u8; 32] =
        seed.as_slice().try_into().map_err(|_| ApiError::BadArgument("an Ed25519 seed is 32 bytes".into()))?;
    Ok(SoftSigner::from_secret(secret))
}

/// The Ed25519 public key (32 bytes) of a 32-byte seed.
#[frb(sync)]
pub fn ed25519_public_key(seed: Vec<u8>) -> Result<Vec<u8>, ApiError> {
    Ok(signer(seed)?.device().to_vec())
}

/// An Ed25519 signature (64 bytes, RFC 8032, deterministic) over `message`.
#[frb(sync)]
pub fn ed25519_sign(seed: Vec<u8>, message: Vec<u8>) -> Result<Vec<u8>, ApiError> {
    Ok(signer(seed)?.sign(&message).to_vec())
}
