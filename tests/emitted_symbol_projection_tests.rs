//! Artifact-level conformance for RFC 120's recoverable `incan-v1` projection.
//!
//! This fixture intentionally invokes the selected compiler through `rustup` and rejects every release other than
//! Rust 1.98.0. Falling back to the ambient toolchain would make a green test say nothing about DD-0002's contract.

#![deny(clippy::expect_used, clippy::unwrap_used)]

use std::error::Error;

#[path = "support/emitted_symbol_artifact.rs"]
mod emitted_symbol_artifact;

#[test]
fn pinned_v0_artifact_recovers_functions_generic_specializations_methods_and_statics() -> Result<(), Box<dyn Error>> {
    let evidence = emitted_symbol_artifact::verify_pinned_release_artifact()?;
    assert_eq!(evidence.recovered_identities.len(), 4);
    assert!(evidence.saw_generic_u64_specialization);
    assert!(evidence.saw_non_incan_host_symbol);
    for identity in [
        &evidence.fixture_input_identity,
        &evidence.artifact_content_identity,
        &evidence.recovered_observation_identity,
    ] {
        assert!(identity.starts_with("sha256:"));
    }
    eprintln!(
        "incan-v1 release artifact measurement: rust={}; platform={}; baseline_bytes={}; projected_bytes={}; \
         delta_bytes={}; baseline_identifier_bytes={}; projected_identifier_bytes={}",
        emitted_symbol_artifact::SELECTED_RUST,
        std::env::consts::OS,
        evidence.baseline_bytes,
        evidence.projected_bytes,
        i128::from(evidence.projected_bytes) - i128::from(evidence.baseline_bytes),
        evidence.baseline_identifier_bytes,
        evidence.projected_identifier_bytes,
    );
    Ok(())
}
