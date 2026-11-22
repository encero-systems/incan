//! Lower static methods, including SDK static methods and their receivers and forwarders.

use incan_mir_plan_native::{
    Callee, CalleeKind, ExternalFunction, FieldProjection, Function, Local, Operand, Parameter, Place, PlanType,
    Projection, RvalueKind, SourceSpan, TerminatorKind, Unwind, assign as plan_assign, block as plan_block,
    place as plan_place,
};
use incan_semantics_core::body_ir::{
    ArgumentBinding, ArgumentElement, BodyIrModule, GlobalPlace, LoafCallable, MethodTarget, OwnershipFact,
    Place as SourcePlace, PlaceOperand,
};
use incan_semantics_core::{IncanInputAccess, IncanType};

use crate::call_arguments::{SlotPolicy, call_parameter_type, has_defaulted_slots, ordered_slot_operands};
use crate::callable_calls::{emit_selected_body_call, lower_callable_result};
use crate::classes::frame_type;
use crate::closure_lifting::synthesize;
use crate::closures::return_representation;
use crate::computed_statics::{guarded_static_call, initialize_module_statics, is_computed_static, static_owner};
use crate::decls::static_slot;
use crate::defaults::library_default_arguments;
use crate::frame::{Builder, read_place, value_type};
use crate::layout_types::{native_source_type, shared_receiver_type};
use crate::list_methods::call_destination;
use crate::method_calls::{
    instantiated_method_body, method_ordinary_operands, owner_method_index, source_method_symbol,
};
use crate::models::field_path;
use crate::nominal_instances::instance_arguments;
use crate::static_storage::computed_static_type;
use crate::type_parameters::substitute;
use crate::types::static_method_result_type;

/// The checked call one static method lowers: its target, receiver read, storage and written arguments.
pub struct StaticCall<'a> {
    pub target: &'a MethodTarget,
    pub read: &'a PlaceOperand,
    pub global: &'a GlobalPlace,
    pub arguments: &'a [ArgumentElement],
    pub destination: &'a Option<SourcePlace>,
}

