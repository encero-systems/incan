//! Lazy iterator adapters chained onto an owned generator through the runtime's own Incan iterator surface.

use incan_lang::lang::surface::iterator_methods::IteratorMethodId;
use incan_mir_plan_native::{
    Callee, CalleeKind, ExternalFunction, Function, ListLeaf, Local, Operand, OperandKind, Parameter, Place, PlanType,
    RvalueKind, SourceSpan, TerminatorKind, TupleElement, Unwind, assign as plan_assign, block as plan_block,
    place as plan_place,
};

use crate::frame::{Builder, external_call, read_place, temporary, terminate_call, value_type};
use crate::generators::{generator_element, generator_runtime_call, generator_shape};
use crate::pending::untranslated;

/// Report whether a checker-selected iterator operation chains a lazy adapter.
pub fn is_generator_adapter(method: IteratorMethodId) -> bool {
    matches!(
        method,
        IteratorMethodId::Take
            | IteratorMethodId::Skip
            | IteratorMethodId::Batch
            | IteratorMethodId::Map
            | IteratorMethodId::Filter
            | IteratorMethodId::TakeWhile
            | IteratorMethodId::SkipWhile
            | IteratorMethodId::FlatMap
            | IteratorMethodId::Chain
            | IteratorMethodId::Enumerate
            | IteratorMethodId::Zip
    )
}

/// Chain one lazy adapter onto the owned source; nothing is drawn until a consumer polls the result.
pub fn generator_adapter(
    builder: &mut Builder,
    method: IteratorMethodId,
    source: Operand,
    arguments: Vec<Operand>,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let source_type = value_type(builder, &source)?;
    source_element(&source_type)?;
    if method == IteratorMethodId::Enumerate {
        if !arguments.is_empty() {
            return Err("invalid Body IR generator enumerate arguments".into());
        }
        return enumerate_adapter(builder, source, source_type, output, span);
    }
    let Ok([argument]) = <[Operand; 1]>::try_from(arguments) else {
        return Err("invalid Body IR generator adapter arguments".into());
    };
    match method {
        IteratorMethodId::Take | IteratorMethodId::Skip | IteratorMethodId::Batch => {
            count_adapter(builder, method, source, source_type, argument, output, span)
        }
        IteratorMethodId::Chain => chain_adapter(builder, source, source_type, argument, output, span),
        IteratorMethodId::Zip => zip_generators(builder, source, argument, output, span),
        IteratorMethodId::Map => map_adapter(builder, source, source_type, argument, output, span),
        IteratorMethodId::FlatMap => flat_map_adapter(builder, source, source_type, argument, output, span),
        _ => predicate_adapter(builder, method, source, source_type, argument, output, span),
    }
}

/// Keep a prefix, drop a prefix, or group items in lists of `count`, as legacy's runtime adapters do.
fn count_adapter(
    builder: &mut Builder,
    method: IteratorMethodId,
    source: Operand,
    source_type: PlanType,
    count: Operand,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    if value_type(builder, &count)? != PlanType::Int {
        return Err("unsupported Body IR generator count-adapter type".into());
    }
    let element = source_element(&source_type)?;
    let mut result = source_type.clone();
    if method == IteratorMethodId::Batch {
        let (leaf, depth) = require_generator(&source_type)?;
        result = PlanType::Generator(leaf, depth + 1);
    }
    if builder.locals[output.local as usize].ty != result {
        return Err("unsupported Body IR generator count-adapter types".into());
    }
    let name = match method {
        IteratorMethodId::Skip => "skip_generator",
        IteratorMethodId::Batch => "batch_generator",
        _ => "take_generator",
    };
    let parameters = vec![source_type, PlanType::Int];
    generator_runtime_call(
        builder,
        name,
        element,
        parameters,
        result,
        vec![source, count],
        output,
        span,
    );
    Ok(())
}

/// Draw the second owned generator's items after the first is exhausted.
fn chain_adapter(
    builder: &mut Builder,
    source: Operand,
    source_type: PlanType,
    second: Operand,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    if owned_generator(builder, &second)? != source_type || builder.locals[output.local as usize].ty != source_type {
        return Err("unsupported Body IR generator chain types".into());
    }
    let element = source_element(&source_type)?;
    generator_runtime_call(
        builder,
        "chain_generators",
        element,
        vec![source_type.clone(), source_type.clone()],
        source_type,
        vec![source, second],
        output,
        span,
    );
    Ok(())
}

