//! Calls through a held function item, pointer or closure, directly or through a standard callable bound.

use incan_mir_plan_native::{CalleeKind, Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign};
use incan_semantics_core::body_ir::{
    ArgumentBinding, ArgumentElement, LocalCallableTarget, MethodTarget, Operand as SourceOperand, OwnershipFact,
    Place as SourcePlace, PlaceOperand,
};

use crate::call_arguments::{SlotPolicy, ordered_slot_operands};
use crate::callable_calls::lower_callable_result;
use crate::checked_values::{partial_flags, place_read, scalar_place};
use crate::closures::{callable_signature, closure_arguments};
use crate::frame::{
    Builder, drop_owned, external_call, forget_owned, place_type, read_place, temporary, track_rvalue_moves, value_type,
};
use crate::list_methods::call_destination;
use crate::list_values::value_operand;
use crate::method_calls::is_std_primitives_method;
use crate::partial_callables::partial_call_operands;
use crate::text_plan::owned_literal;

/// Call an admitted function item or pointer; closure environments retain a distinct named refusal.
pub fn lower_callable_value(
    builder: &mut Builder,
    target: &LocalCallableTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    call_callable_place(
        builder,
        &target.operand,
        &target.binding,
        arguments,
        0,
        destination,
        span,
    )
}

