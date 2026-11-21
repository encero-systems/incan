//! Generator expressions as legacy's lazy adapter chain: an eager first source expanded item by item by a step.

use incan_mir_plan_native::{
    Callee, CalleeKind, ExternalFunction, ListLeaf, Operand, Place, PlanType, RvalueKind, SourceSpan,
    assign as plan_assign, tuple_element_type,
};
use incan_semantics_core::body_ir::{GeneratorBody, Operand as SourceOperand, Rvalue, StatementKind};

use crate::closures::closure_capture_type;
use crate::frame::{Builder, read_place, temporary, terminate_call, value_type};
use crate::generators::{generator_element, generator_runtime_call, generator_shape};
use crate::lists::{is_builtin_iteration, is_owned_list, list_element, list_shape};

/// Return the Body IR local the deferred body polls each first-source item into.
///
/// Body IR materializes a builtin iterator from the captured first source, then loops, polling it first; everything
/// after that poll is one item's step. Any other deferred shape keeps a named refusal.
pub fn first_clause_item(body: &GeneratorBody) -> Result<i64, String> {
    if body.stmts.len() != 2 {
        return Err("unsupported Body IR generator expression body shape".into());
    }
    let iterator = source_iterator(body)?;
    let StatementKind::Loop { body: block } = &body.stmts[1].kind else {
        return Err("unsupported Body IR generator expression body shape".into());
    };
    let Some(first) = block.stmts.first() else {
        return Err("unsupported Body IR generator expression without a source poll".into());
    };
    polled_item(&first.kind, iterator)
}

/// Require the builtin iterator materialized from exactly the captured first source.
fn source_iterator(body: &GeneratorBody) -> Result<i64, String> {
    let refusal = "unsupported Body IR generator expression source protocol";
    let StatementKind::Assign { place, rvalue } = &body.stmts[0].kind else {
        return Err(refusal.into());
    };
    if !place.projection.is_empty() || !reads_local(rvalue, body.source_local.index() as i64) {
        return Err(refusal.into());
    }
    match place.local_id() {
        Some(local) => Ok(local.index() as i64),
        None => Err(refusal.into()),
    }
}

/// Recognize a plain whole read of one Body IR local.
fn reads_local(value: &Rvalue, local: i64) -> bool {
    match value {
        Rvalue::Use(operand) => operand_reads(operand, local),
        _ => false,
    }
}

/// Recognize an operand reading one whole Body IR local.
fn operand_reads(value: &SourceOperand, local: i64) -> bool {
    match value {
        SourceOperand::Place(read) => {
            read.place.projection.is_empty() && read.place.local_id().is_some_and(|found| found.index() as i64 == local)
        }
        SourceOperand::Constant(_) => false,
    }
}

/// Read the builtin poll of the source iterator into one whole item local.
fn polled_item(kind: &StatementKind, iterator: i64) -> Result<i64, String> {
    let refusal = "unsupported Body IR generator expression source poll";
    let StatementKind::IterNext {
        destination,
        iterator: polled,
        protocol,
    } = kind
    else {
        return Err("unsupported Body IR generator expression without a source poll".into());
    };
    if !is_builtin_iteration(protocol) || !destination.projection.is_empty() || !operand_reads(polled, iterator) {
        return Err(refusal.into());
    }
    match destination.local_id() {
        Some(local) => Ok(local.index() as i64),
        None => Err(refusal.into()),
    }
}

