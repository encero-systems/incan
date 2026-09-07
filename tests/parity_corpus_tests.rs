//! Executable backend-cutover parity corpus (#987).
//!
//! Turns the #646 behavior inventory (`workspaces/docs-site/docs/contributing/reference/
//! backend_behavior_inventory.md`) into a runnable corpus with stable case IDs and an explicit disposition per
//! case, per #987's scope. See `tests/support/parity_corpus.rs` for the schema; implementation status lives on
//! issue #652, not in permanent contributor docs, since it describes an in-flight migration state rather than a
//! durable 0.6 end-state contract.
//!
//! Run with: `cargo test --test parity_corpus_tests`
//!
//! ## Why these cases
//!
//! The original source-only seed has grown with the RFC 120 cutover matrix. Package/import rows execute the checked
//! graph and both compiler consumers, while direct replacement package execution stays explicitly unavailable under
//! #989. The release-artifact row compiles and inspects its pinned native fixture; neither form is mislabeled as a
//! two-route source-observable execution.
//!
//! Each case's `evaluate` function probes the *current* compiler directly (not a fixture snapshot of past output),
//! so a behavior change shows up as [`parity_corpus::ComparisonOutcome::Mismatch`] the next time this test runs.

use incan::backend::IrCodegen;
use incan::backend::replacement::provider::{
    PROVIDER_COMPARISON_UNAVAILABLE_REASON, ProviderInputValue, ProviderInvocation, ProviderOperationHost,
    ProviderOperationOutcome, ProviderRuntime,
};
use incan::backend::replacement::{
    ReplacementExecutionError, ReplacementNumericValue, ReplacementValue, execute_free_function_with_providers,
};
use incan::frontend::body_ir::{build_body_ir_module_v0, build_body_ir_module_v0_with_provider_plan};
use incan::frontend::diagnostics::CompileError;
use incan::frontend::library_manifest_index::LibraryManifestIndex;
use incan::frontend::{lexer, parser, typechecker};
use incan::library_manifest::{CompiledProviderMetadata, LibraryManifest, ProviderOperationMetadata};
use incan::provider::{NamespaceAuthority, ProviderIdentity, ProviderPlan, ProviderProvenance, ProviderRecord};
use incan_semantics_core::authority::StaticAuthority;
use incan_semantics_core::receipts::{AttributeSensitivity, ReceiptAttribute, ReceiptStatus, ReplayClassification};
use incan_semantics_core::{
    AuthorityMode, CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
};
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

#[path = "support/emitted_symbol_artifact.rs"]
mod emitted_symbol_artifact;
#[path = "support/parity_corpus.rs"]
mod parity_corpus;
#[path = "support/shadow_capability.rs"]
mod shadow_capability;

/// The original scalar case that exercises the reusable paired-comparison route.
const SHADOW_COMPARED_CASE_ID: &str = "replacement-body-v0-001";
/// The selected canonical list-iteration row that also carries a receipt-backed paired comparison.
const ENUMERATE_ZIP_SHADOW_CASE_ID: &str = "replacement-body-v0-023";

/// Hashed membership has its own stable paired case; adding direct execution alone never widens this list.
const HASHED_SHADOW_CASE_ID: &str = "replacement-body-v0-020";
/// Selected checked string helpers have a separate case; wider string/format behavior stays non-green.
const STRING_HELPER_SHADOW_CASE_ID: &str = "replacement-body-v0-021";
/// Scalar conversions have their own paired case without admitting the broader numeric surface.
const SCALAR_CONVERSIONS_SHADOW_CASE_ID: &str = "replacement-body-v0-022";
/// Unicode-scalar string length has a separate case; other builtin operand profiles stay bounded.
const STRING_LEN_SHADOW_CASE_ID: &str = "replacement-body-v0-024";
/// Scalar JSON stringification has a separate exact-byte paired case.
const JSON_STRINGIFY_SHADOW_CASE_ID: &str = "replacement-body-v0-025";
/// Hashed set/dict entry count has a separate paired case without admitting broader aggregate operations.
const COLLECTION_LEN_SHADOW_CASE_ID: &str = "replacement-body-v0-026";
/// Canonical bounded truthiness has its own paired case without admitting every frontend-supported carrier.
const BOOL_TRUTHINESS_SHADOW_CASE_ID: &str = "replacement-body-v0-027";
/// Nonempty integer-list sorting has a separate paired case without admitting general ordering.
const SORTED_INT_LIST_SHADOW_CASE_ID: &str = "replacement-body-v0-028";
/// Exact numeric carriers have one bounded paired case; #988 still owns their operation surface.
const TYPED_NUMERIC_SHADOW_CASE_ID: &str = "replacement-body-v0-029";
/// Checked primitive `isinstance` targets have a case-scoped paired proof without promoting broad union/nominal work.
const ISINSTANCE_TARGETS_SHADOW_CASE_ID: &str = "replacement-body-v0-030";
const SHADOW_COMPARED_CASE_IDS: [&str; 12] = [
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
const BOOL_TRUTHINESS_SOURCE: &str = include_str!("fixtures/replacement/bool_truthiness.incn");
const HASHED_MEMBERSHIP_SOURCE: &str = include_str!("fixtures/replacement/hashed_membership.incn");
const COLLECTION_LEN_SOURCE: &str = include_str!("fixtures/replacement/collection_len.incn");
const STRING_HELPER_SOURCE: &str = include_str!("fixtures/replacement/string_helpers.incn");
const STRING_LEN_SOURCE: &str = include_str!("fixtures/replacement/string_len.incn");
const JSON_STRINGIFY_SCALARS_SOURCE: &str = include_str!("fixtures/replacement/json_stringify_scalars.incn");
const JSON_STRINGIFY_SCALARS_EXPECTED: &str =
    r#"7|-42|9223372036854775807|-9223372036854775807|true|false|"quote:\" slash:\\ line:\n tab:\t café 😀"|null"#;
const SORTED_INT_LIST_SOURCE: &str = include_str!("fixtures/replacement/sorted_int_list.incn");
const ISINSTANCE_TARGETS_SOURCE: &str = include_str!("fixtures/replacement/isinstance_targets.incn");

use parity_corpus::{
    BehaviorCategory, CheckedIdentityGraph, ComparisonOutcome, Disposition, EvidenceLane, IdentityAssertions,
    IdentityBindingForm, IdentityConformancePlan, IdentityConformanceSubject, IdentityCoverageCell,
    IdentityGraphDeferral, IdentityGraphEntrypoint, IdentityNamespace, IdentityReplacementPlan, IdentityScope,
    IdentitySourceModule, OverallState, ParityCase, ReceiptRef, ReleaseArtifactAssertions,
    SourceIdentityConformancePlan, behavior_observation_identity, exact_rust_identifier,
    identity_conformance_evidence_identity, validate_corpus, validate_identity_coverage,
};

// ============================================================================
// Shared frontend probes
// ============================================================================

/// Lex, parse, and typecheck `src`, returning the typechecker's error messages (empty on success).
///
/// Mirrors the helper already used by `tests/construction_diagnostics_tests.rs` and
/// `tests/semantic_core_parity.rs` — kept local rather than shared because each corpus case wants a plain
/// `ComparisonOutcome`, not a `Result` a caller must unwrap.
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
fn typecheck_warnings(src: &str) -> Result<Vec<CompileError>, String> {
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

/// Fold a `typecheck_err_messages` result (lex/parse failure or typecheck errors) into a `ComparisonOutcome`,
/// given a predicate over the typechecker error messages that decides whether the observed shape still matches
/// the case's documented expectation.
/// Lower `src` to Body IR and report whether every construct in it is faithfully represented.
///
/// This is `EvidenceLane::DirectParserTypechecker` evidence: it exercises the frontend only, asserting that the
/// source is accepted *and* that lowering produced no `unsupported(...)` placeholder. It deliberately proves nothing
/// about execution — a `DirectReplacementBodyIr` row owns that, and neither lane establishes a receipt-aware
/// comparison, which #1146 owns.
fn outcome_from_body_ir(src: &str, expect_desc: &str) -> ComparisonOutcome {
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
fn outcome_from_body_ir_refusal(src: &str, expected_refusal: &str, expect_desc: &str) -> ComparisonOutcome {
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
fn body_ir_snapshot(src: &str, expect_desc: &str) -> Result<String, ComparisonOutcome> {
    let tokens = lexer::lex(src).map_err(|errors| ComparisonOutcome::Incompatible {
        reason: format!("expected {expect_desc}, but lexing failed: {errors:?}"),
    })?;
    let program = parser::parse(&tokens).map_err(|errors| ComparisonOutcome::Incompatible {
        reason: format!("expected {expect_desc}, but parsing failed: {errors:?}"),
    })?;
    // The corpus is a caller of `build_body_ir_module_v0` like any other, so it owes that boundary the same
    // desugared, feature-projected program the CLI path owes it (#1166). A corpus lowering raw parse output would
    // measure a program the real pipeline never produces, and go green on the divergence it exists to surface.
    let program = incan::frontend::body_ir::apply_body_ir_input_contract(
        program,
        std::path::Path::new("parity_987_body_ir.incn"),
    )
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

/// Run one module through the direct route's source-profile gate and classify the refusal it produces.
///
/// This observes admission, not execution: the profile decides which source modules the direct route will run at
/// all, and a boundary it declines never reaches Body IR. `None` means the module was admitted.
fn outcome_from_source_profile(
    src: &str,
    expect: impl FnOnce(Option<&str>) -> bool,
    expect_desc: &str,
) -> ComparisonOutcome {
    let tokens = match lexer::lex(src) {
        Ok(tokens) => tokens,
        Err(errors) => {
            return ComparisonOutcome::Incompatible {
                reason: format!("lex failed before the profile could run: {errors:?}"),
            };
        }
    };
    let program = match parser::parse(&tokens) {
        Ok(program) => program,
        Err(errors) => {
            return ComparisonOutcome::Incompatible {
                reason: format!("parse failed before the profile could run: {errors:?}"),
            };
        }
    };
    let refusal = incan::backend::replacement::source_profile::source_profile_refusal(&program).map(|e| e.to_string());
    if expect(refusal.as_deref()) {
        ComparisonOutcome::Match
    } else {
        ComparisonOutcome::Mismatch {
            detail: format!("expected {expect_desc}, got profile refusal: {refusal:?}"),
        }
    }
}

fn outcome_from_typecheck(src: &str, expect: impl FnOnce(&[String]) -> bool, expect_desc: &str) -> ComparisonOutcome {
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

// ============================================================================
// Case 1 — Supported language contract: match exhaustiveness is enforced
// ============================================================================

const CASE_1_SRC: &str = r#"
enum Color:
    Red
    Green
    Blue

def name(c: Color) -> str:
    match c:
        case Color.Red:
            return "red"
        case Color.Green:
            return "green"
"#;

fn case_supported_match_exhaustiveness() -> ComparisonOutcome {
    outcome_from_typecheck(
        CASE_1_SRC,
        |errs| errs.iter().any(|e| e.to_lowercase().contains("exhaustive")),
        "a non-exhaustive-match diagnostic naming the missing `Blue` arm",
    )
}

// ============================================================================
// Case 2 — Diagnostic behavior: chained comparisons are rejected, not silently re-parsed
// ============================================================================

// Incan does not support Python-style chained comparisons (`a < b < c` as `a < b and b < c`). Verified by direct
// probe: today it type-errors because `(a < b) < c` compares a `bool` to an `int`. The corpus records that this
// stays a rejection, not a silent reinterpretation as chained boolean logic — a real semantic decision, not just
// token shape.
const CASE_2_SRC: &str = r#"
def main() -> None:
    a = 1
    b = 2
    c = 3
    if a < b < c:
        println("chained")
"#;

/// One package consumer: a call into a dependency through the public `pub::` surface.
const PACKAGE_CONSUMER_SRC: &str = r#"
from pub::widgets import build

def f() -> int:
    return build()
"#;

/// Confirm that a package consumer is still refused before the direct route reaches execution.
///
/// This row exists to keep the #989 package boundary counted rather than absent. It confirms current behavior, so
/// it flips the day #1339 ships an executable representation -- at which point the row must be promoted to a real
/// execution rather than quietly left recording a refusal that no longer happens.
fn case_package_consumer_call_is_refused() -> ComparisonOutcome {
    outcome_from_source_profile(
        PACKAGE_CONSUMER_SRC,
        |refusal| refusal.is_some(),
        "the direct route refuses a `pub::` package consumer",
    )
}

/// Confirm that the refusal is still reported as an unreached construct rather than in packaging terms.
///
/// RFC 123 requires a consumer that cannot obtain a usable representation to name the package, the version and the
/// unmet requirement, and never to report the condition as an unsupported language construct. Today it reports
/// `import declaration` -- the same misdiagnosis #1262 fixed for `rust::`. Pinning the current wording keeps the gap
/// visible and makes the row fail when #1339 corrects it.
fn case_package_representation_refusal_is_not_a_language_refusal() -> ComparisonOutcome {
    outcome_from_source_profile(
        PACKAGE_CONSUMER_SRC,
        |refusal| refusal.is_some_and(|text| text.contains("import declaration")),
        "the package boundary still refuses as an unreached construct rather than in packaging terms",
    )
}

fn case_diagnostic_chained_comparison_rejected() -> ComparisonOutcome {
    outcome_from_typecheck(
        CASE_2_SRC,
        |errs| !errs.is_empty(),
        "a type-mismatch diagnostic rejecting the chained comparison",
    )
}

// ============================================================================
// Case 3 — Stdlib/runtime behavior: string membership (`in`) matches the runtime helper
// ============================================================================

const CASE_3_SRC: &str = r#"
def f() -> bool:
    return "a" in "abc"
"#;

fn case_stdlib_runtime_string_membership() -> ComparisonOutcome {
    use incan_core::strings::str_contains;

    if !str_contains("hello", "hell") || str_contains("hello", "xyz") {
        return ComparisonOutcome::Mismatch {
            detail: "incan_core::strings::str_contains no longer matches its documented substring policy".to_string(),
        };
    }

    outcome_from_typecheck(
        CASE_3_SRC,
        |errs| errs.is_empty(),
        "`\"a\" in \"abc\"` to typecheck as bool with no errors, matching the runtime membership helper",
    )
}

/// The Body IR half of case 3, added by #1160.
///
/// The row above evaluates through the stdlib-runtime lane, which proves the substring policy is preserved but
/// says nothing about whether the cutover's own representation can express it. Until #1160, it could not: `in`
/// lowered to an `unsupported(...)` placeholder, so a `Preserved` disposition stood with no Body IR path behind
/// it — precisely the silent parity hole #987 exists to surface. This row is what keeps the two honest together:
/// it fails the moment string membership stops being representable, whatever the runtime helper still does.
fn case_supported_string_membership_reaches_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_3_SRC,
        "string membership to lower to an explicit compiler-owned helper call rather than a placeholder",
    )
}

// ============================================================================
// Case 4 — Generated-artifact behavior: codegen stays inspectable, not semantically authoritative
// ============================================================================

const CASE_4_SRC: &str = r#"
def add(a: int, b: int) -> int:
    return a + b
"#;

fn case_generated_artifact_valid_rust_shape() -> ComparisonOutcome {
    let Ok(tokens) = lexer::lex(CASE_4_SRC) else {
        return ComparisonOutcome::Incompatible {
            reason: "lexer failed on a fixture that must lex cleanly".to_string(),
        };
    };
    let Ok(ast) = parser::parse(&tokens) else {
        return ComparisonOutcome::Incompatible {
            reason: "parser failed on a fixture that must parse cleanly".to_string(),
        };
    };
    let rust_code = match IrCodegen::new().try_generate(&ast) {
        Ok(code) => code,
        Err(e) => {
            return ComparisonOutcome::Mismatch {
                detail: format!("codegen failed on a fixture that must generate cleanly: {e:?}"),
            };
        }
    };
    // This deliberately only checks that the output is syntactically valid Rust (still inspectable), never that
    // it matches a specific token layout — a byte-exact snapshot would make generated-Rust shape the semantic
    // contract, which the #646 inventory and the rust-source-backend deprecation policy both reject.
    match syn::parse_file(&rust_code) {
        Ok(_) => ComparisonOutcome::Match,
        Err(e) => ComparisonOutcome::Mismatch {
            detail: format!("generated Rust is not syntactically valid: {e}"),
        },
    }
}

// ============================================================================
// Case 5 — Supported language contract: lexical bindings shadow ambient builtins
// ============================================================================

// #1116 adopts this as a language contract: a direct module declaration or explicit import is a real lexical
// binding and wins over an ambient core builtin function for unqualified calls. `std.builtins.<name>` remains the
// explicit route to the builtin when the local spelling is shadowed. The corresponding typechecker, codegen, and
// runtime coverage lives alongside this corpus row; #653 must reproduce the same precedence deliberately.
const CASE_5_SRC: &str = r#"
def len(x: int) -> int:
    return x + 1

def main() -> None:
    y = len(5)
    println(y)
"#;

fn case_supported_builtin_len_shadowing() -> ComparisonOutcome {
    outcome_from_typecheck(
        CASE_5_SRC,
        |errs| errs.is_empty(),
        "a module `len` binding to shadow the ambient builtin without a diagnostic",
    )
}

// ============================================================================
// Cases 8 and 9 — Supported language contract: named call/construction binding reaches Body IR (#1158)
// ============================================================================

// Named field construction is the *only* spelling the typechecker accepts for a `model`/`class`: positional
// construction is rejected outright. Before #1158 that spelling lowered to `unsupported(...)`, so no nominal value
// was representable in Body IR at all — the canonical `User(id=..., email=...)` shape in this repository's own
// README included. The cutover must keep both the named-only source rule and its faithful representation.
const CASE_8_SRC: &str = r#"
model Point:
    x: int
    y: int = 5

def main() -> None:
    p = Point(y=2, x=1)
    println(p.x)
"#;

fn case_supported_named_construction_reaches_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_8_SRC,
        "named `model` construction to lower to real Body IR rather than an unsupported placeholder",
    )
}

// Named and defaulted arguments at an ordinary call site, including an argument written out of declaration order.
// The binding is what makes the operand order meaningful; the written order is what makes effect ordering
// meaningful. Both must survive the cutover.
const CASE_9_SRC: &str = r#"
def scale(value: int, factor: int = 2) -> int:
    return value * factor

def main() -> None:
    println(scale(factor=3, value=4))
    println(scale(5))
"#;

fn case_supported_named_call_arguments_reach_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_9_SRC,
        "named, out-of-order, and defaulted call arguments to lower to real Body IR",
    )
}

// ============================================================================
// Cases 10 and 11 — Supported language contract: async surface reaches Body IR (#1164)
// ============================================================================

// `AsyncAwait` is a public capability in the release-pinned baseline. Before #1164 an `await` lowered to a
// placeholder labelled only "prefix-keyword surface expression", so the suspension point — the one fact a task
// runtime needs — did not exist in Body IR at all. The cutover must keep both the source form and its
// representation, including the body-level async fact for a body that awaits nothing.
const CASE_10_SRC: &str = r#"
import std.async

async def fetch() -> int:
    return 7

async def main() -> None:
    value = await fetch()
    println(value)
"#;

fn case_supported_await_reaches_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_10_SRC,
        "`await` to lower to a real Body IR suspension point rather than an unsupported placeholder",
    )
}

// `AsyncRace` collapsed even harder: the whole `race for` expression became one placeholder, erasing every arm,
// arm body, and the shared binding. Both arm forms — a bare expression and a block with a trailing value — are
// part of the source contract.
const CASE_11_SRC: &str = r#"
import std.async

async def fast() -> int:
    return 1

async def slow() -> int:
    return 2

async def main() -> None:
    winner = race for value:
        await fast() => value
        await slow() =>
            doubled = value * 2
            doubled
    println(winner)
"#;

fn case_supported_race_for_reaches_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_11_SRC,
        "`race for` to lower to a real Body IR race with its arms, arm bodies, and bindings intact",
    )
}

// ============================================================================
// Cases 12 and 13 — Supported language contract: spread forms reach Body IR (#1159)
// ============================================================================

// `VariadicAndSpreadCalls` is a public capability. Before #1159 every spread form lowered to a placeholder, and
// for a list or dict literal the placeholder replaced the *whole* literal, so its fixed elements were erased too.
const CASE_12_SRC: &str = r#"
def main() -> None:
    xs = [2, 3]
    values = [1, *xs, 4]
    base = {"a": 1}
    merged = {**base, "b": 2}
    println(len(values))
    println(len(merged))
"#;

fn case_supported_literal_spreads_reach_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_12_SRC,
        "list and dict literal spreads to lower with their fixed elements intact",
    )
}

// Call-site spreads, including the combined form where a named argument sits alongside one. The callee's arity is
// a runtime fact here, so the call records no declared-slot binding — but every written argument form survives.
const CASE_13_SRC: &str = r#"
def log(a: int, b: int, *items: int, **fields: int) -> None:
    println(a)

def main() -> None:
    xs = [3, 4]
    kw = {"k": 5}
    log(1, *xs, b=2, **kw)
"#;

// ============================================================================
// Cases 20 and 21 — Pattern and `raises` assert forms reach Body IR (#1167)
// ============================================================================

// Both rows existed as refusals before #1167: `AssertKind::IsPattern` and `AssertKind::Raises` lowered to
// `unsupported(assert pattern/raises form)`. The pattern row is the one that mattered most, because the refusal was
// not merely incomplete -- `assert o is Some(v)` *binds* `v`, so lowering it to a placeholder dropped the binding
// and every later read of `v` lowered against a name the body never declared.
const CASE_20_SRC: &str = r#"
def run(o: Option[str]) -> None:
    assert o is Some(v)
    print(v)
"#;

const CASE_21_SRC: &str = r#"
def boom() -> int:
    return 1

def run() -> None:
    assert boom() raises ValueError
    assert boom() raises IndexError, "wanted an index error"
"#;

const CASE_22_SRC: &str = r#"
def has_item(xs: List[int], v: int) -> bool:
    return v in xs

def lacks_key(d: Dict[str, int], k: str) -> bool:
    return k not in d
"#;

const CASE_23_SRC: &str = r#"
def joined(xs: List[int], ys: List[int]) -> List[int]:
    return xs + ys
"#;

