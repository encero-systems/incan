//! Adapters from IR operators, expressions and types into the kernel's numeric policy enums; the frontend-side
//! adapters live in `incan_frontend::numeric_adapters`.
use crate::expr::{BinOp as IrBinOp, IrExprKind, TypedExpr, UnaryOp as IrUnaryOp};
use crate::types::IrType;
use incan_core::lang::types::numerics::{self, NumericFamily};
use incan_core::{NumericOp, NumericTy, PowExponentKind};
pub use incan_frontend::numeric_adapters::{numeric_op_from_ast, numeric_ty_from_resolved, pow_exponent_kind_from_ast};

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
