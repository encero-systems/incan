//! RED-first comparison coverage for canonical scalar-conversion failures (#1249).
//!
//! Run with: `cargo test --test replacement_scalar_conversion_shadow_tests`

use std::path::Path;

use incan::backend::replacement::ReplacementValue;
use incan::backend::selection::{FallbackOutcome, ShadowComparisonState};
use incan::backend::shadow::legacy_oven::LegacyOvenCapability;
use incan::backend::shadow::{
    FunctionResultKind, RouteEvidence, ShadowComparison, ShadowComparisonProfile, SourceObservable,
    TypedFunctionResult, compare_source_observable,
};

#[path = "support/shadow_capability.rs"]
mod shadow_capability;

const INT_CONVERSION_FAILURE_SRC: &str =
    "def parse(value: str) -> int:\n    println(\"before conversion\")\n    return int(value)\n";
const FLOAT_CONVERSION_FAILURE_SRC: &str = "def parse(value: str) -> str:\n    println(\"before conversion\")\n    parsed = float(value)\n    return str(parsed)\n";
const FLOAT_LITERAL_DISPLAY_SRC: &str = "def render() -> str:\n    return f\"{str(1_000.50)} {str(1.25e2)}\"\n";
const FLOAT_CAST_EDGE_SRC: &str = "def render() -> str:\n    nan = float(\"NaN\")\n    positive_infinity = float(\"inf\")\n    negative_infinity = float(\"-inf\")\n    out_of_range = float(\"1e9999\")\n    negative_fraction = float(\"-3.9\")\n    return f\"{int(nan)} {int(positive_infinity)} {int(negative_infinity)} {int(out_of_range)} {int(3.9)} {int(negative_fraction)}\"\n";
const ADVERSARIAL_PARSE_INPUT: &str = "AssertionError overflow division by zero";
const SCALAR_CONVERSION_MATRIX_SRC: &str = r#"
def convert() -> str:
    integer = int(42)
    true_int = int(true)
    false_int = int(false)
    parsed = int("42")
    truncated = int(3.9)
    widened = float(10)
    float_identity = float(3.14)
    float_parsed = float("3.14")
    return f"{str(integer)} {str(true)} {str(false)} {str('text')} {str(float_identity)} {true_int} {false_int} {parsed} {truncated} {widened} {float_identity} {float_parsed}"
"#;