fn case_collection_membership_names_its_container() -> ComparisonOutcome {
    let outcome = outcome_from_body_ir(CASE_22_SRC, "collection membership to lower without a placeholder");
    if !matches!(outcome, ComparisonOutcome::Match) {
        return outcome;
    }
    let snapshot = match body_ir_snapshot(CASE_22_SRC, "collection membership to lower") {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    // Absence of a placeholder is not the property. Membership means something different per container -- element
    // lookup for a list, key lookup for a dict -- so the operation has to name which one the source held. A single
    // shared `contains` would satisfy a no-placeholder check while leaving that distinction to be re-derived.
    for helper in ["list_contains", "dict_not_contains_key"] {
        if !snapshot.contains(helper) {
            return ComparisonOutcome::Mismatch {
                detail: format!("collection membership did not name its container as {helper}:\n{snapshot}"),
            };
        }
    }
    if snapshot.contains("str_contains") {
        return ComparisonOutcome::Mismatch {
            detail: format!("collection membership borrowed the string substring policy:\n{snapshot}"),
        };
    }
    ComparisonOutcome::Match
}

fn case_list_concatenation_is_not_a_primitive_addition() -> ComparisonOutcome {
    let outcome = outcome_from_body_ir(CASE_23_SRC, "list concatenation to lower without a placeholder");
    if !matches!(outcome, ComparisonOutcome::Match) {
        return outcome;
    }
    let snapshot = match body_ir_snapshot(CASE_23_SRC, "list concatenation to lower") {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    // This row exists for a defect a no-placeholder check could never see. List `+` lowered *cleanly*, as
    // `BinOp::Add` -- a machine addition over two heap containers -- because the typechecker accepts list
    // concatenation through a builtin branch that records no operator dispatch. The corpus has to assert the
    // operation is a helper call, not merely that something was produced.
    if !snapshot.contains("call helper:list_concat(") {
        return ComparisonOutcome::Mismatch {
            detail: format!("list concatenation did not lower as its own helper:\n{snapshot}"),
        };
    }
    if snapshot.contains(") + ") {
        return ComparisonOutcome::Mismatch {
            detail: format!("list concatenation lowered as a primitive addition:\n{snapshot}"),
        };
    }
    ComparisonOutcome::Match
}

fn case_pattern_assertion_binding_reaches_body_ir() -> ComparisonOutcome {
    let outcome = outcome_from_body_ir(CASE_20_SRC, "a pattern assertion to lower without a placeholder");
    if !matches!(outcome, ComparisonOutcome::Match) {
        return outcome;
    }
    let snapshot = match body_ir_snapshot(CASE_20_SRC, "a pattern assertion to lower") {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    // Absence of a placeholder is not the property this row exists for. The binding has to survive as a declared
    // source binding, because the defect was a silently dropped one -- a body that lowered cleanly while describing
    // a read of something it never declared.
    if !snapshot.contains("is Some(bind(") {
        return ComparisonOutcome::Mismatch {
            detail: format!("the pattern assertion did not bind its payload:\n{snapshot}"),
        };
    }
    if !snapshot.contains("[binding]") {
        return ComparisonOutcome::Mismatch {
            detail: format!("the assertion's binding is not a declared source binding:\n{snapshot}"),
        };
    }
    ComparisonOutcome::Match
}

fn case_raises_assertion_reaches_body_ir() -> ComparisonOutcome {
    let outcome = outcome_from_body_ir(CASE_21_SRC, "a `raises` assertion to lower without a placeholder");
    if !matches!(outcome, ComparisonOutcome::Match) {
        return outcome;
    }
    let snapshot = match body_ir_snapshot(CASE_21_SRC, "a `raises` assertion to lower") {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    // The expected error type is part of the assertion, so it must be carried as a resolved fact rather than left
    // for a consumer to re-resolve from the source spelling. The optional message rides along with it.
    if !snapshot.contains("raises ValueError may_panic") {
        return ComparisonOutcome::Mismatch {
            detail: format!("a `raises` assertion lost its expected error type:\n{snapshot}"),
        };
    }
    if !snapshot.contains("raises IndexError, const(\"wanted an index error\") may_panic") {
        return ComparisonOutcome::Mismatch {
            detail: format!("a `raises` assertion lost its failure message:\n{snapshot}"),
        };
    }
    ComparisonOutcome::Match
}

// ============================================================================
// Case 17 — Body IR input contract: an inactive feature's body never lowers (#1166)
// ============================================================================

// #1166 made the input contract explicit: Body IR consumes a desugared, feature-projected program, and every caller
// owes it that. This row is the corpus holding *itself* to that contract — before #1166 both corpus entry points
// lowered raw parse output, so a divergence between the two pipelines would have gone green here rather than being
// surfaced.
//
// The row proves the feature-projection half, which is constructible in-process. The vocab half of the same
// contract is covered by unit tests instead: a genuinely vocab-authored body needs an import-activated library
// vocabulary with a WASM desugarer artifact, which no corpus row can stand up. Claiming this row proves both
// halves would be the kind of overstated evidence the corpus exists to prevent.
const CASE_17_SRC: &str = r#"
when feature("beta"):
    def gated() -> int:
        return 7

def main() -> int:
    return 1
"#;

fn case_inactive_feature_body_never_reaches_body_ir() -> ComparisonOutcome {
    let outcome = outcome_from_body_ir(
        CASE_17_SRC,
        "a body behind an inactive feature to be projected away before lowering",
    );
    if !matches!(outcome, ComparisonOutcome::Match) {
        return outcome;
    }
    // `outcome_from_body_ir` only proves no placeholder survived. The contract claim is stronger and needs its own
    // assertion: the gated function must be absent entirely, not lowered into something that merely looks clean.
    let tokens = match lexer::lex(CASE_17_SRC) {
        Ok(tokens) => tokens,
        Err(errors) => {
            return ComparisonOutcome::Incompatible {
                reason: format!("case 17 failed to lex: {errors:?}"),
            };
        }
    };
    let program = match parser::parse(&tokens) {
        Ok(program) => program,
        Err(errors) => {
            return ComparisonOutcome::Incompatible {
                reason: format!("case 17 failed to parse: {errors:?}"),
            };
        }
    };
    let program = match incan::frontend::body_ir::apply_body_ir_input_contract(
        program,
        std::path::Path::new("parity_987_body_ir.incn"),
    ) {
        Ok(program) => program,
        Err(errors) => {
            return ComparisonOutcome::Incompatible {
                reason: format!("case 17 input contract refused: {errors:?}"),
            };
        }
    };
    let module_path = vec!["parity_987_body_ir".to_string()];
    let mut checker = typechecker::TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    if let Err(errors) = checker.check_program(&program) {
        return ComparisonOutcome::Mismatch {
            detail: format!("case 17 typecheck reported {errors:?}"),
        };
    }
    let snapshot = build_body_ir_module_v0(&program, &module_path, checker.type_info()).render_snapshot();
    if snapshot.contains("gated") {
        return ComparisonOutcome::Mismatch {
            detail: format!("a body behind an inactive feature reached Body IR:\n{snapshot}"),
        };
    }
    ComparisonOutcome::Match
}

fn case_supported_call_spreads_reach_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_13_SRC,
        "positional, spread, named, and keyword-spread call arguments to lower together",
    )
}

// ============================================================================
// Cases 15 and 16 — Supported language contract: bytes literals and range values reach Body IR (#1165)
// ============================================================================

// A byte-string literal is ordinary accepted source with its own type, but `lower_literal` had no `bir::Constant`
// for it, so every `b"..."` reached a placeholder. The row is about representation, not bytes operations: those
// keep whatever refusal they already had.
const CASE_15_SRC: &str = r#"
def send(payload: bytes) -> int:
    return 1

def main() -> None:
    greeting = b"hi"
    println(send(greeting))
"#;

fn case_supported_bytes_literal_reaches_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_15_SRC,
        "a byte-string literal to lower to its own bytes constant rather than a placeholder",
    )
}

// A range is a value, not only a `for` header. `r = 0..10` has always typechecked, so refusing it in lowering left
// Body IR non-total over accepted programs; binding one and then iterating it exercises both halves.
const CASE_16_SRC: &str = r#"
def main() -> None:
    r = 0..10
    mut total = 0
    for i in r:
        total = total + i
    println(total)
"#;

fn case_supported_range_value_reaches_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_16_SRC,
        "a range bound to a local to lower to a real range value that the loop then iterates",
    )
}

// ============================================================================
// Cases 17 and 18 — Statement-position `loop:` reaches Body IR; `unsafe:` is a stated boundary (#1162)
// ============================================================================

// `bir::StatementKind::Loop` already existed and the expression spelling already emitted it, so the plain
// statement spelling -- the more common one -- was refused by a missing dispatch arm rather than by a missing
// representation. Included with `continue` and a nested loop, because the loop's break/continue vocabulary is
// what makes the row about the construct rather than about one keyword.
const CASE_18_SRC: &str = r#"
def grid(rows: int, cols: int) -> int:
    mut cells = 0
    mut r = 0
    loop:
        if r >= rows:
            break
        mut c = 0
        loop:
            if c >= cols:
                break
            c = c + 1
            if c % 2 == 0:
                continue
            cells = cells + 1
        r = r + 1
    return cells
"#;

fn case_supported_statement_loop_reaches_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_18_SRC,
        "a statement-position `loop:` to lower to a real Body IR loop, nesting and `continue` included",
    )
}

// The corpus's first `Disposition::Unsupported` row. This is a decided boundary, not pending lowering work: an
// `unsafe:` region introduces no Incan scope, so inlining its statements would be trivial -- and would erase the
// acknowledgement the region exists to record, letting a direct replacement execution profile run an explicitly
// authorized region without ever being told. The row asserts the refusal is present *and named*, so inlining the
// region later cannot leave it silently green.
const CASE_19_SRC: &str = r#"
def probe(x: int) -> int:
    return x

def touch(value: int) -> int:
    mut total = 0
    unsafe:
        total = probe(value)
    return total
"#;

fn case_unsafe_region_is_a_stated_refusal() -> ComparisonOutcome {
    outcome_from_body_ir_refusal(
        CASE_19_SRC,
        "unsupported(`unsafe:` acknowledgement region:",
        "an `unsafe:` region to refuse under a named, reasoned boundary rather than lower silently",
    )
}

// ============================================================================
// Case 6 — Diagnostic behavior: dead code after `return` warns (migrated from a silent accept)
// ============================================================================

// This row entered the corpus as bug-compatible behavior: statements after an unconditional `return` typechecked
// with zero diagnostics, so the gap was invisible unless a user read the generated Rust. #1117 migrated it
// deliberately — the typechecker now emits `INCAN-T0101` — so the case asserts the diagnostic contract instead of
// the old silence, and its disposition records the migration rather than freezing either behavior.
//
// The assertion reads the *stable code*, not message prose, so rewording the diagnostic does not silently break
// the corpus while a real change of contract still does.
const CASE_6_SRC: &str = r#"
def f() -> int:
    return 1
    println("dead code")
    return 2
"#;

fn case_diagnostic_unreachable_code_after_return() -> ComparisonOutcome {
    match typecheck_warnings(CASE_6_SRC) {
        Err(reason) => ComparisonOutcome::Incompatible { reason },
        Ok(warnings) => {
            if warnings
                .iter()
                .any(|warning| warning.stable_code() == Some("INCAN-T0101"))
            {
                ComparisonOutcome::Match
            } else {
                ComparisonOutcome::Mismatch {
                    detail: format!(
                        "expected an INCAN-T0101 unreachable-code warning, got warnings: {:?}",
                        warnings.iter().map(|warning| &warning.message).collect::<Vec<_>>()
                    ),
                }
            }
        }
    }
}

// ============================================================================
// #988 replacement execution corpus — stable receipt-bound source cases
// ============================================================================

const REPLACEMENT_BODY_V0_001_SRC: &str = r#"
def add(x: int, y: int) -> int:
    return x + y
"#;

const REPLACEMENT_BODY_V0_002_SRC: &str = r#"
def greet(name: str) -> str:
    return "hello, " + name
"#;

const REPLACEMENT_BODY_V0_003_SRC: &str = r#"
def return_owned() -> str:
    value = "owned"
    return value
"#;

const REPLACEMENT_BODY_V0_004_SRC: &str = r#"
def control_flow() -> int:
    for value in range(1, 5):
        if value % 2 == 0:
            continue
    while false:
        return 0
    return 10
"#;

const REPLACEMENT_BODY_V0_005_SRC: &str = r#"
def guarded_floor_div(a: int, b: int) -> int:
    assert b != 0
    return a // b
"#;

const REPLACEMENT_BODY_V0_006_SRC: &str = r#"
def select_second_pair() -> int:
    pairs = [(1, 2), (4, 5)]
    for a, b in pairs:
        if a == 4:
            return a * 10 + b
    return 0
"#;

const REPLACEMENT_BODY_V0_007_SRC: &str = r#"
def collect_lazy_values() -> int:
    values = (value * 10 for value in range(1, 5) if value > 2).collect()
    return values[0] + values[1]
"#;

const REPLACEMENT_BODY_V0_008_SRC: &str = r#"
def stored_closure() -> int:
    offset = 2
    add: (int) -> int = (value) => value + offset
    return add(40)
"#;

const REPLACEMENT_BODY_V0_009_SRC: &str = r#"
def route(method: int, path: int, content_type: int = 3) -> int:
    return method * 100 + path * 10 + content_type

def partial_defaults() -> int:
    method = 1
    get = partial route(method=method)
    normal = get(4)
    overridden = get(method=7, path=2, content_type=5)
    return normal + overridden
"#;

const REPLACEMENT_BODY_V0_010_SRC: &str = r#"
def counter() -> Generator[int]:
    for value in range(1, 3):
        yield value
    yield 3

def generator_function() -> int:
    values = counter().collect()
    return values[0] * 100 + values[1] * 10 + values[2]
"#;

const REPLACEMENT_BODY_V0_011_SRC: &str = r#"
def generator_adapters() -> int:
    offset = 1
    increment: (int) -> int = (value) => value + offset
    accepted: (int) -> bool = (value) => value > 2
    values = (value for value in range(1, 5)).map(increment).filter(accepted).collect()
    return values[0] * 10 + values[1]
"#;

const REPLACEMENT_BODY_V0_012_SRC: &str = r#"
def score(mut values: list[int]) -> int:
    values[0] = 40
    pair = (values[0], 2)
    return pair.0 + pair.1

def structural_values() -> int:
    values = [1, 2]
    return score(values)
"#;

const REPLACEMENT_BODY_V0_013_SRC: &str = r#"
model Pair:
    left: int
    right: int

def score(pair: Pair) -> int:
    return pair.left + pair.right

def nominal_values() -> int:
    pair = Pair(right=2, left=40)
    return score(pair)
"#;

const REPLACEMENT_BODY_V0_014_SRC: &str = r#"
enum HttpStatus(int):
    Ok = 200
    NotFound = 404

def status_code(status: HttpStatus) -> int:
    return status.value()

def value_enum_values() -> int:
    return status_code(HttpStatus.NotFound)
"#;

const REPLACEMENT_BODY_V0_015_SRC: &str = r#"
enum Signal:
    Ready
    Stop

def score(left: Signal, right: Signal) -> int:
    if left == Signal.Ready and right != Signal.Ready:
        return 42
    return 0

def fieldless_enum_values() -> int:
    return score(Signal.Ready, Signal.Stop)
"#;

const REPLACEMENT_BODY_V0_016_SRC: &str = r#"
model Pair:
    left: int
    right: int

enum Signal:
    Ready
    Stop

def classify(pair: Pair, signal: Signal) -> int:
    match pair:
        case Pair(left=40, right=2):
            match signal:
                case Signal.Ready:
                    return 42
                case Signal.Stop:
                    return 0
        case _:
            return 0
    return 0

def direct_patterns() -> int:
    return classify(Pair(left=40, right=2), Signal.Ready)
"#;

const REPLACEMENT_BODY_V0_017_SRC: &str = r#"
enum Failure:
    Odd

def half(value: int) -> Result[int, Failure]:
    if value % 2 != 0:
        return Err(Failure.Odd)
    return Ok(value // 2)

def quarter(value: int) -> Result[int, Failure]:
    half_value = half(value)?
    return half(half_value)

def direct_result_routing() -> int:
    match quarter(8):
        case Ok(value):
            return value
        case Err(_):
            return 0
    return 0
"#;

const REPLACEMENT_BODY_V0_018_SRC: &str = r#"
import std.async

async def answer() -> int:
    return 42

async def direct_async_await() -> int:
    return await answer()
"#;

const REPLACEMENT_BODY_V0_019_SRC: &str = r#"
import std.async

async def first() -> int:
    return 1

async def second() -> int:
    return 2

async def source_order_race() -> int:
    winner = race for value:
        await first() => value
        await second() => value
    return winner
"#;

// This stays a typed `str` result so the scalar conversion proof is independent of selected string-method work.
// The printed line is source-observable comparison evidence: it proves normal conversion output reaches both route
// receipts rather than treating a matching return value as a substitute for program-stream parity.
const REPLACEMENT_BODY_V0_022_SRC: &str = r#"
def scalar_conversions() -> str:
    parsed_int = int("42")
    parsed_float = float("3.14")
    widened_float = float(10)
    println(f"converted: {parsed_int} {parsed_float} {widened_float}")
    return f"{str(parsed_int)} {parsed_float} {widened_float}"
"#;

const REPLACEMENT_BODY_V0_023_SRC: &str = include_str!("fixtures/replacement/enumerate_zip.incn");

const REPLACEMENT_BODY_V0_029_SRC: &str = r#"
def typed_numeric_profile() -> f32:
    unsigned_min: u8 = 0
    unsigned_max: u8 = 255
    signed_min: i128 = -170141183460469231731687303715884105728
    wide_max: u128 = 340282366920938463463374607431768211455
    rounded: f32 = 1.23456789
    money: decimal[6, 2] = 19.90d
    println(f"{unsigned_min} {unsigned_max} {signed_min} {wide_max} {money}")
    return rounded
"#;

fn replacement_body_v0_001_arguments() -> Vec<ReplacementValue> {
    vec![ReplacementValue::Int(40), ReplacementValue::Int(2)]
}

fn replacement_body_v0_001_expected() -> ReplacementValue {
    ReplacementValue::Int(42)
}

fn replacement_body_v0_002_arguments() -> Vec<ReplacementValue> {
    vec![ReplacementValue::Str("Ada".to_string())]
}

fn replacement_body_v0_002_expected() -> ReplacementValue {
    ReplacementValue::Str("hello, Ada".to_string())
}

fn replacement_body_v0_003_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_003_expected() -> ReplacementValue {
    ReplacementValue::Str("owned".to_string())
}

fn replacement_body_v0_004_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_004_expected() -> ReplacementValue {
    ReplacementValue::Int(10)
}

fn replacement_body_v0_005_arguments() -> Vec<ReplacementValue> {
    vec![ReplacementValue::Int(84), ReplacementValue::Int(2)]
}

fn replacement_body_v0_005_expected() -> ReplacementValue {
    ReplacementValue::Int(42)
}

fn replacement_body_v0_006_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_006_expected() -> ReplacementValue {
    ReplacementValue::Int(45)
}

fn replacement_body_v0_007_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_007_expected() -> ReplacementValue {
    ReplacementValue::Int(70)
}

fn replacement_body_v0_008_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_008_expected() -> ReplacementValue {
    ReplacementValue::Int(42)
}

fn replacement_body_v0_009_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_009_expected() -> ReplacementValue {
    ReplacementValue::Int(868)
}

fn replacement_body_v0_010_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_010_expected() -> ReplacementValue {
    ReplacementValue::Int(123)
}

fn replacement_body_v0_011_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_011_expected() -> ReplacementValue {
    ReplacementValue::Int(34)
}

fn replacement_body_v0_012_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_012_expected() -> ReplacementValue {
    ReplacementValue::Int(42)
}

fn replacement_body_v0_013_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_013_expected() -> ReplacementValue {
    ReplacementValue::Int(42)
}

fn replacement_body_v0_014_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_014_expected() -> ReplacementValue {
    ReplacementValue::Int(404)
}

fn replacement_body_v0_015_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_015_expected() -> ReplacementValue {
    ReplacementValue::Int(42)
}

fn replacement_body_v0_016_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_016_expected() -> ReplacementValue {
    ReplacementValue::Int(42)
}

fn replacement_body_v0_017_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_017_expected() -> ReplacementValue {
    ReplacementValue::Int(2)
}

fn replacement_body_v0_018_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_018_expected() -> ReplacementValue {
    ReplacementValue::Int(42)
}

fn replacement_body_v0_019_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_019_expected() -> ReplacementValue {
    ReplacementValue::Int(1)
}

fn replacement_body_v0_025_expected() -> ReplacementValue {
    ReplacementValue::Str(JSON_STRINGIFY_SCALARS_EXPECTED.to_string())
}

fn replacement_body_v0_022_arguments() -> Vec<ReplacementValue> {
    vec![]
}

fn replacement_body_v0_022_expected() -> ReplacementValue {
    ReplacementValue::Str("42 3.14 10".to_string())
}

/// The selected list-iteration fixture has no entry arguments.
fn replacement_body_v0_023_arguments() -> Vec<ReplacementValue> {
    vec![]
}

/// Stored enumeration contributes ten and Zip contributes thirty-nine.
fn replacement_body_v0_023_expected() -> ReplacementValue {
    ReplacementValue::Int(49)
}

fn replacement_body_v0_029_expected() -> ReplacementValue {
    ReplacementValue::Numeric(ReplacementNumericValue::F32(1.234_567_9_f32))
}

// ============================================================================
// Case 7 — Diagnostic behavior: statement tuple unpack of a non-tuple (migrated from a silent accept)
// ============================================================================

// Entered the corpus as a silent accept: `a, b = 5` typechecked clean, bound both names `Unknown`, and only
// failed while compiling the emitted Rust with an `E0610` naming a `__incan_tuple_unpack_*` binding the user never
// wrote. #1132 migrated it to a source-language decision. Asserted through the message rather than a stable code
// because this family reports under the broad `INCAN-T0001` typecheck code.
const CASE_7_SRC: &str = r#"
def main() -> None:
    a, b = 5
    println(f"{a} {b}")
"#;

fn case_diagnostic_statement_tuple_unpack_of_non_tuple() -> ComparisonOutcome {
    outcome_from_typecheck(
        CASE_7_SRC,
        |errs| {
            errs.iter()
                .any(|error| error.contains("Cannot destructure 2 values from value of type 'int'"))
        },
        "a typechecker diagnostic naming the non-tuple value type",
    )
}

// ============================================================================
// Provider-operation paths (#1156)
// ============================================================================

// The five rows below give the #1156 vertical's paths their own stable #987 dispositions. Each probe drives the
// real pipeline -- source, typecheck, Body-IR lowering with a fixture-controlled provider catalog, direct
// replacement execution -- against a fixture ledger host and a `StaticAuthority`, and asserts what that path is
// contracted to do.
//
// None of them can be comparison-green, and none claims to be. The legacy backend cannot execute a provider
// operation at all, so there is no second route to compare against until #1146 supplies the receipt-bound paired
// comparison; each probe asserts that the execution it observed declared that non-green state explicitly rather
// than leaving it implied.
//
// `ReplacementExecutionPlan` -- the corpus's direct execution shape -- names a function and concrete arguments,
// with nowhere to name the authority source and provider host a provider operation needs. These callbacks therefore
// inspect the real provider receipts internally, while the outer corpus row records only a `BehaviorObserved`
// evidence identity over the callback outcome. It does not fabricate a legacy receipt or borrow the provider's
// nested receipt as if it authorized the whole row. A corpus-visible provider execution receipt belongs with
// #1146's explicit comparison route rather than with this vertical.

/// One ledger charge, plus a same-module caller that invokes it.
///
/// `charge`'s own body returns a different value than the provider host does, so a run that executed the local
/// declaration instead of the provider would be visible in the observable rather than silent.
const PROVIDER_CASE_SRC: &str = r#"
capability ledger_charge:
  description = "Charge one approved ledger account"

@provider_operation(ledger_charge)
def charge(account: str, amount: int) -> int:
  return amount

def settle(account: str, amount: int) -> int:
  return charge(account, amount)
"#;

/// What the fixture ledger does when an authorized charge reaches it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LedgerBehavior {
    /// Settle the charge, adding a fixed fee so the result cannot be confused with the local declaration's.
    Settle,
    /// Settle the charge but withhold the account identifier from the receipt.
    SettleWithSecretAccount,
    /// Refuse the charge after authority was already granted.
    Decline,
}

