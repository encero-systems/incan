//! Function items: a named read of a checked function, authenticated against its declaration and closed signature.
//! These are the function-item steps of the Incan `closures` module.

use incan_mir_plan_native::{Callee, CalleeKind, ExternalFunction, PlanType, SourceSpan};
use incan_semantics_core::body_ir::{
    Body, BodyIrModule, CallableParam, CallableTarget, Callee as SourceCallee, DecoratedBinding,
    DecoratedInitialization, LoafCallable, NamedCallableTarget, Rvalue, StatementKind,
};
use incan_semantics_core::{IncanInputAccess, IncanType};

use crate::async_::{boxed_future_constructor, evaluator_name};
use crate::body_index::BodyIndex;
use crate::call_arguments::call_parameter_type;
use crate::classes::{parameter_local_index, receiver_mode};
use crate::closures::{packed_input, return_representation};
use crate::frame::Builder;
use crate::generators::{generator_type, is_generator_type};
use crate::layout_types::native_source_type;
use crate::type_parameters::{instantiate, instantiate_parameter, request_instance, substitute};

/// A function item: its plan symbol, its `FunctionItem` carrier and its checked declaration name.
pub type FunctionItem = (String, PlanType, String);

/// Retain a function item's exact signature and declaration name from its checked named-read or original identity.
pub fn source_function_item(
    module: &BodyIrModule,
    builder: &mut Builder,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<Option<FunctionItem>, String> {
    let Rvalue::Closure {
        params,
        captured_operands,
        body,
    } = value
    else {
        return Ok(None);
    };
    if !body.function_item && body.original_callable.is_none() {
        return Ok(None);
    }
    let mut inputs = Vec::with_capacity(params.len());
    for parameter in params {
        inputs.push(instantiate_parameter(
            parameter,
            &builder.type_parameters,
            &builder.type_arguments,
        )?);
    }
    if let Some(identity) = &body.original_callable {
        if !captured_operands.is_empty() || !body.capture_locals.is_empty() || !body.stmts.is_empty() {
            return Err("invalid Body IR original callable computation".into());
        }
        let Some(binding) = module
            .decorated_bindings
            .iter()
            .find(|binding| binding.original_body.canonical == body.original_callable)
        else {
            return Err("unsupported Body IR unretained original callable".into());
        };
        let (name, ty) = original_function_item(module, builder, &inputs, binding, span)?;
        return Ok(Some((name, ty, identity.declaration_name.to_string())));
    }
    if !captured_operands.is_empty() || !body.capture_locals.is_empty() || body.stmts.len() != 1 {
        return Err("invalid Body IR function-item forwarding computation".into());
    }
    Ok(Some(checked_function_item(
        module,
        builder,
        &inputs,
        &body.stmts[0].kind,
        span,
    )?))
}

/// Require the same checked declaration and signature as the named read's retained forwarding call.
fn checked_function_item(
    module: &BodyIrModule,
    builder: &mut Builder,
    parameters: &[CallableParam],
    statement: &StatementKind,
    span: &SourceSpan,
) -> Result<FunctionItem, String> {
    let StatementKind::Call { callee, .. } = statement else {
        return Err("invalid Body IR function-item body".into());
    };
    let SourceCallee::Function(target) = callee else {
        return Err("invalid Body IR nonfunction item forwarding target".into());
    };
    let CallableTarget::Named(named) = target else {
        return Err("invalid Body IR indirect function-item forwarding target".into());
    };
    named_function_item(module, builder, parameters, named, span)
}

/// Attach the canonical declaration name to an authenticated function item without reading its signature for a name.
fn named_function_item(
    module: &BodyIrModule,
    builder: &mut Builder,
    parameters: &[CallableParam],
    target: &NamedCallableTarget,
    span: &SourceSpan,
) -> Result<FunctionItem, String> {
    let (name, ty) = function_item_identity(module, builder, parameters, target, span)?;
    match &target.canonical {
        Some(identity) => Ok((name, ty, identity.declaration_name.to_string())),
        None => Err("unsupported Body IR unnamed function item".into()),
    }
}

/// Retain a function item's signature component through its checked frame's ordinary layout context.
///
/// Its inputs and result use the same nominal, scalar, borrowed and collection storage as its own frame. A second
/// signature whitelist would reject admitted borrowed scalar inputs before the native callable can be verified.
fn item_component(builder: &Builder, ty: &IncanType) -> Result<PlanType, String> {
    native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        ty,
        &builder.intrinsics,
        "",
    )
}

