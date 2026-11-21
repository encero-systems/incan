//! Closures whose parameters carry defaults: local partials with presets and the declaration defaults they keep.
//!
//! Legacy gives a partial's preset parameter an `Option` input that the closure resolves against its captured preset.
//! Here each parameter that carries a default instead takes a supplied flag before its value: a call that names the
//! parameter passes `true` and the value, and a call that omits it passes `false` and a placeholder, so the closure
//! evaluates the preset or the declaration default itself, exactly once per call and after the written arguments.
//! The flags a local holds are read with `checked_values::partial_flags`.

use incan_mir_plan_native::{
    Constant, ListLeaf, Operand, Place, PlanType, RvalueKind, SourceSpan, TerminatorKind, assign as plan_assign,
    place as plan_place,
};
use incan_semantics_core::IncanInputAccess;
use incan_semantics_core::body_ir::{
    ArgumentBinding, ArgumentElement, BodyIrModule, CallableParam, CallableParamDefault,
};

use crate::builtins::list_call;
use crate::call_arguments::call_argument_operand;
use crate::frame::{Builder, drop_owned, forget_owned, literal, read_place, temporary, track_rvalue_moves};
use crate::list_values::list_borrow;
use crate::lists::{list_element, list_parameter_type};
use crate::statements::lower_statement;
use crate::text_plan::{clone_text, owned_literal};

/// One flag per declared parameter, set when the parameter carries a preset or a declaration default.
pub fn default_flags(parameters: &[CallableParam]) -> Vec<bool> {
    parameters
        .iter()
        .map(|parameter| !matches!(parameter.default, CallableParamDefault::Required))
        .collect()
}

/// Insert a `bool` supplied flag before each input whose parameter carries a default.
pub fn with_supplied_flags(signature: &[PlanType], flags: &[bool]) -> Result<Vec<PlanType>, String> {
    if signature.len() != flags.len() + 1 {
        return Err("invalid Body IR partial callable signature".into());
    }
    let mut expanded = Vec::with_capacity(signature.len() + flags.len());
    for (flag, input) in flags.iter().zip(signature) {
        if *flag {
            expanded.push(PlanType::Bool);
        }
        expanded.push(input.clone());
    }
    expanded.extend(signature.last().cloned());
    Ok(expanded)
}

/// Remember the default flags of the callable a local now holds, forgetting those of any partial it held before.
pub fn record_partial(builder: &mut Builder, local: i64, flags: Vec<bool>) {
    builder.partial_callables.retain(|entry| entry.0 != local);
    if flags.contains(&true) {
        builder.partial_callables.push((local, flags));
    }
}

/// Pass a call's written arguments, in written order, and then fill the closure's supplied flags.
///
/// A parameter that carries a default receives `true` and its argument when the call names it, and `false` and a
/// placeholder when the call leaves it to the closure. Every other parameter must be named.
pub fn partial_call_operands(
    builder: &mut Builder,
    binding: &ArgumentBinding,
    elements: &[ArgumentElement],
    signature: &[PlanType],
    flags: &[bool],
    span: &SourceSpan,
) -> Result<Vec<Operand>, String> {
    let ArgumentBinding::Resolved {
        arguments: bindings, ..
    } = binding
    else {
        return Err("unsupported Body IR unresolved ArgumentBinding".into());
    };
    let positions = input_positions(flags);
    let mut values: Vec<Option<Operand>> = vec![None; flags.len()];
    let mut named: Vec<usize> = Vec::with_capacity(bindings.len());
    for (bound, element) in bindings.iter().zip(elements) {
        let slot = bound.slot;
        if slot >= flags.len() || named.contains(&slot) {
            return Err("invalid Body IR argument slot map".into());
        }
        named.push(slot);
        let Some(value) = element.as_one() else {
            return Err("unsupported Body IR named or spread ArgumentElement".into());
        };
        let ty = &signature[positions[slot]];
        values[slot] = Some(call_argument_operand(
            builder,
            value,
            ty,
            span,
            false,
            IncanInputAccess::Inferred,
        )?);
    }
    assemble_inputs(builder, values, signature, flags, &positions, span)
}

