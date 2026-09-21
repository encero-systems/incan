//! The source-only rows of the #987 corpus: language-contract, diagnostic, stdlib/runtime and
//! generated-artifact cases, each as an Incan source and the evaluate callback that probes the current compiler.
//!
//! Case 1 — match exhaustiveness is enforced; case 2 — chained comparisons are rejected, with the
//! package-boundary rows; case 3 — string membership matches the runtime helper; case 4 — codegen stays
//! inspectable, not authoritative; case 5 — lexical bindings shadow ambient builtins; case 6 — dead code after
//! `return` warns; case 7 — statement tuple unpack of a non-tuple; cases 8–9 — named call/construction binding
//! (#1158); cases 10–11 — the async surface (#1164); cases 12–13 — spread forms (#1159); cases 15–16 — bytes
//! literals and range values (#1165); case 17 — an inactive feature's body never lowers (#1166); cases 18–19 —
//! statement-position `loop:` and the `unsafe:` boundary (#1162); cases 20–23 — pattern and `raises` asserts
//! (#1167), container membership and list concatenation.

use super::*;

pub(super) const CASE_1_SRC: &str = r#"
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

pub(super) fn case_supported_match_exhaustiveness() -> ComparisonOutcome {
    outcome_from_typecheck(
        CASE_1_SRC,
        |errs| errs.iter().any(|e| e.to_lowercase().contains("exhaustive")),
        "a non-exhaustive-match diagnostic naming the missing `Blue` arm",
    )
}

// Incan does not support Python-style chained comparisons (`a < b < c` as `a < b and b < c`). Verified by direct
// probe: today it type-errors because `(a < b) < c` compares a `bool` to an `int`. The corpus records that this
// stays a rejection, not a silent reinterpretation as chained boolean logic — a real semantic decision, not just
// token shape.
pub(super) const CASE_2_SRC: &str = r#"
def main() -> None:
    a = 1
    b = 2
    c = 3
    if a < b < c:
        println("chained")
"#;

/// One unchanged consumer program observed through native and non-linking package execution.
pub(super) const PACKAGE_CONSUMER_SRC: &str = r#"
from pub::widgets import build

def main() -> None:
    println(build())
"#;

/// Share one real package publication across the two rows and repeated corpus summary queries.
fn package_boundary_observation() -> &'static Result<package_boundary_probe::PackageBoundaryObservation, String> {
    static OBSERVATION: OnceLock<Result<package_boundary_probe::PackageBoundaryObservation, String>> = OnceLock::new();
    OBSERVATION.get_or_init(|| {
        package_boundary_probe::observe_package_boundary(PACKAGE_CONSUMER_SRC).map_err(|error| error.to_string())
    })
}

/// Verify actual native/non-linking output agreement for the bounded source-unavailable materialized package.
pub(super) fn case_package_consumer_call_executes() -> ComparisonOutcome {
    match package_boundary_observation() {
        Ok(observed) if observed.native_stdout == b"42\n" && observed.replacement_stdout == observed.native_stdout => {
            ComparisonOutcome::Match
        }
        observed => ComparisonOutcome::Mismatch {
            detail: format!("expected the same package consumer to print 42 on both routes: {observed:?}"),
        },
    }
}

/// Verify that a missing representation refuses in packaging terms before output or a completed receipt.
pub(super) fn case_package_representation_refusal_is_packaging_error() -> ComparisonOutcome {
    match package_boundary_observation() {
        Ok(observed)
            if !observed.refusal_success
                && observed.refusal_stdout.is_empty()
                && !observed.refusal_receipt_exists
                && observed.refusal_stderr.contains("widgets")
                && observed.refusal_stderr.contains("1.2.3")
                && observed.refusal_stderr.contains("executable representation")
                && !observed.refusal_stderr.contains("INCAN-R988-UNSUPPORTED") =>
        {
            ComparisonOutcome::Match
        }
        observed => ComparisonOutcome::Mismatch {
            detail: format!("expected a package-specific refusal without output or receipt: {observed:?}"),
        },
    }
}