/// A fixture ledger provider, addressed only by the canonical identity of the operation it owns.
///
/// Keying on the identity rather than on a name is the contract, not a detail: a host that matched a provider
/// module name, a call-site spelling, or an emitted Rust name would be the source-meaning duplication this vertical
/// exists to avoid.
struct CorpusLedgerHost {
    operation: CanonicalSymbolId,
    behavior: LedgerBehavior,
    invocations: RefCell<Vec<i64>>,
    releases: Cell<usize>,
}

impl CorpusLedgerHost {
    /// Build a host that executes exactly `operation` and behaves as `behavior` when it is invoked.
    fn new(operation: CanonicalSymbolId, behavior: LedgerBehavior) -> Self {
        Self {
            operation,
            behavior,
            invocations: RefCell::new(Vec::new()),
            releases: Cell::new(0),
        }
    }

    /// The integer amount carried by the input at written position 1, or an error naming what arrived instead.
    fn amount(inputs: &[ProviderInputValue]) -> Result<i64, String> {
        match inputs.iter().find(|input| input.written_position == 1) {
            Some(ProviderInputValue {
                value: ReplacementValue::Int(amount),
                ..
            }) => Ok(*amount),
            other => Err(format!("a charge needs an integer amount, got {other:?}")),
        }
    }
}

impl ProviderOperationHost for CorpusLedgerHost {
    fn operation_kind(&self, operation: &CanonicalSymbolId) -> Option<String> {
        (operation == &self.operation).then(|| "ledger.charge".to_string())
    }

    fn invoke(&self, invocation: &ProviderInvocation<'_, '_>) -> ProviderOperationOutcome {
        let amount = match CorpusLedgerHost::amount(invocation.inputs) {
            Ok(amount) => amount,
            Err(detail) => {
                return ProviderOperationOutcome::Failed {
                    detail,
                    attributes: Vec::new(),
                    replay: ReplayClassification::Unavailable,
                };
            }
        };
        self.invocations.borrow_mut().push(amount);
        match self.behavior {
            LedgerBehavior::Settle => ProviderOperationOutcome::Completed {
                value: ReplacementValue::Int(amount + 5),
                attributes: vec![ReceiptAttribute::public("ledger.amount", amount.to_string())],
                replay: ReplayClassification::FixtureRequired,
            },
            LedgerBehavior::SettleWithSecretAccount => ProviderOperationOutcome::Completed {
                value: ReplacementValue::Int(amount + 5),
                attributes: vec![
                    ReceiptAttribute::public("ledger.amount", amount.to_string()),
                    ReceiptAttribute::redacted("ledger.account", AttributeSensitivity::Secret),
                ],
                replay: ReplayClassification::FixtureRequired,
            },
            LedgerBehavior::Decline => ProviderOperationOutcome::Failed {
                detail: format!("the ledger declined a charge of {amount}"),
                attributes: vec![ReceiptAttribute::public("ledger.amount", amount.to_string())],
                replay: ReplayClassification::FixtureRequired,
            },
        }
    }

    fn release(&self, _operation: &CanonicalSymbolId, _call_span: HirSourceSpan) {
        self.releases.set(self.releases.get() + 1);
    }
}

/// Everything one provider path produced, in the shape the probes assert on.
struct ProviderPathObservation {
    /// The source-level value the execution produced, when it produced one.
    value: Option<ReplacementValue>,
    /// The stable diagnostic code the execution refused with, when it refused.
    error_code: Option<&'static str>,
    /// The status of the single RFC 104 operation receipt the run emitted, when it emitted one.
    receipt_status: Option<ReceiptStatus>,
    /// The keys whose values that receipt withheld.
    redacted_keys: Vec<String>,
    /// Whether the receipt's own authority decision allowed the operation.
    authority_allowed: Option<bool>,
    /// The amounts the ledger was actually asked to charge.
    invocations: Vec<i64>,
    /// How many settlement handles the ledger released.
    releases: usize,
    /// The lifecycle transitions the run recorded, in order.
    lifecycle: Vec<&'static str>,
    /// The backend execution receipts the run finalized, as `(outcome, referenced receipt sequence id)`.
    backend_executions: Vec<(&'static str, u64)>,
    /// Every comparison state those backend receipts declared.
    comparison_reasons: Vec<String>,
}

/// Lower the ledger fixture, run `settle("acct-1", 250)` against a fixture host, and report what happened.
///
/// The catalog key is the operation's canonical identity, minted the way lowering mints it. Nothing tells the call
/// site anything: admission travels entirely through that identity.
fn observe_provider_path(
    behavior: LedgerBehavior,
    mode: AuthorityMode,
    grant_capability: bool,
) -> Result<ProviderPathObservation, String> {
    let tokens = lexer::lex(PROVIDER_CASE_SRC).map_err(|errors| format!("provider fixture lex failure: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("provider fixture parse failure: {errors:?}"))?;
    let module_path = vec!["app".to_string()];
    let mut checker = typechecker::TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("provider fixture typecheck failure: {errors:?}"))?;

    // Admission is projected from a published provider manifest through a selected `ProviderPlan`, never
    // hand-filled into the lowering catalogue -- which #1213 made private precisely so a consumer cannot invent
    // admission a real producer could not have published.
    let descriptors: Vec<ProviderOperationMetadata> = checker
        .type_info()
        .declarations
        .provider_operations
        .values()
        .map(|declared| ProviderOperationMetadata {
            operation: declared.operation.clone(),
            required_capability: declared.required_capability.clone(),
            runtime_requirements: declared.runtime_requirements.clone(),
        })
        .collect();
    let descriptor = descriptors
        .first()
        .ok_or("the provider fixture declares no checked provider operation")?;
    let operation = descriptor.operation.clone();
    let required_capability = descriptor.required_capability.clone();
    let namespace_claims: BTreeSet<Vec<String>> = descriptors
        .iter()
        .filter_map(|descriptor| descriptor.operation.module_path().map(ToOwned::to_owned))
        .collect();

    let mut manifest = LibraryManifest::new("corpus_provider", "0.1.0");
    manifest.contract_metadata.provider = CompiledProviderMetadata {
        operation_descriptors: descriptors,
        ..CompiledProviderMetadata::default()
    };
    let provider_plan = ProviderPlan::new(
        LibraryManifestIndex::default(),
        vec![ProviderRecord {
            identity: ProviderIdentity {
                name: "corpus_provider".to_string(),
                version: "0.1.0".to_string(),
                digest: "fixture:corpus-provider".to_string(),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::Compiler,
            authority: NamespaceAuthority::Compiler,
            namespace_claims: namespace_claims.clone(),
            available: true,
            enabled: true,
            manifest: Some(Arc::new(manifest)),
            artifact: None,
            implementation_facets: Vec::new(),
        }],
        namespace_claims,
    )
    .map_err(|error| error.to_string())?;
    let module =
        build_body_ir_module_v0_with_provider_plan(&program, &module_path, checker.type_info(), &provider_plan)
            .map_err(|error| format!("provider fixture lowering failure: {error}"))?;

    let host = Rc::new(CorpusLedgerHost::new(operation, behavior));
    let authority = StaticAuthority::new(mode, grant_capability.then_some(required_capability));
    let providers = ProviderRuntime::new(Rc::new(authority), host.clone());
    let executed = execute_free_function_with_providers(
        &module,
        "settle",
        &[ReplacementValue::Str("acct-1".to_string()), ReplacementValue::Int(250)],
        &providers,
    );

    let receipts = providers.operation_receipts();
    let receipt = receipts.first();
    // A receipt that contradicts its own fields would make every other assertion here meaningless, so the
    // contract check runs before anything is reported.
    if let Some(receipt) = receipt
        && let Err(violation) = receipt.validate()
    {
        return Err(format!("the emitted operation receipt contradicts itself: {violation}"));
    }
    let executions = providers.provider_executions();
    Ok(ProviderPathObservation {
        value: executed.as_ref().ok().map(|execution| execution.value.clone()),
        error_code: executed.as_ref().err().map(ReplacementExecutionError::diagnostic_code),
        receipt_status: receipt.map(|receipt| receipt.status()),
        redacted_keys: receipt.map(|receipt| receipt.redacted_keys()).unwrap_or_default(),
        authority_allowed: receipt.map(|receipt| receipt.authority().is_allowed()),
        invocations: host.invocations.borrow().clone(),
        releases: host.releases.get(),
        lifecycle: providers
            .lifecycle_evidence()
            .into_iter()
            .map(|event| event.event)
            .collect(),
        backend_executions: executions
            .iter()
            .map(|record| {
                let projection = record.projection();
                (projection.outcome, projection.operation_receipt_sequence_id)
            })
            .collect(),
        comparison_reasons: executions
            .iter()
            .map(|record| record.projection().comparison_reason)
            .collect(),
    })
}

/// Confirm that every backend execution the observation recorded declared an explicitly non-green comparison.
fn provider_comparison_is_explicitly_non_green(observation: &ProviderPathObservation) -> Option<String> {
    if observation.comparison_reasons.is_empty() {
        return Some("a provider path recorded no backend execution receipt at all".to_string());
    }
    observation
        .comparison_reasons
        .iter()
        .find(|reason| reason.as_str() != PROVIDER_COMPARISON_UNAVAILABLE_REASON)
        .map(|reason| format!("a provider execution claimed a comparison state it cannot support: {reason}"))
}

/// Turn a provider path observation plus a claim about it into a corpus outcome, without panicking.
fn provider_outcome(
    observation: Result<ProviderPathObservation, String>,
    claim: impl FnOnce(&ProviderPathObservation) -> Option<String>,
) -> ComparisonOutcome {
    let observation = match observation {
        Ok(observation) => observation,
        Err(reason) => return ComparisonOutcome::Incompatible { reason },
    };
    match provider_comparison_is_explicitly_non_green(&observation).or_else(|| claim(&observation)) {
        Some(detail) => ComparisonOutcome::Mismatch { detail },
        None => ComparisonOutcome::Match,
    }
}

/// An allowed invocation runs the provider and binds a backend receipt to the operation receipt it describes.
fn case_provider_allowed_invocation() -> ComparisonOutcome {
    provider_outcome(
        observe_provider_path(LedgerBehavior::Settle, AuthorityMode::Governed, true),
        |observed| {
            if observed.value != Some(ReplacementValue::Int(255)) {
                return Some(format!(
                    "an allowed charge must produce the provider's settled value, got {:?}",
                    observed.value
                ));
            }
            if observed.receipt_status != Some(ReceiptStatus::Allowed) {
                return Some(format!(
                    "expected an allowed receipt, got {:?}",
                    observed.receipt_status
                ));
            }
            if observed.backend_executions != vec![("allowed", 0)] {
                return Some(format!(
                    "the backend receipt must reference the operation receipt it describes, got {:?}",
                    observed.backend_executions
                ));
            }
            None
        },
    )
}

/// A governed denial emits a denied receipt, reports a source-owned diagnostic, and never reaches the provider.
fn case_provider_governed_denial() -> ComparisonOutcome {
    provider_outcome(
        observe_provider_path(LedgerBehavior::Settle, AuthorityMode::Governed, false),
        |observed| {
            if !observed.invocations.is_empty() {
                return Some(format!(
                    "a denied operation must never reach the provider, but it was invoked with {:?}",
                    observed.invocations
                ));
            }
            if observed.error_code != Some("INCAN-R1156-DENIED") {
                return Some(format!(
                    "a denial must report its own source-owned diagnostic, got {:?}",
                    observed.error_code
                ));
            }
            if observed.receipt_status != Some(ReceiptStatus::Denied) || observed.authority_allowed != Some(false) {
                return Some(format!(
                    "a denial is a recorded outcome over a refusing decision, got {:?}/{:?}",
                    observed.receipt_status, observed.authority_allowed
                ));
            }
            if observed.lifecycle != vec!["denied"] {
                return Some(format!(
                    "a denial acquires nothing, so it has nothing to release: {:?}",
                    observed.lifecycle
                ));
            }
            None
        },
    )
}

/// A provider failure keeps its allowing authority decision and reports its own diagnostic, not a denial's.
fn case_provider_operation_failure() -> ComparisonOutcome {
    provider_outcome(
        observe_provider_path(LedgerBehavior::Decline, AuthorityMode::Governed, true),
        |observed| {
            if observed.error_code != Some("INCAN-R1156-PROVIDER") {
                return Some(format!(
                    "a provider failure is not a denial and reports its own code, got {:?}",
                    observed.error_code
                ));
            }
            if observed.receipt_status != Some(ReceiptStatus::Failed) || observed.authority_allowed != Some(true) {
                return Some(format!(
                    "a failure keeps its allowing authority decision, got {:?}/{:?}",
                    observed.receipt_status, observed.authority_allowed
                ));
            }
            if observed.invocations != vec![250] {
                return Some(format!(
                    "a failure happens after the provider was reached, got {:?}",
                    observed.invocations
                ));
            }
            None
        },
    )
}

/// A withheld attribute classifies the receipt as redacted without changing what the operation returned.
fn case_provider_redaction_classification() -> ComparisonOutcome {
    provider_outcome(
        observe_provider_path(LedgerBehavior::SettleWithSecretAccount, AuthorityMode::Governed, true),
        |observed| {
            if observed.receipt_status != Some(ReceiptStatus::Redacted) {
                return Some(format!(
                    "a receipt with a withheld value must stop claiming it recorded everything, got {:?}",
                    observed.receipt_status
                ));
            }
            if observed.redacted_keys != vec!["ledger.account".to_string()] {
                return Some(format!(
                    "a redacted attribute keeps its key, got {:?}",
                    observed.redacted_keys
                ));
            }
            if observed.value != Some(ReplacementValue::Int(255)) {
                return Some(format!(
                    "redaction changes what is recorded, not what the operation returned, got {:?}",
                    observed.value
                ));
            }
            if observed
                .backend_executions
                .iter()
                .any(|(outcome, _)| *outcome != "redacted")
            {
                return Some(format!(
                    "the backend receipt must record the classification, got {:?}",
                    observed.backend_executions
                ));
            }
            None
        },
    )
}

/// An invocation that failed still releases what it acquired, exactly once and after the failure.
fn case_provider_lifecycle_cleanup() -> ComparisonOutcome {
    provider_outcome(
        observe_provider_path(LedgerBehavior::Decline, AuthorityMode::Governed, true),
        |observed| {
            if observed.lifecycle != vec!["invoked", "failed", "released"] {
                return Some(format!(
                    "cleanup follows the outcome it cleans up after and never precedes the invocation: {:?}",
                    observed.lifecycle
                ));
            }
            if observed.releases != 1 {
                return Some(format!(
                    "an invocation that failed still releases what it acquired, exactly once; got {}",
                    observed.releases
                ));
            }
            None
        },
    )
}

// ============================================================================
// RFC 120 identity-conformance corpus
// ============================================================================

const IDENTITY_PROVIDER_SRC: &str = r#"
pub def imported_lexical(value: int) -> int:
    return value

pub def aliased_lexical(value: int) -> int:
    return value

pub def relayed_lexical(value: int) -> int:
    return value

pub model ImportedMember:
    pub value: int

    def read(self) -> int:
        return self.value

pub model AliasedMember:
    pub value: int

    def read(self) -> int:
        return self.value

pub model RelayedMember:
    pub value: int

    def read(self) -> int:
        return self.value

pub def imported_path(value: int) -> int:
    return value

pub def aliased_path(value: int) -> int:
    return value

"#;

const IDENTITY_FACADE_SRC: &str = r#"
pub from identity_provider import RelayedMember, relayed_lexical
"#;

const IDENTITY_MATRIX_SRC: &str = r#"
from identity_provider import ImportedMember, imported_lexical
from identity_provider import AliasedMember as MemberAlias, aliased_lexical as lexical_alias
from identity_facade import RelayedMember as MemberReexport, relayed_lexical as lexical_reexport
import identity_provider
import identity_provider as provider_alias

def local_lexical(value: int) -> int:
    return value

model LocalMember:
    value: int

    def read(self) -> int:
        return self.value

def local_lexical_function_scope() -> int:
    return local_lexical(1)

def local_lexical_block_scope() -> int:
    if true:
        return local_lexical(1)
    return 0

def imported_lexical_function_scope() -> int:
    return imported_lexical(3)

def imported_lexical_block_scope() -> int:
    if true:
        return imported_lexical(3)
    return 0

def aliased_lexical_function_scope() -> int:
    return lexical_alias(5)

def aliased_lexical_block_scope() -> int:
    if true:
        return lexical_alias(5)
    return 0

def reexported_lexical_function_scope() -> int:
    return lexical_reexport(7)

def reexported_lexical_block_scope() -> int:
    if true:
        return lexical_reexport(7)
    return 0

def local_member_function_scope() -> int:
    local = LocalMember(value=9)
    return local.read()

def local_member_block_scope() -> int:
    local = LocalMember(value=10)
    if true:
        return local.read()
    return 0

def imported_member_function_scope() -> int:
    imported = ImportedMember(value=11)
    return imported.read()

def imported_member_block_scope() -> int:
    imported = ImportedMember(value=12)
    if true:
        return imported.read()
    return 0

def aliased_member_function_scope() -> int:
    aliased = MemberAlias(value=13)
    return aliased.read()

def aliased_member_block_scope() -> int:
    aliased = MemberAlias(value=14)
    if true:
        return aliased.read()
    return 0

def reexported_member_function_scope() -> int:
    relayed = MemberReexport(value=15)
    return relayed.read()

def reexported_member_block_scope() -> int:
    relayed = MemberReexport(value=16)
    if true:
        return relayed.read()
    return 0

def imported_path_function_scope() -> int:
    return identity_provider.imported_path(17)

def imported_path_block_scope() -> int:
    if true:
        return identity_provider.imported_path(17)
    return 0

def aliased_path_function_scope() -> int:
    return provider_alias.aliased_path(19)

def aliased_path_block_scope() -> int:
    if true:
        return provider_alias.aliased_path(19)
    return 0

"#;

/// Every matrix cell the replacement route executes across the module boundary, with the value it must return.
///
/// The returned values are deliberately distinct so a call that reached the wrong declaration is a wrong number
/// rather than a coincidence: were `lexical_alias` resolved to `imported_lexical`, this row would return 3 where 5
/// is required, and both are real declarations that execute.
///
/// Member cells are absent on purpose and are declared in [`IDENTITY_MATRIX_DEFERRED`] instead.
const IDENTITY_MATRIX_ENTRYPOINTS: &[IdentityGraphEntrypoint] = &[
    IdentityGraphEntrypoint {
        function: "local_lexical_function_scope",
        expected: 1,
    },
    IdentityGraphEntrypoint {
        function: "local_lexical_block_scope",
        expected: 1,
    },
    IdentityGraphEntrypoint {
        function: "imported_lexical_function_scope",
        expected: 3,
    },
    IdentityGraphEntrypoint {
        function: "imported_lexical_block_scope",
        expected: 3,
    },
    IdentityGraphEntrypoint {
        function: "aliased_lexical_function_scope",
        expected: 5,
    },
    IdentityGraphEntrypoint {
        function: "aliased_lexical_block_scope",
        expected: 5,
    },
    IdentityGraphEntrypoint {
        function: "reexported_lexical_function_scope",
        expected: 7,
    },
    IdentityGraphEntrypoint {
        function: "reexported_lexical_block_scope",
        expected: 7,
    },
    IdentityGraphEntrypoint {
        function: "imported_path_function_scope",
        expected: 17,
    },
    IdentityGraphEntrypoint {
        function: "imported_path_block_scope",
        expected: 17,
    },
    IdentityGraphEntrypoint {
        function: "aliased_path_function_scope",
        expected: 19,
    },
    IdentityGraphEntrypoint {
        function: "aliased_path_block_scope",
        expected: 19,
    },
];

/// Matrix cells the replacement route refuses today, each bound to the issue that owns closing the gap.
///
/// Every member cell in the matrix reads a field through a model method, and the direct profile retains a model
/// declaration only when it has no methods (`is_direct_replacement_plain_model`), so the constructor is refused
/// before the method is ever reached. That is a language-matrix gap owned by #1291, not an import one: the same
/// refusal occurs for `LocalMember`, which crosses no module boundary at all. #1260 and #1261 supply what these
/// cells still need after that lands -- the imported, aliased, and re-exported identities they construct through.
///
/// The runner requires each of these to actually be refused. A deferral that quietly starts working fails this row
/// rather than passing unnoticed, so the day #1291 lands, someone has to come back and promote these cells.
const IDENTITY_MATRIX_DEFERRED: &[IdentityGraphDeferral] = &[
    IdentityGraphDeferral {
        function: "local_member_function_scope",
        owning_issue: 1291,
    },
    IdentityGraphDeferral {
        function: "local_member_block_scope",
        owning_issue: 1291,
    },
    IdentityGraphDeferral {
        function: "imported_member_function_scope",
        owning_issue: 1291,
    },
    IdentityGraphDeferral {
        function: "imported_member_block_scope",
        owning_issue: 1291,
    },
    IdentityGraphDeferral {
        function: "aliased_member_function_scope",
        owning_issue: 1291,
    },
    IdentityGraphDeferral {
        function: "aliased_member_block_scope",
        owning_issue: 1291,
    },
    IdentityGraphDeferral {
        function: "reexported_member_function_scope",
        owning_issue: 1291,
    },
    IdentityGraphDeferral {
        function: "reexported_member_block_scope",
        owning_issue: 1291,
    },
];

const IDENTITY_MATRIX_MODULES: &[IdentitySourceModule] = &[
    IdentitySourceModule {
        name: "identity_provider",
        path: &["identity_provider"],
        source: IDENTITY_PROVIDER_SRC,
        dependencies: &[],
    },
    IdentitySourceModule {
        name: "identity_facade",
        path: &["identity_facade"],
        source: IDENTITY_FACADE_SRC,
        dependencies: &["identity_provider"],
    },
    IdentitySourceModule {
        name: "identity_matrix",
        path: &["identity_matrix"],
        source: IDENTITY_MATRIX_SRC,
        dependencies: &["identity_provider", "identity_facade"],
    },
];

struct LexicalIdentityRow<'a> {
    label: &'a str,
    binding: IdentityBindingForm,
    target_module: &'a str,
    target_name: &'a str,
    root_binding: &'a str,
    call: &'a str,
    function_body: &'a str,
    block_body: &'a str,
}

#[derive(Clone, Copy)]
struct DeclarationSpanSelector<'a> {
    anchor: &'a str,
    occurrence: usize,
}

struct MemberIdentityRow<'a> {
    label: &'a str,
    binding: IdentityBindingForm,
    target_module: &'a str,
    owner_name: &'a str,
    owner: DeclarationSpanSelector<'a>,
    member: DeclarationSpanSelector<'a>,
    root_binding: &'a str,
    receiver_call: &'a str,
    function_body: &'a str,
    block_body: &'a str,
}

