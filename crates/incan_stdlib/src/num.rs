//! Python-like numeric operations for Incan-generated Rust code.
//!
//! This module provides:
//! - Generic entry points (`py_div`, `py_mod`, `py_floor_div`) working across supported ints/floats.
//! - Specialized suffixed helpers (`*_i64`, `*_f64`) kept for compatibility and tests.
//!
//! Key behaviors:
//! - `py_div`: always returns `f64`.
//! - `py_mod`: remainder has the sign of the divisor (Python semantics).
//! - `py_floor_div`: rounds toward negative infinity (Python `//`).
//! - Zero division (int or float) panics with `ZeroDivisionError: float division by zero`.
//! - NaN/Inf follow IEEE/Rust behavior (documented divergence from Python).
//!
//! ## Examples
//!
//! ```rust
//! use incan_stdlib::num::{py_div, py_floor_div, py_mod};
//!
//! assert_eq!(py_floor_div(7_i64, 3_i64), 2);
//! assert!((py_mod(-7.0_f64, 3.0_f64) - 2.0).abs() < 1e-10);
//! assert!((py_div(7, 2) - 3.5).abs() < 1e-10);
//! ```

/// Python-style floor division for integers.
///
/// Rounds toward negative infinity (unlike Rust's `/` which truncates toward zero).
///
/// ## Examples
///
/// ```
/// use incan_stdlib::num::py_floor_div_i64;
/// assert_eq!(py_floor_div_i64(7, 3), 2);
/// assert_eq!(py_floor_div_i64(-7, 3), -3); // Rust would give -2
/// assert_eq!(py_floor_div_i64(7, -3), -3); // Rust would give -2
/// assert_eq!(py_floor_div_i64(-7, -3), 2);
/// ```
use crate::errors::{raise_value_error, raise_zero_division};

// --- Numeric kernels (hot) ---------------------------------------------------------------------
//
// These are implemented in `incan_stdlib` (rather than calling into `incan_core`) so they can be
// inlined aggressively into generated binaries without requiring LTO.

#[inline(always)]
fn py_mod_i64_impl(a: i64, b: i64) -> i64 {
    debug_assert!(b != 0);
    // Fast path for the dominant benchmark/runtime shape (`a >= 0`, `b > 0`).
    // For this domain, Rust `%` already matches Python modulo semantics.
    //
    // TODO(perf): Teach lowering/codegen to emit native `%` directly when positivity is proven,
    //             so hot loops can bypass helper calls entirely.
    if a >= 0 && b > 0 {
        return a % b;
    }
    // Use `wrapping_rem` to avoid the extra overflow guard Rust emits for `i64::MIN % -1`.
    // (We still panic on division by zero at the wrapper layer for consistent Python-like errors.)
    let r = a.wrapping_rem(b);
    // Python semantics: remainder has the sign of the divisor.
    if (r > 0 && b < 0) || (r < 0 && b > 0) { r + b } else { r }
}

#[inline]
fn py_floor_div_i64_impl(a: i64, b: i64) -> i64 {
    debug_assert!(b != 0);
    let q = a / b;
    let r = a % b;
    // Python semantics: quotient rounds toward negative infinity.
    if r == 0 {
        return q;
    }
    if b > 0 {
        if r < 0 { q - 1 } else { q }
    } else if r > 0 {
        q - 1
    } else {
        q
    }
}

#[inline]
fn py_mod_f64_impl(a: f64, b: f64) -> f64 {
    debug_assert!(b != 0.0);
    let r = a % b;
    if (r > 0.0 && b < 0.0) || (r < 0.0 && b > 0.0) {
        r + b
    } else {
        r
    }
}

#[inline]
fn gcd_u64_impl(mut m: u64, mut n: u64) -> u64 {
    if m == 0 || n == 0 {
        return m | n;
    }

    let shift = (m | n).trailing_zeros();

    m >>= m.trailing_zeros();
    n >>= n.trailing_zeros();

    while m != n {
        if m > n {
            m -= n;
            m >>= m.trailing_zeros();
        } else {
            n -= m;
            n >>= n.trailing_zeros();
        }
    }

    m << shift
}

#[inline]
fn non_negative_i64_or_overflow(value: u64, fn_name: &str) -> i64 {
    match i64::try_from(value) {
        Ok(value) => value,
        Err(_) => raise_value_error(&format!("{fn_name} result overflows Incan int")),
    }
}