/// Keep the items a function value accepts, or the prefix it accepts, or what follows that prefix.
fn predicate_adapter(
    builder: &mut Builder,
    method: IteratorMethodId,
    source: Operand,
    source_type: PlanType,
    predicate: Operand,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    if builder.locals[output.local as usize].ty != source_type {
        return Err("unsupported Body IR generator predicate-adapter types".into());
    }
    let element = source_element(&source_type)?;
    let family = element_family(&element)?;
    if method != IteratorMethodId::Filter && family != "int" {
        return Err("unsupported Body IR generator predicate-adapter element outside int".into());
    }
    let signature = vec![element, PlanType::Bool];
    let pointer = adapter_callable(builder, predicate, signature.clone(), span)?;
    let operation = match method {
        IteratorMethodId::TakeWhile => "take_while",
        IteratorMethodId::SkipWhile => "skip_while",
        _ => "filter",
    };
    external_call(
        builder,
        &format!("incan_native_runtime::{operation}_{family}_generator"),
        vec![source_type.clone(), PlanType::FunctionPointer(signature)],
        source_type,
        vec![source, pointer],
        output,
        span,
    );
    Ok(())
}

/// Transform each item with a function value when it is polled.
///
/// A model item's mapper is synthesized and called through `closure_lifting::synthesize` and
/// `closure_lifting::step_callable`, which refuse by name until they are in Rust.
fn map_adapter(
    builder: &mut Builder,
    source: Operand,
    source_type: PlanType,
    transform: Operand,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let result = builder.locals[output.local as usize].ty.clone();
    let mapped = source_element(&result)?;
    let element = source_element(&source_type)?;
    let signature = vec![element.clone(), mapped.clone()];
    let pointer = adapter_callable(builder, transform, signature.clone(), span)?;
    if let PlanType::Model(..) = element {
        return Err(untranslated("closure_lifting::synthesize"));
    }
    if element == PlanType::String {
        let name = "map_text_callable_generator";
        return text_callable_adapter(
            builder,
            name,
            source,
            source_type,
            pointer,
            mapped,
            result,
            output,
            span,
        );
    }
    let name = format!("map_{}_generator", element_family(&element)?);
    generator_runtime_call(
        builder,
        &name,
        mapped,
        vec![source_type, PlanType::FunctionPointer(signature)],
        result,
        vec![source, pointer],
        output,
        span,
    );
    Ok(())
}

/// Call the retained mapper once for an owned item, then yield its owned result through a single-item generator.
pub fn mapping_step(
    name: &str,
    element: &PlanType,
    mapped: &PlanType,
    result: &PlanType,
    span: &SourceSpan,
) -> Function {
    let pointer = PlanType::FunctionPointer(vec![element.clone(), mapped.clone()]);
    let parameter = |name: &str, ty: &PlanType| Parameter {
        name: name.into(),
        ty: ty.clone(),
        span: span.clone(),
    };
    let parameters = vec![parameter("transform", &pointer), parameter("item", element)];
    let local = |ty: &PlanType| Local {
        ty: ty.clone(),
        span: span.clone(),
    };
    let locals = vec![local(result), local(&pointer), local(element), local(mapped)];
    let call = TerminatorKind::Call(
        Callee {
            kind: CalleeKind::Value(read_place(plan_place(1, span), false, span)),
            span: span.clone(),
        },
        vec![read_place(plan_place(2, span), true, span)],
        plan_place(3, span),
        1,
        Unwind::Continue,
    );
    let yield_item = TerminatorKind::Call(
        Callee {
            kind: CalleeKind::Instantiated("incan_native_runtime::once_generator".into(), vec![mapped.clone()]),
            span: span.clone(),
        },
        vec![read_place(plan_place(3, span), true, span)],
        plan_place(0, span),
        2,
        Unwind::Continue,
    );
    let blocks = vec![
        plan_block(Vec::new(), call, span),
        plan_block(Vec::new(), yield_item, span),
        plan_block(Vec::new(), TerminatorKind::Return, span),
    ];
    Function::new(
        name.to_owned(),
        parameters,
        result.clone(),
        locals,
        blocks,
        span.clone(),
    )
}

