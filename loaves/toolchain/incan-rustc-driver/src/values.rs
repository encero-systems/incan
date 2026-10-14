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

/// Map a prevalidated local, checked component, or nominal receiver projection to rustc's place representation.
pub fn place<'tcx>(tcx: TyCtxt<'tcx>, value: &Place) -> Result<mir::Place<'tcx>, PlanError> {
    let local = mir::Local::from_usize(index(value.local)?);
    let place = mir::Place::from(local);
    Ok(match &value.projection {
        Projection::Whole => place,
        Projection::VariantField(variant, slot, ty) => {
            let downcast = place.project_deeper(
                &[mir::ProjectionElem::Downcast(
                    None,
                    rustc_abi::VariantIdx::from_usize(index(*variant)?),
                )],
                tcx,
            );
            tcx.mk_place_field(downcast, FieldIdx::from_usize(index(*slot)?), native_type(tcx, ty)?)
        }
        Projection::Field(slot, ty) => {
            tcx.mk_place_field(place, FieldIdx::from_usize(index(*slot)?), native_type(tcx, ty)?)
        }
        Projection::Deref(_) => tcx.mk_place_deref(place),
        Projection::DerefField(slot, ty) => tcx.mk_place_field(
            tcx.mk_place_deref(place),
            FieldIdx::from_usize(index(*slot)?),
            native_type(tcx, ty)?,
        ),
        Projection::Fields(fields) | Projection::DerefFields(fields) => {
            let mut projected = if matches!(value.projection, Projection::DerefFields(_)) {
                tcx.mk_place_deref(place)
            } else {
                place
            };
            for field in fields {
                if field.variant >= 0 {
                    projected = projected.project_deeper(
                        &[mir::ProjectionElem::Downcast(
                            None,
                            rustc_abi::VariantIdx::from_usize(index(field.variant)?),
                        )],
                        tcx,
                    );
                }
                projected = tcx.mk_place_field(
                    projected,
                    FieldIdx::from_usize(index(field.slot)?),
                    native_type(tcx, &field.ty)?,
                );
            }
            projected
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

/// Construct the dependency-owned frozen wrapper only after verifying its single static string field.
///
/// The literal allocation is static; its operand takes the field's exact lifetime rather than a body borrow.
fn frozen_text<'tcx>(tcx: TyCtxt<'tcx>, sources: &Sources<'_>, text: &Operand) -> Result<mir::Rvalue<'tcx>, PlanError> {
    // ---- Carrier: the dependency-owned static text layout ----
    let ty = native_type(tcx, &PlanType::FrozenStr)?;
    let rustc_middle::ty::Adt(definition, args) = ty.kind() else {
        return Err(PlanError::Invalid {
            function: "FrozenStr".into(),
            reason: "frozen carrier is not an ADT".into(),
        });
    };
    if !definition.is_struct() || definition.non_enum_variant().fields.len() != 1 {
        return Err(PlanError::Invalid {
            function: "FrozenStr".into(),
            reason: "frozen carrier must have one field".into(),
        });
    }
    // The concrete frozen carrier has no parameters or associated-type projections to normalize.
    let field = definition.non_enum_variant().fields[FieldIdx::from_usize(0)]
        .ty(tcx, *args)
        .skip_normalization();
    if !matches!(field.kind(), rustc_middle::ty::Ref(region, pointee, rustc_hir::Mutability::Not)
        if region.is_static() && pointee.is_str())
    {
        return Err(PlanError::Invalid {
            function: "FrozenStr".into(),
            reason: "frozen carrier field is not static shared text".into(),
        });
    }

    // ---- Literal: the existing allocation with the carrier field's lifetime ----
    let mir::Operand::Constant(mut literal) = operand(tcx, sources, text)? else {
        return Err(PlanError::Invalid {
            function: "FrozenStr".into(),
            reason: "frozen text is not a constant".into(),
        });
    };
    let mir::Const::Val(value, _) = literal.const_ else {
        return Err(PlanError::Invalid {
            function: "FrozenStr".into(),
            reason: "frozen text has no literal allocation".into(),
        });
    };
    literal.const_ = mir::Const::Val(value, field);

    // ---- Construction: the admitted single-field carrier ----
    Ok(mir::Rvalue::Aggregate(
        Box::new(mir::AggregateKind::Adt(
            definition.did(),
            rustc_abi::VariantIdx::from_usize(0),
            *args,
            None,
            None,
        )),
        IndexVec::from_raw(vec![mir::Operand::Constant(literal)]),
    ))
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
        BinaryOp::BitAnd => mir::BinOp::BitAnd,
        BinaryOp::BitOr => mir::BinOp::BitOr,
        BinaryOp::BitXor => mir::BinOp::BitXor,
        BinaryOp::ShiftLeft => mir::BinOp::Shl,
        BinaryOp::ShiftRight => mir::BinOp::Shr,
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
        RvalueKind::UnitFunction(name) => {
            let def = tcx
                .hir_crate_items(())
                .free_items()
                .map(|item| item.owner_id.to_def_id())
                .find(|def| tcx.opt_item_name(*def).is_some_and(|symbol| symbol.as_str() == name))
                .ok_or_else(|| PlanError::UnknownCallee(name.clone()))?;
            mir::Rvalue::Cast(
                mir::CastKind::PointerCoercion(
                    rustc_middle::ty::adjustment::PointerCoercion::ReifyFnPointer(rustc_hir::Safety::Safe),
                    mir::CoercionSource::Implicit,
                ),
                mir::Operand::function_handle(tcx, def, [], rustc_span::DUMMY_SP),
                native_type(tcx, &PlanType::UnitFunction)?,
            )
        }
        RvalueKind::Discriminant(value) => mir::Rvalue::Discriminant(place(tcx, value)?),
        RvalueKind::FrozenText(text) => frozen_text(tcx, sources, text)?,
        RvalueKind::TagToInt(value) => {
            mir::Rvalue::Cast(mir::CastKind::IntToInt, operand(tcx, sources, value)?, tcx.types.i64)
        }
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
        RvalueKind::FunctionItem(callee) => mir::Rvalue::Use(
            mir::Operand::function_handle(
                tcx,
                crate::callees::resolve(tcx, callee)?,
                [],
                sources.span(&callee.span)?,
            ),
            mir::WithRetag::Yes,
        ),
        RvalueKind::ClosureObject(value) => mir::Rvalue::Ref(
            tcx.lifetimes.re_erased,
            mir::BorrowKind::Shared,
            place(tcx, value)?.project_deeper(&[mir::ProjectionElem::Deref], tcx),
        ),
        RvalueKind::ReifyFunction(value) => mir::Rvalue::Cast(
            mir::CastKind::PointerCoercion(
                rustc_middle::ty::adjustment::PointerCoercion::ReifyFnPointer(rustc_hir::Safety::Safe),
                mir::CoercionSource::Implicit,
            ),
            operand(tcx, sources, value)?,
            native_type(tcx, destination_type)?,
        ),
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
                UnaryOp::Invert => mir::UnOp::Not,
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
        RvalueKind::Tuple(elements) => mir::Rvalue::Aggregate(
            Box::new(mir::AggregateKind::Tuple),
            IndexVec::from_raw(
                elements
                    .iter()
                    .map(|element| operand(tcx, sources, element))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        ),

        RvalueKind::Model(_, elements) | RvalueKind::Enum(_, _, elements) => {
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
                    rustc_abi::VariantIdx::from_usize(match value {
                        RvalueKind::Enum(_, variant, _) => index(*variant)?,
                        _ => 0,
                    }),
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
        RvalueKind::MutBorrow(value) => mir::Rvalue::Ref(
            tcx.lifetimes.re_erased,
            mir::BorrowKind::Mut {
                kind: mir::MutBorrowKind::Default,
            },
            place(tcx, value)?,
        ),
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
