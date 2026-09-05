//! Unit coverage for the parts of the comparison that do not need a staged Oven capability.
//!
//! Everything here is about the *rules*: how a typed result report is recovered without consuming program streams,
//! how failures are classified, when
//! two observations may be compared at all, and what survives a comparison that could not run. The end-to-end
//! proof that two real executions agree lives in `tests/shadow_comparison_tests.rs`, because only a real Oven
//! build can supply it.

use super::*;

/// An internal shadow observation must never relay its private program output into the host process streams.
#[test]
fn replacement_shadow_observation_does_not_leak_program_output() -> Result<(), Box<dyn std::error::Error>> {
    const PROBE_ENV: &str = "INCAN_TEST_SHADOW_OUTPUT_CHILD";
    const OUTPUT: &str = "private-shadow-output-1249";
    if std::env::var_os(PROBE_ENV).is_some() {
        let source = format!("def observed() -> int:\n    println(\"{OUTPUT}\")\n    return 42\n");
        let profile = ShadowComparisonProfile::new(source, "observed", Vec::new());
        // Pin actual execution as well as isolation: an earlier profile refusal must not produce a false pass.
        let prepared = PreparedShadowProfile::new(&profile)?;
        let observed = observe_replacement_route(&profile, &prepared)?;
        let execution = observed.execution.ok_or("expected actual direct execution")?;
        assert_eq!(execution.value, ReplacementValue::Int(42));
        assert_eq!(execution.output.stdout(), format!("{OUTPUT}\n").as_bytes());
        return Ok(());
    }
    let child = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "backend::shadow::tests::replacement_shadow_observation_does_not_leak_program_output",
            "--nocapture",
        ])
        .env(PROBE_ENV, "1")
        .output()?;
    assert!(child.status.success(), "{}", String::from_utf8_lossy(&child.stderr));
    assert!(
        !child
            .stdout
            .windows(OUTPUT.len())
            .any(|window| window == OUTPUT.as_bytes())
    );
    assert!(
        !child
            .stderr
            .windows(OUTPUT.len())
            .any(|window| window == OUTPUT.as_bytes())
    );
    Ok(())
}

/// Shadow preparation may admit ordinary float parsing, but the direct route must classify non-finite exact results.
#[test]
fn replacement_shadow_route_classifies_runtime_non_finite_exact_f64_results() -> Result<(), Box<dyn std::error::Error>>
{
    let source = "def exact(value: str) -> f64:\n    return float(value)\n";
    for input in ["NaN", "inf", "-inf", "1e9999"] {
        let profile = ShadowComparisonProfile::new(source, "exact", vec![ReplacementValue::Str(input.to_string())]);
        let prepared = PreparedShadowProfile::new(&profile)?;
        let observed = observe_replacement_route(&profile, &prepared)?;

        assert!(
            observed.execution.is_none(),
            "{input} must not produce direct execution evidence"
        );
        assert_eq!(
            observed.observation.as_ref().map(|observation| &observation.observable),
            Some(&SourceObservable::Failed {
                failure: RuntimeFailureClass::NonFiniteExactF64,
            }),
            "{input} must retain the exact-float failure class"
        );
        assert!(observed.output.stdout().is_empty(), "{input} unexpectedly wrote stdout");
        assert!(observed.output.stderr().is_empty(), "{input} unexpectedly wrote stderr");
        assert!(
            observed.unavailable_reason.is_none(),
            "{input}: classified failure became unavailable"
        );
    }
    Ok(())
}

#[test]
fn exact_float_failure_payloads_are_classified_without_heuristics() -> Result<(), Box<dyn std::error::Error>> {
    for (detail, expected) in [
        (
            "ValueError: non-finite float cannot initialize exact f32",
            RuntimeFailureClass::NonFiniteExactF32,
        ),
        (
            "ValueError: non-finite float cannot initialize exact f64\n",
            RuntimeFailureClass::NonFiniteExactF64,
        ),
    ] {
        assert_eq!(classify_replacement_failure(detail)?, expected);
        assert_eq!(classify_legacy_failure(detail)?, expected);
    }
    Ok(())
}

fn profile() -> ShadowComparisonProfile {
    ShadowComparisonProfile::new(
        "def add(x: int, y: int) -> int:\n    return x + y\n",
        "add",
        vec![ReplacementValue::Int(40), ReplacementValue::Int(2)],
    )
}

/// A source-session provider closure cannot borrow an unrelated adopted native plan.
#[test]
fn mismatched_native_build_inputs_are_unavailable_before_materialization() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let generated_source = workspace.path().join("adopted-main.rs");
    std::fs::write(&generated_source, "fn main() {}\n")?;
    let adopted_receipt = crate::oven::receipt_generated_project(
        &crate::oven::OvenGeneratedProjectRequest::new(
            workspace.path(),
            "shadow-context-fixture",
            "0.1.0",
            "fixture-target",
            "fixture-toolchain",
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", &generated_source)
        .with_build_unit_input("provider-plan", "sha256:adopted-context"),
    )?;
    let adopted_receipt_path = workspace.path().join("adopted-receipt.json");
    std::fs::write(&adopted_receipt_path, serde_json::to_vec(&adopted_receipt)?)?;
    let capability = legacy_oven::LegacyOvenCapability::adopt_baked_project(
        workspace.path().join("store"),
        workspace.path().join("rustc"),
        &adopted_receipt_path,
    )?;
    let context_profile = profile();
    let materialization = ShadowLegacyMaterialization::from_provider_plan(
        std::sync::Arc::new(crate::provider::ProviderPlan::default()),
        std::collections::BTreeMap::from([("provider-plan".to_string(), "sha256:source-session-context".to_string())]),
        context_profile.source_identity(),
    );

    let comparison = compare_source_observable_with_materialization(
        &context_profile,
        &materialization,
        &capability,
        workspace.path(),
    );
    let reason = comparison
        .unavailable_reason()
        .ok_or("mismatched native inputs must not produce a comparison outcome")?;
    assert!(
        reason.contains("does not match the adopted Oven build-unit inputs"),
        "{reason}"
    );
    assert!(comparison.replacement.is_none());
    assert!(comparison.legacy.is_none());
    assert!(comparison.legacy_authority.is_none());
    assert!(comparison.legacy_process.is_none());
    Ok(())
}

