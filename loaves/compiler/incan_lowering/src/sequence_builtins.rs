//! Lower sequence builtins: sorting, repetition, concatenation, truth, powers, and text splitting and joining.

use incan_mir_plan_native::{PlanType, SourceSpan};
use incan_semantics_core::body_ir::{ArgumentElement, Constant as SourceConstant, Operand as SourceOperand};

use crate::checked_values::scalar_place;
use crate::element_places::{element_place_type, is_element_place};
use crate::frame::{Builder, place_type, value_type};
use crate::list_values::value_operand;
use crate::pending::untranslated;

/// Read an argument's retained carrier before selecting a borrow, avoiding an accidental owned read.
pub fn argument_type(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    index: usize,
    span: &SourceSpan,
) -> Result<PlanType, String> {
    let Some(argument) = arguments.get(index) else {
        return Err("invalid Body IR argument type arity".into());
    };
    match argument.as_one() {
        Some(value) => source_operand_type(builder, value, span),
        None => Err("unsupported Body IR argument type spread".into()),
    }
}

/// Read a retained carrier without materializing its value.
///
/// 1. Select an exact local, field or global carrier, or preserve a constant's ordinary typed conversion.
pub fn source_operand_type(
    builder: &mut Builder,
    value: &SourceOperand,
    span: &SourceSpan,
) -> Result<PlanType, String> {
    // 1. Select the exact place carrier or the constant's ordinary typed conversion.
    match value {
        SourceOperand::Place(read) => {
            if read.place.global().is_some() {
                return Err(untranslated("static_storage::computed_static_type"));
            }
            if is_element_place(builder, &read.place)? {
                return element_place_type(builder, &read.place);
            }
            let place = scalar_place(builder, &read.place, span)?;
            Ok(place_type(&builder.locals, &place))
        }
        SourceOperand::Constant(SourceConstant::Str(_)) => Ok(PlanType::String),
        SourceOperand::Constant(SourceConstant::FrozenStr(_)) => Ok(PlanType::FrozenStr),
        SourceOperand::Constant(_) => {
            let operand = value_operand(builder, value, span)?;
            value_type(builder, &operand)
        }
    }
}
