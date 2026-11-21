//! Calls through callable values, closures held by carriers, and callable objects.

use incan_mir_plan_native::{
    CalleeKind, Carrier, Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign, carrier_position,
};
use incan_semantics_core::body_ir::{ArgumentElement, Body, BodyIrModule, NamedCallableTarget, Place as SourcePlace};
use incan_semantics_core::{CanonicalSymbolId, IncanType, SymbolOrigin, canonical_module_identity};

use crate::async_::{construct_future, is_future};
use crate::builtin_calls::lower_builtin;
use crate::call_arguments::{SlotPolicy, ordered_slot_operands};
use crate::callable_values::owned_callable_value;
use crate::calls::{lower_loaf_call, ordered_arguments};
use crate::carrier_operands::boundary_plan_operand;
use crate::checked_values::scalar_place;
use crate::classes::function_name;
use crate::closures::{callable_copy_type, return_representation};
use crate::collections::is_owned_collection;
use crate::derived_defaults::{is_derived_default, lower_derived_default};
use crate::enums::enum_type;
use crate::explicit_arguments::validate_source_input_call;
use crate::frame::{
    Builder, drop_owned, forget_moved_arguments, place_type, read_place, temporary, terminate_call, value_type,
};
use crate::generic_callables::owned_callable_slots;
use crate::layout_types::native_source_type;
use crate::list_methods::call_destination;
use crate::model_values::{is_json_deserialize_method, lower_model_from_json};
use crate::models::model_type;
use crate::pending::untranslated;
use crate::reflection::lower_static_field_metadata;
use crate::statements::is_nominal_type;
use crate::string_conversion::string_conversion_callee;
use crate::traits::{close_trait_dispatch_arguments, instantiate_type, is_lazy_method_body};
use crate::type_parameters::{indexed_static_type_arguments, instantiate, request_instance, substitute};
use crate::validated_constructors::lower_validated_constructor;
use crate::value_enum_methods::lower_value_enum_from_value;
use crate::value_enums::value_enum_from_value_owner;

/// Fill an environment-owning payload selected by the retained union declaration, preserving captured values.
pub fn carrier_callable_operand(
    builder: &mut Builder,
    value: Operand,
    target: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let PlanType::Enum(owner, _) = target else {
        return Ok(value);
    };
    if *owner < builder.enums().len() as i64 {
        return Ok(value);
    }
    let declaration = &builder.intrinsics[carrier_position(*owner) as usize];
    let source = value_type(builder, &value)?;
    if declaration.carrier != Carrier::Declared || !callable_copy_type(&source) {
        return Ok(value);
    }
    let signature = match &source {
        PlanType::FunctionPointer(signature) | PlanType::FunctionItem(_, signature) => signature.clone(),
        _ => return Ok(value),
    };
    for variant in &declaration.variants {
        if variant.fields.len() == 1 && variant.fields[0] == PlanType::Closure(signature.clone()) {
            return owned_callable_value(builder, value, &signature, span);
        }
    }
    Ok(value)
}

/// Close checked call arguments in their trait scope and method scope, then require admitted carriers.
pub fn checked_type_arguments(builder: &Builder, arguments: &[IncanType]) -> Result<Vec<IncanType>, String> {
    let retained = close_trait_dispatch_arguments(&builder.trait_environment, arguments.to_vec());
    let mut closed = Vec::with_capacity(retained.len());
    for argument in &retained {
        let concrete = substitute(argument, &builder.type_parameters, &builder.type_arguments)
            .map_err(|reason| format!("{reason} in checked call arguments of {}", builder.name))?;
        native_source_type(
            builder.constructors(),
            builder.nominals(),
            builder.enums(),
            &builder.unions,
            &concrete,
            &builder.intrinsics,
            "",
        )?;
        closed.push(concrete);
    }
    Ok(closed)
}

