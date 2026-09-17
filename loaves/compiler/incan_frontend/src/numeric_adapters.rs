//! Adapters from the frontend's operators, literals and resolved types into the kernel's numeric policy enums.
use crate::ast::{BinaryOp, Expr, Literal, Spanned, UnaryOp};
use crate::symbols::ResolvedType;
use incan_lang::lang::types::numerics::{self, NumericFamily};
use incan_lang::{NumericOp, NumericTy, PowExponentKind};

/// Map frontend AST BinaryOp to NumericOp.
pub fn numeric_op_from_ast(op: &BinaryOp) -> Option<NumericOp> {
    match op {
        BinaryOp::Add => Some(NumericOp::Add),
        BinaryOp::Sub => Some(NumericOp::Sub),
        BinaryOp::Mul => Some(NumericOp::Mul),
        BinaryOp::Div => Some(NumericOp::Div),
        BinaryOp::FloorDiv => Some(NumericOp::FloorDiv),
        BinaryOp::Mod => Some(NumericOp::Mod),
        BinaryOp::Pow => Some(NumericOp::Pow),
        BinaryOp::Eq => Some(NumericOp::Eq),
        BinaryOp::NotEq => Some(NumericOp::NotEq),
        BinaryOp::Lt => Some(NumericOp::Lt),
        BinaryOp::Gt => Some(NumericOp::Gt),
        BinaryOp::LtEq => Some(NumericOp::LtEq),
        BinaryOp::GtEq => Some(NumericOp::GtEq),
        _ => None,
    }
}

/// Map ResolvedType to NumericTy.
pub fn numeric_ty_from_resolved(ty: &ResolvedType) -> Option<NumericTy> {
    match ty {
        ResolvedType::Int => Some(NumericTy::Int),
        ResolvedType::Float => Some(NumericTy::Float),
        ResolvedType::Numeric(id) => match numerics::info_for(*id).family {
            NumericFamily::SignedInteger | NumericFamily::UnsignedInteger => Some(NumericTy::Int),
            NumericFamily::BinaryFloat => Some(NumericTy::Float),
            NumericFamily::Bool => None,
        },
        _ => None,
    }
}

/// Determine PowExponentKind from an AST expression and its resolved type.
pub fn pow_exponent_kind_from_ast(expr: &Spanned<Expr>, ty: &ResolvedType) -> PowExponentKind {
    let rhs_is_float = matches!(numeric_ty_from_resolved(ty), Some(NumericTy::Float));
    let rhs_int_literal = extract_int_literal(expr);
    PowExponentKind::from_literal_info(rhs_is_float, rhs_int_literal)
}

/// The value of an integer-literal exponent, looking through negation and parentheses; `None` for any other
/// expression, which the power lowering then treats as a runtime exponent.
fn extract_int_literal(expr: &Spanned<Expr>) -> Option<i64> {
    match &expr.node {
        Expr::Literal(Literal::Int(il)) => Some(il.value),
        Expr::Unary(UnaryOp::Neg, inner) => {
            if let Expr::Literal(Literal::Int(il)) = &inner.node {
                Some(-il.value)
            } else {
                None
            }
        }
        Expr::Paren(inner) => extract_int_literal(inner),
        _ => None,
    }
}
