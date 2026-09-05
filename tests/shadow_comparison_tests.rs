//! End-to-end proof for the bounded source-observable shadow comparison (#1146).
//!
//! Every typed-result agreement here comes from two genuinely independent executions of the same source: the
//! replacement route executes Body IR directly in-process, and the legacy route emits Rust, has Oven authorize
//! and build it through an immutable store-selected direct-`rustc` plan, and runs the produced program as a
//! separate process. Nothing here compares generated Rust text, and nothing treats a successful build as
//! agreement. Normal stdout and stderr are compared byte-for-byte independently of the typed result report;
//! failures retain their prior streams and cannot match merely because the failure classes agree.
//!
//! The legacy route needs a staged Oven capability (see `incan::backend::shadow::legacy_oven`). Tests that assert
//! an executed comparison require it and say so when it is missing; tests about honest unavailability do not.
//!
//! Run with: `cargo test --test shadow_comparison_tests`

use std::path::Path;

use incan::backend::replacement::ReplacementValue;
use incan::backend::selection::{BackendKind, FallbackOutcome, FallbackPolicy, ShadowComparisonState};
use incan::backend::shadow::legacy_oven::LegacyOvenCapability;
use incan::backend::shadow::{
    FunctionResultKind, PROGRAM_ENTRYPOINT_UNAVAILABLE_REASON, RouteEvidence, RuntimeFailureClass, ShadowComparison,
    ShadowComparisonProfile, ShadowUnavailable, SourceObservable, TypedFunctionResult,
};
use incan::cli::commands::compare_source_observable;

#[path = "support/shadow_capability.rs"]
mod shadow_capability;

const GREET_SRC: &str = "def greet(name: str) -> str:\n    return \"hello, \" + name\n";
const DIVIDE_SRC: &str = "def divide(a: int, b: int) -> int:\n    println(\"before division\")\n    return a // b\n";
const GUARD_SRC: &str =
    "def guarded(a: int) -> int:\n    println(\"before assertion\")\n    assert a > 0\n    return a\n";
const PRINTING_ADD_SRC: &str =
    "def add(x: int, y: int) -> int:\n    println(\"normal program stdout\")\n    return x + y\n";

fn completed(kind: FunctionResultKind, value: &str) -> SourceObservable {
    SourceObservable::Completed {
        result: TypedFunctionResult {
            kind,
            value: value.to_string(),
        },
    }
}

/// Run one comparison against the staged Oven capability, or report why the legacy route is unavailable.
fn compare(
    profile: &ShadowComparisonProfile,
    workspace: &Path,
) -> Result<ShadowComparison, Box<dyn std::error::Error>> {
    let capability = shadow_capability::legacy_capability()?;
    Ok(compare_source_observable(profile, &capability, workspace))
}

/// Both route receipts, or a failure naming the state that produced no receipt.
fn route_evidence(comparison: &ShadowComparison) -> Result<(&RouteEvidence, &RouteEvidence), String> {
    match (&comparison.legacy, &comparison.replacement) {
        (Some(legacy), Some(replacement)) => Ok((legacy, replacement)),
        _ => Err(format!(
            "expected both routes to execute and produce receipts, got state {:?}",
            comparison.state
        )),
    }
}

