//! Integer range values in source bindings and temporaries.

use incan_mir_plan_native::{Constant, FieldProjection, Operand, PlanType, RvalueKind, SourceSpan, range_field_types};
use incan_semantics_core::body_ir::{AggregateKind, LocalOrigin, PlaceElem};
use incan_semantics_core::{IncanPrimitiveType, IncanType};

use crate::frame::literal;

/// Admit an integer range only in a source binding or temporary, never at a signature or capture boundary.
pub fn range_value_local(ty: &IncanType, origin: &LocalOrigin) -> bool {
    if !matches!(origin, LocalOrigin::UserBinding | LocalOrigin::Temporary) {
        return false;
    }
    integer_range(ty)
}

/// The checker's `Range[int]` value type.
fn integer_range(ty: &IncanType) -> bool {
    match ty {
        IncanType::Generic { base, args } if base == "Range" && args.len() == 1 => {
            matches!(args[0], IncanType::Primitive(IncanPrimitiveType::Int))
        }
        _ => false,
    }
}

/// Project one structural range field through Body IR's declared field order.
pub fn range_field(projection: &PlaceElem) -> Result<FieldProjection, String> {
    let PlaceElem::Field {
        name,
        canonical,
        structural,
    } = projection
    else {
        return Err("unsupported Body IR range index or slice projection".into());
    };
    if !structural {
        return Err("invalid Body IR nonstructural range field".into());
    }
    if canonical.is_some() {
        return Err("invalid Body IR nominal field on a structural range".into());
    }
    for (slot, field) in AggregateKind::RANGE_FIELDS.iter().enumerate() {
        if field == name {
            return Ok(FieldProjection::new(slot as i64, range_field_types()[slot].clone()));
        }
    }
    Err("invalid Body IR range field".into())
}

/// Complete a builtin `range` call's start, end and step, defaulting a missing start to 0 and step to 1.
pub fn builtin_range_bounds(arguments: Vec<Operand>, span: &SourceSpan) -> Result<Vec<Operand>, String> {
    let zero = || literal(Constant::Int(0), span);
    let one = || literal(Constant::Int(1), span);
    let mut bounds = arguments.into_iter();
    match (bounds.next(), bounds.next(), bounds.next(), bounds.next()) {
        (Some(end), None, None, None) => Ok(vec![zero(), end, one()]),
        (Some(start), Some(end), None, None) => Ok(vec![start, end, one()]),
        (Some(start), Some(end), Some(step), None) => Ok(vec![start, end, step]),
        _ => Err("unsupported Body IR builtin range arity".into()),
    }
}

/// Build a source-local range value from its four checked scalar fields in declared order.
pub fn range_aggregate(expected: &PlanType, fields: Vec<Operand>, types: &[PlanType]) -> Result<RvalueKind, String> {
    if *expected != PlanType::RangeValue {
        return Err("unsupported Body IR range value outside a source-local binding".into());
    }
    if types != range_field_types().as_slice() {
        return Err("unsupported Body IR range field types".into());
    }
    Ok(RvalueKind::Range(fields))
}
