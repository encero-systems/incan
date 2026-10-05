//! Scalar places, constants, and expressions in the pinned rustc MIR vocabulary.

use crate::error::PlanError;
use crate::plan::{BinaryOp, Constant, Operand, OperandKind, Place, PlanType, Projection, RvalueKind, UnaryOp};
use crate::spans::Sources;
use crate::types::native_type;
use rustc_abi::FieldIdx;
use rustc_index::IndexVec;
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
        Projection::Field(slot, ty) => {
            tcx.mk_place_field(place, FieldIdx::from_usize(index(*slot)?), native_type(tcx, ty)?)
        }
        Projection::NumericValue(ty) => tcx.mk_place_field(place, FieldIdx::from_u32(0), native_type(tcx, ty)?),
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
        OperandKind::Literal(value) => constant(tcx, value, span)?,
    })
}

/// Encode fixed-width scalar constants without changing their bit representation.
fn constant<'tcx>(tcx: TyCtxt<'tcx>, value: &Constant, span: Span) -> Result<mir::Operand<'tcx>, PlanError> {
    Ok(match value {
        Constant::Int(value) => mir::Operand::const_from_scalar(tcx, tcx.types.i64, Scalar::from_i64(*value), span),
        Constant::Numeric(text, ty) => numeric_constant(tcx, text, ty, span)?,
        Constant::Float(value) => {
            mir::Operand::const_from_scalar(tcx, tcx.types.f64, Scalar::from_u64(value.to_bits()), span)
        }
        Constant::Bool(value) => mir::Operand::const_from_scalar(tcx, tcx.types.bool, Scalar::from_bool(*value), span),
        Constant::Unit => mir::Operand::Constant(Box::new(mir::ConstOperand {
            span,
            user_ty: None,
            const_: mir::Const::Val(mir::ConstValue::ZeroSized, tcx.types.unit),
        })),
        Constant::Text(text) => {
            let alloc_id = tcx.allocate_bytes_dedup(text.as_bytes(), mir::interpret::CTFE_ALLOC_SALT);
            let meta = u64::try_from(text.len()).map_err(|_| PlanError::Invalid {
                function: "text".into(),
                reason: "string literal length exceeds the native representation".into(),
            })?;
            mir::Operand::Constant(Box::new(mir::ConstOperand {
                span,
                user_ty: None,
                const_: mir::Const::Val(
                    mir::ConstValue::Slice { alloc_id, meta },
                    native_type(tcx, &PlanType::StrRef)?,
                ),
            }))
        }
    })
}

