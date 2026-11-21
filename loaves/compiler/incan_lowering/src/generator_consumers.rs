//! Generator consumers: `count`, `sum`, `fold`, `reduce`, `any`, `all`, `find` and `for_each` drain an owned
//! generator.

use incan_lang::lang::surface::iterator_methods::IteratorMethodId;
use incan_mir_plan_native::{CalleeKind, ExternalFunction, ListLeaf, Operand, Place, PlanType, SourceSpan};

use crate::calls::instantiated_external_call;
use crate::frame::{Builder, external_call, terminate_call, value_type};
use crate::generator_adapters::adapter_callable;
use crate::generators::{generator_element, generator_runtime_call, generator_shape};
use crate::pending::untranslated;

/// Report whether a checker-selected iterator operation drains its receiver into one value.
pub fn is_generator_consumer(method: IteratorMethodId) -> bool {
    matches!(
        method,
        IteratorMethodId::Count
            | IteratorMethodId::Sum
            | IteratorMethodId::Fold
            | IteratorMethodId::Reduce
            | IteratorMethodId::Any
            | IteratorMethodId::All
            | IteratorMethodId::Find
            | IteratorMethodId::ForEach
    )
}

/// Return the result a discarded consumer call still produces; `find`'s carrier comes from its destination.
pub fn consumer_result(method: IteratorMethodId, source: &PlanType) -> PlanType {
    if matches!(method, IteratorMethodId::Any | IteratorMethodId::All) {
        return PlanType::Bool;
    }
    if method == IteratorMethodId::ForEach {
        return PlanType::Unit;
    }
    if method == IteratorMethodId::Sum && *source == PlanType::Generator(ListLeaf::Float, 0) {
        return PlanType::Float;
    }
    PlanType::Int
}

/// Drain the owned source through the runtime consumer, which legacy's own iterator protocol implements.
pub fn generator_consumer(
    builder: &mut Builder,
    method: IteratorMethodId,
    source: Operand,
    arguments: Vec<Operand>,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let source_type = value_type(builder, &source)?;
    let Some((leaf, depth)) = generator_shape(&source_type) else {
        return Err("unsupported Body IR generator consumer receiver".into());
    };
    if source_type != PlanType::Generator(leaf.clone(), depth) {
        return Err("unsupported Body IR borrowed generator consumer receiver".into());
    }
    if method == IteratorMethodId::Count {
        if !arguments.is_empty() {
            return Err("invalid Body IR generator count arguments".into());
        }
        let element = generator_element(&leaf, depth);
        require_result(builder, &output, &PlanType::Int)?;
        let parameters = vec![source_type];
        generator_runtime_call(
            builder,
            "count_generator",
            element,
            parameters,
            PlanType::Int,
            vec![source],
            output,
            span,
        );
        return Ok(());
    }
    if source_type == PlanType::Generator(ListLeaf::Str, 0) && method == IteratorMethodId::Any {
        return text_any(builder, source, source_type, arguments, output, span);
    }
    if source_type == PlanType::Generator(ListLeaf::Float, 0) && method == IteratorMethodId::Sum {
        return float_sum(builder, source, source_type, arguments, output, span);
    }
    if source_type != PlanType::Generator(ListLeaf::Int, 0) {
        return Err("unsupported Body IR generator consumer element outside int".into());
    }
    int_consumer(builder, method, source, arguments, output, span)
}

/// Consume the float iterator through Rust's existing Sum implementation, preserving its order and empty identity.
fn float_sum(
    builder: &mut Builder,
    source: Operand,
    source_type: PlanType,
    arguments: Vec<Operand>,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    if !arguments.is_empty() {
        return Err("invalid Body IR generator sum arguments".into());
    }
    require_result(builder, &output, &PlanType::Float)?;
    instantiated_external_call(
        builder,
        "core::iter::Sum::sum",
        vec![PlanType::Float, PlanType::Float, source_type.clone()],
        vec![source_type],
        PlanType::Float,
        vec![source],
        output,
        span,
    )
}

/// Drain text until a by-value callable accepts one item, preserving legacy short-circuit behavior.
fn text_any(
    builder: &mut Builder,
    source: Operand,
    source_type: PlanType,
    arguments: Vec<Operand>,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let Ok([predicate]) = <[Operand; 1]>::try_from(arguments) else {
        return Err("invalid Body IR generator any arguments".into());
    };
    require_result(builder, &output, &PlanType::Bool)?;
    let signature = vec![PlanType::String, PlanType::Bool];
    let callable = adapter_callable(builder, predicate, signature.clone(), span)?;
    let pointer = PlanType::FunctionPointer(signature);
    let path = "incan_native_runtime::any_text_callable_generator";
    builder.externals.push(ExternalFunction {
        path: path.into(),
        type_arguments: vec![pointer.clone()],
        parameters: vec![source_type, pointer.clone()],
        return_type: PlanType::Bool,
        span: span.clone(),
    });
    terminate_call(
        builder,
        CalleeKind::Instantiated(path.into(), vec![pointer]),
        vec![source, callable],
        output,
        span,
    );
    Ok(())
}

