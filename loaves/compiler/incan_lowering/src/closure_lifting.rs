//! Lift closure expressions and generator expressions into planned functions with their captured environments.

use incan_mir_plan_native::{
    Callee, CalleeKind, Function, Local, Operand, OperandKind, Parameter, Place, PlanType, Projection, RvalueKind,
    SourceSpan, assign as plan_assign, place as plan_place,
};
use incan_semantics_core::IncanInputAccess;
use incan_semantics_core::body_ir::{
    Block, BodyIrModule, CallableParam, CallableParamDefault, GeneratorBody, LocalId, NativeClosureStorage,
    Operand as SourceOperand, OwnershipFact, Statement, StatementKind,
};

use crate::builtins::list_call;
use crate::callable_calls::owned_callable_value;
use crate::checked_values::{partial_flags, scalar_place, target_read};
use crate::closures::{
    callable_signature, closure_arguments, closure_capture_type, closure_constructor, function_assignment,
    packed_input, reference_capture,
};
use crate::collections::is_owned_collection;
use crate::element_places::is_element_place;
use crate::explicit_arguments::explicit_place_argument;
use crate::frame::{
    Builder, drop_owned, place_type, read_place, temporary, terminate_call, track_rvalue_moves, value_type,
};
use crate::generator_clauses::{LaterClause, clause_item, later_clauses};
use crate::generator_expressions::{
    construct_chain, first_clause_item, source_generator, source_item, step_capture_type, step_pointer, step_result,
};
use crate::generators::{generator_element, generator_shape};
use crate::hashed_collections::cloned_collection;
use crate::layout_types::shared_receiver_type;
use crate::list_iteration::frozen_list_snapshot;
use crate::list_methods::borrowed_list_generator_call;
use crate::list_values::{list_borrow, value_operand};
use crate::lists::{is_exclusive_borrow, is_owned_list, list_element, list_shape, parameter_storage_type};
use crate::partial_callables::{bind_defaulted_parameter, default_flags, record_partial, with_supplied_flags};
use crate::shared_callables::{record_scalar_closure, scalar_capture};
use crate::statements::{finish_return, is_nominal_type, is_owned_callable, lower_statement, return_operand};
use crate::text_plan::clone_text;
use crate::tuple_iterators::clone_zip;
use crate::tuples::tuple_has_owners;
use crate::type_parameters::merge_instance_requests;

/// Store a Copy callable with its retained item type or the source-required function-pointer coercion.
///
/// An owned closure binding that receives a function item or pointer boxes it in a forwarding environment.
pub fn assign_callable(
    builder: &mut Builder,
    target: Place,
    value: Operand,
    pointer_storage: bool,
    span: &SourceSpan,
) -> Result<(), String> {
    if target.projection != Projection::Whole {
        return Err("unsupported Body IR projected callable assignment".into());
    }
    let source = value_type(builder, &value)?;
    let declared = place_type(&builder.locals, &target);
    if let PlanType::Closure(signature) = &declared {
        // A closure holding borrowed captures keeps its borrowing object in the binding.
        if source == PlanType::BorrowingClosure(signature.clone()) {
            return move_closure(builder, target, value, signature.clone(), false, span, true);
        }
        let owned = owned_callable_value(builder, value, signature, span)?;
        return move_closure(builder, target, owned, signature.clone(), false, span, false);
    }
    let context = format!("binding in {}", builder.name);
    let (ty, operation) = function_assignment(&declared, &source, value, pointer_storage, &context)?;
    builder.locals[target.local as usize].ty = ty;
    record_partial(builder, target.local, Vec::new());
    builder.emit(plan_assign(target, operation, span));
    Ok(())
}

/// Transfer an owned closure into a callable binding, which takes the closure's own boxed type.
pub fn move_closure(
    builder: &mut Builder,
    target: Place,
    value: Operand,
    signature: Vec<PlanType>,
    consumes_environment: bool,
    span: &SourceSpan,
    borrowing: bool,
) -> Result<(), String> {
    if target.projection != Projection::Whole {
        return Err("unsupported Body IR projected closure assignment".into());
    }
    if builder.function_pointer_locals.contains(&target.local) {
        return Err("unsupported Body IR capturing closure stored as a function pointer".into());
    }
    let mut declared = callable_signature(&place_type(&builder.locals, &target))?;
    // A closure whose parameters carry defaults keeps its supplied flags in whichever binding holds it.
    let mut flags = Vec::new();
    if let OperandKind::Move(source) = &value.kind
        && let Some(held) = partial_flags(builder, source.local)
    {
        let held = held.to_vec();
        declared = with_supplied_flags(&declared, &held)?;
        flags = held;
    }
    if declared != signature {
        return Err("invalid Body IR closure assignment signature".into());
    }
    if consumes_environment && !matches!(value.kind, OperandKind::Move(_)) {
        return Err("unsupported Body IR consuming closure assignment without a move".into());
    }
    let scalar_environment = match &value.kind {
        OperandKind::Move(source) => {
            source.projection == Projection::Whole
                && builder.scalar_closures.contains(&source.local)
                && !builder.unproven_closures.contains(&source.local)
        }
        _ => false,
    };
    record_scalar_closure(builder, target.local, scalar_environment);
    track_rvalue_moves(builder, &RvalueKind::Use(value.clone()));
    drop_owned(builder, target.local, span);
    hold_closure(
        builder,
        target.local,
        &signature,
        flags,
        consumes_environment,
        borrowing,
    )?;
    let local = target.local;
    builder.emit(plan_assign(target, RvalueKind::Use(value), span));
    builder.owned.push(local);
    Ok(())
}

