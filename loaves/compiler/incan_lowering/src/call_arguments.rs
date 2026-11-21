//! Call argument storage: the parameter representation each frame slot holds and each call materializes.

use incan_mir_plan_native::PlanType;
use incan_semantics_core::IncanInputAccess;
use incan_semantics_core::body_ir::{Body, CallableParam};

use crate::closures::packed_input;
use crate::generators::is_generator_type;
use crate::lists::exclusive_reference;
use crate::statements::is_nominal_type;

/// A deferred producer owns its value parameters; ordinary frames retain the shared collection ABI.
///
/// Call materialization, frame locals and constructor signatures use this same storage decision so an escaping
/// generator never captures a borrow of a caller's list snapshot. Explicit inputs select their checked access first;
/// deferred producers still refuse that contract until retained dependencies are represented. Inferred shared access
/// arrives at a type with a shared executable form for a private callable, and at any read-only type for a published
/// callable, as on the base; lowering lends both through the same carriers.
pub fn stored_parameter_type(body: &Body, parameter: &CallableParam, ty: PlanType) -> Result<PlanType, String> {
    if parameter.input_access != IncanInputAccess::Inferred {
        if body.is_generator() {
            return Err("unsupported Body IR explicit input access in a deferred producer".into());
        }
        return packed_input(
            ty,
            parameter.kind,
            parameter.mutable,
            parameter.native_owned,
            parameter.input_access,
        );
    }
    if body.is_generator() && !parameter.mutable {
        return Ok(ty);
    }
    packed_input(
        ty,
        parameter.kind,
        parameter.mutable,
        parameter.native_owned,
        parameter.input_access,
    )
}

/// Report whether a value that owns storage has no exclusive reference a `mut` parameter could receive.
pub fn lacks_exclusive_parameter(ty: &PlanType) -> bool {
    if !is_nominal_type(ty) || is_generator_type(ty) {
        return false;
    }
    exclusive_reference(ty).is_none()
}

/// Whether a parameter takes a list, set or dictionary by value rather than through a borrow.
pub fn owned_collection(ty: &PlanType) -> bool {
    matches!(ty, PlanType::List(..) | PlanType::Set(_) | PlanType::Dict(..))
}
