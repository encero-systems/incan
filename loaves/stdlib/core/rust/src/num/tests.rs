use super::*;
use incan_lang::{NumericOp, NumericTy, result_numeric_type};
use std::any::Any;
use std::f64;

fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-10
}

#[test]
fn test_matrix_py_div() {
    assert!(approx_eq(py_div(2_i64, 2_i64), 1.0));
    assert!(approx_eq(py_div(2_i64, 2.0_f64), 1.0));
    assert!(approx_eq(py_div(2.0_f64, 2_i64), 1.0));
    assert!(approx_eq(py_div(2.0_f64, 2.0_f64), 1.0));
}

#[test]
fn test_semantic_core_division_type() {
    assert_eq!(
        result_numeric_type(NumericOp::Div, NumericTy::Int, NumericTy::Int, None),
        NumericTy::Float
    );
}

#[test]
fn test_semantic_core_matches_runtime_signatures() {
    // Division: always float
    let r = py_div(7_i64, 2_i64);
    assert!((&r as &dyn Any).is::<f64>());
    assert_eq!(
        result_numeric_type(NumericOp::Div, NumericTy::Int, NumericTy::Int, None),
        NumericTy::Float
    );

    // FloorDiv: int/int -> int, otherwise float
    let r_int = py_floor_div(7_i64, 2_i64);
    assert!((&r_int as &dyn Any).is::<i64>());
    assert_eq!(
        result_numeric_type(NumericOp::FloorDiv, NumericTy::Int, NumericTy::Int, None),
        NumericTy::Int
    );

    let r_float = py_floor_div(7_i64, 2.0_f64);
    assert!((&r_float as &dyn Any).is::<f64>());
    assert_eq!(
        result_numeric_type(NumericOp::FloorDiv, NumericTy::Int, NumericTy::Float, None),
        NumericTy::Float
    );

    // Mod: int/int -> int, otherwise float
    let m_int = py_mod(7_i64, 3_i64);
    assert!((&m_int as &dyn Any).is::<i64>());
    assert_eq!(
        result_numeric_type(NumericOp::Mod, NumericTy::Int, NumericTy::Int, None),
        NumericTy::Int
    );

    let m_float = py_mod(7.0_f64, 3_i64);
    assert!((&m_float as &dyn Any).is::<f64>());
    assert_eq!(
        result_numeric_type(NumericOp::Mod, NumericTy::Float, NumericTy::Int, None),
        NumericTy::Float
    );
}

#[test]
fn test_matrix_py_mod() {
    assert_eq!(py_mod(7_i64, 3_i64), 1);
    assert!(approx_eq(py_mod(7_i64, 3.0_f64), 1.0));
    assert!(approx_eq(py_mod(7.0_f64, 3_i64), 1.0));
    assert!(approx_eq(py_mod(7.0_f64, 3.0_f64), 1.0));
}

#[test]
fn test_gcd_i64_matches_python_shape() {
    assert_eq!(gcd_i64(54, 24), 6);
    assert_eq!(gcd_i64(-54, 24), 6);
    assert_eq!(gcd_i64(54, -24), 6);
    assert_eq!(gcd_i64(0, 24), 24);
    assert_eq!(gcd_i64(0, 0), 0);
}

#[test]
fn test_lcm_i64_matches_python_shape() {
    assert_eq!(lcm_i64(6, 8), 24);
    assert_eq!(lcm_i64(-6, 8), 24);
    assert_eq!(lcm_i64(6, -8), 24);
    assert_eq!(lcm_i64(0, 8), 0);
    assert_eq!(lcm_i64(0, 0), 0);
}

#[test]
fn test_gcd_i64_reports_unrepresentable_result() {
    let result = std::panic::catch_unwind(|| gcd_i64(i64::MIN, 0));
    let panic = match result {
        Ok(_) => panic!("expected overflow panic"),
        Err(panic) => panic,
    };
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&'static str>().copied())
        .unwrap_or("<non-string panic>");
    assert!(
        message.contains("ValueError: math.gcd result overflows Incan int"),
        "unexpected panic message: {message}"
    );
}

