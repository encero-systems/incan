//! Receipt-backed native and paired evidence for scalar `json_stringify`.

use super::legacy_oven::{self, LegacyOvenCapability};
use super::{
    FunctionResultKind, PreparedShadowProfile, ShadowComparisonProfile, SourceObservable, TypedFunctionResult,
    compare_source_observable_with_materialization,
};
use crate::backend::replacement::ReplacementValue;
use crate::backend::selection::{BackendKind, FallbackOutcome, FallbackPolicy};
use crate::provider::FeatureSelection;

const OPERAND_ONCE_SOURCE: &str = r#"def printed_json_operand() -> int:
    println("once-only JSON operand")
    return 7

def observe() -> str:
    return json_stringify(printed_json_operand())
"#;

const BORROWED_STRING_SOURCE: &str = r#"def observe() -> str:
    source = "kept"
    encoded = json_stringify(source)
    return encoded + "|" + source
"#;

const SCALAR_MATRIX_SOURCE: &str = include_str!("../../../tests/fixtures/replacement/json_stringify_scalars.incn");

const SCALAR_MATRIX_EXPECTED: &str =
    r#"7|-42|9223372036854775807|-9223372036854775807|true|false|"quote:\" slash:\\ line:\n tab:\t café 😀"|null"#;

/// Observe the receipt-backed native route and prove its visible operand effect occurs exactly once.
#[test]
fn native_json_stringify_evaluates_its_operand_once() -> Result<(), Box<dyn std::error::Error>> {
    let profile = ShadowComparisonProfile::new(OPERAND_ONCE_SOURCE, "observe", Vec::new());
    let workspace = tempfile::tempdir()?;
    match LegacyOvenCapability::from_environment() {
        Ok(_) => {}
        Err(unavailable)
            if std::env::var_os("INCAN_SHADOW_REQUIRE_LEGACY_ROUTE")
                .is_some_and(|value| !value.is_empty() && value != "0") =>
        {
            return Err(unavailable.into());
        }
        Err(unavailable) => {
            eprintln!("skipping native JSON observation: {}", unavailable.reason);
            return Ok(());
        }
    }
    let source_path = workspace.path().join("native-json-stringify-profile.incn");
    std::fs::write(&source_path, profile.source())?;
    let materialization = crate::cli::commands::shadow_support::prepare_shadow_legacy_materialization(
        &source_path,
        &FeatureSelection::default(),
        None,
    )?;
    let capability = match LegacyOvenCapability::from_environment_for_materialization(&materialization) {
        Ok(capability) => capability,
        Err(unavailable)
            if std::env::var_os("INCAN_SHADOW_REQUIRE_LEGACY_ROUTE")
                .is_some_and(|value| !value.is_empty() && value != "0") =>
        {
            return Err(unavailable.into());
        }
        Err(unavailable) => {
            eprintln!("skipping native JSON observation: {}", unavailable.reason);
            return Ok(());
        }
    };
    let prepared = PreparedShadowProfile::new(&profile)?;
    let route =
        legacy_oven::observe_legacy_route(&profile, &prepared, &materialization, &capability, workspace.path())?;

    assert!(route.authority.oven_receipt_identity.starts_with("sha256:"));
    assert!(route.authority.oven_build_unit_identity.starts_with("sha256:"));
    assert!(route.authority.direct_rustc_plan_identity.starts_with("sha256:"));
    assert!(!route.authority.cargo_process_started);
    assert_eq!(route.process.exit_code, Some(0));
    assert!(route.process.stderr.is_empty());
    assert_eq!(
        route.observation.ok_or("native result was not observed")?.observable,
        SourceObservable::Completed {
            result: TypedFunctionResult {
                kind: FunctionResultKind::Str,
                value: "7".to_string(),
            },
        }
    );
    assert_eq!(route.process.stdout, b"once-only JSON operand\n");
    Ok(())
}