/// Read the callable an assignment stores; a partial moves out whole, because its binding carries its flags.
pub fn stored_callable(builder: &mut Builder, operand: &SourceOperand, span: &SourceSpan) -> Result<Operand, String> {
    let SourceOperand::Place(read) = operand else {
        return value_operand(builder, operand, span);
    };
    if read.place.global().is_some() || is_element_place(builder, &read.place)? {
        return value_operand(builder, operand, span);
    }
    let source = scalar_place(builder, &read.place, span)?;
    if partial_flags(builder, source.local).is_some() {
        if read.fact != OwnershipFact::Move || source.projection != Projection::Whole {
            return Err("unsupported Body IR partial shared between bindings".into());
        }
        return Ok(read_place(source, true, span));
    }
    value_operand(builder, operand, span)
}

/// Type a binding as the owned closure it now holds and record the closure's supplied flags, if it has any.
///
/// A checked local starts with a provisional source signature; its first value fixes the supplied-flag and Fn/FnOnce
/// ABI. Parameters, return storage and already materialized locals keep their existing ABI across later moves and
/// drops.
fn hold_closure(
    builder: &mut Builder,
    local: i64,
    signature: &[PlanType],
    flags: Vec<bool>,
    consumes_environment: bool,
    borrowing: bool,
) -> Result<(), String> {
    if local <= builder.parameter_count || builder.materialized_closures.contains(&local) {
        match &builder.locals[local as usize].ty {
            PlanType::Closure(held) => {
                if consumes_environment {
                    return Err("unsupported Body IR binding changing callable consumption".into());
                }
                if held != signature {
                    return Err(
                        "unsupported Body IR binding holding closures with different default parameters".into(),
                    );
                }
            }
            PlanType::ClosureOnce(held) => {
                if !consumes_environment || held != signature {
                    return Err("unsupported Body IR binding changing callable consumption or signature".into());
                }
            }
            _ => {}
        }
    }
    let signature = signature.to_vec();
    let object_type = if borrowing {
        PlanType::BorrowingClosure(signature)
    } else if consumes_environment {
        PlanType::ClosureOnce(signature)
    } else {
        PlanType::Closure(signature)
    };
    builder.locals[local as usize].ty = object_type;
    if !builder.materialized_closures.contains(&local) {
        builder.materialized_closures.push(local);
    }
    record_partial(builder, local, flags);
    Ok(())
}

/// Retain one synthesized function per name; a forwarding environment for one item is shared.
pub fn synthesize(builder: &mut Builder, function: Function) {
    if builder
        .synthesized
        .iter()
        .any(|retained| retained.name == function.name)
    {
        return;
    }
    builder.synthesized.push(function);
}

/// The checked parts of one closure expression.
pub struct ClosureExpression<'a> {
    pub parameters: &'a [CallableParam],
    pub captures: &'a [SourceOperand],
    pub capture_locals: &'a [LocalId],
    pub statements: &'a [Statement],
    pub result: &'a SourceOperand,
    pub consumes_environment: bool,
    pub native_storage: &'a Option<NativeClosureStorage>,
}

