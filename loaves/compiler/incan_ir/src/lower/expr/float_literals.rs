//! Integer literals the checker typed as binary floats.

use super::super::super::TypedExpr;
use super::super::super::expr::{IrExprKind, UnaryOp};
use super::super::super::types::IrType;
use super::super::AstLowering;
use incan_frontend::ast;
use incan_lang::lang::types::numerics::NumericTypeId;

impl AstLowering {
    /// Write an integer literal whose checked type is `float`, `f32` or `f64` as the float literal of the same value.
    ///
    /// The checker gives an integer literal the float type its destination expects, wherever the destination is: a
    /// declaration (`x: float = 1`), a reassignment of a `float` binding (`f = 1`), a parameter, a return, a field, or
    /// an element of a tuple or a list whose element type is a float. It refuses every other integer-to-float
    /// assignment (`f = n` with `n: int`). The generated Rust has no integer-to-float literal coercion, so the
    /// literal is lowered as a float literal of its checked type. A negated literal (`-3`) keeps its negation,
    /// around a float operand; the checker records the float type on the negation, not on the literal inside it.
    /// `source` is the expression `lowered` was lowered from, whose literal carries the full magnitude of an
    /// integer too wide for `i64`. Every other expression is left unchanged.
    pub(super) fn write_float_typed_int_literal_as_float(lowered: &mut TypedExpr, source: &ast::Expr) {
        if !Self::is_binary_float_type(&lowered.ty) {
            return;
        }
        let float_ty = lowered.ty.clone();
        match source {
            ast::Expr::Literal(ast::Literal::Int(literal)) => {
                Self::write_int_literal_as_float(lowered, literal, float_ty);
            }
            ast::Expr::Unary(ast::UnaryOp::Neg, operand_source) => {
                let ast::Expr::Literal(ast::Literal::Int(literal)) = &operand_source.node else {
                    return;
                };
                if let IrExprKind::UnaryOp {
                    op: UnaryOp::Neg,
                    operand,
                } = &mut lowered.kind
                {
                    Self::write_int_literal_as_float(operand, literal, float_ty);
                }
            }
            _ => {}
        }
    }

    /// Replace a lowered integer literal with the float literal of `literal`'s magnitude, typed `float_ty`.
    ///
    /// The checker has already refused a magnitude that is not finite in the float type, so the conversion to `f64`
    /// only rounds a magnitude wider than the float's mantissa, as the float literal of that value would.
    fn write_int_literal_as_float(lowered: &mut TypedExpr, literal: &ast::IntLiteral, float_ty: IrType) {
        if !matches!(lowered.kind, IrExprKind::Int(_) | IrExprKind::IntLiteral(_)) {
            return;
        }
        lowered.kind = IrExprKind::Float(literal.magnitude as f64);
        lowered.ty = float_ty;
    }

    /// Return whether `ty` is one of the binary float types: `float`, `f32` or `f64`.
    fn is_binary_float_type(ty: &IrType) -> bool {
        matches!(
            ty,
            IrType::Float | IrType::Numeric(NumericTypeId::F32 | NumericTypeId::F64)
        )
    }
}