/// A session prepared for source A cannot authorize a same-closure comparison over source B.
#[test]
fn mismatched_profile_source_is_unavailable_before_materialization() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let generated_source = workspace.path().join("adopted-main.rs");
    std::fs::write(&generated_source, "fn main() {}\n")?;
    let adopted_receipt = crate::oven::receipt_generated_project(
        &crate::oven::OvenGeneratedProjectRequest::new(
            workspace.path(),
            "shadow-source-fixture",
            "0.1.0",
            "fixture-target",
            "fixture-toolchain",
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", &generated_source)
        .with_build_unit_input("provider-plan", "sha256:shared-context"),
    )?;
    let adopted_receipt_path = workspace.path().join("adopted-receipt.json");
    std::fs::write(&adopted_receipt_path, serde_json::to_vec(&adopted_receipt)?)?;
    let capability = legacy_oven::LegacyOvenCapability::adopt_baked_project(
        workspace.path().join("store"),
        workspace.path().join("rustc"),
        &adopted_receipt_path,
    )?;
    let source_a = profile();
    let source_b = ShadowComparisonProfile::new(
        "def add(x: int, y: int) -> int:\n    return x - y\n",
        "add",
        vec![ReplacementValue::Int(40), ReplacementValue::Int(2)],
    );
    let materialization = ShadowLegacyMaterialization::from_provider_plan(
        std::sync::Arc::new(crate::provider::ProviderPlan::default()),
        std::collections::BTreeMap::from([("provider-plan".to_string(), "sha256:shared-context".to_string())]),
        source_a.source_identity(),
    );

    let comparison =
        compare_source_observable_with_materialization(&source_b, &materialization, &capability, workspace.path());
    let reason = comparison
        .unavailable_reason()
        .ok_or("a profile that differs from its prepared source must not compare")?;
    assert!(
        reason.contains("profile source does not match the source session"),
        "{reason}"
    );
    assert!(comparison.replacement.is_none());
    assert!(comparison.legacy.is_none());
    assert!(comparison.legacy_authority.is_none());
    assert!(comparison.legacy_process.is_none());
    Ok(())
}

fn authority() -> LegacyExecutionAuthority {
    LegacyExecutionAuthority {
        oven_receipt_identity: "sha256:oven-receipt".to_string(),
        oven_build_unit_identity: "sha256:build-unit".to_string(),
        direct_rustc_plan_identity: "sha256:plan".to_string(),
        output_digest: "sha256:output".to_string(),
        cargo_process_started: false,
    }
}

fn observation(profile_identity: &str, observable: SourceObservable, detail: &str) -> RouteObservation {
    RouteObservation {
        profile_kind: SHADOW_COMPARISON_PROFILE_ID.to_string(),
        profile_identity: profile_identity.to_string(),
        output_identity: digest_output(&["test", detail]),
        observable,
        stdout: Vec::new(),
        stderr: Vec::new(),
        detail: detail.to_string(),
    }
}

fn completed(kind: FunctionResultKind, value: &str) -> SourceObservable {
    SourceObservable::Completed {
        result: TypedFunctionResult {
            kind,
            value: value.to_string(),
        },
    }
}

fn report(kind: FunctionResultKind, payload: &str) -> Vec<u8> {
    format!("{}{payload}", result_report_header(kind)).into_bytes()
}

fn process(
    exit_code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
    result_report: Option<Vec<u8>>,
) -> LegacyProcessEvidence {
    LegacyProcessEvidence {
        exit_code,
        stdout: stdout.to_vec(),
        stderr: stderr.to_vec(),
        result_report,
    }
}

// ============================================================================
// Lossless transport
// ============================================================================

#[test]
fn a_typed_result_report_round_trips_exactly() -> Result<(), ShadowUnavailable> {
    assert_eq!(
        decode_result_report(&report(FunctionResultKind::Int, "42"), FunctionResultKind::Int)?,
        TypedFunctionResult {
            kind: FunctionResultKind::Int,
            value: "42".to_string(),
        }
    );
    assert_eq!(
        decode_result_report(&report(FunctionResultKind::Unit, ""), FunctionResultKind::Unit)?,
        TypedFunctionResult {
            kind: FunctionResultKind::Unit,
            value: constructor_name(ConstructorId::None).to_string(),
        }
    );
    Ok(())
}

#[test]
fn the_result_report_protocol_uses_literal_ascii_colon_delimiters() {
    assert_eq!(
        result_report_header(FunctionResultKind::Int).as_bytes(),
        b"incan-shadow-result-v2:int:"
    );
    assert_eq!(
        report(FunctionResultKind::Str, "\nleading\ntrailing\n"),
        b"incan-shadow-result-v2:str:\nleading\ntrailing\n"
    );
}