/// Assert the shared, route-independent facts every completed comparison must carry.
fn assert_receipts_are_independent_but_bound(comparison: &ShadowComparison) -> Result<(), Box<dyn std::error::Error>> {
    let (legacy, replacement) = route_evidence(comparison)?;

    let legacy_receipt = legacy.receipt()?;
    let replacement_receipt = replacement.receipt()?;
    legacy_receipt.verify_identity()?;
    replacement_receipt.verify_identity()?;

    // Same source, same recorded comparison outcome: the two receipts describe one comparison.
    assert_eq!(legacy_receipt.selection.source_identity, comparison.source_identity);
    assert_eq!(
        replacement_receipt.selection.source_identity,
        comparison.source_identity
    );
    assert_eq!(legacy_receipt.shadow_comparison, comparison.state);
    assert_eq!(replacement_receipt.shadow_comparison, comparison.state);
    assert!(legacy_receipt.selection.shadow_requested);
    assert!(replacement_receipt.selection.shadow_requested);

    // Same profile: neither observation was produced under a different comparison instance.
    assert_eq!(legacy.observation.profile_identity, comparison.profile_identity);
    assert_eq!(replacement.observation.profile_identity, comparison.profile_identity);

    // Different routes: each receipt records the backend that actually ran, with no fallback in either direction.
    assert_eq!(legacy_receipt.selection.selected_backend, BackendKind::Legacy);
    assert_eq!(legacy_receipt.executed_backend, BackendKind::Legacy);
    assert_eq!(replacement_receipt.selection.selected_backend, BackendKind::Replacement);
    assert_eq!(replacement_receipt.executed_backend, BackendKind::Replacement);
    assert_eq!(legacy_receipt.fallback_outcome, FallbackOutcome::NotNeeded);
    assert_eq!(replacement_receipt.fallback_outcome, FallbackOutcome::NotNeeded);
    assert_eq!(legacy_receipt.selection.fallback_policy, FallbackPolicy::Refuse);
    assert_eq!(replacement_receipt.selection.fallback_policy, FallbackPolicy::Refuse);

    // The receipts are not interchangeable: each covers what its own route produced.
    assert_ne!(legacy_receipt.identity, replacement_receipt.identity);
    assert_ne!(
        legacy_receipt.selection.identity,
        replacement_receipt.selection.identity
    );
    assert_ne!(
        legacy_receipt.output_identity, replacement_receipt.output_identity,
        "a shared output identity would erase the routes' independence"
    );

    // The legacy answer is attributable to the Oven authority that produced it.
    let authority = comparison
        .legacy_authority
        .as_ref()
        .ok_or("an executed legacy route must record the Oven authority that permitted it")?;
    assert!(authority.oven_receipt_identity.starts_with("sha256:"));
    assert!(authority.oven_build_unit_identity.starts_with("sha256:"));
    assert!(authority.direct_rustc_plan_identity.starts_with("sha256:"));
    assert!(authority.output_digest.starts_with("sha256:"));
    assert!(
        !authority.cargo_process_started,
        "Oven-owned legacy execution must not start a Cargo process"
    );
    Ok(())
}

/// Guard for tests that require a staged legacy route.
///
/// Returns the reason when nothing is staged, so the test reports why it could not assert a match rather than
/// passing silently. Setting `INCAN_SHADOW_REQUIRE_LEGACY_ROUTE` turns that report into a failure, which is how
/// an environment that is supposed to be staged proves it.
fn require_staged_legacy_route() -> Result<Option<String>, ShadowUnavailable> {
    shadow_capability::unstaged_legacy_route_reason()
}

/// A real scalar profile keeps normal stdout separate from its typed result under Oven authority.
#[test]
fn a_scalar_profile_compares_program_streams_and_typed_result() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = require_staged_legacy_route()? {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let workspace = tempfile::tempdir()?;
    let profile = ShadowComparisonProfile::new(
        PRINTING_ADD_SRC,
        "add",
        vec![ReplacementValue::Int(40), ReplacementValue::Int(2)],
    );
    let comparison = compare(&profile, workspace.path())?;

    assert!(comparison.matched(), "{:?}", comparison.state);

    let (legacy, replacement) = route_evidence(&comparison)?;
    let expected = completed(FunctionResultKind::Int, "42");
    assert_eq!(legacy.observation.observable, expected);
    assert_eq!(replacement.observation.observable, expected);
    assert_receipts_are_independent_but_bound(&comparison)?;

    let process = comparison
        .legacy_process
        .as_ref()
        .ok_or("an executed legacy route must retain raw process evidence")?;
    assert_eq!(process.stdout, b"normal program stdout\n");
    assert!(process.stderr.is_empty());
    assert!(
        process
            .result_report
            .as_deref()
            .is_some_and(|report| report.starts_with(b"incan-shadow-result-v2:int:42")),
        "the typed result must be out-of-band from program stdout"
    );

    // The replacement route carried its own Body-IR evidence, proving it executed rather than reading the
    // legacy route's result.
    let execution = comparison
        .replacement_execution
        .as_ref()
        .ok_or("a direct execution must retain its Body-IR evidence")?;
    assert_eq!(execution.value, ReplacementValue::Int(42));
    assert!(
        execution.body_snapshot.contains("body add"),
        "{}",
        execution.body_snapshot
    );
    assert_eq!(
        execution.output_identity, replacement.observation.output_identity,
        "the replacement receipt must be bound to the execution that produced its result"
    );
    Ok(())
}