/// Lift a closure expression's body into a planned function, then bind the value the source requires.
///
/// A closure without captures is that function's item, so it reuses item storage, pointer reification, and calls. A
/// capturing closure boxes one environment holding its captures by value, which is the checker's snapshot contract.
pub fn lower_closure(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: Place,
    closure: &ClosureExpression<'_>,
    span: &SourceSpan,
) -> Result<(), String> {
    if target.projection != Projection::Whole {
        return Err("unsupported Body IR projected closure construction".into());
    }
    if builder.local_offset != 0 {
        return Err("unsupported Body IR closure inside a deferred default".into());
    }
    if closure.captures.len() != closure.capture_locals.len() {
        return Err("invalid Body IR closure capture layout".into());
    }
    let flags = default_flags(closure.parameters);
    let mut signature = callable_signature(&place_type(&builder.locals, &target))?;
    if flags.contains(&true) {
        if closure.captures.is_empty() {
            return Err("unsupported Body IR partial without presets".into());
        }
        signature = with_supplied_flags(&signature, &flags)?;
    }
    let mut operands = Vec::with_capacity(closure.captures.len());
    let mut capture_types = Vec::with_capacity(closure.captures.len());
    for capture in closure.captures {
        let operand = capture_operand(builder, capture, span)?;
        let ty = value_type(builder, &operand)?;
        if let PlanType::ZipIterator(..) = ty
            && !closure.consumes_environment
        {
            return Err("unsupported Body IR iterator closure capture without FnOnce contract".into());
        }
        if !closure_capture_type(&ty, closure.consumes_environment) {
            return Err(format!("unsupported Body IR closure capture {ty:?}"));
        }
        operands.push(operand);
        capture_types.push(ty);
    }
    let name = format!("__incan_closure_{}_{}", builder.name, builder.synthesized.len());
    let lifted = lift_closure(module, builder, &name, &capture_types, closure, &signature, span)?;
    synthesize(builder, lifted);
    let item = RvalueKind::FunctionItem(Callee {
        kind: CalleeKind::Planned(name.clone()),
        span: span.clone(),
    });
    if capture_types.is_empty() && !is_owned_callable(&place_type(&builder.locals, &target)) {
        let item_type = PlanType::FunctionItem(name, signature);
        if builder.function_pointer_locals.contains(&target.local) {
            let held = temporary(builder, item_type, span);
            builder.emit(plan_assign(held.clone(), item, span));
            return assign_callable(builder, target, read_place(held, false, span), true, span);
        }
        builder.locals[target.local as usize].ty = item_type;
        record_partial(builder, target.local, Vec::new());
        builder.emit(plan_assign(target, item, span));
        return Ok(());
    }
    construct_environment(
        builder,
        target,
        &name,
        capture_types,
        operands,
        closure,
        signature,
        flags,
        span,
    )
}

/// Box a capturing closure's environment through its synthesized constructor and bind the owned object.
#[allow(clippy::too_many_arguments)] // The target, lifted name, captures with their values, closure, signature and flags.
fn construct_environment(
    builder: &mut Builder,
    target: Place,
    name: &str,
    capture_types: Vec<PlanType>,
    operands: Vec<Operand>,
    closure: &ClosureExpression<'_>,
    signature: Vec<PlanType>,
    flags: Vec<bool>,
    span: &SourceSpan,
) -> Result<(), String> {
    if builder.function_pointer_locals.contains(&target.local) {
        return Err("unsupported Body IR capturing closure stored as a function pointer".into());
    }
    closure_arguments(&signature)?;
    // An environment holding shared references lives no longer than their shortest borrow, which checking keeps
    // alive. Its object takes that common bound while each capture retains its independent source lifetime.
    let borrowing = capture_types.iter().any(reference_capture);
    if borrowing && flags.contains(&true) {
        return Err("unsupported Body IR closure holding borrowed captures with default parameters".into());
    }
    let (native_send, native_sync) = match closure.native_storage {
        Some(storage) => {
            if borrowing {
                return Err("unsupported Body IR native callback holding borrowed captures".into());
            }
            if !storage.move_capture {
                return Err("invalid Body IR native callback without owned capture storage".into());
            }
            (storage.send, storage.sync)
        }
        None => (false, false),
    };
    let constructor_name = format!("{name}_new");
    let constructor = closure_constructor(
        &constructor_name,
        name,
        &capture_types,
        &signature,
        closure.consumes_environment,
        span,
        native_send,
        native_sync,
        borrowing,
    );
    synthesize(builder, constructor);
    drop_owned(builder, target.local, span);
    hold_closure(
        builder,
        target.local,
        &signature,
        flags,
        closure.consumes_environment,
        borrowing,
    )?;
    if native_send || native_sync {
        builder.locals[target.local as usize].ty = PlanType::NativeClosure(signature, native_send, native_sync);
    }
    let local = target.local;
    terminate_call(builder, CalleeKind::Planned(constructor_name), operands, target, span);
    let scalar_environment = capture_types.iter().all(scalar_capture);
    record_scalar_closure(builder, local, scalar_environment);
    builder.owned.push(local);
    Ok(())
}

/// A capture the closure borrows is a fresh shared loan of its exact place, unless that place already holds a shared
/// reference, which is copied; any other capture is read as a value.
fn capture_operand(builder: &mut Builder, capture: &SourceOperand, span: &SourceSpan) -> Result<Operand, String> {
    if let SourceOperand::Place(read) = capture
        && read.fact == OwnershipFact::Borrow
    {
        let source = scalar_place(builder, &read.place, span)?;
        let referent = place_type(&builder.locals, &source);
        if !reference_capture(&referent) {
            let expected = shared_receiver_type(referent)?;
            return explicit_place_argument(builder, source, read.fact, &expected, IncanInputAccess::Shared, span);
        }
    }
    value_operand(builder, capture, span)
}