/// Drain an integer generator; each function value takes its items by value, as legacy's callable ABI does.
///
/// `find` reads its Option carrier through `options::carrier_layout`, which refuses by name until it is in Rust.
fn int_consumer(
    builder: &mut Builder,
    method: IteratorMethodId,
    source: Operand,
    arguments: Vec<Operand>,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let generator = PlanType::Generator(ListLeaf::Int, 0);
    if method == IteratorMethodId::Sum {
        if !arguments.is_empty() {
            return Err("invalid Body IR generator sum arguments".into());
        }
        return consume(
            builder,
            "sum_int_generator",
            vec![generator],
            PlanType::Int,
            vec![source],
            output,
            span,
        );
    }
    if matches!(method, IteratorMethodId::Fold | IteratorMethodId::Reduce) {
        return accumulate(builder, method, source, arguments, output, span);
    }
    let Ok([function]) = <[Operand; 1]>::try_from(arguments) else {
        return Err("invalid Body IR generator consumer arguments".into());
    };
    let returns = if method == IteratorMethodId::ForEach {
        PlanType::Unit
    } else {
        PlanType::Bool
    };
    let signature = vec![PlanType::Int, returns.clone()];
    let callable = adapter_callable(builder, function, signature.clone(), span)?;
    let parameters = vec![generator, PlanType::FunctionPointer(signature)];
    if method == IteratorMethodId::Find {
        return Err(untranslated("options::carrier_layout"));
    }
    let name = int_consumer_name(method)?;
    consume(builder, name, parameters, returns, vec![source, callable], output, span)
}

/// Combine the integer items from an initial value, as `fold` and `reduce` both do.
fn accumulate(
    builder: &mut Builder,
    method: IteratorMethodId,
    source: Operand,
    arguments: Vec<Operand>,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let Ok([initial, function]) = <[Operand; 2]>::try_from(arguments) else {
        return Err("unsupported Body IR generator accumulator arguments".into());
    };
    if value_type(builder, &initial)? != PlanType::Int {
        return Err("unsupported Body IR generator accumulator arguments".into());
    }
    let signature = vec![PlanType::Int, PlanType::Int, PlanType::Int];
    let combine = adapter_callable(builder, function, signature.clone(), span)?;
    let parameters = vec![
        PlanType::Generator(ListLeaf::Int, 0),
        PlanType::Int,
        PlanType::FunctionPointer(signature),
    ];
    let name = accumulator_name(method)?;
    consume(
        builder,
        name,
        parameters,
        PlanType::Int,
        vec![source, initial, combine],
        output,
        span,
    )
}

/// Project one typed predicate consumer to its native runtime routine.
fn int_consumer_name(method: IteratorMethodId) -> Result<&'static str, String> {
    match method {
        IteratorMethodId::Any => Ok("any_int_generator"),
        IteratorMethodId::All => Ok("all_int_generator"),
        IteratorMethodId::ForEach => Ok("for_each_int_generator"),
        _ => Err("unsupported Body IR integer generator consumer".into()),
    }
}

/// Project one typed accumulator consumer to its native runtime routine.
fn accumulator_name(method: IteratorMethodId) -> Result<&'static str, String> {
    match method {
        IteratorMethodId::Fold => Ok("fold_int_generator"),
        IteratorMethodId::Reduce => Ok("reduce_int_generator"),
        _ => Err("unsupported Body IR integer generator accumulator".into()),
    }
}

/// Call one monomorphic runtime consumer into its checked destination.
fn consume(
    builder: &mut Builder,
    name: &str,
    parameters: Vec<PlanType>,
    result: PlanType,
    operands: Vec<Operand>,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    require_result(builder, &output, &result)?;
    external_call(
        builder,
        &format!("incan_native_runtime::{name}"),
        parameters,
        result,
        operands,
        output,
        span,
    );
    Ok(())
}

/// Refuse a consumer whose checked destination does not hold the runtime result.
fn require_result(builder: &Builder, output: &Place, result: &PlanType) -> Result<(), String> {
    if builder.locals[output.local as usize].ty != *result {
        return Err("unsupported Body IR generator consumer result type".into());
    }
    Ok(())
}
