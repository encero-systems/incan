//! Checks for execution-profile authority before provider discovery, native materialization, or either shadow route.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::{
    BuiltinAbsSumOverflowBehavior, PreparedShadowProfile, ShadowComparisonProfile, ShadowComparisonState,
    ShadowLegacyMaterialization, abs_sum_options_for_oven_profile, classify_observations, compare_source_observable,
    legacy_oven::LegacyOvenCapability, observe_replacement_route,
};
use crate::oven::{OvenGeneratedProjectRequest, receipt_generated_project, write_receipt};
use crate::provider::ProviderPlan;

/// Known native profiles map deliberately; an arbitrary profile is never a debug alias.
#[test]
fn abs_sum_profile_mapping_is_explicit() -> Result<(), Box<dyn std::error::Error>> {
    for profile in ["debug", crate::oven::OVEN_COMPILER_TEST_PROFILE] {
        assert_eq!(
            abs_sum_options_for_oven_profile(profile)?.builtin_abs_sum_overflow,
            BuiltinAbsSumOverflowBehavior::Checked
        );
    }
    assert_eq!(
        abs_sum_options_for_oven_profile("release")?.builtin_abs_sum_overflow,
        BuiltinAbsSumOverflowBehavior::ReleaseWrapping
    );
    let error = abs_sum_options_for_oven_profile("custom-fast")
        .err()
        .ok_or("unknown profile must refuse")?;
    assert!(error.reason.contains("custom-fast"));
    Ok(())
}

/// Raw profile identity remains significant even when two profiles choose the same builtin behavior.
#[test]
fn abs_sum_comparison_identity_binds_raw_profile_and_resolved_behavior() -> Result<(), Box<dyn std::error::Error>> {
    let profile = ShadowComparisonProfile::new("def observed() -> int:\n    return 42\n", "observed", vec![]);
    let debug = PreparedShadowProfile::new(&profile, "debug")?;
    let suite = PreparedShadowProfile::new(&profile, crate::oven::OVEN_COMPILER_TEST_PROFILE)?;
    let release = PreparedShadowProfile::new(&profile, "release")?;
    assert_ne!(debug.profile_identity, suite.profile_identity);
    assert_ne!(debug.profile_identity, release.profile_identity);
    assert_ne!(suite.profile_identity, release.profile_identity);
    assert_ne!(profile.profile_identity(), debug.profile_identity);

    let debug_result = observe_replacement_route(&profile, &debug)?;
    let release_result = observe_replacement_route(&profile, &release)?;
    let debug_observation = debug_result.observation.ok_or("debug route must execute")?;
    let release_observation = release_result.observation.ok_or("release route must execute")?;
    assert_ne!(debug_observation.output_identity, release_observation.output_identity);
    assert!(matches!(
        classify_observations(&debug_observation, &release_observation),
        ShadowComparisonState::Unavailable { .. }
    ));
    Ok(())
}

/// A verified but unsupported intent refuses before provider/source-session discovery, native-input compatibility,
/// materialization, direct execution, or native workspace creation.
#[test]
fn unknown_abs_sum_profile_refuses_before_either_route() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempfile::tempdir()?;
    let source = "def observed() -> int:\n    println(\"must not execute\")\n    return 42\n";
    let source_path = fixture.path().join("case.incn");
    std::fs::write(&source_path, source)?;
    // This is only a synthetic identity-valid receipt for the refusal test: no native plan, compiler, or execution is
    // claimed.
    let request = OvenGeneratedProjectRequest::new(
        fixture.path(),
        "unknown-profile-fixture",
        "0.0.0",
        "test-target",
        "test-toolchain",
        "future-profile",
        vec![],
    )
    .with_generated_source("test-source", &source_path);
    let receipt = receipt_generated_project(&request)?;
    let receipt_path = fixture.path().join("receipt.json");
    write_receipt(&receipt, &receipt_path)?;
    let capability = LegacyOvenCapability::adopt_baked_project(
        fixture.path().join("absent-store"),
        fixture.path().join("absent-rustc"),
        &receipt_path,
    )?;
    let workspace = fixture.path().join("must-not-be-created");
    let profile = ShadowComparisonProfile::new(source, "observed", vec![]);
    // The unsupported profile must refuse before native-input compatibility or materialization. This identity-bound
    // synthetic context therefore proves ordering only; it is not a staged SDK/provider authority.
    let materialization = ShadowLegacyMaterialization::from_provider_plan(
        Arc::new(ProviderPlan::default()),
        BTreeMap::new(),
        profile.source_identity(),
    );
    let comparison = compare_source_observable(&profile, &materialization, &capability, &workspace);
    assert!(
        comparison
            .unavailable_reason()
            .ok_or("unknown profile must be unavailable")?
            .contains("future-profile")
    );
    assert!(comparison.legacy.is_none());
    assert!(comparison.replacement.is_none());
    assert!(comparison.legacy_process.is_none());
    assert!(comparison.legacy_authority.is_none());
    assert!(comparison.replacement_execution.is_none());
    assert!(comparison.replacement_output.is_none());
    assert!(!workspace.exists());

    let invalid_source = ShadowComparisonProfile::new("not valid source", "observed", vec![]);
    let error = PreparedShadowProfile::new(&invalid_source, "future-profile")
        .err()
        .ok_or("profile must refuse first")?;
    assert!(error.reason.contains("future-profile"));
    Ok(())
}