/// Retain an item's explicit input loan against its full nominal context; other inputs use owned components.
fn item_input(builder: &Builder, ty: &IncanType) -> Result<PlanType, String> {
    item_component(builder, ty)
}

/// A function item's result: a lazy generator, as `callable_result` admits one, or any item component.
fn item_result(builder: &Builder, ty: &IncanType) -> Result<PlanType, String> {
    if let Some(Ok(result)) = generator_type(ty)
        && is_generator_type(&result)
    {
        return Ok(result);
    }
    item_component(builder, ty)
}

/// A diagnostic source name becomes a plan item only after its canonical, instance and physical identities agree.
fn function_item_identity(
    module: &BodyIrModule,
    builder: &mut Builder,
    parameters: &[CallableParam],
    target: &NamedCallableTarget,
    span: &SourceSpan,
) -> Result<(String, PlanType), String> {
    let Some(identity) = &target.canonical else {
        return Err("unsupported Body IR unproven function item".into());
    };
    if let Some(body) = module.body_for_canonical_target(identity) {
        if target.direct_call_id.as_ref() != Some(&body.direct_call_id) || target.name != body.name {
            return Err("invalid Body IR function-item identity".into());
        }
        return instantiated_item_signature(module, builder, parameters, body, target, span);
    }
    if let Some(declaration) = module
        .loaf_callables
        .iter()
        .find(|declaration| declaration.canonical == *identity)
    {
        if !target.type_args.is_empty() {
            return Err("unsupported Body IR generic Loaf function item".into());
        }
        return loaf_item_signature(builder, parameters, declaration, span);
    }
    let index = imported_item_body(module, &builder.program.body_indices, target)?;
    instantiated_item_signature(module, builder, parameters, &module.bodies[index], target, span)
}

/// Close and schedule a generic item from its checked ordered arguments before admitting its callable signature.
fn instantiated_item_signature(
    module: &BodyIrModule,
    builder: &mut Builder,
    parameters: &[CallableParam],
    body: &Body,
    target: &NamedCallableTarget,
    span: &SourceSpan,
) -> Result<(String, PlanType), String> {
    if target.type_args.is_empty() {
        return item_signature(module, builder, parameters, body, span, None);
    }
    let mut arguments = Vec::with_capacity(target.type_args.len());
    for argument in &target.type_args {
        arguments.push(substitute(argument, &builder.type_parameters, &builder.type_arguments)?);
    }
    let closed = instantiate(body, &arguments, &[])?;
    request_instance(&mut builder.instances, body, arguments, Vec::new(), false);
    item_signature(module, builder, parameters, &closed, span, None)
}

