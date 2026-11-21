//! Admission of monomorphic scalar builtins by checked identity and representation.

use incan_mir_plan_native::{CalleeKind, ExternalFunction, Operand, Place, PlanType, SourceSpan};
use incan_semantics_core::body_ir::BuiltinFnId;

use crate::collections::is_owned_collection;
use crate::frame::{Builder, external_call, terminate_call, value_type};
use crate::tuples::tuple_has_owners;

/// Select runtime entry points only for representations whose legacy contract is retained.
pub fn scalar_builtin(builtin: BuiltinFnId, ty: &PlanType) -> Option<(&'static str, PlanType)> {
    match (builtin, ty) {
        (BuiltinFnId::Abs, PlanType::Int) => Some(("absolute_int", PlanType::Int)),
        (BuiltinFnId::Bool, PlanType::Int) => Some(("truth_int", PlanType::Bool)),
        (BuiltinFnId::Bool, PlanType::Float) => Some(("truth_float", PlanType::Bool)),
        (BuiltinFnId::Bool, PlanType::Bool) => Some(("truth_bool", PlanType::Bool)),
        _ => None,
    }
}

/// Retain one generic instantiation and its exact safe runtime signature.
#[allow(clippy::too_many_arguments)] // The Incan helper's eight parts of one runtime call.
pub fn list_call(
    builder: &mut Builder,
    name: &str,
    element: PlanType,
    parameters: Vec<PlanType>,
    result: PlanType,
    arguments: Vec<Operand>,
    destination: Place,
    span: &SourceSpan,
) {
    let path = format!("incan_native_runtime::{name}");
    let owned = matches!(result, PlanType::Model(..) | PlanType::Enum(..))
        || result == PlanType::String
        || is_owned_collection(&result)
        || tuple_has_owners(&result);
    let local = destination.local;
    builder.externals.push(ExternalFunction {
        path: path.clone(),
        type_arguments: vec![element.clone()],
        parameters,
        return_type: result,
        span: span.clone(),
    });
    terminate_call(
        builder,
        CalleeKind::Instantiated(path, vec![element]),
        arguments,
        destination,
        span,
    );
    if owned {
        builder.owned.push(local);
    }
}

/// Combine pure Body IR operands; source short-circuiting is retained by frontend branches.
pub fn emit_boolean_operator(
    builder: &mut Builder,
    destination: Place,
    operands: Vec<Operand>,
    name: &str,
    span: &SourceSpan,
) -> Result<(), String> {
    if operands.len() != 2
        || value_type(builder, &operands[0])? != PlanType::Bool
        || value_type(builder, &operands[1])? != PlanType::Bool
    {
        return Err("unsupported Body IR non-boolean logical operands".into());
    }
    external_call(
        builder,
        &format!("incan_native_runtime::{name}"),
        vec![PlanType::Bool, PlanType::Bool],
        PlanType::Bool,
        operands,
        destination,
        span,
    );
    Ok(())
}

/// Bind exponentiation only when both operands retain the legacy scalar carrier.
pub fn emit_power_operator(
    builder: &mut Builder,
    destination: Place,
    operands: Vec<Operand>,
    span: &SourceSpan,
) -> Result<(), String> {
    let ty = value_type(builder, &operands[0])?;
    if value_type(builder, &operands[1])? != ty {
        return Err("unsupported Body IR mixed Pow operands".into());
    }
    let path = match ty {
        PlanType::Float | PlanType::F64 => "f64::powf",
        PlanType::F32 => "f32::powf",
        PlanType::Int => "incan_native_runtime::power_i64",
        _ => return Err("unsupported Body IR Pow carrier".into()),
    };
    external_call(
        builder,
        path,
        vec![ty.clone(), ty.clone()],
        ty,
        operands,
        destination,
        span,
    );
    Ok(())
}
