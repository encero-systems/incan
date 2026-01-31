//! Provide shared, pure semantic helpers and canonical language vocabulary for the Incan compiler and runtime.
//!
//! This crate is intentionally small and dependency-light. It contains deterministic helpers that both:
//! - the compiler can use for typechecking/const-eval/lowering decisions, and
//! - the runtime/stdlib can use to enforce the same semantics at runtime.
//!
//! ## Notes
//!
//! - This is a “semantic core” crate: **no IO**, no global state, and no compiler-specific types.
//! - Current scope: numeric policy (Python-like semantics), string semantics (Unicode-scalar indexing/slicing,
//!   comparisons, membership, concat, shared error messages), and canonical language vocabulary.

pub mod errors;
pub mod indexing;
pub mod lang;
pub mod strings;

/// Represent the numeric category used by semantic policy.
///
/// This is not a concrete runtime type. It exists to describe “int-like” and “float-like” behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericTy {
    Int,
    Float,
}

/// Represent a numeric operator subject to promotion/coercion rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericOp {
    Add,
    Sub,
    Mul,
    Div,
    /// `//` (Python-style floor division): returns `Int` for `Int // Int`, otherwise `Float`.
    FloorDiv,
    Mod,
    Pow,
    // Comparisons (for coercion, not result type)
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
}

/// Classify the exponent for `**` so policy can decide `Int` vs `Float` results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowExponentKind {
    /// A non-negative integer literal (e.g., `2`, `0`)
    NonNegativeIntLiteral,
    /// A negative integer literal (e.g., `-1`)
    NegativeIntLiteral,
    /// A variable or non-literal expression
    Variable,
    /// A float literal or expression
    Float,
}

impl PowExponentKind {
    /// Classify a `**` exponent based on literal detection and rhs float-ness.
    ///
    /// ## Parameters
    /// - `rhs_is_float`: whether the exponent expression is a float type.
    /// - `rhs_int_literal`: if the exponent is an integer literal, its value.
    ///
    /// ## Returns
    /// - (`PowExponentKind`): the derived exponent category.
    ///
    /// ## Notes
    /// - This helper does not evaluate expressions; it only classifies based on type + literal-ness.
    pub fn from_literal_info(rhs_is_float: bool, rhs_int_literal: Option<i64>) -> Self {
        if rhs_is_float {
            PowExponentKind::Float
        } else if let Some(val) = rhs_int_literal {
            if val >= 0 {
                PowExponentKind::NonNegativeIntLiteral
            } else {
                PowExponentKind::NegativeIntLiteral
            }
        } else {
            PowExponentKind::Variable
        }
    }
}

