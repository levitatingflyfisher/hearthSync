//! Hostile nesting (ADR 0003): the dCBOR decoder recurses once per nesting level with
//! no limit, so 65,000 bytes of `0x81` (an array holding an array holding ...) would
//! exhaust the stack and abort the whole process. Every decode path in the kernel
//! runs `cbor::nesting_ok` first. Each test here feeds a 65,000-deep payload through
//! one entry point, on a thread with a small stack, and asserts it is refused.
//!
//! This is its own test binary on purpose: a stack overflow aborts the process, and
//! only this binary should die if the guard ever goes missing.

mod common;
use common::{root, seed, APP, T0};

use hearth_sync_kernel::api::{
    sealed_handover, stored_info, ApiError, Collection, FieldDef, Kernel, Merge, OpenArgs, Record, Schema, ValueType,
};
use hearth_sync_kernel::cbor::{nesting_ok, MAX_DEPTH};
use hearth_sync_kernel::keys::{DeviceSigner, SoftSigner};
use hearth_sync_kernel::op::{self, Reject};
use hearth_sync_kernel::persist::{TAG_APP, TAG_META, TAG_OP};
use hearth_sync_kernel::seal::{envelope_ref, SealKeys, SealKind};
use hearth_sync_kernel::sha256;
use hearth_sync_kernel::state::State;
use hearth_sync_kernel::sync::Snapshot;
use proptest::prelude::*;

const DEEP: usize = 65_000;

/// `DEEP` nested one-element arrays around a zero: well-formed CBOR, 65,001 bytes,
/// under `MAX_OP_BYTES`.
fn deep() -> Vec<u8> {
    let mut v = vec![0x81; DEEP];
    v.push(0x00);
    v
}

/// Run `f` on a thread with a 256 KiB stack: far less than 65,000 decoder frames.
fn small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new().stack_size(256 * 1024).spawn(f).unwrap().join().unwrap()
}

fn schema() -> Schema {
    Schema {
        collections: vec![Collection {
            name: "feeds".into(),
            merge: Merge::Lww {
                fields: vec![FieldDef { name: "ml".into(), ty: ValueType::Int, nullable: true }],
                container: None,
            },
        }],
        horizon_ms: 20_000,
        keep_full_history: false,
    }
}

fn kernel(records: Vec<Record>) -> Result<Kernel, ApiError> {
    Kernel::open(OpenArgs {
        app: APP.into(),
        seed: seed().to_vec(),
        device: SoftSigner::from_secret([1; 32]).device().to_vec(),
        schema: schema(),
        records,
        now: T0,
    })
}

fn keys() -> SealKeys {
    SealKeys::derive(&root(), APP)
}

#[test]
fn the_guard_refuses_deep_input_without_recursing() {
    assert!(small_stack(|| !nesting_ok(&deep(), MAX_DEPTH)));
}

#[test]
fn op_decode_and_op_id_refuse_a_deep_op() {
    assert_eq!(small_stack(|| op::decode(&deep()).err()), Some(Reject::NotCanonical));
    assert_eq!(small_stack(|| op::op_id(&deep())), None);
}

#[test]
fn envelope_ref_refuses_a_deep_envelope() {
    assert_eq!(small_stack(|| envelope_ref(&deep())), None);
    assert!(small_stack(|| keys().open(SealKind::Op, &deep()).is_err()));
}

#[test]
fn state_and_snapshot_decoding_refuse_deep_bytes() {
    assert!(small_stack(|| State::decode(&deep()).is_err()));
    assert!(small_stack(|| Snapshot::decode(&deep()).is_err()));
}

#[test]
fn ingest_rejects_a_sealed_op_whose_plaintext_is_deep() {
    let code = small_stack(|| {
        let mut k = kernel(vec![]).unwrap();
        let env = keys().seal_op(&deep());
        let o = k.ingest(vec![env], T0).unwrap();
        o.rejected.into_iter().map(|r| (r.id, r.code)).collect::<Vec<_>>()
    });
    assert_eq!(code, vec![(sha256(&deep()).to_vec(), "not_canonical".to_string())]);
}

#[test]
fn a_deep_envelope_handed_to_ingest_is_a_bad_seal() {
    let codes = small_stack(|| {
        let mut k = kernel(vec![]).unwrap();
        k.ingest(vec![deep()], T0).unwrap().rejected.into_iter().map(|r| r.code).collect::<Vec<_>>()
    });
    assert_eq!(codes, vec!["bad_seal".to_string()]);
}

#[test]
fn sync_messages_with_a_deep_plaintext_are_bad_messages() {
    let (req, off, ho) = small_stack(|| {
        let mut k = kernel(vec![]).unwrap();
        let msg = keys().seal(SealKind::Msg, None, &deep());
        (k.request(msg.clone()).err(), k.accept(msg.clone(), T0).err(), k.offer(msg).err())
    });
    assert_eq!(req, Some(ApiError::BadMessage));
    assert_eq!(off, Some(ApiError::BadMessage));
    assert_eq!(ho, Some(ApiError::BadMessage));
}

