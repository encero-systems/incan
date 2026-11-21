//! Generated `Validate` constructors: construct one checked model, then return its validator's `Result`. These are
//! the validated-constructor steps of the Incan `callable_calls` module.

use incan_mir_plan_native::{CalleeKind, Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign};
use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::body_ir::{
    ArgumentBinding, ArgumentElement, Body, BodyIrModule, BoundArgument, CallableParamDefault, NamedCallableTarget,
    Place as SourcePlace, ValidatedConstructor,
};

use crate::callable_calls::emit_selected_body_call;
use crate::calls::instantiated_external_call;
use crate::classes::{frame_type, function_name};
use crate::closures::return_representation;
use crate::defaults::evaluate_default;
use crate::frame::{Builder, read_place, temporary, track_rvalue_moves};
use crate::layout_types::{native_source_type, stored_field_type};
use crate::list_methods::call_destination;
use crate::model_construction::model_field_operand;
use crate::type_parameters::substitute;

/// Construct one checked `Validate` model, then return its exact validator body's `Result`.
pub fn lower_validated_constructor(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &NamedCallableTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<bool, String> {
    let Some((owner, model_type)) = validated_model_owner(builder, target)? else {
        return Ok(false);
    };
    let Some(contract) = builder.nominals()[owner].validated_constructor.clone() else {
        return Err("invalid Body IR validated constructor without a retained contract".into());
    };
    let required = validated_required_operands(builder, owner, &contract, &target.binding, arguments, span)?;
    let fields = validated_model_fields(module, builder, owner, &contract, required, span)?;
    let value = temporary(builder, model_type, span);
    let construction = RvalueKind::Model(owner as i64, fields);
    track_rvalue_moves(builder, &construction);
    builder.emit(plan_assign(value.clone(), construction, span));
    builder.owned.push(value.local);
    match validated_method_body(module, builder, owner, &contract.validator)? {
        Some(body) => {
            let receiver_type = frame_type(
                builder.constructors(),
                builder.nominals(),
                builder.enums(),
                &body.locals[0],
                &builder.intrinsics,
                "",
            )?;
            require_validated_receiver(owner, &receiver_type)?;
            let receiver = temporary(builder, receiver_type, span);
            builder.emit(plan_assign(receiver.clone(), RvalueKind::Borrow(value), span));
            let callee = CalleeKind::Planned(function_name(module, &body));
            let operands = vec![read_place(receiver, true, span)];
            emit_selected_body_call(module, builder, &body, callee, operands, destination, span)?;
        }
        None => lower_compiled_validator(module, builder, owner, &contract.validator, value, destination, span)?,
    }
    Ok(true)
}

/// Resolve a generated constructor only through its checked receiver layout and retained exact identity.
fn validated_model_owner(builder: &Builder, target: &NamedCallableTarget) -> Result<Option<(usize, PlanType)>, String> {
    let Some(identity) = &target.canonical else {
        return Ok(None);
    };
    if !builder.constructors().validated_constructors.contains(identity) {
        return Ok(None);
    }
    if !target.type_args.is_empty() {
        return Err("unsupported Body IR type arguments on a validated constructor".into());
    }
    let Some(receiver) = &target.receiver_type else {
        return Err("invalid Body IR validated constructor without a receiver type".into());
    };
    let closed = substitute(receiver, &builder.type_parameters, &builder.type_arguments)?;
    let model_type = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &closed,
        &builder.intrinsics,
        "",
    )?;
    let PlanType::Model(owner, _) = &model_type else {
        return Err("invalid Body IR validated constructor receiver type".into());
    };
    let owner = *owner as usize;
    match &builder.nominals()[owner].validated_constructor {
        Some(contract) => {
            if contract.constructor != *identity {
                return Err("invalid Body IR validated constructor owner".into());
            }
        }
        None => return Err("invalid Body IR validated constructor receiver".into()),
    }
    Ok(Some((owner, model_type)))
}

