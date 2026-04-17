//! Parity checks between compiler-side const-eval expectations and runtime stdlib semantics.
//! Pilot scope: numeric expressions (small matrix).

use incan_core::strings::str_contains;
use incan_core::{NumericOp, NumericTy, result_numeric_type};
use incan_stdlib::num::{py_div, py_floor_div, py_mod};

fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-10
}

#[test]
fn consteval_vs_runtime_numeric_policy() {
    // Result category expectations (policy)
    assert_eq!(
        result_numeric_type(NumericOp::Div, NumericTy::Int, NumericTy::Int, None),
        NumericTy::Float
    );
    assert_eq!(
        result_numeric_type(NumericOp::FloorDiv, NumericTy::Int, NumericTy::Int, None),
        NumericTy::Int
    );
    assert_eq!(
        result_numeric_type(NumericOp::FloorDiv, NumericTy::Int, NumericTy::Float, None),
        NumericTy::Float
    );
    assert_eq!(
        result_numeric_type(NumericOp::Mod, NumericTy::Int, NumericTy::Int, None),
        NumericTy::Int
    );
    assert_eq!(
        result_numeric_type(NumericOp::Mod, NumericTy::Float, NumericTy::Int, None),
        NumericTy::Float
    );

    // Runtime behavior samples (aligns with policy categories above)
    assert!(approx_eq(py_div(7_i64, 2_i64), 3.5));
    assert_eq!(py_floor_div(7_i64, 2_i64), 3_i64);
    assert!(approx_eq(py_floor_div(7_i64, 2.0_f64), 3.0_f64));
    assert_eq!(py_mod(7_i64, 3_i64), 1_i64);
    assert!(approx_eq(py_mod(7.0_f64, 3_i64), 1.0_f64));
}

#[test]
#[should_panic(expected = "ZeroDivisionError: float division by zero")]
fn runtime_div_zero_matches_policy_error() {
    let _ = py_div(1_i64, 0_i64);
}

#[test]
#[should_panic(expected = "ZeroDivisionError: float division by zero")]
fn runtime_mod_zero_matches_policy_error() {
    let _ = py_mod(1_i64, 0_i64);
}

#[test]
#[should_panic(expected = "ZeroDivisionError: float division by zero")]
fn runtime_floordiv_zero_matches_policy_error() {
    let _ = py_floor_div(1_i64, 0_i64);
}

// ------------------------------------------------------------
// Compiler const-eval vs runtime parity (numeric expressions)
// ------------------------------------------------------------

/// Evaluate a tiny Incan snippet with consts and return Ok(errors.len()) or Err(error_messages)
fn run_const_eval_snippet(src: &str) -> Result<usize, Vec<String>> {
    use incan::frontend::{lexer, parser, typechecker};

    let tokens = lexer::lex(src).map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>())?;
    let ast = parser::parse(&tokens).map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>())?;
    match typechecker::check(&ast) {
        Ok(_) => Ok(0),
        Err(errs) => Err(errs.into_iter().map(|e| e.message).collect()),
    }
}

#[test]
fn consteval_vs_runtime_numeric_values_and_errors() {
    // Successful const-eval cases should produce no errors and runtime should match policy expectations.
    let ok_src = r#"
pub const A: int = 7 // 2    # floor div int/int => int
pub const B: float = 7 // 2.0 # floor div int/float => float
pub const C: int = 7 % 3      # mod int/int => int
pub const D: float = 7.0 % 3  # mod float/int => float
pub const E: float = 7 / 2    # div => float
"#;
    assert_eq!(run_const_eval_snippet(ok_src), Ok(0));

    // Runtime sanity checks (mirrors the const expressions above)
    assert_eq!(py_floor_div(7_i64, 2_i64), 3_i64);
    assert!(approx_eq(py_floor_div(7_i64, 2.0_f64), 3.0_f64));
    assert_eq!(py_mod(7_i64, 3_i64), 1_i64);
    assert!(approx_eq(py_mod(7.0_f64, 3_i64), 1.0_f64));
    assert!(approx_eq(py_div(7_i64, 2_i64), 3.5_f64));

    // Error case: division by zero should surface as a const-eval error
    let err_src = r#"
pub const Z: int = 1 / 0
"#;
    let errs = match run_const_eval_snippet(err_src) {
        Err(errs) => errs,
        Ok(_) => panic!("expected a const-eval error for division by zero"),
    };
    // Accept any const-eval error for division by zero (message text may differ from runtime panic string)
    assert!(
        !errs.is_empty(),
        "expected a const-eval error for division by zero, got none"
    );
}

#[test]
fn string_membership_shared_rule() {
    // Runtime helper from semantic core
    assert!(str_contains("hello", "hell"));
    assert!(!str_contains("hello", "xyz"));

    // Typechecker path: ensure `"a" in "abc"` typechecks and yields bool (no errors)
    let src = r#"
def f() -> bool:
  return "a" in "abc"
"#;
    assert_eq!(run_const_eval_snippet(src), Ok(0));
}