/// Both routes must preserve the exact scalar JSON bytes, empty program streams, and independent receipts.
#[test]
fn scalar_json_stringify_matches_the_receipt_backed_native_route() -> Result<(), Box<dyn std::error::Error>> {
    let profile = ShadowComparisonProfile::new(SCALAR_MATRIX_SOURCE, "observe", Vec::new());
    let workspace = tempfile::tempdir()?;
    match LegacyOvenCapability::from_environment() {
        Ok(_) => {}
        Err(unavailable)
            if std::env::var_os("INCAN_SHADOW_REQUIRE_LEGACY_ROUTE")
                .is_some_and(|value| !value.is_empty() && value != "0") =>
        {
            return Err(unavailable.into());
        }
        Err(unavailable) => {
            eprintln!("skipping paired scalar JSON observation: {}", unavailable.reason);
            return Ok(());
        }
    }
    let source_path = workspace.path().join("scalar-json-stringify-profile.incn");
    std::fs::write(&source_path, profile.source())?;
    let materialization = crate::cli::commands::shadow_support::prepare_shadow_legacy_materialization(
        &source_path,
        &FeatureSelection::default(),
        None,
    )?;
    let capability = match LegacyOvenCapability::from_environment_for_materialization(&materialization) {
        Ok(capability) => capability,
        Err(unavailable)
            if std::env::var_os("INCAN_SHADOW_REQUIRE_LEGACY_ROUTE")
                .is_some_and(|value| !value.is_empty() && value != "0") =>
        {
            return Err(unavailable.into());
        }
        Err(unavailable) => {
            eprintln!("skipping paired scalar JSON observation: {}", unavailable.reason);
            return Ok(());
        }
    };
    let comparison =
        compare_source_observable_with_materialization(&profile, &materialization, &capability, workspace.path());

    assert!(comparison.matched(), "{:?}", comparison.state);
    let execution = comparison
        .replacement_execution
        .as_ref()
        .ok_or("missing direct scalar JSON execution")?;
    assert_eq!(
        execution.value,
        ReplacementValue::Str(SCALAR_MATRIX_EXPECTED.to_string())
    );
    assert!(execution.output.stdout().is_empty());
    assert!(execution.output.stderr().is_empty());
    let legacy = comparison
        .legacy
        .as_ref()
        .ok_or("missing native scalar JSON evidence")?;
    let replacement = comparison
        .replacement
        .as_ref()
        .ok_or("missing direct scalar JSON evidence")?;
    assert_eq!(legacy.observation.stdout, replacement.observation.stdout);
    assert_eq!(legacy.observation.stderr, replacement.observation.stderr);
    for (receipt, backend) in [
        (legacy.receipt()?, BackendKind::Legacy),
        (replacement.receipt()?, BackendKind::Replacement),
    ] {
        receipt.verify_identity()?;
        assert_eq!(receipt.executed_backend, backend);
        assert_eq!(receipt.selection.fallback_policy, FallbackPolicy::Refuse);
        assert_eq!(receipt.fallback_outcome, FallbackOutcome::NotNeeded);
        assert_eq!(receipt.selection.source_identity, comparison.source_identity);
        assert_eq!(receipt.shadow_comparison, comparison.state);
    }
    assert_ne!(legacy.receipt()?.identity, replacement.receipt()?.identity);
    assert!(
        !comparison
            .legacy_authority
            .as_ref()
            .ok_or("missing scalar JSON Oven authority")?
            .cargo_process_started
    );
    Ok(())
}

/// Exactly-once lowering must not turn the builtin's existing borrow into a move of a named string.
#[test]
fn json_stringify_preserves_a_named_string_for_later_use() -> Result<(), Box<dyn std::error::Error>> {
    let profile = ShadowComparisonProfile::new(BORROWED_STRING_SOURCE, "observe", Vec::new());
    let workspace = tempfile::tempdir()?;
    match LegacyOvenCapability::from_environment() {
        Ok(_) => {}
        Err(unavailable)
            if std::env::var_os("INCAN_SHADOW_REQUIRE_LEGACY_ROUTE")
                .is_some_and(|value| !value.is_empty() && value != "0") =>
        {
            return Err(unavailable.into());
        }
        Err(unavailable) => {
            eprintln!("skipping paired JSON ownership observation: {}", unavailable.reason);
            return Ok(());
        }
    }
    let source_path = workspace.path().join("json-stringify-ownership-profile.incn");
    std::fs::write(&source_path, profile.source())?;
    let materialization = crate::cli::commands::shadow_support::prepare_shadow_legacy_materialization(
        &source_path,
        &FeatureSelection::default(),
        None,
    )?;
    let capability = match LegacyOvenCapability::from_environment_for_materialization(&materialization) {
        Ok(capability) => capability,
        Err(unavailable)
            if std::env::var_os("INCAN_SHADOW_REQUIRE_LEGACY_ROUTE")
                .is_some_and(|value| !value.is_empty() && value != "0") =>
        {
            return Err(unavailable.into());
        }
        Err(unavailable) => {
            eprintln!("skipping paired JSON ownership observation: {}", unavailable.reason);
            return Ok(());
        }
    };
    let comparison =
        compare_source_observable_with_materialization(&profile, &materialization, &capability, workspace.path());

    assert!(comparison.matched(), "{:?}", comparison.state);
    assert_eq!(
        comparison
            .replacement_execution
            .as_ref()
            .ok_or("missing direct JSON ownership execution")?
            .value,
        ReplacementValue::Str("\"kept\"|kept".to_string())
    );
    Ok(())
}
