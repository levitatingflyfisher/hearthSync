//! The bridge surface: byte buffers in, byte buffers and plain structs out (§6).
//! All functions are `sync` so web runs on the main thread with no worker pool,
//! which is what lets a PWA work without COOP/COEP headers.

use crate::kernel;

pub struct SpikeOp {
    pub signable: Vec<u8>,
    pub signed: Vec<u8>,
    pub id: Vec<u8>,
    pub device: Vec<u8>,
    pub sig: Vec<u8>,
}

#[flutter_rust_bridge::frb(sync)]
pub fn op_id(op: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    Ok(kernel::op_id(&op)?.to_vec())
}

#[flutter_rust_bridge::frb(sync)]
pub fn verify_ed25519(pk: Vec<u8>, msg: Vec<u8>, sig: Vec<u8>) -> bool {
    kernel::verify_ed25519(&pk, &msg, &sig)
}

/// Demo only: signs with a caller-supplied seed. The kernel proper never sees a secret.
#[flutter_rust_bridge::frb(sync)]
pub fn spike_sign_op(
    seed: Vec<u8>,
    app: String,
    hlc_millis: u64,
    hlc_counter: u32,
    body: Vec<u8>,
) -> anyhow::Result<SpikeOp> {
    let seed: [u8; 32] = seed
        .try_into()
        .map_err(|_| anyhow::anyhow!("seed must be 32 bytes"))?;
    let op = kernel::sign_op(&seed, &app, &[], hlc_millis, hlc_counter, &body);
    Ok(SpikeOp {
        signable: op.signable,
        signed: op.signed,
        id: op.id.to_vec(),
        device: op.device.to_vec(),
        sig: op.sig.to_vec(),
    })
}

#[flutter_rust_bridge::frb(sync)]
pub fn verify_op(signed: Vec<u8>) -> anyhow::Result<bool> {
    kernel::verify_op(&signed)
}
