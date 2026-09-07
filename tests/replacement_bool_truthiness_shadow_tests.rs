//! Same-source proof for bounded canonical `bool` through direct Body IR and receipt-backed native execution.

use incan::backend::replacement::ReplacementValue;
use incan::backend::selection::{BackendKind, FallbackOutcome, FallbackPolicy};
use incan::backend::shadow::ShadowComparisonProfile;
use incan::cli::commands::compare_source_observable;

#[path = "support/shadow_capability.rs"]
mod shadow_capability;

const BOOL_TRUTHINESS_SOURCE: &str = include_str!("fixtures/replacement/bool_truthiness.incn");

/// Both routes return the same truthiness result and preserve exact ordinary streams.
#[test]
fn bool_truthiness_matches_the_receipt_backed_native_route() -> Result<(), Box<dyn std::error::Error>> {
    incan::compiler_stack::run_on_compiler_stack(|| check_bool_truthiness().map_err(|error| error.to_string()))
        .map_err(Into::into)
}

/// Compile and execute on the same compiler-sized stack used by the CLI.
fn check_bool_truthiness() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let profile = ShadowComparisonProfile::new(BOOL_TRUTHINESS_SOURCE, "bool_truthiness", Vec::new());
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
        .ok_or("missing direct bool-truthiness execution")?;
    assert_eq!(execution.value, ReplacementValue::Bool(true));
    assert_eq!(execution.output.stdout(), b"bool truthiness\n");
    assert!(execution.output.stderr().is_empty());

    let legacy = comparison
        .legacy
        .as_ref()
        .ok_or("missing native bool-truthiness evidence")?;
    let replacement = comparison
        .replacement
        .as_ref()
        .ok_or("missing direct bool-truthiness evidence")?;
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
            .ok_or("missing bool-truthiness Oven authority")?
            .cargo_process_started
    );
    Ok(())
}