/// Resolve a checked type against the frame's tables.
fn frame_carrier(builder: &Builder, ty: &IncanType) -> Result<PlanType, String> {
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

/// Call a checked SDK method under the actual static guard after evaluating its bound ordinary arguments.
pub fn lower_static_sdk_method(
    module: &BodyIrModule,
    builder: &mut Builder,
    delegation: &LoafCallable,
    call: &StaticCall<'_>,
    span: &SourceSpan,
) -> Result<(), String> {
    let StaticCall {
        target,
        read,
        global,
        arguments,
        destination,
    } = *call;
    if let Some(frame) = delegation.declaration_frame()
        && frame
            .params
            .iter()
            .any(|parameter| parameter.input_access != IncanInputAccess::Inferred)
    {
        return Err("unsupported Body IR explicit input through a computed static SDK method".into());
    }
    if !read.place.projection.is_empty() || arguments.is_empty() {
        return Err("standard library static method argument count or projected receiver is unsupported".into());
    }
    if target.loaf_trait.is_some() || !target.type_args.is_empty() || !delegation.native_type_arguments.is_empty() {
        return Err("standard library static methods cannot supply explicit trait or method type parameters".into());
    }
    match &target.binding {
        ArgumentBinding::Resolved {
            arguments: bound,
            defaulted_slots,
            ..
        } => {
            if bound.len() + 1 != arguments.len()
                || bound.len() + defaulted_slots.len() + 1 != delegation.parameters.len()
            {
                return Err("standard library static method argument binding is unsupported".into());
            }
        }
        _ => return Err("standard library static method arguments have no checked binding".into()),
    }
    let slot = static_slot(&builder.program.statics, global)?;
    if !is_computed_static(&builder.program.statics[slot]) {
        return Err("this standard library static method requires a computed static receiver".into());
    }
    let owner = frame_carrier(builder, &global.ty)?;
    let PlanType::Model(index, _) = &owner else {
        return Err("this standard library static method requires a model receiver".into());
    };
    let index = *index;
    let names: Vec<String> = delegation.type_parameters.clone();
    let declaration = &builder.nominals()[index as usize];
    let closed = instance_arguments(declaration)?;
    if closed.len() != names.len() {
        return Err(format!(
            "standard library static method owner type parameters differ for {} on {}: declared={names:?}, retained={closed:?}",
            delegation.canonical.declaration_name, declaration.name
        ));
    }
    let receiver = static_sdk_receiver(builder, &delegation.parameters[0], &names, &closed, &owner)?;
    match &receiver {
        PlanType::ModelRef(receiver_index, _) => {
            if *receiver_index != index || read.fact != OwnershipFact::Borrow {
                return Err("the standard library static method requires a shared receiver".into());
            }
        }
        PlanType::ModelMutRef(receiver_index, _) => {
            if *receiver_index != index || read.fact != OwnershipFact::MutBorrow {
                return Err("the standard library static method requires a mutable receiver".into());
            }
        }
        _ => return Err("the standard library static method receiver signature is unsupported".into()),
    }
    let returned = substitute(&delegation.return_type, &names, &closed)?;
    let result = frame_carrier(builder, &returned)?;
    if !static_method_result_type(&result) {
        return Err("the standard library static method result is borrowed or has no proven type".into());
    }
    let Some(path) = delegation.rust_path() else {
        return Err("the standard library static method has no checked native identity".into());
    };
    let mut type_arguments = Vec::with_capacity(closed.len());
    for argument in &closed {
        type_arguments.push(frame_carrier(builder, argument)?);
    }
    let (parameters, ordinary_types) = static_sdk_parameter_types(builder, delegation, receiver, &names, &closed)?;
    let operands = if has_defaulted_slots(&target.binding) {
        library_default_arguments(
            module,
            builder,
            delegation,
            &target.binding,
            &arguments[1..],
            &ordinary_types,
            span,
            1,
        )?
    } else {
        let policy = SlotPolicy {
            offset: 1,
            ..SlotPolicy::default()
        };
        ordered_slot_operands(builder, &target.binding, arguments, &ordinary_types, span, &policy)?
    };
    let name = format!(
        "{}__incan_static_sdk_{slot}_{}",
        builder.name,
        builder.synthesized.len()
    );
    let forwarder = static_sdk_forwarder(&name, &path, &type_arguments, &parameters, &result, span);
    builder.synthesized.push(forwarder);
    builder.externals.push(ExternalFunction {
        path,
        type_arguments,
        parameters,
        return_type: result.clone(),
        span: span.clone(),
    });
    let owner_module = static_owner(&builder.program.statics[slot]);
    initialize_module_statics(builder, &owner_module, span);
    let output = call_destination(builder, destination, result.clone(), span)?;
    let callee = guarded_static_call(builder, slot, name, read.fact == OwnershipFact::MutBorrow)?;
    lower_callable_result(builder, callee, operands, output, result, span)
}

/// Close ordinary static-method parameters and retain the declaration frame's passing representation.
fn static_sdk_parameter_types(
    builder: &Builder,
    delegation: &LoafCallable,
    receiver: PlanType,
    names: &[String],
    closed: &[IncanType],
) -> Result<(Vec<PlanType>, Vec<PlanType>), String> {
    let mut parameters = vec![receiver];
    let mut ordinary_types = Vec::new();
    for position in 1..delegation.parameters.len() {
        let declared = substitute(&delegation.parameters[position], names, closed)?;
        let mut parameter = frame_carrier(builder, &declared)?;
        if let Some(frame) = delegation.declaration_frame() {
            parameter = call_parameter_type(builder, frame, &frame.params[position])?;
        }
        parameters.push(parameter.clone());
        ordinary_types.push(parameter);
    }
    Ok((parameters, ordinary_types))
}

/// Close an SDK receiver's referent explicitly and require the exact static carrier before preserving its borrow mode.
fn static_sdk_receiver(
    builder: &Builder,
    declared: &IncanType,
    names: &[String],
    arguments: &[IncanType],
    owner: &PlanType,
) -> Result<PlanType, String> {
    let (mutable, referent) = match declared {
        IncanType::Ref(inner) => (false, substitute(inner, names, arguments)?),
        IncanType::RefMut(inner) => (true, substitute(inner, names, arguments)?),
        _ => return Err("a standard library static method cannot consume its receiver".into()),
    };
    let concrete = frame_carrier(builder, &referent)?;
    if concrete != *owner {
        return Err("the standard library static method receiver storage differs from its checked type".into());
    }
    match concrete {
        PlanType::Model(index, name) => Ok(if mutable {
            PlanType::ModelMutRef(index, name)
        } else {
            PlanType::ModelRef(index, name)
        }),
        _ => Err("the standard library static method receiver requires a nominal type".into()),
    }
}

/// Forward the checked receiver and ordinary values while the enclosing static accessor owns the guard.
fn static_sdk_forwarder(
    name: &str,
    path: &str,
    type_arguments: &[PlanType],
    inputs: &[PlanType],
    result: &PlanType,
    span: &SourceSpan,
) -> Function {
    let callee = if type_arguments.is_empty() {
        CalleeKind::External(path.to_string())
    } else {
        CalleeKind::Instantiated(path.to_string(), type_arguments.to_vec())
    };
    let mut parameters = Vec::with_capacity(inputs.len());
    let mut locals = vec![local(result, span)];
    let mut arguments = Vec::with_capacity(inputs.len());
    for (index, input) in inputs.iter().enumerate() {
        parameters.push(Parameter {
            name: format!("__incan_argument_{index}"),
            ty: input.clone(),
            span: span.clone(),
        });
        locals.push(local(input, span));
        let moved = index != 0 || matches!(input, PlanType::ModelMutRef(..));
        arguments.push(read_place(plan_place(index as i64 + 1, span), moved, span));
    }
    let invocation = forwarding_call(callee, arguments, span);
    Function::new(
        name.to_string(),
        parameters,
        result.clone(),
        locals,
        vec![
            plan_block(Vec::new(), invocation, span),
            plan_block(Vec::new(), TerminatorKind::Return, span),
        ],
        span.clone(),
    )
}

/// One local of a synthesized forwarder.
fn local(ty: &PlanType, span: &SourceSpan) -> Local {
    Local {
        ty: ty.clone(),
        span: span.clone(),
    }
}

/// A forwarder's single call, writing its result to the return slot and continuing at the return block.
fn forwarding_call(callee: CalleeKind, arguments: Vec<Operand>, span: &SourceSpan) -> TerminatorKind {
    TerminatorKind::Call(
        Callee {
            kind: callee,
            span: span.clone(),
        },
        arguments,
        plan_place(0, span),
        1,
        Unwind::Continue,
    )
}

/// Validate canonical storage, selected receiver passing and a nonescaping result before calling under its guard.
pub fn lower_static_method_body(
    module: &BodyIrModule,
    builder: &mut Builder,
    call: &StaticCall<'_>,
    span: &SourceSpan,
) -> Result<(), String> {
    let StaticCall {
        target,
        read,
        global,
        arguments,
        destination,
    } = *call;
    let slot = static_slot(&builder.program.statics, global)?;
    if !is_computed_static(&builder.program.statics[slot]) {
        return Err("unsupported Body IR noncomputed global method receiver".into());
    }
    let owner = frame_carrier(builder, &global.ty)?;
    if !matches!(owner, PlanType::Model(..)) {
        return Err("unsupported Body IR nonmodel global method receiver".into());
    }
    let selected = computed_static_type(builder, &read.place, global)?;
    let index = owner_method_index(module, target, &selected, builder)?;
    let body = instantiated_method_body(module, builder, target, index, arguments, span)?;
    let receiver = frame_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &body.locals[0],
        &builder.intrinsics,
        "",
    )?;
    match &receiver {
        PlanType::ModelMutRef(..) => {
            if read.fact != OwnershipFact::MutBorrow {
                return Err("unsupported Body IR global method without checked mutable receiver".into());
            }
        }
        PlanType::ModelRef(..) | PlanType::EnumRef(..) => {
            if read.fact != OwnershipFact::Borrow {
                return Err("unsupported Body IR global method without checked shared receiver".into());
            }
        }
        _ => return Err("unsupported Body IR global method receiver frame".into()),
    }
    let declared_result = frame_carrier(builder, &body.return_type)?;
    let result = return_representation(&body, declared_result)?;
    if !static_method_result_type(&result) {
        return Err("unsupported Body IR global method borrowed or unproven result".into());
    }
    let operands = method_ordinary_operands(module, builder, target, arguments, &body, span)?;
    let owner_module = static_owner(&builder.program.statics[slot]);
    initialize_module_statics(builder, &owner_module, span);
    let mut symbol = source_method_symbol(module, builder, target, &body, span)?;
    if !read.place.projection.is_empty() {
        let fields = field_path(
            builder.tables(),
            owner.clone(),
            &read.place.projection,
            read.place.projection.len(),
        )?;
        let mut inputs = vec![receiver];
        for operand in &operands {
            inputs.push(value_type(builder, operand)?);
        }
        let root_reference = if read.fact == OwnershipFact::MutBorrow {
            match &owner {
                PlanType::Model(index, name) => PlanType::ModelMutRef(*index, name.clone()),
                _ => return Err("unsupported Body IR static projection mutable root".into()),
            }
        } else {
            shared_receiver_type(owner)?
        };
        let wrapper = format!(
            "{}__incan_static_projection_{slot}_{}",
            builder.name,
            builder.synthesized.len()
        );
        let forwarder = static_projection_forwarder(&wrapper, &symbol, root_reference, fields, &inputs, &result, span);
        synthesize(builder, forwarder);
        symbol = wrapper;
    }
    let callee = guarded_static_call(builder, slot, symbol, read.fact == OwnershipFact::MutBorrow)?;
    emit_selected_body_call(module, builder, &body, callee, operands, destination, span)
}

