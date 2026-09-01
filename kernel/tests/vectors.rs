//! The shared vectors in `vectors/` were built by hand in Python and signed by
//! pyca/cryptography (`vectors/make_vectors.py`). No expected byte here comes from
//! the code under test.

use hearth_sync_kernel::keys::{DeviceSigner, HouseholdRoot, SoftSigner};
use hearth_sync_kernel::op::{self, Op, Value};
use hearth_sync_kernel::replica::{Config, Replica};
use hearth_sync_kernel::store::OpStore;
use serde_json::Value as J;

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn load(name: &str) -> J {
    let path = format!("{}/../vectors/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn arr<const N: usize>(s: &str) -> [u8; N] {
    unhex(s).try_into().unwrap()
}

#[test]
fn enroll_key_matches_independent_hkdf() {
    let v = load("ops_v1.json");
    let root = HouseholdRoot::from_seed(arr(v["household_seed"].as_str().unwrap()));
    assert_eq!(root.enroll_public("lullaby").to_vec(), unhex(v["enroll_public"].as_str().unwrap()));
}

#[test]
fn every_valid_op_round_trips_and_re_signs_byte_for_byte() {
    let v = load("ops_v1.json");
    let signers: std::collections::BTreeMap<String, SoftSigner> = v["devices"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(_, d)| {
            (d["public"].as_str().unwrap().to_string(), SoftSigner::from_secret(arr(d["seed"].as_str().unwrap())))
        })
        .collect();
    for o in v["valid_in_order"].as_array().unwrap() {
        let name = o["name"].as_str().unwrap();
        let signed = unhex(o["signed"].as_str().unwrap());
        let (op, id) = Op::decode_verified(&signed).unwrap_or_else(|r| panic!("{name}: {r:?}"));
        assert_eq!(id.to_vec(), unhex(o["id"].as_str().unwrap()), "{name} id");
        assert_eq!(op.encode(), signed, "{name}: decode then encode");
        assert_eq!(op.signable_bytes(), unhex(o["signable"].as_str().unwrap()), "{name} signable");
        // Ed25519 is deterministic: the kernel's signer reproduces pyca's signature.
        let signer = &signers[o["device"].as_str().unwrap()];
        assert_eq!(signer.sign(&op.signable_bytes()).to_vec(), unhex(o["sig"].as_str().unwrap()), "{name} sig");
        assert_eq!(op::op_id(&signed).unwrap(), id);
    }
}

#[test]
fn valid_chain_delivers_and_folds_to_the_hand_written_view() {
    let v = load("ops_v1.json");
    let now = v["ingest_now_millis"].as_u64().unwrap();
    let mut r: Replica = Replica::observer("lullaby", arr(v["enroll_public"].as_str().unwrap()), Config::default());
    let ops: Vec<Vec<u8>> =
        v["valid_in_order"].as_array().unwrap().iter().map(|o| unhex(o["signed"].as_str().unwrap())).collect();
    // Deliver in reverse to exercise pending parents too.
    let rep = r.ingest(ops.iter().rev(), now);
    assert!(rep.rejected.is_empty(), "{:?}", rep.rejected);
    assert_eq!(r.store().len(), ops.len());
    assert_eq!(r.pending_count(), 0);

    let want = &v["expected_view_after_valid"];
    let s = r.state();
    let r1 = s.row("feeds", "r1").expect("r1 restored");
    let w1 = want["feeds/r1"]["fields"].as_object().unwrap();
    assert_eq!(r1.len(), w1.len());
    assert_eq!(r1["ml"], Value::Int(w1["ml"].as_i64().unwrap()));
    assert_eq!(r1["delta"], Value::Int(-5));
    assert_eq!(r1["note"], Value::Text("left side".into()));
    assert_eq!(r1["done"], Value::Bool(true));
    assert_eq!(r1["extra"], Value::Null);
    assert_eq!(r1["blob"], Value::Bytes(unhex(w1["blob"].as_str().unwrap())));
    assert_eq!(s.row("feeds", "r2").unwrap()["ml"], Value::Int(90));
    assert!(s.set("groceries").is_empty());
    let votes: Vec<Value> = s.stream("votes").into_iter().map(|(_, v)| v).collect();
    assert_eq!(votes, vec![Value::Text("alice>bob".into())]);
    let enrolled: Vec<String> = s.enrolled.keys().map(|d| hearth_sync_kernel::hex(d)).collect();
    let want_enrolled: Vec<String> =
        want["enrolled"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
    assert_eq!(enrolled, want_enrolled);
    let forgotten: Vec<String> = s.forgotten().iter().map(|d| hearth_sync_kernel::hex(d)).collect();
    assert_eq!(forgotten, vec![want["forgotten"][0].as_str().unwrap().to_string()]);
    assert!(r.excluded().is_empty(), "B's ops are all inside the cut");
}

#[test]
fn hostile_vectors_are_rejected_with_the_named_reason() {
    let v = load("ops_v1.json");
    let now = v["ingest_now_millis"].as_u64().unwrap();
    let ops: Vec<Vec<u8>> =
        v["valid_in_order"].as_array().unwrap().iter().map(|o| unhex(o["signed"].as_str().unwrap())).collect();
    for h in v["hostile"].as_array().unwrap() {
        let name = h["name"].as_str().unwrap();
        let mut r: Replica = Replica::observer("lullaby", arr(v["enroll_public"].as_str().unwrap()), Config::default());
        r.ingest(&ops[..2], now); // a_enroll, a_put
        if name == "observed_not_in_past" {
            r.ingest(&ops, now);
        }
        if name == "child_of_rejected" {
            let parent = &v["hostile"][0];
            assert_eq!(parent["name"], "bad_signature");
            r.ingest([unhex(parent["signed"].as_str().unwrap())], now);
        }
        let bytes = unhex(h["signed"].as_str().unwrap());
        let rep = r.ingest([&bytes], now);
        let id: [u8; 32] = arr(h["id"].as_str().unwrap());
        let got = rep.rejected.iter().find(|(i, _)| *i == id).map(|(_, why)| why.code());
        assert_eq!(got, Some(h["reject"].as_str().unwrap()), "{name}: {rep:?}");
        assert!(!r.store().contains(&id), "{name} must not be delivered");
    }
}

/// The spike's independent vector still hashes to the same id under the kernel's
/// canonical-only `op_id` and verifies under strict Ed25519 (its body format predates v1).
#[test]
fn spike_vector_primitives_still_hold() {
    let path = format!("{}/../spike/vectors/op_vector_v1.json", env!("CARGO_MANIFEST_DIR"));
    let v: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let signed = unhex(v["signed"].as_str().unwrap());
    assert_eq!(
        op::op_id(&signed).unwrap().to_vec(),
        unhex("50e9a80b1d54d8bab138f3c5be8215bb389e3c779d5eab31c86ab9a7ab030057")
    );
    assert!(op::verify_strict(
        &arr(v["device"].as_str().unwrap()),
        &unhex(v["signable"].as_str().unwrap()),
        &arr(v["sig"].as_str().unwrap())
    ));
}