/// Call a function or closure received through a standard callable bound, as legacy calls its `Fn` value.
///
/// The checker selects the callable trait's `__call__` for `func(value)` on a type parameter bounded by it; a closed
/// instance holding a function item, pointer or closure calls that value with the remaining arguments. Any other
/// receiver, a nominal callable object included, keeps its ordinary method dispatch.
pub fn lower_callable_bound_call(
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<bool, String> {
    if !is_std_primitives_method(target, "traits", "callable", "__call__") || arguments.is_empty() {
        return Ok(false);
    }
    let Some(SourceOperand::Place(read)) = arguments[0].as_one() else {
        return Ok(false);
    };
    let held = scalar_place(builder, &read.place, span)?;
    if callable_signature(&place_type(&builder.locals, &held)).is_err() {
        return Ok(false);
    }
    call_callable_place(builder, read, &target.binding, arguments, 1, destination, span)?;
    Ok(true)
}

/// Call the function item, pointer or closure a local holds, binding the arguments from position `first` on.
///
/// A direct call passes every argument; a call through a callable bound's method passes the receiver first.
pub fn call_callable_place(
    builder: &mut Builder,
    held: &PlaceOperand,
    binding: &ArgumentBinding,
    arguments: &[ArgumentElement],
    first: usize,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let callable = scalar_place(builder, &held.place, span)?;
    let signature = callable_signature(&place_type(&builder.locals, &callable))?;
    let Some(result) = signature.last().cloned() else {
        return Err("invalid Body IR callable signature without return type".into());
    };
    let operands = match partial_flags(builder, callable.local).map(<[bool]>::to_vec) {
        Some(flags) => {
            // A partial's supplied flags follow its written arguments, so it binds from the first argument only.
            if first != 0 {
                return Err("unsupported Body IR partial called through a callable bound".into());
            }
            partial_call_operands(builder, binding, arguments, &signature, &flags, span)?
        }
        None => {
            let parameters = &signature[..signature.len() - 1];
            let policy = SlotPolicy {
                offset: first,
                ..SlotPolicy::default()
            };
            ordered_slot_operands(builder, binding, arguments, parameters, span, &policy)?
        }
    };
    let output = match destination {
        Some(place) => scalar_place(builder, place, span)?,
        None => temporary(builder, result.clone(), span),
    };
    match place_type(&builder.locals, &callable) {
        PlanType::ClosureOnce(_) => {
            if held.fact != OwnershipFact::Move || !held.last_use {
                return Err("unsupported Body IR consuming callable requires a moved last-use invocation".into());
            }
            return call_closure_once(builder, callable, &signature, operands, output, span);
        }
        PlanType::Closure(_) | PlanType::ClosureRef(_) | PlanType::BorrowingClosureRef(_) => {
            return call_closure(builder, callable, &signature, operands, output, span);
        }
        PlanType::BorrowingClosure(_) => {
            let local = callable.local;
            call_closure(builder, callable, &signature, operands, output, span)?;
            if held.last_use && local > builder.parameter_count {
                drop_owned(builder, local, span);
            }
            return Ok(());
        }
        _ => {}
    }
    let value = place_read(builder, &held.place, held.fact, span)?;
    lower_callable_result(builder, CalleeKind::Value(value), operands, output, result, span)
}

/// Transfer an owned callable environment once, preserving packed argument and result ownership.
fn call_closure_once(
    builder: &mut Builder,
    callable: Place,
    signature: &[PlanType],
    operands: Vec<Operand>,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let packed = packed_arguments(builder, signature, operands, span)?;
    forget_owned(builder, callable.local);
    lower_callable_result(
        builder,
        CalleeKind::CallClosureOnce(signature.to_vec()),
        vec![read_place(callable, true, span), read_place(packed, true, span)],
        output,
        signature[signature.len() - 1].clone(),
        span,
    )
}

/// Pack the call's inputs into one tuple and invoke the borrowed callable object.
pub fn call_closure(
    builder: &mut Builder,
    callable: Place,
    signature: &[PlanType],
    operands: Vec<Operand>,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let packed = packed_arguments(builder, signature, operands, span)?;
    let borrowing = matches!(
        place_type(&builder.locals, &callable),
        PlanType::BorrowingClosure(_) | PlanType::BorrowingClosureRef(_)
    );
    let receiver = callable_object(builder, callable, signature, span)?;
    let callee = if borrowing {
        CalleeKind::CallBorrowingClosure(signature.to_vec())
    } else {
        CalleeKind::CallClosure(signature.to_vec())
    };
    lower_callable_result(
        builder,
        callee,
        vec![receiver, read_place(packed, true, span)],
        output,
        signature[signature.len() - 1].clone(),
        span,
    )
}

/// Pack a call's inputs into the callable's argument tuple, transferring each moved owner.
fn packed_arguments(
    builder: &mut Builder,
    signature: &[PlanType],
    operands: Vec<Operand>,
    span: &SourceSpan,
) -> Result<Place, String> {
    let elements = closure_arguments(signature)?;
    let packed = temporary(builder, PlanType::CallableArguments(elements), span);
    let tuple = RvalueKind::Tuple(operands);
    track_rvalue_moves(builder, &tuple);
    builder.emit(plan_assign(packed.clone(), tuple, span));
    Ok(packed)
}

/// Borrow an owned closure, or copy a received view, as the shared callable object a call takes.
pub fn callable_object(
    builder: &mut Builder,
    callable: Place,
    signature: &[PlanType],
    span: &SourceSpan,
) -> Result<Operand, String> {
    let view_type = match place_type(&builder.locals, &callable) {
        PlanType::ClosureRef(_) | PlanType::BorrowingClosureRef(_) => return Ok(read_place(callable, false, span)),
        PlanType::Closure(_) => PlanType::ClosureRef(signature.to_vec()),
        PlanType::BorrowingClosure(_) => PlanType::BorrowingClosureRef(signature.to_vec()),
        _ => return Err("unsupported Body IR callable without an object representation".into()),
    };
    let view = temporary(builder, view_type, span);
    builder.emit(plan_assign(view.clone(), RvalueKind::ClosureObject(callable), span));
    Ok(read_place(view, false, span))
}

/// Register an authenticated named read before a decorator or ordinary argument can observe its name.
pub fn register_callable_item(
    builder: &mut Builder,
    value: Operand,
    name: &str,
    span: &SourceSpan,
) -> Result<(), String> {
    let address = callable_address(builder, value, span)?;
    let module_identity = builder.module_identity.clone();
    let module = owned_literal(builder, &module_identity, span);
    let label = owned_literal(builder, name, span);
    forget_owned(builder, module.local);
    forget_owned(builder, label.local);
    let output = temporary(builder, PlanType::Unit, span);
    external_call(
        builder,
        "incan_native_runtime::register_callable_name",
        vec![PlanType::String, PlanType::USize, PlanType::String],
        PlanType::Unit,
        vec![read_place(module, true, span), address, read_place(label, true, span)],
        output,
        span,
    );
    Ok(())
}

/// Resolve a registered declaration name independently of the pointer's argument and result signature.
pub fn lower_callable_name(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if !builder.callable_name_dynamic_profile {
        return Err("unsupported Body IR callable-name lookup across registration modules".into());
    }
    let [argument] = arguments else {
        return Err("invalid Body IR callable-name operand count".into());
    };
    let Some(value) = argument.as_one() else {
        return Err("unsupported Body IR callable-name spread operand".into());
    };
    let callback = value_operand(builder, value, span)?;
    let address = callable_address(builder, callback, span)?;
    let module_identity = builder.module_identity.clone();
    let module = owned_literal(builder, &module_identity, span);
    forget_owned(builder, module.local);
    let output = call_destination(builder, destination, PlanType::String, span)?;
    external_call(
        builder,
        "incan_native_runtime::callable_name",
        vec![PlanType::String, PlanType::USize],
        PlanType::String,
        vec![read_place(module, true, span), address],
        output,
        span,
    );
    Ok(())
}

/// Expose the address of the exact checked pointer, reifying an item without replacing its signature.
fn callable_address(builder: &mut Builder, value: Operand, span: &SourceSpan) -> Result<Operand, String> {
    let pointer = match value_type(builder, &value)? {
        PlanType::FunctionItem(_, signature) => {
            let held = temporary(builder, PlanType::FunctionPointer(signature), span);
            builder.emit(plan_assign(held.clone(), RvalueKind::ReifyFunction(value), span));
            read_place(held, false, span)
        }
        PlanType::FunctionPointer(_) => value,
        _ => return Err("unsupported Body IR callable-name receiver without a retained function pointer".into()),
    };
    let address = temporary(builder, PlanType::USize, span);
    builder.emit(plan_assign(address.clone(), RvalueKind::FunctionAddress(pointer), span));
    Ok(read_place(address, false, span))
}
