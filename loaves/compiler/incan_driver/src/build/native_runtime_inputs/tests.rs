//! Execute the receipt-bound Incan engine using genuine ordinary producer capabilities (#1337/#1698).

use std::fs;
use std::path::PathBuf;

use oven_rustc::native_loaf::prepare_resolved_native_loafs;
use oven_rustc::rustc::{resolve_active_rustc, rustc_host_target, rustc_identity};
use oven_store::OvenBuildIntent;
use oven_store::store::PublishedOvenStore;

use super::{ordinary_runtime_inputs, validate_ordinary_closure};

/// Read explicit test publisher inputs, leaving process environment and production discovery unchanged.
fn fixture_path(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("ordinary engine control requires {name}").into())
}

/// Verify actual first/repeat exchange, root/record/alias changes, current intent and missing original native bytes.
///
/// The central native runner supplies a small resolved graph with two independent target-domain units and one
/// host-domain unit. Preparation uses the real native producer in a private output tree, never remints records from
/// payload DTOs. This remains ignored outside its explicit native selector because that production preparation may
/// compile native inputs.
#[test]
#[ignore = "requires an explicit ordinary native graph and receipt-bound compiler engine"]
fn ordinary_engine_original_closure_first_repeat_and_refusals() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let rustc = resolve_active_rustc()?;
    let host = rustc_host_target(&rustc)?;
    let toolchain = rustc_identity(&rustc)?;
    let prepared = prepare_resolved_native_loafs(
        &fixture_path("INCAN_ORDINARY_ENGINE_GRAPH")?,
        &fixture_path("INCAN_ORDINARY_ENGINE_INDEX")?,
        &fixture_path("INCAN_ORDINARY_ENGINE_BLOBS")?,
        root.path(),
        &rustc,
        &host,
        "debug",
    )?;
    let units = prepared
        .graph()
        .units()
        .values()
        .filter(|unit| unit.record().source.domain == "target")
        .take(2)
        .collect::<Vec<_>>();
    let [first, second] = units.as_slice() else {
        return Err("ordinary engine fixture requires two distinct target-domain records".into());
    };
    let first_root = first.declared_root("first")?;
    let second_root = second.declared_root("second")?;
    let closure = prepared.graph().select(&[first_root.clone(), second_root.clone()])?;
    let intent = OvenBuildIntent {
        target: host.clone(),
        toolchain,
        profile: "debug".to_string(),
        features: Vec::new(),
    };
    let initial = ordinary_runtime_inputs(&closure, &intent, &rustc, &[], &[], &[])?;
    assert_eq!(
        initial,
        ordinary_runtime_inputs(&closure, &intent, &rustc, &[], &[], &[])?
    );
    assert!(!initial.keys().any(|key| key.contains("sdk")));
    let only_first = prepared.graph().select(std::slice::from_ref(&first_root))?;
    let first_inputs = ordinary_runtime_inputs(&only_first, &intent, &rustc, &[], &[], &[])?;
    assert_ne!(initial["ordinary-native-roots"], first_inputs["ordinary-native-roots"]);
    let only_second = prepared.graph().select(std::slice::from_ref(&second_root))?;
    let second_inputs = ordinary_runtime_inputs(&only_second, &intent, &rustc, &[], &[], &[])?;
    assert_ne!(
        first_inputs["ordinary-native-records"],
        second_inputs["ordinary-native-records"]
    );
    let renamed = prepared.graph().select(&[first.declared_root("renamed")?])?;
    let renamed_inputs = ordinary_runtime_inputs(&renamed, &intent, &rustc, &[], &[], &[])?;
    assert_eq!(
        first_inputs["ordinary-native-records"],
        renamed_inputs["ordinary-native-records"]
    );
    assert_ne!(
        first_inputs["ordinary-native-roots"],
        renamed_inputs["ordinary-native-roots"]
    );
    for field in ["profile", "target", "toolchain"] {
        let mut wrong = intent.clone();
        match field {
            "profile" => wrong.profile = "release".to_string(),
            "target" => wrong.target = "wrong-target".to_string(),
            _ => wrong.toolchain = "wrong-toolchain".to_string(),
        }
        assert!(ordinary_runtime_inputs(&closure, &wrong, &rustc, &[], &[], &[]).is_err());
    }
    let malformed = prepared.graph().select(&[first.declared_root("bad-alias")?])?;
    assert!(ordinary_runtime_inputs(&malformed, &intent, &rustc, &[], &[], &[]).is_err());
    let mut unknown = first_root.clone();
    unknown.record_identity = oven_store::digest_bytes(b"absent-original-owner");
    assert!(prepared.graph().select(&[unknown]).is_err());
    assert!(prepared.graph().select(&[first_root.clone(), first_root]).is_err());
    let host_unit = prepared
        .graph()
        .units()
        .values()
        .find(|unit| unit.record().source.domain == "host")
        .ok_or("ordinary engine fixture requires one host-domain native record")?;
    let host_closure = prepared.graph().select(&[host_unit.declared_root("host_unit")?])?;
    ordinary_runtime_inputs(&host_closure, &intent, &rustc, &[], &[], &[])?;
    let rustc_output = oven_store::store::digest_regular_file(&rustc)?.1;
    let rustc_commit = oven_rustc::rustc::rustc_commit_hash(&rustc).ok_or("missing rustc commit")?;
    assert!(validate_ordinary_closure(&host_closure, &intent, "wrong-host", &rustc_output, &rustc_commit).is_err());
    assert!(validate_ordinary_closure(&closure, &intent, &host, "wrong-executable", &rustc_commit).is_err());
    assert!(validate_ordinary_closure(&closure, &intent, &host, &rustc_output, "wrong-commit").is_err());
    // Locate the original record payload through actual read-only Store admission only for fixture damage.
    let shared = closure.shared_root(first.identity(), "test-native")?;
    let record_owners = PublishedOvenStore::new(&shared.store)
        .select_payloads_matching_for_execution(|manifest| manifest.identity == first.identity())?;
    let [record_owner] = record_owners.as_slice() else {
        return Err("exact original record fixture owner is missing or ambiguous".into());
    };
    let payload = record_owner
        .artifact_root
        .parent()
        .ok_or("record artifact root has no entry")?
        .join("payload");
    let held_payload = payload.with_extension("temporarily-missing");
    fs::rename(&payload, &held_payload)?;
    let missing_record = ordinary_runtime_inputs(&closure, &intent, &rustc, &[], &[], &[]);
    fs::rename(held_payload, payload)?;
    assert!(missing_record.is_err());
    assert_eq!(
        initial,
        ordinary_runtime_inputs(&closure, &intent, &rustc, &[], &[], &[])?
    );
    let original = first.output()?;
    let displaced = original.with_extension("temporarily-missing");
    fs::rename(&original, &displaced)?;
    let missing = ordinary_runtime_inputs(&closure, &intent, &rustc, &[], &[], &[]);
    fs::rename(displaced, original)?;
    assert!(missing.is_err());
    Ok(())
}