/// Build a closure body as a planned function taking its captures, then its inputs, by value.
///
/// The enclosing frame's locals keep their Body IR slots after those arguments, so the closure's statements lower with
/// the ordinary local offset; a prologue moves each argument into the capture or parameter local the body reads. A
/// parameter that carries a default takes a supplied flag before its value (see `partial_callables`).
fn lift_closure(
    module: &BodyIrModule,
    builder: &mut Builder,
    name: &str,
    captures: &[PlanType],
    closure: &ClosureExpression<'_>,
    signature: &[PlanType],
    span: &SourceSpan,
) -> Result<Function, String> {
    let supplied_flags = default_flags(closure.parameters).iter().filter(|flag| **flag).count();
    if closure.parameters.len() + supplied_flags + 1 != signature.len() {
        return Err("invalid Body IR closure parameter count".into());
    }
    let Some((result, inputs)) = signature.split_last() else {
        return Err("invalid Body IR closure parameter count".into());
    };
    let mut arguments = captures.to_vec();
    arguments.extend_from_slice(inputs);
    let count = arguments.len() as i64;
    let lifted_parameters = argument_parameters(&arguments, span);
    let mut lifted = lifted_frame(builder, name, &arguments, result.clone(), span);
    lifted.consuming_closure_frame = closure.consumes_environment;
    for (index, captured_local) in closure.capture_locals.iter().enumerate().take(captures.len()) {
        let destination = captured_local.index() as i64 + 1 + count;
        // A shared reference capture is stored as that reference; the body reads its borrowed value through it.
        let captured = if reference_capture(&arguments[index]) {
            arguments[index].clone()
        } else {
            lifted.locals[destination as usize].ty.clone()
        };
        bind_closure_input(
            &mut lifted,
            destination,
            index as i64 + 1,
            &arguments[index],
            &captured,
            span,
        )?;
    }
    bind_closure_parameters(
        module,
        &mut lifted,
        closure.parameters,
        &arguments,
        captures.len(),
        count,
        span,
    )?;
    for statement in closure.statements {
        lower_statement(module, &mut lifted, statement)?;
    }
    if !lifted.closed[lifted.current] {
        let value = return_operand(&mut lifted, closure.result, span)?;
        finish_return(&mut lifted, value, span)?;
    }
    // A generator expression's step lifted inside the closure, with its environment constructor, belongs to the
    // enclosing plan too; any other function synthesized there keeps the nested-closure refusal.
    for function in std::mem::take(&mut lifted.synthesized) {
        if !function.name.starts_with("__incan_generator_step_") {
            return Err("unsupported Body IR closure nested in a closure".into());
        }
        synthesize(builder, function);
    }
    adopt_lifted(builder, &lifted);
    Ok(Function::new(
        name.to_owned(),
        lifted_parameters,
        result.clone(),
        lifted.locals,
        lifted.blocks,
        span.clone(),
    ))
}

/// Bind each closure parameter from its argument, after the captures; a defaulted one reads its supplied flag first.
fn bind_closure_parameters(
    module: &BodyIrModule,
    lifted: &mut Builder,
    parameters: &[CallableParam],
    arguments: &[PlanType],
    start: usize,
    count: i64,
    span: &SourceSpan,
) -> Result<(), String> {
    let mut position = start;
    for parameter in parameters {
        let destination = parameter.local.index() as i64 + 1 + count;
        if let CallableParamDefault::Required = parameter.default {
            let declared = lifted.locals[destination as usize].ty.clone();
            // A closure checked against a callable type whose input is `mut` takes that input as the borrow too.
            let mutable = parameter.mutable || parameter_storage_type(&declared, true) == arguments[position];
            let mut storage = parameter_storage_type(&declared, mutable);
            if parameter.native_owned || parameter.input_access != IncanInputAccess::Inferred {
                storage = packed_input(
                    declared,
                    parameter.kind,
                    parameter.mutable,
                    parameter.native_owned,
                    parameter.input_access,
                )?;
            }
            let argument = position as i64 + 1;
            bind_closure_input(lifted, destination, argument, &arguments[position], &storage, span)?;
            position += 1;
        } else {
            // The supplied flag comes first, then the value it describes.
            let value_type = arguments[position + 1].clone();
            let flag_slot = position as i64 + 1;
            bind_defaulted_parameter(module, lifted, parameter, destination, flag_slot, value_type, span)?;
            position += 2;
        }
    }
    Ok(())
}