/// Expand each integer item with a function value and draw the expansion's items lazily.
///
/// An expansion is a list, or a generator whose own items are drawn only as the result is polled.
fn flat_map_adapter(
    builder: &mut Builder,
    source: Operand,
    source_type: PlanType,
    expand: Operand,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let result = builder.locals[output.local as usize].ty.clone();
    let (leaf, depth) = require_generator(&result)?;
    let mapped = generator_element(&leaf, depth);
    if source_type == PlanType::Generator(ListLeaf::Str, 0) {
        let signature = vec![PlanType::String, PlanType::List(leaf, depth + 1)];
        let pointer = adapter_callable(builder, expand, signature, span)?;
        let name = "flat_map_text_callable_generator";
        return text_callable_adapter(
            builder,
            name,
            source,
            source_type,
            pointer,
            mapped,
            result,
            output,
            span,
        );
    }
    if source_type != PlanType::Generator(ListLeaf::Int, 0) {
        return Err("unsupported Body IR generator flat_map element outside int".into());
    }
    if expansion_result(builder, &expand)? == PlanType::Set(ListLeaf::Int) {
        if result != source_type {
            return Err("unsupported Body IR set expansion outside integer generator".into());
        }
        let signature = vec![PlanType::Int, PlanType::Set(ListLeaf::Int)];
        let pointer = adapter_callable(builder, expand, signature.clone(), span)?;
        external_call(
            builder,
            "incan_native_runtime::flat_map_int_set_generator",
            vec![source_type, PlanType::FunctionPointer(signature)],
            result,
            vec![source, pointer],
            output,
            span,
        );
        return Ok(());
    }
    let (signature, name) = if expands_to_generators(builder, &expand, &result)? {
        (vec![PlanType::Int, result.clone()], "flat_map_int_generators")
    } else {
        (
            vec![PlanType::Int, PlanType::List(leaf, depth + 1)],
            "flat_map_int_generator",
        )
    };
    let pointer = adapter_callable(builder, expand, signature.clone(), span)?;
    generator_runtime_call(
        builder,
        name,
        mapped,
        vec![source_type, PlanType::FunctionPointer(signature)],
        result,
        vec![source, pointer],
        output,
        span,
    );
    Ok(())
}

/// The retained signature of a function value, when it is a function pointer or item.
fn callable_signature(ty: &PlanType) -> Option<&[PlanType]> {
    match ty {
        PlanType::FunctionPointer(signature) | PlanType::FunctionItem(_, signature) => Some(signature),
        _ => None,
    }
}

/// Read the retained callback result without assuming every flat_map expansion is a list.
fn expansion_result(builder: &Builder, expand: &Operand) -> Result<PlanType, String> {
    match callable_signature(&value_type(builder, expand)?) {
        Some([_, result]) => Ok(result.clone()),
        _ => Err("unsupported Body IR generator adapter callable signature".into()),
    }
}

/// Instantiate the runtime's by-value text callable boundary with the checked result and function-pointer types.
#[allow(clippy::too_many_arguments)] // The adapter's runtime name, both inputs with their types, result and output.
fn text_callable_adapter(
    builder: &mut Builder,
    name: &str,
    source: Operand,
    source_type: PlanType,
    callable: Operand,
    element: PlanType,
    result: PlanType,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let pointer = value_type(builder, &callable)?;
    let path = format!("incan_native_runtime::{name}");
    let types = vec![element, pointer.clone()];
    builder.externals.push(ExternalFunction {
        path: path.clone(),
        type_arguments: types.clone(),
        parameters: vec![source_type, pointer],
        return_type: result,
        span: span.clone(),
    });
    terminate_call(
        builder,
        CalleeKind::Instantiated(path, types),
        vec![source, callable],
        output,
        span,
    );
    Ok(())
}

/// Report whether a `flat_map` function value returns the result's own generator type rather than a list.
fn expands_to_generators(builder: &Builder, expand: &Operand, result: &PlanType) -> Result<bool, String> {
    Ok(matches!(
        callable_signature(&value_type(builder, expand)?),
        Some([_, returned]) if returned == result
    ))
}

/// Pair each item with its zero-based position, drawing it only when polled.
fn enumerate_adapter(
    builder: &mut Builder,
    source: Operand,
    source_type: PlanType,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let element = source_element(&source_type)?;
    let result = pair_output(builder, &output, TupleElement::Int, pair_component(&element)?)?;
    let parameters = vec![source_type];
    generator_runtime_call(
        builder,
        "enumerate_generator",
        element,
        parameters,
        result,
        vec![source],
        output,
        span,
    );
    Ok(())
}

