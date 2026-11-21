//! Result's builtin combinators, lowered as one branch on the receiver's active variant.
//!
//! `map`, `and_then` and `inspect` call their callback on the `Ok` payload, and `map_err`, `or_else` and `inspect_err`
//! on the `Err` payload. The callback is a function item, a function pointer, a closure, or a model whose own
//! `__call__` method makes it callable. The other variant passes through unchanged, as the standard library's own
//! bodies do.

use incan_lang::lang::surface::result_methods::ResultMethodId;
use incan_mir_plan_native::{
    CalleeKind, Carrier, EnumDeclaration, Operand, OperandKind, Place, PlanType, Projection, RvalueKind, SourceSpan,
    TerminatorKind, assign as plan_assign,
};
use incan_semantics_core::body_ir::{
    ArgumentElement, BodyIrModule, Operand as SourceOperand, OwnershipFact, Place as SourcePlace,
};

use crate::callable_calls::lower_callable_result;
use crate::carrier_operands::boundary_plan_operand;
use crate::checked_values::scalar_place;
use crate::classes::{frame_type, function_name, parameter_local_index};
use crate::closures::callable_signature;
use crate::collections::{collection_owned, collection_reference, is_collection};
use crate::frame::{
    Builder, drop_owned, forget_owned, place_type, read_place, temporary, track_rvalue_moves, value_type,
};
use crate::layout_types::native_source_type;
use crate::list_values::list_borrow;
use crate::options::{
    assign_carrier, carrier_layout, carrier_payload_value, replace_owned_destination, variant_condition,
};
use crate::pending::untranslated;
use crate::trait_links::contains;

/// What a combinator does with the selected payload's callback result.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// The callback's result becomes that variant's new payload.
    Wrap,
    /// The callback's result is the whole new Result.
    Chain,
    /// The callback only sees the payload and the receiver is returned unchanged.
    Observe,
}

/// What one builtin Result combinator does.
///
/// `slot` is the variant whose payload the callback receives: zero for `Ok`, one for `Err`.
#[derive(Clone, Copy)]
pub struct Combinator {
    pub slot: i64,
    pub effect: Effect,
}

/// Select one checker-authenticated Result combinator.
pub fn result_combinator(method: ResultMethodId) -> Option<Combinator> {
    let (slot, effect) = match method {
        ResultMethodId::Map => (0, Effect::Wrap),
        ResultMethodId::MapErr => (1, Effect::Wrap),
        ResultMethodId::AndThen => (0, Effect::Chain),
        ResultMethodId::OrElse => (1, Effect::Chain),
        ResultMethodId::Inspect => (0, Effect::Observe),
        ResultMethodId::InspectErr => (1, Effect::Observe),
        _ => return None,
    };
    Some(Combinator { slot, effect })
}

/// Take the receiver as an owned carrier, then branch on its active variant and call the callback on one side.
pub fn lower_result_combinator(
    module: &BodyIrModule,
    builder: &mut Builder,
    combinator: Combinator,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 2 {
        return Err("invalid Body IR Result combinator arity".into());
    }
    let source = owned_receiver(builder, &arguments[0], span)?;
    let (_, input) = carrier_layout(builder, &place_type(&builder.locals, &source))?;
    if input.carrier != Carrier::Result {
        return Err("unsupported Body IR Result combinator on another carrier".into());
    }
    let callback = callback_place(builder, &arguments[1], span)?;
    if combinator.effect == Effect::Observe {
        let ty = place_type(&builder.locals, &source);
        let output = match destination {
            Some(place) => scalar_place(builder, place, span)?,
            None => temporary(builder, ty, span),
        };
        let selection = Selection {
            input: &input,
            source,
            callback,
            output,
        };
        return observe(module, builder, combinator.slot, selection, span);
    }
    let Some(place) = destination else {
        return Err("unsupported Body IR discarded Result combinator result".into());
    };
    let output = scalar_place(builder, place, span)?;
    let selection = Selection {
        input: &input,
        source,
        callback,
        output,
    };
    transform(module, builder, combinator, selection, span)
}