struct PathIdentityRow<'a> {
    label: &'a str,
    binding: IdentityBindingForm,
    expected_module_path: &'a [&'a str],
    expected_module_name: &'a str,
    target_name: &'a str,
    module_binding: &'a str,
    call: &'a str,
    function_body: &'a str,
    block_body: &'a str,
}

fn require_same_identity(
    label: &str,
    expected: &CanonicalSymbolId,
    actual: &CanonicalSymbolId,
    evidence: &mut Vec<String>,
) -> Result<(), String> {
    if actual != expected {
        return Err(format!(
            "{label} reconstructed or selected the wrong identity: expected {}, got {}",
            expected.render_compact(),
            actual.render_compact()
        ));
    }
    evidence.push(format!("{label}: {}", actual.render_compact()));
    Ok(())
}

fn require_body_consumer(
    graph: &CheckedIdentityGraph,
    body: &str,
    expected: &CanonicalSymbolId,
    evidence: &mut Vec<String>,
    label: &str,
) -> Result<(), String> {
    let identities = graph.body_consumer_identities("identity_matrix", body)?;
    if !identities.iter().any(|identity| identity == expected) {
        return Err(format!(
            "{label} Body IR lost {}, retaining {identities:?}",
            expected.render_compact()
        ));
    }
    evidence.push(format!("{label}: {}", expected.render_compact()));
    Ok(())
}

fn verify_lexical_matrix_row(
    graph: &CheckedIdentityGraph,
    row: &LexicalIdentityRow<'_>,
    assertions: &mut IdentityAssertions,
) -> Result<(), String> {
    let target = graph.declaration_identity(
        row.target_module,
        row.target_name,
        SemanticSourceTargetKind::Function,
        SymbolNamespace::OrdinaryLexical,
    )?;
    let module_binding = graph.hir_identity("identity_matrix", row.root_binding)?;
    require_same_identity(
        &format!("{} lexical/module", row.label),
        &target,
        &module_binding,
        &mut assertions.checked_relations,
    )?;
    assertions
        .hir_consumers
        .push(format!("{} lexical/module HIR: {}", row.label, target.render_compact()));

    let function_reference = graph.resolved_identity("identity_matrix", row.call, 0)?;
    require_same_identity(
        &format!("{} lexical/function", row.label),
        &target,
        &function_reference,
        &mut assertions.checked_relations,
    )?;
    require_body_consumer(
        graph,
        row.function_body,
        &target,
        &mut assertions.body_ir_consumers,
        &format!("{} lexical/function", row.label),
    )?;

    let block_reference = graph.resolved_identity("identity_matrix", row.call, 1)?;
    require_same_identity(
        &format!("{} lexical/block", row.label),
        &target,
        &block_reference,
        &mut assertions.checked_relations,
    )?;
    require_body_consumer(
        graph,
        row.block_body,
        &target,
        &mut assertions.body_ir_consumers,
        &format!("{} lexical/block", row.label),
    )?;
    let target_projection = graph.require_emitted_projection(row.target_module, &target)?;
    let root_projection = graph.require_emitted_projection("identity_matrix", &target)?;
    let projection_evidence = format!("{target_projection}; {root_projection}");
    assertions.legacy_projections.push(target_projection);
    assertions.legacy_projections.push(root_projection);
    assertions.coverage_cells.extend([
        IdentityCoverageCell {
            binding: row.binding,
            namespace: IdentityNamespace::Lexical,
            scope: IdentityScope::Module,
            checked_identity: target.render_compact(),
            hir_identity: Some(module_binding.render_compact()),
            body_ir_identity: None,
            emitted_projection: Some(projection_evidence.clone()),
        },
        IdentityCoverageCell {
            binding: row.binding,
            namespace: IdentityNamespace::Lexical,
            scope: IdentityScope::Function,
            checked_identity: function_reference.render_compact(),
            hir_identity: None,
            body_ir_identity: Some(target.render_compact()),
            emitted_projection: Some(projection_evidence.clone()),
        },
        IdentityCoverageCell {
            binding: row.binding,
            namespace: IdentityNamespace::Lexical,
            scope: IdentityScope::Block,
            checked_identity: block_reference.render_compact(),
            hir_identity: None,
            body_ir_identity: Some(target.render_compact()),
            emitted_projection: Some(projection_evidence),
        },
    ]);
    Ok(())
}

fn verify_member_matrix_row(
    graph: &CheckedIdentityGraph,
    row: &MemberIdentityRow<'_>,
    assertions: &mut IdentityAssertions,
) -> Result<(), String> {
    let owner = graph.declaration_identity_at_source_anchor(
        row.target_module,
        row.owner.anchor,
        row.owner.occurrence,
        SemanticSourceTargetKind::Model,
        SymbolNamespace::OrdinaryLexical,
    )?;
    if owner.declaration_name != row.owner_name {
        return Err(format!(
            "{} owner span selected `{}` instead of `{}`",
            row.label, owner.declaration_name, row.owner_name
        ));
    }
    let declared_member = graph.declaration_identity_at_source_anchor(
        row.target_module,
        row.member.anchor,
        row.member.occurrence,
        SemanticSourceTargetKind::Method,
        SymbolNamespace::Member,
    )?;
    if declared_member.declaration_name != "read"
        || declared_member.kind != SemanticSourceTargetKind::Method
        || declared_member.namespace != SymbolNamespace::Member
    {
        return Err(format!(
            "{} member declaration selected a non-method identity: {}",
            row.label,
            declared_member.render_compact()
        ));
    }
    if declared_member.declaration_span.start < owner.declaration_span.start
        || declared_member.declaration_span.end > owner.declaration_span.end
    {
        return Err(format!(
            "{} member declaration {} is outside owner {}",
            row.label,
            declared_member.render_compact(),
            owner.render_compact()
        ));
    }
    let module_binding = graph.hir_identity("identity_matrix", row.root_binding)?;
    require_same_identity(
        &format!("{} member/module owner", row.label),
        &owner,
        &module_binding,
        &mut assertions.checked_relations,
    )?;
    assertions.hir_consumers.push(format!(
        "{} member/module HIR owner={} declared-member={}",
        row.label,
        owner.render_compact(),
        declared_member.render_compact()
    ));

    let function_reference = graph.resolved_identity("identity_matrix", row.receiver_call, 0)?;
    require_same_identity(
        &format!("{} member/function", row.label),
        &declared_member,
        &function_reference,
        &mut assertions.checked_relations,
    )?;
    require_body_consumer(
        graph,
        row.function_body,
        &declared_member,
        &mut assertions.body_ir_consumers,
        &format!("{} member/function", row.label),
    )?;

    let block_reference = graph.resolved_identity("identity_matrix", row.receiver_call, 1)?;
    require_same_identity(
        &format!("{} member/block", row.label),
        &declared_member,
        &block_reference,
        &mut assertions.checked_relations,
    )?;
    require_body_consumer(
        graph,
        row.block_body,
        &declared_member,
        &mut assertions.body_ir_consumers,
        &format!("{} member/block", row.label),
    )?;
    let target_projection = graph.require_emitted_projection(row.target_module, &declared_member)?;
    let root_projection = graph.require_emitted_projection("identity_matrix", &declared_member)?;
    let projection_evidence = format!("{target_projection}; {root_projection}");
    assertions.legacy_projections.push(target_projection);
    assertions.legacy_projections.push(root_projection);
    assertions.coverage_cells.extend([
        IdentityCoverageCell {
            binding: row.binding,
            namespace: IdentityNamespace::Member,
            scope: IdentityScope::Owner,
            checked_identity: declared_member.render_compact(),
            hir_identity: None,
            body_ir_identity: None,
            emitted_projection: Some(projection_evidence.clone()),
        },
        IdentityCoverageCell {
            binding: row.binding,
            namespace: IdentityNamespace::Member,
            scope: IdentityScope::Function,
            checked_identity: function_reference.render_compact(),
            hir_identity: None,
            body_ir_identity: Some(declared_member.render_compact()),
            emitted_projection: Some(projection_evidence.clone()),
        },
        IdentityCoverageCell {
            binding: row.binding,
            namespace: IdentityNamespace::Member,
            scope: IdentityScope::Block,
            checked_identity: block_reference.render_compact(),
            hir_identity: None,
            body_ir_identity: Some(declared_member.render_compact()),
            emitted_projection: Some(projection_evidence),
        },
    ]);
    Ok(())
}

fn verify_path_matrix_row(
    graph: &CheckedIdentityGraph,
    row: &PathIdentityRow<'_>,
    assertions: &mut IdentityAssertions,
) -> Result<(), String> {
    let module_binding = graph.hir_identity("identity_matrix", row.module_binding)?;
    let expected_module = CanonicalSymbolId {
        namespace: SymbolNamespace::ModulePath,
        origin: SymbolOrigin::Module(
            row.expected_module_path
                .iter()
                .map(|segment| (*segment).to_string())
                .collect(),
        ),
        declaration_name: row.expected_module_name.to_string(),
        kind: SemanticSourceTargetKind::Module,
        scope_discriminant: None,
        declaration_span: HirSourceSpan::new(0, 0),
    };
    require_same_identity(
        &format!("{} path/module", row.label),
        &expected_module,
        &module_binding,
        &mut assertions.checked_relations,
    )?;
    assertions.hir_consumers.push(format!(
        "{} path/module HIR: {}",
        row.label,
        module_binding.render_compact()
    ));
    assertions.coverage_cells.push(IdentityCoverageCell {
        binding: row.binding,
        namespace: IdentityNamespace::ModulePath,
        scope: IdentityScope::Module,
        checked_identity: module_binding.render_compact(),
        hir_identity: Some(module_binding.render_compact()),
        body_ir_identity: None,
        emitted_projection: None,
    });

    let target = graph.declaration_identity(
        "identity_provider",
        row.target_name,
        SemanticSourceTargetKind::Function,
        SymbolNamespace::OrdinaryLexical,
    )?;
    let function_reference = graph.resolved_identity("identity_matrix", row.call, 0)?;
    require_same_identity(
        &format!("{} path/function", row.label),
        &target,
        &function_reference,
        &mut assertions.checked_relations,
    )?;
    require_body_consumer(
        graph,
        row.function_body,
        &target,
        &mut assertions.body_ir_consumers,
        &format!("{} path/function", row.label),
    )?;

    let block_reference = graph.resolved_identity("identity_matrix", row.call, 1)?;
    require_same_identity(
        &format!("{} path/block", row.label),
        &target,
        &block_reference,
        &mut assertions.checked_relations,
    )?;
    require_body_consumer(
        graph,
        row.block_body,
        &target,
        &mut assertions.body_ir_consumers,
        &format!("{} path/block", row.label),
    )?;
    assertions
        .legacy_projections
        .push(graph.require_emitted_projection("identity_provider", &target)?);
    assertions
        .legacy_projections
        .push(graph.require_emitted_projection("identity_matrix", &target)?);
    Ok(())
}

fn verify_wrong_path_target_selection(graph: &CheckedIdentityGraph) -> Result<IdentityAssertions, String> {
    let mut assertions = IdentityAssertions {
        coverage_cells: Vec::new(),
        checked_relations: Vec::new(),
        hir_consumers: Vec::new(),
        body_ir_consumers: Vec::new(),
        legacy_projections: Vec::new(),
        artifact_observations: Vec::new(),
    };
    verify_path_matrix_row(
        graph,
        &PathIdentityRow {
            label: "wrong-target negative",
            binding: IdentityBindingForm::Import,
            expected_module_path: &["identity_facade"],
            expected_module_name: "identity_facade",
            target_name: "imported_path",
            module_binding: "identity_provider",
            call: "identity_provider.imported_path(17)",
            function_body: "imported_path_function_scope",
            block_body: "imported_path_block_scope",
        },
        &mut assertions,
    )?;
    Err("wrong-target module-path selection was incorrectly accepted".to_string())
}