#[test]
fn test_lcm_i64_reports_unrepresentable_result() {
    let result = std::panic::catch_unwind(|| lcm_i64(i64::MIN, 1));
    let panic = match result {
        Ok(_) => panic!("expected overflow panic"),
        Err(panic) => panic,
    };
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&'static str>().copied())
        .unwrap_or("<non-string panic>");
    assert!(
        message.contains("ValueError: math.lcm result overflows Incan int"),
        "unexpected panic message: {message}"
    );
}

#[test]
fn test_decimal128_from_literal_preserves_coefficient_and_scale() {
    let value = Decimal128::from_literal("19.99d");
    assert_eq!(value.coefficient(), 1999);
    assert_eq!(value.scale(), 2);
    assert_eq!(value.to_string(), "19.99");

    let whole = Decimal128::from_literal("1000d");
    assert_eq!(whole.coefficient(), 1000);
    assert_eq!(whole.scale(), 0);
    assert_eq!(whole.to_string(), "1000");
}

#[test]
fn test_integer_resize_helpers() {
    assert_eq!(try_resize::<_, i8>(127_i16), Some(127_i8));
    assert_eq!(try_resize::<_, i8>(128_i16), None);
    assert_eq!(saturating_resize::<_, i8>(240_i16), i8::MAX);
    assert_eq!(saturating_resize::<_, u8>(-1_i16), 0_u8);
    assert_eq!(saturating_resize::<_, i8>(255_u16), i8::MAX);
}

#[test]
fn exact_float_guards_preserve_finite_values_and_lossless_widening() {
    assert_eq!(require_finite_f32(1.25_f32), 1.25_f32);
}

#[test]
fn exact_f32_guard_refuses_arithmetic_overflow() -> Result<(), String> {
    let maximum = f32::MAX;
    let panic = std::panic::catch_unwind(|| require_finite_f32(maximum * maximum))
        .err()
        .ok_or_else(|| "overflowed f32 unexpectedly crossed the exact boundary".to_string())?;
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&'static str>().copied())
        .ok_or_else(|| "exact-f32 refusal used a non-string panic payload".to_string())?;
    assert_eq!(message, "ValueError: non-finite float cannot initialize exact f32");
    Ok(())
}

#[test]
fn test_matrix_py_floor_div() {
    assert_eq!(py_floor_div(7_i64, 3_i64), 2);
    assert!(approx_eq(py_floor_div(7_i64, 3.0_f64), 2.0));
    assert!(approx_eq(py_floor_div(7.0_f64, 3_i64), 2.0));
    assert!(approx_eq(py_floor_div(7.0_f64, 3.0_f64), 2.0));
}

#[test]
fn test_py_floor_div_signs() {
    assert_eq!(py_floor_div_i64(-7, 3), -3);
    assert_eq!(py_floor_div_i64(7, -3), -3);
    assert_eq!(py_floor_div_i64(-7, -3), 2);
    assert_eq!(py_floor_div_i64(-9, 3), -3);
    assert!(approx_eq(py_floor_div(-7.0_f64, 3.0_f64), -3.0));
    assert!(approx_eq(py_floor_div(7.0_f64, -3.0_f64), -3.0));
    assert!(approx_eq(py_floor_div(-7.0_f64, -3.0_f64), 2.0));
}

#[test]
#[should_panic(expected = "ValueError: integer floor division result overflows Incan int")]
fn test_py_floor_div_i64_refuses_an_unrepresentable_quotient() {
    let _ = py_floor_div_i64(i64::MIN, -1);
}

#[test]
fn test_py_mod_signs() {
    assert_eq!(py_mod_i64(7, 3), 1);
    assert_eq!(py_mod_i64(-7, 3), 2);
    assert_eq!(py_mod_i64(7, -3), -2);
    assert_eq!(py_mod_i64(-7, -3), -1);
    assert!(approx_eq(py_mod(7.0_f64, 3.0_f64), 1.0));
    assert!(approx_eq(py_mod(-7.0_f64, 3.0_f64), 2.0));
    assert!(approx_eq(py_mod(7.0_f64, -3.0_f64), -2.0));
    assert!(approx_eq(py_mod(-7.0_f64, -3.0_f64), -1.0));
}