/// The receiver's carrier layout, its owned temporary, the callback and the result's destination.
struct Selection<'a> {
    input: &'a EnumDeclaration,
    source: Place,
    callback: Place,
    output: Place,
}

/// Hold the receiver in a fresh owned temporary.
///
/// A moved receiver is taken whole. A receiver the source reads again is cloned, so the original stays usable.
fn owned_receiver(builder: &mut Builder, argument: &ArgumentElement, span: &SourceSpan) -> Result<Place, String> {
    let read = match argument.as_one() {
        Some(SourceOperand::Place(read)) => read,
        Some(_) => return Err("unsupported Body IR Result combinator nonplace receiver".into()),
        None => return Err("unsupported Body IR Result combinator spread receiver".into()),
    };
    let place = scalar_place(builder, &read.place, span)?;
    let ty = place_type(&builder.locals, &place);
    let value = match read.fact {
        OwnershipFact::Move => {
            if place.projection != Projection::Whole {
                return Err("unsupported Body IR partial Result combinator receiver move".into());
            }
            read_place(place, true, span)
        }
        OwnershipFact::Clone | OwnershipFact::Borrow => carrier_payload_value(builder, place, span)?,
        _ => return Err("unsupported Body IR Result combinator receiver ownership".into()),
    };
    let source = temporary(builder, ty, span);
    track_rvalue_moves(builder, &RvalueKind::Use(value.clone()));
    builder.emit(plan_assign(source.clone(), RvalueKind::Use(value), span));
    builder.owned.push(source.local);
    Ok(source)
}

/// The local holding the callback; a callback is only ever borrowed or copied, never consumed.
fn callback_place(builder: &mut Builder, argument: &ArgumentElement, span: &SourceSpan) -> Result<Place, String> {
    match argument.as_one() {
        Some(SourceOperand::Place(read)) => {
            if read.fact == OwnershipFact::Unknown || read.fact == OwnershipFact::MutBorrow {
                return Err("unsupported Body IR Result combinator callback ownership".into());
            }
            scalar_place(builder, &read.place, span)
        }
        Some(_) => Err("unsupported Body IR Result combinator nonplace callback".into()),
        None => Err("unsupported Body IR Result combinator spread callback".into()),
    }
}

/// Call the callback with a copy of the selected payload, then return the receiver unchanged.
///
/// Legacy hands the callback a borrow of the payload. A callback cannot change or keep its parameter, so a copy, or a
/// clone of an owned payload, is observed identically.
fn observe(
    module: &BodyIrModule,
    builder: &mut Builder,
    slot: i64,
    selection: Selection<'_>,
    span: &SourceSpan,
) -> Result<(), String> {
    let Selection {
        input,
        source,
        callback,
        output,
    } = selection;
    let payload_type = input.variants[slot as usize].fields[0].clone();
    let selected = builder.new_block(span);
    let join = builder.new_block(span);
    let owners = builder.owned.clone();
    let condition = variant_condition(builder, &source, slot, span);
    builder.finish(TerminatorKind::SwitchBool(condition, join, selected), span);
    builder.select(selected);
    let payload = Place {
        local: source.local,
        projection: Projection::VariantField(slot, 0, payload_type.clone()),
        span: span.clone(),
    };
    let argument = carrier_payload_value(builder, payload, span)?;
    let unit = temporary(builder, PlanType::Unit, span);
    let call = Callback {
        callback,
        argument,
        payload: payload_type,
        output: unit,
        result: PlanType::Unit,
    };
    call_callback(module, builder, call, span)?;
    builder.finish(TerminatorKind::Goto(join), span);
    builder.select(join);
    builder.owned = owners;
    forget_owned(builder, source.local);
    replace_owned_destination(builder, output, RvalueKind::Use(read_place(source, true, span)), span);
    Ok(())
}

