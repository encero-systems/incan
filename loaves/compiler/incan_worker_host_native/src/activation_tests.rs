//! Activation keys, source projection and early refusals, compared with the reference worker host's outputs
//! (`run1/golden/worker-activation-1.log`, label `runtime-worker-ref-8`).

use internal_models_native::namespace_references::NamespaceReference;
use internal_models_native::records::RecordKey;

use crate::checked_activation::{projected_owner_source, resolve_checking_activation};
use crate::checking_preparation::{completed_checker_key, prepare_checking};
use crate::namespace_frames::{namespace_frame_reader, read_namespace_frame};
use crate::record_protocol::{Evaluate, Prepare};

const D1: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const HOME: &str = "/nonexistent-runtime-home";
const CHECKER: &str = "incan.checker-free-functions/21";

fn key(family: &str, identity: &str, facet: &str) -> RecordKey {
    RecordKey {
        family: family.into(),
        identity: identity.into(),
        identity_profile: "incan.identity/1".into(),
        origin: "project é".into(),
        facet: facet.into(),
        context: "current".into(),
    }
}

fn prepare(snapshot: &str, profile: &str) -> Result<String, String> {
    let request = Prepare {
        stage: "check-body".into(),
        snapshot: snapshot.into(),
        demand_root: D1.into(),
        producer_profile: profile.into(),
    };
    prepare_checking(HOME, &request).map(|prepared| prepared.prepared_root)
}

fn evaluate(reservation: &str, generation: i64) -> Option<String> {
    let evaluation = Evaluate {
        work_root: D1.into(),
        snapshot: D1.into(),
        prepared_root: D1.into(),
        reservation: reservation.into(),
        cancellation_generation: generation,
    };
    resolve_checking_activation(HOME, &evaluation).err()
}

fn frame(key: RecordKey, segment: &str) -> Option<String> {
    let reference = NamespaceReference {
        key,
        segment: segment.into(),
        offset: 0,
        length: 4,
        frame_digest: Vec::new(),
        value_digest: D1.into(),
    };
    let mut reader = namespace_frame_reader(HOME);
    read_namespace_frame(&mut reader, &reference).err()
}

#[test]
fn completed_checker_keys_keep_their_spelling() -> Result<(), String> {
    let prefix = "incan.compiler.completed-checking\t";
    assert_eq!(
        completed_checker_key(&key("local", "owner-1", "main"))?,
        format!("{prefix}4f52e91e5fdd88ce05f98fb3c06034752490937364b494a463c88d2005c84921")
    );
    assert_eq!(
        completed_checker_key(&key("final", "pkg.mod::f", "f"))?,
        format!("{prefix}cac56e4035588ee290764e3fe13c6d080d929d054d4217b0d4a7398cf071af06")
    );
    assert_eq!(
        completed_checker_key(&key("local", "bad\tidentity", "main"))
            .err()
            .as_deref(),
        Some("compiler record key fields cannot contain store delimiters")
    );
    Ok(())
}

#[test]
fn owner_sources_keep_their_own_line_after_the_header() {
    let owner = "def f() -> None: pass";
    assert_eq!(projected_owner_source("", owner), owner);
    assert_eq!(
        projected_owner_source("import a\n", owner),
        format!("import a\n{owner}")
    );
    assert_eq!(projected_owner_source("import a", owner), format!("import a\n{owner}"));
}

#[test]
fn checking_preparation_refuses_before_reading_the_store() {
    assert_eq!(
        prepare(D1, "incan.other/1").err().as_deref(),
        Some("unsupported checking producer profile")
    );
    assert_eq!(
        prepare(D1, CHECKER).err().as_deref(),
        Some("missing-binding: /nonexistent-runtime-home/bakery.view")
    );
    assert_eq!(
        prepare("short", CHECKER).err().as_deref(),
        Some("missing-descriptor: short")
    );
}

#[test]
fn checking_evaluation_requires_a_reservation_and_a_bound_snapshot() {
    let refusal = "checking evaluation requires a reservation and cancellation generation";
    assert_eq!(evaluate("", 0).as_deref(), Some(refusal));
    assert_eq!(evaluate("r", -1).as_deref(), Some(refusal));
    assert_eq!(
        evaluate("r", 0).as_deref(),
        Some("missing-binding: /nonexistent-runtime-home/bakery.view")
    );
}

#[test]
fn namespace_frames_refuse_invalid_segments_and_missing_stores() {
    let page = || key("namespace", "m", "page");
    let invalid_name = format!("{}G", &D1[..63]);
    assert_eq!(
        frame(page(), "abc").as_deref(),
        Some("namespace frame segment has an invalid length")
    );
    assert_eq!(
        frame(page(), &invalid_name).as_deref(),
        Some("namespace frame segment has an invalid name")
    );
    assert_eq!(
        frame(key("namespace", "m\nx", "page"), "abc").as_deref(),
        Some("compiler record key fields cannot contain store delimiters")
    );
    let segment = format!("/nonexistent-runtime-home/segments/{D1}.seg");
    assert_eq!(
        frame(page(), D1),
        Some(format!(
            "store integrity error in {segment}: store io {segment}: No such file or directory (os error 2)"
        ))
    );
}

#[test]
fn namespace_frame_readers_describe_their_bakery_anchor() {
    let reader = namespace_frame_reader(HOME);
    assert_eq!(
        format!("{reader:?}"),
        "NamespaceFrameReader { directory: \"/nonexistent-runtime-home\", mapped_segments: 0 }"
    );
}