#[test]
fn test_floor_div_mod_relationship() {
    // a == (a // b) * b + (a % b)
    for a in [-10, -7, -1, 0, 1, 7, 10] {
        for b in [-3, -1, 1, 3] {
            let q = py_floor_div_i64(a, b);
            let r = py_mod_i64(a, b);
            assert_eq!(a, q * b + r, "a={}, b={}, q={}, r={}", a, b, q, r);
        }
    }
}

// --- Zero division panics ---

#[test]
#[should_panic(expected = "ZeroDivisionError: division by zero")]
fn test_div_zero_int() {
    let _ = py_div(1_i64, 0_i64);
}

#[test]
#[should_panic(expected = "ZeroDivisionError: float division by zero")]
fn test_div_zero_float() {
    let _ = py_div(1.0_f64, 0.0_f64);
}

#[test]
#[should_panic(expected = "ZeroDivisionError: float division by zero")]
fn test_div_zero_mixed_float_divisor() {
    let _ = py_div(1_i64, 0.0_f64);
}

#[test]
#[should_panic(expected = "ZeroDivisionError: integer division or modulo by zero")]
fn test_mod_zero_int() {
    let _ = py_mod(1_i64, 0_i64);
}

#[test]
#[should_panic(expected = "ZeroDivisionError: float modulo")]
fn test_mod_zero_float() {
    let _ = py_mod(1.0_f64, 0.0_f64);
}

#[test]
#[should_panic(expected = "ZeroDivisionError: float modulo")]
fn test_mod_zero_mixed_int_divisor() {
    let _ = py_mod(1.0_f64, 0_i64);
}

#[test]
#[should_panic(expected = "ZeroDivisionError: integer division or modulo by zero")]
fn test_floor_div_zero_int() {
    let _ = py_floor_div(1_i64, 0_i64);
}

#[test]
#[should_panic(expected = "ZeroDivisionError: float floor division by zero")]
fn test_floor_div_zero_float() {
    let _ = py_floor_div(1.0_f64, 0.0_f64);
}

#[test]
#[should_panic(expected = "ZeroDivisionError: float floor division by zero")]
fn test_floor_div_zero_mixed_float_divisor() {
    let _ = py_floor_div(1_i64, 0.0_f64);
}

/// Integer true division widened to `f64` keeps the integer message (#1813).
#[test]
#[should_panic(expected = "ZeroDivisionError: division by zero")]
fn integer_true_division_by_zero_names_no_float_issue1813() {
    let _ = py_div_int(7.0, 0.0);
}

/// Run `operation` and return the message it panicked with; an operation that returns is an error.
fn panic_message(operation: fn()) -> Result<String, String> {
    let payload = match std::panic::catch_unwind(operation) {
        Ok(()) => return Err("the operation returned instead of raising".to_string()),
        Err(payload) => payload,
    };
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|message| (*message).to_string()))
        .ok_or_else(|| "the panic payload is not a message".to_string())
}

/// Exact `f32` operators raise the float messages (#1813).
#[test]
fn exact_f32_zero_divisors_raise_float_messages_issue1813() -> Result<(), String> {
    assert_eq!(
        panic_message(|| {
            let _ = py_div_f32(1.0, 0.0);
        })?,
        "ZeroDivisionError: float division by zero"
    );
    assert_eq!(
        panic_message(|| {
            let _ = py_floor_div_f32(1.0, 0.0);
        })?,
        "ZeroDivisionError: float floor division by zero"
    );
    assert_eq!(
        panic_message(|| {
            let _ = py_mod_f32(1.0, 0.0);
        })?,
        "ZeroDivisionError: float modulo"
    );
    Ok(())
}

/// Unsigned floor division and modulo keep their type and compute Python's results (#1813).
#[test]
fn unsigned_floor_division_and_modulo_keep_their_type_issue1813() {
    assert_eq!(py_floor_div_unsigned(7_u8, 2), 3_u8);
    assert_eq!(py_mod_unsigned(7_u8, 2), 1_u8);
    assert_eq!(py_floor_div_unsigned(u128::MAX, 2), u128::MAX / 2);
    assert_eq!(py_mod_unsigned(10_usize, 4), 2_usize);
}

