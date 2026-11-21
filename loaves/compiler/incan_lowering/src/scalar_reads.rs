//! Borrowed Copy scalars read through their actual storage.

use incan_mir_plan_native::{Local, Place, PlanType, Projection};

use crate::numerics::numeric_carrier;

/// Read a borrowed Copy scalar through its actual storage without allocating, cloning or consuming a reference value.
pub fn scalar_storage_place(locals: &[Local], reference_values: &[i64], source: Place) -> Result<Place, String> {
    if source.projection != Projection::Whole || reference_values.contains(&source.local) {
        return Ok(source);
    }
    if let PlanType::ScalarRef(elements) | PlanType::ScalarMutRef(elements) = &locals[source.local as usize].ty {
        if elements.len() != 1 {
            return Err("invalid Body IR scalar reference storage arity".into());
        }
        let referent = &elements[0];
        if numeric_carrier(referent)
            || matches!(
                referent,
                PlanType::Bool
                    | PlanType::Unit
                    | PlanType::Decimal
                    | PlanType::FrozenStr
                    | PlanType::FrozenBytes
                    | PlanType::StaticText
                    | PlanType::StaticBytes
            )
        {
            return Ok(Place {
                local: source.local,
                projection: Projection::Deref(referent.clone()),
                span: source.span,
            });
        }
    }
    Ok(source)
}
