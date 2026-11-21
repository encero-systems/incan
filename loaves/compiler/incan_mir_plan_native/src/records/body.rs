//! Round-trip a plan function's types, places, operands, operations and statements for keyed plan products.
//!
//! Every model and enum position, whether in a type, a projection, a construction or a variant test, crosses one
//! [`NominalRemap`]: capture replaces it by a product-local ordinal, and binding replaces the ordinal by the position
//! the current unit resolved for the declaration the product names.

use crate::body::{
    BinaryOp, Callee, CalleeKind, Constant, FieldProjection, FrozenValue, Operand, OperandKind, Place, Projection,
    Rvalue, RvalueKind, Statement, StatementKind, UnaryOp,
};
use crate::nominal::{NominalKind, NominalRemap, remap_type};
use crate::records::wire_body_records::{
    StoredConstant, StoredFieldStep, StoredFrozenValue, StoredOperand, StoredPlace, StoredProjection, StoredRead,
    StoredRvalue, StoredSpan, StoredStatement, StoredStatementKind, StoredType,
};
use crate::types::{PlanType, SourceSpan, source_span};

/// Retain file text and both one-based coordinates without exposing an allocation identity.
pub(crate) fn store_span(span: &SourceSpan) -> StoredSpan {
    StoredSpan {
        file: span.file.as_str().to_owned(),
        line: span.line,
        column: span.column,
    }
}

/// Use the plan owner's shared-file constructor for restored diagnostic locations.
pub(crate) fn restore_span(span: &StoredSpan) -> SourceSpan {
    source_span(&span.file, span.line, span.column)
}

/// Retain a complete layout type.
///
/// Scalar and text carriers keep their Incan-facing spelling; every other carrier crosses the layout type codec with
/// each model and enum position translated to its product-local ordinal.
pub(crate) fn store_type(ty: &PlanType, remap: &mut NominalRemap) -> Result<StoredType, String> {
    Ok(match ty {
        PlanType::Int => StoredType::Int,
        PlanType::Float => StoredType::Float,
        PlanType::Bool => StoredType::Bool,
        PlanType::Unit => StoredType::Unit,
        PlanType::CheckedInt => StoredType::CheckedInt,
        PlanType::Never => StoredType::Never,
        PlanType::String => StoredType::Text,
        PlanType::StrRef => StoredType::LiteralTextView,
        PlanType::StringRef => StoredType::TextView,
        PlanType::StringArray(length) => StoredType::TextValues(*length),
        PlanType::StrArray(length) => StoredType::LiteralTextValues(*length),
        PlanType::StringArrayRef(length) => StoredType::TextValuesView(*length),
        PlanType::StrArrayRef(length) => StoredType::LiteralTextValuesView(*length),
        PlanType::StringSlice => StoredType::TextSequence,
        PlanType::StrSlice => StoredType::LiteralTextSequence,
        // A translated layout type holds no source-trait identity: the translation refuses one first.
        _ => StoredType::Layout(remap_type(ty, remap)?),
    })
}

/// Restore the exact carrier, binding each product-local ordinal to its resolved unit position.
pub(crate) fn restore_type(stored: &StoredType, remap: &mut NominalRemap) -> Result<PlanType, String> {
    Ok(match stored {
        StoredType::Int => PlanType::Int,
        StoredType::Float => PlanType::Float,
        StoredType::Bool => PlanType::Bool,
        StoredType::Unit => PlanType::Unit,
        StoredType::CheckedInt => PlanType::CheckedInt,
        StoredType::Never => PlanType::Never,
        StoredType::Text => PlanType::String,
        StoredType::LiteralTextView => PlanType::StrRef,
        StoredType::TextView => PlanType::StringRef,
        StoredType::TextValues(length) => PlanType::StringArray(*length),
        StoredType::LiteralTextValues(length) => PlanType::StrArray(*length),
        StoredType::TextValuesView(length) => PlanType::StringArrayRef(*length),
        StoredType::LiteralTextValuesView(length) => PlanType::StrArrayRef(*length),
        StoredType::TextSequence => PlanType::StringSlice,
        StoredType::LiteralTextSequence => PlanType::StrSlice,
        StoredType::Layout(layout) => remap_type(layout, remap)?,
    })
}

