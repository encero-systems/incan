//! Calls through callable values, closures held by carriers, and callable objects.

use incan_mir_plan_native::{
    CalleeKind, Carrier, Operand, Place, PlanType, Projection, RvalueKind, SourceSpan, assign as plan_assign,
    carrier_position,
};
use incan_semantics_core::body_ir::{
    ArgumentElement, Body, BodyIrModule, NamedCallableTarget, Operand as SourceOperand, Place as SourcePlace,
    StringConversionContract,
};
use incan_semantics_core::{CanonicalSymbolId, IncanType, SymbolOrigin, canonical_module_identity};

use crate::async_::{construct_future, is_future};
use crate::builtin_calls::lower_builtin;
use crate::callable_values::callable_object;
use crate::calls::{lower_loaf_call, ordered_arguments};
use crate::carrier_operands::boundary_plan_operand;
use crate::checked_values::{partial_flags, scalar_place};
use crate::classes::function_name;
use crate::closure_lifting::synthesize;
use crate::closures::{
    borrowed_callable_forwarder, callable_copy_type, callable_signature, closure_arguments, closure_constructor,
    pointer_forwarder, return_representation,
};
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
use crate::string_conversion_contracts::conversion_trait;
use crate::traits::{close_trait_dispatch_arguments, instantiate_type, is_lazy_method_body};
use crate::type_parameters::{indexed_static_type_arguments, instantiate, request_instance, substitute};
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
    if let Some(step) = special_named_target(module, builder, target)? {
        return Err(untranslated(step));
    }
    if is_derived_default(target) {
        return lower_derived_default(module, builder, target, destination, span);
    }
    if is_json_deserialize_method(target) {
        return lower_model_from_json(module, builder, target, arguments, destination, span);
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
    let owned = owned_callable_slots(builder, body, arguments, &target.binding, 0)?;
    request_instance(&mut builder.instances, body, closed, owned, false);
    lower_local_call(module, builder, target, &typed_body, arguments, destination, span)
}

