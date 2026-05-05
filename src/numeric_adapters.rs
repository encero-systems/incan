//! Shared adapters to map frontend/backend types into numeric policy enums.
use crate::backend::ir::expr::{BinOp as IrBinOp, IrExprKind, TypedExpr, UnaryOp as IrUnaryOp};
use crate::backend::ir::types::IrType;
use crate::frontend::ast::{BinaryOp, Expr, Literal, Spanned, UnaryOp};
use crate::frontend::symbols::ResolvedType;
use incan_core::lang::types::numerics::{self, NumericFamily};
use incan_core::{NumericOp, NumericTy, PowExponentKind};

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

/// Map backend IR BinOp to NumericOp.
pub fn numeric_op_from_ir(op: &IrBinOp) -> Option<NumericOp> {
    match op {
        IrBinOp::Add => Some(NumericOp::Add),
        IrBinOp::Sub => Some(NumericOp::Sub),
        IrBinOp::Mul => Some(NumericOp::Mul),
        IrBinOp::Div => Some(NumericOp::Div),
        IrBinOp::FloorDiv => Some(NumericOp::FloorDiv),
        IrBinOp::Mod => Some(NumericOp::Mod),
        IrBinOp::Pow => Some(NumericOp::Pow),
        IrBinOp::Eq => Some(NumericOp::Eq),
        IrBinOp::Ne => Some(NumericOp::NotEq),
        IrBinOp::Lt => Some(NumericOp::Lt),
        IrBinOp::Gt => Some(NumericOp::Gt),
        IrBinOp::Le => Some(NumericOp::LtEq),
        IrBinOp::Ge => Some(NumericOp::GtEq),
        _ => None,
    }
}

/// Map backend IrType to NumericTy.
pub fn ir_type_to_numeric_ty(ty: &IrType) -> Option<NumericTy> {
    match ty {
        IrType::Int => Some(NumericTy::Int),
        IrType::Float => Some(NumericTy::Float),
        IrType::Numeric(id) => match numerics::info_for(*id).family {
            NumericFamily::SignedInteger | NumericFamily::UnsignedInteger => Some(NumericTy::Int),
            NumericFamily::BinaryFloat => Some(NumericTy::Float),
            NumericFamily::Bool => None,
        },
        _ => None,
    }
}

/// Determine PowExponentKind from a typed IR expression.
pub fn pow_exponent_kind_from_ir(expr: &TypedExpr) -> PowExponentKind {
    let rhs_is_float = matches!(expr.ty, IrType::Float);
    let rhs_int_literal = match &expr.kind {
        IrExprKind::Int(n) => Some(*n),
        IrExprKind::UnaryOp {
            op: IrUnaryOp::Neg,
            operand,
        } => {
            if let IrExprKind::Int(n) = &operand.kind {
                Some(-n)
            } else {
                None
            }
        }
        _ => None,
    };
    PowExponentKind::from_literal_info(rhs_is_float, rhs_int_literal)
}

/// Determine PowExponentKind from an AST expression and its resolved type.
pub fn pow_exponent_kind_from_ast(expr: &Spanned<Expr>, ty: &ResolvedType) -> PowExponentKind {
    let rhs_is_float = matches!(numeric_ty_from_resolved(ty), Some(NumericTy::Float));
    let rhs_int_literal = extract_int_literal(expr);
    PowExponentKind::from_literal_info(rhs_is_float, rhs_int_literal)
}

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