/// Retain an ordered type list through one translation.
pub(crate) fn store_types(types: &[PlanType], remap: &mut NominalRemap) -> Result<Vec<StoredType>, String> {
    types.iter().map(|ty| store_type(ty, remap)).collect()
}

/// Restore an ordered type list through one translation.
pub(crate) fn restore_types(stored: &[StoredType], remap: &mut NominalRemap) -> Result<Vec<PlanType>, String> {
    stored.iter().map(|value| restore_type(value, remap)).collect()
}

/// Preserve every projection; its types and field paths cross the same nominal translation.
pub(crate) fn store_place(place: &Place, remap: &mut NominalRemap) -> Result<StoredPlace, String> {
    Ok(StoredPlace {
        local: place.local,
        projection: store_projection(&place.projection, remap)?,
        span: store_span(&place.span),
    })
}

/// Restore the exact projection rather than infer it from a carrier.
pub(crate) fn restore_place(place: &StoredPlace, remap: &mut NominalRemap) -> Result<Place, String> {
    Ok(Place {
        local: place.local,
        projection: restore_projection(&place.projection, remap)?,
        span: restore_span(&place.span),
    })
}

/// Retain one projection; slots and variants are positions within the named declaration's checked layout.
fn store_projection(projection: &Projection, remap: &mut NominalRemap) -> Result<StoredProjection, String> {
    Ok(match projection {
        Projection::Whole => StoredProjection::Whole,
        Projection::Value => StoredProjection::Value,
        Projection::Overflow => StoredProjection::Overflow,
        Projection::NumericValue(ty) => StoredProjection::NumericValue(store_type(ty, remap)?),
        Projection::VariantField(variant, slot, ty) => {
            StoredProjection::VariantField(*variant, *slot, store_type(ty, remap)?)
        }
        Projection::Field(slot, ty) => StoredProjection::Field(*slot, store_type(ty, remap)?),
        Projection::Deref(ty) => StoredProjection::Deref(store_type(ty, remap)?),
        Projection::DerefField(slot, ty) => StoredProjection::DerefField(*slot, store_type(ty, remap)?),
        Projection::Fields(steps) => StoredProjection::Fields(store_steps(steps, remap)?),
        Projection::DerefFields(steps) => StoredProjection::DerefFields(store_steps(steps, remap)?),
    })
}

/// Restore one projection at the current positions of the declarations it reads.
fn restore_projection(projection: &StoredProjection, remap: &mut NominalRemap) -> Result<Projection, String> {
    Ok(match projection {
        StoredProjection::Whole => Projection::Whole,
        StoredProjection::Value => Projection::Value,
        StoredProjection::Overflow => Projection::Overflow,
        StoredProjection::NumericValue(ty) => Projection::NumericValue(restore_type(ty, remap)?),
        StoredProjection::VariantField(variant, slot, ty) => {
            Projection::VariantField(*variant, *slot, restore_type(ty, remap)?)
        }
        StoredProjection::Field(slot, ty) => Projection::Field(*slot, restore_type(ty, remap)?),
        StoredProjection::Deref(ty) => Projection::Deref(restore_type(ty, remap)?),
        StoredProjection::DerefField(slot, ty) => Projection::DerefField(*slot, restore_type(ty, remap)?),
        StoredProjection::Fields(steps) => Projection::Fields(restore_steps(steps, remap)?),
        StoredProjection::DerefFields(steps) => Projection::DerefFields(restore_steps(steps, remap)?),
    })
}

/// Retain a field path in order.
fn store_steps(steps: &[FieldProjection], remap: &mut NominalRemap) -> Result<Vec<StoredFieldStep>, String> {
    steps
        .iter()
        .map(|step| {
            Ok(StoredFieldStep {
                slot: step.slot,
                ty: store_type(&step.ty, remap)?,
                variant: step.variant,
                dereferences: step.dereferences,
            })
        })
        .collect()
}

/// Restore a field path in order.
fn restore_steps(stored: &[StoredFieldStep], remap: &mut NominalRemap) -> Result<Vec<FieldProjection>, String> {
    stored
        .iter()
        .map(|step| {
            Ok(FieldProjection {
                slot: step.slot,
                ty: restore_type(&step.ty, remap)?,
                variant: step.variant,
                dereferences: step.dereferences,
            })
        })
        .collect()
}