/// An unsigned zero divisor raises `ZeroDivisionError`, not Rust's native panic (#1813).
#[test]
#[should_panic(expected = "ZeroDivisionError: integer division or modulo by zero")]
fn unsigned_floor_division_by_zero_raises_zero_division_error_issue1813() {
    let _ = py_floor_div_unsigned(7_u32, 0);
}

/// An unsigned zero divisor raises `ZeroDivisionError` for `%` too (#1813).
#[test]
#[should_panic(expected = "ZeroDivisionError: integer division or modulo by zero")]
fn unsigned_modulo_by_zero_raises_zero_division_error_issue1813() {
    let _ = py_mod_unsigned(7_u64, 0);
}

/// RFC 009: signed floor division and modulo keep their exact width and compute Python's results.
#[test]
fn signed_floor_division_and_modulo_keep_their_type() {
    assert_eq!(py_floor_div_signed(-7_i8, 2), -4_i8);
    assert_eq!(py_floor_div_signed(7_i16, -2), -4_i16);
    assert_eq!(py_floor_div_signed(-7_i32, -2), 3_i32);
    assert_eq!(py_floor_div_signed(6_i128, 3), 2_i128);
    assert_eq!(py_mod_signed(-7_i8, 3), 2_i8);
    assert_eq!(py_mod_signed(7_i16, -3), -2_i16);
    assert_eq!(py_mod_signed(-7_isize, -3), -1_isize);
    assert_eq!(py_mod_signed(i8::MIN, -1), 0_i8);
    for dividend in -20_i32..=20 {
        for divisor in [-7_i32, -3, -1, 1, 2, 5] {
            let (dividend_i64, divisor_i64) = (i64::from(dividend), i64::from(divisor));
            assert_eq!(
                i64::from(py_floor_div_signed(dividend, divisor)),
                py_floor_div_i64(dividend_i64, divisor_i64)
            );
            assert_eq!(
                i64::from(py_mod_signed(dividend, divisor)),
                py_mod_i64(dividend_i64, divisor_i64)
            );
        }
    }
}

/// A signed zero divisor raises `ZeroDivisionError`, not Rust's native panic.
#[test]
#[should_panic(expected = "ZeroDivisionError: integer division or modulo by zero")]
fn signed_modulo_by_zero_raises_zero_division_error() {
    let _ = py_mod_signed(7_i16, 0);
}

/// The minimum of a signed type floor-divided by `-1` does not fit the type and raises `ValueError`.
#[test]
#[should_panic(expected = "ValueError: integer floor division result overflows i8")]
fn signed_floor_division_overflow_raises_value_error() {
    let _ = py_floor_div_signed(i8::MIN, -1);
}

/// Decimal equality, ordering and hashing are numeric, whatever scale a value was written with (#1810).
#[test]
fn decimal_equality_ordering_and_hashing_are_numeric_issue1810() {
    use std::collections::HashSet;

    let one_point_five = Decimal128::from_literal("1.5d");
    let one_point_fifty = Decimal128::from_literal("1.50d");
    let one_point_forty_nine = Decimal128::from_literal("1.49d");
    assert_eq!(one_point_five, one_point_fifty);
    assert!(one_point_forty_nine < one_point_five);
    assert!(one_point_fifty > one_point_forty_nine);
    assert_eq!(one_point_five.cmp(&one_point_fifty), Ordering::Equal);
    assert!(Decimal128::from_literal("-1.5d") < Decimal128::from_literal("-1.49d"));
    assert_eq!(Decimal128::from_literal("0.00d"), Decimal128::from_literal("0d"));

    let set: HashSet<Decimal128> = [one_point_five, one_point_fifty, one_point_forty_nine]
        .into_iter()
        .collect();
    assert_eq!(set.len(), 2);

    // Display keeps the written scale.
    assert_eq!(one_point_fifty.to_string(), "1.50");
    assert_eq!(one_point_five.to_string(), "1.5");
}

// --- NaN/Inf divergence (documented) ---

#[test]
fn test_nan_behavior_mod() {
    let res = py_mod(f64::NAN, 2.0_f64);
    assert!(res.is_nan());
}

#[test]
fn test_inf_behavior_mod() {
    let res = py_mod(f64::INFINITY, 2.0_f64);
    assert!(res.is_nan() || res.is_infinite()); // IEEE behavior; documented divergence
}