pub(super) fn case_diagnostic_chained_comparison_rejected() -> ComparisonOutcome {
    outcome_from_typecheck(
        CASE_2_SRC,
        |errs| !errs.is_empty(),
        "a type-mismatch diagnostic rejecting the chained comparison",
    )
}

pub(super) const CASE_3_SRC: &str = r#"
def f() -> bool:
    return "a" in "abc"
"#;

pub(super) fn case_stdlib_runtime_string_membership() -> ComparisonOutcome {
    use incan_lang::strings::str_contains;

    if !str_contains("hello", "hell") || str_contains("hello", "xyz") {
        return ComparisonOutcome::Mismatch {
            detail: "incan_lang::strings::str_contains no longer matches its documented substring policy".to_string(),
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
pub(super) fn case_supported_string_membership_reaches_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_3_SRC,
        "string membership to lower to an explicit compiler-owned helper call rather than a placeholder",
    )
}

pub(super) const CASE_4_SRC: &str = r#"
def add(a: int, b: int) -> int:
    return a + b
"#;

pub(super) fn case_generated_artifact_valid_rust_shape() -> ComparisonOutcome {
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

// #1116 adopts this as a language contract: a direct module declaration or explicit import is a real lexical
// binding and wins over an ambient core builtin function for unqualified calls. `std.builtins.<name>` remains the
// explicit route to the builtin when the local spelling is shadowed. The corresponding typechecker, codegen, and
// runtime coverage lives alongside this corpus row; #653 must reproduce the same precedence deliberately.
pub(super) const CASE_5_SRC: &str = r#"
def len(x: int) -> int:
    return x + 1

def main() -> None:
    y = len(5)
    println(y)
"#;

pub(super) fn case_supported_builtin_len_shadowing() -> ComparisonOutcome {
    outcome_from_typecheck(
        CASE_5_SRC,
        |errs| errs.is_empty(),
        "a module `len` binding to shadow the ambient builtin without a diagnostic",
    )
}

// Named field construction is the *only* spelling the typechecker accepts for a `model`/`class`: positional
// construction is rejected outright. Before #1158 that spelling lowered to `unsupported(...)`, so no nominal value
// was representable in Body IR at all — the canonical `User(id=..., email=...)` shape in this repository's own
// README included. The cutover must keep both the named-only source rule and its faithful representation.
pub(super) const CASE_8_SRC: &str = r#"
model Point:
    x: int
    y: int = 5

def main() -> None:
    p = Point(y=2, x=1)
    println(p.x)
"#;

pub(super) fn case_supported_named_construction_reaches_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_8_SRC,
        "named `model` construction to lower to real Body IR rather than an unsupported placeholder",
    )
}

// Named and defaulted arguments at an ordinary call site, including an argument written out of declaration order.
// The binding is what makes the operand order meaningful; the written order is what makes effect ordering
// meaningful. Both must survive the cutover.
pub(super) const CASE_9_SRC: &str = r#"
def scale(value: int, factor: int = 2) -> int:
    return value * factor

def main() -> None:
    println(scale(factor=3, value=4))
    println(scale(5))
"#;

pub(super) fn case_supported_named_call_arguments_reach_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_9_SRC,
        "named, out-of-order, and defaulted call arguments to lower to real Body IR",
    )
}

// `AsyncAwait` is a public capability in the release-pinned baseline. Before #1164 an `await` lowered to a
// placeholder labelled only "prefix-keyword surface expression", so the suspension point — the one fact a task
// runtime needs — did not exist in Body IR at all. The cutover must keep both the source form and its
// representation, including the body-level async fact for a body that awaits nothing.
pub(super) const CASE_10_SRC: &str = r#"
import std.async

async def fetch() -> int:
    return 7

async def main() -> None:
    value = await fetch()
    println(value)
"#;

pub(super) fn case_supported_await_reaches_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_10_SRC,
        "`await` to lower to a real Body IR suspension point rather than an unsupported placeholder",
    )
}

