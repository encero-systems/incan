//! Snapshot descriptor, store binding and directory key codecs, pinned to the reference worker host (release run 6 on
//! f0fa3b405d): the outputs of `run1/golden/worker-snapshot-1.incn` in the runtime lane.

use crate::record_directory::directory_member_key;
use crate::record_snapshot::{
    SnapshotHandle, StoreView, canonical_roots, encode_snapshot_handle, encode_snapshot_record, encode_store_binding,
    encode_store_view, portable_store_view, read_snapshot_record, snapshot_handle_digest, snapshot_record_key,
    staged_key, valid_digest,
};

const D1: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const D2: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
const STORE_BINDING: &str = "43524d420101005001010773747669657700131702b09273cf12b3cde99a4e8af1303f33bce000bdf5cdbcba9248f67a1fac01002301000c161e0b686f6d652022762220c3a9092f622f62616b6572790773746f72652d31";
const SNAPSHOT_RECORD: &str = "43524d420101009601010107636d70736e700079341e596a1090557eee35ec2d717a75fbbfc98f4be932d0f5786b29a0aaaa3c010069010019212b373d525a5f18696e63616e2e636f6d70696c65722d7265636f7264732f310300000000000000096c656173652d2278220b6d616e69666573742dc3a9057265762d32020100081007696e63616e2e6107696e63616e2e620773746f72652d3104686f6d65";
const PORTABLE_VIEW: &str = "7b22686f6d655f76696577223a22686f6d65205c22765c2220c3a9222c226c6f6361746f72223a222f622f62616b657279222c22736368656d61223a22696e63616e2e73746f72652d766965772f31222c2273746f72655f6964223a2273746f72652d31227d0a";
const SNAPSHOT_DIGEST: &str = "e96424936c2ec8c6c51662e1e46b88409701e4c99de046ee7c5f1a649c26d2c7";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn handle(generation: i64, roots: &[&str]) -> SnapshotHandle {
    SnapshotHandle {
        store_id: "store-1".into(),
        generation,
        manifest: "manifest-é".into(),
        view: "home".into(),
        view_revision: "rev-2".into(),
        allowed_roots: roots.iter().map(|root| (*root).to_owned()).collect(),
        lease: "lease-\"x\"".into(),
        format_profile: "incan.compiler-records/1".into(),
    }
}

#[test]
fn store_views_keep_their_json_portable_and_crumb_bytes() -> Result<(), String> {
    let view = StoreView {
        store_id: "store-1".into(),
        locator: "/b/bakery".into(),
        home_view: "home \"v\" é".into(),
    };
    assert_eq!(
        encode_store_view(&view)?,
        r#"{"home_view":"home \"v\" é","locator":"/b/bakery","schema":"incan.store-view/1","store_id":"store-1"}"#
    );
    assert_eq!(hex(&portable_store_view(&view)?), PORTABLE_VIEW);
    assert_eq!(hex(&encode_store_binding(&view)?), STORE_BINDING);
    Ok(())
}

#[test]
fn snapshot_descriptors_keep_their_json_crumb_and_digest() -> Result<(), String> {
    assert_eq!(
        encode_snapshot_handle(&handle(3, &["incan.a", "incan.b\tsub"]))?,
        r#"{"allowed_roots":["incan.a","incan.b\tsub"],"format_profile":"incan.compiler-records/1","generation":3,"lease":"lease-\"x\"","manifest":"manifest-é","schema":"incan.compiler-snapshot/1","store_id":"store-1","view":"home","view_revision":"rev-2"}"#
    );
    let canonical = handle(3, &["incan.a", "incan.b"]);
    let record = encode_snapshot_record(&canonical)?;
    assert_eq!(hex(&record), SNAPSHOT_RECORD);
    let refusal = "snapshot descriptor requires a positive generation and canonical allowed roots";
    assert_eq!(encode_snapshot_record(&handle(0, &["incan.a"])).err().as_deref(), Some(refusal));
    assert_eq!(encode_snapshot_record(&handle(1, &["incan.b", "incan.a"])).err().as_deref(), Some(refusal));
    assert_eq!(snapshot_handle_digest(&canonical)?, SNAPSHOT_DIGEST);
    assert_eq!(read_snapshot_record(&record, SNAPSHOT_DIGEST)?, canonical);
    assert_eq!(read_snapshot_record(&record, D1).err().as_deref(), Some("descriptor digest mismatch"));
    assert_eq!(read_snapshot_record(&record[..30], SNAPSHOT_DIGEST).err().as_deref(), Some("descriptor digest mismatch"));
    let legacy = encode_snapshot_handle(&handle(2, &["incan.a"]))?;
    let legacy_digest = crate::record_snapshot::digest(legacy.as_bytes());
    assert_eq!(read_snapshot_record(legacy.as_bytes(), &legacy_digest)?, handle(2, &["incan.a"]));
    Ok(())
}

#[test]
fn keys_digests_and_roots_keep_their_spelling() {
    assert_eq!(snapshot_record_key(D2), format!("incan.compiler.snapshot\t{D2}"));
    assert_eq!(staged_key("value", D1), format!("incan.compiler.staged-value\t{D1}"));
    assert!(valid_digest(D1) && valid_digest(D2));
    assert!(!valid_digest("ABC") && !valid_digest(&D2.to_uppercase()));
    let roots = |values: &[&str]| values.iter().map(|value| (*value).to_owned()).collect::<Vec<_>>();
    assert_eq!(canonical_roots(&roots(&["b", "a", "b"])), Ok(roots(&["a", "b"])));
    assert_eq!(
        canonical_roots(&roots(&[])).err().as_deref(),
        Some("snapshot descriptor requires at least one allowed root")
    );
    for invalid in [&["a", ""][..], &["a\tb"][..], &[" a"][..]] {
        assert_eq!(
            canonical_roots(&roots(invalid)).err().as_deref(),
            Some("snapshot allowed roots must be nonempty record-family names")
        );
    }
    assert_eq!(canonical_roots(&roots(&["z"])), Ok(roots(&["z"])));
}

#[test]
fn directory_member_keys_keep_their_order_and_refusals() {
    let member = |key: String, revision: &str| directory_member_key("seal", &key, revision);
    assert_eq!(
        member(format!("loaf/a/b/c/00000001/{D1}"), D2),
        Ok(format!("seal\tloaf/a/b/c/{D1}/00000001/{D2}"))
    );
    let ordinal = Err("invalid compiler directory ordinal".to_owned());
    assert_eq!(member(format!("loaf/a/b/c/0000001/{D1}"), D2), ordinal);
    assert_eq!(member(format!("loaf/a/b/c/0000000x/{D1}"), D2), ordinal);
    let identity = Err("invalid compiler directory member identity".to_owned());
    assert_eq!(member(format!("pkg/a/b/c/00000001/{D1}"), D2), identity);
    assert_eq!(member(format!("loaf/a/b/c/00000001/{D1}"), "short"), identity);
    assert_eq!(
        member(format!("loaf/a/b/c/123456789/{D2}"), D1),
        Ok(format!("seal\tloaf/a/b/c/{D2}/123456789/{D1}"))
    );
}
