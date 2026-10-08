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
//! Per #987's own plan step 3 ("add a narrow source-only seed corpus before public package or Rust-interop
//! rows"), every seed case here uses a direct-parser/typechecker, generated-project-run, or codegen-snapshot
//! lane. Package/import, Rust-interop, vocab, and downstream lanes are deferred until the required compiler/ABI
//! decisions are available (plan step 4). Issue #987 records the expansion criteria; a seed case must never imply
//! that an unavailable lane has been proven.
//!
//! Each case's `evaluate` function probes the *current* compiler directly (not a fixture snapshot of past output),
//! so a behavior change shows up as [`parity_corpus::ComparisonOutcome::Mismatch`] the next time this test runs.

use incan::backend::IrCodegen;
use incan::backend::replacement::provider::{
    PROVIDER_COMPARISON_UNAVAILABLE_REASON, ProviderInputValue, ProviderInvocation, ProviderOperationHost,
    ProviderOperationOutcome, ProviderRuntime,
};
use incan::backend::replacement::{ReplacementExecutionError, ReplacementValue, execute_free_function_with_providers};
use incan::frontend::body_ir::{build_body_ir_module_v0, build_body_ir_module_v0_with_provider_plan};
use incan::frontend::diagnostics::CompileError;
use incan::frontend::library_manifest_index::LibraryManifestIndex;
use incan::frontend::{lexer, parser, typechecker};
use incan::library_manifest::{CompiledProviderMetadata, LibraryManifest, ProviderOperationMetadata};
use incan::provider::{NamespaceAuthority, ProviderIdentity, ProviderPlan, ProviderProvenance, ProviderRecord};
use incan_semantics_core::authority::StaticAuthority;
use incan_semantics_core::receipts::{AttributeSensitivity, ReceiptAttribute, ReceiptStatus, ReplayClassification};
use incan_semantics_core::{AuthorityMode, CanonicalSymbolId, HirSourceSpan};
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

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
const SHADOW_COMPARED_CASE_IDS: [&str; 10] = [
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

use parity_corpus::{
    BehaviorCategory, ComparisonOutcome, Disposition, EvidenceLane, OverallState, ParityCase, ReceiptRef,
    validate_corpus,
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
// These rows carry a legacy-side corpus receipt because `ReplacementExecutionPlan` -- the corpus's own direct
// execution shape -- names a function and concrete arguments, with nowhere to name the authority source and
// provider host a provider operation needs. Their provider executions therefore produce real #986
// selection/execution receipts inside the probe, which the probe asserts on, rather than corpus-visible
// `ReceiptRef::ReplacementExecuted` evidence. Binding them into the corpus receipt is corpus-schema work in
// `tests/support/parity_corpus.rs`, and belongs with #1146's comparison route rather than with this vertical.

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

/// The grant spelling the selected capability renders to, and therefore the one a governed run must hold.
const PROVIDER_GRANT: &str = "app.ledger_charge";

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
    grants: &[&str],
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
    let operation = descriptors
        .first()
        .map(|descriptor| descriptor.operation.clone())
        .ok_or("the provider fixture declares no checked provider operation")?;
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
    let authority = StaticAuthority::new(mode, grants.iter().map(|grant| (*grant).to_string()));
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
        observe_provider_path(LedgerBehavior::Settle, AuthorityMode::Governed, &[PROVIDER_GRANT]),
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
        observe_provider_path(LedgerBehavior::Settle, AuthorityMode::Governed, &[]),
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
        observe_provider_path(LedgerBehavior::Decline, AuthorityMode::Governed, &[PROVIDER_GRANT]),
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
        observe_provider_path(
            LedgerBehavior::SettleWithSecretAccount,
            AuthorityMode::Governed,
            &[PROVIDER_GRANT],
        ),
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
        observe_provider_path(LedgerBehavior::Decline, AuthorityMode::Governed, &[PROVIDER_GRANT]),
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
// Seed corpus
// ============================================================================

/// The narrow, source-only seed corpus (#987 plan step 3). Package/import, Rust-interop, vocab, and downstream
/// rows are deferred to plan step 4 until their compiler/ABI contracts are available; the public parity-corpus
/// reference and #987 record that expansion boundary.
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
            replacement_execution: Some(parity_corpus::ReplacementExecutionPlan {
                function: "sorted_int_list",
                arguments: Vec::new,
                expected: || ReplacementValue::Int(29_320_233),
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
            replacement_execution: None,
        },
    ]
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
            // `validate_corpus` directly, so this placeholder source is never hashed into a real receipt.
            source: "",
            evaluate: Some(|| ComparisonOutcome::Match),
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
            // `validate_corpus` directly, so this placeholder source is never hashed into a real receipt.
            source: "",
            evaluate: Some(|| ComparisonOutcome::Match),
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
            // `validate_corpus` directly, so this placeholder source is never hashed into a real receipt.
            source: "",
            evaluate: Some(|| ComparisonOutcome::Match),
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
            // `validate_corpus` directly, so this placeholder source is never hashed into a real receipt.
            source: "",
            evaluate: Some(|| ComparisonOutcome::Match),
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
fn seed_corpus_every_case_confirms_its_documented_current_behavior() {
    let summary = parity_corpus::summarize(&seed_corpus());
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
        summary.receipt_schema_available,
        "the summary must say the #986 receipt schema is available now that PR #1120 landed it"
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
        28,
        "the nineteen original direct cases plus hashed membership, selected string helpers, scalar conversions, canonical Enumerate/Zip, string length, scalar JSON, hashed collection length, bounded bool truthiness and nonempty integer-list sorting must stay stable in #987"
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
        "receipt_schema_available",
        "source_observable_comparison_available",
        "cases",
    ] {
        assert!(
            value.get(field).is_some(),
            "CI summary is missing required top-level field `{field}`: {value}"
        );
    }

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