/// Each declared parameter's value input position in the expanded signature.
fn input_positions(flags: &[bool]) -> Vec<usize> {
    let mut positions = Vec::with_capacity(flags.len());
    let mut position = 0;
    for flag in flags {
        if *flag {
            position += 1;
        }
        positions.push(position);
        position += 1;
    }
    positions
}

/// Order the evaluated values and supplied flags as the expanded signature lists them.
fn assemble_inputs(
    builder: &mut Builder,
    values: Vec<Option<Operand>>,
    signature: &[PlanType],
    flags: &[bool],
    positions: &[usize],
    span: &SourceSpan,
) -> Result<Vec<Operand>, String> {
    let mut operands = Vec::with_capacity(signature.len());
    for (slot, value) in values.into_iter().enumerate() {
        match value {
            Some(value) => {
                if flags[slot] {
                    operands.push(literal(Constant::Bool(true), span));
                }
                operands.push(value);
            }
            None => {
                if !flags[slot] {
                    return Err("unsupported Body IR omitted argument without a default".into());
                }
                operands.push(literal(Constant::Bool(false), span));
                operands.push(placeholder(builder, &signature[positions[slot]], span)?);
            }
        }
    }
    Ok(operands)
}

/// A value the closure never reads, passed for an omitted parameter whose flag says it was not supplied.
fn placeholder(builder: &mut Builder, ty: &PlanType, span: &SourceSpan) -> Result<Operand, String> {
    match ty {
        PlanType::Int => Ok(literal(Constant::Int(0), span)),
        PlanType::Float => Ok(literal(Constant::Float(0.0), span)),
        PlanType::Bool => Ok(literal(Constant::Bool(false), span)),
        PlanType::String => {
            let text = owned_literal(builder, "", span);
            Ok(read_place(text, true, span))
        }
        PlanType::ListRef(leaf, depth) => {
            // A list parameter is a view, so the placeholder views an empty list the caller owns until its scope ends.
            let empty = temporary(builder, PlanType::List(leaf.clone(), *depth), span);
            empty_list(builder, &empty, leaf, *depth, span);
            list_borrow(builder, empty, false, span)
        }
        _ => Err("unsupported Body IR omitted partial parameter of this type".into()),
    }
}

/// Store a new empty list in `output`, which owns it from then on; it allocates nothing until an item is added.
fn empty_list(builder: &mut Builder, output: &Place, leaf: &ListLeaf, depth: i64, span: &SourceSpan) {
    let ty = PlanType::List(leaf.clone(), depth);
    list_call(
        builder,
        "empty_list",
        list_element(leaf, depth),
        Vec::new(),
        ty,
        Vec::new(),
        output.clone(),
        span,
    );
}

