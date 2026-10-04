//! What every subject of the parity corpus shares: the stable case identifiers of the paired-comparison rows,
//! the fixture sources the seed corpus embeds, and the frontend probes (typecheck, Body IR lowering and
//! snapshot) the evaluate callbacks fold into a `ComparisonOutcome`.

use super::*;

/// The original scalar case that exercises the reusable paired-comparison route.
pub(super) const SHADOW_COMPARED_CASE_ID: &str = "replacement-body-v0-001";

/// The selected canonical list-iteration row that also carries a receipt-backed paired comparison.
pub(super) const ENUMERATE_ZIP_SHADOW_CASE_ID: &str = "replacement-body-v0-023";

/// Hashed membership has its own stable paired case; adding direct execution alone never widens this list.
pub(super) const HASHED_SHADOW_CASE_ID: &str = "replacement-body-v0-020";

/// Selected checked string helpers have a separate case; wider string/format behavior stays non-green.
pub(super) const STRING_HELPER_SHADOW_CASE_ID: &str = "replacement-body-v0-021";

/// Scalar conversions have their own paired case without admitting the broader numeric surface.
pub(super) const SCALAR_CONVERSIONS_SHADOW_CASE_ID: &str = "replacement-body-v0-022";

/// Unicode-scalar string length has a separate case; other builtin operand profiles stay bounded.
pub(super) const STRING_LEN_SHADOW_CASE_ID: &str = "replacement-body-v0-024";

/// Scalar JSON stringification has a separate exact-byte paired case.
pub(super) const JSON_STRINGIFY_SHADOW_CASE_ID: &str = "replacement-body-v0-025";

/// Hashed set/dict entry count has a separate paired case without admitting broader aggregate operations.
pub(super) const COLLECTION_LEN_SHADOW_CASE_ID: &str = "replacement-body-v0-026";

/// Canonical bounded truthiness has its own paired case without admitting every frontend-supported carrier.
pub(super) const BOOL_TRUTHINESS_SHADOW_CASE_ID: &str = "replacement-body-v0-027";

/// Nonempty integer-list sorting has a separate paired case without admitting general ordering.
pub(super) const SORTED_INT_LIST_SHADOW_CASE_ID: &str = "replacement-body-v0-028";

/// Exact numeric carriers have one bounded paired case; #988 still owns their operation surface.
pub(super) const TYPED_NUMERIC_SHADOW_CASE_ID: &str = "replacement-body-v0-029";

/// Checked primitive `isinstance` targets have a case-scoped paired proof without promoting broad union/nominal work.
pub(super) const ISINSTANCE_TARGETS_SHADOW_CASE_ID: &str = "replacement-body-v0-030";

pub(super) const SHADOW_COMPARED_CASE_IDS: [&str; 12] = [
    SHADOW_COMPARED_CASE_ID,
    HASHED_SHADOW_CASE_ID,
    STRING_HELPER_SHADOW_CASE_ID,
    SCALAR_CONVERSIONS_SHADOW_CASE_ID,
    ENUMERATE_ZIP_SHADOW_CASE_ID,
    STRING_LEN_SHADOW_CASE_ID,
    JSON_STRINGIFY_SHADOW_CASE_ID,
    COLLECTION_LEN_SHADOW_CASE_ID,
    BOOL_TRUTHINESS_SHADOW_CASE_ID,
    SORTED_INT_LIST_SHADOW_CASE_ID,
    TYPED_NUMERIC_SHADOW_CASE_ID,
    ISINSTANCE_TARGETS_SHADOW_CASE_ID,
];

pub(super) const BOOL_TRUTHINESS_SOURCE: &str = include_str!("../fixtures/replacement/bool_truthiness.incn");

pub(super) const HASHED_MEMBERSHIP_SOURCE: &str = include_str!("../fixtures/replacement/hashed_membership.incn");

pub(super) const COLLECTION_LEN_SOURCE: &str = include_str!("../fixtures/replacement/collection_len.incn");

pub(super) const STRING_HELPER_SOURCE: &str = include_str!("../fixtures/replacement/string_helpers.incn");

pub(super) const STRING_LEN_SOURCE: &str = include_str!("../fixtures/replacement/string_len.incn");

pub(super) const JSON_STRINGIFY_SCALARS_SOURCE: &str =
    include_str!("../fixtures/replacement/json_stringify_scalars.incn");

pub(super) const JSON_STRINGIFY_SCALARS_EXPECTED: &str =
    r#"7|-42|9223372036854775807|-9223372036854775807|true|false|"quote:\" slash:\\ line:\n tab:\t café 😀"|null"#;

pub(super) const SORTED_INT_LIST_SOURCE: &str = include_str!("../fixtures/replacement/sorted_int_list.incn");

pub(super) const ISINSTANCE_TARGETS_SOURCE: &str = include_str!("../fixtures/replacement/isinstance_targets.incn");

/// Lex, parse, and typecheck `src`, returning the typechecker's error messages (empty on success).
///
/// Mirrors the helper already used by `loaves/compiler/incan_emit/tests/construction_diagnostics_tests.rs` and
/// `loaves/compiler/incan_frontend/tests/semantic_core_parity.rs` — kept local rather than shared because each corpus
/// case wants a plain `ComparisonOutcome`, not a `Result` a caller must unwrap.
fn typecheck_err_messages(src: &str) -> Result<Vec<String>, Vec<String>> {
    let tokens = lexer::lex(src).map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>())?;
    let ast = parser::parse(&tokens).map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>())?;
    let mut tc = typechecker::TypeChecker::new();
    match tc.check_program(&ast) {
        Ok(()) => Ok(vec![]),
        Err(errs) => Ok(errs.into_iter().map(|e| e.message).collect()),
    }
}