/// Bind the generated constructor's written arguments to its required canonical field slots.
fn validated_required_operands(
    builder: &mut Builder,
    owner: usize,
    contract: &ValidatedConstructor,
    binding: &ArgumentBinding,
    elements: &[ArgumentElement],
    span: &SourceSpan,
) -> Result<Vec<Operand>, String> {
    match binding {
        ArgumentBinding::Resolved {
            arguments: bindings,
            defaulted_slots,
        }
        | ArgumentBinding::ResolvedDeclarationOrder {
            arguments: bindings,
            defaulted_slots,
        } => {
            if !defaulted_slots.is_empty() {
                return Err("invalid Body IR defaults on validated constructor parameters".into());
            }
            bound_validated_operands(builder, owner, contract, bindings, elements, span)
        }
        ArgumentBinding::UnresolvedPositional => {
            if elements.len() != contract.required_fields.len() {
                return Err("invalid Body IR validated constructor arity".into());
            }
            let mut operands = Vec::with_capacity(elements.len());
            for (position, element) in elements.iter().enumerate() {
                let Some(value) = element.as_one() else {
                    return Err("unsupported Body IR spread validated constructor argument".into());
                };
                operands.push(model_field_operand(
                    builder,
                    owner,
                    contract.required_fields[position],
                    value,
                    span,
                )?);
            }
            Ok(operands)
        }
    }
}

/// Order a resolved written-argument map by generated constructor parameter slot.
fn bound_validated_operands(
    builder: &mut Builder,
    owner: usize,
    contract: &ValidatedConstructor,
    bindings: &[BoundArgument],
    elements: &[ArgumentElement],
    span: &SourceSpan,
) -> Result<Vec<Operand>, String> {
    let count = contract.required_fields.len();
    if bindings.len() != count || elements.len() != count {
        return Err("invalid Body IR validated constructor binding".into());
    }
    let mut operands = Vec::with_capacity(count);
    for parameter in 0..count {
        let mut written = None;
        for (position, bound) in bindings.iter().enumerate() {
            if bound.slot == parameter {
                if written.is_some() {
                    return Err("invalid Body IR duplicate validated constructor binding".into());
                }
                written = Some(position);
            }
        }
        let Some(written) = written else {
            return Err("invalid Body IR incomplete validated constructor binding".into());
        };
        let Some(value) = elements[written].as_one() else {
            return Err("unsupported Body IR spread validated constructor argument".into());
        };
        operands.push(model_field_operand(
            builder,
            owner,
            contract.required_fields[parameter],
            value,
            span,
        )?);
    }
    Ok(operands)
}

/// Fill required fields from constructor arguments and every remaining field from its declaration default.
fn validated_model_fields(
    module: &BodyIrModule,
    builder: &mut Builder,
    owner: usize,
    contract: &ValidatedConstructor,
    required: Vec<Operand>,
    span: &SourceSpan,
) -> Result<Vec<Operand>, String> {
    let declaration = builder.nominals()[owner].clone();
    let required_count = required.len();
    let mut required = required.into_iter();
    let mut fields = Vec::with_capacity(declaration.fields.len());
    let mut required_position = 0;
    for slot in 0..declaration.fields.len() {
        if required_position < contract.required_fields.len() && contract.required_fields[required_position] == slot {
            let Some(operand) = required.next() else {
                return Err("invalid Body IR validated constructor required field map".into());
            };
            fields.push(operand);
            required_position += 1;
            continue;
        }
        let Some(defaults) = &declaration.field_default_body else {
            return Err("invalid Body IR validated constructor missing field defaults".into());
        };
        if slot >= defaults.params.len() {
            return Err("invalid Body IR validated constructor default slot".into());
        }
        if !matches!(defaults.params[slot].default, CallableParamDefault::Source(_)) {
            return Err("invalid Body IR validated constructor unavailable field default".into());
        }
        let ty = stored_field_type(
            builder.constructors(),
            builder.nominals(),
            builder.enums(),
            &builder.unions,
            &declaration,
            slot,
            &builder.intrinsics,
        )?;
        fields.push(evaluate_default(module, builder, defaults, slot, &ty, span, true)?);
    }
    if required_position != required_count {
        return Err("invalid Body IR validated constructor required field map".into());
    }
    Ok(fields)
}