/// Preserve ordinary constants without narrowing integer digits, floating-point values or sized literals.
fn store_constant(value: &Constant, remap: &mut NominalRemap) -> Result<StoredConstant, String> {
    Ok(match value {
        Constant::Int(value) => StoredConstant::Int(*value),
        Constant::Float(value) => StoredConstant::Float(*value),
        Constant::Bool(value) => StoredConstant::Bool(*value),
        Constant::Unit => StoredConstant::Unit,
        Constant::Text(value) => StoredConstant::Text(value.clone()),
        Constant::Numeric(digits, ty) => StoredConstant::Numeric(digits.clone(), store_type(ty, remap)?),
        Constant::TypeToken(types) => StoredConstant::TypeToken(store_types(types, remap)?),
        Constant::RustItem(..) => return Err("scalar function products refuse this native constant".to_owned()),
    })
}

/// Retain an effect-free static collection's values in source order, each scalar as an ordinary constant.
fn store_frozen_values(values: &[FrozenValue], remap: &mut NominalRemap) -> Result<Vec<StoredFrozenValue>, String> {
    values
        .iter()
        .map(|value| {
            Ok(match value {
                FrozenValue::Scalar(constant) => StoredFrozenValue::Scalar(store_constant(constant, remap)?),
                FrozenValue::Bytes(bytes) => StoredFrozenValue::Bytes(bytes.clone()),
                FrozenValue::Collection(ty, nested) => {
                    StoredFrozenValue::Collection(store_type(ty, remap)?, store_frozen_values(nested, remap)?)
                }
            })
        })
        .collect()
}

/// Recover a static collection's exact values and nesting.
fn restore_frozen_values(values: &[StoredFrozenValue], remap: &mut NominalRemap) -> Result<Vec<FrozenValue>, String> {
    values
        .iter()
        .map(|value| {
            Ok(match value {
                StoredFrozenValue::Scalar(constant) => FrozenValue::Scalar(restore_constant(constant, remap)?),
                StoredFrozenValue::Bytes(bytes) => FrozenValue::Bytes(bytes.clone()),
                StoredFrozenValue::Collection(ty, nested) => {
                    FrozenValue::Collection(restore_type(ty, remap)?, restore_frozen_values(nested, remap)?)
                }
            })
        })
        .collect()
}

/// Recover the producer's exact constant variant.
fn restore_constant(value: &StoredConstant, remap: &mut NominalRemap) -> Result<Constant, String> {
    Ok(match value {
        StoredConstant::Int(value) => Constant::Int(*value),
        StoredConstant::Float(value) => Constant::Float(*value),
        StoredConstant::Bool(value) => Constant::Bool(*value),
        StoredConstant::Unit => Constant::Unit,
        StoredConstant::Text(value) => Constant::Text(value.clone()),
        StoredConstant::Numeric(digits, ty) => Constant::Numeric(digits.clone(), restore_type(ty, remap)?),
        StoredConstant::TypeToken(types) => Constant::TypeToken(restore_types(types, remap)?),
    })
}

/// Retain the operand's ownership operation independently of its source position.
pub(crate) fn store_operand(operand: &Operand, remap: &mut NominalRemap) -> Result<StoredOperand, String> {
    let span = store_span(&operand.span);
    let kind = match &operand.kind {
        OperandKind::Copy(place) => StoredRead::Copy(store_place(place, remap)?),
        OperandKind::Move(place) => StoredRead::Move(store_place(place, remap)?),
        OperandKind::Literal(value) => StoredRead::Literal(store_constant(value, remap)?),
    };
    Ok(StoredOperand { kind, span })
}

/// Restore explicit copy, move and literal reads without recomputing ownership.
pub(crate) fn restore_operand(operand: &StoredOperand, remap: &mut NominalRemap) -> Result<Operand, String> {
    let span = restore_span(&operand.span);
    let kind = match &operand.kind {
        StoredRead::Copy(place) => OperandKind::Copy(restore_place(place, remap)?),
        StoredRead::Move(place) => OperandKind::Move(restore_place(place, remap)?),
        StoredRead::Literal(value) => OperandKind::Literal(restore_constant(value, remap)?),
    };
    Ok(Operand { kind, span })
}