/// Name a lifted function's arguments in order.
fn argument_parameters(arguments: &[PlanType], span: &SourceSpan) -> Vec<Parameter> {
    arguments
        .iter()
        .enumerate()
        .map(|(index, ty)| Parameter {
            name: format!("__incan_argument_{index}"),
            ty: ty.clone(),
            span: span.clone(),
        })
        .collect()
}

/// Start a lifted function's frame: its result slot, its arguments, then the enclosing frame's locals.
///
/// The enclosing locals keep their Body IR slots behind the arguments, so the lifted statements lower with the
/// ordinary local offset. An enclosing frame that is itself lifted, such as a generator-expression step whose later
/// clause lifts another, contributes its Body IR slots without its own arguments.
pub fn lifted_frame(
    builder: &Builder,
    name: &str,
    arguments: &[PlanType],
    result: PlanType,
    span: &SourceSpan,
) -> Builder {
    let count = arguments.len() as i64;
    let mut locals = Vec::with_capacity(1 + arguments.len() + builder.locals.len());
    locals.push(Local {
        ty: result,
        span: span.clone(),
    });
    locals.extend(arguments.iter().map(|argument| Local {
        ty: argument.clone(),
        span: span.clone(),
    }));
    let first = (1 + builder.local_offset) as usize;
    locals.extend(builder.locals.iter().skip(first).cloned());
    // Only the enclosing frame's own Body IR slots move into the lifted frame; a deferred default's caller locals sit
    // below its offset and stay behind.
    let offset = builder.local_offset;
    let pointer_locals = builder
        .function_pointer_locals
        .iter()
        .filter(|local| **local > offset)
        .map(|local| local - offset + count)
        .collect();
    let scoped_locals = builder
        .scoped_locals
        .iter()
        .filter(|(local, _)| *local > offset)
        .map(|(local, scope)| (local - offset + count, *scope))
        .collect();
    let mut lifted = Builder {
        program: builder.program.clone(),
        locals,
        parameter_count: count,
        blocks: Vec::new(),
        current: 0,
        source: builder.source.clone(),
        file: builder.file.clone(),
        line_starts: builder.line_starts.clone(),
        module_identity: builder.module_identity.clone(),
        callable_identity: None,
        loops: Vec::new(),
        loop_owners: Vec::new(),
        externals: Vec::new(),
        owned: Vec::new(),
        cleanup_chains: Vec::new(),
        closed: Vec::new(),
        iterators: Vec::new(),
        borrowed_items: Vec::new(),
        in_place_items: Vec::new(),
        partial_callables: Vec::new(),
        scalar_closures: Vec::new(),
        unproven_closures: Vec::new(),
        unions: builder.unions.clone(),
        local_offset: count,
        consuming_closure_frame: false,
        intrinsics: builder.intrinsics.clone(),
        generator_handle: -1,
        instances: Vec::new(),
        type_parameters: builder.type_parameters.clone(),
        type_arguments: builder.type_arguments.clone(),
        callable_type_arguments: Vec::new(),
        trait_environment: builder.trait_environment.clone(),
        function_pointer_locals: pointer_locals,
        name: name.to_owned(),
        synthesized: Vec::new(),
        materialized_closures: Vec::new(),
        step_element: None,
        anchor: None,
        iteration_locals: Vec::new(),
        step_bindings: Vec::new(),
        step_clauses: Vec::new(),
        scoped_locals,
        scope_parents: builder.scope_parents.clone(),
        loop_scopes: Vec::new(),
        loop_exit_blocks: Vec::new(),
        loop_exit_owners: Vec::new(),
        forbidden_reads: Vec::new(),
        moved_tuple_fields: Vec::new(),
        body_identity: builder.body_identity.clone(),
        checked_local_types: Vec::new(),
        initializing_decorator: builder.initializing_decorator.clone(),
        cached_decorator: builder.cached_decorator,
        callable_name_dynamic_profile: builder.callable_name_dynamic_profile,
        reference_values: Vec::new(),
        returned_view: IncanInputAccess::Inferred,
        carries_view: false,
        invocation_storage: Vec::new(),
        released: Vec::new(),
    };
    lifted.new_block(span);
    lifted
}

/// Borrow a lifted frame while carrying only its runtime signatures and checked instantiations into the enclosing
/// plan.
pub fn adopt_lifted(builder: &mut Builder, lifted: &Builder) {
    builder.externals.extend(lifted.externals.iter().cloned());
    merge_instance_requests(&mut builder.instances, &lifted.instances);
}