/// Lex, parse, and typecheck `src`, returning the typechecker's non-fatal warnings, or a reason the probe could
/// not run at all.
///
/// Kept separate from [`typecheck_err_messages`] because warnings ride their own channel: `check_program` reports
/// only hard errors, so a case asserting a *warning* that read the error channel would pass for the wrong reason —
/// a silently accepted program and a correctly warned one look identical from there.
pub(super) fn typecheck_warnings(src: &str) -> Result<Vec<CompileError>, String> {
    let tokens = lexer::lex(src).map_err(|errs| format!("lex failed: {:?}", messages(errs)))?;
    let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {:?}", messages(errs)))?;
    let mut tc = typechecker::TypeChecker::new();
    tc.check_program(&ast)
        .map_err(|errs| format!("typecheck failed: {:?}", messages(errs)))?;
    Ok(tc.take_warnings())
}

/// Reduce diagnostics to their message text for probe-failure reporting.
fn messages(errors: Vec<CompileError>) -> Vec<String> {
    errors.into_iter().map(|error| error.message).collect()
}

/// Fold a `typecheck_err_messages` result (lex/parse failure or typecheck errors) into a `ComparisonOutcome`, given a
/// predicate over the typechecker error messages that decides whether the observed shape still matches the case's
/// documented expectation. Lower `src` to Body IR and report whether every construct in it is faithfully represented.
///
/// This is `EvidenceLane::DirectParserTypechecker` evidence: it exercises the frontend only, asserting that the
/// source is accepted *and* that lowering produced no `unsupported(...)` placeholder. It deliberately proves nothing
/// about execution — a `DirectReplacementBodyIr` row owns that, and neither lane establishes a receipt-aware
/// comparison, which #1146 owns.
pub(super) fn outcome_from_body_ir(src: &str, expect_desc: &str) -> ComparisonOutcome {
    let snapshot = match body_ir_snapshot(src, expect_desc) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    if snapshot.contains("unsupported(") {
        return ComparisonOutcome::Mismatch {
            detail: format!("expected {expect_desc}, but Body IR still contains a placeholder:\n{snapshot}"),
        };
    }
    ComparisonOutcome::Match
}

/// Lower `src` to Body IR and report whether it refuses under the exact label `expected_refusal` names.
///
/// The sibling of [`outcome_from_body_ir`] for a row whose disposition is `Disposition::Unsupported`: the case's
/// documented behavior is a *stated refusal*, so proving it means the placeholder is present and says which
/// construct it is, not merely that some placeholder exists. Matching on the label rather than on the bare
/// `unsupported(` prefix is what stops this row from staying green if the construct were later inlined and some
/// unrelated statement in the same source started refusing instead.
pub(super) fn outcome_from_body_ir_refusal(src: &str, expected_refusal: &str, expect_desc: &str) -> ComparisonOutcome {
    let snapshot = match body_ir_snapshot(src, expect_desc) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    if !snapshot.contains(expected_refusal) {
        return ComparisonOutcome::Mismatch {
            detail: format!("expected {expect_desc}, but Body IR did not carry that refusal:\n{snapshot}"),
        };
    }
    ComparisonOutcome::Match
}

/// Lex, parse, typecheck, and lower `src`, returning the rendered Body IR snapshot.
///
/// The shared front half of [`outcome_from_body_ir`] and [`outcome_from_body_ir_refusal`]. A failure before
/// lowering is returned as the `ComparisonOutcome` the caller should report rather than as an error the caller
/// must re-describe: lex/parse failures are `Incompatible` (the probe could not run at all), while a typecheck
/// failure is a real `Mismatch` (the source is supposed to be accepted).
pub(super) fn body_ir_snapshot(src: &str, expect_desc: &str) -> Result<String, ComparisonOutcome> {
    let tokens = lexer::lex(src).map_err(|errors| ComparisonOutcome::Incompatible {
        reason: format!("expected {expect_desc}, but lexing failed: {errors:?}"),
    })?;
    let program = parser::parse(&tokens).map_err(|errors| ComparisonOutcome::Incompatible {
        reason: format!("expected {expect_desc}, but parsing failed: {errors:?}"),
    })?;
    // The corpus is a caller of `build_body_ir_module_v0` like any other, so it owes that boundary the same
    // desugared, feature-projected program the CLI path owes it (#1166). A corpus lowering raw parse output would
    // measure a program the real pipeline never produces, and go green on the divergence it exists to surface.
    let program =
        incan_frontend::body_ir::apply_body_ir_input_contract(program, std::path::Path::new("parity_987_body_ir.incn"))
            .map_err(|errors| ComparisonOutcome::Incompatible {
                reason: format!("expected {expect_desc}, but the Body IR input contract refused: {errors:?}"),
            })?;
    let module_path = vec!["parity_987_body_ir".to_string()];
    let mut checker = typechecker::TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| ComparisonOutcome::Mismatch {
            detail: format!("expected {expect_desc}, but typechecking reported {errors:?}"),
        })?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()).render_snapshot())
}

pub(super) fn outcome_from_typecheck(
    src: &str,
    expect: impl FnOnce(&[String]) -> bool,
    expect_desc: &str,
) -> ComparisonOutcome {
    match typecheck_err_messages(src) {
        Err(errs) => ComparisonOutcome::Incompatible {
            reason: format!("lex/parse failed before typecheck could run: {errs:?}"),
        },
        Ok(errs) => {
            if expect(&errs) {
                ComparisonOutcome::Match
            } else {
                ComparisonOutcome::Mismatch {
                    detail: format!("expected {expect_desc}, got typechecker messages: {errs:?}"),
                }
            }
        }
    }
}