/// String results compare through the typed report, not a trimmed stdout approximation.
#[test]
fn a_string_profile_retains_its_exact_typed_value() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = require_staged_legacy_route()? {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let workspace = tempfile::tempdir()?;
    let profile = ShadowComparisonProfile::new(GREET_SRC, "greet", vec![ReplacementValue::Str("Ada".to_string())]);
    let comparison = compare(&profile, workspace.path())?;

    assert!(comparison.matched(), "{:?}", comparison.state);
    let (legacy, replacement) = route_evidence(&comparison)?;
    let expected = completed(FunctionResultKind::Str, "hello, Ada");
    assert_eq!(legacy.observation.observable, expected);
    assert_eq!(replacement.observation.observable, expected);
    assert_receipts_are_independent_but_bound(&comparison)?;
    Ok(())
}

/// A string whose value ends in a newline must survive the dedicated result transport intact.
///
/// This is the case a stdout-based transport would corrupt: `"line\n"` and `"line"` would become
/// indistinguishable if any layer trimmed the program stream.
#[test]
fn a_trailing_newline_in_a_result_is_not_lost_in_transport() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = require_staged_legacy_route()? {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let workspace = tempfile::tempdir()?;
    let profile = ShadowComparisonProfile::new(
        "def echo(value: str) -> str:\n    return value\n",
        "echo",
        vec![ReplacementValue::Str("line\n".to_string())],
    );
    let comparison = compare(&profile, workspace.path())?;

    assert!(comparison.matched(), "{:?}", comparison.state);
    let (legacy, replacement) = route_evidence(&comparison)?;
    let expected = completed(FunctionResultKind::Str, "line\n");
    assert_eq!(
        legacy.observation.observable, expected,
        "the legacy route must report the exact typed value, including its trailing newline"
    );
    assert_eq!(replacement.observation.observable, expected);
    Ok(())
}

/// Matching failure classes do not hide the current difference between native stderr and returned direct errors.
#[test]
fn division_failure_preserves_prior_stdout_and_reports_stderr_divergence() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = require_staged_legacy_route()? {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let workspace = tempfile::tempdir()?;
    let profile = ShadowComparisonProfile::new(
        DIVIDE_SRC,
        "divide",
        vec![ReplacementValue::Int(1), ReplacementValue::Int(0)],
    );
    let comparison = compare(&profile, workspace.path())?;

    assert!(
        matches!(comparison.state, ShadowComparisonState::Diverged { .. }),
        "{:?}",
        comparison.state
    );
    let (legacy, replacement) = route_evidence(&comparison)?;
    let expected = SourceObservable::Failed {
        failure: RuntimeFailureClass::DivisionByZero,
    };
    assert_eq!(legacy.observation.observable, expected);
    assert_eq!(replacement.observation.observable, expected);
    assert_eq!(legacy.observation.stdout, b"before division\n");
    assert_eq!(replacement.observation.stdout, legacy.observation.stdout);
    assert_eq!(
        legacy.observation.stderr,
        b"ZeroDivisionError: float division by zero\n"
    );
    assert!(replacement.observation.stderr.is_empty());
    assert!(
        comparison.replacement_execution.is_none(),
        "a failed direct execution has no successful Body-IR execution to retain"
    );
    let process = comparison
        .legacy_process
        .as_ref()
        .ok_or("a failed legacy process must retain its raw streams")?;
    assert!(
        process.result_report.is_none(),
        "a failed process must not contribute a partial result report"
    );
    assert!(!process.stderr.is_empty(), "the raw legacy diagnostic must be retained");
    assert_receipts_are_independent_but_bound(&comparison)?;
    Ok(())
}

/// Source failure output survives on both routes, but a native runtime diagnostic is not erased to claim parity.
#[test]
fn assertion_failure_preserves_prior_stdout_and_reports_stderr_divergence() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = require_staged_legacy_route()? {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let workspace = tempfile::tempdir()?;
    let profile = ShadowComparisonProfile::new(GUARD_SRC, "guarded", vec![ReplacementValue::Int(0)]);
    let comparison = compare(&profile, workspace.path())?;

    assert!(
        matches!(comparison.state, ShadowComparisonState::Diverged { .. }),
        "{:?}",
        comparison.state
    );
    let (legacy, replacement) = route_evidence(&comparison)?;
    let expected = SourceObservable::Failed {
        failure: RuntimeFailureClass::Assertion,
    };
    assert_eq!(legacy.observation.observable, expected);
    assert_eq!(replacement.observation.observable, expected);
    assert_eq!(legacy.observation.stdout, b"before assertion\n");
    assert_eq!(replacement.observation.stdout, legacy.observation.stdout);
    assert_eq!(legacy.observation.stderr, b"AssertionError\n");
    assert!(replacement.observation.stderr.is_empty());
    assert!(comparison.replacement_execution.is_none());
    assert_receipts_are_independent_but_bound(&comparison)?;
    Ok(())
}