// `AsyncRace` collapsed even harder: the whole `race for` expression became one placeholder, erasing every arm,
// arm body, and the shared binding. Both arm forms — a bare expression and a block with a trailing value — are
// part of the source contract.
pub(super) const CASE_11_SRC: &str = r#"
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

pub(super) fn case_supported_race_for_reaches_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_11_SRC,
        "`race for` to lower to a real Body IR race with its arms, arm bodies, and bindings intact",
    )
}

// `VariadicAndSpreadCalls` is a public capability. Before #1159 every spread form lowered to a placeholder, and
// for a list or dict literal the placeholder replaced the *whole* literal, so its fixed elements were erased too.
pub(super) const CASE_12_SRC: &str = r#"
def main() -> None:
    xs = [2, 3]
    values = [1, *xs, 4]
    base = {"a": 1}
    merged = {**base, "b": 2}
    println(len(values))
    println(len(merged))
"#;

pub(super) fn case_supported_literal_spreads_reach_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_12_SRC,
        "list and dict literal spreads to lower with their fixed elements intact",
    )
}

// Call-site spreads, including the combined form where a named argument sits alongside one. The callee's arity is
// a runtime fact here, so the call records no declared-slot binding — but every written argument form survives.
pub(super) const CASE_13_SRC: &str = r#"
def log(a: int, b: int, *items: int, **fields: int) -> None:
    println(a)

def main() -> None:
    xs = [3, 4]
    kw = {"k": 5}
    log(1, *xs, b=2, **kw)
"#;

// Both rows existed as refusals before #1167: `AssertKind::IsPattern` and `AssertKind::Raises` lowered to
// `unsupported(assert pattern/raises form)`. The pattern row is the one that mattered most, because the refusal was
// not merely incomplete -- `assert o is Some(v)` *binds* `v`, so lowering it to a placeholder dropped the binding
// and every later read of `v` lowered against a name the body never declared.
pub(super) const CASE_20_SRC: &str = r#"
def run(o: Option[str]) -> None:
    assert o is Some(v)
    print(v)
"#;

pub(super) const CASE_21_SRC: &str = r#"
def boom() -> int:
    return 1

def run() -> None:
    assert boom() raises ValueError
    assert boom() raises IndexError, "wanted an index error"
"#;

pub(super) const CASE_22_SRC: &str = r#"
def has_item(xs: List[int], v: int) -> bool:
    return v in xs

def lacks_key(d: Dict[str, int], k: str) -> bool:
    return k not in d
"#;

pub(super) const CASE_23_SRC: &str = r#"
def joined(xs: List[int], ys: List[int]) -> List[int]:
    return xs + ys
"#;