/// Read a compiled function value's signature from its Loaf declaration, without requiring its implementation.
fn loaf_item_signature(
    builder: &mut Builder,
    parameters: &[CallableParam],
    declaration: &LoafCallable,
    span: &SourceSpan,
) -> Result<(String, PlanType), String> {
    if !declaration.type_parameters.is_empty() || !declaration.native_type_arguments.is_empty() {
        return Err("unsupported Body IR generic Loaf function item".into());
    }
    if parameters.len() != declaration.parameters.len() {
        return Err("invalid Body IR Loaf function-item parameter count".into());
    }
    let frame = declaration.declaration_frame();
    let mut signature = Vec::with_capacity(parameters.len() + 1);
    for (index, parameter) in parameters.iter().enumerate() {
        if parameter.ty != declaration.parameters[index] {
            return Err("invalid Body IR Loaf function-item parameter contract".into());
        }
        let component = item_input(builder, &declaration.parameters[index])?;
        let mut input = packed_input(
            component,
            parameter.kind,
            parameter.mutable,
            parameter.native_owned,
            parameter.input_access,
        )?;
        match frame {
            Some(frame) => {
                if parameter.input_access != frame.params[index].input_access {
                    return Err("invalid Body IR Loaf function-item input access contract".into());
                }
                input = call_parameter_type(builder, frame, &frame.params[index])?;
            }
            None => {
                if parameter.input_access != IncanInputAccess::Inferred {
                    return Err(
                        "unsupported native Loaf function-item explicit input access without a provider frame".into(),
                    );
                }
            }
        }
        signature.push(input);
    }
    let mut result = item_result(builder, &declaration.return_type)?;
    if let Some(frame) = frame {
        result = return_representation(frame, result)?;
    }
    signature.push(result.clone());
    let name = declaration.item.clone();
    builder.externals.push(ExternalFunction {
        path: name.clone(),
        type_arguments: Vec::new(),
        parameters: signature[..signature.len() - 1].to_vec(),
        return_type: result,
        span: span.clone(),
    });
    Ok((name.clone(), PlanType::FunctionItem(name, signature)))
}

/// Use the registered external declaration for a Loaf item and the local declaration for a source item.
pub fn function_item_callee(builder: &Builder, name: &str, span: &SourceSpan) -> Callee {
    let external = builder
        .externals
        .iter()
        .any(|declaration| declaration.path == name && declaration.type_arguments.is_empty());
    let kind = if external {
        CalleeKind::External(name.to_string())
    } else {
        CalleeKind::Planned(name.to_string())
    };
    Callee {
        kind,
        span: span.clone(),
    }
}

/// Find the one retained body, in another module of the project graph, that an imported function item names.
///
/// Its exact canonical identity selects the body, as for a direct call to it. A published package fragment carries no
/// source-local call id; a call id that is present must agree with the body.
fn imported_item_body(
    module: &BodyIrModule,
    bodies: &BodyIndex,
    target: &NamedCallableTarget,
) -> Result<usize, String> {
    let candidates = match &target.canonical {
        Some(identity) => bodies.candidates(identity),
        None => Vec::new(),
    };
    if candidates.is_empty() {
        return Err("unsupported Body IR imported function item without a retained body".into());
    }
    if candidates.len() != 1 {
        return Err("unsupported Body IR ambiguous imported function item".into());
    }
    let found = candidates[0];
    if let Some(call_id) = &target.direct_call_id
        && *call_id != module.bodies[found].direct_call_id
    {
        return Err("invalid Body IR function-item identity".into());
    }
    Ok(found)
}

/// Select a checked function's callable item; deferred callables name their lazy constructor, never their producer.
///
/// An async item reifies a synchronous future constructor over the same checked inputs.
fn item_signature(
    module: &BodyIrModule,
    builder: &mut Builder,
    parameters: &[CallableParam],
    body: &Body,
    span: &SourceSpan,
    selected_name: Option<String>,
) -> Result<(String, PlanType), String> {
    if parameters.len() != body.params.len() {
        return Err("invalid Body IR function-item parameter count".into());
    }
    let mut signature = Vec::with_capacity(parameters.len() + 1);
    for (index, parameter) in parameters.iter().enumerate() {
        let declared = &body.params[index];
        if parameter.input_access != declared.input_access {
            return Err("invalid Body IR function-item input access contract".into());
        }
        if parameter.kind != declared.kind {
            return Err("unsupported Body IR function-item parameter role".into());
        }
        validate_item_receiver(body, index, parameter)?;
        let component = item_input(builder, &parameter.ty)?;
        let input = packed_input(
            component.clone(),
            parameter.kind,
            parameter.mutable,
            parameter.native_owned,
            parameter.input_access,
        )?;
        if body.is_generator() && parameter.input_access != IncanInputAccess::Inferred {
            return Err("unsupported Body IR explicit input access in a deferred producer".into());
        }
        if body.is_generator() && input != component {
            return Err("unsupported Body IR generator function item with borrowed parameters".into());
        }
        signature.push(input);
    }
    let result = item_result(builder, &body.return_type)?;
    let name = selected_name.unwrap_or_else(|| evaluator_name(module, body));
    if body.is_async {
        let constructor = format!("{name}__incan_future");
        let future = boxed_future_constructor(
            constructor.clone(),
            CalleeKind::Planned(name),
            signature.clone(),
            result.clone(),
            span.clone(),
        )?;
        builder.synthesized.push(future);
        signature.push(PlanType::Future(vec![result]));
        return Ok((constructor.clone(), PlanType::FunctionItem(constructor, signature)));
    }
    signature.push(result);
    // Generator entrypoints retain their ordinary constructor symbol beside the separately named producer.
    Ok((name.clone(), PlanType::FunctionItem(name, signature)))
}

