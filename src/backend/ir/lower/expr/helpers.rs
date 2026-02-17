//! Small helper utilities for expression lowering: pow exponent classification and literal extraction.

use super::super::super::types::IrType;
use super::super::AstLowering;
use crate::frontend::ast::{self, Spanned};
use incan_core::PowExponentKind;

impl AstLowering {
    /// Determine `PowExponentKind` for a power expression's right operand.
    ///
    /// Used to implement Python-like `**` semantics where `int ** int` yields `Int` only for non-negative int literal
    /// exponents; otherwise `Float`.
    pub(in crate::backend::ir::lower) fn pow_exponent_kind(
        right_ast: &Spanned<ast::Expr>,
        right_ty: &IrType,
    ) -> PowExponentKind {
        let rhs_is_float = matches!(right_ty, IrType::Float);
        let rhs_int_literal = Self::extract_int_literal(right_ast);
        PowExponentKind::from_literal_info(rhs_is_float, rhs_int_literal)
    }

    /// Extract an integer literal value from an AST expression.
    pub(in crate::backend::ir::lower) fn extract_int_literal(expr: &Spanned<ast::Expr>) -> Option<i64> {
        match &expr.node {
            ast::Expr::Literal(ast::Literal::Int(n)) => Some(*n),
            ast::Expr::Unary(ast::UnaryOp::Neg, inner) => {
                if let ast::Expr::Literal(ast::Literal::Int(n)) = &inner.node {
                    Some(-n)
                } else {
                    None
                }
            }
            ast::Expr::Paren(inner) => Self::extract_int_literal(inner),
            _ => None,
        }
    }
}