/// Retain ordered operands through one translation.
pub(crate) fn store_operands(operands: &[Operand], remap: &mut NominalRemap) -> Result<Vec<StoredOperand>, String> {
    operands.iter().map(|operand| store_operand(operand, remap)).collect()
}

/// Restore ordered operands through one translation.
pub(crate) fn restore_operands(stored: &[StoredOperand], remap: &mut NominalRemap) -> Result<Vec<Operand>, String> {
    stored.iter().map(|operand| restore_operand(operand, remap)).collect()
}

/// Encode the native operation by an explicit schema tag, independently of its display spelling.
pub(crate) fn store_binary(operation: &BinaryOp) -> i64 {
    match operation {
        BinaryOp::Add => 0,
        BinaryOp::Subtract => 1,
        BinaryOp::Multiply => 2,
        BinaryOp::Divide => 3,
        BinaryOp::Equal => 4,
        BinaryOp::NotEqual => 5,
        BinaryOp::Less => 6,
        BinaryOp::LessEqual => 7,
        BinaryOp::Greater => 8,
        BinaryOp::GreaterEqual => 9,
        BinaryOp::BitAnd => 10,
        BinaryOp::BitOr => 11,
        BinaryOp::BitXor => 12,
        BinaryOp::ShiftLeft => 13,
        BinaryOp::ShiftRight => 14,
    }
}

/// Refuse unknown operation tags rather than map them to an approximate arithmetic operation.
pub(crate) fn restore_binary(tag: i64) -> Result<BinaryOp, String> {
    Ok(match tag {
        0 => BinaryOp::Add,
        1 => BinaryOp::Subtract,
        2 => BinaryOp::Multiply,
        3 => BinaryOp::Divide,
        4 => BinaryOp::Equal,
        5 => BinaryOp::NotEqual,
        6 => BinaryOp::Less,
        7 => BinaryOp::LessEqual,
        8 => BinaryOp::Greater,
        9 => BinaryOp::GreaterEqual,
        10 => BinaryOp::BitAnd,
        11 => BinaryOp::BitOr,
        12 => BinaryOp::BitXor,
        13 => BinaryOp::ShiftLeft,
        14 => BinaryOp::ShiftRight,
        _ => return Err("unknown scalar binary operation tag".to_owned()),
    })
}

/// Encode each ordinary native unary operation explicitly.
fn store_unary(operation: &UnaryOp) -> i64 {
    match operation {
        UnaryOp::Not => 0,
        UnaryOp::Negate => 1,
        UnaryOp::Invert => 2,
    }
}

/// Decode only the unary operations defined by the private schema.
fn restore_unary(tag: i64) -> Result<UnaryOp, String> {
    Ok(match tag {
        0 => UnaryOp::Not,
        1 => UnaryOp::Negate,
        2 => UnaryOp::Invert,
        _ => return Err("unknown scalar unary operation tag".to_owned()),
    })
}

/// Capture self-contained operations; nominal constructions name their owner by product-local ordinal.
fn store_rvalue(value: &RvalueKind, remap: &mut NominalRemap) -> Result<StoredRvalue, String> {
    Ok(match value {
        RvalueKind::Use(operand) => StoredRvalue::Use(store_operand(operand, remap)?),
        RvalueKind::Binary(op, left, right) => StoredRvalue::Binary(
            store_binary(op),
            store_operand(left, remap)?,
            store_operand(right, remap)?,
        ),
        RvalueKind::Unary(op, operand) => StoredRvalue::Unary(store_unary(op), store_operand(operand, remap)?),
        RvalueKind::IntToFloat(operand) => StoredRvalue::IntToFloat(store_operand(operand, remap)?),
        RvalueKind::FloatToInt(operand) => StoredRvalue::FloatToInt(store_operand(operand, remap)?),
        RvalueKind::BoolToInt(operand) => StoredRvalue::BoolToInt(store_operand(operand, remap)?),
        RvalueKind::Array(operands) => StoredRvalue::Array(store_operands(operands, remap)?),
        RvalueKind::Borrow(place) => StoredRvalue::Borrow(store_place(place, remap)?),
        RvalueKind::UnsizeSlice(operand) => StoredRvalue::UnsizeSlice(store_operand(operand, remap)?),
        _ => return store_nominal_rvalue(value, remap),
    })
}