/// Map an operation name; overflow variants are selected only for integer arithmetic.
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
        RvalueKind::NumericCast(value, source, target) => {
            let source_float = matches!(source, PlanType::Float | PlanType::F32 | PlanType::F64);
            let target_float = matches!(target, PlanType::Float | PlanType::F32 | PlanType::F64);
            let kind = match (source_float, target_float) {
                (true, true) => mir::CastKind::FloatToFloat,
                (true, false) => mir::CastKind::FloatToInt,
                (false, true) => mir::CastKind::IntToFloat,
                (false, false) => mir::CastKind::IntToInt,
            };
            mir::Rvalue::Cast(kind, operand(tcx, sources, value)?, native_type(tcx, target)?)
        }
        RvalueKind::IntToFloat(value) => {
            mir::Rvalue::Cast(mir::CastKind::IntToFloat, operand(tcx, sources, value)?, tcx.types.f64)
        }
        RvalueKind::FloatToInt(value) => {
            mir::Rvalue::Cast(mir::CastKind::FloatToInt, operand(tcx, sources, value)?, tcx.types.i64)
        }
        RvalueKind::BoolToInt(value) => {
            mir::Rvalue::Cast(mir::CastKind::IntToInt, operand(tcx, sources, value)?, tcx.types.i64)
        }
        RvalueKind::Unary(op, value) => mir::Rvalue::UnaryOp(
            match op {
                UnaryOp::Not => mir::UnOp::Not,
                UnaryOp::Negate => mir::UnOp::Neg,
            },
            operand(tcx, sources, value)?,
        ),
        RvalueKind::Binary(op, left, right) => mir::Rvalue::BinaryOp(
            binary(
                op,
                matches!(destination_type, PlanType::CheckedInt | PlanType::CheckedNumeric(_)),
            ),
            Box::new((operand(tcx, sources, left)?, operand(tcx, sources, right)?)),
        ),
        RvalueKind::Model(_, elements) => {
            let ty = native_type(tcx, destination_type)?;
            let rustc_middle::ty::Adt(definition, args) = ty.kind() else {
                return Err(PlanError::Invalid {
                    function: "model".into(),
                    reason: "model construction requires an ADT destination".into(),
                });
            };
            let operands = elements
                .iter()
                .map(|value| operand(tcx, sources, value))
                .collect::<Result<Vec<_>, _>>()?;
            mir::Rvalue::Aggregate(
                Box::new(mir::AggregateKind::Adt(
                    definition.did(),
                    rustc_abi::VariantIdx::from_u32(0),
                    *args,
                    None,
                    None,
                )),
                IndexVec::from_raw(operands),
            )
        }
        RvalueKind::Array(elements) => {
            let element = match destination_type {
                PlanType::StringArray(_) => native_type(tcx, &PlanType::String)?,
                PlanType::StrArray(_) => native_type(tcx, &PlanType::StrRef)?,
                _ => {
                    return Err(PlanError::Invalid {
                        function: "array".into(),
                        reason: "array expression requires a formatting array destination".into(),
                    });
                }
            };
            let operands = elements
                .iter()
                .map(|value| operand(tcx, sources, value))
                .collect::<Result<Vec<_>, _>>()?;
            mir::Rvalue::Aggregate(
                Box::new(mir::AggregateKind::Array(element)),
                IndexVec::from_raw(operands),
            )
        }
        RvalueKind::Borrow(value) => {
            mir::Rvalue::Ref(tcx.lifetimes.re_erased, mir::BorrowKind::Shared, place(tcx, value)?)
        }
        RvalueKind::UnsizeSlice(value) => mir::Rvalue::Cast(
            mir::CastKind::PointerCoercion(
                rustc_middle::ty::adjustment::PointerCoercion::Unsize,
                mir::CoercionSource::Implicit,
            ),
            operand(tcx, sources, value)?,
            native_type(tcx, destination_type)?,
        ),
    })
}

/// Decode exact frontend-owned numeric payloads only after the Incan admission table selects their carrier.
fn numeric_constant<'tcx>(
    tcx: TyCtxt<'tcx>,
    text: &str,
    ty: &PlanType,
    span: Span,
) -> Result<mir::Operand<'tcx>, PlanError> {
    let invalid = || PlanError::Invalid {
        function: "numeric constant".into(),
        reason: format!("invalid payload {text}"),
    };
    let native = native_type(tcx, ty)?;
    let scalar = match ty {
        PlanType::ISize => Scalar::from_int(
            text.parse::<i128>().map_err(|_| invalid())?,
            tcx.data_layout.pointer_size(),
        ),
        PlanType::USize => Scalar::from_uint(
            text.parse::<u128>().map_err(|_| invalid())?,
            tcx.data_layout.pointer_size(),
        ),
        PlanType::Int => Scalar::from_i64(text.parse::<i64>().map_err(|_| invalid())?),
        PlanType::F32 => Scalar::from_u32(text.parse::<u32>().map_err(|_| invalid())?),
        PlanType::F64 => Scalar::from_u64(text.parse::<u64>().map_err(|_| invalid())?),
        PlanType::I8 => Scalar::from_i8(text.parse::<i8>().map_err(|_| invalid())?),
        PlanType::I16 => Scalar::from_i16(text.parse::<i16>().map_err(|_| invalid())?),
        PlanType::I32 => Scalar::from_i32(text.parse::<i32>().map_err(|_| invalid())?),
        PlanType::I128 => Scalar::from_i128(text.parse::<i128>().map_err(|_| invalid())?),
        PlanType::U8 => Scalar::from_u8(text.parse::<u8>().map_err(|_| invalid())?),
        PlanType::U16 => Scalar::from_u16(text.parse::<u16>().map_err(|_| invalid())?),
        PlanType::U32 => Scalar::from_u32(text.parse::<u32>().map_err(|_| invalid())?),
        PlanType::U64 => Scalar::from_u64(text.parse::<u64>().map_err(|_| invalid())?),
        PlanType::U128 => Scalar::from_u128(text.parse::<u128>().map_err(|_| invalid())?),
        _ => return Err(invalid()),
    };
    Ok(mir::Operand::const_from_scalar(tcx, native, scalar, span))
}