// --- Generic helpers and sealed traits ---------------------------------------------------------

mod sealed {
    // TODO: Consider making the "canonical float" configurable (e.g., via a type alias or associated type) if we add
    //       multiple float backends (f32/f64) or want platform-specific tuning. Today we canonicalize to f64.

    // --- Sealed traits ---
    /// Sealing trait to restrict external implementations.
    pub trait Sealed {}
    impl Sealed for i64 {}
    impl Sealed for f64 {}

    // --- Incan integer types ---
    /// Marker for Incan integer types (future-friendly: add i8/i16/i32/i64/etc.).
    pub trait IncanInt: Sealed {
        fn to_float(self) -> f64;
        fn is_zero(&self) -> bool;
    }

    impl IncanInt for i64 {
        #[inline]
        fn to_float(self) -> f64 {
            self as f64
        }
        #[inline]
        fn is_zero(&self) -> bool {
            *self == 0
        }
    }

    // --- Incan float types ---
    /// Marker for Incan float types (future-friendly: add f32/f64/etc.).
    pub trait IncanFloat: Sealed {
        fn to_float(self) -> f64;
        fn is_zero(&self) -> bool;
    }

    impl IncanFloat for f64 {
        #[inline]
        fn to_float(self) -> f64 {
            self
        }
        #[inline]
        fn is_zero(&self) -> bool {
            *self == 0.0
        }
    }

    // --- Unified numeric trait ---
    /// Unified numeric trait to allow shared bounds across supported ints/floats.
    pub trait IncanNumeric: Sealed {
        fn to_float(self) -> f64;
        fn is_zero(&self) -> bool;
    }

    impl IncanNumeric for i64 {
        #[inline]
        fn to_float(self) -> f64 {
            <Self as IncanInt>::to_float(self)
        }
        #[inline]
        fn is_zero(&self) -> bool {
            <Self as IncanInt>::is_zero(self)
        }
    }

    impl IncanNumeric for f64 {
        #[inline]
        fn to_float(self) -> f64 {
            <Self as IncanFloat>::to_float(self)
        }
        #[inline]
        fn is_zero(&self) -> bool {
            <Self as IncanFloat>::is_zero(self)
        }
    }
}

/// Python-like division: always returns `f64`, uses `f64` math.
///
/// ## Returns
///
/// - (`f64`): the quotient `lhs / rhs`
///
/// ## Panics
///
/// - `ZeroDivisionError: float division by zero` if `rhs` is zero (finite zero)
///
/// ## Notes
///
/// - NaN/Inf follow IEEE/Rust behavior (documented divergence from Python).
///
/// ## Examples
///
/// ```incan
/// result = py_div(7, 2)  # result is 3.5
/// # semantically the same as:
/// # result = 7 / 2
/// ```
///
/// ```rust
/// use incan_stdlib::num::py_div;
/// assert!((py_div(7_i64, 2_i64) - 3.5).abs() < 1e-10);
/// assert!((py_div(7_i64, 2.0_f64) - 3.5).abs() < 1e-10);
/// ```
pub fn py_div<L, R>(lhs: L, rhs: R) -> f64
where
    L: sealed::IncanNumeric,
    R: sealed::IncanNumeric,
{
    let l: f64 = lhs.to_float();
    let r: f64 = rhs.to_float();
    if r == 0.0 {
        raise_zero_division();
    }
    l / r
}

/// Python-like modulo (remainder has the sign of the divisor).
///
/// ## Returns
///
/// - Same numeric type as the operation result (see implementations)
///
/// ## Panics
///
/// - `ZeroDivisionError: float division by zero` if `rhs` is zero (finite zero)
///
/// ## Notes
///
/// - Remainder sign matches the divisor (Python semantics).
/// - NaN/Inf follow IEEE/Rust behavior (documented divergence from Python).
///
/// ## Examples
///
/// ```incan
/// result = py_mod(7, 2)  # result is 1
/// # semantically the same as:
/// # result = 7 % 2
/// ```
///
/// ```rust
/// use incan_stdlib::num::py_mod;
/// assert_eq!(py_mod(7_i64, 3_i64), 1);
/// assert!((py_mod(-7.0_f64, 3.0_f64) - 2.0).abs() < 1e-10);
/// ```
#[inline]
pub fn py_mod<L, R>(lhs: L, rhs: R) -> <L as PyModImpl<R>>::Output
where
    L: PyModImpl<R>,
    R: sealed::IncanNumeric + Copy,
{
    if rhs.is_zero() {
        raise_zero_division();
    }
    <L as PyModImpl<R>>::py_mod(lhs, rhs)
}