/// Capture constructions, variant tests, casts and aliases; owner indexes become product-local ordinals.
fn store_nominal_rvalue(value: &RvalueKind, remap: &mut NominalRemap) -> Result<StoredRvalue, String> {
    Ok(match value {
        RvalueKind::NativeAlias(operand, source, target) => StoredRvalue::NativeAlias(
            store_operand(operand, remap)?,
            store_type(source, remap)?,
            store_type(target, remap)?,
        ),
        RvalueKind::FrozenText(operand) => StoredRvalue::FrozenText(store_operand(operand, remap)?),
        RvalueKind::FrozenBytes(bytes) => StoredRvalue::FrozenBytes(bytes.clone()),
        RvalueKind::FrozenCollection(ty, values) => {
            StoredRvalue::FrozenCollection(store_type(ty, remap)?, store_frozen_values(values, remap)?)
        }
        RvalueKind::NumericCast(operand, source, target) => StoredRvalue::NumericCast(
            store_operand(operand, remap)?,
            store_type(source, remap)?,
            store_type(target, remap)?,
        ),
        RvalueKind::Enum(owner, variant, operands) => {
            let slot = remap.slot(NominalKind::Enum, *owner, "")?;
            StoredRvalue::Enum(slot.index, *variant, store_operands(operands, remap)?)
        }
        RvalueKind::Discriminant(place) => StoredRvalue::Discriminant(store_place(place, remap)?),
        RvalueKind::TagToInt(operand) => StoredRvalue::TagToInt(store_operand(operand, remap)?),
        RvalueKind::Model(owner, operands) => {
            let slot = remap.slot(NominalKind::Model, *owner, "")?;
            StoredRvalue::Model(slot.index, store_operands(operands, remap)?)
        }
        RvalueKind::Tuple(operands) => StoredRvalue::Tuple(store_operands(operands, remap)?),
        RvalueKind::Range(operands) => StoredRvalue::Range(store_operands(operands, remap)?),
        RvalueKind::MutBorrow(place) => StoredRvalue::MutBorrow(store_place(place, remap)?),
        RvalueKind::IsVariant(place, variant) => StoredRvalue::IsVariant(store_place(place, remap)?, *variant),
        RvalueKind::ReifyFunction(operand) => StoredRvalue::ReifyFunction(store_operand(operand, remap)?),
        RvalueKind::FunctionAddress(operand) => StoredRvalue::FunctionAddress(store_operand(operand, remap)?),
        RvalueKind::ClosureObject(place) => StoredRvalue::ClosureObject(store_place(place, remap)?),
        RvalueKind::FunctionItem(callee) => match &callee.kind {
            CalleeKind::Planned(name) => StoredRvalue::PlannedFunctionItem(name.clone(), store_span(&callee.span)),
            _ => return Err("scalar function product requires a planned function item".to_owned()),
        },
        _ => return Err("scalar function products refuse this native operation".to_owned()),
    })
}

/// Recover the exact native operation; the checker and lowering's decisions are never repeated.
fn restore_rvalue(value: &StoredRvalue, remap: &mut NominalRemap) -> Result<RvalueKind, String> {
    Ok(match value {
        StoredRvalue::Use(operand) => RvalueKind::Use(restore_operand(operand, remap)?),
        StoredRvalue::Binary(op, left, right) => RvalueKind::Binary(
            restore_binary(*op)?,
            restore_operand(left, remap)?,
            restore_operand(right, remap)?,
        ),
        StoredRvalue::Unary(op, operand) => RvalueKind::Unary(restore_unary(*op)?, restore_operand(operand, remap)?),
        StoredRvalue::IntToFloat(operand) => RvalueKind::IntToFloat(restore_operand(operand, remap)?),
        StoredRvalue::FloatToInt(operand) => RvalueKind::FloatToInt(restore_operand(operand, remap)?),
        StoredRvalue::BoolToInt(operand) => RvalueKind::BoolToInt(restore_operand(operand, remap)?),
        StoredRvalue::Array(operands) => RvalueKind::Array(restore_operands(operands, remap)?),
        StoredRvalue::Borrow(place) => RvalueKind::Borrow(restore_place(place, remap)?),
        StoredRvalue::UnsizeSlice(operand) => RvalueKind::UnsizeSlice(restore_operand(operand, remap)?),
        _ => return restore_nominal_rvalue(value, remap),
    })
}

