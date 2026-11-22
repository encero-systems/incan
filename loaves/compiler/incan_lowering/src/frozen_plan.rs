//! Static frozen literals preserve the legacy carrier's lifetime and checked storage types.

use incan_mir_plan_native::{Constant, FrozenValue, Operand, PlanType, RvalueKind, SourceSpan, assign as plan_assign};
use incan_semantics_core::body_ir::Constant as RustConstant;

use crate::frame::{Builder, read_place, temporary, text_literal};
use crate::models::model_type;

/// Recognize immutable static carriers whose read-only wrapper observations own no allocation.
pub fn frozen_observation_type(ty: &PlanType) -> bool {
    matches!(
        ty,
        PlanType::StaticText
            | PlanType::StaticBytes
            | PlanType::FrozenStr
            | PlanType::FrozenBytes
            | PlanType::FrozenCollection(..)
    )
}

/// Materialize a fresh checked static wrapper by value, without imposing Copy on its element type.
pub fn frozen_collection_literal(
    builder: &mut Builder,
    value: &RustConstant,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let RustConstant::FrozenAggregate(ty, elements) = value else {
        return Err("unsupported Body IR nonstatic frozen collection".to_owned());
    };
    let storage = model_type(builder.constructors(), builder.nominals(), ty, &builder.file)?;
    let values = frozen_elements(builder, elements)?;
    let output = temporary(builder, storage.clone(), span);
    builder.emit(plan_assign(
        output.clone(),
        RvalueKind::FrozenCollection(storage, values),
        span,
    ));
    Ok(read_place(output, true, span))
}

/// Translate retained literal trees without evaluating expressions or reading syntax.
fn frozen_elements(builder: &mut Builder, elements: &[RustConstant]) -> Result<Vec<FrozenValue>, String> {
    let mut values = Vec::with_capacity(elements.len());
    for element in elements {
        values.push(match element {
            RustConstant::Int(value) => FrozenValue::Scalar(Constant::Int(*value)),
            RustConstant::Bool(value) => FrozenValue::Scalar(Constant::Bool(*value)),
            RustConstant::Str(value) | RustConstant::FrozenStr(value) => {
                FrozenValue::Scalar(Constant::Text(value.clone()))
            }
            RustConstant::Bytes(_) => FrozenValue::Bytes(frozen_byte_data(element)?),
            RustConstant::FrozenAggregate(ty, nested) => FrozenValue::Collection(
                model_type(builder.constructors(), builder.nominals(), ty, &builder.file)?,
                frozen_elements(builder, nested)?,
            ),
            _ => return Err("unsupported Body IR frozen collection element".to_owned()),
        });
    }
    Ok(values)
}

/// Wrap checked static text only; dynamically allocated text cannot enter a FrozenStr carrier.
pub fn frozen_literal(builder: &mut Builder, value: &RustConstant, span: &SourceSpan) -> Result<Operand, String> {
    let Some(text) = literal_text(value) else {
        return Err("unsupported Body IR nonstatic FrozenStr literal".to_owned());
    };
    let output = temporary(builder, PlanType::FrozenStr, span);
    builder.emit(plan_assign(
        output.clone(),
        RvalueKind::FrozenText(text_literal(&text, span)),
        span,
    ));
    Ok(read_place(output, false, span))
}

/// Read literal bytes for text observation without discarding the constant's retained storage identity.
pub fn literal_text(value: &RustConstant) -> Option<String> {
    match value {
        RustConstant::Str(text) | RustConstant::FrozenStr(text) => Some(text.clone()),
        _ => None,
    }
}

/// Retain exact static byte data for the SDK's copyable frozen wrapper, without allocating an owned byte vector.
pub fn frozen_bytes_literal(builder: &mut Builder, value: &RustConstant, span: &SourceSpan) -> Result<Operand, String> {
    let values = frozen_byte_data(value)?;
    let output = temporary(builder, PlanType::FrozenBytes, span);
    builder.emit(plan_assign(output.clone(), RvalueKind::FrozenBytes(values), span));
    Ok(read_place(output, false, span))
}

/// Read byte data at one explicitly typed foreign boundary.
fn frozen_byte_data(value: &RustConstant) -> Result<Vec<i64>, String> {
    match value {
        RustConstant::Bytes(bytes) => Ok(bytes.iter().map(|byte| i64::from(*byte)).collect()),
        _ => Err("unsupported Body IR nonstatic FrozenBytes literal".to_owned()),
    }
}