fn validate_identity_matrix_coverage(cells: &[IdentityCoverageCell]) -> Result<(), String> {
    let mut expected = BTreeSet::new();
    for binding in [
        IdentityBindingForm::Local,
        IdentityBindingForm::Import,
        IdentityBindingForm::Alias,
        IdentityBindingForm::ReExport,
    ] {
        for scope in [IdentityScope::Module, IdentityScope::Function, IdentityScope::Block] {
            expected.insert((binding, IdentityNamespace::Lexical, scope));
        }
        for scope in [IdentityScope::Owner, IdentityScope::Function, IdentityScope::Block] {
            expected.insert((binding, IdentityNamespace::Member, scope));
        }
    }
    for binding in [IdentityBindingForm::Import, IdentityBindingForm::Alias] {
        expected.insert((binding, IdentityNamespace::ModulePath, IdentityScope::Module));
    }
    let actual = cells
        .iter()
        .map(|cell| (cell.binding, cell.namespace, cell.scope))
        .collect::<BTreeSet<_>>();
    if actual != expected || cells.len() != expected.len() {
        return Err(format!(
            "RFC 120 typed coverage differs from the semantically valid contract: missing={:?}, unexpected={:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        ));
    }
    Ok(())
}

fn verify_wrong_owner_member_selection(graph: &CheckedIdentityGraph) -> Result<IdentityAssertions, String> {
    let mut assertions = IdentityAssertions {
        coverage_cells: Vec::new(),
        checked_relations: Vec::new(),
        hir_consumers: Vec::new(),
        body_ir_consumers: Vec::new(),
        legacy_projections: Vec::new(),
        artifact_observations: Vec::new(),
    };
    verify_member_matrix_row(
        graph,
        &MemberIdentityRow {
            label: "wrong-owner negative",
            binding: IdentityBindingForm::Import,
            target_module: "identity_provider",
            owner_name: "ImportedMember",
            owner: DeclarationSpanSelector {
                anchor: "pub model ImportedMember:",
                occurrence: 0,
            },
            // Occurrence 1 is `AliasedMember.read`, deliberately outside the selected owner declaration span.
            member: DeclarationSpanSelector {
                anchor: "def read(self) -> int:",
                occurrence: 1,
            },
            root_binding: "ImportedMember",
            receiver_call: "imported.read()",
            function_body: "imported_member_function_scope",
            block_body: "imported_member_block_scope",
        },
        &mut assertions,
    )?;
    Err("wrong-owner member selection was incorrectly accepted".to_string())
}

fn verify_identity_matrix(graph: &CheckedIdentityGraph) -> Result<IdentityAssertions, String> {
    let mut assertions = IdentityAssertions {
        coverage_cells: Vec::new(),
        checked_relations: Vec::new(),
        hir_consumers: Vec::new(),
        body_ir_consumers: Vec::new(),
        legacy_projections: Vec::new(),
        artifact_observations: Vec::new(),
    };
    for row in [
        LexicalIdentityRow {
            label: "local",
            binding: IdentityBindingForm::Local,
            target_module: "identity_matrix",
            target_name: "local_lexical",
            root_binding: "local_lexical",
            call: "local_lexical(1)",
            function_body: "local_lexical_function_scope",
            block_body: "local_lexical_block_scope",
        },
        LexicalIdentityRow {
            label: "import",
            binding: IdentityBindingForm::Import,
            target_module: "identity_provider",
            target_name: "imported_lexical",
            root_binding: "imported_lexical",
            call: "imported_lexical(3)",
            function_body: "imported_lexical_function_scope",
            block_body: "imported_lexical_block_scope",
        },
        LexicalIdentityRow {
            label: "alias",
            binding: IdentityBindingForm::Alias,
            target_module: "identity_provider",
            target_name: "aliased_lexical",
            root_binding: "lexical_alias",
            call: "lexical_alias(5)",
            function_body: "aliased_lexical_function_scope",
            block_body: "aliased_lexical_block_scope",
        },
        LexicalIdentityRow {
            label: "re-export",
            binding: IdentityBindingForm::ReExport,
            target_module: "identity_provider",
            target_name: "relayed_lexical",
            root_binding: "lexical_reexport",
            call: "lexical_reexport(7)",
            function_body: "reexported_lexical_function_scope",
            block_body: "reexported_lexical_block_scope",
        },
    ] {
        verify_lexical_matrix_row(graph, &row, &mut assertions)?;
    }
    for row in [
        MemberIdentityRow {
            label: "local",
            binding: IdentityBindingForm::Local,
            target_module: "identity_matrix",
            owner_name: "LocalMember",
            owner: DeclarationSpanSelector {
                anchor: "model LocalMember:",
                occurrence: 0,
            },
            member: DeclarationSpanSelector {
                anchor: "def read(self) -> int:",
                occurrence: 0,
            },
            root_binding: "LocalMember",
            receiver_call: "local.read()",
            function_body: "local_member_function_scope",
            block_body: "local_member_block_scope",
        },
        MemberIdentityRow {
            label: "import",
            binding: IdentityBindingForm::Import,
            target_module: "identity_provider",
            owner_name: "ImportedMember",
            owner: DeclarationSpanSelector {
                anchor: "pub model ImportedMember:",
                occurrence: 0,
            },
            member: DeclarationSpanSelector {
                anchor: "def read(self) -> int:",
                occurrence: 0,
            },
            root_binding: "ImportedMember",
            receiver_call: "imported.read()",
            function_body: "imported_member_function_scope",
            block_body: "imported_member_block_scope",
        },
        MemberIdentityRow {
            label: "alias",
            binding: IdentityBindingForm::Alias,
            target_module: "identity_provider",
            owner_name: "AliasedMember",
            owner: DeclarationSpanSelector {
                anchor: "pub model AliasedMember:",
                occurrence: 0,
            },
            member: DeclarationSpanSelector {
                anchor: "def read(self) -> int:",
                occurrence: 1,
            },
            root_binding: "MemberAlias",
            receiver_call: "aliased.read()",
            function_body: "aliased_member_function_scope",
            block_body: "aliased_member_block_scope",
        },
        MemberIdentityRow {
            label: "re-export",
            binding: IdentityBindingForm::ReExport,
            target_module: "identity_provider",
            owner_name: "RelayedMember",
            owner: DeclarationSpanSelector {
                anchor: "pub model RelayedMember:",
                occurrence: 0,
            },
            member: DeclarationSpanSelector {
                anchor: "def read(self) -> int:",
                occurrence: 2,
            },
            root_binding: "MemberReexport",
            receiver_call: "relayed.read()",
            function_body: "reexported_member_function_scope",
            block_body: "reexported_member_block_scope",
        },
    ] {
        verify_member_matrix_row(graph, &row, &mut assertions)?;
    }
    for row in [
        PathIdentityRow {
            label: "import",
            binding: IdentityBindingForm::Import,
            expected_module_path: &["identity_provider"],
            expected_module_name: "identity_provider",
            target_name: "imported_path",
            module_binding: "identity_provider",
            call: "identity_provider.imported_path(17)",
            function_body: "imported_path_function_scope",
            block_body: "imported_path_block_scope",
        },
        PathIdentityRow {
            label: "alias",
            binding: IdentityBindingForm::Alias,
            expected_module_path: &["identity_provider"],
            expected_module_name: "identity_provider",
            target_name: "aliased_path",
            module_binding: "provider_alias",
            call: "provider_alias.aliased_path(19)",
            function_body: "aliased_path_function_scope",
            block_body: "aliased_path_block_scope",
        },
    ] {
        verify_path_matrix_row(graph, &row, &mut assertions)?;
    }
    validate_identity_matrix_coverage(&assertions.coverage_cells)?;
    Ok(assertions)
}

const LET_SHADOW_SRC: &str = r#"
def shadow_let() -> int:
    mut total = 0
    x = 1
    if true:
        let x = 2
        total += x
    return total + x
"#;

const LET_SHADOW_MODULES: &[IdentitySourceModule] = &[IdentitySourceModule {
    name: "identity_let_shadow",
    path: &["identity_let_shadow"],
    source: LET_SHADOW_SRC,
    dependencies: &[],
}];

const MUT_SHADOW_SRC: &str = r#"
def shadow_mut() -> int:
    mut total = 0
    mut x = 4
    if true:
        mut x = 7
        total += x
    return total + x
"#;

const MUT_SHADOW_MODULES: &[IdentitySourceModule] = &[IdentitySourceModule {
    name: "identity_mut_shadow",
    path: &["identity_mut_shadow"],
    source: MUT_SHADOW_SRC,
    dependencies: &[],
}];

const GENERIC_BINDER_SRC: &str = r#"
def generic_identity[T](value: T) -> T:
    return value

def generic_entry() -> int:
    return generic_identity[int](42)
"#;

const GENERIC_BINDER_MODULES: &[IdentitySourceModule] = &[IdentitySourceModule {
    name: "identity_generic",
    path: &["identity_generic"],
    source: GENERIC_BINDER_SRC,
    dependencies: &[],
}];

const BUILTIN_REBINDING_SRC: &str = r#"
def len(value: int) -> int:
    return value + 1

def builtin_entry() -> int:
    return len(4) + std.builtins.len([1, 2, 3])
"#;

const BUILTIN_REBINDING_MODULES: &[IdentitySourceModule] = &[IdentitySourceModule {
    name: "identity_builtin",
    path: &["identity_builtin"],
    source: BUILTIN_REBINDING_SRC,
    dependencies: &[],
}];

fn no_replacement_arguments() -> Vec<ReplacementValue> {
    Vec::new()
}

fn expected_three() -> ReplacementValue {
    ReplacementValue::Int(3)
}

fn expected_eleven() -> ReplacementValue {
    ReplacementValue::Int(11)
}

fn expected_forty_two() -> ReplacementValue {
    ReplacementValue::Int(42)
}

fn expected_eight() -> ReplacementValue {
    ReplacementValue::Int(8)
}

fn verify_shadow_binding(
    graph: &CheckedIdentityGraph,
    module: &str,
    body: &str,
    declaration_name: &str,
) -> Result<IdentityAssertions, String> {
    // Binding tokens introduce declarations and are intentionally absent from the checked-reference map. Read the
    // identities selected by the two use sites, then require Body IR to carry those same declaration identities.
    let inner_read = graph.resolved_identity(module, "x", 2)?;
    let outer_read = graph.resolved_identity(module, "x", 3)?;
    if outer_read == inner_read {
        return Err("same-spelled shadow declarations collapsed to one canonical identity".to_string());
    }
    let mut checked_relations = Vec::new();
    checked_relations.push(format!(
        "shadow declarations distinct: {} != {}",
        outer_read.render_compact(),
        inner_read.render_compact()
    ));

    let locals = graph.body_local_identities(module, body, "x")?;
    if locals.len() != 2
        || !locals.iter().any(|identity| identity == &outer_read)
        || !locals.iter().any(|identity| identity == &inner_read)
    {
        return Err(format!(
            "Body IR did not retain both canonical shadow locals, got {locals:?}"
        ));
    }
    let function = graph.declaration_identity(
        module,
        declaration_name,
        SemanticSourceTargetKind::Function,
        SymbolNamespace::OrdinaryLexical,
    )?;
    let hir_function = graph.hir_identity(module, declaration_name)?;
    require_same_identity("shadow entry HIR", &function, &hir_function, &mut checked_relations)?;
    Ok(IdentityAssertions {
        coverage_cells: Vec::new(),
        checked_relations,
        hir_consumers: vec![format!("shadow entry HIR: {}", hir_function.render_compact())],
        body_ir_consumers: locals
            .iter()
            .map(|identity| format!("shadow local: {}", identity.render_compact()))
            .collect(),
        legacy_projections: vec![graph.require_emitted_projection(module, &function)?],
        artifact_observations: Vec::new(),
    })
}

fn verify_let_shadow(graph: &CheckedIdentityGraph) -> Result<IdentityAssertions, String> {
    verify_shadow_binding(graph, "identity_let_shadow", "shadow_let", "shadow_let")
}

fn verify_mut_shadow(graph: &CheckedIdentityGraph) -> Result<IdentityAssertions, String> {
    verify_shadow_binding(graph, "identity_mut_shadow", "shadow_mut", "shadow_mut")
}

fn verify_generic_binder(graph: &CheckedIdentityGraph) -> Result<IdentityAssertions, String> {
    // The binder token introduces a declaration, so it deliberately does not appear in the
    // checker-owned reference map. Its annotations are references to that declaration and carry
    // the canonical GenericBinder identity into downstream consumers.
    let parameter = graph.resolved_identity("identity_generic", "T", 1)?;
    let return_type = graph.resolved_identity("identity_generic", "T", 2)?;
    if parameter.kind != SemanticSourceTargetKind::GenericBinder {
        return Err(format!(
            "generic parameter annotation did not retain its GenericBinder identity: {}",
            parameter.render_compact()
        ));
    }
    let mut checked_relations = Vec::new();
    require_same_identity(
        "generic binder annotations",
        &parameter,
        &return_type,
        &mut checked_relations,
    )?;
    let concrete_int = graph.resolved_identity("identity_generic", "int", 1)?;
    if concrete_int == parameter || concrete_int.kind == SemanticSourceTargetKind::GenericBinder {
        return Err(format!(
            "generic binder collapsed into concrete `int`: binder={}, concrete={}",
            parameter.render_compact(),
            concrete_int.render_compact()
        ));
    }
    checked_relations.push(format!(
        "generic binder/concrete distinct: {} != {}",
        parameter.render_compact(),
        concrete_int.render_compact()
    ));
    let generic_function = graph.declaration_identity(
        "identity_generic",
        "generic_identity",
        SemanticSourceTargetKind::Function,
        SymbolNamespace::OrdinaryLexical,
    )?;
    let call = graph.resolved_identity("identity_generic", "generic_identity[int](42)", 0)?;
    require_same_identity(
        "generic callable selection",
        &generic_function,
        &call,
        &mut checked_relations,
    )?;
    let body_consumers = graph.body_consumer_identities("identity_generic", "generic_entry")?;
    if !body_consumers.iter().any(|identity| identity == &generic_function) {
        return Err("generic call target did not survive into replacement-facing Body IR".to_string());
    }
    let hir_function = graph.hir_identity("identity_generic", "generic_identity")?;
    require_same_identity(
        "generic function HIR",
        &generic_function,
        &hir_function,
        &mut checked_relations,
    )?;
    Ok(IdentityAssertions {
        coverage_cells: Vec::new(),
        checked_relations,
        hir_consumers: vec![format!("generic function HIR: {}", hir_function.render_compact())],
        body_ir_consumers: body_consumers
            .iter()
            .map(|identity| format!("generic Body IR consumer: {}", identity.render_compact()))
            .collect(),
        legacy_projections: vec![graph.require_emitted_projection("identity_generic", &generic_function)?],
        artifact_observations: Vec::new(),
    })
}

fn verify_builtin_rebinding(graph: &CheckedIdentityGraph) -> Result<IdentityAssertions, String> {
    let local = graph.declaration_identity(
        "identity_builtin",
        "len",
        SemanticSourceTargetKind::Function,
        SymbolNamespace::OrdinaryLexical,
    )?;
    let local_call = graph.resolved_identity("identity_builtin", "len(4)", 0)?;
    let builtin_call = graph.resolved_identity("identity_builtin", "std.builtins.len([1, 2, 3])", 0)?;
    if builtin_call.kind != SemanticSourceTargetKind::Builtin
        || builtin_call.namespace != SymbolNamespace::OrdinaryLexical
        || builtin_call == local
    {
        return Err(format!(
            "builtin qualification did not stay distinct from the ordinary lexical binding: local={}, builtin={}",
            local.render_compact(),
            builtin_call.render_compact()
        ));
    }
    let mut checked_relations = Vec::new();
    require_same_identity(
        "ordinary builtin-name rebinding",
        &local,
        &local_call,
        &mut checked_relations,
    )?;
    checked_relations.push(format!(
        "ordinary/builtin distinct: {} != {}",
        local.render_compact(),
        builtin_call.render_compact()
    ));
    let body_consumers = graph.body_consumer_identities("identity_builtin", "builtin_entry")?;
    if !body_consumers.iter().any(|identity| identity == &local)
        || !body_consumers.iter().any(|identity| identity == &builtin_call)
    {
        return Err(format!(
            "Body IR did not retain both local and builtin targets: {body_consumers:?}"
        ));
    }
    let hir_local = graph.hir_identity("identity_builtin", "len")?;
    require_same_identity(
        "ordinary builtin-name binding HIR",
        &local,
        &hir_local,
        &mut checked_relations,
    )?;
    Ok(IdentityAssertions {
        coverage_cells: Vec::new(),
        checked_relations,
        hir_consumers: vec![format!("local len HIR: {}", hir_local.render_compact())],
        body_ir_consumers: body_consumers
            .iter()
            .map(|identity| format!("builtin row Body IR consumer: {}", identity.render_compact()))
            .collect(),
        legacy_projections: vec![graph.require_emitted_projection("identity_builtin", &local)?],
        artifact_observations: Vec::new(),
    })
}

fn verify_release_artifact() -> Result<ReleaseArtifactAssertions, String> {
    static EVIDENCE: OnceLock<Result<ReleaseArtifactAssertions, String>> = OnceLock::new();
    EVIDENCE
        .get_or_init(|| {
            let evidence = emitted_symbol_artifact::verify_pinned_release_artifact()
                .map_err(|error| format!("pinned release artifact verification failed: {error}"))?;
            if evidence.recovered_identities.len() != 4
                || !evidence.saw_generic_u64_specialization
                || !evidence.saw_non_incan_host_symbol
            {
                return Err(format!(
                    "pinned release artifact returned incomplete evidence: {evidence:?}"
                ));
            }
            Ok(ReleaseArtifactAssertions {
                assertions: IdentityAssertions {
                    coverage_cells: Vec::new(),
                    checked_relations: Vec::new(),
                    hir_consumers: Vec::new(),
                    body_ir_consumers: Vec::new(),
                    legacy_projections: evidence
                        .recovered_identities
                        .iter()
                        .map(|identity| format!("recovered incan-v1: {}", identity.render_compact()))
                        .collect(),
                    artifact_observations: vec![
                        format!("rustc {} optimized v0 artifact", emitted_symbol_artifact::SELECTED_RUST),
                        "generic specialization u64 recovered".to_string(),
                        "host_bridge classified as non-Incan".to_string(),
                        format!(
                            "artifact bytes baseline={} projected={}",
                            evidence.baseline_bytes, evidence.projected_bytes
                        ),
                        format!(
                            "identifier bytes baseline={} projected={}",
                            evidence.baseline_identifier_bytes, evidence.projected_identifier_bytes
                        ),
                    ],
                },
                fixture_input_identity: evidence.fixture_input_identity,
                artifact_content_identity: evidence.artifact_content_identity,
                recovered_observation_identity: evidence.recovered_observation_identity,
            })
        })
        .clone()
}

// ============================================================================
// Seed corpus
// ============================================================================

/// The stable #987 corpus, including RFC 120's executable checked-identity and artifact-projection rows.
/// Package/import execution remains a named #989 boundary rather than an inferred success.
fn seed_corpus() -> Vec<ParityCase> {
    vec![
        ParityCase {
            id: "parity-987-0001",
            title: "Match expressions over enums must be exhaustive",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "tests/parity_corpus_tests.rs::case_supported_match_exhaustiveness",
            disposition: Disposition::Preserved,
            source: CASE_1_SRC,
            evaluate: Some(case_supported_match_exhaustiveness),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0002",
            title: "Chained comparisons are rejected with a type-mismatch diagnostic",
            category: BehaviorCategory::DiagnosticBehavior,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "tests/parity_corpus_tests.rs::case_diagnostic_chained_comparison_rejected",
            disposition: Disposition::Preserved,
            source: CASE_2_SRC,
            evaluate: Some(case_diagnostic_chained_comparison_rejected),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0003",
            title: "String membership (`in`) matches the runtime helper's substring policy",
            category: BehaviorCategory::StdlibRuntimeBehavior,
            lane: EvidenceLane::GeneratedProjectRun,
            evidence: "tests/parity_corpus_tests.rs::case_stdlib_runtime_string_membership",
            disposition: Disposition::Preserved,
            source: CASE_3_SRC,
            evaluate: Some(case_stdlib_runtime_string_membership),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0004",
            title: "Generated Rust stays syntactically valid and inspectable, not the semantic contract",
            category: BehaviorCategory::GeneratedArtifactBehavior,
            lane: EvidenceLane::CodegenSnapshot,
            evidence: "tests/parity_corpus_tests.rs::case_generated_artifact_valid_rust_shape",
            disposition: Disposition::Preserved,
            source: CASE_4_SRC,
            evaluate: Some(case_generated_artifact_valid_rust_shape),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0005",
            title: "A lexical builtin-name collision (`len`) preserves the module binding",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "tests/parity_corpus_tests.rs::case_supported_builtin_len_shadowing",
            disposition: Disposition::Preserved,
            source: CASE_5_SRC,
            evaluate: Some(case_supported_builtin_len_shadowing),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0006",
            title: "Dead code after `return` reports an unreachable-code warning (INCAN-T0101)",
            category: BehaviorCategory::DiagnosticBehavior,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "tests/parity_corpus_tests.rs::case_diagnostic_unreachable_code_after_return",
            disposition: Disposition::IntentionalMigration {
                owning_issue: 1117,
                migration_note: "Migrated by #1117 before 0.6 cutover: statements after a `return` in the same \
                                  block now raise the non-fatal `INCAN-T0101` warning instead of typechecking \
                                  silently. Migration guidance for the replacement backend: the contract is the \
                                  frontend diagnostic, not generated Rust's own `unreachable_code` lint, so a \
                                  replacement backend must not be relied on to reproduce it. The rule is \
                                  deliberately block-local — it does not model divergence through `if`/`else`, \
                                  `match`, or loops — and existing user code with dead code still compiles, \
                                  because the diagnostic is a warning and never an error.",
            },
            source: CASE_6_SRC,
            evaluate: Some(case_diagnostic_unreachable_code_after_return),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0007",
            title: "Statement tuple unpack of a non-tuple value is a typechecker error",
            category: BehaviorCategory::DiagnosticBehavior,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "tests/parity_corpus_tests.rs::case_diagnostic_statement_tuple_unpack_of_non_tuple",
            disposition: Disposition::IntentionalMigration {
                owning_issue: 1132,
                migration_note: "Migrated by #1132 before 0.6 cutover: `a, b = <non-tuple>` and the `TupleAssign` \
                                  spelling now raise a source-span typechecker error naming the resolved value \
                                  type, instead of binding every name `Unknown` and failing later in generated \
                                  Rust. Migration guidance for the replacement backend: the contract is the \
                                  frontend diagnostic, so a replacement backend must not be relied on to \
                                  reproduce it, and must never emit a tuple-field projection into a value with no \
                                  such fields. A value is destructurable only when its shape is actually known: \
                                  inferred tuples, annotated `tuple[A, B]`, and Rust-interop paths whose tuple \
                                  spelling the compiler can read. `Unknown` and `Never` stay silent as recovery \
                                  states; a bare type variable and an opaque Rust path are both refused, because \
                                  \"not proven tuple-shaped\" must not be treated as destructurable.",
            },
            source: CASE_7_SRC,
            evaluate: Some(case_diagnostic_statement_tuple_unpack_of_non_tuple),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0008",
            title: "Named `model` construction lowers to Body IR with a resolved field binding",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "#1158; src/frontend/body_ir.rs::tests::named_construction_lowers_to_a_constructor_aggregate_with_a_resolved_field_binding",
            disposition: Disposition::Preserved,
            source: CASE_8_SRC,
            evaluate: Some(case_supported_named_construction_reaches_body_ir),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0009",
            title: "Named, out-of-order, and defaulted call arguments lower to Body IR",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "#1158; src/frontend/body_ir.rs::tests::{out_of_order_named_call_arguments_evaluate_in_written_source_order, an_omitted_defaulted_argument_is_recorded_as_a_defaulted_slot}",
            disposition: Disposition::Preserved,
            source: CASE_9_SRC,
            evaluate: Some(case_supported_named_call_arguments_reach_body_ir),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0010",
            title: "`await` lowers to a Body IR suspension point with a destination",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "#1164; src/frontend/body_ir.rs::tests::lowers_await_as_an_explicit_suspension_point_with_a_destination",
            disposition: Disposition::Preserved,
            source: CASE_10_SRC,
            evaluate: Some(case_supported_await_reaches_body_ir),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0011",
            title: "`race for` lowers to a Body IR race with per-arm bindings and bodies",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "#1164; src/frontend/body_ir.rs::tests::lowers_a_two_arm_race_with_per_arm_bindings_and_pre_selection_awaitables",
            disposition: Disposition::Preserved,
            source: CASE_11_SRC,
            evaluate: Some(case_supported_race_for_reaches_body_ir),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0012",
            title: "List and dict literal spreads lower with their fixed elements intact",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "#1159; src/frontend/body_ir.rs::tests::fixed_elements_keep_their_positions_on_both_sides_of_a_spread",
            disposition: Disposition::Preserved,
            source: CASE_12_SRC,
            evaluate: Some(case_supported_literal_spreads_reach_body_ir),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0013",
            title: "Positional, spread, named, and keyword-spread call arguments lower together",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "#1159; src/frontend/body_ir.rs::tests::a_mixed_call_keeps_every_written_argument_form",
            disposition: Disposition::Preserved,
            source: CASE_13_SRC,
            evaluate: Some(case_supported_call_spreads_reach_body_ir),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0014",
            title: "String membership (`in`) is representable in Body IR, not only in the runtime helper",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "#1160; src/frontend/body_ir/tests.rs::lowers_string_membership_as_an_explicit_helper_call_with_its_runtime_requirement",
            disposition: Disposition::Preserved,
            source: CASE_3_SRC,
            evaluate: Some(case_supported_string_membership_reaches_body_ir),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0015",
            title: "Byte-string literals are representable in Body IR",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "#1165; src/frontend/body_ir/tests.rs::bytes_literals_lower_to_their_own_constant_rather_than_a_string",
            disposition: Disposition::Preserved,
            source: CASE_15_SRC,
            evaluate: Some(case_supported_bytes_literal_reaches_body_ir),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0016",
            title: "A range bound to a local is representable in Body IR and iterates from that value",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "#1165; src/frontend/body_ir/tests.rs::a_bound_range_iterates_with_the_same_facts_as_the_inline_range",
            disposition: Disposition::Preserved,
            source: CASE_16_SRC,
            evaluate: Some(case_supported_range_value_reaches_body_ir),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0018",
            title: "Statement-position `loop:` is representable in Body IR, not only the expression spelling",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "#1162; src/frontend/body_ir/tests.rs::a_statement_position_loop_lowers_to_the_same_loop_the_expression_spelling_produces",
            disposition: Disposition::Preserved,
            source: CASE_18_SRC,
            evaluate: Some(case_supported_statement_loop_reaches_body_ir),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0019",
            title: "An `unsafe:` acknowledgement region refuses in Body IR under a named, stated boundary",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "#1162; src/frontend/body_ir/tests.rs::an_unsafe_region_refuses_under_a_named_permanent_boundary",
            // The corpus's first real `Unsupported` row, so it carries the full migration note the schema asks
            // for rather than a pointer to one.
            disposition: Disposition::Unsupported {
                owning_issue: 1162,
                migration_note: "An `unsafe:` region records an explicit acknowledgement that the operations \
                                 inside it require authorization. It introduces no separate Incan scope, so \
                                 lowering its statements into the enclosing block would be a two-line change — \
                                 and would erase exactly the fact the region exists to carry, leaving a direct \
                                 replacement execution profile running an authorized region it was never told \
                                 about. Body IR v0 has no acknowledgement fact a consumer could weigh, so the \
                                 region refuses under a named label stating that it is refused by design \
                                 (`BodyBuilder::refuse_unsafe_region` in src/frontend/body_ir/stmt.rs). \
                                 Cutover impact: a program whose `unsafe:` region must execute cannot use the \
                                 replacement backend; the legacy Rust-emission backend keeps compiling it \
                                 unchanged, so no accepted program regresses. Reversing this disposition means \
                                 designing the acknowledgement representation first and deciding who may admit \
                                 it — adding a dispatch arm alone would be the silent execution this row \
                                 exists to prevent. Owned by #1162 until that design lands.",
            },
            source: CASE_19_SRC,
            evaluate: Some(case_unsafe_region_is_a_stated_refusal),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0020",
            title: "A pattern assertion binds its payload as a declared local",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "#1167; src/frontend/body_ir/tests.rs::\
                       a_pattern_assertion_binding_is_a_declared_local_read_by_the_statements_after_it",
            disposition: Disposition::Preserved,
            source: CASE_20_SRC,
            evaluate: Some(case_pattern_assertion_binding_reaches_body_ir),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0022",
            title: "Collection membership names the container it was written over",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "#1246; src/frontend/body_ir/tests.rs::\
                       lowers_collection_membership_as_a_helper_call_naming_its_own_container",
            disposition: Disposition::Preserved,
            source: CASE_22_SRC,
            evaluate: Some(case_collection_membership_names_its_container),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0023",
            title: "List concatenation is a helper call rather than a primitive addition",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "#1246; src/frontend/body_ir/tests.rs::\
                       lowers_list_concatenation_as_a_helper_call_rather_than_a_primitive_addition",
            disposition: Disposition::Preserved,
            source: CASE_23_SRC,
            evaluate: Some(case_list_concatenation_is_not_a_primitive_addition),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0021",
            title: "A `raises` assertion carries its resolved expected error type",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "#1167; src/frontend/body_ir/tests.rs::\
                       a_raises_assertion_retains_the_resolved_expected_error_rather_than_its_spelling",
            disposition: Disposition::Preserved,
            source: CASE_21_SRC,
            evaluate: Some(case_raises_assertion_reaches_body_ir),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-0017",
            title: "A body behind an inactive feature never reaches Body IR",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "#1166; src/cli/commands/build.rs::tests::\
                       the_replacement_build_never_executes_a_main_behind_an_inactive_feature",
            disposition: Disposition::Preserved,
            source: CASE_17_SRC,
            evaluate: Some(case_inactive_feature_body_never_reaches_body_ir),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "replacement-body-v0-001",
            title: "Parameterized integer addition executes through Body IR",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#988 replacement-body-v0-001 executed directly; #1146 two-route comparison in \
                       tests/shadow_comparison_tests.rs and tests/parity_corpus_tests.rs::seed_corpus",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_001_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "add",
                arguments: replacement_body_v0_001_arguments,
                expected: replacement_body_v0_001_expected,
                // The original #1146 scalar case now uses the separate typed-result report, never a program stream.
                shadow_comparison: true,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-002",
            title: "Parameterized string concatenation executes through Body IR",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#988 replacement-body-v0-002; tests/parity_corpus_tests.rs::seed_corpus",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_002_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "greet",
                arguments: replacement_body_v0_002_arguments,
                expected: replacement_body_v0_002_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-003",
            title: "Owned local return preserves move evidence through Body IR",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#988 replacement-body-v0-003; tests/parity_corpus_tests.rs::seed_corpus",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_003_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "return_owned",
                arguments: replacement_body_v0_003_arguments,
                expected: replacement_body_v0_003_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-004",
            title: "Normalized range, branch, and while control flow execute through Body IR",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#988 replacement-body-v0-004; tests/parity_corpus_tests.rs::seed_corpus",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_004_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "control_flow",
                arguments: replacement_body_v0_004_arguments,
                expected: replacement_body_v0_004_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-005",
            title: "Assertion and floor division execute through Body IR",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#988 replacement-body-v0-005; tests/parity_corpus_tests.rs::seed_corpus",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_005_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "guarded_floor_div",
                arguments: replacement_body_v0_005_arguments,
                expected: replacement_body_v0_005_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-006",
            title: "Scalar tuple collection loops destructure through Body IR",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#988 replacement-body-v0-006; tests/parity_corpus_tests.rs::seed_corpus",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_006_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "select_second_pair",
                arguments: replacement_body_v0_006_arguments,
                expected: replacement_body_v0_006_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-007",
            title: "Lazy generator expressions materialize through Body IR only when collected",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1123; tests/replacement_backend_execution_tests.rs::replacement_executes_a_lazy_generator_expression_only_when_collect_consumes_it",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_007_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "collect_lazy_values",
                arguments: replacement_body_v0_007_arguments,
                expected: replacement_body_v0_007_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-008",
            title: "Captured stored closures execute in isolated direct Body-IR frames",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1152; tests/replacement_backend_execution_tests.rs::replacement_executes_a_captured_stored_closure_in_an_isolated_frame",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_008_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "stored_closure",
                arguments: replacement_body_v0_008_arguments,
                expected: replacement_body_v0_008_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-009",
            title: "Partial presets and declaration defaults bind through direct Body IR",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1152; tests/replacement_backend_execution_tests.rs::replacement_executes_partial_presets_source_defaults_and_named_overrides",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_009_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "partial_defaults",
                arguments: replacement_body_v0_009_arguments,
                expected: replacement_body_v0_009_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-010",
            title: "Generator-function frames resume directly through Body IR",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1152; tests/replacement_backend_execution_tests.rs::replacement_resumes_a_generator_function_without_replaying_its_prefix",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_010_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "generator_function",
                arguments: replacement_body_v0_010_arguments,
                expected: replacement_body_v0_010_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-011",
            title: "Lazy generator map and filter adapters invoke local Body-IR callables",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1152; tests/replacement_backend_execution_tests.rs::replacement_executes_lazy_generator_adapters_with_local_callbacks",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_011_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "generator_adapters",
                arguments: replacement_body_v0_011_arguments,
                expected: replacement_body_v0_011_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-012",
            title: "Source-local tuple/list values and exact sibling dispatch execute through Body IR",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1154; tests/replacement_backend_execution_tests.rs::replacement_executes_source_local_tuple_list_index_and_mutation_through_a_direct_callable; comparison remains non-green until #1154 produces paired source-observable evidence through #1146's completed route",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_012_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "structural_values",
                arguments: replacement_body_v0_012_arguments,
                expected: replacement_body_v0_012_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-013",
            title: "Source-local plain model values and canonical field reads execute through Body IR",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1154; tests/replacement_backend_execution_tests.rs::replacement_executes_source_local_nominal_model_values_through_a_direct_callable; comparison remains non-green until #1154 produces paired source-observable evidence through #1146's completed route",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_013_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "nominal_values",
                arguments: replacement_body_v0_013_arguments,
                expected: replacement_body_v0_013_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-014",
            title: "Source-local RFC 032 value-enum members extract scalar values through Body IR",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1154; tests/replacement_backend_execution_tests.rs::replacement_executes_source_local_value_enum_members_through_a_direct_callable; comparison remains non-green until #1154 produces paired source-observable evidence through #1146's completed route",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_014_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "value_enum_values",
                arguments: replacement_body_v0_014_arguments,
                expected: replacement_body_v0_014_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-015",
            title: "Source-local fieldless normal-enum values compare through retained Body IR identities",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1154; tests/replacement_backend_execution_tests.rs::replacement_executes_source_local_fieldless_enum_values_through_a_direct_callable; comparison remains non-green until #1154 produces paired source-observable evidence through #1146's completed route",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_015_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "fieldless_enum_values",
                arguments: replacement_body_v0_015_arguments,
                expected: replacement_body_v0_015_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-016",
            title: "Source-local nominal and fieldless-enum patterns dispatch through retained Body IR identities",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1154; tests/replacement_backend_execution_tests.rs::replacement_executes_identity_selected_nominal_and_fieldless_enum_match_patterns; comparison remains non-green until #1154 produces paired source-observable evidence through #1146's completed route",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_016_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "direct_patterns",
                arguments: replacement_body_v0_016_arguments,
                expected: replacement_body_v0_016_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-017",
            title: "Intrinsic Result construction, same-error propagation, and pattern dispatch execute through Body IR",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1154; tests/replacement_backend_execution_tests.rs::replacement_executes_same_error_result_routing_and_pattern_dispatch; comparison remains non-green until #1154 produces paired source-observable evidence through #1146's completed route",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_017_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "direct_result_routing",
                arguments: replacement_body_v0_017_arguments,
                expected: replacement_body_v0_017_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-018",
            title: "Source-local async await executes through direct Body-IR task frames",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1155; tests/replacement_backend_execution_tests.rs::replacement_executes_a_source_local_async_task_and_binds_its_lifecycle_evidence; comparison remains non-green until #1155 produces paired source-observable evidence through #1146's completed route",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_018_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "direct_async_await",
                arguments: replacement_body_v0_018_arguments,
                expected: replacement_body_v0_018_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-019",
            title: "Source-order ready ties execute through direct Body-IR race task frames",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1155; tests/replacement_backend_execution_tests.rs::replacement_executes_source_order_async_race_ties_with_loser_cancellation; comparison remains non-green until #1155 produces paired source-observable evidence through #1146's completed route",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_019_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "source_order_race",
                arguments: replacement_body_v0_019_arguments,
                expected: replacement_body_v0_019_expected,
                shadow_comparison: false,
            }),
        },
        ParityCase {
            id: HASHED_SHADOW_CASE_ID,
            title: "Hashed scalar-key set and dictionary membership agrees across independent routes",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1247; tests/replacement_hashed_shadow_tests.rs::hashed_membership_matches_the_receipt_backed_native_route; all four key kinds and membership helpers, typed-empty constructors, exact stdout and a separate boolean result",
            disposition: Disposition::Preserved,
            source: HASHED_MEMBERSHIP_SOURCE,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "membership",
                arguments: Vec::new,
                expected: || ReplacementValue::Bool(true),
                shadow_comparison: true,
            }),
        },
        ParityCase {
            id: STRING_HELPER_SHADOW_CASE_ID,
            title: "Canonical selected string helpers agree across independent routes",
            category: BehaviorCategory::StdlibRuntimeBehavior,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1256; tests/replacement_string_helper_shadow_tests.rs::selected_string_helpers_match_the_receipt_backed_native_route; seven retained helper identities, shared Unicode and separator behavior, exact stdout and a separate boolean result",
            disposition: Disposition::Preserved,
            source: STRING_HELPER_SOURCE,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "string_helpers",
                arguments: Vec::new,
                expected: || ReplacementValue::Bool(true),
                shadow_comparison: true,
            }),
        },
        ParityCase {
            id: "replacement-body-v0-022",
            title: "Checked scalar conversions preserve typed results and program output through both routes",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1249; tests/replacement_scalar_conversion_tests.rs::replacement_executes_checked_unary_scalar_conversions; tests/replacement_scalar_conversion_shadow_tests.rs::scalar_conversion_failure_keeps_its_canonical_class_before_legacy_substring_heuristics",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_022_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "scalar_conversions",
                arguments: replacement_body_v0_022_arguments,
                expected: replacement_body_v0_022_expected,
                shadow_comparison: true,
            }),
        },
        ParityCase {
            id: ENUMERATE_ZIP_SHADOW_CASE_ID,
            title: "Canonical stored Enumerate and direct Zip preserve source order through both routes",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1249; tests/fixtures/replacement/enumerate_zip.incn; \
                       tests/parity_corpus_tests.rs::the_enumerate_zip_row_carries_two_route_receipts_and_exact_output",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_023_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "enumerate_zip_profile",
                arguments: replacement_body_v0_023_arguments,
                expected: replacement_body_v0_023_expected,
                shadow_comparison: true,
            }),
        },
        ParityCase {
            id: STRING_LEN_SHADOW_CASE_ID,
            title: "Global and method string length agree on Unicode-scalar semantics across independent routes",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1249; tests/replacement_string_len_shadow_tests.rs::string_len_matches_the_receipt_backed_native_route; global builtin and checked method-helper identities, five Unicode rows, exact stdout and a separate boolean result",
            disposition: Disposition::Preserved,
            source: STRING_LEN_SOURCE,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "string_len",
                arguments: Vec::new,
                expected: || ReplacementValue::Bool(true),
                shadow_comparison: true,
            }),
        },
        ParityCase {
            id: JSON_STRINGIFY_SHADOW_CASE_ID,
            title: "Scalar JSON stringification agrees across independent routes",
            category: BehaviorCategory::StdlibRuntimeBehavior,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1249; src/backend/shadow/json_stringify_tests.rs::scalar_json_stringify_matches_the_receipt_backed_native_route; int/bool/str/None exact bytes, empty streams, and independent route receipts",
            disposition: Disposition::Preserved,
            source: JSON_STRINGIFY_SCALARS_SOURCE,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "observe",
                arguments: Vec::new,
                expected: replacement_body_v0_025_expected,
                shadow_comparison: true,
            }),
        },
        ParityCase {
            id: COLLECTION_LEN_SHADOW_CASE_ID,
            title: "Hashed set and dict length returns duplicate-normalized entry counts across independent routes",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1249; tests/replacement_collection_len_shadow_tests.rs::collection_len_matches_the_receipt_backed_native_route; canonical builtin identity, populated/duplicate/typed-empty counts, exact stdout and a separate integer result",
            disposition: Disposition::Preserved,
            source: COLLECTION_LEN_SOURCE,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "collection_len",
                arguments: Vec::new,
                expected: || ReplacementValue::Int(2200),
                shadow_comparison: true,
            }),
        },
        ParityCase {
            id: BOOL_TRUTHINESS_SHADOW_CASE_ID,
            title: "Canonical bool preserves bounded scalar and container truthiness across independent routes",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1249; tests/replacement_bool_truthiness_shadow_tests.rs::bool_truthiness_matches_the_receipt_backed_native_route; canonical builtin identity, empty/nonempty scalar and container behavior, exact stdout and a separate boolean result",
            disposition: Disposition::Preserved,
            source: BOOL_TRUTHINESS_SOURCE,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "bool_truthiness",
                arguments: Vec::new,
                expected: || ReplacementValue::Bool(true),
                shadow_comparison: true,
            }),
        },
        ParityCase {
            id: SORTED_INT_LIST_SHADOW_CASE_ID,
            title: "Canonical sorted preserves a fresh ascending nonempty integer list across independent routes",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1249; tests/replacement_sorted_int_list_shadow_tests.rs::sorted_int_list_matches_the_receipt_backed_native_route; canonical builtin identity, negative/duplicate ordering, source-list preservation, exact stdout and a separate integer result",
            disposition: Disposition::Preserved,
            source: SORTED_INT_LIST_SOURCE,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "sorted_int_list",
                arguments: Vec::new,
                expected: || ReplacementValue::Int(29_320_233),
                shadow_comparison: true,
            }),
        },
        ParityCase {
            id: TYPED_NUMERIC_SHADOW_CASE_ID,
            title: "Exact-width and decimal carriers preserve checked values across independent routes",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1279; tests/replacement_typed_numeric_tests.rs; tests/replacement_scalar_conversion_shadow_tests.rs; representative u8/i128/u128 endpoints, f32 rounding, decimal scale, exact stdout, typed cast edges, and an f32 result",
            disposition: Disposition::Preserved,
            source: REPLACEMENT_BODY_V0_029_SRC,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "typed_numeric_profile",
                arguments: Vec::new,
                expected: replacement_body_v0_029_expected,
                shadow_comparison: true,
            }),
        },
        ParityCase {
            id: ISINSTANCE_TARGETS_SHADOW_CASE_ID,
            title: "Checked primitive isinstance targets preserve true and false union narrowing across independent routes",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1281; tests/replacement_isinstance_shadow_tests.rs::checked_isinstance_targets_match_the_receipt_backed_native_route; retained compiler-owned target type/span, int/bool/str/float targets, true/false union branches, exact stdout/stderr and a separate boolean result; closed #1154 delivered the current direct nominal/value substrate and open #988 owns broader replacement execution",
            disposition: Disposition::Preserved,
            source: ISINSTANCE_TARGETS_SOURCE,
            evaluate: None,
            identity_conformance: None,
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "isinstance_targets",
                arguments: Vec::new,
                expected: || ReplacementValue::Bool(true),
                shadow_comparison: true,
            }),
        },
        ParityCase {
            id: "parity-987-1156-provider-allowed",
            title: "An allowed provider operation executes and its backend receipt references the RFC 104 receipt",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1156; tests/parity_corpus_tests.rs::case_provider_allowed_invocation; src/backend/replacement/provider.rs",
            disposition: Disposition::IntentionalMigration {
                owning_issue: 1156,
                migration_note: "New at cutover rather than preserved: the legacy backend cannot execute a \
                                  provider-service operation at all, so this is a deliberate migration from \
                                  \"refuse\" to \"execute under an RFC 104 authority decision\". The observable \
                                  contract is the provider's own result plus an operation receipt the backend \
                                  execution receipt references; generated Rust is not the contract. Comparison \
                                  stays non-green until #1146 supplies a receipt-bound paired comparison, because \
                                  there is no second route that can run this operation.",
            },
            source: PROVIDER_CASE_SRC,
            evaluate: Some(case_provider_allowed_invocation),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-1156-provider-denied",
            title: "A governed denial emits a denied receipt and never reaches the provider",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1156; tests/parity_corpus_tests.rs::case_provider_governed_denial; src/backend/replacement/provider.rs",
            disposition: Disposition::IntentionalMigration {
                owning_issue: 1156,
                migration_note: "New at cutover: a governed run refuses an ungranted capability before the \
                                  provider is reached, reports the refusal at the invocation's own source span, \
                                  and still records the denial as a receipt. Migration guidance: the denial is a \
                                  first-class recorded outcome, not an absence of one, so a consumer must read the \
                                  receipt rather than infer refusal from a missing result. Comparison stays \
                                  non-green until #1146.",
            },
            source: PROVIDER_CASE_SRC,
            evaluate: Some(case_provider_governed_denial),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-1156-provider-failed",
            title: "A provider failure keeps its allowing authority decision and reports its own diagnostic",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1156; tests/parity_corpus_tests.rs::case_provider_operation_failure; src/backend/replacement/provider.rs",
            disposition: Disposition::IntentionalMigration {
                owning_issue: 1156,
                migration_note: "New at cutover: authority was granted and the operation itself failed, which is \
                                  a different outcome from a denial and carries a different diagnostic code. \
                                  Migration guidance: a consumer must not collapse the two, because only one of \
                                  them is fixed by granting a capability. Comparison stays non-green until #1146.",
            },
            source: PROVIDER_CASE_SRC,
            evaluate: Some(case_provider_operation_failure),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-1156-provider-redacted",
            title: "A withheld provider attribute classifies its receipt as redacted without changing the result",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1156; tests/parity_corpus_tests.rs::case_provider_redaction_classification; src/backend/replacement/provider.rs",
            disposition: Disposition::IntentionalMigration {
                owning_issue: 1156,
                migration_note: "New at cutover: the publishing host decides redaction and the receipt records \
                                  the classification, so a withheld value keeps its key and sensitivity while its \
                                  value never reaches a sink. Migration guidance: redaction has exactly one owner, \
                                  and the backend must not re-derive it from the value later. Comparison stays \
                                  non-green until #1146.",
            },
            source: PROVIDER_CASE_SRC,
            evaluate: Some(case_provider_redaction_classification),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-1156-provider-cleanup",
            title: "An invocation that failed still releases what it acquired, exactly once",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "#1156; tests/parity_corpus_tests.rs::case_provider_lifecycle_cleanup; src/backend/replacement/provider.rs",
            disposition: Disposition::IntentionalMigration {
                owning_issue: 1156,
                migration_note: "New at cutover: cleanup is unconditional for an invocation that started, and \
                                  never runs for one that was denied or refused before it started. Migration \
                                  guidance: the lifecycle vocabulary is the contract a consumer reads, not the \
                                  host's internal resource handling. Comparison stays non-green until #1146.",
            },
            source: PROVIDER_CASE_SRC,
            evaluate: Some(case_provider_lifecycle_cleanup),
            identity_conformance: None,
            replacement_execution: None,
        },
        // ---- #989 public-boundary rows the replacement route cannot yet reach ----
        //
        // These are declared rather than evaluated on purpose. A package consumer needs a baked dependency and an
        // executable representation of its public surface, and neither exists yet; declaring the rows keeps the
        // boundary counted and owned instead of absent, which is what #989's disposition model asks for. Each names
        // the issue that makes it executable, so the row fails review the day that issue closes and nothing here
        // changes.
        ParityCase {
            id: "parity-987-989-package-consumer-call",
            title: "A call into a package dependency executes on a route that does not link Rust",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::PackageImportBoundary,
            evidence: "#989; RFC 123; #1339 owns the executable representation this row needs",
            disposition: Disposition::Unsupported {
                owning_issue: 1339,
                migration_note: "A package publishes signatures, checked API and canonical identities -- enough to \
                                  typecheck a call into it, and nothing a non-linking route can execute. The direct \
                                  route therefore refuses every `pub::` import, so moving a declaration into a \
                                  package removes execution routes it had as a local module. RFC 123 is Planned and \
                                  #1339 implements the representation that closes this. Until then the boundary is \
                                  unavailable rather than passing, and no result may claim package parity.",
            },
            source: PACKAGE_CONSUMER_SRC,
            evaluate: Some(case_package_consumer_call_is_refused),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-989-package-representation-refusal",
            title: "A missing or uninterpretable package representation refuses in packaging terms, before any result",
            category: BehaviorCategory::DiagnosticBehavior,
            lane: EvidenceLane::PackageImportBoundary,
            evidence: "#989; RFC 123 reference-level rules; #1339",
            disposition: Disposition::Unsupported {
                owning_issue: 1339,
                migration_note: "RFC 123 requires a consumer that cannot obtain a usable representation to refuse \
                                  before producing any result, naming the package, the version and the requirement \
                                  it did not meet -- and never to report the condition as an unsupported language \
                                  construct. Today a `pub::` import reports `import declaration`, which is the same \
                                  misdiagnosis #1262 fixed for `rust::`: it sends a reader to the language when the \
                                  problem is packaging. Owned by #1339.",
            },
            source: PACKAGE_CONSUMER_SRC,
            evaluate: Some(case_package_representation_refusal_is_not_a_language_refusal),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-120-01-identity-matrix",
            title: "RFC 120 identities survive every semantically valid binding, namespace, and scope cell",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::PackageImportBoundary,
            evidence: "RFC 120 typed Cutover conformance coverage; tests/parity_corpus_tests.rs::verify_identity_matrix",
            disposition: Disposition::Preserved,
            source: IDENTITY_MATRIX_SRC,
            evaluate: None,
            identity_conformance: Some(IdentityConformancePlan::SourceGraph(SourceIdentityConformancePlan {
                modules: IDENTITY_MATRIX_MODULES,
                root_module: "identity_matrix",
                verify: verify_identity_matrix,
                replacement: IdentityReplacementPlan::Graph {
                    root_module: "identity_matrix",
                    entrypoints: IDENTITY_MATRIX_ENTRYPOINTS,
                    deferred: IDENTITY_MATRIX_DEFERRED,
                },
                comparison_reason: "the checked graph and the cross-module replacement route both executed, but no independent legacy execution was run for a source-observable comparison",
            })),
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-120-02-let-shadow",
            title: "Explicit let shadowing preserves distinct checked and replacement local identities",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "RFC 120 Cutover conformance; tests/parity_corpus_tests.rs::verify_let_shadow; tests/replacement_backend_execution_tests.rs::replacement_executes_let_shadowing_by_local_identity",
            disposition: Disposition::Preserved,
            source: LET_SHADOW_SRC,
            evaluate: None,
            identity_conformance: Some(IdentityConformancePlan::SourceGraph(SourceIdentityConformancePlan {
                modules: LET_SHADOW_MODULES,
                root_module: "identity_let_shadow",
                verify: verify_let_shadow,
                replacement: IdentityReplacementPlan::Direct {
                    module: "identity_let_shadow",
                    function: "shadow_let",
                    arguments: no_replacement_arguments,
                    expected: expected_three,
                },
                comparison_reason: "the checked graph and replacement route executed, but no independent legacy execution was run for a source-observable comparison",
            })),
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-120-03-mut-shadow",
            title: "Explicit mut shadowing preserves distinct checked and replacement local identities",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "RFC 120 Cutover conformance; tests/parity_corpus_tests.rs::verify_mut_shadow; tests/replacement_backend_execution_tests.rs::replacement_executes_mut_shadowing_by_local_identity",
            disposition: Disposition::Preserved,
            source: MUT_SHADOW_SRC,
            evaluate: None,
            identity_conformance: Some(IdentityConformancePlan::SourceGraph(SourceIdentityConformancePlan {
                modules: MUT_SHADOW_MODULES,
                root_module: "identity_mut_shadow",
                verify: verify_mut_shadow,
                replacement: IdentityReplacementPlan::Direct {
                    module: "identity_mut_shadow",
                    function: "shadow_mut",
                    arguments: no_replacement_arguments,
                    expected: expected_eleven,
                },
                comparison_reason: "the checked graph and replacement route executed, but no independent legacy execution was run for a source-observable comparison",
            })),
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-120-04-generic-binder",
            title: "Generic binders and their callable target retain distinct canonical identities",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "RFC 120 Cutover conformance; tests/parity_corpus_tests.rs::verify_generic_binder",
            disposition: Disposition::Preserved,
            source: GENERIC_BINDER_SRC,
            evaluate: None,
            identity_conformance: Some(IdentityConformancePlan::SourceGraph(SourceIdentityConformancePlan {
                modules: GENERIC_BINDER_MODULES,
                root_module: "identity_generic",
                verify: verify_generic_binder,
                replacement: IdentityReplacementPlan::Direct {
                    module: "identity_generic",
                    function: "generic_entry",
                    arguments: no_replacement_arguments,
                    expected: expected_forty_two,
                },
                comparison_reason: "the checked graph and replacement route executed, but no independent legacy execution was run for a source-observable comparison",
            })),
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-120-05-builtin-rebinding",
            title: "Ordinary builtin-name rebinding stays distinct from explicit std.builtins lookup",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectReplacementBodyIr,
            evidence: "RFC 120 Cutover conformance; tests/parity_corpus_tests.rs::verify_builtin_rebinding",
            disposition: Disposition::Preserved,
            source: BUILTIN_REBINDING_SRC,
            evaluate: None,
            identity_conformance: Some(IdentityConformancePlan::SourceGraph(SourceIdentityConformancePlan {
                modules: BUILTIN_REBINDING_MODULES,
                root_module: "identity_builtin",
                verify: verify_builtin_rebinding,
                replacement: IdentityReplacementPlan::Direct {
                    module: "identity_builtin",
                    function: "builtin_entry",
                    arguments: no_replacement_arguments,
                    expected: expected_eight,
                },
                comparison_reason: "the checked graph and replacement route executed, but no independent legacy execution was run for a source-observable comparison",
            })),
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-120-06-release-artifact",
            title: "Pinned release artifacts recover four Incan identity categories and reject host frames",
            category: BehaviorCategory::GeneratedArtifactBehavior,
            lane: EvidenceLane::GeneratedProjectRun,
            evidence: "RFC 120 Cutover conformance; tests/support/emitted_symbol_artifact.rs::verify_pinned_release_artifact",
            disposition: Disposition::Preserved,
            source: "RFC 120 pinned Rust 1.98.0 release artifact",
            evaluate: None,
            identity_conformance: Some(IdentityConformancePlan::ReleaseArtifact {
                verify: verify_release_artifact,
                comparison_reason: "the optimized artifact was built and its symbol table inspected, but no source-observable backend execution comparison was run",
            }),
            replacement_execution: None,
        },
    ]
}

