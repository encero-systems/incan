//! Defaulted parameter slots: the representations a call slot may own or borrow.

use incan_mir_plan_native::PlanType;

use crate::collections::is_collection;
use crate::lists::is_list;
use crate::numerics::numeric_carrier;

/// Admit default representations a call slot owns or borrows: scalars of any numeric carrier, text, borrowed lists and
/// hashed collections, and owned models, enums (including `Option`, `Result` and unions) and tuples.
pub fn default_type(ty: &PlanType) -> Result<(), String> {
    if is_list(ty) || is_collection(ty) || numeric_carrier(ty) {
        return Ok(());
    }
    match ty {
        // Models, Option, Result, source enum and tuple values move out of the default's frame like any owned result.
        PlanType::Bool | PlanType::String | PlanType::Model(..) | PlanType::Enum(..) | PlanType::Tuple(_) => Ok(()),
        _ => Err("unsupported Body IR defaulted parameter type".into()),
    }
}
