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
use incan::backend::replacement::ReplacementValue;
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::diagnostics::CompileError;
use incan::frontend::{lexer, parser, typechecker};
use std::path::PathBuf;

#[path = "support/parity_corpus.rs"]
mod parity_corpus;
#[path = "support/shadow_capability.rs"]
mod shadow_capability;

/// The one corpus row that declares the bounded #1146 source-observable comparison profile.
///
/// Named once so the "exactly one green row" contract is stated in a single place; widening it is a deliberate
/// edit here, not a side effect of adding another direct-execution row.
const SHADOW_COMPARED_CASE_ID: &str = "replacement-body-v0-001";

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
    let tokens = match lexer::lex(src) {
        Ok(tokens) => tokens,
        Err(errors) => {
            return ComparisonOutcome::Incompatible {
                reason: format!("expected {expect_desc}, but lexing failed: {errors:?}"),
            };
        }
    };
    let program = match parser::parse(&tokens) {
        Ok(program) => program,
        Err(errors) => {
            return ComparisonOutcome::Incompatible {
                reason: format!("expected {expect_desc}, but parsing failed: {errors:?}"),
            };
        }
    };
    let module_path = vec!["parity_987_body_ir".to_string()];
    let mut checker = typechecker::TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    if let Err(errors) = checker.check_program(&program) {
        return ComparisonOutcome::Mismatch {
            detail: format!("expected {expect_desc}, but typechecking reported {errors:?}"),
        };
    }
    let snapshot = build_body_ir_module_v0(&program, &module_path, checker.type_info()).render_snapshot();
    if snapshot.contains("unsupported(") {
        return ComparisonOutcome::Mismatch {
            detail: format!("expected {expect_desc}, but Body IR still contains a placeholder:\n{snapshot}"),
        };
    }
    ComparisonOutcome::Match
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
                // The one row #1146 proves end to end: its module holds a single named free function with scalar
                // parameters, so the legacy route can call it from a generated entrypoint and print the result.
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
    ]
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
fn only_a_row_with_a_real_two_route_comparison_can_be_green() -> Result<(), Box<dyn std::error::Error>> {
    // This is the corpus's core promise: direct replacement execution does not become green parity merely because
    // it has a receipt, and generated Rust never counts as proof. Exactly one row declares the bounded #1146
    // comparison profile; it is green only when that comparison actually ran through Oven and agreed.
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
            green,
            vec![SHADOW_COMPARED_CASE_ID],
            "exactly the one row with a proven two-route comparison may be green"
        );
        assert_eq!(summary.green, 1);
        assert_eq!(summary.non_green_shadow_unavailable, summary.total_cases - 1);
    } else {
        // No comparison ran, so nothing may be green — including the row that declares one.
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

/// The one row that declares the bounded #1146 comparison profile.
fn compared_row(summary: &parity_corpus::CorpusSummary) -> Option<&parity_corpus::CaseReport> {
    summary.cases.iter().find(|case| case.id == SHADOW_COMPARED_CASE_ID)
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
    assert_eq!(observable, "completed(\"42\")");
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
        14,
        "the six #988 cases, #1123's lazy-generator case, #1152's four callable/runtime cases, and #1154's structural, nominal, and value-enum cases must stay stable in #987"
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

    for row in replacement_rows {
        assert_eq!(row.lane, EvidenceLane::DirectReplacementBodyIr);
        if row.id == SHADOW_COMPARED_CASE_ID && summary.source_observable_comparison_available {
            // When the comparison ran, this row's evidence is the comparison itself, covered by
            // `the_compared_row_carries_two_route_receipts_and_its_oven_authority`.
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
                // Rows that never declared a comparison say so; the declaring row, when its comparison could
                // not run, names the boundary that stopped it instead. Neither may imply generated Rust proved
                // anything.
                let expected_reason = if row.id == SHADOW_COMPARED_CASE_ID {
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