pub(super) fn case_collection_membership_names_its_container() -> ComparisonOutcome {
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

pub(super) fn case_list_concatenation_is_not_a_primitive_addition() -> ComparisonOutcome {
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

pub(super) fn case_pattern_assertion_binding_reaches_body_ir() -> ComparisonOutcome {
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

pub(super) fn case_raises_assertion_reaches_body_ir() -> ComparisonOutcome {
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

// #1166 made the input contract explicit: Body IR consumes a desugared, feature-projected program, and every caller
// owes it that. This row is the corpus holding *itself* to that contract — before #1166 both corpus entry points
// lowered raw parse output, so a divergence between the two pipelines would have gone green here rather than being
// surfaced.
//
// The row proves the feature-projection half, which is constructible in-process. The vocab half of the same
// contract is covered by unit tests instead: a genuinely vocab-authored body needs an import-activated library
// vocabulary with a WASM desugarer artifact, which no corpus row can stand up. Claiming this row proves both
// halves would be the kind of overstated evidence the corpus exists to prevent.
pub(super) const CASE_17_SRC: &str = r#"
when feature("beta"):
    def gated() -> int:
        return 7

def main() -> int:
    return 1
"#;

pub(super) fn case_inactive_feature_body_never_reaches_body_ir() -> ComparisonOutcome {
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
    let program = match incan_frontend::body_ir::apply_body_ir_input_contract(
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

pub(super) fn case_supported_call_spreads_reach_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_13_SRC,
        "positional, spread, named, and keyword-spread call arguments to lower together",
    )
}

// A byte-string literal is ordinary accepted source with its own type, but `lower_literal` had no `bir::Constant`
// for it, so every `b"..."` reached a placeholder. The row is about representation, not bytes operations: those
// keep whatever refusal they already had.
pub(super) const CASE_15_SRC: &str = r#"
def send(payload: bytes) -> int:
    return 1

def main() -> None:
    greeting = b"hi"
    println(send(greeting))
"#;

pub(super) fn case_supported_bytes_literal_reaches_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_15_SRC,
        "a byte-string literal to lower to its own bytes constant rather than a placeholder",
    )
}

// A range is a value, not only a `for` header. `r = 0..10` has always typechecked, so refusing it in lowering left
// Body IR non-total over accepted programs; binding one and then iterating it exercises both halves.
pub(super) const CASE_16_SRC: &str = r#"
def main() -> None:
    r = 0..10
    mut total = 0
    for i in r:
        total = total + i
    println(total)
"#;

pub(super) fn case_supported_range_value_reaches_body_ir() -> ComparisonOutcome {
    outcome_from_body_ir(
        CASE_16_SRC,
        "a range bound to a local to lower to a real range value that the loop then iterates",
    )
}

// `bir::StatementKind::Loop` already existed and the expression spelling already emitted it, so the plain
// statement spelling -- the more common one -- was refused by a missing dispatch arm rather than by a missing
// representation. Included with `continue` and a nested loop, because the loop's break/continue vocabulary is
// what makes the row about the construct rather than about one keyword.
pub(super) const CASE_18_SRC: &str = r#"
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

pub(super) fn case_supported_statement_loop_reaches_body_ir() -> ComparisonOutcome {
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
pub(super) const CASE_19_SRC: &str = r#"
def probe(x: int) -> int:
    return x

def touch(value: int) -> int:
    mut total = 0
    unsafe:
        total = probe(value)
    return total
"#;

pub(super) fn case_unsafe_region_is_a_stated_refusal() -> ComparisonOutcome {
    outcome_from_body_ir_refusal(
        CASE_19_SRC,
        "unsupported(`unsafe:` acknowledgement region:",
        "an `unsafe:` region to refuse under a named, reasoned boundary rather than lower silently",
    )
}

// This row entered the corpus as bug-compatible behavior: statements after an unconditional `return` typechecked
// with zero diagnostics, so the gap was invisible unless a user read the generated Rust. #1117 migrated it
// deliberately — the typechecker now emits `INCAN-T0101` — so the case asserts the diagnostic contract instead of
// the old silence, and its disposition records the migration rather than freezing either behavior.
//
// The assertion reads the *stable code*, not message prose, so rewording the diagnostic does not silently break
// the corpus while a real change of contract still does.
pub(super) const CASE_6_SRC: &str = r#"
def f() -> int:
    return 1
    println("dead code")
    return 2
"#;

pub(super) fn case_diagnostic_unreachable_code_after_return() -> ComparisonOutcome {
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

// Entered the corpus as a silent accept: `a, b = 5` typechecked clean, bound both names `Unknown`, and only
// failed while compiling the emitted Rust with an `E0610` naming a `__incan_tuple_unpack_*` binding the user never
// wrote. #1132 migrated it to a source-language decision. Asserted through the message rather than a stable code
// because this family reports under the broad `INCAN-T0001` typecheck code.
pub(super) const CASE_7_SRC: &str = r#"
def main() -> None:
    a, b = 5
    println(f"{a} {b}")
"#;

pub(super) fn case_diagnostic_statement_tuple_unpack_of_non_tuple() -> ComparisonOutcome {
    outcome_from_typecheck(
        CASE_7_SRC,
        |errs| {
            errs.iter()
                .any(|error| error.contains("Cannot destructure 2 values from value of type 'int'"))
        },
        "a typechecker diagnostic naming the non-tuple value type",
    )
}
