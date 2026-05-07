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
use core::fmt;

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

/// Runtime representation for RFC 009 `decimal[p, s]` values.
///
/// Precision and scale are checked by the compiler at Incan boundaries. The runtime keeps the coefficient plus
/// literal scale so generated programs have a stable, toolchain-owned Rust type without depending on a third-party
/// decimal crate before arithmetic semantics are specified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Decimal128 {
    coefficient: i128,
    scale: u8,
}

impl Decimal128 {
    /// Construct a decimal from its scaled integer coefficient and scale.
    pub fn new(coefficient: i128, scale: u8) -> Self {
        Self { coefficient, scale }
    }

    /// Return the scaled integer coefficient.
    pub fn coefficient(self) -> i128 {
        self.coefficient
    }

    /// Return the number of fractional decimal digits represented by the coefficient.
    pub fn scale(self) -> u8 {
        self.scale
    }

    /// Parse a compiler-validated decimal literal spelling into the runtime representation.
    pub fn from_literal(literal: &str) -> Self {
        let body = literal.strip_suffix('d').unwrap_or(literal);
        let Some((coefficient, scale)) = parse_decimal_literal_body(body) else {
            raise_value_error(&format!("invalid decimal literal `{literal}`"));
        };
        Self::new(coefficient, scale)
    }
}

impl fmt::Display for Decimal128 {
    /// Format the decimal using its stored scale.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.scale == 0 {
            return write!(f, "{}", self.coefficient);
        }
        let negative = self.coefficient < 0;
        let digits = self.coefficient.unsigned_abs().to_string();
        let scale = usize::from(self.scale);
        if digits.len() <= scale {
            if negative {
                write!(f, "-")?;
            }
            write!(f, "0.")?;
            for _ in 0..(scale - digits.len()) {
                write!(f, "0")?;
            }
            write!(f, "{digits}")
        } else {
            let split = digits.len() - scale;
            if negative {
                write!(f, "-")?;
            }
            write!(f, "{}.{}", &digits[..split], &digits[split..])
        }
    }
}

/// Parse the decimal literal body after the optional `d` suffix has been removed.
fn parse_decimal_literal_body(body: &str) -> Option<(i128, u8)> {
    if body.is_empty() || body.contains('e') || body.contains('E') {
        return None;
    }
    let (integer, fractional) = body.split_once('.').unwrap_or((body, ""));
    if integer.is_empty() && fractional.is_empty() {
        return None;
    }
    let scale = u8::try_from(fractional.len()).ok()?;
    let mut coefficient = String::with_capacity(integer.len() + fractional.len());
    coefficient.push_str(if integer.is_empty() { "0" } else { integer });
    coefficient.push_str(fractional);
    coefficient.parse::<i128>().ok().map(|value| (value, scale))
}

pub trait IncanTryResize<T> {
    /// Attempt an integer resize and return `None` when the target cannot represent the value.
    fn incan_try_resize(self) -> Option<T>;
}

impl<T, U> IncanTryResize<U> for T
where
    U: TryFrom<T>,
{
    /// Attempt an integer resize using the target type's `TryFrom` implementation.
    fn incan_try_resize(self) -> Option<U> {
        U::try_from(self).ok()
    }
}

/// Attempt an integer resize and return `None` when the target cannot represent the value.
pub fn try_resize<T, U>(value: T) -> Option<U>
where
    T: IncanTryResize<U>,
{
    value.incan_try_resize()
}

pub trait IncanSaturatingResize<T> {
    /// Resize an integer and clamp to the target range when the value is outside it.
    fn incan_saturating_resize(self) -> T;
}

/// Resize an integer and clamp to the target range when the value is outside it.
pub fn saturating_resize<T, U>(value: T) -> U
where
    T: IncanSaturatingResize<U>,
{
    value.incan_saturating_resize()
}

macro_rules! impl_saturating_signed_to_signed_for_src {
    ($src:ty => $($dst:ty),* $(,)?) => {
        $(
            impl IncanSaturatingResize<$dst> for $src {
                /// Resize an integer and clamp to the signed target range.
                fn incan_saturating_resize(self) -> $dst {
                    (self as i128).clamp(<$dst>::MIN as i128, <$dst>::MAX as i128) as $dst
                }
            }
        )*
    };
}

macro_rules! impl_saturating_signed_to_signed {
    ($($src:ty),* $(,)?) => {
        $(impl_saturating_signed_to_signed_for_src!($src => i8, i16, i32, i64, i128, isize);)*
    };
}

macro_rules! impl_saturating_signed_to_unsigned_for_src {
    ($src:ty => $($dst:ty),* $(,)?) => {
        $(
            impl IncanSaturatingResize<$dst> for $src {
                /// Resize a signed integer and clamp negative values to zero for unsigned targets.
                fn incan_saturating_resize(self) -> $dst {
                    if self <= 0 {
                        0
                    } else {
                        (self as u128).min(<$dst>::MAX as u128) as $dst
                    }
                }
            }
        )*
    };
}

macro_rules! impl_saturating_signed_to_unsigned {
    ($($src:ty),* $(,)?) => {
        $(impl_saturating_signed_to_unsigned_for_src!($src => u8, u16, u32, u64, u128, usize);)*
    };
}

macro_rules! impl_saturating_unsigned_to_signed_for_src {
    ($src:ty => $($dst:ty),* $(,)?) => {
        $(
            impl IncanSaturatingResize<$dst> for $src {
                /// Resize an unsigned integer and clamp to the signed target maximum.
                fn incan_saturating_resize(self) -> $dst {
                    (self as u128).min(<$dst>::MAX as u128) as $dst
                }
            }
        )*
    };
}

macro_rules! impl_saturating_unsigned_to_signed {
    ($($src:ty),* $(,)?) => {
        $(impl_saturating_unsigned_to_signed_for_src!($src => i8, i16, i32, i64, i128, isize);)*
    };
}

macro_rules! impl_saturating_unsigned_to_unsigned_for_src {
    ($src:ty => $($dst:ty),* $(,)?) => {
        $(
            impl IncanSaturatingResize<$dst> for $src {
                /// Resize an unsigned integer and clamp to the unsigned target maximum.
                fn incan_saturating_resize(self) -> $dst {
                    (self as u128).min(<$dst>::MAX as u128) as $dst
                }
            }
        )*
    };
}

macro_rules! impl_saturating_unsigned_to_unsigned {
    ($($src:ty),* $(,)?) => {
        $(impl_saturating_unsigned_to_unsigned_for_src!($src => u8, u16, u32, u64, u128, usize);)*
    };
}

impl_saturating_signed_to_signed!(i8, i16, i32, i64, i128, isize);
impl_saturating_signed_to_unsigned!(i8, i16, i32, i64, i128, isize);
impl_saturating_unsigned_to_signed!(u8, u16, u32, u64, u128, usize);
impl_saturating_unsigned_to_unsigned!(u8, u16, u32, u64, u128, usize);

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
