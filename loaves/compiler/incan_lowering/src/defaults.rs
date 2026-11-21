//! Defaulted parameter slots: the representations a call slot may own or borrow.

use incan_mir_plan_native::{Constant, Local, Operand, OperandKind, PlanType, SourceSpan};
use incan_semantics_core::IncanInputAccess;
use incan_semantics_core::body_ir::{
    ArgumentBinding, ArgumentElement, Body, BodyIrModule, CallableParamDefault, LoafCallable, Operand as SourceOperand,
};

use crate::call_arguments::call_argument_operand;
use crate::collections::is_collection;
use crate::explicit_arguments::{argument_access, parameter_accesses};
use crate::frame::{Builder, drop_owned, literal};
use crate::layout_types::native_source_type;
use crate::list_iteration::list_root_place;
use crate::lists::is_list;
use crate::numerics::numeric_carrier;
use crate::statements::lower_statement;

/// Admit default representations a call slot owns or borrows: scalars of any numeric carrier, text, borrowed lists and
/// hashed collections, and owned models, enums (including `Option`, `Result` and unions) and tuples.
pub fn default_type(ty: &PlanType) -> Result<(), String> {
    if is_list(ty) || is_collection(ty) || numeric_carrier(ty) {
        return Ok(());
    }
    match ty {
        // Models, Option, Result, source enum and tuple values move out of the default's frame like any owned result.
        PlanType::Bool | PlanType::String | PlanType::Model(..) | PlanType::Enum(..) | PlanType::Tuple(_) => Ok(()),
        _ => Err("unsupported Body IR defaulted parameter type".into()),
    }
}

/// Use the retained constructor frame only when checked field slots are omitted.
pub fn model_default_arguments(
    module: &BodyIrModule,
    builder: &mut Builder,
    index: usize,
    binding: &ArgumentBinding,
    elements: &[ArgumentElement],
    span: &SourceSpan,
) -> Result<Vec<Operand>, String> {
    let program = builder.program.clone();
    let declaration = &program.nominals[index];
    let Some(body) = &declaration.field_default_body else {
        return Err(format!(
            "unsupported Body IR unavailable field defaults on {}",
            declaration.name
        ));
    };
    if body.canonical.as_ref() != Some(&declaration.canonical) || body.params.len() != declaration.fields.len() {
        return Err("invalid Body IR model default frame".into());
    }
    let mut types = Vec::with_capacity(declaration.field_types.len());
    for ty in &declaration.field_types {
        types.push(native_source_type(
            builder.constructors(),
            builder.nominals(),
            builder.enums(),
            &builder.unions,
            ty,
            &builder.intrinsics,
            "",
        )?);
    }
    let call = DefaultCall {
        body,
        parameter_offset: 0,
        owned: true,
    };
    default_arguments(module, builder, binding, elements, &types, &call, span)
}

/// The declaration frame that owns a call's defaults, the slots its receiver takes, and the passing policy.
pub struct DefaultCall<'a> {
    pub body: &'a Body,
    pub parameter_offset: usize,
    pub owned: bool,
}

/// Fill canonical slots, using a fresh declaration-local frame for each deferred default.
///
/// `types` lists the ordinary slots the binding numbers. A method passes a `parameter_offset` of one: its receiver is
/// bound separately, yet stays the first parameter of the declaration whose defaults are evaluated. With `owned`, a
/// collection slot receives an owned value, as a model construction stores it, instead of a call's borrow.
pub fn default_arguments(
    module: &BodyIrModule,
    builder: &mut Builder,
    binding: &ArgumentBinding,
    elements: &[ArgumentElement],
    types: &[PlanType],
    call: &DefaultCall<'_>,
    span: &SourceSpan,
) -> Result<Vec<Operand>, String> {
    if call.body.params.len() != types.len() + call.parameter_offset {
        return Err("invalid Body IR default parameter frame length".into());
    }
    let accesses = parameter_accesses(&call.body.params, call.parameter_offset)?;
    let mut operands = written_operands(builder, binding, elements, types, span, call.owned, &accesses)?;
    for index in omitted_slots(binding, types.len())? {
        let parameter = index + call.parameter_offset;
        operands[index] = evaluate_default(module, builder, call.body, parameter, &types[index], span, call.owned)?;
    }
    Ok(operands)
}

