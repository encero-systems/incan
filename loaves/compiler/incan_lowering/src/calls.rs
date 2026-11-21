//! Dispatch checked calls, and call Loaf and delegated native functions.

use incan_mir_plan_native::{CalleeKind, ExternalFunction, Operand, Place, PlanType, SourceSpan};
use incan_semantics_core::body_ir::{
    ArgumentBinding, ArgumentElement, Body, BodyIrModule, CallableTarget, Callee, LoafCallable, NamedCallableTarget,
    Place as SourcePlace, Statement, StatementKind,
};
use incan_semantics_core::{IncanType, SemanticSourceTargetKind, SymbolOrigin};

use crate::async_::construct_boxed_future;
use crate::builtin_calls::lower_runtime_helper;
use crate::call_arguments::{SlotPolicy, call_parameter_type, has_defaulted_slots, ordered_slot_operands};
use crate::callable_calls::{checked_type_arguments, has_adapter_slot, lower_callable_result, lower_named_target};
use crate::callable_values::lower_callable_value;
use crate::closures::return_representation;
use crate::defaults::{DefaultCall, default_arguments, library_default_arguments};
use crate::explicit_arguments::{parameter_accesses, validate_source_input_call};
use crate::frame::Builder;
use crate::layout_types::native_source_type;
use crate::list_methods::call_destination;
use crate::method_calls::lower_method_call;
use crate::pending::untranslated;
use crate::tuples::type_name;
use crate::type_parameters::substitute;
use crate::unions::require_native_union_signature;

/// Normalize checked primitive adapters before the existing dispatcher consumes the complete call record.
pub fn lower_call(
    module: &BodyIrModule,
    builder: &mut Builder,
    statement: &Statement,
    span: &SourceSpan,
) -> Result<(), String> {
    let StatementKind::Call {
        destination,
        callee,
        args,
        ..
    } = &statement.kind
    else {
        return Err("expected Body IR Call".into());
    };
    match callee {
        Callee::Function(CallableTarget::Named(target)) => {
            lower_named_target(module, builder, target, args, destination, span)
        }
        Callee::Function(CallableTarget::Local(target)) => {
            lower_callable_value(builder, target, args, destination, span)
        }
        Callee::Method(target) => {
            if let Some(identity) = &target.canonical
                && has_adapter_slot(module, identity)
            {
                return Err(untranslated("primitive_traits::normalized_primitive_receiver"));
            }
            lower_method_call(module, builder, target, args, destination, span)
        }
        Callee::Helper(helper) => lower_runtime_helper(builder, *helper, args, destination, span),
        Callee::ProviderOperation(_) => Err("unsupported Body IR ProviderOperation".into()),
        Callee::TypeMetadata(_) => {
            if !args.is_empty() {
                return Err("invalid Body IR type metadata value arguments".into());
            }
            Err(untranslated("type_metadata::lower_type_metadata"))
        }
        Callee::CheckedForeign(_) => Err(untranslated("foreign_calls::lower_foreign_call")),
        Callee::CheckedBridge(_) => Err(untranslated("c_bridges::lower_checked_bridge")),
    }
}

/// Call a linked Loaf function (DD-0007) or a checker-selected Rust instantiation (DD-0006).
///
/// The callable is selected by the checker's canonical identity, never by a spelling, and its declaration record
/// supplies the signature the driver checks against the Loaf's metadata.
pub fn lower_loaf_call(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &NamedCallableTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if target.direct_call_id.is_some() {
        return Err("Body IR Loaf call has a local call identity".into());
    }
    let mut mismatch = "no retained callable with the selected identity".to_string();
    for callable in &module.loaf_callables {
        let Some(identity) = &target.canonical else {
            return Err("unsupported Body IR unproven Loaf call target".into());
        };
        if *identity != callable.canonical {
            continue;
        }
        if let SymbolOrigin::RustCrate(_) = identity.origin {
            let written = checked_type_arguments(builder, &target.type_args)?;
            // A template from another source declaration may use binders absent from this frame.
            match checked_type_arguments(builder, &callable.native_type_arguments) {
                Err(reason) => {
                    mismatch = reason;
                    continue;
                }
                Ok(retained) => {
                    if written != retained {
                        mismatch = format!(
                            "native arguments differ: call {}, declaration {}",
                            type_list(&written),
                            type_list(&retained)
                        );
                        continue;
                    }
                }
            }
            if let Some(reason) = native_result_mismatch(builder, callable, destination)? {
                mismatch = format!("retained native signatures disagree with the checked result slot: {reason}");
                continue;
            }
        }
        return lower_delegated_call(module, builder, callable, target, arguments, destination, span);
    }
    Err(format!(
        "unsupported Body IR imported or unresolved canonical call target {} in {}: {mismatch}",
        target.name, builder.name
    ))
}

/// The Incan spelling of a list of checked types, as a refusal prints it.
fn type_list(types: &[IncanType]) -> String {
    let items: Vec<String> = types.iter().map(|ty| format!("{ty:?}")).collect();
    format!("[{}]", items.join(", "))
}

