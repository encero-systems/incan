//! Comparison coverage for canonical scalar-conversion success and failure behavior (#1249, #1278).
//!
//! Run with: `cargo test --test replacement_scalar_conversion_shadow_tests`

use std::path::Path;

use incan::backend::replacement::{ReplacementNumericValue, ReplacementValue};
use incan::backend::selection::{FallbackOutcome, ShadowComparisonState};
use incan::backend::shadow::legacy_oven::LegacyOvenCapability;
use incan::backend::shadow::{
    FunctionResultKind, RouteEvidence, ShadowComparison, ShadowComparisonProfile, SourceObservable, TypedFunctionResult,
};
use incan::cli::commands::compare_source_observable;
use incan_core::lang::types::numerics::NumericTypeId;

#[path = "support/shadow_capability.rs"]
mod shadow_capability;

const INT_CONVERSION_FAILURE_SRC: &str =
    "def parse(value: str) -> int:\n    println(\"before conversion\")\n    return int(value)\n";
const FLOAT_CONVERSION_FAILURE_SRC: &str = "def parse(value: str) -> str:\n    println(\"before conversion\")\n    parsed = float(value)\n    return str(parsed)\n";
const EXACT_FLOAT_FAILURE_SRC: &str =
    "def exact(value: str) -> f64:\n    println(\"before exact conversion\")\n    return float(value)\n";
const FLOAT_LITERAL_DISPLAY_SRC: &str = "def render() -> str:\n    return f\"{str(1_000.50)} {str(1.25e2)}\"\n";
const FLOAT_CAST_EDGE_SRC: &str = "def render() -> str:\n    nan = float(\"NaN\")\n    positive_infinity = float(\"inf\")\n    negative_infinity = float(\"-inf\")\n    out_of_range = float(\"1e9999\")\n    negative_fraction = float(\"-3.9\")\n    return f\"{int(nan)} {int(positive_infinity)} {int(negative_infinity)} {int(out_of_range)} {int(3.9)} {int(negative_fraction)}\"\n";
const TYPED_CAST_EDGE_SRC: &str = "def minimum() -> i128:\n    return -170141183460469231731687303715884105728\n\ndef render() -> str:\n    low = minimum()\n    wide: u128 = 340282366920938463463374607431768211455\n    return f\"{low} {int(low)} {int(wide)} {float(low)} {float(wide)}\"\n";
const ADVERSARIAL_PARSE_INPUT: &str = "AssertionError overflow division by zero";
const SCALAR_CONVERSION_MATRIX_SRC: &str = r#"
def convert() -> str:
    integer = int(42)
    true_int = int(true)
    false_int = int(false)
    parsed = int("1_000")
    truncated = int(3.9)
    widened = float(10)
    float_identity = float(3.14)
    float_parsed = float("1_000.50")
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