/// Select one local validator body, leaving a compiled dependency validator to its retained callable.
fn validated_method_body(
    module: &BodyIrModule,
    builder: &Builder,
    owner: usize,
    identity: &CanonicalSymbolId,
) -> Result<Option<Body>, String> {
    let mut selected = None;
    for index in builder.program.body_indices.candidates(identity) {
        let body = &module.bodies[index];
        if body.canonical.as_ref() != Some(identity)
            || body.params.len() != 1
            || body.locals.is_empty()
            || body.is_async
            || !body.type_parameters.is_empty()
        {
            continue;
        }
        match frame_type(
            builder.constructors(),
            builder.nominals(),
            builder.enums(),
            &body.locals[0],
            &builder.intrinsics,
            "",
        ) {
            Ok(PlanType::ModelRef(candidate, _)) if candidate as usize == owner => {}
            _ => continue,
        }
        if selected.is_some() {
            return Err("invalid Body IR validated constructor with multiple validator bodies".into());
        }
        selected = Some(index);
    }
    Ok(selected.map(|index| module.bodies[index].clone()))
}

/// Require the validator receiver to borrow the exact model layout constructed by the generated method.
fn require_validated_receiver(owner: usize, receiver_type: &PlanType) -> Result<(), String> {
    match receiver_type {
        PlanType::ModelRef(selected, _) => {
            if *selected as usize == owner {
                Ok(())
            } else {
                Err("invalid Body IR Validate receiver owner".into())
            }
        }
        _ => Err("invalid Body IR Validate receiver type".into()),
    }
}

/// Call a dependency model's compiled validator through its producer-authenticated callable record.
fn lower_compiled_validator(
    module: &BodyIrModule,
    builder: &mut Builder,
    owner: usize,
    identity: &CanonicalSymbolId,
    value: Place,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let mut candidates = module
        .loaf_callables
        .iter()
        .filter(|candidate| candidate.canonical == *identity);
    let Some(callable) = candidates.next() else {
        return Err("unsupported Body IR validated constructor without one validator body or callable".into());
    };
    if candidates.next().is_some() {
        return Err("invalid Body IR validated constructor with multiple compiled validators".into());
    }
    if callable.parameters.len() != 1 || !callable.type_parameters.is_empty() {
        return Err("unsupported Body IR compiled Validate signature".into());
    }
    let resolve = |builder: &Builder, ty| {
        native_source_type(
            builder.constructors(),
            builder.nominals(),
            builder.enums(),
            &builder.unions,
            ty,
            &builder.intrinsics,
            "",
        )
    };
    let receiver_type = resolve(builder, &callable.parameters[0])?;
    require_validated_receiver(owner, &receiver_type)?;
    let receiver = temporary(builder, receiver_type.clone(), span);
    builder.emit(plan_assign(receiver.clone(), RvalueKind::Borrow(value), span));
    let mut result = resolve(builder, &callable.return_type)?;
    let Some(frame) = callable.declaration_frame() else {
        return Err("invalid Body IR compiled Validate declaration frame".into());
    };
    result = return_representation(frame, result)?;
    let output = call_destination(builder, destination, result.clone(), span)?;
    let Some(path) = callable.rust_path() else {
        return Err("invalid Body IR compiled Validate native identity".into());
    };
    instantiated_external_call(
        builder,
        &path,
        Vec::new(),
        vec![receiver_type],
        result,
        vec![read_place(receiver, true, span)],
        output,
        span,
    )
}