/// Build a generator expression as legacy's lazy adapter chain over its eagerly evaluated first source.
///
/// The first source becomes an owned generator now. Frozen collections first yield their ordered owned snapshot, as
/// legacy's eager first-source evaluation does. A lifted step receives each item only when a consumer polls and
/// returns that item's accepted element, or nothing, so no clause, filter or element runs before it is drawn. A
/// consuming closure admits only a canonical zip source and no separate step captures; reusable closures and deferred
/// defaults retain their ordinary source and capture rules.
pub fn lower_generator_expression(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: Place,
    source: &SourceOperand,
    captures: &[SourceOperand],
    body: &GeneratorBody,
    span: &SourceSpan,
) -> Result<(), String> {
    if target.projection != Projection::Whole {
        return Err("unsupported Body IR projected generator expression".into());
    }
    if builder.consuming_closure_frame && !captures.is_empty() {
        return Err("unsupported Body IR consuming closure generator expression captures".into());
    }
    if captures.len() != body.capture_locals.len() {
        return Err("invalid Body IR generator expression capture layout".into());
    }
    let item_local = first_clause_item(body)?;
    let mut evaluated = generator_source_value(builder, source, span)?;
    if builder.consuming_closure_frame && !matches!(value_type(builder, &evaluated)?, PlanType::ZipIterator(..)) {
        return Err("unsupported Body IR consuming closure generator source outside canonical zip".into());
    }
    if let PlanType::FrozenCollection(..) = value_type(builder, &evaluated)? {
        let snapshot = frozen_list_snapshot(builder, evaluated, span, false)?;
        evaluated = read_place(snapshot, true, span);
    }
    let first = source_generator(builder, evaluated, span)?;
    // The step's environment holds each capture by value, read once now, as legacy's `move` closure takes it.
    let mut operands = Vec::with_capacity(captures.len());
    let mut capture_types = Vec::with_capacity(captures.len());
    for capture in captures {
        let operand = value_operand(builder, capture, span)?;
        let ty = value_type(builder, &operand)?;
        if !step_capture_type(&ty) {
            let reason = format!(
                "unsupported Body IR generator expression capture {ty:?} in {}",
                builder.name
            );
            return Err(format!("{reason} at {}:{}", span.line, span.column));
        }
        operands.push(operand);
        capture_types.push(ty);
    }
    let bindings: Vec<i64> = body.capture_locals.iter().map(|local| local.index() as i64).collect();
    let item = source_item(builder, &first)?;
    let name = format!("__incan_generator_step_{}_{}", builder.name, builder.synthesized.len());
    let StatementKind::Loop { body: block } = &body.stmts[1].kind else {
        return Err("unsupported Body IR generator expression body shape".into());
    };
    let result = place_type(&builder.locals, &target);
    let step = GeneratorStep {
        name: &name,
        item: item.clone(),
        captures: capture_types.clone(),
        capture_locals: bindings,
        result: result.clone(),
        item_local,
    };
    let lifted = lift_generator_step(module, builder, step, block, span)?;
    synthesize(builder, lifted);
    drop_owned(builder, target.local, span);
    let callable = step_callable(builder, &name, item, result, &capture_types, operands, span);
    let local = target.local;
    construct_chain(builder, first, callable, target, span)?;
    builder.owned.push(local);
    Ok(())
}

/// Borrow a stored list source for a lazy expression, while moving independently owned sources as before.
///
/// A borrowed list's lazy cursor is `list_methods::borrowed_list_generator_call`, whose slice view refuses by name
/// until `operand_borrows::list_slice_view` is in Rust.
fn generator_source_value(builder: &mut Builder, source: &SourceOperand, span: &SourceSpan) -> Result<Operand, String> {
    if let SourceOperand::Place(read) = source {
        let root = scalar_place(builder, &read.place, span)?;
        let ty = place_type(&builder.locals, &root);
        if let Some((leaf, depth)) = list_shape(&ty) {
            if read.fact == OwnershipFact::Move && ty == PlanType::List(leaf.clone(), depth) {
                return Ok(read_place(root, true, span));
            }
            let reference = list_borrow(builder, root, false, span)?;
            let output = temporary(builder, PlanType::Generator(leaf.clone(), depth - 1), span);
            borrowed_list_generator_call(builder, reference, &leaf, depth, &output, span)?;
            builder.owned.push(output.local);
            return Ok(read_place(output, true, span));
        }
    }
    value_operand(builder, source, span)
}

/// Pass a capture-free step as its function pointer, and a capturing one as a boxed environment over its captures.
pub fn step_callable(
    builder: &mut Builder,
    name: &str,
    item: PlanType,
    result: PlanType,
    captures: &[PlanType],
    operands: Vec<Operand>,
    span: &SourceSpan,
) -> Operand {
    let signature = vec![item, result];
    if captures.is_empty() {
        return step_pointer(builder, name, signature, span);
    }
    let constructor_name = format!("{name}_new");
    let constructor = closure_constructor(
        &constructor_name,
        name,
        captures,
        &signature,
        false,
        span,
        false,
        false,
        false,
    );
    synthesize(builder, constructor);
    let environment = temporary(builder, PlanType::Closure(signature), span);
    terminate_call(
        builder,
        CalleeKind::Planned(constructor_name),
        operands,
        environment.clone(),
        span,
    );
    builder.owned.push(environment.local);
    read_place(environment, true, span)
}