/// Pair the items of two owned generators until either is exhausted, as `zip` over iterators does.
pub fn zip_generators(
    builder: &mut Builder,
    first: Operand,
    second: Operand,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let first_type = owned_generator(builder, &first)?;
    let second_type = owned_generator(builder, &second)?;
    let left = source_element(&first_type)?;
    let right = source_element(&second_type)?;
    let result = pair_output(builder, &output, pair_component(&left)?, pair_component(&right)?)?;
    let path = "incan_native_runtime::zip_generators";
    let types = vec![left, right];
    builder.externals.push(ExternalFunction {
        path: path.into(),
        type_arguments: types.clone(),
        parameters: vec![first_type, second_type],
        return_type: result,
        span: span.clone(),
    });
    terminate_call(
        builder,
        CalleeKind::Instantiated(path.into(), types),
        vec![first, second],
        output,
        span,
    );
    Ok(())
}

/// Require the generator of pairs the frame laid out for a pair adapter's checked destination.
fn pair_output(builder: &Builder, output: &Place, left: TupleElement, right: TupleElement) -> Result<PlanType, String> {
    let result = PlanType::Generator(ListLeaf::Tuple(vec![left, right]), 0);
    if builder.locals[output.local as usize].ty != result {
        return Err("unsupported Body IR generator pair destination".into());
    }
    Ok(result)
}

/// Lay out one scalar or text pair component.
fn pair_component(element: &PlanType) -> Result<TupleElement, String> {
    match element {
        PlanType::Int => Ok(TupleElement::Int),
        PlanType::Float => Ok(TupleElement::Float),
        PlanType::Bool => Ok(TupleElement::Bool),
        PlanType::String => Ok(TupleElement::String),
        _ => Err("unsupported Body IR generator pair component outside scalars and text".into()),
    }
}

/// Require a moved, owned generator argument; a borrowed one would be read twice.
fn owned_generator(builder: &Builder, value: &Operand) -> Result<PlanType, String> {
    let ty = value_type(builder, value)?;
    match value.kind {
        OperandKind::Move(_) => {
            source_element(&ty)?;
            Ok(ty)
        }
        _ => Err("unsupported Body IR generator argument without a move".into()),
    }
}

/// Read an owned generator's item layout.
fn require_generator(ty: &PlanType) -> Result<(ListLeaf, i64), String> {
    let Some((leaf, depth)) = generator_shape(ty) else {
        return Err("unsupported Body IR generator adapter result outside generators".into());
    };
    if *ty != PlanType::Generator(leaf.clone(), depth) {
        return Err("unsupported Body IR borrowed generator adapter value".into());
    }
    Ok((leaf, depth))
}

/// Read the element an owned generator yields.
fn source_element(ty: &PlanType) -> Result<PlanType, String> {
    let (leaf, depth) = require_generator(ty)?;
    Ok(generator_element(&leaf, depth))
}

/// Name the runtime adapter whose function value takes this element by value, as legacy's callable ABI does.
fn element_family(element: &PlanType) -> Result<&'static str, String> {
    match element {
        PlanType::Int => Ok("int"),
        PlanType::Float => Ok("float"),
        PlanType::Bool => Ok("bool"),
        _ => Err("unsupported Body IR generator adapter element outside int, float and bool".into()),
    }
}

/// Pass a function value as the pointer legacy's adapter calls; a capturing closure keeps its named refusal.
pub fn adapter_callable(
    builder: &mut Builder,
    value: Operand,
    signature: Vec<PlanType>,
    span: &SourceSpan,
) -> Result<Operand, String> {
    match value_type(builder, &value)? {
        PlanType::FunctionPointer(found) => {
            if found != signature {
                return Err("unsupported Body IR generator adapter callable signature".into());
            }
            Ok(value)
        }
        PlanType::FunctionItem(_, found) => {
            if found != signature {
                return Err("unsupported Body IR generator adapter callable signature".into());
            }
            let pointer = temporary(builder, PlanType::FunctionPointer(signature), span);
            builder.emit(plan_assign(pointer.clone(), RvalueKind::ReifyFunction(value), span));
            Ok(read_place(pointer, false, span))
        }
        _ => Err("unsupported Body IR generator adapter callable outside function values".into()),
    }
}
