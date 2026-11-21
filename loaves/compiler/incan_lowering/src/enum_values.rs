//! Shared JSON reads of retained source enum layouts with an exact checked serde capability.

use incan_mir_plan_native::{
    CalleeKind, ExternalFunction, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign,
};
use incan_semantics_core::body_ir::Operand as SourceOperand;

use crate::checked_values::scalar_place;
use crate::frame::{Builder, place_type, read_place, temporary, terminate_call};

/// Borrow a whole source enum through its retained Serialize derive; propagated raw-value refusals remain upstream.
pub fn serialize_enum_operand(
    builder: &mut Builder,
    value: &SourceOperand,
    destination: Place,
    span: &SourceSpan,
) -> Result<bool, String> {
    let SourceOperand::Place(read) = value else {
        return Ok(false);
    };
    if !read.place.projection.is_empty() || read.place.local_id().is_none() {
        return Ok(false);
    }
    let source = scalar_place(builder, &read.place, span)?;
    let PlanType::Enum(owner, name) = place_type(&builder.locals, &source) else {
        return Ok(false);
    };
    if owner < 0 || owner as usize >= builder.enums().len() {
        return Ok(false);
    }
    if !builder.enums()[owner as usize]
        .derives
        .iter()
        .any(|derive| derive == "serde::Serialize")
    {
        return Err("unsupported Body IR enum JSON serialization without serde::Serialize".into());
    }
    let reference_type = PlanType::EnumRef(owner, name.clone());
    let reference = temporary(builder, reference_type.clone(), span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(source), span));
    let path = "incan_native_runtime::serialize_model";
    let ty = PlanType::Enum(owner, name);
    builder.externals.push(ExternalFunction {
        path: path.into(),
        type_arguments: vec![ty.clone()],
        parameters: vec![reference_type],
        return_type: PlanType::String,
        span: span.clone(),
    });
    let local = destination.local;
    terminate_call(
        builder,
        CalleeKind::Instantiated(path.into(), vec![ty]),
        vec![read_place(reference, false, span)],
        destination,
        span,
    );
    builder.owned.push(local);
    Ok(true)
}