/// Borrow the retained field path under the root's guard and call its selected method without a snapshot.
fn static_projection_forwarder(
    name: &str,
    method: &str,
    root_reference: PlanType,
    fields: Vec<FieldProjection>,
    inputs: &[PlanType],
    result: &PlanType,
    span: &SourceSpan,
) -> Function {
    let mutable = matches!(root_reference, PlanType::ModelMutRef(..));
    let mut parameters = vec![Parameter {
        name: "__incan_root".into(),
        ty: root_reference.clone(),
        span: span.clone(),
    }];
    let mut locals = vec![local(result, span), local(&root_reference, span)];
    let mut arguments = Vec::with_capacity(inputs.len());
    for (index, input) in inputs.iter().enumerate().skip(1) {
        parameters.push(Parameter {
            name: format!("__incan_argument_{index}"),
            ty: input.clone(),
            span: span.clone(),
        });
        locals.push(local(input, span));
        arguments.push(read_place(plan_place(index as i64 + 1, span), true, span));
    }
    let receiver = plan_place(locals.len() as i64, span);
    locals.push(local(&inputs[0], span));
    let field = Place {
        local: 1,
        projection: Projection::DerefFields(fields),
        span: span.clone(),
    };
    let borrow = if mutable {
        RvalueKind::MutBorrow(field)
    } else {
        RvalueKind::Borrow(field)
    };
    let mut operands = vec![read_place(receiver.clone(), true, span)];
    operands.extend(arguments);
    let invocation = forwarding_call(CalleeKind::Planned(method.to_string()), operands, span);
    Function::new(
        name.to_string(),
        parameters,
        result.clone(),
        locals,
        vec![
            plan_block(vec![plan_assign(receiver, borrow, span)], invocation, span),
            plan_block(Vec::new(), TerminatorKind::Return, span),
        ],
        span.clone(),
    )
}
