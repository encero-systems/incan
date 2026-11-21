//! Native value representations shared by primitive lowering and guarded call admission.

use incan_mir_plan_native::{ListLeaf, PlanType};
use incan_semantics_core::IncanPrimitiveType;

use crate::numerics::{numeric_carrier, numeric_type};

/// Admit values that leave a static guard by value, including retained Option, Result and source enum carriers.
pub fn static_method_result_type(ty: &PlanType) -> bool {
    if numeric_carrier(ty) {
        return true;
    }
    matches!(
        ty,
        PlanType::Bool
            | PlanType::Unit
            | PlanType::String
            | PlanType::FunctionPointer(_)
            | PlanType::Model(..)
            | PlanType::Enum(..)
            | PlanType::List(..)
    )
}

/// Map the admitted primitive vocabulary once, including exact sized numeric carriers.
pub fn primitive_type(primitive: &IncanPrimitiveType) -> Result<PlanType, String> {
    match primitive {
        IncanPrimitiveType::Numeric(kind) => numeric_type(*kind),
        IncanPrimitiveType::Int => Ok(PlanType::Int),
        IncanPrimitiveType::Float => Ok(PlanType::Float),
        IncanPrimitiveType::Bool => Ok(PlanType::Bool),
        IncanPrimitiveType::Unit => Ok(PlanType::Unit),
        IncanPrimitiveType::Str => Ok(PlanType::String),
        IncanPrimitiveType::FrozenStr => Ok(PlanType::FrozenStr),
        IncanPrimitiveType::FrozenBytes => Ok(PlanType::FrozenBytes),
        IncanPrimitiveType::Bytes => Ok(PlanType::List(ListLeaf::U8, 1)),
    }
}