/// Fill a compiled Loaf call's slots from its declaration-owned defaults, evaluating statements in their own frame.
///
/// Defaults follow the record's parameters, so a method's receiver comes first and a `parameter_offset` of one skips
/// it, as for a source method. A retained declaration frame supplies the same passing policy as a source call;
/// synthetic native records use their owned ABI contract.
#[allow(clippy::too_many_arguments)]
pub fn library_default_arguments(
    module: &BodyIrModule,
    builder: &mut Builder,
    declaration: &LoafCallable,
    binding: &ArgumentBinding,
    elements: &[ArgumentElement],
    types: &[PlanType],
    span: &SourceSpan,
    parameter_offset: usize,
) -> Result<Vec<Operand>, String> {
    if let Some(frame) = declaration.declaration_frame() {
        if frame.canonical.as_ref() != Some(&declaration.canonical) {
            return Err("invalid Body IR Loaf default frame identity".into());
        }
        let call = DefaultCall {
            body: frame,
            parameter_offset,
            owned: false,
        };
        return default_arguments(module, builder, binding, elements, types, &call, span);
    }
    let defaults = &declaration.parameter_defaults;
    if defaults.len() != types.len() + parameter_offset {
        return Err("unsupported Body IR stdlib call omitting an argument its record has no default for".into());
    }
    let mut operands = written_operands(builder, binding, elements, types, span, true, &[])?;
    for index in omitted_slots(binding, types.len())? {
        operands[index] = library_default(builder, &defaults[index + parameter_offset], &types[index], span)?;
    }
    Ok(operands)
}

/// Place already evaluated arguments by their retained slots, enforcing retained declaration order and leaving omitted
/// slots as unit placeholders.
fn written_operands(
    builder: &mut Builder,
    binding: &ArgumentBinding,
    elements: &[ArgumentElement],
    types: &[PlanType],
    span: &SourceSpan,
    owned: bool,
    accesses: &[IncanInputAccess],
) -> Result<Vec<Operand>, String> {
    let (bindings, defaults) = match binding {
        ArgumentBinding::Resolved {
            arguments,
            defaulted_slots,
        } => (arguments, defaulted_slots),
        ArgumentBinding::ResolvedDeclarationOrder {
            arguments,
            defaulted_slots,
        } => {
            for index in 1..arguments.len() {
                if arguments[index].slot <= arguments[index - 1].slot {
                    return Err("unsupported Body IR binding outside declaration order".into());
                }
            }
            (arguments, defaulted_slots)
        }
        _ => return Err("unsupported Body IR unresolved ArgumentBinding".into()),
    };
    if bindings.len() != elements.len() || bindings.len() + defaults.len() != types.len() {
        return Err("unsupported Body IR incomplete ArgumentBinding".into());
    }
    let mut operands: Vec<Operand> = (0..types.len()).map(|_| literal(Constant::Unit, span)).collect();
    let mut occupied: Vec<usize> = Vec::new();
    for (bound, element) in bindings.iter().zip(elements) {
        let slot = bound.slot;
        if slot >= types.len() || occupied.contains(&slot) {
            return Err("invalid Body IR argument slot map".into());
        }
        occupied.push(slot);
        let Some(value) = element.as_one() else {
            return Err("unsupported Body IR spread ArgumentElement".into());
        };
        let access = argument_access(accesses, types.len(), slot)?;
        operands[slot] = call_argument_operand(builder, value, &types[slot], span, owned, access)?;
    }
    Ok(operands)
}

/// Retain defaults from either checked evaluation policy, each slot in range and none also written.
fn omitted_slots(binding: &ArgumentBinding, count: usize) -> Result<Vec<usize>, String> {
    let (bindings, defaults) = match binding {
        ArgumentBinding::Resolved {
            arguments,
            defaulted_slots,
        }
        | ArgumentBinding::ResolvedDeclarationOrder {
            arguments,
            defaulted_slots,
        } => (arguments, defaulted_slots),
        _ => return Err("unsupported Body IR unresolved ArgumentBinding".into()),
    };
    let mut occupied: Vec<usize> = bindings.iter().map(|bound| bound.slot).collect();
    let mut omitted = Vec::with_capacity(defaults.len());
    for slot in defaults {
        if *slot >= count || occupied.contains(slot) {
            return Err("invalid Body IR default slot map".into());
        }
        occupied.push(*slot);
        omitted.push(*slot);
    }
    Ok(omitted)
}