#[test]
fn a_snapshot_with_a_deep_plaintext_is_a_bad_message() {
    let e = small_stack(|| {
        let mut k = kernel(vec![]).unwrap();
        let snap = keys().seal(SealKind::Snapshot, Some(&[7; 32]), &deep());
        k.adopt_snapshot(snap, vec![], T0).err()
    });
    assert_eq!(e, Some(ApiError::BadMessage));
}

#[test]
fn deep_stored_records_are_refused_at_open_and_by_the_keyless_readers() {
    for key in [vec![TAG_META], [vec![TAG_OP], vec![9; 32]].concat(), [vec![TAG_APP], vec![9; 32]].concat()] {
        let r = small_stack(move || {
            let recs = vec![Record { key, value: Some(deep()) }];
            let open = kernel(recs.clone()).err();
            let info = stored_info(recs.clone());
            let handover = sealed_handover(recs, schema(), T0).map(|v| v.len());
            (open.map(|e| matches!(e, ApiError::Persist(_))), info, handover.is_ok_and(|n| n > 0))
        });
        assert_eq!(r.0, Some(true), "open refuses the record");
        assert_eq!(r.1, None, "stored_info reads nothing from it");
        assert!(!r.2, "sealed_handover hands out nothing from it");
    }
}

/// No false refusals: everything the kernel itself encodes passes the guard, with
/// room to spare (ADR 0003 records the deepest the suites encode against the limit).
#[test]
fn the_kernels_own_encodings_pass_the_guard() {
    use common::{joined, v};
    use hearth_sync_kernel::op::Value;
    let mut a = joined(1, T0);
    let mut b = joined(2, T0 + 1);
    b.ingest(a.log(), T0 + 2);
    a.ingest(b.log(), T0 + 2);
    a.put(
        "feeds",
        "r1",
        [("ml", v(120)), ("note", Value::Text("hi".into())), ("raw", Value::Bytes(vec![1, 2]))],
        T0 + 3,
    )
    .unwrap();
    a.set_add("tags", Value::Null, T0 + 4).unwrap();
    a.append("log", Value::Bool(true), T0 + 5).unwrap();
    a.delete("feeds", "r1", T0 + 6).unwrap();
    a.restore("feeds", "r1", T0 + 7).unwrap();
    a.set_remove("tags", Value::Null, T0 + 8).unwrap();
    a.forget(b.device().unwrap(), T0 + 9).unwrap();
    a.checkpoint(T0 + 10).unwrap();
    let deepest = |d: &[u8]| (1..=MAX_DEPTH).find(|&m| nesting_ok(d, m)).expect("within the limit");
    let mut max = 0;
    for op in a.log() {
        max = max.max(deepest(&op));
    }
    max = max.max(deepest(&a.state().encode()));
    let _ = a.compact(T0 + 10 + 100 * common::DAY);
    if let Some(s) = a.snapshot() {
        max = max.max(deepest(&s.encode()));
    }
    assert!(max <= MAX_DEPTH / 2, "deepest kernel encoding {max} leaves headroom under {MAX_DEPTH}");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Any nesting from 0 to twice the limit, wrapped around a scalar: the guard
    /// passes exactly what is within the limit, and every decoder refuses the rest
    /// without overflowing (up to 64 levels of recursion is safe on any stack).
    #[test]
    fn the_guard_cuts_at_the_limit(depth in 0usize..=2 * MAX_DEPTH, arr in any::<bool>()) {
        // An array level is `0x81`; a map level is `0xa1 0x00` (key 0, then the next level).
        let mut v: Vec<u8> = if arr { vec![0x81; depth] } else { (0..depth).flat_map(|_| [0xa1, 0x00]).collect() };
        v.push(0x00);
        // Depth counts levels of items: a scalar alone is one deep, so `depth`
        // containers around it make `depth + 1`.
        prop_assert_eq!(nesting_ok(&v, MAX_DEPTH), depth < MAX_DEPTH);
        if depth >= MAX_DEPTH {
            prop_assert!(op::decode(&v).is_err());
            prop_assert!(op::op_id(&v).is_none());
            prop_assert!(envelope_ref(&v).is_none());
            prop_assert!(State::decode(&v).is_err());
            prop_assert!(Snapshot::decode(&v).is_err());
        }
    }

    /// Random bytes: the guard never panics, and anything it refuses the decoders
    /// refuse too.
    #[test]
    fn random_bytes_never_get_past_a_refusal(data in prop::collection::vec(any::<u8>(), 0..96)) {
        if !nesting_ok(&data, MAX_DEPTH) {
            prop_assert!(op::op_id(&data).is_none());
            prop_assert!(envelope_ref(&data).is_none());
        }
    }
}