/// One generator-expression step to lift: its name, item, captured environment and result.
struct GeneratorStep<'a> {
    name: &'a str,
    item: PlanType,
    captures: Vec<PlanType>,
    capture_locals: Vec<i64>,
    result: PlanType,
    item_local: i64,
}

/// Build one generator-expression step: from a first-source item to the generator of that item's accepted elements.
///
/// The step takes the expression's captures, then the item, as a capturing closure's lifted function does. It binds
/// each to the local the deferred body reads, then lowers every statement after the item's poll; a yield returns its
/// element, a later `for` clause returns the nested chain it builds, and falling through returns an empty generator.
fn lift_generator_step(
    module: &BodyIrModule,
    builder: &mut Builder,
    step: GeneratorStep<'_>,
    block: &Block,
    span: &SourceSpan,
) -> Result<Function, String> {
    let Some((leaf, depth)) = generator_shape(&step.result) else {
        return Err("unsupported Body IR generator expression type".into());
    };
    let mut arguments = step.captures;
    arguments.push(step.item);
    let mut bindings = step.capture_locals;
    bindings.push(step.item_local);
    let mut lifted = lifted_frame(builder, step.name, &arguments, step.result.clone(), span);
    lifted.step_element = Some(generator_element(&leaf, depth));
    lifted.step_clauses = later_clauses(block);
    bind_step_arguments(&mut lifted, &arguments, &bindings, span)?;
    lifted.step_bindings = bindings;
    let parameters = argument_parameters(&arguments, span);
    for statement in block.stmts.iter().skip(1) {
        lower_statement(module, &mut lifted, statement)?;
        if lifted.closed[lifted.current] {
            break;
        }
    }
    if !lifted.closed[lifted.current] {
        let filtered = step_result(&mut lifted, None, span)?;
        finish_return(&mut lifted, filtered, span)?;
    }
    // A later clause's step and environment constructor belong to the enclosing plan too.
    for function in std::mem::take(&mut lifted.synthesized) {
        synthesize(builder, function);
    }
    adopt_lifted(builder, &lifted);
    Ok(Function::new(
        step.name.to_owned(),
        parameters,
        step.result,
        lifted.locals,
        lifted.blocks,
        span.clone(),
    ))
}

/// Find the step's later clause that polls this iterator local, or -1 outside a step or for any other local.
pub fn later_clause_index(builder: &Builder, iterator: i64) -> i64 {
    if iterator < 0 {
        return -1;
    }
    builder
        .step_clauses
        .iter()
        .position(|clause| clause.0 == iterator)
        .map_or(-1, |index| index as i64)
}

/// Return a later `for` clause's own chain as the step's result, as legacy's nested `flat_map` does.
///
/// The clause's source is drawn lazily from a snapshot of a list, or from a generator the step built. Its step is
/// lifted like the first one; its environment holds the outer step's bound locals, cloned for each item it expands.
pub fn lower_later_clause(
    module: &BodyIrModule,
    builder: &mut Builder,
    clause: usize,
    body: &Block,
    span: &SourceSpan,
) -> Result<(), String> {
    let (iterator, source, assigned): LaterClause = builder.step_clauses[clause].clone();
    let item_local = clause_item(body);
    if item_local < 0 {
        return Err("unsupported Body IR generator expression later for clause poll".into());
    }
    let first = clause_source(builder, source + 1 + builder.local_offset, span)?;
    let item = source_item(builder, &first)?;
    let mut environment: Vec<i64> = Vec::new();
    let mut types = Vec::new();
    let mut operands = Vec::new();
    let bound: Vec<i64> = builder.step_bindings.iter().chain(&assigned).copied().collect();
    for local in bound {
        if local != iterator && local != source && !environment.contains(&local) {
            let operand = environment_operand(builder, local + 1 + builder.local_offset, span)?;
            environment.push(local);
            types.push(value_type(builder, &operand)?);
            operands.push(operand);
        }
    }
    let result = builder.locals[0].ty.clone();
    let name = format!("__incan_generator_step_{}_{}", builder.name, builder.synthesized.len());
    let step = GeneratorStep {
        name: &name,
        item: item.clone(),
        captures: types.clone(),
        capture_locals: environment,
        result: result.clone(),
        item_local,
    };
    let lifted = lift_generator_step(module, builder, step, body, span)?;
    synthesize(builder, lifted);
    let callable = step_callable(builder, &name, item, result.clone(), &types, operands, span);
    // The chain moves straight into the return slot, so it never joins the step's owners.
    let output = temporary(builder, result, span);
    construct_chain(builder, first, callable, output.clone(), span)?;
    finish_return(builder, read_place(output, true, span), span)
}