/// A receiver parameter keeps its checked passing contract; any other parameter keeps its exact checked type.
fn validate_item_receiver(body: &Body, index: usize, parameter: &CallableParam) -> Result<(), String> {
    let declared = &body.params[index].ty;
    match receiver_mode(&body.locals[parameter_local_index(body, index)].origin) {
        Some(mutable) => match &parameter.ty {
            IncanType::Ref(inner) => {
                if mutable || inner.as_ref() != declared {
                    return Err("unsupported Body IR original callable shared receiver contract".into());
                }
            }
            IncanType::RefMut(inner) => {
                if !mutable || inner.as_ref() != declared {
                    return Err("unsupported Body IR original callable mutable receiver contract".into());
                }
            }
            _ => return Err("unsupported Body IR original callable receiver passing contract".into()),
        },
        None => {
            if parameter.ty != *declared {
                return Err("unsupported Body IR function-item parameter contract".into());
            }
        }
    }
    Ok(())
}

/// Select the original through its checked initializer policy, never through operand inference.
fn original_function_item(
    module: &BodyIrModule,
    builder: &mut Builder,
    parameters: &[CallableParam],
    binding: &DecoratedBinding,
    span: &SourceSpan,
) -> Result<(String, PlanType), String> {
    if binding.initialization == DecoratedInitialization::PerInstantiation {
        return initialized_original_function_item(module, builder, parameters, binding, span);
    }
    if binding.initialization != DecoratedInitialization::ModuleStorage || binding.initializer.is_some() {
        return Err("unsupported Body IR module decorator initialization policy".into());
    }
    let body = &binding.original_body;
    if !body.type_parameters.is_empty() {
        return Err("unsupported Body IR module decorator with type parameters".into());
    }
    let original_name = format!("{}__incan_original", evaluator_name(module, body));
    item_signature(module, builder, parameters, body, span, Some(original_name))
}

/// Select a cached instantiation's original only inside its authenticated closed initializer frame.
fn initialized_original_function_item(
    module: &BodyIrModule,
    builder: &mut Builder,
    parameters: &[CallableParam],
    binding: &DecoratedBinding,
    span: &SourceSpan,
) -> Result<(String, PlanType), String> {
    let original = &binding.original_body;
    if binding.initializer.is_none() || original.type_parameters.is_empty() {
        return Err("unsupported Body IR cached decorator without a generic initializer".into());
    }
    if builder.initializing_decorator != original.canonical || builder.type_parameters != original.type_parameters {
        return Err("unsupported Body IR original callable outside its closed decorator initializer".into());
    }
    let arguments = builder.type_arguments.clone();
    let closed = instantiate(original, &arguments, &[])?;
    let original_name = format!("{}__incan_original", evaluator_name(module, &closed));
    item_signature(module, builder, parameters, &closed, span, Some(original_name))
}