/// Determine the numeric result category for a binary operation.
///
/// ## Parameters
/// - `op`: the numeric operator.
/// - `lhs`: numeric category of the left operand.
/// - `rhs`: numeric category of the right operand.
/// - `pow_exp_kind`: exponent classification for `Pow` (`**`) operations.
///
/// ## Returns
/// - (`NumericTy`): `Int` or `Float` per Incan's numeric policy.
///
/// ## Notes
/// - `/` always yields `Float` (even `Int / Int`).
/// - `//`, `%`, `+`, `-`, `*` yield `Float` if either operand is `Float`, otherwise `Int`.
/// - `**` yields `Int` only for `Int ** Int` with a non-negative integer literal exponent; otherwise `Float`.
///
/// ## Examples
/// ```rust
/// use incan_core::{NumericOp, NumericTy, PowExponentKind, result_numeric_type};
/// assert_eq!(
///     result_numeric_type(NumericOp::Div, NumericTy::Int, NumericTy::Int, None),
///     NumericTy::Float
/// );
/// assert_eq!(
///     result_numeric_type(
///         NumericOp::Pow,
///         NumericTy::Int,
///         NumericTy::Int,
///         Some(PowExponentKind::NonNegativeIntLiteral)
///     ),
///     NumericTy::Int
/// );
/// ```
pub fn result_numeric_type(
    op: NumericOp,
    lhs: NumericTy,
    rhs: NumericTy,
    pow_exp_kind: Option<PowExponentKind>,
) -> NumericTy {
    match op {
        NumericOp::Div => NumericTy::Float,

        // FloorDiv: returns int when both are int, float when either is float
        NumericOp::FloorDiv | NumericOp::Mod | NumericOp::Add | NumericOp::Sub | NumericOp::Mul => {
            if lhs == NumericTy::Float || rhs == NumericTy::Float {
                NumericTy::Float
            } else {
                NumericTy::Int
            }
        }

        NumericOp::Pow => {
            // Int result only when: both operands Int AND exponent is non-negative int literal
            if lhs == NumericTy::Int && rhs == NumericTy::Int {
                match pow_exp_kind {
                    Some(PowExponentKind::NonNegativeIntLiteral) => NumericTy::Int,
                    _ => NumericTy::Float,
                }
            } else {
                NumericTy::Float
            }
        }

        // Comparisons don't produce numeric results, but this function is about operand types
        // so we return Float if either side is Float (for coercion purposes).
        NumericOp::Eq | NumericOp::NotEq | NumericOp::Lt | NumericOp::LtEq | NumericOp::Gt | NumericOp::GtEq => {
            if lhs == NumericTy::Float || rhs == NumericTy::Float {
                NumericTy::Float
            } else {
                NumericTy::Int
            }
        }
    }
}

/// Determine what promotions are needed to perform a numeric binary operation.
///
/// ## Parameters
/// - `op`: the numeric operator.
/// - `lhs`: numeric category of the left operand.
/// - `rhs`: numeric category of the right operand.
/// - `pow_exp_kind`: exponent classification for `Pow` (`**`) operations.
///
/// ## Returns
/// - `(bool, bool)`: `(lhs_to_float, rhs_to_float)`; whether each operand should be promoted to `Float`.
///
/// ## Notes
/// - Promotions are driven by the computed result category (see [`result_numeric_type`]).
pub fn needs_float_promotion(
    op: NumericOp,
    lhs: NumericTy,
    rhs: NumericTy,
    pow_exp_kind: Option<PowExponentKind>,
) -> (bool, bool) {
    let result_ty = result_numeric_type(op, lhs, rhs, pow_exp_kind);

    if result_ty == NumericTy::Float {
        (lhs == NumericTy::Int, rhs == NumericTy::Int)
    } else {
        (false, false)
    }
}

/// Check whether an operator is a numeric arithmetic operator.
///
/// ## Returns
/// - (`bool`): `true` for `+`, `-`, `*`, `/`, `//`, `%`, `**`.
pub fn is_numeric_arithmetic_op(op: NumericOp) -> bool {
    matches!(
        op,
        NumericOp::Add
            | NumericOp::Sub
            | NumericOp::Mul
            | NumericOp::Div
            | NumericOp::FloorDiv
            | NumericOp::Mod
            | NumericOp::Pow
    )
}

/// Check whether an operator is a numeric comparison operator.
///
/// ## Returns
/// - (`bool`): `true` for `==`, `!=`, `<`, `<=`, `>`, `>=`.
pub fn is_numeric_comparison_op(op: NumericOp) -> bool {
    matches!(
        op,
        NumericOp::Eq | NumericOp::NotEq | NumericOp::Lt | NumericOp::LtEq | NumericOp::Gt | NumericOp::GtEq
    )
}

// =====================================================================
// Runtime-facing numeric helpers (pure; shared with stdlib)
// =====================================================================

/// Python-like modulo for integers (sign of divisor).
///
/// ## Parameters
/// - `a`: dividend
/// - `b`: divisor (must be non-zero)
///
/// ## Returns
/// - (`i64`): remainder with the sign of the divisor.
#[inline]
pub fn py_mod_i64_impl(a: i64, b: i64) -> i64 {
    debug_assert!(b != 0);
    // Use `wrapping_rem` to avoid treating `i64::MIN % -1` as an overflow case.
    // This better matches Python semantics (which has unbounded ints) for this edge case.
    let r = a.wrapping_rem(b);
    if (r > 0 && b < 0) || (r < 0 && b > 0) { r + b } else { r }
}