/// Move the active payload out of the receiver: the selected side goes to the callback, the other side is rewrapped.
///
/// `Wrap` rewraps the callback's result in the selected variant; `Chain` returns the callback's Result as it is. The
/// untouched payload crosses its checked source boundary, so native borrowed text becomes owned text when required.
fn transform(
    module: &BodyIrModule,
    builder: &mut Builder,
    combinator: Combinator,
    selection: Selection<'_>,
    span: &SourceSpan,
) -> Result<(), String> {
    let Selection {
        input,
        source,
        callback,
        output,
    } = selection;
    let slot = combinator.slot;
    let output_type = place_type(&builder.locals, &output);
    let (owner, result) = carrier_layout(builder, &output_type)?;
    if result.carrier != Carrier::Result {
        return Err("unsupported Body IR Result combinator output carrier".into());
    }
    let payload_type = input.variants[slot as usize].fields[0].clone();
    let callback_result = if combinator.effect == Effect::Wrap {
        result.variants[slot as usize].fields[0].clone()
    } else {
        output_type
    };
    let selected = builder.new_block(span);
    let other = builder.new_block(span);
    let join = builder.new_block(span);
    let condition = variant_condition(builder, &source, slot, span);
    builder.finish(TerminatorKind::SwitchBool(condition, other, selected), span);
    // The carrier has one active payload. Both edges move it, so neither drops the emptied carrier.
    forget_owned(builder, source.local);
    let owners = builder.owned.clone();
    builder.select(other);
    let kept_slot = 1 - slot;
    let kept_type = input.variants[kept_slot as usize].fields[0].clone();
    let kept = Place {
        local: source.local,
        projection: Projection::VariantField(kept_slot, 0, kept_type),
        span: span.clone(),
    };
    let kept_target = result.variants[kept_slot as usize].fields[0].clone();
    let converted = boundary_plan_operand(builder, read_place(kept, true, span), &kept_target, span)?;
    if value_type(builder, &converted)? != kept_target {
        return Err("unsupported Body IR Result combinator changing the untouched side".into());
    }
    assign_carrier(builder, output.clone(), owner, kept_slot, vec![converted], span);
    let retained = builder.owned.clone();
    builder.finish(TerminatorKind::Goto(join), span);
    builder.select(selected);
    builder.owned = owners;
    let payload = Place {
        local: source.local,
        projection: Projection::VariantField(slot, 0, payload_type.clone()),
        span: span.clone(),
    };
    let argument = read_place(payload, true, span);
    if combinator.effect == Effect::Wrap {
        let value = temporary(builder, callback_result.clone(), span);
        let call = Callback {
            callback,
            argument,
            payload: payload_type,
            output: value.clone(),
            result: callback_result,
        };
        call_callback(module, builder, call, span)?;
        assign_carrier(builder, output, owner, slot, vec![read_place(value, true, span)], span);
    } else {
        let call = Callback {
            callback,
            argument,
            payload: payload_type,
            output,
            result: callback_result,
        };
        call_callback(module, builder, call, span)?;
    }
    builder.finish(TerminatorKind::Goto(join), span);
    builder.select(join);
    builder.owned = retained;
    Ok(())
}

/// One callback call: the held callback, its argument with the payload's type, and where its result goes.
struct Callback {
    callback: Place,
    argument: Operand,
    payload: PlanType,
    output: Place,
    result: PlanType,
}

/// Call the held callback at its checked ABI, retaining an owned payload through an implicit shared view.
///
/// A closure callback is called through `callable_calls::call_closure`, which refuses by name until it is in Rust.
fn call_callback(
    module: &BodyIrModule,
    builder: &mut Builder,
    call: Callback,
    span: &SourceSpan,
) -> Result<(), String> {
    let ty = place_type(&builder.locals, &call.callback);
    if let PlanType::Model(index, _) = ty {
        return call_callable_model(module, builder, index, call, span);
    }
    let signature = callable_signature(&ty)?;
    if signature.len() != 2 || signature[1] != call.result {
        return Err("unsupported Body IR Result combinator callback signature".into());
    }
    let (prepared, held) = callback_argument(builder, call.argument, &call.payload, &signature[0], span)?;
    match ty {
        PlanType::Closure(_)
        | PlanType::ClosureRef(_)
        | PlanType::BorrowingClosure(_)
        | PlanType::BorrowingClosureRef(_) => {
            return Err(untranslated("callable_calls::call_closure"));
        }
        _ => {
            let callee = CalleeKind::Value(read_place(call.callback, false, span));
            lower_callable_result(builder, callee, vec![prepared], call.output, call.result, span)?;
        }
    }
    drop_callback_argument(builder, held, span);
    Ok(())
}