#[test]
fn typed_numeric_result_reports_preserve_kind_and_validate_canonical_payloads() -> Result<(), ShadowUnavailable> {
    for (kind, payload) in [
        (FunctionResultKind::Float, "3.5"),
        (FunctionResultKind::Numeric(NumericTypeId::F32), "3.5"),
        (FunctionResultKind::Numeric(NumericTypeId::F64), "3.5"),
        (
            FunctionResultKind::Numeric(NumericTypeId::I128),
            "-170141183460469231731687303715884105728",
        ),
        (
            FunctionResultKind::Numeric(NumericTypeId::U128),
            "340282366920938463463374607431768211455",
        ),
        (FunctionResultKind::Decimal { precision: 6, scale: 2 }, "19.90"),
    ] {
        assert_eq!(
            decode_result_report(&report(kind, payload), kind)?,
            TypedFunctionResult {
                kind,
                value: payload.to_string(),
            }
        );
    }

    assert!(
        decode_result_report(
            &report(FunctionResultKind::Numeric(NumericTypeId::U8), "256"),
            FunctionResultKind::Numeric(NumericTypeId::U8)
        )
        .is_err()
    );
    assert!(
        decode_result_report(
            &report(FunctionResultKind::Numeric(NumericTypeId::F32), "NaN"),
            FunctionResultKind::Numeric(NumericTypeId::F32)
        )
        .is_err()
    );
    assert!(decode_result_report(&report(FunctionResultKind::Float, "inf"), FunctionResultKind::Float).is_err());
    assert!(
        decode_result_report(
            &report(FunctionResultKind::Decimal { precision: 5, scale: 2 }, "1234.5"),
            FunctionResultKind::Decimal { precision: 5, scale: 2 }
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn trailing_newlines_in_a_result_survive_transport() -> Result<(), ShadowUnavailable> {
    // The report payload never shares stdout, so `"x"`, `"x\n"`, and `"\n"` stay distinct regardless of program
    // output bytes.
    let one = decode_result_report(&report(FunctionResultKind::Str, "x"), FunctionResultKind::Str)?;
    let newline = decode_result_report(&report(FunctionResultKind::Str, "x\n"), FunctionResultKind::Str)?;
    let only_newline = decode_result_report(&report(FunctionResultKind::Str, "\n"), FunctionResultKind::Str)?;
    assert_eq!(one.value, "x");
    assert_eq!(newline.value, "x\n");
    assert_eq!(only_newline.value, "\n");
    assert_ne!(
        completed(FunctionResultKind::Str, &one.value),
        completed(FunctionResultKind::Str, &newline.value)
    );
    Ok(())
}

#[test]
fn a_result_containing_header_text_is_still_recovered_exactly() -> Result<(), ShadowUnavailable> {
    let payload = format!("before\n{RESULT_REPORT_VERSION}\nafter");
    assert_eq!(
        decode_result_report(&report(FunctionResultKind::Str, &payload), FunctionResultKind::Str)?.value,
        payload
    );
    Ok(())
}

#[test]
fn a_malformed_or_wrongly_typed_report_is_unavailable_rather_than_guessed() {
    assert!(decode_result_report(b"42\n", FunctionResultKind::Int).is_err());
    assert!(decode_result_report(b"", FunctionResultKind::Int).is_err());
    assert!(decode_result_report(&report(FunctionResultKind::Bool, "true"), FunctionResultKind::Int).is_err());
    assert!(decode_result_report(&report(FunctionResultKind::Bool, "yes"), FunctionResultKind::Bool).is_err());
    assert!(
        decode_result_report(
            &report(FunctionResultKind::Unit, "unexpected"),
            FunctionResultKind::Unit
        )
        .is_err()
    );
}

#[test]
fn a_non_utf8_string_report_is_unavailable_rather_than_lossily_converted() {
    let mut payload = result_report_header(FunctionResultKind::Str).into_bytes();
    payload.push(0xFF);
    assert!(decode_result_report(&payload, FunctionResultKind::Str).is_err());
}

#[test]
fn the_generated_entrypoint_writes_a_typed_result_without_touching_program_streams() -> Result<(), ShadowUnavailable> {
    let profile = profile();
    let prepared = PreparedShadowProfile::new(&profile)?;
    let program = profile.legacy_program_source(
        prepared.result_kind,
        Path::new("/worker-owned/result"),
        &prepared.wrapper_identifiers,
    )?;
    assert!(program.contains("def add(x: int, y: int) -> int:"), "{program}");
    assert!(
        program.contains(
            "from rust::std::fs import rename as __incan_shadow_fs_rename_v1, write as __incan_shadow_fs_write_v1"
        ),
        "{program}"
    );
    assert!(
        program.contains("from rust::std::path import Path as __incan_shadow_rust_path_v1"),
        "{program}"
    );
    assert!(
        program.contains("from rust::std::process import exit as __incan_shadow_process_exit_v1"),
        "{program}"
    );
    assert!(
        program.contains(
            "def main() -> None:\n    \"\"\"Publish this harness call's typed result without sharing program streams.\"\"\""
        ),
        "{program}"
    );
    assert!(
        program.contains("__incan_shadow_result_value_v1 = add(40, 2)"),
        "{program}"
    );
    assert!(
        program.contains(
            "match __incan_shadow_fs_write_v1(__incan_shadow_rust_path_v1.new(\"/worker-owned/result.next\"),"
        ),
        "{program}"
    );
    assert!(
        program.contains("Err(_) => __incan_shadow_process_exit_v1(86)"),
        "{program}"
    );
    assert!(
        program.contains("match __incan_shadow_fs_rename_v1(__incan_shadow_rust_path_v1.new(\"/worker-owned/result.next\"), __incan_shadow_rust_path_v1.new(\"/worker-owned/result\"))"),
        "{program}"
    );
    assert!(
        program.contains("Err(_) => __incan_shadow_process_exit_v1(87)"),
        "{program}"
    );
    assert!(!program.contains("println("), "{program}");
    Ok(())
}

// ============================================================================
// Classification
// ============================================================================

#[test]
fn agreeing_observations_record_the_profile_and_the_compared_value() {
    let state = classify_observations(
        &observation("sha256:profile", completed(FunctionResultKind::Int, "42"), "legacy"),
        &observation(
            "sha256:profile",
            completed(FunctionResultKind::Int, "42"),
            "replacement",
        ),
    );
    assert_eq!(
        state,
        ShadowComparisonState::Matched {
            profile_kind: SHADOW_COMPARISON_PROFILE_ID.to_string(),
            profile_identity: "sha256:profile".to_string(),
            observable: observation("sha256:profile", completed(FunctionResultKind::Int, "42"), "legacy").describe(),
        }
    );
}

/// Equal returned values do not hide differences in either stream, its order, or non-UTF-8 bytes.
#[test]
fn raw_stream_differences_diverge_even_when_results_match() {
    for (legacy_stdout, legacy_stderr, replacement_stdout, replacement_stderr) in [
        (&b"first\nsecond\n"[..], &b""[..], &b"second\nfirst\n"[..], &b""[..]),
        (&b"x\n"[..], &b""[..], &b"x"[..], &b""[..]),
        (&b"x"[..], &b""[..], &b""[..], &b"x"[..]),
        (&b""[..], &b"warning\n"[..], &b""[..], &b""[..]),
        (&b""[..], &b"\xff"[..], &b""[..], &b"\xfe"[..]),
    ] {
        let mut legacy = observation("sha256:profile", completed(FunctionResultKind::Int, "42"), "legacy");
        let mut replacement = observation(
            "sha256:profile",
            completed(FunctionResultKind::Int, "42"),
            "replacement",
        );
        legacy.stdout = legacy_stdout.to_vec();
        legacy.stderr = legacy_stderr.to_vec();
        replacement.stdout = replacement_stdout.to_vec();
        replacement.stderr = replacement_stderr.to_vec();
        assert!(matches!(
            classify_observations(&legacy, &replacement),
            ShadowComparisonState::Diverged { .. }
        ));
    }
}

/// A shared failure class does not erase output written before that failure.
#[test]
fn failed_observations_compare_both_streams() {
    let outcome = SourceObservable::Failed {
        failure: RuntimeFailureClass::Assertion,
    };
    let mut legacy = observation("sha256:profile", outcome.clone(), "legacy failure");
    let mut replacement = observation("sha256:profile", outcome, "direct failure");
    legacy.stdout = b"before failure\n".to_vec();
    replacement.stdout = legacy.stdout.clone();
    legacy.stderr = b"\xffdiagnostic\n".to_vec();
    replacement.stderr = legacy.stderr.clone();
    assert!(matches!(
        classify_observations(&legacy, &replacement),
        ShadowComparisonState::Matched { .. }
    ));
    replacement.stdout.clear();
    assert!(matches!(
        classify_observations(&legacy, &replacement),
        ShadowComparisonState::Diverged { .. }
    ));
}

/// An unclassifiable direct failure retains accepted output without manufacturing a successful receipt.
#[test]
fn partial_direct_output_survives_unclassifiable_failure() -> Result<(), Box<dyn std::error::Error>> {
    let profile = ShadowComparisonProfile::new(
        "def observed() -> int:\n    println(\"before invalid index\")\n    values = [1]\n    return values[5]\n",
        "observed",
        Vec::new(),
    );
    let prepared = PreparedShadowProfile::new(&profile)?;
    let observed = observe_replacement_route(&profile, &prepared)?;
    assert!(observed.observation.is_none());
    assert!(observed.execution.is_none());
    assert_eq!(observed.output.stdout(), b"before invalid index\n");
    let comparison = assemble_comparison(
        &profile,
        profile.profile_identity(),
        Ok(observed),
        Err(ShadowUnavailable::new("legacy unstaged")),
    );
    assert!(!comparison.matched());
    assert!(comparison.replacement.is_none());
    let output = comparison
        .replacement_output
        .as_ref()
        .ok_or("partial direct output must survive")?;
    assert_eq!(output.stdout(), b"before invalid index\n");
    assert!(output.stderr().is_empty());
    let reason = comparison
        .unavailable_reason()
        .ok_or("unclassified failure must remain unavailable")?;
    assert!(reason.contains("cannot classify"), "{reason}");
    assert!(!reason.contains("neither route executed"), "{reason}");
    Ok(())
}

/// Failed-execution evidence must bind the bytes that preceded the same failure, not just its message.
#[test]
fn failure_output_identity_changes_with_prior_output() -> Result<(), Box<dyn std::error::Error>> {
    let mut identities = Vec::new();
    for text in ["first", "second"] {
        let source = format!("def observed() -> int:\n    println(\"{text}\")\n    assert false\n    return 0\n");
        let profile = ShadowComparisonProfile::new(source, "observed", Vec::new());
        let prepared = PreparedShadowProfile::new(&profile)?;
        let observed = observe_replacement_route(&profile, &prepared)?;
        let observation = observed.observation.ok_or("assertion should be classified")?;
        assert_eq!(observation.stdout, format!("{text}\n").as_bytes());
        identities.push(observation.output_identity);
    }
    assert_ne!(identities[0], identities[1]);
    Ok(())
}

#[test]
fn differing_results_diverge_and_name_both_sides() {
    let state = classify_observations(
        &observation(
            "sha256:profile",
            completed(FunctionResultKind::Int, "42"),
            "legacy detail",
        ),
        &observation(
            "sha256:profile",
            completed(FunctionResultKind::Int, "43"),
            "replacement detail",
        ),
    );
    let ShadowComparisonState::Diverged {
        profile_kind,
        profile_identity,
        detail,
    } = state
    else {
        panic!("differing observables must diverge");
    };
    assert_eq!(profile_kind, SHADOW_COMPARISON_PROFILE_ID);
    assert_eq!(profile_identity, "sha256:profile");
    assert!(detail.contains("completed(Int, \"42\")"), "{detail}");
    assert!(detail.contains("completed(Int, \"43\")"), "{detail}");
}

#[test]
fn a_whitespace_only_difference_diverges_and_stays_visible_in_the_detail() {
    let state = classify_observations(
        &observation("sha256:profile", completed(FunctionResultKind::Str, "x"), "legacy"),
        &observation(
            "sha256:profile",
            completed(FunctionResultKind::Str, "x\n"),
            "replacement",
        ),
    );
    let ShadowComparisonState::Diverged { detail, .. } = state else {
        panic!("`x` and `x\\n` are different results and must diverge");
    };
    assert!(detail.contains(r#"completed(Str, "x")"#), "{detail}");
    assert!(detail.contains(r#"completed(Str, "x\n")"#), "{detail}");
}

#[test]
fn observations_of_different_profiles_are_never_compared() {
    // Pairing two unrelated profile instances would manufacture a verdict about a comparison nobody ran.
    let state = classify_observations(
        &observation("sha256:profile-a", completed(FunctionResultKind::Int, "42"), "legacy"),
        &observation(
            "sha256:profile-b",
            completed(FunctionResultKind::Int, "43"),
            "replacement",
        ),
    );
    let ShadowComparisonState::Unavailable { reason } = state else {
        panic!("cross-profile observations must not produce a comparison verdict");
    };
    assert!(reason.contains("different comparison profiles"), "{reason}");
}

#[test]
fn a_completed_result_never_matches_a_runtime_failure() {
    let state = classify_observations(
        &observation(
            "sha256:profile",
            SourceObservable::Failed {
                failure: RuntimeFailureClass::Assertion,
            },
            "legacy",
        ),
        &observation("sha256:profile", completed(FunctionResultKind::Int, "0"), "replacement"),
    );
    assert!(matches!(state, ShadowComparisonState::Diverged { .. }));
}

#[test]
fn different_failure_classes_diverge_rather_than_agreeing_that_something_broke() {
    let state = classify_observations(
        &observation(
            "sha256:profile",
            SourceObservable::Failed {
                failure: RuntimeFailureClass::ArithmeticOverflow,
            },
            "legacy",
        ),
        &observation(
            "sha256:profile",
            SourceObservable::Failed {
                failure: RuntimeFailureClass::DivisionByZero,
            },
            "replacement",
        ),
    );
    assert!(matches!(state, ShadowComparisonState::Diverged { .. }));
}

#[test]
fn an_overflow_is_not_classified_as_a_division_by_zero() -> Result<(), ShadowUnavailable> {
    // The executor spells an unrepresentable quotient as an "integer division overflow". Reading the word
    // "division" and filing it as a division by zero would make two different behaviors compare equal.
    assert_eq!(
        classify_replacement_failure("integer division overflow")?,
        RuntimeFailureClass::ArithmeticOverflow
    );
    assert_eq!(
        classify_replacement_failure("division or modulo by zero")?,
        RuntimeFailureClass::DivisionByZero
    );
    assert_eq!(
        classify_legacy_failure("attempt to divide with overflow")?,
        RuntimeFailureClass::ArithmeticOverflow
    );
    assert_eq!(
        classify_legacy_failure("ZeroDivisionError: division by zero")?,
        RuntimeFailureClass::DivisionByZero
    );
    Ok(())
}

/// Keep canonical conversion identity ahead of unrelated words in the rejected input.
#[test]
fn canonical_conversion_failures_precede_incidental_diagnostic_words() -> Result<(), ShadowUnavailable> {
    for input in [
        "AssertionError overflow division by zero",
        "assertion",
        "overflow",
        "division by zero",
        "",
        "é\n' to float",
    ] {
        for (payload, expected_label) in [
            (
                incan_core::errors::IncanError::cannot_convert_to_int(input).to_string(),
                "conversion-int",
            ),
            (
                incan_core::errors::IncanError::cannot_convert_to_float(input).to_string(),
                "conversion-float",
            ),
        ] {
            assert_eq!(classify_replacement_failure(&payload)?.label(), expected_label);
            assert_eq!(
                classify_legacy_failure(&format!("{payload}\n"))?.label(),
                expected_label
            );
        }
    }
    Ok(())
}

/// A similar-looking message is not canonical conversion evidence without its complete framing.
#[test]
fn canonical_conversion_classification_requires_the_complete_payload() {
    for detail in [
        "cannot convert 'abc' to int",
        "ValueError: cannot convert 'abc' to integer",
        "ValueError: cannot convert 'abc' to float trailing",
        "ValueError: cannot convert 'abc to int",
        "valueerror: cannot convert 'abc' to int",
        "prefix ValueError: cannot convert 'abc' to int",
        "ValueError: cannot convert 'abc' to int\ntrailing",
        "ValueError: cannot convert 'abc' to int\n\n",
    ] {
        assert!(classify_replacement_failure(detail).is_err(), "{detail:?}");
        assert!(classify_legacy_failure(detail).is_err(), "{detail:?}");
    }
}

#[test]
fn an_unclassifiable_failure_stays_unavailable_on_both_routes() {
    assert!(classify_legacy_failure("Segmentation fault").is_err());
    assert!(classify_replacement_failure("something went wrong").is_err());
}

#[test]
fn a_failing_legacy_exit_is_never_read_as_a_result() {
    // A non-zero exit must be classified as a failure, not decoded as output that happens to be framed.
    let observed = observe_legacy_process(
        SHADOW_COMPARISON_PROFILE_ID,
        "sha256:profile",
        &authority(),
        &process(
            Some(101),
            b"program stdout",
            b"",
            Some(report(FunctionResultKind::Int, "42")),
        ),
        FunctionResultKind::Int,
    );
    assert!(
        observed.is_err(),
        "an unclassifiable failing exit must not yield a result"
    );
}

#[test]
fn the_legacy_output_identity_covers_its_oven_authority() -> Result<(), ShadowUnavailable> {
    let baseline = observe_legacy_process(
        SHADOW_COMPARISON_PROFILE_ID,
        "sha256:profile",
        &authority(),
        &process(
            Some(0),
            b"program stdout",
            b"program stderr",
            Some(report(FunctionResultKind::Int, "42")),
        ),
        FunctionResultKind::Int,
    )?;
    let mut other_plan = authority();
    other_plan.direct_rustc_plan_identity = "sha256:different-plan".to_string();
    let under_other_plan = observe_legacy_process(
        SHADOW_COMPARISON_PROFILE_ID,
        "sha256:profile",
        &other_plan,
        &process(
            Some(0),
            b"program stdout",
            b"program stderr",
            Some(report(FunctionResultKind::Int, "42")),
        ),
        FunctionResultKind::Int,
    )?;
    assert_eq!(baseline.observable, under_other_plan.observable);
    assert_ne!(
        baseline.output_identity, under_other_plan.output_identity,
        "the same observed result under a different Oven authority is different evidence"
    );
    Ok(())
}

// ============================================================================
// Partial evidence
// ============================================================================

#[test]
fn an_executed_replacement_route_survives_an_unavailable_legacy_route() -> Result<(), Box<dyn std::error::Error>> {
    let profile = profile();
    let prepared = PreparedShadowProfile::new(&profile)?;
    let replacement = observe_replacement_route(&profile, &prepared)?;
    let comparison = assemble_comparison(
        &profile,
        profile.profile_identity(),
        Ok(replacement),
        Err(ShadowUnavailable::new("no Oven plan is staged")),
    );

    let reason = comparison
        .unavailable_reason()
        .ok_or("a missing legacy route must record an unavailable comparison")?;
    assert!(reason.contains("no Oven plan is staged"), "{reason}");
    assert!(!comparison.matched());
    assert!(comparison.legacy.is_none(), "the legacy route did not execute");

    // The replacement route really ran, so its receipt and Body-IR evidence must not be thrown away.
    let replacement_evidence = comparison
        .replacement
        .as_ref()
        .ok_or("an executed replacement route must keep its receipt")?;
    let replacement_receipt = replacement_evidence.receipt()?;
    replacement_receipt.verify_identity()?;
    assert_eq!(replacement_receipt.shadow_comparison, comparison.state);
    assert_eq!(
        replacement_evidence.observation.observable,
        completed(FunctionResultKind::Int, "42"),
        "the retained observation must be the one that was really executed"
    );
    let execution = comparison
        .replacement_execution
        .as_ref()
        .ok_or("an executed replacement route must keep its Body-IR execution")?;
    assert_eq!(execution.value, ReplacementValue::Int(42));
    Ok(())
}

#[test]
fn an_executed_route_without_a_receipt_reports_that_rather_than_vanishing() {
    // Receipt finalization cannot fail for this module's fixed inputs, so this covers the contract rather than a
    // reachable path: if it ever does fail, the route's execution stays visible and the missing receipt reads as
    // the explicit failure it is.
    let evidence = RouteEvidence {
        receipt: None,
        observation: observation("sha256:profile", completed(FunctionResultKind::Int, "42"), "legacy"),
    };
    let Err(reason) = evidence.receipt() else {
        panic!("an evidence entry with no receipt must not report one");
    };
    assert!(reason.contains("could not be finalized"), "{reason}");
    assert_eq!(
        evidence.observation.observable,
        completed(FunctionResultKind::Int, "42"),
        "the observation must survive a missing receipt"
    );
}

#[test]
fn a_comparison_with_no_executed_route_keeps_no_receipts() {
    let comparison = assemble_comparison(
        &profile(),
        profile().profile_identity(),
        Err(ShadowUnavailable::new("replacement refused")),
        Err(ShadowUnavailable::new("legacy unstaged")),
    );
    let Some(reason) = comparison.unavailable_reason() else {
        panic!("two failed routes must record an unavailable comparison");
    };
    assert!(reason.contains("replacement refused"), "{reason}");
    assert!(reason.contains("legacy unstaged"), "{reason}");
    assert!(comparison.legacy.is_none());
    assert!(comparison.replacement.is_none());
    assert!(comparison.legacy_authority.is_none());
    assert!(comparison.legacy_process.is_none());
}

// ============================================================================
// Profile boundaries
// ============================================================================

#[test]
fn observing_a_program_entrypoint_is_outside_the_profile() {
    let profile = ShadowComparisonProfile::new("def main() -> int:\n    return 42\n", "main", vec![]);
    let Err(unavailable) = PreparedShadowProfile::new(&profile) else {
        panic!("a `main` observation must be refused");
    };
    assert_eq!(unavailable.reason, PROGRAM_ENTRYPOINT_UNAVAILABLE_REASON);
}

#[test]
fn an_inactive_feature_gated_function_is_unavailable_to_both_comparison_routes() {
    let profile = ShadowComparisonProfile::new(
        "when feature(\"beta\"):\n    def gated() -> int:\n        return 42\n",
        "gated",
        vec![],
    );

    let Err(replacement) = PreparedShadowProfile::new(&profile) else {
        panic!("neither route must prepare a function projected out by an inactive feature");
    };
    assert!(
        replacement
            .reason
            .contains("absent from the manifest-free feature projection"),
        "{replacement:?}"
    );
}

#[test]
fn a_non_scalar_argument_is_refused_rather_than_guessed() {
    let profile = ShadowComparisonProfile::new(
        "def echo(value: int) -> int:\n    return value\n",
        "echo",
        vec![ReplacementValue::Tuple(vec![ReplacementValue::Int(1)])],
    );
    let identifiers = GeneratedWrapperIdentifiers::for_version(1);
    let Err(unavailable) =
        profile.legacy_program_source(FunctionResultKind::Int, Path::new("/worker-owned/result"), &identifiers)
    else {
        panic!("a tuple argument has no source literal and must be refused");
    };
    assert!(unavailable.reason.contains("source literals"), "{}", unavailable.reason);
}

#[test]
fn string_arguments_are_escaped_into_incan_literals() -> Result<(), ShadowUnavailable> {
    let profile = ShadowComparisonProfile::new(
        "def greet(name: str) -> str:\n    return \"hello, \" + name\n",
        "greet",
        vec![ReplacementValue::Str("A\"da\\".to_string())],
    );
    let prepared = PreparedShadowProfile::new(&profile)?;
    let program = profile.legacy_program_source(
        prepared.result_kind,
        Path::new("/worker-owned/result"),
        &prepared.wrapper_identifiers,
    )?;
    assert!(
        program.contains(r#"__incan_shadow_result_value_v1 = greet("A\"da\\")"#),
        "{program}"
    );
    Ok(())
}

#[test]
fn arguments_are_part_of_the_profile_identity() {
    let source = "def add(x: int, y: int) -> int:\n    return x + y\n";
    let first = ShadowComparisonProfile::new(source, "add", vec![ReplacementValue::Int(40), ReplacementValue::Int(2)]);
    let second = ShadowComparisonProfile::new(source, "add", vec![ReplacementValue::Int(40), ReplacementValue::Int(3)]);
    assert_ne!(first.profile_identity(), second.profile_identity());
    assert_eq!(
        first.source_identity(),
        second.source_identity(),
        "the same module must keep one source identity across argument lists"
    );
}

#[test]
fn exact_numeric_argument_kind_is_part_of_the_profile_identity() {
    let source = "def identity(value: f64) -> f64:\n    return value\n";
    let f32_argument = ShadowComparisonProfile::new(
        source,
        "identity",
        vec![ReplacementValue::Numeric(ReplacementNumericValue::F32(1.0))],
    );
    let f64_argument = ShadowComparisonProfile::new(
        source,
        "identity",
        vec![ReplacementValue::Numeric(ReplacementNumericValue::F64(1.0))],
    );
    assert_ne!(f32_argument.profile_identity(), f64_argument.profile_identity());
}

#[test]
fn typed_numeric_arguments_and_results_prepare_through_checked_types() -> Result<(), ShadowUnavailable> {
    let cases = [
        (
            "def identity(value: f32) -> f32:\n    return value\n",
            ReplacementNumericValue::F32(1.25),
            FunctionResultKind::Numeric(NumericTypeId::F32),
        ),
        (
            "def identity(value: u128) -> u128:\n    return value\n",
            ReplacementNumericValue::Unsigned {
                kind: NumericTypeId::U128,
                value: u128::MAX,
            },
            FunctionResultKind::Numeric(NumericTypeId::U128),
        ),
        (
            "def identity(value: decimal[6, 2]) -> decimal[6, 2]:\n    return value\n",
            ReplacementNumericValue::Decimal {
                precision: 6,
                scale: 2,
                coefficient: 1990,
                literal_scale: 2,
            },
            FunctionResultKind::Decimal { precision: 6, scale: 2 },
        ),
    ];
    for (source, argument, expected_kind) in cases {
        let profile = ShadowComparisonProfile::new(source, "identity", vec![ReplacementValue::Numeric(argument)]);
        let prepared = PreparedShadowProfile::new(&profile)?;
        assert_eq!(prepared.result_kind, expected_kind);
    }
    Ok(())
}

#[test]
fn malformed_typed_numeric_arguments_are_refused_before_source_synthesis() -> Result<(), ShadowUnavailable> {
    let profile = ShadowComparisonProfile::new(
        "def identity(value: decimal[5, 2]) -> decimal[5, 2]:\n    return value\n",
        "identity",
        vec![ReplacementValue::Numeric(ReplacementNumericValue::Decimal {
            precision: 5,
            scale: 2,
            coefficient: 12345,
            literal_scale: 0,
        })],
    );
    let unavailable = match argument_literal(&profile.arguments[0]) {
        Err(unavailable) => unavailable,
        Ok(_) => {
            return Err(ShadowUnavailable::new(
                "a decimal exceeding its declared integer-width budget must be refused",
            ));
        }
    };
    assert!(
        unavailable.reason.contains("malformed `decimal[5, 2]`"),
        "{unavailable:?}"
    );
    Ok(())
}

#[test]
fn source_bindings_do_not_shadow_generated_result_transport_imports() -> Result<(), ShadowUnavailable> {
    let profile = ShadowComparisonProfile::new(
        "const IoError: int = 1\n\
         const RustPath: int = 2\n\n\
         def write() -> int:\n    return 42\n\n\
         def rename() -> int:\n    return 7\n",
        "write",
        vec![],
    );
    let prepared = PreparedShadowProfile::new(&profile)?;
    let program = profile.legacy_program_source(
        prepared.result_kind,
        Path::new("/worker-owned/result"),
        &prepared.wrapper_identifiers,
    )?;

    assert!(program.contains("def write() -> int:"), "{program}");
    assert!(program.contains("def rename() -> int:"), "{program}");
    assert!(program.contains("const IoError: int = 1"), "{program}");
    assert!(program.contains("const RustPath: int = 2"), "{program}");
    emit_legacy_rust(&program)?;
    Ok(())
}

#[test]
fn a_successful_legacy_process_without_a_result_report_is_transport_unavailable()
-> Result<(), Box<dyn std::error::Error>> {
    let observed = observe_legacy_process(
        SHADOW_COMPARISON_PROFILE_ID,
        "sha256:profile",
        &authority(),
        &process(Some(0), b"program stdout", b"program stderr", None),
        FunctionResultKind::Int,
    );
    let Err(unavailable) = observed else {
        return Err("a missing source-authored report must not become a completed program observation".into());
    };
    assert!(
        unavailable.reason.contains("source-authored result report"),
        "{unavailable:?}"
    );
    Ok(())
}

#[test]
fn private_transport_exit_statuses_are_unavailable_before_stderr_is_classified()
-> Result<(), Box<dyn std::error::Error>> {
    for (exit_code, step) in [
        (RESULT_TRANSPORT_WRITE_EXIT_STATUS, "write"),
        (RESULT_TRANSPORT_RENAME_EXIT_STATUS, "rename"),
    ] {
        let observed = observe_legacy_process(
            SHADOW_COMPARISON_PROFILE_ID,
            "sha256:profile",
            &authority(),
            &process(Some(exit_code), b"program stdout", b"assertion failed", None),
            FunctionResultKind::Int,
        );
        let Err(unavailable) = observed else {
            return Err(
                format!("private transport status {exit_code} must not become a source failure observation").into(),
            );
        };
        assert!(unavailable.reason.contains(step), "{unavailable:?}");
        assert!(unavailable.reason.contains("result transport"), "{unavailable:?}");
    }
    Ok(())
}

#[cfg(feature = "cli")]
#[test]
fn forced_source_transport_failures_keep_program_streams_and_stay_unavailable() -> Result<(), Box<dyn std::error::Error>>
{
    let capability = match legacy_oven::LegacyOvenCapability::from_environment() {
        Ok(capability) => capability,
        Err(unavailable) if std::env::var_os("INCAN_SHADOW_REQUIRE_LEGACY_ROUTE").is_none() => {
            eprintln!("skipping source transport-failure evidence: {}", unavailable.reason);
            return Ok(());
        }
        Err(unavailable) => return Err(unavailable.into()),
    };
    let profile = ShadowComparisonProfile::new(
        "def announce() -> int:\n    println(\"program stdout before transport\")\n    return 42\n",
        "announce",
        vec![],
    );
    let prepared = PreparedShadowProfile::new(&profile)?;

    for (failure, exit_code, step) in [
        (
            legacy_oven::ForcedResultTransportFailure::Write,
            RESULT_TRANSPORT_WRITE_EXIT_STATUS,
            "write",
        ),
        (
            legacy_oven::ForcedResultTransportFailure::Rename,
            RESULT_TRANSPORT_RENAME_EXIT_STATUS,
            "rename",
        ),
    ] {
        let workspace = tempfile::tempdir()?;
        let source_path = workspace.path().join("transport-failure-shadow-profile.incn");
        std::fs::write(&source_path, profile.source())?;
        let materialization = crate::cli::commands::shadow_support::prepare_shadow_legacy_materialization(
            &source_path,
            &crate::provider::FeatureSelection::default(),
            None,
        )?;
        let legacy = legacy_oven::observe_legacy_route_with_forced_transport_failure(
            &profile,
            &prepared,
            &materialization,
            &capability,
            workspace.path(),
            failure,
        )?;

        assert!(legacy.observation.is_none(), "{:?}", legacy.process);
        assert_eq!(legacy.process.exit_code, Some(exit_code), "{:?}", legacy.process);
        assert_eq!(
            legacy.process.stdout, b"program stdout before transport\n",
            "{:?}",
            legacy.process
        );
        assert!(legacy.process.stderr.is_empty(), "{:?}", legacy.process);
        assert!(legacy.process.result_report.is_none(), "{:?}", legacy.process);
        let reason = legacy
            .unavailable_reason
            .as_deref()
            .ok_or("a forced source transport failure must be unavailable")?;
        assert!(reason.contains(step), "{reason}");
        assert!(reason.contains("result transport"), "{reason}");
    }
    Ok(())
}

#[test]
fn a_source_entrypoint_is_unavailable_even_when_another_function_is_selected() -> Result<(), Box<dyn std::error::Error>>
{
    let profile = ShadowComparisonProfile::new(
        "def helper() -> int:\n    return 42\n\n\
         def main() -> int:\n    return 7\n",
        "helper",
        vec![],
    );

    let Err(unavailable) = PreparedShadowProfile::new(&profile) else {
        return Err("a source `main` must be refused before either route executes".into());
    };
    assert_eq!(unavailable.reason, PROGRAM_ENTRYPOINT_UNAVAILABLE_REASON);
    Ok(())
}

#[test]
fn a_source_process_import_is_unavailable_before_private_transport_statuses_can_be_imitated()
-> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "from rust::std::process import exit\n\n\
         def helper() -> int:\n    return 42\n",
        "import rust::std::process\n\n\
         def helper() -> int:\n    return 42\n",
        "from rust::std import process\n\n\
         def helper() -> int:\n    return 42\n",
    ] {
        let profile = ShadowComparisonProfile::new(source, "helper", vec![]);
        let Err(unavailable) = PreparedShadowProfile::new(&profile) else {
            return Err("source process imports must not impersonate private result-transport statuses".into());
        };
        assert!(unavailable.reason.contains("rust::std::process"), "{unavailable:?}");
    }
    Ok(())
}

#[test]
fn a_source_binding_that_matches_an_older_generated_temporary_selects_a_fresh_stem() -> Result<(), ShadowUnavailable> {
    let profile = ShadowComparisonProfile::new(
        "def __incan_shadow_result_value_v1() -> int:\n    return 42\n",
        "__incan_shadow_result_value_v1",
        vec![],
    );

    let prepared = PreparedShadowProfile::new(&profile)?;
    assert_eq!(
        prepared.wrapper_identifiers.result_value,
        "__incan_shadow_result_value_v2"
    );
    let program = profile.legacy_program_source(
        prepared.result_kind,
        Path::new("/worker-owned/result"),
        &prepared.wrapper_identifiers,
    )?;
    assert!(
        program.contains("__incan_shadow_result_value_v2 = __incan_shadow_result_value_v1()"),
        "{program}"
    );
    emit_legacy_rust(&program)?;
    Ok(())
}
