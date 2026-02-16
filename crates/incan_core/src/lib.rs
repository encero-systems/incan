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