#[test]
fn rfc_120_member_coverage_rejects_a_consistent_wrong_owner_selection() {
    let case = ParityCase {
        id: "parity-987-120-negative-wrong-owner",
        title: "Wrong-owner member identity must fail conformance",
        category: BehaviorCategory::SupportedLanguageContract,
        lane: EvidenceLane::PackageImportBoundary,
        evidence: "tests/parity_corpus_tests.rs::verify_wrong_owner_member_selection",
        disposition: Disposition::Preserved,
        source: IDENTITY_MATRIX_SRC,
        evaluate: None,
        identity_conformance: Some(IdentityConformancePlan::SourceGraph(SourceIdentityConformancePlan {
            modules: IDENTITY_MATRIX_MODULES,
            root_module: "identity_matrix",
            verify: verify_wrong_owner_member_selection,
            replacement: IdentityReplacementPlan::Unavailable {
                owning_issue: 1332,
                reason: "a negative fixture is rejected during conformance, so no route runs it; #1332 owns the paired reference route these rows would need if they ever did",
            },
            comparison_reason: "negative conformance fixture must fail before execution",
        })),
        replacement_execution: None,
    };
    let report = parity_corpus::evaluate_case(&case);
    assert_eq!(report.overall_state, OverallState::NonGreenBehavior);
    assert!(
        matches!(
            report.behavior_outcome,
            ComparisonOutcome::Mismatch { ref detail } if detail.contains("outside owner")
        ),
        "wrong-owner member selection must fail on exact declaration spans: {:?}",
        report.behavior_outcome
    );
}