/// Disambiguate checked native records by the checked type of the call's whole result slot in its own frame.
fn native_result_mismatch(
    builder: &Builder,
    declaration: &LoafCallable,
    destination: &Option<SourcePlace>,
) -> Result<Option<String>, String> {
    let Some(place) = destination else {
        return Ok(None);
    };
    if !place.projection.is_empty() || builder.local_offset != 0 {
        return Ok(None);
    }
    let Some(local) = place.local_id() else {
        return Ok(None);
    };
    let index = local.index();
    if builder.checked_local_types.is_empty() {
        return Ok(None);
    }
    if index >= builder.checked_local_types.len() {
        return Err("invalid checked native result slot".into());
    }
    let actual = substitute(
        &builder.checked_local_types[index],
        &builder.type_parameters,
        &builder.type_arguments,
    )?;
    match substitute(
        &declaration.return_type,
        &builder.type_parameters,
        &builder.type_arguments,
    ) {
        Err(reason) => Ok(Some(reason)),
        Ok(selected) => {
            if actual == selected {
                return Ok(None);
            }
            Ok(Some(format!(
                "checked {}; retained {}",
                type_name(&actual),
                type_name(&selected)
            )))
        }
    }
}

/// Emit the external call for one matched delegation: a proven forwarder's Rust callee or the SDK crate item itself.
///
/// A generic crate item takes the call's checked type arguments for its declared type parameters, in order, and the
/// driver instantiates the item with them before checking its signature. An associated SDK trait function closes Self
/// from its checked type receiver before its method arguments. A retained Rust signature also closes the enclosing
/// source declaration's binders before native type admission.
fn lower_delegated_call(
    module: &BodyIrModule,
    builder: &mut Builder,
    delegation: &LoafCallable,
    target: &NamedCallableTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let names: Vec<String> = delegation.type_parameters.clone();
    let checked_rust = native_delegation(delegation);
    let closed = library_function_arguments(builder, target, &names)?;
    if closed.len() != names.len() || (!checked_rust && names.is_empty() && !target.type_args.is_empty()) {
        return Err("unsupported Body IR nonlocal function with type parameters".into());
    }
    // A checked native signature in a source body can name that body's binders. A compiled Loaf instead owns its
    // declaration binders; neither signature infers a type from a runtime operand.
    let (signature_names, signature_arguments) = if checked_rust {
        (builder.type_parameters.clone(), builder.type_arguments.clone())
    } else {
        (names, closed.clone())
    };
    let resolve = |builder: &Builder, ty: &IncanType| {
        native_source_type(
            builder.constructors(),
            builder.nominals(),
            builder.enums(),
            &builder.unions,
            ty,
            &builder.intrinsics,
            &builder.file,
        )
    };
    let mut parameters = Vec::with_capacity(delegation.parameters.len());
    for parameter in &delegation.parameters {
        let instantiated = substitute(parameter, &signature_names, &signature_arguments)?;
        require_native_union_signature(&instantiated, &builder.intrinsics, checked_rust)?;
        parameters.push(resolve(builder, &instantiated)?);
    }
    if let Some(frame) = delegation.declaration_frame() {
        validate_source_input_call(module, builder, frame)?;
        parameters = Vec::with_capacity(frame.params.len());
        for parameter in &frame.params {
            parameters.push(call_parameter_type(builder, frame, parameter)?);
        }
    }
    let returned = substitute(&delegation.return_type, &signature_names, &signature_arguments)?;
    require_native_union_signature(&returned, &builder.intrinsics, checked_rust)?;
    let mut result = resolve(builder, &returned)?;
    if let Some(frame) = delegation.declaration_frame() {
        result = return_representation(frame, result)?;
    }
    let native_arguments = if checked_rust {
        checked_type_arguments(builder, &delegation.native_type_arguments)?
    } else {
        closed
    };
    let mut type_arguments = Vec::with_capacity(native_arguments.len());
    for argument in &native_arguments {
        type_arguments.push(resolve(builder, argument)?);
    }
    if checked_rust && !delegation.native_callable_parameters.is_empty() {
        return Err(untranslated("native_callables::native_callable_storage"));
    }
    let operands = library_call_operands(
        module,
        builder,
        delegation,
        &target.binding,
        arguments,
        &parameters,
        span,
        0,
        true,
    )?;
    let output = call_destination(builder, destination, result.clone(), span)?;
    let Some(path) = delegation.rust_path() else {
        return Err("the standard library call target has no checked native identity".into());
    };
    if let PlanType::Future(outputs) = &result {
        let [output_type] = outputs.as_slice() else {
            return Err("the standard library future requires exactly one output type".into());
        };
        let callee = if type_arguments.is_empty() {
            CalleeKind::External(path)
        } else {
            CalleeKind::Instantiated(path, type_arguments)
        };
        return construct_boxed_future(builder, callee, parameters, output_type.clone(), operands, output, span);
    }
    instantiated_external_call(
        builder,
        &path,
        type_arguments,
        parameters,
        result,
        operands,
        output,
        span,
    )
}

