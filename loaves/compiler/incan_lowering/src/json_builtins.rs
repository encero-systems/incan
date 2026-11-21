//! The JSON builtin and the JSON protocol default: models and enums through their Serialize derive, scalars, text and
//! lists through the runtime's serde boundary.

use incan_mir_plan_native::{
    CalleeKind, ExternalFunction, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign,
};
use incan_semantics_core::body_ir::{
    ArgumentElement, Constant as SourceConstant, Operand as SourceOperand, Place as SourcePlace,
};

use crate::checked_values::scalar_place;
use crate::element_places::is_element_place;
use crate::enum_values::serialize_enum_operand;
use crate::frame::{
    Builder, drop_owned, external_call, forget_owned, place_type, read_place, terminate_call, value_type,
};
use crate::list_methods::call_destination;
use crate::list_values::{checked_list_argument, value_operand};
use crate::lists::{is_list, require_list_shape};
use crate::model_values::serialize_model_operand;
use crate::operand_borrows::borrow_text_operand;
use crate::text_plan::owned_literal;

/// Serialize exactly one checked operand through the same implementation as the JSON protocol default.
pub fn lower_model_json_builtin(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 1 {
        return Err("unsupported Body IR json_stringify arity".into());
    }
    let Some(value) = arguments[0].as_one() else {
        return Err("unsupported Body IR json_stringify spread argument".into());
    };
    let result = call_destination(builder, destination, PlanType::String, span)?;
    if serialize_model_operand(builder, value, result.clone(), span)? {
        return Ok(());
    }
    if serialize_enum_operand(builder, value, result.clone(), span)? {
        return Ok(());
    }
    serialize_scalar_json(builder, value, result, span)
}

/// Preserve canonical scalar carriers and borrow string storage for the legacy JSON builtin.
fn serialize_scalar_json(
    builder: &mut Builder,
    value: &SourceOperand,
    result: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    match value {
        SourceOperand::Constant(SourceConstant::Str(_)) => return serialize_json_text(builder, value, result, span),
        SourceOperand::Constant(SourceConstant::None) => {
            // Legacy gives untyped None a unit payload; every empty Option serializes as null.
            let text = owned_literal(builder, "null", span);
            forget_owned(builder, text.local);
            drop_owned(builder, result.local, span);
            let local = result.local;
            builder.emit(plan_assign(result, RvalueKind::Use(read_place(text, true, span)), span));
            builder.owned.push(local);
            return Ok(());
        }
        SourceOperand::Place(read) => {
            if !is_element_place(builder, &read.place)? {
                let place = scalar_place(builder, &read.place, span)?;
                let ty = place_type(&builder.locals, &place);
                if matches!(ty, PlanType::String | PlanType::StringRef | PlanType::StrRef) {
                    return serialize_json_text(builder, value, result, span);
                }
                if is_list(&ty) {
                    return serialize_json_list(builder, value, &ty, result, span);
                }
            }
        }
        SourceOperand::Constant(_) => {}
    }
    let operand = value_operand(builder, value, span)?;
    let ty = value_type(builder, &operand)?;
    let name = match ty {
        PlanType::Int => "serialize_int",
        PlanType::Float => "serialize_float",
        PlanType::Bool => "serialize_bool",
        _ => return Err(format!("unsupported Body IR JSON serialization type {ty:?}")),
    };
    external_call(
        builder,
        &format!("incan_native_runtime::{name}"),
        vec![ty],
        PlanType::String,
        vec![operand],
        result,
        span,
    );
    Ok(())
}

/// Serialize a borrowed list through the same serde stringify boundary legacy's `json_stringify` uses.
fn serialize_json_list(
    builder: &mut Builder,
    value: &SourceOperand,
    ty: &PlanType,
    result: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let (leaf, depth) = require_list_shape(ty, "unsupported Body IR JSON serialization of a non-list")?;
    let reference = checked_list_argument(builder, value, false, span)?;
    serialize_model_call(
        builder,
        PlanType::List(leaf.clone(), depth),
        PlanType::ListRef(leaf, depth),
        reference,
        result,
        span,
    );
    Ok(())
}

/// Borrow string text into serde's generic boundary, avoiding recursion through runtime's serialize_string wrapper.
fn serialize_json_text(
    builder: &mut Builder,
    value: &SourceOperand,
    result: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let operand = borrow_text_operand(builder, value, span)?;
    serialize_model_call(builder, PlanType::String, PlanType::StringRef, operand, result, span);
    Ok(())
}

/// Call the runtime's generic serialize over one shared reference; the produced text is a new owner.
fn serialize_model_call(
    builder: &mut Builder,
    owned: PlanType,
    reference: PlanType,
    argument: incan_mir_plan_native::Operand,
    result: Place,
    span: &SourceSpan,
) {
    let path = "incan_native_runtime::serialize_model";
    builder.externals.push(ExternalFunction {
        path: path.into(),
        type_arguments: vec![owned.clone()],
        parameters: vec![reference],
        return_type: PlanType::String,
        span: span.clone(),
    });
    let local = result.local;
    terminate_call(
        builder,
        CalleeKind::Instantiated(path.into(), vec![owned]),
        vec![argument],
        result,
        span,
    );
    builder.owned.push(local);
}