/// Run one comparison against the staged Oven capability.
fn compare(
    profile: &ShadowComparisonProfile,
    workspace: &Path,
) -> Result<ShadowComparison, Box<dyn std::error::Error>> {
    let capability = LegacyOvenCapability::from_environment()?;
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

/// Conversion payload text must not be reclassified by older assertion/overflow/division substring rules.
#[test]
fn scalar_conversion_failure_keeps_its_canonical_class_before_legacy_substring_heuristics()
-> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason() {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    for (source, input, expected_label, expected_type) in [
        (
            INT_CONVERSION_FAILURE_SRC,
            ADVERSARIAL_PARSE_INPUT,
            "conversion-int",
            "int",
        ),
        (
            FLOAT_CONVERSION_FAILURE_SRC,
            ADVERSARIAL_PARSE_INPUT,
            "conversion-float",
            "float",
        ),
    ] {
        let workspace = tempfile::tempdir()?;
        let profile = ShadowComparisonProfile::new(source, "parse", vec![ReplacementValue::Str(input.to_string())]);
        let comparison = compare(&profile, workspace.path())?;

        assert!(
            matches!(comparison.state, ShadowComparisonState::Diverged { .. }),
            "matching conversion classes must still retain the native/direct stderr difference: {:?}",
            comparison.state
        );
        let (legacy, replacement) = route_evidence(&comparison)?;
        let SourceObservable::Failed {
            failure: legacy_failure,
        } = &legacy.observation.observable
        else {
            return Err("the native conversion must fail with a classified observable".into());
        };
        let SourceObservable::Failed {
            failure: replacement_failure,
        } = &replacement.observation.observable
        else {
            return Err("the direct conversion must fail with a classified observable".into());
        };
        assert_eq!(legacy_failure.label(), expected_label);
        assert_eq!(replacement_failure.label(), expected_label);
        assert_eq!(legacy.observation.stdout, b"before conversion\n");
        assert_eq!(replacement.observation.stdout, legacy.observation.stdout);
        assert_eq!(
            legacy.observation.stderr,
            format!("ValueError: cannot convert '{input}' to {expected_type}\n").as_bytes()
        );
        assert!(replacement.observation.stderr.is_empty());
        assert!(
            comparison.replacement_execution.is_none(),
            "a failed direct conversion cannot publish a successful Body-IR execution"
        );

        let legacy_receipt = legacy.receipt()?;
        let replacement_receipt = replacement.receipt()?;
        legacy_receipt.verify_identity()?;
        replacement_receipt.verify_identity()?;
        assert_eq!(legacy_receipt.shadow_comparison, comparison.state);
        assert_eq!(replacement_receipt.shadow_comparison, comparison.state);
        assert_eq!(legacy_receipt.fallback_outcome, FallbackOutcome::NotNeeded);
        assert_eq!(replacement_receipt.fallback_outcome, FallbackOutcome::NotNeeded);
    }
    Ok(())
}

/// Every admitted conversion pair must agree with its existing native implementation, including identity and bool
/// cases.
#[test]
fn every_admitted_scalar_conversion_pair_matches_the_native_route() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason() {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let workspace = tempfile::tempdir()?;
    let profile = ShadowComparisonProfile::new(SCALAR_CONVERSION_MATRIX_SRC, "convert", vec![]);
    let comparison = compare(&profile, workspace.path())?;
    assert!(comparison.matched(), "{:?}", comparison.state);
    let (legacy, replacement) = route_evidence(&comparison)?;
    let expected = SourceObservable::Completed {
        result: TypedFunctionResult {
            kind: FunctionResultKind::Str,
            value: "42 true false text 3.14 1 0 42 3 10 3.14 3.14".to_string(),
        },
    };
    assert_eq!(legacy.observation.observable, expected);
    assert_eq!(replacement.observation.observable, legacy.observation.observable);
    assert!(legacy.observation.stdout.is_empty());
    assert!(replacement.observation.stdout.is_empty());
    assert!(legacy.observation.stderr.is_empty());
    assert!(replacement.observation.stderr.is_empty());
    Ok(())
}

/// Lexer-normalized source literals must compare through the same f64 display semantics on both routes.
#[test]
fn ordinary_float_literal_display_matches_the_native_route() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason() {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let workspace = tempfile::tempdir()?;
    let profile = ShadowComparisonProfile::new(FLOAT_LITERAL_DISPLAY_SRC, "render", vec![]);
    let comparison = compare(&profile, workspace.path())?;

    assert!(comparison.matched(), "{:?}", comparison.state);
    let (legacy, replacement) = route_evidence(&comparison)?;
    let expected = SourceObservable::Completed {
        result: TypedFunctionResult {
            kind: FunctionResultKind::Str,
            value: "1000.5 125".to_string(),
        },
    };
    assert_eq!(legacy.observation.observable, expected);
    assert_eq!(replacement.observation.observable, legacy.observation.observable);
    assert!(legacy.observation.stdout.is_empty());
    assert!(replacement.observation.stdout.is_empty());
    assert!(legacy.observation.stderr.is_empty());
    assert!(replacement.observation.stderr.is_empty());
    Ok(())
}

/// Existing Rust parser and `as i64` edge behavior is observed through two independent routes, not redefined here.
#[test]
fn float_parser_and_int_cast_edges_match_the_native_route() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason() {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let workspace = tempfile::tempdir()?;
    let profile = ShadowComparisonProfile::new(FLOAT_CAST_EDGE_SRC, "render", vec![]);
    let comparison = compare(&profile, workspace.path())?;

    assert!(comparison.matched(), "{:?}", comparison.state);
    let (legacy, replacement) = route_evidence(&comparison)?;
    let expected = SourceObservable::Completed {
        result: TypedFunctionResult {
            kind: FunctionResultKind::Str,
            value: "0 9223372036854775807 -9223372036854775808 9223372036854775807 3 -3".to_string(),
        },
    };
    assert_eq!(legacy.observation.observable, expected);
    assert_eq!(replacement.observation.observable, legacy.observation.observable);
    assert!(legacy.observation.stdout.is_empty());
    assert!(replacement.observation.stdout.is_empty());
    assert!(legacy.observation.stderr.is_empty());
    assert!(replacement.observation.stderr.is_empty());
    Ok(())
}
