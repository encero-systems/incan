//! Lazy generator layouts, producer frames and their runtime operations.

use incan_mir_plan_native::{
    Callee, CalleeKind, EnumDeclaration, ExternalFunction, Function, ListLeaf, Local, Operand, OperandKind, Parameter,
    Place, PlanType, Projection, RvalueKind, SourceSpan, StatementKind, TerminatorKind, Unwind, assign as plan_assign,
    block as plan_block, place as plan_place,
};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{Body, BodyIrModule};

use crate::checked_values::location;
use crate::classes::function_name;
use crate::collections::{leaf_type, primitive_leaf};
use crate::frame::{Builder, drop_owned, exhaust_loop, read_place, temporary, terminate_call, value_type};
use crate::function_frames::frame_parameters;
use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;
use crate::tuples::{generic_tuple_type, tuple_type, type_name};
use crate::unions::collect_unions;

/// Admit primitive generators and flat pair iterators through their checked element layouts.
pub fn generator_type(ty: &IncanType) -> Option<Result<PlanType, String>> {
    let IncanType::Generic { base, args } = ty else {
        return None;
    };
    if base != "Generator" && base != "Iterator" {
        return None;
    }
    if args.len() != 1 {
        return Some(Err(format!("unsupported Body IR type {}", type_name(ty))));
    }
    if base == "Iterator"
        && let Ok(PlanType::Tuple(elements)) = iterator_pair_type(&args[0])
        && elements.len() == 2
    {
        return Some(Ok(PlanType::ZipIterator(elements[0].clone(), elements[1].clone())));
    }
    match generator_item(&args[0]) {
        Ok((leaf, depth)) => Some(Ok(PlanType::Generator(leaf, depth))),
        Err(_) => Some(Err(format!("unsupported Body IR type {}", type_name(ty)))),
    }
}

/// Lay out a primitive item, or a list nesting one, such as the lists `batch` yields.
fn generator_item(ty: &IncanType) -> Result<(ListLeaf, i64), String> {
    if let IncanType::Generic { base, args } = ty
        && base == "List"
        && args.len() == 1
    {
        let (leaf, depth) = generator_item(&args[0])?;
        return Ok((leaf, depth + 1));
    }
    Ok((primitive_leaf(ty, false)?, 0))
}

fn iterator_pair_type(ty: &IncanType) -> Result<PlanType, String> {
    match ty {
        IncanType::Tuple(elements) => tuple_type(elements),
        IncanType::Generic { .. } => generic_tuple_type(ty),
        _ => Err("unsupported Body IR iterator element".into()),
    }
}

/// A lazy generator or its mutable reference, never a materialized list.
pub fn generator_shape(ty: &PlanType) -> Option<(ListLeaf, i64)> {
    match ty {
        PlanType::Generator(leaf, depth) | PlanType::GeneratorMutRef(leaf, depth) => Some((leaf.clone(), *depth)),
        _ => None,
    }
}

pub fn is_generator_type(ty: &PlanType) -> bool {
    matches!(ty, PlanType::Generator(..))
}

pub fn is_generator_reference(ty: &PlanType) -> bool {
    matches!(ty, PlanType::GeneratorMutRef(..))
}

/// The generator's exact yielded type; depth zero is its immediate retained leaf.
pub fn generator_element(leaf: &ListLeaf, depth: i64) -> PlanType {
    if depth == 0 {
        return leaf_type(leaf);
    }
    PlanType::List(leaf.clone(), depth)
}

/// Move the implicit yield handle into parameter slot one after lowering the ordinary Body IR frame.
pub fn producer_frame(function: &mut Function, handle: i64, leaf: &ListLeaf, depth: i64) {
    // Body IR's local mapping remains unchanged during lowering. Only the completed plan is permuted, so the implicit
    // runtime argument never impersonates a checked source local or changes evaluation order.
    let handle_slot = handle as usize;
    if handle_slot >= 1 {
        function.locals[1..=handle_slot].rotate_right(1);
    } else {
        function.locals[1] = function.locals[handle_slot].clone();
    }
    function.parameters.insert(
        0,
        Parameter {
            name: "__incan_yield".into(),
            ty: PlanType::GeneratorYield(leaf.clone(), depth),
            span: function.span.clone(),
        },
    );
    for block in &mut function.blocks {
        for statement in &mut block.statements {
            swap_statement(&mut statement.kind, handle);
        }
        swap_terminator(&mut block.terminator.kind, handle);
    }
}