/// Snapshot a list, set or zip source per item, or move a generator the step built.
fn clause_source(builder: &mut Builder, slot: i64, span: &SourceSpan) -> Result<Operand, String> {
    let place = plan_place(slot, span);
    let ty = builder.locals[slot as usize].ty.clone();
    match &ty {
        PlanType::Set(_) => {
            let snapshot = cloned_collection(builder, place, span)?;
            return source_generator(builder, snapshot, span);
        }
        PlanType::ZipIterator(..) => {
            let snapshot = clone_zip(builder, place, span)?;
            return source_generator(builder, snapshot, span);
        }
        _ => {}
    }
    if let Some((leaf, depth)) = generator_shape(&ty) {
        if ty != PlanType::Generator(leaf, depth) {
            return Err("unsupported Body IR generator expression later for clause over a borrowed generator".into());
        }
        return Ok(read_place(place, true, span));
    }
    if !is_owned_list(&ty) {
        return Err(
            "unsupported Body IR generator expression later for clause source outside lists and generators".into(),
        );
    }
    let snapshot = environment_operand(builder, slot, span)?;
    source_generator(builder, snapshot, span)
}

/// Copy a scalar or clone an owned snapshot or callable so the outer step retains its reusable environment.
fn environment_operand(builder: &mut Builder, slot: i64, span: &SourceSpan) -> Result<Operand, String> {
    let place = plan_place(slot, span);
    let ty = builder.locals[slot as usize].ty.clone();
    if !step_capture_type(&ty) {
        return Err(
            "unsupported Body IR generator expression clause environment outside scalars, owned text and lists".into(),
        );
    }
    match &ty {
        PlanType::ZipIterator(..) => return clone_zip(builder, place, span),
        PlanType::Closure(_) => return target_read(builder, place, OwnershipFact::Clone, span),
        _ => {}
    }
    if ty == PlanType::String {
        let text = clone_text(builder, place, span)?;
        builder.owned.push(text.local);
        return Ok(read_place(text, true, span));
    }
    let Some((leaf, depth)) = list_shape(&ty) else {
        return Ok(read_place(place, false, span));
    };
    let reference = list_borrow(builder, place, false, span)?;
    let copy = temporary(builder, ty.clone(), span);
    list_call(
        builder,
        "clone_list",
        list_element(&leaf, depth),
        vec![PlanType::ListRef(leaf, depth)],
        ty,
        vec![reference],
        copy.clone(),
        span,
    );
    builder.owned.push(copy.local);
    Ok(read_place(copy, true, span))
}

/// Move each step argument into the Body IR local the deferred body reads; an owned one is the step's to drop.
fn bind_step_arguments(
    lifted: &mut Builder,
    arguments: &[PlanType],
    bindings: &[i64],
    span: &SourceSpan,
) -> Result<(), String> {
    let count = arguments.len() as i64;
    for (index, (argument, binding)) in arguments.iter().zip(bindings).enumerate() {
        let destination = binding + 1 + count;
        if lifted.locals[destination as usize].ty != *argument {
            return Err("unsupported Body IR generator expression item binding".into());
        }
        // A tuple item holding text, such as an `enumerate` pair over strings, is owned like text.
        let owned = *argument == PlanType::String
            || is_owned_list(argument)
            || tuple_has_owners(argument)
            || matches!(argument, PlanType::ZipIterator(..) | PlanType::Closure(_));
        let value = read_place(plan_place(index as i64 + 1, span), owned, span);
        lifted.emit(plan_assign(plan_place(destination, span), RvalueKind::Use(value), span));
        if owned {
            lifted.owned.push(destination);
        }
    }
    Ok(())
}

/// Move one closure argument into the capture or parameter local the body reads, which stores it as `storage`: a
/// capture by value, a parameter as a function frame stores it (a list as a view, a `mut` value as its borrow).
pub fn bind_closure_input(
    lifted: &mut Builder,
    destination: i64,
    argument: i64,
    ty: &PlanType,
    storage: &PlanType,
    span: &SourceSpan,
) -> Result<(), String> {
    if storage != ty {
        return Err("invalid Body IR closure argument type".into());
    }
    lifted.locals[destination as usize].ty = storage.clone();
    // Text and an owned collection, model, enum or carrier input move into the local the body reads, which then owns
    // them; an exclusive borrow moves in too, and every other input is a copy or a shared view.
    let owned = *ty == PlanType::String
        || is_nominal_type(ty)
        || is_owned_collection(ty)
        || matches!(ty, PlanType::ZipIterator(..));
    let value = read_place(plan_place(argument, span), owned || is_exclusive_borrow(ty), span);
    lifted.emit(plan_assign(plan_place(destination, span), RvalueKind::Use(value), span));
    if owned {
        lifted.owned.push(destination);
    }
    Ok(())
}
