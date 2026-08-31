//! Same-source proof for selected string helpers through direct Body IR and the receipt-backed native route.

use incan::backend::replacement::ReplacementValue;
use incan::backend::selection::{BackendKind, FallbackOutcome, FallbackPolicy};
use incan::backend::shadow::{ShadowComparisonProfile, compare_source_observable};

#[path = "support/shadow_capability.rs"]
mod shadow_capability;

/// The minimal omitted-separator source must compile and agree on the native route, not only in direct execution.
#[test]
fn default_split_separator_compiles_and_matches_native() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason() {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let capability = shadow_capability::legacy_capability()?;
    let workspace = tempfile::tempdir()?;
    let source = include_str!("codegen_snapshots/string_split_default.incn");
    let profile = ShadowComparisonProfile::new(
        source,
        "split_default",
        vec![ReplacementValue::Str(" a b ".to_string())],
    );
    let comparison = compare_source_observable(&profile, &capability, workspace.path());
    assert!(comparison.matched(), "{:?}", comparison.state);
    let execution = comparison
        .replacement_execution
        .as_ref()
        .ok_or("missing direct execution")?;
    assert_eq!(execution.value, ReplacementValue::Str(" a b ".to_string()));
    assert!(execution.output.stdout().is_empty());
    assert!(execution.output.stderr().is_empty());
    Ok(())
}

/// Both routes must agree on shared string semantics, exact program streams and a separately transported result.
#[test]
fn selected_string_helpers_match_the_receipt_backed_native_route() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason() {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let capability = shadow_capability::legacy_capability()?;
    let workspace = tempfile::tempdir()?;
    let source = include_str!("fixtures/replacement/string_helpers.incn");
    let profile = ShadowComparisonProfile::new(source, "string_helpers", Vec::new());
    let comparison = compare_source_observable(&profile, &capability, workspace.path());
    assert!(comparison.matched(), "{:?}", comparison.state);

    let execution = comparison
        .replacement_execution
        .as_ref()
        .ok_or("missing direct execution")?;
    assert_eq!(execution.value, ReplacementValue::Bool(true));
    assert_eq!(execution.output.stdout(), b"string helper checks\n");
    assert!(execution.output.stderr().is_empty());
    for helper in [
        "str_upper",
        "str_lower",
        "str_strip",
        "str_replace",
        "str_join",
        "str_split",
        "str_contains",
    ] {
        assert!(
            execution.body_snapshot.contains(&format!("call helper:{helper}")),
            "{helper}: {}",
            execution.body_snapshot
        );
    }

    let legacy = comparison.legacy.as_ref().ok_or("missing native evidence")?;
    let replacement = comparison.replacement.as_ref().ok_or("missing replacement evidence")?;
    assert_eq!(legacy.observation.stdout, replacement.observation.stdout);
    assert_eq!(legacy.observation.stderr, replacement.observation.stderr);
    let legacy_receipt = legacy.receipt()?;
    let replacement_receipt = replacement.receipt()?;
    for (receipt, backend) in [
        (legacy_receipt, BackendKind::Legacy),
        (replacement_receipt, BackendKind::Replacement),
    ] {
        receipt.verify_identity()?;
        assert_eq!(receipt.executed_backend, backend);
        assert_eq!(receipt.selection.fallback_policy, FallbackPolicy::Refuse);
        assert_eq!(receipt.fallback_outcome, FallbackOutcome::NotNeeded);
        assert_eq!(receipt.selection.source_identity, comparison.source_identity);
        assert_eq!(receipt.shadow_comparison, comparison.state);
    }
    assert_ne!(legacy_receipt.identity, replacement_receipt.identity);
    assert_ne!(legacy_receipt.output_identity, replacement_receipt.output_identity);
    Ok(())
}