/// Insert the implicit handle before source parameters while preserving every other slot order.
fn swap_index(index: i64, handle: i64) -> i64 {
    if index == handle {
        return 1;
    }
    if index > 0 && index < handle {
        return index + 1;
    }
    index
}

/// Preserve projections and provenance while permuting a frame slot.
fn swap_place(value: &mut Place, handle: i64) {
    value.local = swap_index(value.local, handle);
}

/// Keep ownership facts intact when permuting the completed producer frame.
fn swap_operand(value: &mut Operand, handle: i64) {
    if let OperandKind::Copy(place) | OperandKind::Move(place) = &mut value.kind {
        swap_place(place, handle);
    }
}

/// Permute argument slots in their retained evaluation order.
fn swap_operands(values: &mut [Operand], handle: i64) {
    for value in values {
        swap_operand(value, handle);
    }
}

/// Permute every place-bearing plan expression without changing its operation.
fn swap_rvalue(value: &mut RvalueKind, handle: i64) {
    match value {
        RvalueKind::Use(value)
        | RvalueKind::NativeAlias(value, ..)
        | RvalueKind::Unary(_, value)
        | RvalueKind::NumericCast(value, ..)
        | RvalueKind::IntToFloat(value)
        | RvalueKind::FloatToInt(value)
        | RvalueKind::BoolToInt(value)
        | RvalueKind::TagToInt(value)
        | RvalueKind::PointerCast(value, _)
        | RvalueKind::PointerAddress(value)
        | RvalueKind::UnsizeSlice(value)
        | RvalueKind::UnsizeTraitObject(value, ..)
        | RvalueKind::FrozenText(value)
        | RvalueKind::ReifyFunction(value)
        | RvalueKind::FunctionAddress(value) => swap_operand(value, handle),
        RvalueKind::Binary(_, left, right) | RvalueKind::PointerOffset(left, right) => {
            swap_operand(left, handle);
            swap_operand(right, handle);
        }
        RvalueKind::Array(values)
        | RvalueKind::Tuple(values)
        | RvalueKind::Range(values)
        | RvalueKind::Model(_, values)
        | RvalueKind::Enum(_, _, values) => swap_operands(values, handle),
        RvalueKind::Discriminant(place)
        | RvalueKind::IsVariant(place, _)
        | RvalueKind::Borrow(place)
        | RvalueKind::MutBorrow(place)
        | RvalueKind::RawAddressOf(place, _)
        | RvalueKind::ClosureObject(place) => swap_place(place, handle),
        RvalueKind::FunctionItem(callee) => swap_callee(callee, handle),
        RvalueKind::UnitFunction(_) | RvalueKind::FrozenBytes(_) | RvalueKind::FrozenCollection(..) => {}
    }
}

/// Preserve explicit storage lifetimes alongside assignments during producer permutation.
fn swap_statement(value: &mut StatementKind, handle: i64) {
    match value {
        StatementKind::Assign(place, value) => {
            swap_place(place, handle);
            swap_rvalue(&mut value.kind, handle);
        }
        StatementKind::StorageLive(index) | StatementKind::StorageDead(index) => *index = swap_index(*index, handle),
    }
}

/// A locally held callable is itself a frame operand, so its slot moves with every other operand.
fn swap_callee(value: &mut Callee, handle: i64) {
    if let CalleeKind::Value(operand) = &mut value.kind {
        swap_operand(operand, handle);
    }
}

/// Keep all control-flow targets and unwind edges while permuting operand slots.
fn swap_terminator(value: &mut TerminatorKind, handle: i64) {
    match value {
        TerminatorKind::SwitchBool(condition, ..) => swap_operand(condition, handle),
        TerminatorKind::Call(callee, arguments, destination, ..) => {
            swap_callee(callee, handle);
            swap_operands(arguments, handle);
            swap_place(destination, handle);
        }
        TerminatorKind::Drop(place, ..) => swap_place(place, handle),
        TerminatorKind::AssertOverflow(condition, _, left, right, ..) => {
            swap_operand(condition, handle);
            swap_operand(left, handle);
            swap_operand(right, handle);
        }
        _ => {}
    }
}

/// Use the frontend-reserved prefix so producer declarations and spawn callbacks cannot collide with source names.
pub fn producer_name(name: &str) -> String {
    format!("__incan_generator_producer_{name}")
}