/// Python-like floor division (rounds toward negative infinity).
///
/// ## Returns
///
/// - Same numeric type as the operation result (see implementations)
///
/// ## Panics
///
/// - `ZeroDivisionError: float division by zero` if `rhs` is zero (finite zero)
///
/// ## Notes
///
/// - Rounds toward negative infinity (Python `//`).
/// - NaN/Inf follow IEEE/Rust behavior (documented divergence from Python).
///
/// ## Examples
///
/// ```incan
/// result = py_floor_div(7, 2)  # result is 3
/// # semantically the same as:
/// # result = 7 // 2
/// ```
///
/// ```rust
/// use incan_stdlib::num::py_floor_div;
/// assert_eq!(py_floor_div(7_i64, 3_i64), 2);
/// assert!((py_floor_div(-7.0_f64, 3.0_f64) + 3.0).abs() < 1e-10);
/// ```
#[inline]
pub fn py_floor_div<L, R>(lhs: L, rhs: R) -> <L as PyFloorDivImpl<R>>::Output
where
    L: PyFloorDivImpl<R>,
    R: sealed::IncanNumeric + Copy,
{
    if rhs.is_zero() {
        raise_zero_division();
    }
    <L as PyFloorDivImpl<R>>::py_floor_div(lhs, rhs)
}

// --- Internal helpers reused across impls ------------------------------------------------------

// --- Python-like modulo -----------------------------------------------------------------------

/// Trait for Python-like modulo across type pairs.
pub trait PyModImpl<Rhs>: sealed::Sealed {
    type Output;
    fn py_mod(self, rhs: Rhs) -> Self::Output;
}

impl PyModImpl<i64> for i64 {
    type Output = i64;
    #[inline]
    fn py_mod(self, rhs: i64) -> Self::Output {
        py_mod_i64_impl(self, rhs)
    }
}

impl PyModImpl<f64> for i64 {
    type Output = f64;
    #[inline]
    fn py_mod(self, rhs: f64) -> Self::Output {
        py_mod_f64_impl(self as f64, rhs)
    }
}

impl PyModImpl<i64> for f64 {
    type Output = f64;
    #[inline]
    fn py_mod(self, rhs: i64) -> Self::Output {
        py_mod_f64_impl(self, rhs as f64)
    }
}

impl PyModImpl<f64> for f64 {
    type Output = f64;
    #[inline]
    fn py_mod(self, rhs: f64) -> Self::Output {
        py_mod_f64_impl(self, rhs)
    }
}

impl PyFloorDivImpl<i64> for i64 {
    type Output = i64;
    #[inline]
    fn py_floor_div(self, rhs: i64) -> Self::Output {
        py_floor_div_i64_impl(self, rhs)
    }
}

// --- Python-like floor division ----------------------------------------------------------------

/// Trait for Python-like floor division across type pairs.
pub trait PyFloorDivImpl<Rhs>: sealed::Sealed {
    type Output;
    fn py_floor_div(self, rhs: Rhs) -> Self::Output;
}

impl PyFloorDivImpl<f64> for i64 {
    type Output = f64;
    #[inline]
    fn py_floor_div(self, rhs: f64) -> Self::Output {
        (self as f64 / rhs).floor()
    }
}

impl PyFloorDivImpl<i64> for f64 {
    type Output = f64;
    #[inline]
    fn py_floor_div(self, rhs: i64) -> Self::Output {
        (self / rhs as f64).floor()
    }
}

impl PyFloorDivImpl<f64> for f64 {
    type Output = f64;
    #[inline]
    fn py_floor_div(self, rhs: f64) -> Self::Output {
        (self / rhs).floor()
    }
}

// --- Compatibility wrappers (existing API) ----------------------------------------------------

#[inline]
pub fn py_floor_div_i64(a: i64, b: i64) -> i64 {
    if b == 0 {
        raise_zero_division();
    }
    py_floor_div_i64_impl(a, b)
}