/// Hand the eagerly evaluated first source to the chain as an owned generator.
///
/// Lists and sets yield their owned snapshots, and builtin `zip` over lists yields its snapshot's pairs.
pub fn source_generator(builder: &mut Builder, value: Operand, span: &SourceSpan) -> Result<Operand, String> {
    let ty = value_type(builder, &value)?;
    match &ty {
        PlanType::Set(leaf) => {
            let result = PlanType::Generator(leaf.clone(), 0);
            let element = generator_element(leaf, 0);
            return owned_source(builder, "set_generator", element, ty.clone(), result, value, span);
        }
        PlanType::ZipIterator(left, right) => {
            let result = PlanType::Generator(ListLeaf::Tuple(vec![left.clone(), right.clone()]), 0);
            let output = temporary(builder, result.clone(), span);
            let path = "incan_native_runtime::zip_lists_generator";
            let types = vec![tuple_element_type(left), tuple_element_type(right)];
            builder.externals.push(ExternalFunction {
                path: path.into(),
                type_arguments: types.clone(),
                parameters: vec![ty.clone()],
                return_type: result,
                span: span.clone(),
            });
            terminate_call(
                builder,
                CalleeKind::Instantiated(path.into(), types),
                vec![value],
                output.clone(),
                span,
            );
            builder.owned.push(output.local);
            return Ok(read_place(output, true, span));
        }
        _ => {}
    }
    if let Some((leaf, depth)) = generator_shape(&ty) {
        if ty != PlanType::Generator(leaf, depth) {
            return Err("unsupported Body IR borrowed generator expression source".into());
        }
        return Ok(value);
    }
    let Some((leaf, depth)) = list_shape(&ty) else {
        return Err("unsupported Body IR generator expression source".into());
    };
    if ty != PlanType::List(leaf.clone(), depth) {
        return Err("unsupported Body IR borrowed generator expression source".into());
    }
    let result = PlanType::Generator(leaf.clone(), depth - 1);
    let element = list_element(&leaf, depth);
    owned_source(builder, "list_generator", element, ty, result, value, span)
}

/// Wrap one owned collection snapshot in its runtime generator, owned by the frame until the chain consumes it.
fn owned_source(
    builder: &mut Builder,
    name: &str,
    element: PlanType,
    ty: PlanType,
    result: PlanType,
    value: Operand,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let output = temporary(builder, result.clone(), span);
    generator_runtime_call(
        builder,
        name,
        element,
        vec![ty],
        result,
        vec![value],
        output.clone(),
        span,
    );
    builder.owned.push(output.local);
    Ok(read_place(output, true, span))
}

/// Read the item type the source generator yields to each step.
pub fn source_item(builder: &Builder, source: &Operand) -> Result<PlanType, String> {
    match generator_shape(&value_type(builder, source)?) {
        Some((leaf, depth)) => Ok(generator_element(&leaf, depth)),
        None => Err("invalid native generator expression source".into()),
    }
}

/// Name the runtime chain family for a first-source item the step takes by value.
///
/// A tuple, owned list, or model item uses the chain generic over its item, preserving the step's by-value ownership.
fn item_family(item: &PlanType) -> Result<&'static str, String> {
    match item {
        PlanType::Int => Ok("int"),
        PlanType::Float => Ok("float"),
        PlanType::Bool => Ok("bool"),
        PlanType::String => Ok("str"),
        PlanType::Tuple(_) | PlanType::Model(..) => Ok("item"),
        _ if is_owned_list(item) => Ok("item"),
        _ => Err(
            "unsupported Body IR generator expression item outside scalars, text, tuples, owned lists and models"
                .into(),
        ),
    }
}

/// Admit reusable step captures: Copy scalars and function pointers, or owned text, list, callable and zip snapshots.
///
/// Owned values are cloned out of the environment for each call, because the step runs once per item. A zip clone
/// preserves both cursors without polling; borrowed zip handles and arbitrary generators remain refused.
pub fn step_capture_type(ty: &PlanType) -> bool {
    match ty {
        PlanType::Model(..) => false,
        PlanType::FunctionPointer(_) | PlanType::Closure(_) | PlanType::ZipIterator(..) => true,
        PlanType::Tuple(elements) => {
            // Destructured loop items remain bound as a tuple in each nested step. Only scalar Copy tuples cross here.
            elements.iter().all(|element| {
                let item = tuple_element_type(element);
                match item {
                    PlanType::Tuple(_) => step_capture_type(&item),
                    _ => closure_capture_type(&item, false) && item != PlanType::String,
                }
            })
        }
        _ => closure_capture_type(ty, false) || is_owned_list(ty),
    }
}

/// Reify the lifted step as the function pointer a capture-free chain calls.
pub fn step_pointer(builder: &mut Builder, step: &str, signature: Vec<PlanType>, span: &SourceSpan) -> Operand {
    let function = temporary(
        builder,
        PlanType::FunctionItem(step.to_owned(), signature.clone()),
        span,
    );
    let item = Callee {
        kind: CalleeKind::Planned(step.to_owned()),
        span: span.clone(),
    };
    builder.emit(plan_assign(function.clone(), RvalueKind::FunctionItem(item), span));
    let pointer = temporary(builder, PlanType::FunctionPointer(signature), span);
    builder.emit(plan_assign(
        pointer.clone(),
        RvalueKind::ReifyFunction(read_place(function, false, span)),
        span,
    ));
    read_place(pointer, false, span)
}