/// Call a builtin, a retained project function, or a proven stdlib forwarder selected by canonical identity.
pub fn lower_named_target(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &NamedCallableTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if lower_static_field_metadata(builder, target, arguments, destination, span)? {
        return Ok(());
    }
    if let Some(builtin) = target.builtin {
        if !target.type_args.is_empty() {
            return Err("unsupported Body IR builtin type arguments".into());
        }
        return lower_builtin(builder, builtin, arguments, destination, span);
    }
    if lower_validated_constructor(module, builder, target, arguments, destination, span)? {
        return Ok(());
    }
    if is_derived_default(target) {
        return lower_derived_default(module, builder, target, destination, span);
    }
    if is_json_deserialize_method(target) {
        return lower_model_from_json(module, builder, target, arguments, destination, span);
    }
    if let Some((callee, result)) = string_conversion_callee(module, builder, target, span)? {
        let policy = SlotPolicy {
            owned: true,
            ..SlotPolicy::default()
        };
        let operands = ordered_slot_operands(builder, &target.binding, arguments, &[PlanType::String], span, &policy)?;
        // The result temporary is reserved before the destination, as the reference reserves it.
        let mut output = temporary(builder, result.clone(), span);
        if let Some(place) = destination {
            output = scalar_place(builder, place, span)?;
        }
        if place_type(&builder.locals, &output) != result {
            return Err("unsupported Body IR string conversion Result destination".into());
        }
        return lower_callable_result(builder, callee, operands, output, result, span);
    }
    if let Some(owner) = value_enum_from_value_owner(module, target)? {
        return lower_value_enum_from_value(builder, owner, arguments, destination, span);
    }
    let Some(identity) = &target.canonical else {
        return Err("unsupported Body IR builtin or unproven NamedCallableTarget".into());
    };
    if is_trait_associated_target(module, target) && static_receiver_owner(builder, module, target)?.is_some() {
        return Err(untranslated("traits::owner_callable_identity"));
    }
    let Some(index) = builder.program.body_indices.unique(identity) else {
        return lower_loaf_call(module, builder, target, arguments, destination, span);
    };
    let body = &module.bodies[index];
    if !body.type_parameters.is_empty() {
        return lower_type_parameter_call(module, builder, target, body, arguments, destination, span);
    }
    if !target.type_args.is_empty() {
        return Err("unsupported Body IR type arguments on a function without type parameters".into());
    }
    static_receiver_owner(builder, module, target)?;
    if is_lazy_method_body(module, body) {
        request_instance(&mut builder.instances, body, Vec::new(), Vec::new(), false);
    }
    lower_local_call(module, builder, target, body, arguments, destination, span)
}

/// Schedule the checker-selected closed frame, refusing every unadmitted type argument before use.
fn lower_type_parameter_call(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &NamedCallableTarget,
    body: &Body,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    static_receiver_owner(builder, module, target)?;
    let indexed = indexed_static_type_arguments(builder.constructors(), target)?;
    let closed = checked_type_arguments(builder, &indexed)?;
    let typed_body = instantiate(body, &closed, &[])?;
    let owned = owned_callable_slots(builder, body, &typed_body, arguments, &target.binding, 0)?;
    let concrete_body = if owned.is_empty() {
        typed_body
    } else {
        instantiate(body, &closed, &owned)?
    };
    request_instance(&mut builder.instances, body, closed, owned, false);
    lower_local_call(module, builder, target, &concrete_body, arguments, destination, span)
}

/// Authenticate an exact retained adapter slot before inspecting an unrelated call's operands.
pub fn has_adapter_slot(module: &BodyIrModule, slot: &CanonicalSymbolId) -> bool {
    module.nominal_declarations.iter().any(|declaration| {
        declaration.trait_adoptions.iter().any(|adoption| {
            adoption.primitive_adapter.is_some()
                && adoption.methods.as_ref().is_some_and(|methods| methods.contains(slot))
        })
    })
}

/// Return whether a retained adoption records this associated target as its checked trait slot.
fn is_trait_associated_target(module: &BodyIrModule, target: &NamedCallableTarget) -> bool {
    match &target.canonical {
        Some(identity) => module
            .trait_implementations
            .iter()
            .any(|implementation| implementation.method == *identity),
        None => false,
    }
}

/// Close an associated call's checked receiver and return its retained nominal owner when present.
fn static_receiver_owner(
    builder: &Builder,
    module: &BodyIrModule,
    target: &NamedCallableTarget,
) -> Result<Option<PlanType>, String> {
    let Some(template) = &target.receiver_type else {
        return Ok(None);
    };
    let retained = match &builder.trait_environment {
        Some(instantiation) => instantiate_type(template, &instantiation.receiver_type, &instantiation.arguments),
        None => template.clone(),
    };
    let closed = substitute(&retained, &builder.type_parameters, &builder.type_arguments)
        .map_err(|reason| format!("{reason} in static receiver of {}", target.name))?;
    if let Some(owner) = enum_type(&module.enum_declarations, &closed) {
        return Ok(Some(owner));
    }
    match model_type(builder.constructors(), &module.nominal_declarations, &closed, "")? {
        PlanType::Model(owner, name) => Ok(Some(PlanType::Model(owner, name))),
        _ => Err("unsupported Body IR nonnominal static receiver".into()),
    }
}

