//! Receipt-backed paired comparison for the selected canonical enumerate/Zip profile.

use incan::backend::replacement::ReplacementValue;
use incan::backend::selection::{BackendKind, FallbackOutcome, FallbackPolicy, ShadowComparisonState};
use incan::backend::shadow::{
    FunctionResultKind, RouteEvidence, ShadowComparison, ShadowComparisonProfile, SourceObservable, TypedFunctionResult,
};
use incan::cli::commands::compare_source_observable;

#[path = "support/shadow_capability.rs"]
mod shadow_capability;

const ENUMERATE_ZIP_SOURCE: &str = r#"
def enumerate_zip_profile() -> int:
  values = [10, 20]
  labels = ["ten"]
  mut total = 0
  for pair in enumerate(values):
    println(pair.0)
    println(pair.1)
    total += pair.0 + pair.1
  for zipped_pair in zip(values, labels):
    println(zipped_pair.0)
    println(zipped_pair.1)
    total += zipped_pair.0
  return total
"#;

/// Return the completed typed observable expected from each independently executed route.
fn completed(kind: FunctionResultKind, value: &str) -> SourceObservable {
    SourceObservable::Completed {
        result: TypedFunctionResult {
            kind,
            value: value.to_string(),
        },
    }
}

/// Require both routes to have executed and retained their own receipt evidence.
fn route_evidence(comparison: &ShadowComparison) -> Result<(&RouteEvidence, &RouteEvidence), String> {
    match (&comparison.legacy, &comparison.replacement) {
        (Some(legacy), Some(replacement)) => Ok((legacy, replacement)),
        _ => Err(format!(
            "enumerate/Zip comparison must execute both routes, got {:?}",
            comparison.state
        )),
    }
}

/// Global list enumerate/Zip calls compare exact streams and a typed scalar result through independent receipts.
#[test]
fn selected_enumerate_and_zip_match_through_the_staged_legacy_route() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason()? {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let capability = shadow_capability::legacy_capability()?;
    let workspace = tempfile::tempdir()?;
    let profile = ShadowComparisonProfile::new(ENUMERATE_ZIP_SOURCE, "enumerate_zip_profile", vec![]);
    let comparison = compare_source_observable(&profile, &capability, workspace.path());

    assert!(comparison.matched(), "{:?}", comparison.state);
    let (legacy, replacement) = route_evidence(&comparison)?;
    let expected = completed(FunctionResultKind::Int, "41");
    assert_eq!(legacy.observation.observable, expected);
    assert_eq!(replacement.observation.observable, expected);
    assert_eq!(legacy.observation.stdout, b"0\n10\n1\n20\n10\nten\n");
    assert_eq!(replacement.observation.stdout, legacy.observation.stdout);
    assert!(legacy.observation.stderr.is_empty());
    assert_eq!(replacement.observation.stderr, legacy.observation.stderr);

    let legacy_receipt = legacy.receipt()?;
    let replacement_receipt = replacement.receipt()?;
    legacy_receipt.verify_identity()?;
    replacement_receipt.verify_identity()?;
    assert_eq!(legacy_receipt.selection.source_identity, comparison.source_identity);
    assert_eq!(
        replacement_receipt.selection.source_identity,
        comparison.source_identity
    );
    assert_eq!(legacy_receipt.shadow_comparison, comparison.state);
    assert_eq!(replacement_receipt.shadow_comparison, comparison.state);
    assert_eq!(legacy_receipt.selection.selected_backend, BackendKind::Legacy);
    assert_eq!(legacy_receipt.executed_backend, BackendKind::Legacy);
    assert_eq!(replacement_receipt.selection.selected_backend, BackendKind::Replacement);
    assert_eq!(replacement_receipt.executed_backend, BackendKind::Replacement);
    assert_eq!(legacy_receipt.fallback_outcome, FallbackOutcome::NotNeeded);
    assert_eq!(replacement_receipt.fallback_outcome, FallbackOutcome::NotNeeded);
    assert_eq!(legacy_receipt.selection.fallback_policy, FallbackPolicy::Refuse);
    assert_eq!(replacement_receipt.selection.fallback_policy, FallbackPolicy::Refuse);
    assert_ne!(legacy_receipt.identity, replacement_receipt.identity);
    assert_ne!(legacy_receipt.output_identity, replacement_receipt.output_identity);

    let authority = comparison
        .legacy_authority
        .as_ref()
        .ok_or("a matched legacy route must retain Oven execution authority")?;
    assert!(authority.oven_receipt_identity.starts_with("sha256:"));
    assert!(authority.oven_build_unit_identity.starts_with("sha256:"));
    assert!(authority.direct_rustc_plan_identity.starts_with("sha256:"));
    assert!(!authority.cargo_process_started);

    let process = comparison
        .legacy_process
        .as_ref()
        .ok_or("a matched legacy route must retain raw process evidence")?;
    assert_eq!(process.stdout, b"0\n10\n1\n20\n10\nten\n");
    assert!(process.stderr.is_empty());
    assert!(
        process
            .result_report
            .as_deref()
            .is_some_and(|report| report.starts_with(b"incan-shadow-result-v2:int:41")),
        "legacy result transport must remain separate from program stdout"
    );

    let execution = comparison
        .replacement_execution
        .as_ref()
        .ok_or("a matched replacement route must retain direct execution evidence")?;
    assert_eq!(execution.value, ReplacementValue::Int(41));
    assert_eq!(execution.output.stdout(), b"0\n10\n1\n20\n10\nten\n");
    assert!(execution.output.stderr().is_empty());
    assert_eq!(execution.output_identity, replacement.observation.output_identity);
    assert!(matches!(comparison.state, ShadowComparisonState::Matched { .. }));
    Ok(())
}
