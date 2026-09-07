//! Same-source proof for bounded checked `isinstance` targets through direct Body IR and native Oven execution.

use incan::backend::replacement::ReplacementValue;
use incan::backend::selection::{BackendKind, FallbackOutcome, FallbackPolicy};
use incan::backend::shadow::ShadowComparisonProfile;
use incan::cli::commands::compare_source_observable;

#[path = "support/shadow_capability.rs"]
mod shadow_capability;

const ISINSTANCE_SOURCE: &str = include_str!("fixtures/replacement/isinstance_targets.incn");

#[test]
fn checked_isinstance_targets_match_the_receipt_backed_native_route() -> Result<(), Box<dyn std::error::Error>> {
    incan::compiler_stack::run_on_compiler_stack(|| check_isinstance_targets().map_err(|error| error.to_string()))
        .map_err(Into::into)
}

/// Require the bounded primitive targets, true/false union branches, typed result, and exact streams to agree.
fn check_isinstance_targets() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let profile = ShadowComparisonProfile::new(ISINSTANCE_SOURCE, "isinstance_targets", Vec::new());
    let capability = match shadow_capability::legacy_capability() {
        Ok(capability) => capability,
        Err(unavailable) if shadow_capability::legacy_route_is_required() => return Err(unavailable.into()),
        Err(unavailable) => {
            eprintln!("skipping: {}", unavailable.reason);
            return Ok(());
        }
    };
    let comparison = compare_source_observable(&profile, &capability, workspace.path());
    assert!(comparison.matched(), "{:?}", comparison.state);

    let execution = comparison
        .replacement_execution
        .as_ref()
        .ok_or("missing direct isinstance execution")?;
    assert_eq!(execution.value, ReplacementValue::Bool(true));
    assert_eq!(execution.output.stdout(), b"isinstance targets\n");
    assert!(execution.output.stderr().is_empty());

    let legacy = comparison.legacy.as_ref().ok_or("missing native isinstance evidence")?;
    let replacement = comparison
        .replacement
        .as_ref()
        .ok_or("missing direct isinstance evidence")?;
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
    assert!(
        !comparison
            .legacy_authority
            .as_ref()
            .ok_or("missing isinstance Oven authority")?
            .cargo_process_started
    );
    Ok(())
}