/// Call a canonical retained body; local span identities must agree when present.
fn lower_local_call(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &NamedCallableTarget,
    body: &Body,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    match &target.direct_call_id {
        // Publication removes source-local call ids even within one package fragment. Its exact validated canonical
        // target selected this unique body; source modules still require their local span proof.
        None => match &body.canonical {
            Some(identity) => match &identity.origin {
                SymbolOrigin::Module(_) => {
                    if canonical_module_identity(identity).as_deref() == Some(builder.module_identity.as_str()) {
                        return Err("unsupported Body IR local NamedCallableTarget without direct identity".into());
                    }
                }
                SymbolOrigin::Package { .. } => {}
                _ => return Err("unsupported Body IR unproven NamedCallableTarget".into()),
            },
            None => return Err("unsupported Body IR unproven NamedCallableTarget".into()),
        },
        Some(call_id) => {
            if *call_id != body.direct_call_id {
                return Err("Body IR call identity disagrees with target body".into());
            }
        }
    }
    let operands = ordered_arguments(module, builder, &target.binding, arguments, body, span)?;
    if body.is_async {
        let Some(place) = destination else {
            return Err("unsupported Body IR async call without carrier".into());
        };
        let output = scalar_place(builder, place, span)?;
        return construct_future(module, builder, body, operands, output, span);
    }
    let callee = CalleeKind::Planned(function_name(module, body));
    emit_selected_body_call(module, builder, body, callee, operands, destination, span)
}

/// Preserve the selected body's result carrier and unwind cleanup for direct and guarded static calls.
pub fn emit_selected_body_call(
    module: &BodyIrModule,
    builder: &mut Builder,
    body: &Body,
    callee: CalleeKind,
    operands: Vec<Operand>,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    validate_source_input_call(module, builder, body)?;
    let declared = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &body.return_type,
        &builder.intrinsics,
        &builder.file,
    )?;
    let result_type = return_representation(body, declared)?;
    let output = call_destination(builder, destination, result_type.clone(), span)?;
    lower_callable_result(builder, callee, operands, output, result_type, span)
}

/// Replace an owned return through its checked destination carrier, preserving unwind cleanup.
///
/// 1. Materialize owned results in their declared return carrier.
/// 2. Apply the ordinary checked value-boundary conversions before dropping the old destination.
/// 3. Transfer the converted owner to the destination; trivial results use their destination directly.
pub fn lower_callable_result(
    builder: &mut Builder,
    callee: CalleeKind,
    operands: Vec<Operand>,
    output: Place,
    result_type: PlanType,
    span: &SourceSpan,
) -> Result<(), String> {
    let mut owned_closure = false;
    if let PlanType::Closure(_) = result_type {
        // The destination's function type takes the returned closure's own boxed representation.
        if output.projection != incan_mir_plan_native::Projection::Whole {
            return Err("unsupported Body IR projected closure result".into());
        }
        builder.locals[output.local as usize].ty = result_type.clone();
        owned_closure = true;
    }
    if result_type == PlanType::String
        || is_nominal_type(&result_type)
        || is_owned_collection(&result_type)
        || owned_closure
        || is_future(&result_type)
    {
        // 1. Materialize the result in its declared return carrier.
        let result = temporary(builder, result_type, span);
        terminate_call(builder, callee, operands, result.clone(), span);
        builder.owned.push(result.local);
        // 2. Convert the checked boundary before replacing the old destination.
        let target_type = place_type(&builder.locals, &output);
        let value = boundary_plan_operand(builder, read_place(result, true, span), &target_type, span)?;
        drop_owned(builder, output.local, span);
        // 3. Transfer exactly the selected owner to its destination.
        forget_moved_arguments(builder, std::slice::from_ref(&value));
        let local = output.local;
        builder.emit(plan_assign(output, RvalueKind::Use(value), span));
        builder.owned.push(local);
    } else {
        terminate_call(builder, callee, operands, output, span);
    }
    Ok(())
}