/// Expand the source through the step; nothing is polled until a consumer asks.
///
/// A scalar item's capture-free step is the function pointer legacy's own chain calls. A text item's step, or one
/// holding captures in its boxed environment, goes through the chain that takes any callable of the item. A tuple or
/// owned list item's step goes through the callable chain generic over the item, preserving a boxed environment's
/// exact by-value signature as well as a capture-free function pointer.
pub fn construct_chain(
    builder: &mut Builder,
    source: Operand,
    step: Operand,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let item = source_item(builder, &source)?;
    let family = item_family(&item)?;
    let result = builder.locals[output.local as usize].ty.clone();
    let callable = value_type(builder, &step)?;
    let source_type = value_type(builder, &source)?;
    let Some((leaf, depth)) = generator_shape(&result) else {
        return Err("unsupported Body IR generator expression type".into());
    };
    let element = generator_element(&leaf, depth);
    let signature = vec![item.clone(), result.clone()];
    if family == "item" {
        if callable != PlanType::FunctionPointer(signature.clone()) && callable != PlanType::Closure(signature) {
            return Err("invalid Body IR compound generator step signature".into());
        }
        let types = vec![item, element, callable.clone()];
        let path = "incan_native_runtime::item_callable_step_generator";
        return instantiated_chain(
            builder,
            path,
            types,
            source_type,
            callable,
            result,
            source,
            step,
            output,
            span,
        );
    }
    if callable == PlanType::FunctionPointer(signature) && family != "str" {
        let name = format!("{family}_step_generator");
        let parameters = vec![source_type, callable];
        generator_runtime_call(
            builder,
            &name,
            element,
            parameters,
            result,
            vec![source, step],
            output,
            span,
        );
        return Ok(());
    }
    let path = format!("incan_native_runtime::{family}_callable_step_generator");
    let types = vec![element, callable.clone()];
    instantiated_chain(
        builder,
        &path,
        types,
        source_type,
        callable,
        result,
        source,
        step,
        output,
        span,
    )
}

/// Declare and call one chain constructor instantiated over its item, element and callable types.
#[allow(clippy::too_many_arguments)] // The chain's path, instantiation, both inputs with their types, result and span.
fn instantiated_chain(
    builder: &mut Builder,
    path: &str,
    types: Vec<PlanType>,
    source_type: PlanType,
    callable: PlanType,
    result: PlanType,
    source: Operand,
    step: Operand,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    builder.externals.push(ExternalFunction {
        path: path.to_owned(),
        type_arguments: types.clone(),
        parameters: vec![source_type, callable],
        return_type: result,
        span: span.clone(),
    });
    terminate_call(
        builder,
        CalleeKind::Instantiated(path.to_owned(), types),
        vec![source, step],
        output,
        span,
    );
    Ok(())
}

/// Build a step's return value: a generator over its one accepted element, or an empty one for a filtered item.
///
/// The value moves straight into the return slot, so it never joins the frame's owners.
pub fn step_result(builder: &mut Builder, accepted: Option<Operand>, span: &SourceSpan) -> Result<Operand, String> {
    let result = builder.locals[0].ty.clone();
    let Some((leaf, depth)) = generator_shape(&result) else {
        return Err("invalid native generator expression step result".into());
    };
    let element = generator_element(&leaf, depth);
    let output = temporary(builder, result.clone(), span);
    match accepted {
        Some(value) => {
            if value_type(builder, &value)? != element {
                return Err("unsupported Body IR generator expression element coercion".into());
            }
            let parameters = vec![element.clone()];
            generator_runtime_call(
                builder,
                "once_generator",
                element,
                parameters,
                result,
                vec![value],
                output.clone(),
                span,
            );
        }
        None => {
            generator_runtime_call(
                builder,
                "empty_generator",
                element,
                Vec::new(),
                result,
                Vec::new(),
                output.clone(),
                span,
            );
        }
    }
    Ok(read_place(output, true, span))
}