/// Retain the canonical lazy constructor beside its separately named producer in either entrypoint; the step
/// `program_refusals::append_generator_constructor` names.
pub fn append_generator_constructor(
    constructors: &ConstructorIndex,
    functions: &mut Vec<Function>,
    module: &BodyIrModule,
    body: &Body,
    source: &str,
    file: &str,
    intrinsics: &[EnumDeclaration],
) -> Result<(), String> {
    if !body.is_generator() {
        return Ok(());
    }
    let ty = native_source_type(
        constructors,
        &module.nominal_declarations,
        &module.enum_declarations,
        &collect_unions(module),
        &body.return_type,
        intrinsics,
        "",
    )?;
    let Some((leaf, depth)) = generator_shape(&ty) else {
        return Err("unsupported Body IR generator return type".into());
    };
    let parameters = frame_parameters(constructors, module, body, source, file, intrinsics)?;
    functions.push(generator_constructor(
        &function_name(module, body),
        parameters,
        &leaf,
        depth,
        &location(&body.span, source, file),
    ));
    Ok(())
}

/// Construct the lazy runtime wrapper without executing the producer's prefix.
pub fn generator_constructor(
    name: &str,
    parameters: Vec<Parameter>,
    leaf: &ListLeaf,
    depth: i64,
    span: &SourceSpan,
) -> Function {
    let result = PlanType::Generator(leaf.clone(), depth);
    let output = plan_place(0, span);
    let call = Callee {
        kind: CalleeKind::SpawnGenerator(producer_name(name), leaf.clone(), depth),
        span: span.clone(),
    };
    let mut locals = vec![Local {
        ty: result.clone(),
        span: span.clone(),
    }];
    let mut captures = Vec::with_capacity(parameters.len());
    let mut blocks = Vec::new();
    for (index, parameter_declaration) in parameters.iter().enumerate() {
        locals.push(Local {
            ty: parameter_declaration.ty.clone(),
            span: parameter_declaration.span.clone(),
        });
        let parameter = plan_place(index as i64 + 1, span);
        captures.push(read_place(parameter.clone(), true, span));
        // Legacy validates exact f32 ingress before constructing the deferred producer.
        if parameter_declaration.ty == PlanType::F32 {
            let checked = Callee {
                kind: CalleeKind::External("incan_std_primitives::num::require_finite_f32".into()),
                span: span.clone(),
            };
            let next = blocks.len() as i64 + 1;
            blocks.push(plan_block(
                Vec::new(),
                TerminatorKind::Call(
                    checked,
                    vec![read_place(parameter.clone(), false, span)],
                    parameter,
                    next,
                    Unwind::Continue,
                ),
                span,
            ));
        }
    }
    let next = blocks.len() as i64 + 1;
    blocks.push(plan_block(
        Vec::new(),
        TerminatorKind::Call(call, captures, output, next, Unwind::Continue),
        span,
    ));
    blocks.push(plan_block(Vec::new(), TerminatorKind::Return, span));
    Function::new(name.to_owned(), parameters, result, locals, blocks, span.clone())
}

/// Send one owned value through the existing runtime's rendezvous yield handle.
pub fn generator_yield(builder: &mut Builder, value: Operand, span: &SourceSpan) -> Result<(), String> {
    if builder.generator_handle < 0 {
        return Err("unsupported Body IR Yield outside generator producer".into());
    }
    let handle = plan_place(builder.generator_handle, span);
    let PlanType::GeneratorYield(leaf, depth) = builder.locals[handle.local as usize].ty.clone() else {
        return Err("invalid native generator yield handle".into());
    };
    if value_type(builder, &value)? != generator_element(&leaf, depth) {
        return Err("unsupported Body IR generator yield coercion".into());
    }
    let reference = temporary(builder, PlanType::GeneratorYieldRef(leaf.clone(), depth), span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(handle), span));
    let output = temporary(builder, PlanType::Unit, span);
    terminate_call(
        builder,
        CalleeKind::YieldGenerator(leaf, depth),
        vec![read_place(reference, false, span), value],
        output,
        span,
    );
    Ok(())
}

/// Consume the runtime generator once, preserving its lazy producer and exact yield order.
pub fn generator_collect(
    builder: &mut Builder,
    receiver: Place,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let ty = builder.locals[receiver.local as usize].ty.clone();
    let Some((leaf, depth)) = generator_shape(&ty) else {
        return Err("unsupported Body IR generator collect receiver".into());
    };
    if ty != PlanType::Generator(leaf.clone(), depth) {
        return Err("unsupported Body IR collect of borrowed generator".into());
    }
    terminate_call(
        builder,
        CalleeKind::CollectGenerator(leaf, depth),
        vec![read_place(receiver, true, span)],
        output,
        span,
    );
    Ok(())
}

