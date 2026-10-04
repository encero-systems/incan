//! Scalar places, constants, and expressions in the pinned rustc MIR vocabulary.

use crate::error::PlanError;
use crate::plan::{BinaryOp, Constant, Operand, OperandKind, Place, PlanType, Projection, RvalueKind, UnaryOp};
use crate::spans::Sources;
use rustc_abi::FieldIdx;
use rustc_middle::mir::{self, interpret::Scalar};
use rustc_middle::ty::TyCtxt;
use rustc_span::Span;

/// Convert a prevalidated signed index without truncation.
pub fn index(value: i64) -> Result<usize, PlanError> {
    usize::try_from(value).map_err(|_| PlanError::Invalid {
        function: "index".into(),
        reason: format!("invalid index {value}"),
    })
}

/// Map a local or checked-pair projection to rustc's place representation.
pub fn place<'tcx>(tcx: TyCtxt<'tcx>, value: &Place) -> Result<mir::Place<'tcx>, PlanError> {
    let local = mir::Local::from_usize(index(value.local)?);
    let place = mir::Place::from(local);
    Ok(match &value.projection {
        Projection::Whole => place,
        Projection::Value => tcx.mk_place_field(place, FieldIdx::from_u32(0), tcx.types.i64),
        Projection::Overflow => tcx.mk_place_field(place, FieldIdx::from_u32(1), tcx.types.bool),
    })
}

/// Translate explicit ownership intent and scalar constants, retaining the operand's source span.
pub fn operand<'tcx>(
    tcx: TyCtxt<'tcx>,
    sources: &Sources<'_>,
    value: &Operand,
) -> Result<mir::Operand<'tcx>, PlanError> {
    let span = sources.span(&value.span)?;
    Ok(match &value.kind {
        OperandKind::Copy(value) => mir::Operand::Copy(place(tcx, value)?),
        OperandKind::Move(value) => mir::Operand::Move(place(tcx, value)?),
        OperandKind::Literal(value) => constant(tcx, value, span),
    })
}

/// Encode fixed-width scalar constants without changing their bit representation.
fn constant<'tcx>(tcx: TyCtxt<'tcx>, value: &Constant, span: Span) -> mir::Operand<'tcx> {
    match value {
        Constant::Int(value) => mir::Operand::const_from_scalar(tcx, tcx.types.i64, Scalar::from_i64(*value), span),
        Constant::Float(value) => {
            mir::Operand::const_from_scalar(tcx, tcx.types.f64, Scalar::from_u64(value.to_bits()), span)
        }
        Constant::Bool(value) => mir::Operand::const_from_scalar(tcx, tcx.types.bool, Scalar::from_bool(*value), span),
        Constant::Unit => mir::Operand::Constant(Box::new(mir::ConstOperand {
            span,
            user_ty: None,
            const_: mir::Const::Val(mir::ConstValue::ZeroSized, tcx.types.unit),
        })),
    }
}

/// Map an operation name; overflow variants are selected only for int arithmetic.
pub fn binary(op: &BinaryOp, checked: bool) -> mir::BinOp {
    match op {
        BinaryOp::Add if checked => mir::BinOp::AddWithOverflow,
        BinaryOp::Subtract if checked => mir::BinOp::SubWithOverflow,
        BinaryOp::Multiply if checked => mir::BinOp::MulWithOverflow,
        BinaryOp::Add => mir::BinOp::Add,
        BinaryOp::Subtract => mir::BinOp::Sub,
        BinaryOp::Multiply => mir::BinOp::Mul,
        BinaryOp::Divide => mir::BinOp::Div,
        BinaryOp::Equal => mir::BinOp::Eq,
        BinaryOp::NotEqual => mir::BinOp::Ne,
        BinaryOp::Less => mir::BinOp::Lt,
        BinaryOp::LessEqual => mir::BinOp::Le,
        BinaryOp::Greater => mir::BinOp::Gt,
        BinaryOp::GreaterEqual => mir::BinOp::Ge,
    }
}

/// Translate a scalar expression; preflight validation has established all result and operand types.
pub fn rvalue<'tcx>(
    tcx: TyCtxt<'tcx>,
    sources: &Sources<'_>,
    value: &RvalueKind,
    destination_type: &PlanType,
) -> Result<mir::Rvalue<'tcx>, PlanError> {
    Ok(match value {
        RvalueKind::Use(value) => mir::Rvalue::Use(operand(tcx, sources, value)?, mir::WithRetag::Yes),
        RvalueKind::IntToFloat(value) => {
            mir::Rvalue::Cast(mir::CastKind::IntToFloat, operand(tcx, sources, value)?, tcx.types.f64)
        }
        RvalueKind::Unary(op, value) => mir::Rvalue::UnaryOp(
            match op {
                UnaryOp::Not => mir::UnOp::Not,
                UnaryOp::Negate => mir::UnOp::Neg,
            },
            operand(tcx, sources, value)?,
        ),
        RvalueKind::Binary(op, left, right) => mir::Rvalue::BinaryOp(
            binary(op, matches!(destination_type, PlanType::CheckedInt)),
            Box::new((operand(tcx, sources, left)?, operand(tcx, sources, right)?)),
        ),
    })
}