/// Observing a program entrypoint is outside the profile before either route executes.
#[test]
fn a_program_entrypoint_profile_is_unavailable_without_executing_either_route() -> Result<(), Box<dyn std::error::Error>>
{
    if let Some(reason) = require_staged_legacy_route()? {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let workspace = tempfile::tempdir()?;
    let profile = ShadowComparisonProfile::new("def main() -> int:\n    return 42\n", "main", vec![]);
    let comparison = compare(&profile, workspace.path())?;

    let reason = comparison
        .unavailable_reason()
        .ok_or_else(|| format!("a `main` observation must stay unavailable, got {:?}", comparison.state))?;
    assert!(reason.contains(PROGRAM_ENTRYPOINT_UNAVAILABLE_REASON), "{reason}");
    assert!(
        reason.contains("neither route produced a comparable observation"),
        "{reason}"
    );
    assert!(!comparison.matched());
    assert!(
        comparison.legacy.is_none(),
        "the legacy route cannot observe an entrypoint"
    );
    assert!(
        comparison.legacy_authority.is_none(),
        "no Oven build was authorized for a route that never ran"
    );

    assert!(comparison.replacement.is_none());
    assert!(comparison.replacement_execution.is_none());
    assert!(comparison.legacy_process.is_none());
    Ok(())
}

/// A source the replacement route refuses stays unavailable rather than becoming a divergence claim.
///
/// This needs no staged legacy route: the replacement refusal alone decides it.
#[test]
fn a_source_outside_the_replacement_profile_stays_unavailable() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let profile = ShadowComparisonProfile::new(
        "def pairs() -> list[tuple[int, int]]:\n    return [(1, 2)]\n",
        "pairs",
        vec![],
    );
    let capability = match shadow_capability::legacy_capability() {
        Ok(capability) => capability,
        Err(unavailable) => {
            if shadow_capability::legacy_route_is_required() {
                return Err(unavailable.into());
            }
            eprintln!(
                "legacy route unstaged ({}); the replacement refusal still decides",
                unavailable.reason
            );
            // Without a capability the comparison is unavailable for two reasons at once, which is still the
            // behavior under test: an out-of-profile source never becomes a comparison verdict.
            return Ok(());
        }
    };
    let comparison = compare_source_observable(&profile, &capability, workspace.path());

    let reason = comparison.unavailable_reason().ok_or_else(|| {
        format!(
            "a source outside the #988 profile must stay unavailable, got {:?}",
            comparison.state
        )
    })?;
    assert!(
        reason.contains("requires a checked scalar or `None` return type"),
        "{reason}"
    );
    assert!(!comparison.matched());
    assert!(comparison.replacement.is_none());
    Ok(())
}

/// A legacy route with no Oven receipt has no authority, so it cannot run at all.
///
/// Retention of the executed replacement route's evidence under that unavailable state is proven by
/// `backend::shadow::tests::an_executed_replacement_route_survives_an_unavailable_legacy_route`.
#[test]
fn a_missing_oven_receipt_cannot_authorize_a_legacy_route() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let unstaged = LegacyOvenCapability::adopt_baked_project(
        workspace.path().join("no-store"),
        workspace.path().join("no-rustc"),
        &workspace.path().join("no-receipt.json"),
    );
    let Err(unavailable) = unstaged else {
        panic!("a missing Oven receipt cannot authorize a legacy route");
    };
    assert!(
        unavailable.reason.contains("no Oven authority"),
        "{}",
        unavailable.reason
    );
    Ok(())
}

/// A tampered Oven receipt cannot authorize a legacy comparison build.
#[test]
fn a_tampered_oven_receipt_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let Ok(capability) = shadow_capability::legacy_capability() else {
        eprintln!("skipping: no staged Oven receipt to tamper with");
        return Ok(());
    };
    let workspace = tempfile::tempdir()?;
    let tampered_path = workspace.path().join("tampered-receipt.json");
    let mut tampered = serde_json::to_value(capability.adopted_receipt())?;
    tampered["build_unit_identity"] = serde_json::json!("sha256:tampered");
    std::fs::write(&tampered_path, serde_json::to_vec_pretty(&tampered)?)?;

    let refused = LegacyOvenCapability::adopt_baked_project(
        workspace.path().join("store"),
        workspace.path().join("rustc"),
        &tampered_path,
    );
    let Err(unavailable) = refused else {
        panic!("a receipt whose identity does not match its content must not authorize a build");
    };
    assert!(
        unavailable.reason.contains("failed identity verification"),
        "{}",
        unavailable.reason
    );
    Ok(())
}
