//! Same-source hashed membership proof through direct Body IR and the receipt-backed native route.
//!
//! Helper-unit agreement is not this test's authority: both routes execute the same source, compare its ordinary
//! output and separately transported boolean result, and retain independent no-fallback receipts.

use incan::backend::replacement::ReplacementValue;
use incan::backend::selection::{BackendKind, FallbackOutcome, FallbackPolicy};
use incan::backend::shadow::ShadowComparisonProfile;
use incan::cli::commands::compare_source_observable;

#[path = "support/shadow_capability.rs"]
mod shadow_capability;

const MEMBERSHIP_SOURCE: &str = include_str!("fixtures/replacement/hashed_membership.incn");

/// The same source must agree under independent native and direct execution, including program output.
#[test]
fn hashed_membership_matches_the_receipt_backed_native_route() -> Result<(), Box<dyn std::error::Error>> {
    incan::compiler_stack::run_on_compiler_stack(|| check_hashed_membership().map_err(|error| error.to_string()))
        .map_err(Into::into)
}

/// Compile and execute the wide membership predicate on the same compiler-sized stack as the CLI.
fn check_hashed_membership() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason()? {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let capability = shadow_capability::legacy_capability()?;
    let workspace = tempfile::tempdir()?;
    let profile = ShadowComparisonProfile::new(MEMBERSHIP_SOURCE, "membership", Vec::new());
    let comparison = compare_source_observable(&profile, &capability, workspace.path());
    assert!(comparison.matched(), "{:?}", comparison.state);

    let execution = comparison
        .replacement_execution
        .as_ref()
        .ok_or("missing direct execution")?;
    assert_eq!(execution.value, ReplacementValue::Bool(true));
    assert_eq!(execution.output.stdout(), b"hashed membership\n");
    assert!(execution.output.stderr().is_empty());
    for helper in [
        "set_contains",
        "set_not_contains",
        "dict_contains_key",
        "dict_not_contains_key",
    ] {
        assert!(
            execution.body_snapshot.contains(helper),
            "{helper}: {}",
            execution.body_snapshot
        );
    }

    let legacy = comparison.legacy.as_ref().ok_or("missing native execution evidence")?;
    let replacement = comparison
        .replacement
        .as_ref()
        .ok_or("missing direct execution evidence")?;
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