/// Distinguish metadata-selected Rust items from compiled source Loaf signatures before admitting local carriers.
pub fn native_delegation(delegation: &LoafCallable) -> bool {
    matches!(delegation.canonical.origin, SymbolOrigin::RustCrate(_))
}

/// Close SDK associated Self and nominal owner arguments through the enclosing checked frame's binders.
fn library_function_arguments(
    builder: &Builder,
    target: &NamedCallableTarget,
    names: &[String],
) -> Result<Vec<IncanType>, String> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let associated = target
        .canonical
        .as_ref()
        .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Method);
    if names[0] == "Self" && associated {
        let Some(receiver) = &target.receiver_type else {
            return Err("the standard library associated operation has no checked Self type".into());
        };
        let mut closed = checked_type_arguments(builder, std::slice::from_ref(receiver))?;
        closed.extend(checked_type_arguments(builder, &target.type_args)?);
        return Ok(closed);
    }
    match &target.receiver_type {
        Some(IncanType::Generic { args, .. }) => {
            let owner_arguments = checked_type_arguments(builder, args)?;
            if !target.type_args.is_empty() {
                // Older checked nominal calls also retain their owner arguments on the associated call. Exact
                // agreement across both views and all declared binders preserves that proven representation.
                let retained = checked_type_arguments(builder, &target.type_args)?;
                if owner_arguments.len() != names.len() || retained != owner_arguments {
                    return Err("the standard library owner and method type arguments are unsupported".into());
                }
            }
            Ok(owner_arguments)
        }
        _ => checked_type_arguments(builder, &target.type_args),
    }
}

/// Bind a compiled Loaf call's arguments, each passed owned as the crate item takes it.
///
/// An omitted argument evaluates its declaration-owned default in an isolated frame. An offset of one skips a method
/// receiver, the record's first parameter.
#[allow(clippy::too_many_arguments)]
pub fn library_call_operands(
    module: &BodyIrModule,
    builder: &mut Builder,
    delegation: &LoafCallable,
    binding: &ArgumentBinding,
    arguments: &[ArgumentElement],
    types: &[PlanType],
    span: &SourceSpan,
    offset: usize,
    positional: bool,
) -> Result<Vec<Operand>, String> {
    if has_defaulted_slots(binding) {
        let written = &arguments[offset.min(arguments.len())..];
        return library_default_arguments(module, builder, delegation, binding, written, types, span, offset);
    }
    if let Some(frame) = delegation.declaration_frame() {
        if frame.canonical.as_ref() != Some(&delegation.canonical) {
            return Err("invalid Body IR argument declaration frame identity".into());
        }
        let accesses = parameter_accesses(&frame.params, offset)?;
        if accesses.len() != types.len() {
            return Err("invalid Body IR argument declaration frame length".into());
        }
        let policy = SlotPolicy {
            offset,
            owned: false,
            positional,
            accesses,
        };
        return ordered_slot_operands(builder, binding, arguments, types, span, &policy);
    }
    let policy = SlotPolicy {
        offset,
        owned: true,
        positional,
        accesses: Vec::new(),
    };
    ordered_slot_operands(builder, binding, arguments, types, span, &policy)
}

/// Declare one selected external signature and preserve owned-result replacement and cleanup.
#[allow(clippy::too_many_arguments)]
pub fn instantiated_external_call(
    builder: &mut Builder,
    path: &str,
    type_arguments: Vec<PlanType>,
    parameters: Vec<PlanType>,
    result: PlanType,
    operands: Vec<Operand>,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    builder.externals.push(ExternalFunction {
        path: path.to_string(),
        type_arguments: type_arguments.clone(),
        parameters,
        return_type: result.clone(),
        span: span.clone(),
    });
    let callee = if type_arguments.is_empty() {
        CalleeKind::External(path.to_string())
    } else {
        CalleeKind::Instantiated(path.to_string(), type_arguments)
    };
    lower_callable_result(builder, callee, operands, output, result, span)
}

/// Use the body's parameter types when materializing arguments in source order.
pub fn ordered_arguments(
    module: &BodyIrModule,
    builder: &mut Builder,
    binding: &ArgumentBinding,
    elements: &[ArgumentElement],
    body: &Body,
    span: &SourceSpan,
) -> Result<Vec<Operand>, String> {
    let _ = module;
    // Legacy evaluates permuted generator calls in declaration order, after Body IR has evaluated written order.
    if body.is_generator()
        && let ArgumentBinding::Resolved { arguments, .. } = binding
        && arguments.iter().any(|bound| bound.slot != bound.written_position)
    {
        return Err("unsupported Body IR reordered generator ArgumentBinding".into());
    }
    let mut types = Vec::with_capacity(body.params.len());
    for parameter in &body.params {
        types.push(call_parameter_type(builder, body, parameter)?);
    }
    if has_defaulted_slots(binding) {
        let call = DefaultCall {
            body,
            parameter_offset: 0,
            owned: false,
        };
        return default_arguments(module, builder, binding, elements, &types, &call, span);
    }
    let policy = SlotPolicy {
        accesses: parameter_accesses(&body.params, 0)?,
        ..SlotPolicy::default()
    };
    ordered_slot_operands(builder, binding, elements, &types, span, &policy)
}