/// Python-style floor division for floats.
///
/// Returns `(a / b).floor()`.
///
/// ## Examples
///
/// ```
/// use incan_stdlib::num::py_floor_div_f64;
/// assert!((py_floor_div_f64(7.0, 3.0) - 2.0).abs() < 1e-10);
/// assert!((py_floor_div_f64(-7.0, 3.0) - (-3.0)).abs() < 1e-10);
/// assert!((py_floor_div_f64(7.0, -3.0) - (-3.0)).abs() < 1e-10);
/// ```
#[inline]
pub fn py_floor_div_f64(a: f64, b: f64) -> f64 {
    if b == 0.0 {
        raise_zero_division();
    }
    (a / b).floor()
}

/// Python-style modulo for integers.
///
/// The result has the same sign as the divisor (unlike Rust's `%`).
///
/// Satisfies: `a == py_floor_div_i64(a, b) * b + py_mod_i64(a, b)`
///
/// ## Examples
///
/// ```
/// use incan_stdlib::num::py_mod_i64;
/// assert_eq!(py_mod_i64(7, 3), 1);
/// assert_eq!(py_mod_i64(-7, 3), 2); // Rust % gives -1
/// assert_eq!(py_mod_i64(7, -3), -2); // Rust % gives 1
/// assert_eq!(py_mod_i64(-7, -3), -1);
/// ```
#[inline(always)]
pub fn py_mod_i64(a: i64, b: i64) -> i64 {
    if b == 0 {
        raise_zero_division();
    }
    py_mod_i64_impl(a, b)
}

/// Python-style modulo for floats.
///
/// The result has the same sign as the divisor.
///
/// ## Examples
///
/// ```
/// use incan_stdlib::num::py_mod_f64;
/// assert!((py_mod_f64(7.0, 3.0) - 1.0).abs() < 1e-10);
/// assert!((py_mod_f64(-7.0, 3.0) - 2.0).abs() < 1e-10);
/// assert!((py_mod_f64(7.0, -3.0) - (-2.0)).abs() < 1e-10);
/// ```
#[inline]
pub fn py_mod_f64(a: f64, b: f64) -> f64 {
    if b == 0.0 {
        raise_zero_division();
    }
    py_mod_f64_impl(a, b)
}

/// Greatest common divisor for signed 64-bit integers.
///
/// The result is always non-negative and matches Python's `math.gcd` behavior for `int` when it
/// fits in Incan's signed 64-bit `int`.
///
/// ## Panics
///
/// Raises `ValueError` if the mathematical result exceeds `i64::MAX`.
#[inline]
pub fn gcd_i64(a: i64, b: i64) -> i64 {
    non_negative_i64_or_overflow(gcd_u64_impl(a.unsigned_abs(), b.unsigned_abs()), "math.gcd")
}

/// Lowest common multiple for signed 64-bit integers.
///
/// Returns `0` if either input is `0`.
///
/// ## Panics
///
/// Raises `ValueError` if the mathematical result exceeds `i64::MAX`.
#[inline]
pub fn lcm_i64(a: i64, b: i64) -> i64 {
    if a == 0 || b == 0 {
        return 0;
    }
    let gcd = gcd_u64_impl(a.unsigned_abs(), b.unsigned_abs());
    let lcm = (a.unsigned_abs() / gcd)
        .checked_mul(b.unsigned_abs())
        .unwrap_or_else(|| raise_value_error("math.lcm result overflows Incan int"));
    non_negative_i64_or_overflow(lcm, "math.lcm")
}

// --- Tests -------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use incan_core::{NumericOp, NumericTy, result_numeric_type};
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
    #[should_panic(expected = "ZeroDivisionError: float division by zero")]
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
    fn test_mod_zero_int() {
        let _ = py_mod(1_i64, 0_i64);
    }

    #[test]
    #[should_panic(expected = "ZeroDivisionError: float division by zero")]
    fn test_mod_zero_float() {
        let _ = py_mod(1.0_f64, 0.0_f64);
    }

    #[test]
    #[should_panic(expected = "ZeroDivisionError: float division by zero")]
    fn test_floor_div_zero_int() {
        let _ = py_floor_div(1_i64, 0_i64);
    }

    #[test]
    #[should_panic(expected = "ZeroDivisionError: float division by zero")]
    fn test_floor_div_zero_float() {
        let _ = py_floor_div(1.0_f64, 0.0_f64);
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
}
