//! Calls through callable values, closures held by carriers, and callable objects.

use incan_mir_plan_native::{Carrier, Operand, PlanType, SourceSpan, carrier_position};

use crate::closures::callable_copy_type;
use crate::frame::{Builder, value_type};
use crate::pending::untranslated;

/// Fill an environment-owning payload selected by the retained union declaration, preserving captured values.
pub fn carrier_callable_operand(
    builder: &mut Builder,
    value: Operand,
    target: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let _ = span;
    let PlanType::Enum(owner, _) = target else {
        return Ok(value);
    };
    if *owner < builder.enums().len() as i64 {
        return Ok(value);
    }
    let declaration = &builder.intrinsics[carrier_position(*owner) as usize];
    let source = value_type(builder, &value)?;
    if declaration.carrier != Carrier::Declared || !callable_copy_type(&source) {
        return Ok(value);
    }
    let signature = match &source {
        PlanType::FunctionPointer(signature) | PlanType::FunctionItem(_, signature) => signature.clone(),
        _ => return Ok(value),
    };
    for variant in &declaration.variants {
        if variant.fields.len() == 1 && variant.fields[0] == PlanType::Closure(signature.clone()) {
            return Err(untranslated("callable_calls::owned_callable_value"));
        }
    }
    Ok(value)
}
