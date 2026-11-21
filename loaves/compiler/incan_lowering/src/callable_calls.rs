//! Calls through callable values, closures held by carriers, and callable objects.

use incan_mir_plan_native::{
    CalleeKind, Carrier, Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign, carrier_position,
};
use incan_semantics_core::body_ir::{
    ArgumentElement, Body, BodyIrModule, NamedCallableTarget, Place as SourcePlace, StringConversionContract,
};
use incan_semantics_core::{
    CanonicalSymbolId, IncanType, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin, canonical_module_identity,
};

use crate::async_::is_future;
use crate::builtin_calls::lower_builtin;
use crate::calls::{lower_loaf_call, ordered_arguments};
use crate::carrier_operands::boundary_plan_operand;
use crate::classes::function_name;
use crate::closures::{callable_copy_type, return_representation};
use crate::collections::is_owned_collection;
use crate::enums::enum_type;
use crate::explicit_arguments::validate_source_input_call;
use crate::frame::{
    Builder, drop_owned, forget_moved_arguments, place_type, read_place, temporary, terminate_call, value_type,
};
use crate::layout_types::native_source_type;
use crate::list_methods::call_destination;
use crate::models::model_type;
use crate::pending::untranslated;
use crate::statements::is_nominal_type;
use crate::string_conversion_contracts::conversion_trait;
use crate::traits::{close_trait_dispatch_arguments, instantiate_type, is_lazy_method_body};
use crate::type_parameters::{request_instance, substitute};

/// Fill an environment-owning payload selected by the retained union declaration, preserving captured values.
pub fn carrier_callable_operand(
    builder: &mut Builder,
    value: Operand,
    target: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let _ = span;
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
            return Err(untranslated("callable_calls::owned_callable_value"));
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
    if let Some(identity) = &target.canonical
        && identity.kind == SemanticSourceTargetKind::Method
        && identity.declaration_name == "__fields__"
        && target.receiver_type.is_some()
    {
        return Err(untranslated("reflection::lower_static_field_metadata"));
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
        return Err(untranslated("callable_calls::lower_type_parameter_call"));
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
    if has_adapter_slot(module, identity) {
        return Ok(Some("primitive_traits::normalized_primitive_static"));
    }
    let method = identity.kind == SemanticSourceTargetKind::Method;
    if method && identity.declaration_name == "__fields__" && target.receiver_type.is_some() {
        return Ok(Some("reflection::lower_static_field_metadata"));
    }
    if builder.constructors().validated_constructors.contains(identity) {
        return Ok(Some("callable_calls::lower_validated_constructor"));
    }
    if method && identity.declaration_name == "default" && identity_in(identity, "std", "derives", "copying") {
        return Ok(Some("derived_defaults::lower_derived_default"));
    }
    if method && identity.declaration_name == "from_json" && identity_in(identity, "std", "serde", "json") {
        return Ok(Some("model_values::lower_model_from_json"));
    }
    if let StringConversionContract::Selected(selection) = &module.string_conversion_contract
        && selection.method_identity == *identity
        && !conversion_trait(module)?.is_empty()
    {
        return Ok(Some("string_conversion::string_conversion_callee"));
    }
    if identity.declaration_name == "from_value"
        && identity.namespace == SymbolNamespace::Member
        && method
        && module.value_enum_declarations.iter().any(|declaration| {
            identity.origin == declaration.canonical.origin
                && identity.scope_discriminant == declaration.canonical.scope_discriminant
                && identity.declaration_span == declaration.canonical.declaration_span
        })
    {
        return Ok(Some("value_enums::lower_value_enum_from_value"));
    }
    Ok(None)
}

/// Whether a canonical identity is declared in the standard module `std.<package>.<module>`, written in source or
/// compiled into its standard Loaf.
fn identity_in(identity: &CanonicalSymbolId, root: &str, package: &str, module: &str) -> bool {
    match &identity.origin {
        SymbolOrigin::Module(path) => path.len() == 3 && path[0] == root && path[1] == package && path[2] == module,
        SymbolOrigin::Package { library, module_path } => {
            let expected = if package == "derives" {
                "incan_stdlib_primitives"
            } else {
                "incan_stdlib_data"
            };
            library == expected && module_path.len() == 2 && module_path[0] == package && module_path[1] == module
        }
        _ => false,
    }
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
        return Err(untranslated("async_::construct_future"));
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
    if let PlanType::Closure(_) = result_type {
        return Err(untranslated("callable_calls::owned closure result"));
    }
    if result_type == PlanType::String
        || is_nominal_type(&result_type)
        || is_owned_collection(&result_type)
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