/// Conversion failures retain their canonical class and original input on both execution routes.
#[test]
fn scalar_conversion_failures_keep_their_canonical_class_and_original_input() -> Result<(), Box<dyn std::error::Error>>
{
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason()? {
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
        (INT_CONVERSION_FAILURE_SRC, "1__000", "conversion-int", "int"),
        (FLOAT_CONVERSION_FAILURE_SRC, "1_000._50", "conversion-float", "float"),
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

/// Generated/native and replacement execution both reject non-finite values at an exact-f64 return boundary.
#[test]
fn non_finite_exact_f64_returns_fail_with_the_same_class_on_required_routes() -> Result<(), Box<dyn std::error::Error>>
{
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason()? {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    for input in ["NaN", "inf", "-inf", "1e9999"] {
        let workspace = tempfile::tempdir()?;
        let profile = ShadowComparisonProfile::new(
            EXACT_FLOAT_FAILURE_SRC,
            "exact",
            vec![ReplacementValue::Str(input.to_string())],
        );
        let comparison = compare(&profile, workspace.path())?;

        assert!(
            matches!(comparison.state, ShadowComparisonState::Diverged { .. }),
            "matching exact-float failure classes retain the native/direct stderr difference: {:?}",
            comparison.state
        );
        let (legacy, replacement) = route_evidence(&comparison)?;
        let expected = SourceObservable::Failed {
            failure: incan::backend::shadow::RuntimeFailureClass::NonFiniteExactF64,
        };
        assert_eq!(legacy.observation.observable, expected, "native route accepted {input}");
        assert_eq!(
            replacement.observation.observable, expected,
            "replacement route accepted {input}"
        );
        assert_eq!(legacy.observation.stdout, b"before exact conversion\n");
        assert_eq!(replacement.observation.stdout, legacy.observation.stdout);
        assert_eq!(
            legacy.observation.stderr,
            b"ValueError: non-finite float cannot initialize exact f64\n"
        );
        assert!(replacement.observation.stderr.is_empty());
        assert!(comparison.replacement_execution.is_none());
        legacy.receipt()?.verify_identity()?;
        replacement.receipt()?.verify_identity()?;
    }
    Ok(())
}

/// Every admitted conversion pair must agree with its existing native implementation, including identity and bool
/// cases.
#[test]
fn every_admitted_scalar_conversion_pair_matches_the_native_route() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason()? {
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
            value: "42 true false text 3.14 1 0 1000 3 10 3.14 1000.5".to_string(),
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
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason()? {
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

/// Exact numeric arguments, result type, Display output, and typed result transport must agree with native code.
#[test]
fn typed_numeric_carriers_match_the_native_route_without_widening() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason()? {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let rounded = 1.234_567_9_f32;
    let cases = vec![
        (
            "def identity(value: f32) -> f32:\n    println(value)\n    return value\n",
            ReplacementValue::Numeric(ReplacementNumericValue::F32(rounded)),
            FunctionResultKind::Numeric(NumericTypeId::F32),
            rounded.to_string(),
        ),
        (
            "def identity(value: u128) -> u128:\n    println(value)\n    return value\n",
            ReplacementValue::Numeric(ReplacementNumericValue::Unsigned {
                kind: NumericTypeId::U128,
                value: u128::MAX,
            }),
            FunctionResultKind::Numeric(NumericTypeId::U128),
            u128::MAX.to_string(),
        ),
        (
            "def identity(value: decimal[6, 2]) -> decimal[6, 2]:\n    println(value)\n    return value\n",
            ReplacementValue::Numeric(ReplacementNumericValue::Decimal {
                precision: 6,
                scale: 2,
                coefficient: 1990,
                literal_scale: 2,
            }),
            FunctionResultKind::Decimal { precision: 6, scale: 2 },
            "19.90".to_string(),
        ),
    ];

    for (source, argument, kind, value) in cases {
        let workspace = tempfile::tempdir()?;
        let profile = ShadowComparisonProfile::new(source, "identity", vec![argument]);
        let comparison = compare(&profile, workspace.path())?;
        assert!(comparison.matched(), "{:?}", comparison.state);
        let (legacy, replacement) = route_evidence(&comparison)?;
        let expected = SourceObservable::Completed {
            result: TypedFunctionResult {
                kind,
                value: value.clone(),
            },
        };
        assert_eq!(legacy.observation.observable, expected);
        assert_eq!(replacement.observation.observable, legacy.observation.observable);
        assert_eq!(legacy.observation.stdout, format!("{value}\n").as_bytes());
        assert_eq!(replacement.observation.stdout, legacy.observation.stdout);
        assert!(legacy.observation.stderr.is_empty());
        assert!(replacement.observation.stderr.is_empty());
    }
    Ok(())
}

/// Existing Rust parser and `as i64` edge behavior is observed through two independent routes, not redefined here.
#[test]
fn float_parser_and_int_cast_edges_match_the_native_route() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason()? {
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

/// i128 minimum transport and typed integer-to-int/float casts must agree with the native emitter at its edges.
#[test]
fn typed_integer_transport_and_cast_edges_match_the_native_route() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason()? {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let workspace = tempfile::tempdir()?;
    let profile = ShadowComparisonProfile::new(TYPED_CAST_EDGE_SRC, "render", vec![]);
    let comparison = compare(&profile, workspace.path())?;

    assert!(comparison.matched(), "{:?}", comparison.state);
    let (legacy, replacement) = route_evidence(&comparison)?;
    let expected = SourceObservable::Completed {
        result: TypedFunctionResult {
            kind: FunctionResultKind::Str,
            value: format!(
                "{} {} {} {} {}",
                i128::MIN,
                i128::MIN as i64,
                u128::MAX as i64,
                i128::MIN as f64,
                u128::MAX as f64
            ),
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