/// Pass a stdlib default, which a crate-call record holds only as a closed constant with no statements to run.
fn library_default(
    builder: &mut Builder,
    default: &CallableParamDefault,
    ty: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    default_type(ty)?;
    match default {
        CallableParamDefault::Source(computation) => {
            if !computation.stmts.is_empty() {
                return Err("unsupported Body IR stdlib default with statements".into());
            }
            call_argument_operand(builder, &computation.result, ty, span, true, IncanInputAccess::Inferred)
        }
        CallableParamDefault::Unsupported { description, .. } => {
            Err(format!("unsupported Body IR default: {description}"))
        }
        _ => Err("unsupported Body IR unavailable defaulted parameter".into()),
    }
}

/// Isolate default-local identities from caller locals and drop temporary owners after extracting the result.
///
/// With `owned`, a collection default is moved into the slot that stores it; otherwise the call borrows it and its
/// owner stays in the caller frame until the call returns.
pub fn evaluate_default(
    module: &BodyIrModule,
    builder: &mut Builder,
    body: &Body,
    index: usize,
    ty: &PlanType,
    span: &SourceSpan,
    owned: bool,
) -> Result<Operand, String> {
    let access = body.params[index].input_access;
    if matches!(
        access,
        IncanInputAccess::Shared | IncanInputAccess::InferredShared | IncanInputAccess::Exclusive
    ) {
        return Err("unsupported Body IR borrowed default requires retained invocation owner".into());
    }
    default_type(ty)?;
    let computation = match &body.params[index].default {
        CallableParamDefault::Source(computation) => computation,
        CallableParamDefault::Unsupported { description, .. } => {
            return Err(format!("unsupported Body IR default: {description}"));
        }
        _ => return Err("unsupported Body IR unavailable defaulted parameter".into()),
    };
    let incoming = builder.owned.clone();
    let saved = builder.local_offset;
    // Body IR owns these local types; reserve a distinct copy of that frame in the caller.
    let offset = builder.locals.len() as i64;
    builder.locals.push(Local {
        ty: PlanType::Unit,
        span: span.clone(),
    });
    for local in &body.locals {
        let ty = native_source_type(
            builder.constructors(),
            builder.nominals(),
            builder.enums(),
            &builder.unions,
            &local.ty,
            &builder.intrinsics,
            "",
        )?;
        builder.locals.push(Local { ty, span: span.clone() });
    }
    builder.local_offset = offset;
    // The computation runs for this construction or call, so its statements report that site.
    let anchor = builder.anchor.replace(span.clone());
    for statement in &computation.stmts {
        lower_statement(module, builder, statement)?;
    }
    builder.anchor = anchor;
    let before = builder.owned.clone();
    let result = call_argument_operand(builder, &computation.result, ty, span, owned, access)?;
    let mut result_owner = match &result.kind {
        OperandKind::Move(place) => place.local,
        _ => -1,
    };
    if !owned && (is_list(ty) || is_collection(ty)) {
        result_owner = borrowed_default_owner(builder, &computation.result, &before, span)?;
    }
    builder.local_offset = saved;
    let retained = builder.owned.clone();
    for owner in retained.iter().rev() {
        if !incoming.contains(owner) && *owner != result_owner {
            drop_owned(builder, *owner, span);
        }
    }
    Ok(result)
}

/// Find the collection a borrowed default argument refers to.
///
/// The call borrows it, so it stays owned by the caller frame and drops when that scope ends, after the call. A literal
/// default, such as a bytes literal, or a global read is the owner its borrow created after the owners in `before`.
/// Prefer that owner so a global is read exactly once and its borrowed snapshot is retained.
fn borrowed_default_owner(
    builder: &mut Builder,
    value: &SourceOperand,
    before: &[i64],
    span: &SourceSpan,
) -> Result<i64, String> {
    let created: Vec<i64> = builder
        .owned
        .iter()
        .copied()
        .filter(|owner| !before.contains(owner))
        .collect();
    if created.len() == 1 {
        return Ok(created[0]);
    }
    if created.is_empty()
        && let SourceOperand::Place(read) = value
    {
        return Ok(list_root_place(builder, &read.place, span)?.local);
    }
    Err("unsupported Body IR constant collection default".into())
}