/// Restore constructions and variant tests at the current positions of their owners.
fn restore_nominal_rvalue(value: &StoredRvalue, remap: &mut NominalRemap) -> Result<RvalueKind, String> {
    Ok(match value {
        StoredRvalue::NativeAlias(operand, source, target) => RvalueKind::NativeAlias(
            restore_operand(operand, remap)?,
            restore_type(source, remap)?,
            restore_type(target, remap)?,
        ),
        StoredRvalue::FrozenText(operand) => RvalueKind::FrozenText(restore_operand(operand, remap)?),
        StoredRvalue::FrozenBytes(bytes) => RvalueKind::FrozenBytes(bytes.clone()),
        StoredRvalue::FrozenCollection(ty, values) => {
            RvalueKind::FrozenCollection(restore_type(ty, remap)?, restore_frozen_values(values, remap)?)
        }
        StoredRvalue::NumericCast(operand, source, target) => RvalueKind::NumericCast(
            restore_operand(operand, remap)?,
            restore_type(source, remap)?,
            restore_type(target, remap)?,
        ),
        StoredRvalue::Enum(ordinal, variant, operands) => {
            let slot = remap.slot(NominalKind::Enum, *ordinal, "")?;
            RvalueKind::Enum(slot.index, *variant, restore_operands(operands, remap)?)
        }
        StoredRvalue::Discriminant(place) => RvalueKind::Discriminant(restore_place(place, remap)?),
        StoredRvalue::TagToInt(operand) => RvalueKind::TagToInt(restore_operand(operand, remap)?),
        StoredRvalue::Model(ordinal, operands) => {
            let slot = remap.slot(NominalKind::Model, *ordinal, "")?;
            RvalueKind::Model(slot.index, restore_operands(operands, remap)?)
        }
        StoredRvalue::Tuple(operands) => RvalueKind::Tuple(restore_operands(operands, remap)?),
        StoredRvalue::Range(operands) => RvalueKind::Range(restore_operands(operands, remap)?),
        StoredRvalue::MutBorrow(place) => RvalueKind::MutBorrow(restore_place(place, remap)?),
        StoredRvalue::IsVariant(place, variant) => RvalueKind::IsVariant(restore_place(place, remap)?, *variant),
        StoredRvalue::ReifyFunction(operand) => RvalueKind::ReifyFunction(restore_operand(operand, remap)?),
        StoredRvalue::FunctionAddress(operand) => RvalueKind::FunctionAddress(restore_operand(operand, remap)?),
        StoredRvalue::ClosureObject(place) => RvalueKind::ClosureObject(restore_place(place, remap)?),
        StoredRvalue::PlannedFunctionItem(name, span) => RvalueKind::FunctionItem(Callee {
            kind: CalleeKind::Planned(name.clone()),
            span: restore_span(span),
        }),
        _ => return Err("unknown stored native operation".to_owned()),
    })
}

/// Retain assignments and explicit storage boundaries without dropping nested source spans.
pub(crate) fn store_statement(statement: &Statement, remap: &mut NominalRemap) -> Result<StoredStatement, String> {
    let span = store_span(&statement.span);
    let kind = match &statement.kind {
        StatementKind::Assign(place, value) => {
            let destination = store_place(place, remap)?;
            StoredStatementKind::Assign(destination, store_rvalue(&value.kind, remap)?, store_span(&value.span))
        }
        StatementKind::StorageLive(local) => StoredStatementKind::Live(*local),
        StatementKind::StorageDead(local) => StoredStatementKind::Dead(*local),
    };
    Ok(StoredStatement { kind, span })
}

/// Restore every statement field, including the value's independent source location.
pub(crate) fn restore_statement(statement: &StoredStatement, remap: &mut NominalRemap) -> Result<Statement, String> {
    let span = restore_span(&statement.span);
    let kind = match &statement.kind {
        StoredStatementKind::Assign(place, value, value_span) => {
            let destination = restore_place(place, remap)?;
            StatementKind::Assign(
                destination,
                Rvalue {
                    kind: restore_rvalue(value, remap)?,
                    span: restore_span(value_span),
                },
            )
        }
        StoredStatementKind::Live(local) => StatementKind::StorageLive(*local),
        StoredStatementKind::Dead(local) => StatementKind::StorageDead(*local),
    };
    Ok(Statement { kind, span })
}