#[test]
fn rfc_120_module_path_coverage_rejects_a_consistent_wrong_target_selection() {
    let case = ParityCase {
        id: "parity-987-120-negative-wrong-path-target",
        title: "Wrong-target module-path identity must fail conformance",
        category: BehaviorCategory::SupportedLanguageContract,
        lane: EvidenceLane::PackageImportBoundary,
        evidence: "tests/parity_corpus_tests.rs::verify_wrong_path_target_selection",
        disposition: Disposition::Preserved,
        source: IDENTITY_MATRIX_SRC,
        evaluate: None,
        identity_conformance: Some(IdentityConformancePlan::SourceGraph(SourceIdentityConformancePlan {
            modules: IDENTITY_MATRIX_MODULES,
            root_module: "identity_matrix",
            verify: verify_wrong_path_target_selection,
            replacement: IdentityReplacementPlan::Unavailable {
                owning_issue: 1332,
                reason: "a negative fixture is rejected during conformance, so no route runs it; #1332 owns the paired reference route these rows would need if they ever did",
            },
            comparison_reason: "negative conformance fixture must fail before execution",
        })),
        replacement_execution: None,
    };
    let report = parity_corpus::evaluate_case(&case);
    assert_eq!(report.overall_state, OverallState::NonGreenBehavior);
    assert!(
        matches!(
            report.behavior_outcome,
            ComparisonOutcome::Mismatch { ref detail }
                if detail.contains("path/module reconstructed or selected the wrong identity")
        ),
        "module-path selection must fail on the exact expected origin and name: {:?}",
        report.behavior_outcome
    );
}

#[test]
fn rfc_120_emitted_projection_requires_an_exact_rust_identifier_token() -> Result<(), Box<dyn std::error::Error>> {
    let projection = "__incan_v1_001122";
    let lookalikes =
        format!("fn {projection}_suffix() {{}}\n// fn {projection}() {{}}\nconst TEXT: &str = \"{projection}\";");
    assert!(
        exact_rust_identifier(&lookalikes, projection).is_err(),
        "prefixes, comments, and string literals must not prove an emitted projection"
    );
    let emitted = format!("fn {projection}() {{}}");
    assert_eq!(exact_rust_identifier(&emitted, projection)?, projection);
    Ok(())
}

#[test]
fn rfc_120_typed_coverage_rejects_invalid_scope_and_missing_carriers() {
    let invalid_scope = IdentityCoverageCell {
        binding: IdentityBindingForm::Local,
        namespace: IdentityNamespace::Member,
        scope: IdentityScope::Module,
        checked_identity: "member".to_string(),
        hir_identity: None,
        body_ir_identity: None,
        emitted_projection: Some("projection".to_string()),
    };
    assert!(
        matches!(
            validate_identity_coverage(&[invalid_scope]),
            Err(detail) if detail.contains("not a semantically valid namespace/scope combination")
        ),
        "member declarations must use owner scope"
    );

    let missing_hir = IdentityCoverageCell {
        binding: IdentityBindingForm::Import,
        namespace: IdentityNamespace::Lexical,
        scope: IdentityScope::Module,
        checked_identity: "callable".to_string(),
        hir_identity: None,
        body_ir_identity: None,
        emitted_projection: Some("projection".to_string()),
    };
    assert!(
        matches!(
            validate_identity_coverage(&[missing_hir]),
            Err(detail) if detail.contains("invalid HIR carrier presence")
        ),
        "a module-scope lexical cell must prove its HIR carrier"
    );

    let missing_projection = IdentityCoverageCell {
        binding: IdentityBindingForm::ReExport,
        namespace: IdentityNamespace::Member,
        scope: IdentityScope::Owner,
        checked_identity: "member".to_string(),
        hir_identity: None,
        body_ir_identity: None,
        emitted_projection: None,
    };
    assert!(
        matches!(
            validate_identity_coverage(&[missing_projection]),
            Err(detail) if detail.contains("invalid emitted projection carrier presence")
        ),
        "an owner-scope member cell must prove the linker's source-declaration projection"
    );
}

#[test]
fn rfc_120_rows_publish_real_conformance_evidence_without_fabricating_legacy_execution()
-> Result<(), Box<dyn std::error::Error>> {
    let summary = parity_corpus::summarize(&seed_corpus());
    let rows = summary
        .cases
        .iter()
        .filter(|row| row.id.starts_with("parity-987-120-"))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 6, "the stable RFC 120 corpus rows must remain complete");
    for row in &rows {
        assert_eq!(
            row.overall_state,
            OverallState::NonGreenShadowUnavailable,
            "{} produced the wrong state from {:?}",
            row.id,
            row.behavior_outcome
        );
        let ReceiptRef::IdentityConformanceObserved {
            replacement_receipt_identity,
            evidence_identity,
            comparison_reason,
        } = &row.receipt
        else {
            return Err(format!(
                "{} lost its identity-conformance observation: {:?}",
                row.id, row.receipt
            )
            .into());
        };
        assert!(!comparison_reason.is_empty());
        let evidence = row
            .identity_conformance
            .as_ref()
            .ok_or_else(|| format!("{} omitted its conformance evidence", row.id))?;
        match (&evidence.subject, row.id) {
            (IdentityConformanceSubject::SourceGraph { graph_identity }, id)
                if id != "parity-987-120-06-release-artifact" =>
            {
                assert!(!graph_identity.is_empty());
            }
            (
                IdentityConformanceSubject::ReleaseArtifact {
                    fixture_input_identity,
                    artifact_content_identity,
                    recovered_observation_identity,
                },
                "parity-987-120-06-release-artifact",
            ) => {
                for identity in [
                    fixture_input_identity,
                    artifact_content_identity,
                    recovered_observation_identity,
                ] {
                    assert!(identity.starts_with("sha256:"));
                }
            }
            (subject, id) => return Err(format!("{id} reported the wrong conformance subject: {subject:?}").into()),
        }
        assert_eq!(
            identity_conformance_evidence_identity(evidence),
            *evidence_identity,
            "{} published an evidence identity that cannot be recomputed from its report",
            row.id
        );
        assert_eq!(
            evidence.evidence_identity, *evidence_identity,
            "{} split the receipt and report evidence identities",
            row.id
        );
        let mut tampered = evidence.clone();
        tampered.checked_relations.push("tampered checked relation".to_string());
        assert_ne!(
            identity_conformance_evidence_identity(&tampered),
            *evidence_identity,
            "{} evidence digest ignored a serialized checked relation",
            row.id
        );
        let mut tampered_subject = evidence.clone();
        match &mut tampered_subject.subject {
            IdentityConformanceSubject::SourceGraph { graph_identity } => graph_identity.push_str("-tampered"),
            IdentityConformanceSubject::ReleaseArtifact {
                artifact_content_identity,
                ..
            } => artifact_content_identity.push_str("-tampered"),
        }
        assert_ne!(
            identity_conformance_evidence_identity(&tampered_subject),
            *evidence_identity,
            "{} evidence digest ignored its typed conformance subject",
            row.id
        );
        if matches!(
            row.id,
            "parity-987-120-01-identity-matrix"
                | "parity-987-120-02-let-shadow"
                | "parity-987-120-03-mut-shadow"
                | "parity-987-120-04-generic-binder"
                | "parity-987-120-05-builtin-rebinding"
        ) {
            assert!(
                replacement_receipt_identity
                    .as_deref()
                    .is_some_and(|identity| identity.starts_with("sha256:"))
            );
            assert!(evidence.replacement_output_identity.is_some());
        } else {
            assert_eq!(replacement_receipt_identity, &None);
            assert_eq!(evidence.replacement_output_identity, None);
        }
    }

    let matrix = rows
        .iter()
        .find(|row| row.id == "parity-987-120-01-identity-matrix")
        .and_then(|row| row.identity_conformance.as_ref())
        .ok_or("RFC 120 identity matrix evidence is missing")?;
    assert_eq!(matrix.coverage_cells.len(), 26);
    // The matrix used to record #989 as owning an unavailable replacement route. #1260 and #1261 made cross-module
    // execution real, so the row now carries an executed output identity instead of an owner for its absence.
    assert_eq!(matrix.replacement_unavailable_issue, None);

    let artifact = rows
        .iter()
        .find(|row| row.id == "parity-987-120-06-release-artifact")
        .and_then(|row| row.identity_conformance.as_ref())
        .ok_or("RFC 120 release-artifact evidence is missing")?;
    assert_eq!(artifact.legacy_projections.len(), 4);
    assert!(
        artifact
            .artifact_observations
            .iter()
            .any(|item| item.contains("non-Incan"))
    );
    Ok(())
}

#[test]
fn rfc_120_expected_value_mismatch_retains_the_observed_replacement_receipt_and_evidence() {
    let case = ParityCase {
        id: "parity-987-120-negative-expected-value",
        title: "A completed replacement mismatch retains its execution evidence",
        category: BehaviorCategory::SupportedLanguageContract,
        lane: EvidenceLane::DirectReplacementBodyIr,
        evidence: "tests/parity_corpus_tests.rs::rfc_120_expected_value_mismatch_retains_the_observed_replacement_receipt_and_evidence",
        disposition: Disposition::Preserved,
        source: GENERIC_BINDER_SRC,
        evaluate: None,
        identity_conformance: Some(IdentityConformancePlan::SourceGraph(SourceIdentityConformancePlan {
            modules: GENERIC_BINDER_MODULES,
            root_module: "identity_generic",
            verify: verify_generic_binder,
            replacement: IdentityReplacementPlan::Direct {
                module: "identity_generic",
                function: "generic_entry",
                arguments: no_replacement_arguments,
                expected: expected_three,
            },
            comparison_reason: "negative fixture executed one replacement route only",
        })),
        replacement_execution: None,
    };
    let report = parity_corpus::evaluate_case(&case);
    assert_eq!(report.overall_state, OverallState::NonGreenBehavior);
    assert!(
        matches!(
            report.behavior_outcome,
            ComparisonOutcome::Mismatch { ref detail }
                if detail.contains("returned Int(42), expected Int(3)")
        ),
        "unexpected mismatch outcome: {:?}",
        report.behavior_outcome
    );
    assert!(matches!(
        report.receipt,
        ReceiptRef::IdentityConformanceObserved {
            replacement_receipt_identity: Some(ref identity),
            ..
        } if identity.starts_with("sha256:")
    ));
    assert!(
        report
            .identity_conformance
            .as_ref()
            .is_some_and(|evidence| evidence.replacement_output_identity.is_some()),
        "completed mismatch must retain the output identity that its receipt finalized"
    );
}

/// The #1156 provider paths each carry a stable disposition and none of them claims a comparison it cannot support.
///
/// Stated as its own test rather than left to the aggregate green-count assertion: the "every path has a stable
/// #987 disposition" contract is about these five rows specifically, and an aggregate count would still pass if one
/// of them quietly disappeared.
#[test]
fn every_provider_path_carries_a_stable_non_green_disposition() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = seed_corpus();
    let expected = [
        "parity-987-1156-provider-allowed",
        "parity-987-1156-provider-denied",
        "parity-987-1156-provider-failed",
        "parity-987-1156-provider-redacted",
        "parity-987-1156-provider-cleanup",
    ];
    for id in expected {
        let case = corpus
            .iter()
            .find(|case| case.id == id)
            .ok_or(format!("the #1156 corpus row `{id}` must remain in the corpus"))?;
        match &case.disposition {
            Disposition::IntentionalMigration { owning_issue, .. } if *owning_issue == 1156 => {}
            disposition => {
                return Err(
                    format!("`{id}` must be an intentional migration owned by #1156, got {disposition:?}").into(),
                );
            }
        }
    }

    let summary = parity_corpus::summarize(&seed_corpus());
    for id in expected {
        let report = summary
            .cases
            .iter()
            .find(|case| case.id == id)
            .ok_or(format!("the summary must report `{id}`"))?;
        assert_eq!(
            report.overall_state,
            OverallState::NonGreenShadowUnavailable,
            "`{id}` must stay non-green until #1146 supplies a receipt-bound paired comparison",
        );
    }
    Ok(())
}

// ============================================================================
// Red-state proof: the schema must surface gaps, not default to green
// ============================================================================

/// Build cases that are individually malformed in a distinct way, to prove [`validate_corpus`] catches each
/// problem rather than letting a broken case pass silently. These are never added to [`seed_corpus`].
fn malformed_cases_for_red_state_proof() -> Vec<ParityCase> {
    vec![
        ParityCase {
            id: "parity-987-dup",
            title: "First of a duplicate pair",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "tests/parity_corpus_tests.rs (red-state fixture)",
            disposition: Disposition::Preserved,
            // Never reaches `evaluate_case` — `red_state_validate_corpus_...` calls
            // `validate_corpus` directly, so this placeholder source is never evaluated into an observation.
            source: "",
            evaluate: Some(|| ComparisonOutcome::Match),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-dup",
            title: "Second of a duplicate pair (same id — must be flagged)",
            category: BehaviorCategory::SupportedLanguageContract,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "tests/parity_corpus_tests.rs (red-state fixture)",
            disposition: Disposition::Preserved,
            // Never reaches `evaluate_case` — `red_state_validate_corpus_...` calls
            // `validate_corpus` directly, so this placeholder source is never evaluated into an observation.
            source: "",
            evaluate: Some(|| ComparisonOutcome::Match),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-empty-title",
            title: "",
            category: BehaviorCategory::DiagnosticBehavior,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "tests/parity_corpus_tests.rs (red-state fixture)",
            disposition: Disposition::Preserved,
            // Never reaches `evaluate_case` — `red_state_validate_corpus_...` calls
            // `validate_corpus` directly, so this placeholder source is never evaluated into an observation.
            source: "",
            evaluate: Some(|| ComparisonOutcome::Match),
            identity_conformance: None,
            replacement_execution: None,
        },
        ParityCase {
            id: "parity-987-unsupported-no-issue",
            title: "Unsupported disposition missing an owning issue and note",
            category: BehaviorCategory::AccidentalAcceptedBehavior,
            lane: EvidenceLane::DirectParserTypechecker,
            evidence: "tests/parity_corpus_tests.rs (red-state fixture)",
            disposition: Disposition::Unsupported {
                owning_issue: 0,
                migration_note: "",
            },
            // Never reaches `evaluate_case` — `red_state_validate_corpus_...` calls
            // `validate_corpus` directly, so this placeholder source is never evaluated into an observation.
            source: "",
            evaluate: Some(|| ComparisonOutcome::Match),
            identity_conformance: None,
            replacement_execution: None,
        },
    ]
}

#[test]
fn red_state_validate_corpus_catches_duplicate_ids_missing_titles_and_unowned_dispositions() {
    let violations = validate_corpus(&malformed_cases_for_red_state_proof());

    let has_violation_matching = |case_id: &str, needle: &str| {
        violations
            .iter()
            .any(|v| v.case_id == case_id && v.problem.contains(needle))
    };

    assert!(
        has_violation_matching("parity-987-dup", "duplicate"),
        "expected a duplicate-id violation, got: {violations:?}"
    );
    assert!(
        has_violation_matching("parity-987-empty-title", "title"),
        "expected an empty-title violation, got: {violations:?}"
    );
    assert!(
        has_violation_matching("parity-987-unsupported-no-issue", "owning issue"),
        "expected a missing-owning-issue violation, got: {violations:?}"
    );
    assert!(
        has_violation_matching("parity-987-unsupported-no-issue", "migration note"),
        "expected a missing-migration-note violation, got: {violations:?}"
    );
    // Four distinct cases, at least one violation each (duplicate id reports on the second occurrence only).
    assert!(
        violations.len() >= 4,
        "expected at least 4 violations across the malformed fixtures, got {}: {violations:?}",
        violations.len()
    );
}

// ============================================================================
// Green-state proof: the real seed corpus is structurally sound and behaviorally confirmed
// ============================================================================

#[test]
fn seed_corpus_has_no_structural_violations() {
    let violations = validate_corpus(&seed_corpus());
    assert!(
        violations.is_empty(),
        "seed corpus has structural violations: {violations:?}"
    );
}

#[test]
fn seed_corpus_ids_are_stable_and_globally_unique() {
    let ids: Vec<&str> = seed_corpus().iter().map(|c| c.id).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "seed corpus case ids must be globally unique");
    for id in &ids {
        assert!(
            id.starts_with("parity-987-") || id.starts_with("replacement-body-v0-"),
            "case id {id} must carry a stable #987 or #988 replacement-body namespace prefix"
        );
    }
}

#[test]
fn seed_corpus_every_case_confirms_its_documented_current_behavior() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = seed_corpus();
    let summary = parity_corpus::summarize(&corpus);
    let regressions: Vec<&parity_corpus::CaseReport> = summary
        .cases
        .iter()
        .filter(|c| !c.behavior_outcome.is_green())
        .collect();
    assert!(
        regressions.is_empty(),
        "seed corpus cases whose evaluate() no longer confirms the documented current behavior (this means the \
         compiler's actual behavior drifted from what this corpus recorded — update the case or investigate the \
         regression, do not silently accept it): {regressions:#?}"
    );
    for case in corpus.iter().filter(|case| case.evaluate.is_some()) {
        let report = summary
            .cases
            .iter()
            .find(|report| report.id == case.id)
            .ok_or_else(|| format!("summary omitted callback row `{}`", case.id))?;
        let ReceiptRef::BehaviorObserved {
            evidence_identity,
            comparison_reason,
        } = &report.receipt
        else {
            return Err(format!(
                "callback row `{}` fabricated or borrowed an execution receipt: {:?}",
                case.id, report.receipt
            )
            .into());
        };
        assert_eq!(
            behavior_observation_identity(case.id, case.evidence, case.source, &report.behavior_outcome),
            *evidence_identity,
            "callback row `{}` did not bind its actual outcome into evidence",
            case.id
        );
        assert_ne!(
            behavior_observation_identity(
                case.id,
                case.evidence,
                case.source,
                &ComparisonOutcome::Mismatch {
                    detail: "tampered callback outcome".to_string(),
                },
            ),
            *evidence_identity,
            "callback row `{}` evidence ignored its observed outcome",
            case.id
        );
        assert!(!comparison_reason.is_empty());
    }
    Ok(())
}

#[test]
fn only_rows_with_real_two_route_comparisons_can_be_green() -> Result<(), Box<dyn std::error::Error>> {
    // This is the corpus's core promise: direct replacement execution does not become green parity merely because
    // it has a receipt, and generated Rust never counts as proof. Only rows that declare the bounded #1146
    // comparison profile are green, and each is green only when that comparison actually ran through Oven and
    // agreed.
    //
    // The branch is taken on what the summary reports, not on whether a capability could be *resolved*: a staged
    // capability whose Oven build then fails has run no comparison, and must not be treated as if it had.
    let summary = parity_corpus::summarize(&seed_corpus());
    assert!(
        summary.execution_receipt_schema_available,
        "the summary must say the #986 execution-receipt schema is available now that PR #1120 landed it"
    );
    assert!(summary.cases_with_execution_receipts > 0);
    assert!(
        summary.cases_with_execution_receipts < summary.total_cases,
        "callback and artifact-only observations must not be counted as execution receipts"
    );
    assert_eq!(summary.non_green_shadow_diverged, 0);
    assert_eq!(summary.non_green_behavior, 0);

    let green: Vec<&str> = summary
        .cases
        .iter()
        .filter(|case| case.overall_state == OverallState::Green)
        .map(|case| case.id)
        .collect();

    if summary.source_observable_comparison_available {
        assert_eq!(
            green, SHADOW_COMPARED_CASE_IDS,
            "each selected row needs its own proven comparison; one matched row must not hide another unavailable row"
        );
        assert_eq!(summary.green, SHADOW_COMPARED_CASE_IDS.len());
        assert_eq!(
            summary.non_green_shadow_unavailable,
            summary.total_cases - SHADOW_COMPARED_CASE_IDS.len()
        );
    } else {
        // No comparison ran, so nothing may be green — including rows that declare one.
        require_staging_when_demanded(&summary)?;
        assert!(
            green.is_empty(),
            "no row may be green without a real comparison: {green:?}"
        );
        assert_eq!(summary.green, 0);
        assert_eq!(summary.non_green_shadow_unavailable, summary.total_cases);
    }
    Ok(())
}