/// Python-like floor division for integers (rounds toward negative infinity).
///
/// ## Parameters
/// - `a`: dividend
/// - `b`: divisor (must be non-zero)
///
/// ## Returns
/// - (`i64`): quotient rounded toward negative infinity.
#[inline]
pub fn py_floor_div_i64_impl(a: i64, b: i64) -> i64 {
    debug_assert!(b != 0);
    let q = a / b;
    let r = a % b;
    if (r > 0 && b < 0) || (r < 0 && b > 0) { q - 1 } else { q }
}

/// Python-like modulo for floats (sign of divisor).
///
/// ## Parameters
/// - `a`: dividend
/// - `b`: divisor (must be non-zero)
///
/// ## Returns
/// - (`f64`): remainder with the sign of the divisor.
#[inline]
pub fn py_mod_f64_impl(a: f64, b: f64) -> f64 {
    debug_assert!(b != 0.0);
    let r = a % b;
    if (r > 0.0 && b < 0.0) || (r < 0.0 && b > 0.0) {
        r + b
    } else {
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_div_always_float() {
        assert_eq!(
            result_numeric_type(NumericOp::Div, NumericTy::Int, NumericTy::Int, None),
            NumericTy::Float
        );
        assert_eq!(
            result_numeric_type(NumericOp::Div, NumericTy::Int, NumericTy::Float, None),
            NumericTy::Float
        );
        assert_eq!(
            result_numeric_type(NumericOp::Div, NumericTy::Float, NumericTy::Int, None),
            NumericTy::Float
        );
        assert_eq!(
            result_numeric_type(NumericOp::Div, NumericTy::Float, NumericTy::Float, None),
            NumericTy::Float
        );
    }

    #[test]
    fn test_mod_promotion() {
        assert_eq!(
            result_numeric_type(NumericOp::Mod, NumericTy::Int, NumericTy::Int, None),
            NumericTy::Int
        );
        assert_eq!(
            result_numeric_type(NumericOp::Mod, NumericTy::Int, NumericTy::Float, None),
            NumericTy::Float
        );
        assert_eq!(
            result_numeric_type(NumericOp::Mod, NumericTy::Float, NumericTy::Int, None),
            NumericTy::Float
        );
    }

    #[test]
    fn test_pow_literal_exponent() {
        // Non-negative int literal exponent → Int result
        assert_eq!(
            result_numeric_type(
                NumericOp::Pow,
                NumericTy::Int,
                NumericTy::Int,
                Some(PowExponentKind::NonNegativeIntLiteral)
            ),
            NumericTy::Int
        );
        // Negative int literal → Float result
        assert_eq!(
            result_numeric_type(
                NumericOp::Pow,
                NumericTy::Int,
                NumericTy::Int,
                Some(PowExponentKind::NegativeIntLiteral)
            ),
            NumericTy::Float
        );
        // Variable exponent → Float result
        assert_eq!(
            result_numeric_type(
                NumericOp::Pow,
                NumericTy::Int,
                NumericTy::Int,
                Some(PowExponentKind::Variable)
            ),
            NumericTy::Float
        );
    }

    #[test]
    fn test_needs_float_promotion() {
        assert_eq!(
            needs_float_promotion(NumericOp::Add, NumericTy::Int, NumericTy::Int, None),
            (false, false)
        );
        assert_eq!(
            needs_float_promotion(NumericOp::Add, NumericTy::Int, NumericTy::Float, None),
            (true, false)
        );
        assert_eq!(
            needs_float_promotion(NumericOp::Add, NumericTy::Float, NumericTy::Int, None),
            (false, true)
        );
    }
}
