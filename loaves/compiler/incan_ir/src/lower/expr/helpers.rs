//! Small helper utilities for expression lowering: pow exponent classification, literal extraction, and the
//! no-argument `count()` on a list.

use super::super::super::expr::{BuiltinFn, IrExprKind};
use super::super::super::types::IrType;
use super::super::AstLowering;
use crate::TypedExpr;
use incan_frontend::ast::{self, Spanned};
use incan_lang::PowExponentKind;

impl AstLowering {
    /// Determine `PowExponentKind` for a power expression's right operand.
    ///
    /// Used to implement Python-like `**` semantics where `int ** int` yields `Int` only for non-negative int literal
    /// exponents; otherwise `Float`.
    pub(in crate::lower) fn pow_exponent_kind(right_ast: &Spanned<ast::Expr>, right_ty: &IrType) -> PowExponentKind {
        let rhs_is_float = matches!(right_ty, IrType::Float);
        let rhs_int_literal = Self::extract_int_literal(right_ast);
        PowExponentKind::from_literal_info(rhs_is_float, rhs_int_literal)
    }

    /// Extract an integer literal value from an AST expression.
    pub(in crate::lower) fn extract_int_literal(expr: &Spanned<ast::Expr>) -> Option<i64> {
        match &expr.node {
            ast::Expr::Literal(ast::Literal::Int(il)) => Some(il.value),
            ast::Expr::Unary(ast::UnaryOp::Neg, inner) => {
                if let ast::Expr::Literal(ast::Literal::Int(il)) = &inner.node {
                    Some(-il.value)
                } else {
                    None
                }
            }
            ast::Expr::Paren(inner) => Self::extract_int_literal(inner),
            _ => None,
        }
    }

    /// Lower a no-argument `count()` on a list to the list's length (#1776).
    ///
    /// On a list the argument count picks the form, as the checker resolved it: `count(value)` is the list method the
    /// collection classification already selects, and `count()` is the iterator terminal, which counts every item of
    /// the list. That count is the list's length, so the call lowers as `len(items)` does: without materializing an
    /// iterator, whose adapter would require the element type to be cloneable and would copy the list to count it.
    pub(in crate::lower) fn lower_list_item_count(receiver: TypedExpr) -> IrExprKind {
        IrExprKind::BuiltinCall {
            func: BuiltinFn::Len,
            args: vec![receiver],
        }
    }
}