/// Bind a parameter that carries a default from its supplied flag: the passed value, or else its preset or default.
///
/// The flag is the argument at `flag_slot` and the value the one after it. An omitted text value is dropped unread. A
/// list parameter views its existing captured preset, or its computed declaration default. Only a computed default
/// needs a holder local until the closure returns; the supplied path initializes that holder with an empty list. Any
/// other default whose computation would leave owned values behind stays refused.
#[allow(clippy::too_many_arguments)] // The module, lifted frame, parameter, both slots, carrier and span.
pub fn bind_defaulted_parameter(
    module: &BodyIrModule,
    lifted: &mut Builder,
    parameter: &CallableParam,
    destination: i64,
    flag_slot: i64,
    ty: PlanType,
    span: &SourceSpan,
) -> Result<(), String> {
    let slot = destination as usize;
    if list_parameter_type(&lifted.locals[slot].ty, false) != ty {
        return Err("invalid Body IR closure argument type".into());
    }
    lifted.locals[slot].ty = ty.clone();
    let mut holder = -1;
    if let (PlanType::ListRef(leaf, depth), CallableParamDefault::Source(_)) = (&ty, &parameter.default) {
        holder = temporary(lifted, PlanType::List(leaf.clone(), *depth), span).local;
    }
    let supplied = lifted.new_block(span);
    let omitted = lifted.new_block(span);
    let joined = lifted.new_block(span);
    let flag = read_place(plan_place(flag_slot, span), false, span);
    lifted.finish(TerminatorKind::SwitchBool(flag, omitted, supplied), span);
    let incoming = lifted.owned.clone();
    lifted.select(supplied);
    let owned = ty == PlanType::String;
    let passed = read_place(plan_place(flag_slot + 1, span), owned, span);
    lifted.emit(plan_assign(
        plan_place(destination, span),
        RvalueKind::Use(passed),
        span,
    ));
    if let PlanType::ListRef(leaf, depth) = &ty
        && holder >= 0
    {
        empty_list(lifted, &plan_place(holder, span), leaf, *depth, span);
    }
    lifted.finish(TerminatorKind::Goto(joined), span);
    lifted.select(omitted);
    lifted.owned = incoming.clone();
    if owned {
        lifted.owned.push(flag_slot + 1);
        drop_owned(lifted, flag_slot + 1, span);
    }
    let value = default_value(module, lifted, parameter, &ty, span)?;
    // The preset's copy or the computed default moves into the parameter, so it needs no drop of its own.
    track_rvalue_moves(lifted, &RvalueKind::Use(value.clone()));
    if holder >= 0 {
        hold_default_list(lifted, &incoming, holder, destination, span)?;
    } else {
        lifted.emit(plan_assign(plan_place(destination, span), RvalueKind::Use(value), span));
    }
    if lifted.owned != incoming {
        return Err("unsupported Body IR partial default leaving owned values".into());
    }
    lifted.finish(TerminatorKind::Goto(joined), span);
    lifted.select(joined);
    lifted.owned = incoming;
    if owned {
        lifted.owned.push(destination);
    }
    if holder >= 0 {
        lifted.owned.push(holder);
    }
    Ok(())
}

/// Move the one list a default computation created into the parameter's holder, and view the holder instead.
///
/// The view the computation returned is left unused; the holder outlives the parameter until the closure returns.
fn hold_default_list(
    lifted: &mut Builder,
    incoming: &[i64],
    holder: i64,
    destination: i64,
    span: &SourceSpan,
) -> Result<(), String> {
    let created: Vec<i64> = lifted
        .owned
        .iter()
        .copied()
        .filter(|owner| !incoming.contains(owner))
        .collect();
    let [computed] = created[..] else {
        return Err("unsupported Body IR partial default leaving owned values".into());
    };
    let moved = read_place(plan_place(computed, span), true, span);
    lifted.emit(plan_assign(plan_place(holder, span), RvalueKind::Use(moved), span));
    forget_owned(lifted, computed);
    lifted.emit(plan_assign(
        plan_place(destination, span),
        RvalueKind::Borrow(plan_place(holder, span)),
        span,
    ));
    Ok(())
}

/// Evaluate an omitted parameter's preset, copied from the closure's capture, or its declaration default.
fn default_value(
    module: &BodyIrModule,
    lifted: &mut Builder,
    parameter: &CallableParam,
    ty: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    match &parameter.default {
        CallableParamDefault::PartialPreset { capture } => {
            let source = plan_place(capture.index() as i64 + 1 + lifted.local_offset, span);
            if *ty == PlanType::String {
                let copy = clone_text(lifted, source, span)?;
                return Ok(read_place(copy, true, span));
            }
            if let PlanType::ListRef(..) = ty {
                return list_borrow(lifted, source, false, span);
            }
            Ok(read_place(source, false, span))
        }
        CallableParamDefault::Source(computation) => {
            for statement in &computation.stmts {
                lower_statement(module, lifted, statement)?;
            }
            call_argument_operand(lifted, &computation.result, ty, span, false, IncanInputAccess::Inferred)
        }
        _ => Err("unsupported Body IR partial parameter default".into()),
    }
}
