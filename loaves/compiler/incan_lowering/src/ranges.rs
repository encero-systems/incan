//! Integer range values in source bindings and temporaries.

use incan_mir_plan_native::{FieldProjection, range_field_types};
use incan_semantics_core::body_ir::{AggregateKind, LocalOrigin, PlaceElem};
use incan_semantics_core::{IncanPrimitiveType, IncanType};

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