/// The special call route a named target selects before ordinary dispatch, if any: primitive adapters, generated
/// static metadata, builtins, validated constructors, derived defaults, JSON decoding, string conversion and value
/// enum members each lower through their own step.
fn special_named_target(
    module: &BodyIrModule,
    builder: &Builder,
    target: &NamedCallableTarget,
) -> Result<Option<&'static str>, String> {
    if target.builtin.is_some() {
        return Ok(Some("builtin_calls::lower_builtin"));
    }
    let Some(identity) = &target.canonical else {
        return Ok(None);
    };
    if builder.constructors().validated_constructors.contains(identity) {
        return Ok(Some("callable_calls::lower_validated_constructor"));
    }
    if let StringConversionContract::Selected(selection) = &module.string_conversion_contract
        && selection.method_identity == *identity
        && !conversion_trait(module)?.is_empty()
    {
        return Ok(Some("string_conversion::string_conversion_callee"));
    }
    Ok(None)
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
    let owned_closure = matches!(result_type, PlanType::Closure(_));
    if owned_closure {
        // The destination's function type takes the returned closure's own boxed representation.
        if output.projection != Projection::Whole {
            return Err("unsupported Body IR projected closure result".into());
        }
        builder.locals[output.local as usize].ty = result_type.clone();
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

/// Own a forwarding environment so either a borrowed argument or a union payload retains the function value.
fn pointer_callable_owner(
    builder: &mut Builder,
    pointer: Place,
    signature: &[PlanType],
    span: &SourceSpan,
) -> Result<Place, String> {
    closure_arguments(signature)?;
    let name = format!("__incan_pointer_closure_{}_{}", builder.name, builder.synthesized.len());
    let forwarder = pointer_forwarder(&name, signature, span)?;
    synthesize(builder, forwarder);
    let constructor = format!("{name}_new");
    let captures = [PlanType::FunctionPointer(signature.to_vec())];
    let object = closure_constructor(
        &constructor,
        &name,
        &captures,
        signature,
        false,
        span,
        false,
        false,
        false,
    );
    synthesize(builder, object);
    let boxed = temporary(builder, PlanType::Closure(signature.to_vec()), span);
    let local = boxed.local;
    let arguments = vec![read_place(pointer, false, span)];
    terminate_call(
        builder,
        CalleeKind::Planned(constructor),
        arguments,
        boxed.clone(),
        span,
    );
    builder.owned.push(local);
    Ok(boxed)
}

/// Retain a function item's forwarding environment at any environment-owning value boundary.
pub fn owned_callable_value(
    builder: &mut Builder,
    value: Operand,
    signature: &[PlanType],
    span: &SourceSpan,
) -> Result<Operand, String> {
    let source = value_type(builder, &value)?;
    if source == PlanType::Closure(signature.to_vec()) {
        return Ok(value);
    }
    if !callable_copy_type(&source) || callable_signature(&source)? != signature {
        return Err("unsupported Body IR callable ownership conversion".into());
    }
    let pointer = temporary(builder, PlanType::FunctionPointer(signature.to_vec()), span);
    let operation = match source {
        PlanType::FunctionItem(..) => RvalueKind::ReifyFunction(value),
        _ => RvalueKind::Use(value),
    };
    builder.emit(plan_assign(pointer.clone(), operation, span));
    let boxed = pointer_callable_owner(builder, pointer, signature, span)?;
    Ok(read_place(boxed, true, span))
}

/// Adapt a callable to the parameter's shared view or call-owned lending envelope.
pub fn callable_argument(
    builder: &mut Builder,
    value: &SourceOperand,
    signature: &[PlanType],
    span: &SourceSpan,
    borrowing: bool,
) -> Result<Operand, String> {
    let SourceOperand::Place(read) = value else {
        return Err("unsupported Body IR constant passed to a closure-holding parameter".into());
    };
    let place = scalar_place(builder, &read.place, span)?;
    if partial_flags(builder, place.local).is_some() {
        return Err("unsupported Body IR partial passed to a closure-holding parameter".into());
    }
    let source = place_type(&builder.locals, &place);
    if callable_signature(&source)? != signature {
        return Err("invalid Body IR callable argument signature".into());
    }
    let owner = match source {
        PlanType::FunctionItem(..) => {
            let pointer = temporary(builder, PlanType::FunctionPointer(signature.to_vec()), span);
            let item = read_place(place, false, span);
            builder.emit(plan_assign(pointer.clone(), RvalueKind::ReifyFunction(item), span));
            pointer_callable_owner(builder, pointer, signature, span)?
        }
        PlanType::FunctionPointer(_) => pointer_callable_owner(builder, place, signature, span)?,
        _ => place,
    };
    if borrowing {
        return borrowed_callable_argument(builder, owner, signature, span);
    }
    callable_object(builder, owner, signature, span)
}

/// Move a call-owned envelope that lends the source callable for the callee's calls.
fn borrowed_callable_argument(
    builder: &mut Builder,
    callable: Place,
    signature: &[PlanType],
    span: &SourceSpan,
) -> Result<Operand, String> {
    let borrowing_source = match place_type(&builder.locals, &callable) {
        PlanType::BorrowingClosure(_) | PlanType::BorrowingClosureRef(_) => true,
        PlanType::Closure(_) | PlanType::ClosureRef(_) => false,
        _ => return Err("unsupported Body IR borrowing callable argument".into()),
    };
    let source = callable_object(builder, callable, signature, span)?;
    let capture = if borrowing_source {
        PlanType::BorrowingClosureRef(signature.to_vec())
    } else {
        PlanType::ClosureRef(signature.to_vec())
    };
    let name = format!(
        "__incan_borrowed_callable_{}_{}",
        builder.name,
        builder.synthesized.len()
    );
    let forwarder = borrowed_callable_forwarder(&name, signature, borrowing_source, span)?;
    synthesize(builder, forwarder);
    let constructor = format!("{name}_new");
    let object = closure_constructor(
        &constructor,
        &name,
        &[capture],
        signature,
        false,
        span,
        false,
        false,
        true,
    );
    synthesize(builder, object);
    let boxed = temporary(builder, PlanType::BorrowingClosure(signature.to_vec()), span);
    let local = boxed.local;
    terminate_call(
        builder,
        CalleeKind::Planned(constructor),
        vec![source],
        boxed.clone(),
        span,
    );
    builder.owned.push(local);
    Ok(read_place(boxed, true, span))
}