/// Fail rather than report a skip when this environment declares that a comparison must have run.
///
/// Reads the reason straight off the compared row, so the failure says what actually stopped the comparison.
fn require_staging_when_demanded(summary: &parity_corpus::CorpusSummary) -> Result<(), Box<dyn std::error::Error>> {
    let reason = compared_row(summary)
        .map(|row| match &row.receipt {
            ReceiptRef::ReplacementExecuted { comparison_reason, .. } => comparison_reason.clone(),
            receipt => format!("{receipt:?}"),
        })
        .unwrap_or_else(|| "the compared row is missing from the corpus".to_string());
    assert!(
        !shadow_capability::legacy_route_is_required(),
        "{} is set but no source-observable comparison ran: {reason}",
        shadow_capability::REQUIRE_LEGACY_ROUTE_ENV
    );
    eprintln!("no source-observable comparison ran: {reason}");
    Ok(())
}

/// The original scalar row that declares the bounded #1146 comparison profile.
fn compared_row(summary: &parity_corpus::CorpusSummary) -> Option<&parity_corpus::CaseReport> {
    summary.cases.iter().find(|case| case.id == SHADOW_COMPARED_CASE_ID)
}

/// Canonical Enumerate/Zip bind exact source output and an integer result to two independent route receipts.
#[test]
fn the_enumerate_zip_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == ENUMERATE_ZIP_SHADOW_CASE_ID)
        .ok_or("missing Enumerate/Zip comparison row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        profile_kind,
        profile_identity,
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
    } = &row.receipt
    else {
        return Err(format!("Enumerate/Zip needs matched two-route evidence, got {:?}", row.receipt).into());
    };
    let stdout = b"left\nleft\nright\npair\npair\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(profile_kind, incan::backend::shadow::SHADOW_COMPARISON_PROFILE_ID);
    assert!(profile_identity.starts_with("sha256:"));
    assert_eq!(
        observable,
        &format!(
            "completed(Int, \"49\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(
        !legacy_authority.cargo_process_started,
        "the native observation must be attributable to Oven rather than a Cargo process"
    );
    Ok(())
}

/// The compared row's evidence must name both routes' receipts and the Oven authority behind the legacy one.
#[test]
fn the_compared_row_carries_two_route_receipts_and_its_oven_authority() -> Result<(), Box<dyn std::error::Error>> {
    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = compared_row(&summary).ok_or("the compared row must be present in the corpus")?;
    assert_eq!(row.overall_state, OverallState::Green);

    let ReceiptRef::ShadowMatched {
        profile_kind,
        profile_identity,
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
    } = &row.receipt
    else {
        return Err(format!(
            "{} must carry matched two-route evidence, got {:?}",
            row.id, row.receipt
        )
        .into());
    };
    // #1153 links on the stable kind and cites the instance identity; a receipt must carry both.
    assert_eq!(profile_kind, incan::backend::shadow::SHADOW_COMPARISON_PROFILE_ID);
    assert!(profile_identity.starts_with("sha256:"));
    let empty_stream_digest = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    assert_eq!(
        observable,
        &format!(
            "completed(Int, \"42\"); stdout=0 bytes ({empty_stream_digest}); stderr=0 bytes ({empty_stream_digest})"
        )
    );
    assert!(legacy_receipt_identity.starts_with("sha256:"));
    assert!(replacement_receipt_identity.starts_with("sha256:"));
    assert_ne!(
        legacy_receipt_identity, replacement_receipt_identity,
        "the two routes' receipts differ by selected and executed backend and must not be conflated"
    );
    assert_ne!(
        legacy_output_identity, replacement_output_identity,
        "each route's output identity must cover what that route actually produced"
    );

    // The legacy answer is attributable to a real Oven build, not an ad-hoc compiler invocation.
    assert!(legacy_authority.oven_receipt_identity.starts_with("sha256:"));
    assert!(legacy_authority.oven_build_unit_identity.starts_with("sha256:"));
    assert!(legacy_authority.direct_rustc_plan_identity.starts_with("sha256:"));
    assert!(
        !legacy_authority.cargo_process_started,
        "Oven-owned legacy execution must not start a Cargo process"
    );
    Ok(())
}

/// Hash membership binds exact program output and its typed result to two independent route receipts.
#[test]
fn the_hashed_membership_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == HASHED_SHADOW_CASE_ID)
        .ok_or("missing hashed membership row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        profile_kind,
        profile_identity,
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
    } = &row.receipt
    else {
        return Err(format!(
            "hashed membership needs matched two-route evidence, got {:?}",
            row.receipt
        )
        .into());
    };
    assert_eq!(profile_kind, incan::backend::shadow::SHADOW_COMPARISON_PROFILE_ID);
    assert_eq!(
        observable,
        "completed(Bool, \"true\"); stdout=18 bytes (sha256:25eebc99ccbd29d7f5bb03931768c3c19a466df57a8c3deddcd7a7e1830ab04a); stderr=0 bytes (sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855)"
    );
    for identity in [
        profile_identity,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// The selected string row binds the typed result and both exact streams to independent no-fallback receipts.
#[test]
fn the_string_helper_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == STRING_HELPER_SHADOW_CASE_ID)
        .ok_or("missing selected string-helper row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
        ..
    } = &row.receipt
    else {
        return Err(format!("string helpers need matched two-route evidence, got {:?}", row.receipt).into());
    };
    let stdout = b"string helper checks\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(
        observable,
        &format!(
            "completed(Bool, \"true\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// Scalar JSON binds its exact returned bytes and empty program streams to two independently verified receipts.
#[test]
fn the_scalar_json_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == JSON_STRINGIFY_SHADOW_CASE_ID)
        .ok_or("missing scalar JSON row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        profile_kind,
        profile_identity,
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
    } = &row.receipt
    else {
        return Err(format!("scalar JSON needs matched two-route evidence, got {:?}", row.receipt).into());
    };
    let empty_stream_digest = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    assert_eq!(profile_kind, incan::backend::shadow::SHADOW_COMPARISON_PROFILE_ID);
    assert_eq!(
        observable,
        &format!(
            "completed(Str, {:?}); stdout=0 bytes ({empty_stream_digest}); stderr=0 bytes ({empty_stream_digest})",
            JSON_STRINGIFY_SCALARS_EXPECTED
        )
    );
    for identity in [
        profile_identity,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// Hashed entry count binds duplicate normalization and exact streams to two independently verified receipts.
#[test]
fn the_collection_len_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == COLLECTION_LEN_SHADOW_CASE_ID)
        .ok_or("missing collection-length row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
        ..
    } = &row.receipt
    else {
        return Err(format!(
            "collection length needs matched two-route evidence, got {:?}",
            row.receipt
        )
        .into());
    };
    let stdout = b"collection len\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(
        observable,
        &format!(
            "completed(Int, \"2200\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// Canonical truthiness binds its bounded carrier result and exact streams to independently verified receipts.
#[test]
fn the_bool_truthiness_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == BOOL_TRUTHINESS_SHADOW_CASE_ID)
        .ok_or("missing bool-truthiness row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
        ..
    } = &row.receipt
    else {
        return Err(format!(
            "bool truthiness needs matched two-route evidence, got {:?}",
            row.receipt
        )
        .into());
    };
    let stdout = b"bool truthiness\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(
        observable,
        &format!(
            "completed(Bool, \"true\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// Integer-list sorting binds order, source preservation, and exact streams to independently verified receipts.
#[test]
fn the_sorted_int_list_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == SORTED_INT_LIST_SHADOW_CASE_ID)
        .ok_or("missing sorted-integer-list row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
        ..
    } = &row.receipt
    else {
        return Err(format!(
            "sorted integer list needs matched two-route evidence, got {:?}",
            row.receipt
        )
        .into());
    };
    let stdout = b"sorted int list\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(
        observable,
        &format!(
            "completed(Int, \"29320233\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// The typed-numeric row binds exact carrier identity, decimal scale, f32 rounding, and streams to both receipts.
#[test]
fn the_typed_numeric_row_carries_exact_type_and_two_route_receipts() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == TYPED_NUMERIC_SHADOW_CASE_ID)
        .ok_or("missing typed-numeric row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        profile_kind,
        profile_identity,
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
    } = &row.receipt
    else {
        return Err(format!("typed numerics need matched two-route evidence, got {:?}", row.receipt).into());
    };
    let stdout = b"0 255 -170141183460469231731687303715884105728 340282366920938463463374607431768211455 19.90\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(profile_kind, incan::backend::shadow::SHADOW_COMPARISON_PROFILE_ID);
    assert!(profile_identity.starts_with("sha256:"));
    assert_eq!(
        observable,
        &format!(
            "completed(Numeric(F32), \"1.2345679\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// Checked `isinstance` targets bind their exact type-test output to independently verified route receipts.
#[test]
fn the_isinstance_targets_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == ISINSTANCE_TARGETS_SHADOW_CASE_ID)
        .ok_or("missing checked-isinstance-target row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
        ..
    } = &row.receipt
    else {
        return Err(format!(
            "checked isinstance targets need matched two-route evidence, got {:?}",
            row.receipt
        )
        .into());
    };
    let stdout = b"isinstance targets\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(
        observable,
        &format!(
            "completed(Bool, \"true\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// The string-length row binds Unicode behavior and both exact streams to independent no-fallback receipts.
#[test]
fn the_string_len_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == STRING_LEN_SHADOW_CASE_ID)
        .ok_or("missing string-length row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
        ..
    } = &row.receipt
    else {
        return Err(format!("string length needs matched two-route evidence, got {:?}", row.receipt).into());
    };
    let stdout = b"string len\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(
        observable,
        &format!(
            "completed(Bool, \"true\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// Scalar conversions bind a typed `str` result and their visible output to two independent route receipts.
#[test]
fn the_scalar_conversions_row_carries_two_route_receipts_and_exact_output() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let summary = parity_corpus::summarize(&seed_corpus());
    if !summary.source_observable_comparison_available {
        return require_staging_when_demanded(&summary);
    }
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == SCALAR_CONVERSIONS_SHADOW_CASE_ID)
        .ok_or("missing scalar-conversions comparison row")?;
    assert_eq!(row.overall_state, OverallState::Green);
    let ReceiptRef::ShadowMatched {
        profile_kind,
        profile_identity,
        observable,
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        legacy_authority,
    } = &row.receipt
    else {
        return Err(format!(
            "scalar conversions need matched two-route evidence, got {:?}",
            row.receipt
        )
        .into());
    };
    let stdout = b"converted: 42 3.14 10\n";
    let stdout_digest = format!("sha256:{:x}", Sha256::digest(stdout));
    let stderr_digest = format!("sha256:{:x}", Sha256::digest(b""));
    assert_eq!(profile_kind, incan::backend::shadow::SHADOW_COMPARISON_PROFILE_ID);
    assert!(profile_identity.starts_with("sha256:"));
    assert_eq!(
        observable,
        &format!(
            "completed(Str, \"42 3.14 10\"); stdout={} bytes ({stdout_digest}); stderr=0 bytes ({stderr_digest})",
            stdout.len()
        )
    );
    for identity in [
        legacy_receipt_identity,
        replacement_receipt_identity,
        legacy_output_identity,
        replacement_output_identity,
        &legacy_authority.oven_receipt_identity,
        &legacy_authority.oven_build_unit_identity,
        &legacy_authority.direct_rustc_plan_identity,
    ] {
        assert!(identity.starts_with("sha256:"), "{identity}");
    }
    assert_ne!(legacy_receipt_identity, replacement_receipt_identity);
    assert_ne!(legacy_output_identity, replacement_output_identity);
    assert!(!legacy_authority.cargo_process_started);
    Ok(())
}

/// A comparison that could not run still leaves the row the replacement execution it really performed.
#[test]
fn an_unavailable_comparison_keeps_the_rows_replacement_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let summary = parity_corpus::summarize(&seed_corpus());
    if summary.source_observable_comparison_available {
        eprintln!("skipping: the comparison ran, so this row reports agreement rather than degraded evidence");
        return Ok(());
    }
    let row = compared_row(&summary).ok_or("the compared row must be present in the corpus")?;

    assert_eq!(row.overall_state, OverallState::NonGreenShadowUnavailable);
    let ReceiptRef::ReplacementExecuted {
        receipt_identity,
        body_snapshot,
        comparison_reason,
        ..
    } = &row.receipt
    else {
        return Err(format!(
            "an unavailable comparison must still report the replacement execution that ran, got {:?}",
            row.receipt
        )
        .into());
    };
    assert!(receipt_identity.starts_with("sha256:"));
    assert!(
        body_snapshot.contains("body add"),
        "the retained evidence must be the real Body-IR execution: {body_snapshot}"
    );
    assert!(!comparison_reason.is_empty(), "the row must say why no comparison ran");
    Ok(())
}

/// An unstaged Enumerate/Zip comparison remains explicitly non-green while retaining its direct receipt evidence.
#[test]
fn an_unavailable_enumerate_zip_comparison_keeps_its_replacement_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let summary = parity_corpus::summarize(&seed_corpus());
    let row = summary
        .cases
        .iter()
        .find(|row| row.id == ENUMERATE_ZIP_SHADOW_CASE_ID)
        .ok_or("the Enumerate/Zip comparison row must be present in the corpus")?;
    if matches!(&row.receipt, ReceiptRef::ShadowMatched { .. }) {
        eprintln!(
            "skipping: the Enumerate/Zip comparison ran, so this row reports agreement rather than degraded evidence"
        );
        return Ok(());
    }
    assert_eq!(row.overall_state, OverallState::NonGreenShadowUnavailable);
    let ReceiptRef::ReplacementExecuted {
        receipt_identity,
        body_snapshot,
        comparison_reason,
        ..
    } = &row.receipt
    else {
        return Err(format!(
            "an unavailable Enumerate/Zip comparison must retain direct replacement evidence, got {:?}",
            row.receipt
        )
        .into());
    };
    assert!(receipt_identity.starts_with("sha256:"));
    assert!(
        body_snapshot.contains("body enumerate_zip_profile"),
        "the retained evidence must be the real Enumerate/Zip Body-IR execution: {body_snapshot}"
    );
    assert!(
        !comparison_reason.is_empty(),
        "the row must name why its requested native comparison did not run"
    );
    Ok(())
}

/// Bind each selected direct-replacement source case to its own receipt and complete Body-IR proof evidence.
#[test]
fn replacement_body_v0_cases_have_receipt_bound_non_green_execution_evidence() -> Result<(), Box<dyn std::error::Error>>
{
    let summary = parity_corpus::summarize(&seed_corpus());
    let replacement_rows: Vec<&parity_corpus::CaseReport> = summary
        .cases
        .iter()
        .filter(|case| case.id.starts_with("replacement-body-v0-"))
        .collect();
    assert_eq!(
        replacement_rows.len(),
        30,
        "the nineteen original direct cases plus hashed membership, selected string helpers, scalar conversions, canonical Enumerate/Zip, string length, scalar JSON, hashed collection length, bounded bool truthiness, nonempty integer-list sorting, typed numeric carriers and checked isinstance targets must stay stable in #987"
    );
    let nominal_row = replacement_rows
        .iter()
        .find(|row| row.id == "replacement-body-v0-013")
        .ok_or("the #1154 nominal Body-IR row must remain in the corpus")?;
    let ReceiptRef::ReplacementExecuted { body_snapshot, .. } = &nominal_row.receipt else {
        return Err("the #1154 nominal Body-IR row must retain a direct execution receipt".into());
    };
    assert!(
        body_snapshot.contains("executed nominal constructor name=Pair id=decl:")
            && body_snapshot.contains("fields=[left, right]"),
        "the #1154 nominal row must bind its receipt evidence to the retained declaration identity and canonical layout: {body_snapshot}"
    );
    let value_enum_row = replacement_rows
        .iter()
        .find(|row| row.id == "replacement-body-v0-014")
        .ok_or("the #1154 value-enum Body-IR row must remain in the corpus")?;
    let ReceiptRef::ReplacementExecuted { body_snapshot, .. } = &value_enum_row.receipt else {
        return Err("the #1154 value-enum Body-IR row must retain a direct execution receipt".into());
    };
    assert!(
        body_snapshot.contains("executed value-enum variant name=HttpStatus::NotFound enum_id=decl:")
            && body_snapshot.contains("raw=404")
            && body_snapshot.contains("extracted value-enum scalar name=HttpStatus::NotFound"),
        "the #1154 value-enum row must bind receipt evidence to retained enum/member identities and scalar extraction: {body_snapshot}"
    );
    let fieldless_enum_row = replacement_rows
        .iter()
        .find(|row| row.id == "replacement-body-v0-015")
        .ok_or("the #1154 fieldless-enum Body-IR row must remain in the corpus")?;
    let ReceiptRef::ReplacementExecuted { body_snapshot, .. } = &fieldless_enum_row.receipt else {
        return Err("the #1154 fieldless-enum Body-IR row must retain a direct execution receipt".into());
    };
    assert!(
        body_snapshot.contains("executed fieldless-enum variant name=Signal::Ready enum_id=decl:")
            && body_snapshot.contains("executed fieldless-enum variant name=Signal::Stop enum_id=decl:"),
        "the #1154 fieldless-enum row must bind receipt evidence to retained enum/member identities: {body_snapshot}"
    );
    let pattern_row = replacement_rows
        .iter()
        .find(|row| row.id == "replacement-body-v0-016")
        .ok_or("the #1154 direct-pattern Body-IR row must remain in the corpus")?;
    let ReceiptRef::ReplacementExecuted { body_snapshot, .. } = &pattern_row.receipt else {
        return Err("the #1154 direct-pattern Body-IR row must retain a direct execution receipt".into());
    };
    assert!(
        body_snapshot.contains("nominal Pair id=decl:")
            && body_snapshot.contains("fieldless fieldless_enum_variant(Signal::Ready")
            && body_snapshot.contains("executed direct match arm"),
        "the #1154 pattern row must bind receipt evidence to retained targets and a selected direct arm: {body_snapshot}"
    );
    let result_row = replacement_rows
        .iter()
        .find(|row| row.id == "replacement-body-v0-017")
        .ok_or("the #1154 direct-Result Body-IR row must remain in the corpus")?;
    let ReceiptRef::ReplacementExecuted { body_snapshot, .. } = &result_row.receipt else {
        return Err("the #1154 direct-Result Body-IR row must retain a direct execution receipt".into());
    };
    assert!(
        body_snapshot.contains("result_ok(")
            && body_snapshot.contains("same_error_type=Failure")
            && body_snapshot.contains("executed Result::ok construction")
            && body_snapshot.contains("executed Result try route=ok"),
        "the #1154 Result row must bind receipt evidence to explicit construction and same-error routing: {body_snapshot}"
    );

    for row in replacement_rows {
        assert_eq!(row.lane, EvidenceLane::DirectReplacementBodyIr);
        if SHADOW_COMPARED_CASE_IDS.contains(&row.id) && summary.source_observable_comparison_available {
            // When the comparison ran, this row's evidence is the comparison itself. The dedicated receipt tests
            // above verify each compared row's typed result, exact streams, and independent route authority.
            continue;
        }
        assert_eq!(row.overall_state, OverallState::NonGreenShadowUnavailable);
        match &row.receipt {
            ReceiptRef::ReplacementExecuted {
                selection_identity,
                receipt_identity,
                output_identity,
                body_snapshot,
                ownership_reads,
                runtime_requirements,
                task_lifecycle,
                comparison_reason,
            } => {
                assert!(selection_identity.starts_with("sha256:"));
                assert!(receipt_identity.starts_with("sha256:"));
                assert!(output_identity.starts_with("sha256:"));
                assert!(body_snapshot.contains("body "));
                assert!(
                    ownership_reads
                        .iter()
                        .all(|read| read.span_end >= read.span_start && !read.fact.is_empty()),
                    "{} lost canonical ownership evidence: {ownership_reads:?}",
                    row.id
                );
                assert!(
                    runtime_requirements
                        .iter()
                        .all(|requirement| !requirement.requirement.is_empty()),
                    "{} emitted an invalid runtime-requirement projection: {runtime_requirements:?}",
                    row.id
                );
                if matches!(row.id, "replacement-body-v0-018" | "replacement-body-v0-019") {
                    assert!(
                        task_lifecycle.iter().any(|event| event.event == "constructed")
                            && task_lifecycle.iter().any(|event| event.event == "completed"),
                        "{} needs receipt-bound task construction/completion evidence: {task_lifecycle:?}",
                        row.id
                    );
                }
                // Rows that never declared a comparison say so; the declaring row, when its comparison could
                // not run, names the boundary that stopped it instead. Neither may imply generated Rust proved
                // anything.
                let expected_reason = if SHADOW_COMPARED_CASE_IDS.contains(&row.id) {
                    "the legacy route did not execute"
                } else {
                    "does not declare the bounded #1146 source-observable"
                };
                assert!(
                    comparison_reason.contains(expected_reason) || comparison_reason.contains("not staged"),
                    "{} must state why no comparison was made rather than implying generated-Rust evidence: \
                     {comparison_reason}",
                    row.id
                );
            }
            receipt => {
                return Err(format!(
                    "{} needs its own replacement execution receipt, got {receipt:?}",
                    row.id
                )
                .into());
            }
        }
    }
    Ok(())
}

// ============================================================================
// CI-readable summary emission (#655 consumer contract)
// ============================================================================

/// Where the CI-readable summary is written, honoring a harness-selected `CARGO_TARGET_DIR` when set (matching
/// `tests/support/mod.rs`'s convention for other generated test artifacts) and falling back to the crate-local
/// `target/` directory otherwise.
fn summary_output_path() -> PathBuf {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
    target_dir.join("parity-corpus").join("summary.json")
}

#[test]
fn ci_summary_serializes_with_the_fields_655_needs_and_is_written_to_a_stable_path()
-> Result<(), Box<dyn std::error::Error>> {
    let summary = parity_corpus::summarize(&seed_corpus());
    let json = serde_json::to_string_pretty(&summary)?;
    let value: serde_json::Value = serde_json::from_str(&json)?;

    for field in [
        "schema_version",
        "total_cases",
        "green",
        "non_green_shadow_unavailable",
        "non_green_shadow_diverged",
        "non_green_behavior",
        "execution_receipt_schema_available",
        "cases_with_execution_receipts",
        "source_observable_comparison_available",
        "cases",
    ] {
        assert!(
            value.get(field).is_some(),
            "CI summary is missing required top-level field `{field}`: {value}"
        );
    }
    assert!(
        value.get("receipt_schema_available").is_none(),
        "schema v7 must not retain the ambiguous field that implied every row had a receipt"
    );

    let cases = value
        .get("cases")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("CI summary `cases` must be a JSON array: {value}"))?;
    assert_eq!(cases.len(), seed_corpus().len());
    for case in cases {
        for field in [
            "id",
            "title",
            "category",
            "lane",
            "evidence",
            "disposition_kind",
            "behavior_outcome",
            "receipt",
            "identity_conformance",
            "overall_state",
        ] {
            assert!(
                case.get(field).is_some(),
                "CI summary case row is missing required field `{field}`: {case}"
            );
        }
    }

    let output_path = summary_output_path();
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output_path, &json)?;
    Ok(())
}