/// Borrow the live generator or reborrow an existing iterator alias without copying its mutable reference.
pub fn generator_reference(builder: &mut Builder, receiver: Place, span: &SourceSpan) -> Result<Operand, String> {
    let ty = builder.locals[receiver.local as usize].ty.clone();
    let Some((leaf, depth)) = generator_shape(&ty) else {
        return Err("unsupported Body IR generator borrow receiver".into());
    };
    let mut source = receiver;
    if ty == PlanType::GeneratorMutRef(leaf.clone(), depth) {
        source.projection = Projection::Deref(PlanType::Generator(leaf.clone(), depth));
    }
    let reference = temporary(builder, PlanType::GeneratorMutRef(leaf, depth), span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::MutBorrow(source), span));
    Ok(read_place(reference, true, span))
}

/// Retain one exact Incan-runtime signature, generic over the generator element, alongside its call.
#[allow(clippy::too_many_arguments)] // The Incan helper's eight parts of one runtime call.
pub fn generator_runtime_call(
    builder: &mut Builder,
    name: &str,
    element: PlanType,
    parameters: Vec<PlanType>,
    result: PlanType,
    operands: Vec<Operand>,
    output: Place,
    span: &SourceSpan,
) {
    let path = format!("incan_native_runtime::{name}");
    builder.externals.push(ExternalFunction {
        path: path.clone(),
        type_arguments: vec![element.clone()],
        parameters,
        return_type: result,
        span: span.clone(),
    });
    terminate_call(
        builder,
        CalleeKind::Instantiated(path, vec![element]),
        operands,
        output,
        span,
    );
}

/// Poll one lazy item, branching to the existing loop exit on exhaustion without replaying the prefix.
pub fn generator_next(builder: &mut Builder, receiver: Place, output: &Place, span: &SourceSpan) -> Result<(), String> {
    if builder.loops.is_empty() {
        return Err("invalid Body IR generator poll outside loop".into());
    }
    let Some((leaf, depth)) = generator_shape(&builder.locals[receiver.local as usize].ty) else {
        return Err("unsupported Body IR generator poll receiver".into());
    };
    let element = generator_element(&leaf, depth);
    let storage_type = PlanType::List(leaf.clone(), depth + 1);
    let storage = temporary(builder, storage_type.clone(), span);
    generator_runtime_call(
        builder,
        "empty_list",
        element.clone(),
        Vec::new(),
        storage_type,
        Vec::new(),
        storage.clone(),
        span,
    );
    builder.owned.push(storage.local);
    let buffer_type = PlanType::ListMutRef(leaf.clone(), depth + 1);
    let item_reference = temporary(builder, buffer_type.clone(), span);
    builder.emit(plan_assign(
        item_reference.clone(),
        RvalueKind::MutBorrow(storage.clone()),
        span,
    ));
    let reference = generator_reference(builder, receiver, span)?;
    let available = temporary(builder, PlanType::Bool, span);
    generator_runtime_call(
        builder,
        "poll_generator",
        element.clone(),
        vec![PlanType::GeneratorMutRef(leaf, depth), buffer_type.clone()],
        PlanType::Bool,
        vec![reference, read_place(item_reference, true, span)],
        available.clone(),
        span,
    );
    let exhausted = builder.new_block(span);
    let selected = builder.new_block(span);
    // The selected branch resumes from the owners live before the exhausted branch drops the poll buffer.
    let owners = builder.owned.clone();
    builder.finish(
        TerminatorKind::SwitchBool(read_place(available, false, span), exhausted, selected),
        span,
    );
    // Each branch owns the initialized poll buffer independently; even exhaustion must drop it.
    builder.select(exhausted);
    drop_owned(builder, storage.local, span);
    exhaust_loop(builder, span);
    builder.select(selected);
    builder.owned = owners;
    drop_owned(builder, output.local, span);
    let final_reference = temporary(builder, buffer_type.clone(), span);
    builder.emit(plan_assign(
        final_reference.clone(),
        RvalueKind::MutBorrow(storage.clone()),
        span,
    ));
    generator_runtime_call(
        builder,
        "pop_list",
        element.clone(),
        vec![buffer_type],
        element.clone(),
        vec![read_place(final_reference, true, span)],
        output.clone(),
        span,
    );
    if element == PlanType::String || depth > 0 {
        builder.owned.push(output.local);
    }
    drop_owned(builder, storage.local, span);
    Ok(())
}