/// Borrow an owned builtin collection for a view parameter, keeping consumed payload storage alive until return.
fn callback_argument(
    builder: &mut Builder,
    argument: Operand,
    payload: &PlanType,
    parameter: &PlanType,
    span: &SourceSpan,
) -> Result<(Operand, Option<i64>), String> {
    if parameter == payload {
        return Ok((argument, None));
    }
    if !shared_callback_parameter(payload, parameter) {
        return Err("unsupported Body IR Result combinator callback signature".into());
    }
    match &argument.kind {
        OperandKind::Copy(source) => Ok((list_borrow(builder, source.clone(), false, span)?, None)),
        OperandKind::Move(_) => {
            let held = temporary(builder, payload.clone(), span);
            replace_owned_destination(builder, held.clone(), RvalueKind::Use(argument), span);
            let local = held.local;
            Ok((list_borrow(builder, held, false, span)?, Some(local)))
        }
        _ => Err("unsupported Body IR borrowed Result callback payload without storage".into()),
    }
}

/// Require an exact implicit shared list, set or dictionary view, never a new element layout or exclusive loan.
fn shared_callback_parameter(payload: &PlanType, parameter: &PlanType) -> bool {
    if let PlanType::List(leaf, depth) = payload {
        return *parameter == PlanType::ListRef(leaf.clone(), *depth);
    }
    is_collection(payload)
        && collection_owned(payload) == *payload
        && *parameter == collection_reference(payload, false)
}

/// Release the consumed payload after the callback returns; its call's unwind cleanup retains the same owner.
fn drop_callback_argument(builder: &mut Builder, held: Option<i64>, span: &SourceSpan) {
    if let Some(local) = held {
        drop_owned(builder, local, span);
    }
}

/// Call a model's own `__call__` method on a shared borrow of the model, as legacy calls the callable object.
fn call_callable_model(
    module: &BodyIrModule,
    builder: &mut Builder,
    index: i64,
    call: Callback,
    span: &SourceSpan,
) -> Result<(), String> {
    let owner = builder.nominals()[index as usize].canonical.clone();
    let found: Vec<usize> = module
        .bodies
        .iter()
        .enumerate()
        .filter_map(|(position, body)| {
            let identity = body.canonical.as_ref()?;
            let owned = identity.origin == owner.origin && contains(&owner, identity);
            (owned && identity.declaration_name.as_str() == "__call__").then_some(position)
        })
        .collect();
    let [position] = found[..] else {
        return Err("unsupported Body IR Result combinator callback without one own __call__ method".into());
    };
    let body = &module.bodies[position];
    if !body.type_parameters.is_empty() || body.params.len() != 2 || body.params[1].mutable {
        return Err("unsupported Body IR Result combinator callback __call__ shape".into());
    }
    let receiver_type = frame_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &body.locals[0],
        &builder.intrinsics,
        "",
    )?;
    let PlanType::ModelRef(..) = receiver_type else {
        return Err("unsupported Body IR Result combinator callback __call__ receiver".into());
    };
    let parameter = frame_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &body.locals[parameter_local_index(body, 1)],
        &builder.intrinsics,
        "",
    )?;
    let returned = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &body.return_type,
        &builder.intrinsics,
        "",
    )?;
    if returned != call.result {
        return Err("unsupported Body IR Result combinator callback signature".into());
    }
    let (prepared, held) = callback_argument(builder, call.argument, &call.payload, &parameter, span)?;
    let reference = temporary(builder, receiver_type, span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(call.callback), span));
    let operands = vec![read_place(reference, true, span), prepared];
    let callee = CalleeKind::Planned(function_name(module, body));
    lower_callable_result(builder, callee, operands, call.output, call.result, span)?;
    drop_callback_argument(builder, held, span);
    Ok(())
}
