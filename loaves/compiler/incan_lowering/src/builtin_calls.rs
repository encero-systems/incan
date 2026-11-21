//! Builtin function calls and their scalar boundary checks.

use incan_mir_plan_native::{
    CalleeKind, Constant, ExternalFunction, ListLeaf, Operand, Place, PlanType, Projection, RvalueKind, SourceSpan,
    TerminatorKind, TupleElement, assign as plan_assign, tuple_element_type,
};
use incan_semantics_core::body_ir::{
    ArgumentElement, BuiltinFnId, Constant as SourceConstant, HelperOp, Place as SourcePlace,
};

use crate::builtins::{emit_boolean_operator, list_call, scalar_builtin};
use crate::checked_values::scalar_operand;
use crate::checked_values::scalar_place;
use crate::decls::library_enum_item;
use crate::formatting::display_operand;
use crate::frame::{
    Builder, drop_owned, external_call, forget_owned, literal, place_type, read_place, temporary, terminate_call,
    value_type,
};
use crate::generator_adapters::zip_generators;
use crate::generators::generator_shape;
use crate::hashed_collections::collection_contains;
use crate::json_builtins::lower_model_json_builtin;
use crate::list_methods::{
    argument_place, argument_value, borrowed_list_argument, call_destination, lower_list_builtin, lower_list_helper,
    shared_receiver_place,
};
use crate::list_values::{list_borrow, sequence_read_storage, value_operand};
use crate::lists::{list_element, list_shape, require_list_shape};
use crate::numerics::{float_conversion, integer_conversion};
use crate::numerics::{has_sized_numeric, numeric_carrier, numeric_name};
use crate::operand_borrows::{borrow_str_operand, lower_text_helper};
use crate::operators::{operand_type, scalar_operator_operand};
use crate::pending::untranslated;
use crate::ranges::builtin_range_bounds;
use crate::sequence_builtins::{
    argument_type, lower_join_helper, lower_list_concat, lower_list_repeat, lower_sequence_builtin, lower_split_helper,
    lower_truth_builtin, source_operand_type, text_argument,
};
use crate::strings::text_helper;
use crate::text_plan::{format_parts, owned_literal};
use crate::tuple_iterators::{enumerated_element, zip_call, zip_shape};
use incan_semantics_core::body_ir::Operand as SourceOperand;

/// Keep exact f32 values finite at assignment and parameter boundaries, as legacy does.
pub fn validate_exact_float(builder: &mut Builder, target: &Place, span: &SourceSpan) {
    if place_type(&builder.locals, target) == PlanType::F32 {
        let operand = read_place(target.clone(), false, span);
        external_call(
            builder,
            "incan_std_primitives::num::require_finite_f32",
            vec![PlanType::F32],
            PlanType::F32,
            vec![operand],
            target.clone(),
            span,
        );
    }
}

/// Materialize a checker-admitted destination carrier at typed boundaries, never guess binary promotions.
pub fn coerce_numeric_operand(
    builder: &mut Builder,
    operand: Operand,
    target: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if !numeric_carrier(target) {
        return Ok(operand);
    }
    let source = value_type(builder, &operand)?;
    if source == *target {
        return Ok(operand);
    }
    if !numeric_carrier(&source) {
        return Err("unsupported Body IR nonnumeric implicit conversion".into());
    }
    let converted = temporary(builder, target.clone(), span);
    builder.emit(plan_assign(
        converted.clone(),
        RvalueKind::NumericCast(operand, source, target.clone()),
        span,
    ));
    validate_exact_float(builder, &converted, span);
    Ok(read_place(converted, false, span))
}

/// Combine pure Body IR operands; source short-circuiting is retained by frontend branches.
pub fn lower_boolean_operator(
    builder: &mut Builder,
    destination: Place,
    left: &SourceOperand,
    right: &SourceOperand,
    name: &str,
    span: &SourceSpan,
) -> Result<(), String> {
    let operands = vec![
        scalar_operator_operand(builder, left, span)?,
        scalar_operator_operand(builder, right, span)?,
    ];
    emit_boolean_operator(builder, destination, operands, name, span)
}

/// Call the runtime's Python-semantics `//` or `%`, which handle negative operands and division by zero.
pub fn lower_numeric_helper(
    builder: &mut Builder,
    destination: Place,
    left: &SourceOperand,
    right: &SourceOperand,
    operation: &str,
    span: &SourceSpan,
) -> Result<(), String> {
    let ty = place_type(&builder.locals, &destination);
    if has_sized_numeric(&ty) {
        let arguments = vec![
            scalar_operand(builder, left, span)?,
            scalar_operand(builder, right, span)?,
        ];
        if value_type(builder, &arguments[0])? != ty || value_type(builder, &arguments[1])? != ty {
            return Err("unsupported Body IR mixed numeric division or modulo".into());
        }
        external_call(
            builder,
            &format!("incan_native_runtime::{operation}_{}", numeric_name(&ty)),
            vec![ty.clone(), ty.clone()],
            ty,
            arguments,
            destination.clone(),
            span,
        );
        validate_exact_float(builder, &destination, span);
        return Ok(());
    }
    let suffix = match ty {
        PlanType::Int => "i64",
        PlanType::Float => "f64",
        _ => return Err("unsupported Body IR division or modulo type".into()),
    };
    let path = format!("incan_std_primitives::num::{operation}_{suffix}");
    let arguments = vec![
        value_operand(builder, left, span)?,
        value_operand(builder, right, span)?,
    ];
    external_call(
        builder,
        &path,
        vec![ty.clone(), ty.clone()],
        ty,
        arguments,
        destination,
        span,
    );
    Ok(())
}

/// Dispatch checked builtin identities rather than matching source spellings.
pub fn lower_builtin(
    builder: &mut Builder,
    builtin: BuiltinFnId,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if builtin == BuiltinFnId::Bool {
        return lower_truth_builtin(builder, arguments, destination, span);
    }
    if arguments.len() == 1 && builtin == BuiltinFnId::Abs {
        let operand = argument_value(builder, arguments, 0, span)?;
        let ty = value_type(builder, &operand)?;
        if let Some((name, result)) = scalar_builtin(builtin, &ty) {
            let output = call_destination(builder, destination, result.clone(), span)?;
            let path = format!("incan_native_runtime::{name}");
            external_call(builder, &path, vec![ty], result, vec![operand], output, span);
            return Ok(());
        }
    }
    match builtin {
        BuiltinFnId::Print => lower_print(builder, arguments, destination, span),
        BuiltinFnId::Float => lower_scalar_conversion(builder, arguments, destination, false, span),
        BuiltinFnId::Int => lower_scalar_conversion(builder, arguments, destination, true, span),
        BuiltinFnId::Str => lower_str_builtin(builder, arguments, destination, span),
        BuiltinFnId::Sum | BuiltinFnId::Sorted => {
            lower_sequence_builtin(builder, builtin, arguments, destination, span)
        }
        BuiltinFnId::Min | BuiltinFnId::Max => lower_extrema_builtin(builder, builtin, arguments, destination, span),
        BuiltinFnId::ReadFile | BuiltinFnId::WriteFile => Err(untranslated("builtin_calls::lower_file_builtin")),
        BuiltinFnId::Zip => lower_zip_builtin(builder, arguments, destination, span),
        BuiltinFnId::Enumerate => lower_enumerate_builtin(builder, arguments, destination, span),
        BuiltinFnId::Len => lower_list_builtin(builder, arguments, destination, span),
        BuiltinFnId::JsonStringify => lower_model_json_builtin(builder, arguments, destination, span),
        BuiltinFnId::Range => lower_range_builtin(builder, arguments, destination, span),
        _ => Err(format!("unsupported Body IR builtin {builtin:?}")),
    }
}

/// Print space-separated arguments and initialize the checked Unit result when the call is an expression.
fn lower_print(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    // `parts` holds the literal text around each value: "" before the first, " " between values, "" after the last.
    let mut parts = vec![String::new()];
    let mut values: Vec<Place> = Vec::new();
    for argument in arguments {
        match argument.as_one() {
            Some(value) => values.push(display_operand(builder, value, span)?),
            None => return Err("unsupported Body IR spread print argument".into()),
        }
        parts.push(" ".to_string());
    }
    if let Some(last) = parts.last_mut() {
        *last = String::new();
    }
    let text = temporary(builder, PlanType::String, span);
    format_parts(builder, text.clone(), &parts, &values, span);
    // Expression arms read this call's retained result; an unrelated Unit temporary leaves that read uninitialized.
    // The Unit temporary is reserved even when the call has a destination, as the reference lowering reserves it.
    let mut output = temporary(builder, PlanType::Unit, span);
    if let Some(place) = destination {
        output = scalar_place(builder, place, span)?;
    }
    let path = "incan_native_runtime::println_text";
    external_call(
        builder,
        path,
        vec![PlanType::String],
        PlanType::Unit,
        vec![read_place(text, true, span)],
        output,
        span,
    );
    Ok(())
}

/// Lower checked scalar builtins through the numeric admission table.
fn lower_scalar_conversion(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    integer: bool,
    span: &SourceSpan,
) -> Result<(), String> {
    let Some(place) = destination else {
        return Err("Body IR scalar conversion has no destination".into());
    };
    let output = scalar_place(builder, place, span)?;
    if lower_text_scalar_conversion(builder, arguments, &output, integer, span)? {
        return Ok(());
    }
    let mut operands = Vec::with_capacity(arguments.len());
    for element in arguments {
        match element.as_one() {
            Some(value) => operands.push(value_operand(builder, value, span)?),
            None => return Err("unsupported Body IR spread ArgumentElement".into()),
        }
    }
    if operands.is_empty() {
        let zero = if integer {
            Constant::Int(0)
        } else {
            Constant::Float(0.0)
        };
        builder.emit(plan_assign(output, RvalueKind::Use(literal(zero, span)), span));
        return Ok(());
    }
    if operands.len() != 1 {
        return Err("unsupported Body IR scalar conversion arity".into());
    }
    let Some(operand) = operands.pop() else {
        return Err("unsupported Body IR scalar conversion arity".into());
    };
    let ty = value_type(builder, &operand)?;
    if has_sized_numeric(&ty) {
        let result_type = if integer { PlanType::Int } else { PlanType::Float };
        builder.emit(plan_assign(
            output,
            RvalueKind::NumericCast(operand, ty, result_type),
            span,
        ));
        return Ok(());
    }
    let conversion = if integer {
        integer_conversion(operand, &ty)
    } else {
        float_conversion(operand, &ty)
    };
    builder.emit(plan_assign(output, conversion?, span));
    Ok(())
}

/// Inspect the retained operand type before parsing borrowed text, without materializing an indexed value twice.
fn lower_text_scalar_conversion(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    output: &Place,
    integer: bool,
    span: &SourceSpan,
) -> Result<bool, String> {
    if arguments.len() != 1 {
        return Ok(false);
    }
    let Some(value) = arguments[0].as_one() else {
        return Err("unsupported Body IR spread scalar conversion".into());
    };
    let text_input = match value {
        SourceOperand::Constant(SourceConstant::FrozenStr(_)) => {
            return Err("static text cannot be converted to a number by a numeric cast".into());
        }
        SourceOperand::Constant(SourceConstant::Str(_)) => true,
        SourceOperand::Constant(_) => false,
        SourceOperand::Place(_) => matches!(
            source_operand_type(builder, value, span)?,
            PlanType::String | PlanType::StringRef | PlanType::StrRef | PlanType::FrozenStr
        ),
    };
    if !text_input {
        return Ok(false);
    }
    let text = display_operand(builder, value, span)?;
    let reference = temporary(builder, PlanType::StringRef, span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(text.clone()), span));
    let (name, result) = if integer {
        ("parse_int", PlanType::Int)
    } else {
        ("parse_float", PlanType::Float)
    };
    external_call(
        builder,
        &format!("incan_native_runtime::{name}"),
        vec![PlanType::StringRef],
        result,
        vec![read_place(reference, false, span)],
        output.clone(),
        span,
    );
    drop_owned(builder, text.local, span);
    Ok(true)
}

/// Materialize str() using the same scalar display conversions as legacy formatting.
fn lower_str_builtin(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let text = match arguments {
        [] => owned_literal(builder, "", span),
        [argument] => match argument.as_one() {
            Some(value) => display_operand(builder, value, span)?,
            None => return Err("unsupported Body IR spread str argument".into()),
        },
        _ => return Err("unsupported Body IR str arity".into()),
    };
    assign_text_result(builder, text, destination, span)
}

/// Move an owned text temporary into the call's destination, replacing what it held, or drop a discarded one.
pub fn assign_text_result(
    builder: &mut Builder,
    text: Place,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    match destination {
        Some(place) => {
            let output = scalar_place(builder, place, span)?;
            forget_owned(builder, text.local);
            drop_owned(builder, output.local, span);
            let local = output.local;
            builder.emit(plan_assign(output, RvalueKind::Use(read_place(text, true, span)), span));
            builder.owned.push(local);
        }
        None => drop_owned(builder, text.local, span),
    }
    Ok(())
}

/// Call a source model's derived Clone on a shared reference to it, yielding the owned copy.
pub fn clone_model_reference(
    builder: &mut Builder,
    reference: Operand,
    index: i64,
    name: &str,
    span: &SourceSpan,
) -> Operand {
    let result = temporary(builder, PlanType::Model(index, name.to_string()), span);
    terminate_call(
        builder,
        CalleeKind::CloneModel(index, name.to_string()),
        vec![reference],
        result.clone(),
        span,
    );
    builder.owned.push(result.local);
    read_place(result, true, span)
}

/// Materialize an owned enum through its retained Clone implementation while keeping the referenced owner intact.
pub fn clone_enum_reference(
    builder: &mut Builder,
    reference: Operand,
    index: i64,
    name: &str,
    span: &SourceSpan,
) -> Operand {
    let result = temporary(builder, PlanType::Enum(index, name.to_string()), span);
    let mut callee = CalleeKind::CloneEnum(index, name.to_string());
    let native = builder
        .enums()
        .get(index as usize)
        .is_some_and(|declaration| !library_enum_item(declaration).is_empty());
    if index >= 0 && native {
        let owned = PlanType::Enum(index, name.to_string());
        let borrowed = PlanType::EnumRef(index, name.to_string());
        let path = "core::clone::Clone::clone";
        builder.externals.push(ExternalFunction {
            path: path.to_string(),
            type_arguments: vec![owned.clone()],
            parameters: vec![borrowed],
            return_type: owned.clone(),
            span: span.clone(),
        });
        callee = CalleeKind::Instantiated(path.to_string(), vec![owned]);
    }
    terminate_call(builder, callee, vec![reference], result.clone(), span);
    builder.owned.push(result.local);
    read_place(result, true, span)
}

/// Dispatch retained runtime helper identities and name unsupported variants without taking their value.
pub fn lower_runtime_helper(
    builder: &mut Builder,
    helper: HelperOp,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    match helper {
        HelperOp::InstallPanicHook => return Err(untranslated("builtin_calls::install_panic_hook")),
        HelperOp::RaiseValueError => return lower_raise_value_error(builder, arguments, destination, span),
        HelperOp::CallableName => return Err(untranslated("callable_calls::lower_callable_name")),
        HelperOp::SetContains | HelperOp::DictContainsKey => {
            return collection_contains(builder, false, arguments, destination, span);
        }
        HelperOp::SetNotContains | HelperOp::DictNotContainsKey => {
            return collection_contains(builder, true, arguments, destination, span);
        }
        HelperOp::StrSplit => return lower_split_helper(builder, arguments, destination, span),
        HelperOp::StrJoin => return lower_join_helper(builder, arguments, destination, span),
        HelperOp::ListConcat => return lower_list_concat(builder, arguments, destination, span),
        HelperOp::ListRepeat | HelperOp::RangeListFill => {
            return lower_list_repeat(builder, arguments, destination, span);
        }
        HelperOp::ListContains => return lower_list_helper(builder, false, arguments, destination, span),
        HelperOp::ListNotContains => return lower_list_helper(builder, true, arguments, destination, span),
        _ => {}
    }
    if let Some((name, result)) = text_helper(helper) {
        return lower_text_helper(builder, name, result, arguments, destination, span);
    }
    match helper {
        HelperOp::RangeValidate => {
            if arguments.len() != 3 {
                return Err("invalid Body IR RangeValidate arity".into());
            }
            let output = temporary(builder, PlanType::Unit, span);
            let operands = fixed_arguments(builder, arguments, span)?;
            let int_triple = vec![PlanType::Int, PlanType::Int, PlanType::Int];
            external_call(
                builder,
                "incan_native_runtime::validate_range",
                int_triple,
                PlanType::Unit,
                operands,
                output,
                span,
            );
            Ok(())
        }
        HelperOp::NewtypeConstraintValidate => {
            if arguments.len() != 3 {
                return Err("invalid Body IR NewtypeConstraintValidate arity".into());
            }
            let output = call_destination(builder, destination, PlanType::Unit, span)?;
            let operands = fixed_arguments(builder, arguments, span)?;
            external_call(
                builder,
                "incan_native_runtime::validate_newtype_constraint",
                vec![PlanType::Bool, PlanType::String, PlanType::String],
                PlanType::Unit,
                operands,
                output,
                span,
            );
            Ok(())
        }
        HelperOp::NewtypeHookExtract | HelperOp::NewtypeFieldHookExtract => {
            Err(untranslated("newtype_hooks::lower_newtype_hook_result"))
        }
        HelperOp::NewtypeSumAdd | HelperOp::NewtypeSumExtract => Err(untranslated("newtype_sums::newtype sums")),
        HelperOp::ValidationErrorNew | HelperOp::ValidationErrorWithCode => {
            Err(untranslated("validation_errors::lower_validation_error"))
        }
        _ => Err(format!("unsupported Body IR Helper {helper:?}")),
    }
}

/// Raise the checked value error with its borrowed message; the call never returns.
fn lower_raise_value_error(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 1 {
        return Err("invalid Body IR RaiseValueError arity".into());
    }
    if destination.is_some() {
        return Err("invalid Body IR diverging helper destination".into());
    }
    let Some(message) = arguments[0].as_one() else {
        return Err("invalid Body IR RaiseValueError message spread".into());
    };
    let operands = vec![borrow_str_operand(builder, message, span)?];
    let output = temporary(builder, PlanType::Never, span);
    external_call(
        builder,
        "incan_std_primitives::errors::raise_value_error",
        vec![PlanType::StrRef],
        PlanType::Never,
        operands,
        output,
        span,
    );
    builder.finish(TerminatorKind::Unreachable, span);
    Ok(())
}

/// Read already ordered positional elements, refusing spreading by name.
fn fixed_arguments(
    builder: &mut Builder,
    elements: &[ArgumentElement],
    span: &SourceSpan,
) -> Result<Vec<Operand>, String> {
    let mut result = Vec::with_capacity(elements.len());
    for element in elements {
        match element.as_one() {
            Some(value) => result.push(value_operand(builder, value, span)?),
            None => return Err("unsupported Body IR spread ArgumentElement".into()),
        }
    }
    Ok(result)
}

/// Classify a homogeneous scalar list before borrowing its value for the minimum or maximum implementation.
fn lower_extrema_builtin(
    builder: &mut Builder,
    builtin: BuiltinFnId,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 1 {
        return Err("unsupported Body IR extrema builtin arity".into());
    }
    let source_type = argument_type(builder, arguments, 0, span)?;
    let (leaf, depth) = require_list_shape(&source_type, "unsupported Body IR extrema builtin type")?;
    if depth != 1 {
        return Err("unsupported Body IR nested extrema list".into());
    }
    let (suffix, result) = match leaf {
        ListLeaf::Int => ("int", PlanType::Int),
        ListLeaf::Float => ("float", PlanType::Float),
        ListLeaf::Bool => ("bool", PlanType::Bool),
        ListLeaf::Str => ("text", PlanType::String),
        _ => return Err("unsupported Body IR extrema list element".into()),
    };
    let name = if builtin == BuiltinFnId::Max {
        "maximum"
    } else {
        "minimum"
    };
    let output = call_destination(builder, destination, result.clone(), span)?;
    let reference = borrowed_list_argument(builder, arguments, 0, false, span)?;
    let local = output.local;
    external_call(
        builder,
        &format!("incan_native_runtime::{name}_{suffix}"),
        vec![PlanType::ListRef(leaf, depth)],
        result.clone(),
        vec![reference],
        output,
        span,
    );
    if result == PlanType::String {
        builder.owned.push(local);
    }
    Ok(())
}

/// Construct a checked builtin `range` value as legacy's lazy integer range, rejecting a zero step now.
///
/// The checker types the call as a list, but legacy never materializes one: the value is a native range that only
/// iteration consumes. The call's fresh temporary therefore holds the runtime's integer generator, and a consumer
/// that needs a list materializes that fresh owned range at the checked parameter boundary.
fn lower_range_builtin(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let fixed = fixed_arguments(builder, arguments, span)?;
    let bounds = builtin_range_bounds(fixed, span)?;
    for bound in &bounds {
        if value_type(builder, bound)? != PlanType::Int {
            return Err("unsupported Body IR builtin range bound type".into());
        }
    }
    let list = PlanType::List(ListLeaf::Int, 1);
    let output = call_destination(builder, destination, list.clone(), span)?;
    if output.projection != Projection::Whole || builder.owned.contains(&output.local) {
        return Err("unsupported Body IR builtin range value outside a fresh temporary".into());
    }
    if place_type(&builder.locals, &output) != list {
        return Err("unsupported Body IR builtin range destination type".into());
    }
    let result = PlanType::Generator(ListLeaf::Int, 0);
    builder.locals[output.local as usize].ty = result.clone();
    let local = output.local;
    let parameters = vec![PlanType::Int, PlanType::Int, PlanType::Int];
    external_call(
        builder,
        "incan_native_runtime::range_generator",
        parameters,
        result,
        bounds,
        output,
        span,
    );
    builder.owned.push(local);
    Ok(())
}

/// Construct lazy zip only from canonical identity and two checked primitive-list arguments.
fn lower_zip_builtin(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 2 {
        return Err("unsupported Body IR zip arity".into());
    }
    let output = call_destination(builder, destination, PlanType::Unit, span)?;
    let checked = builder.locals[output.local as usize].ty.clone();
    // The frame lays out zip over two iterators as a generator of pairs, which draws from both lazily.
    if generator_shape(&checked).is_some() {
        let first = argument_value(builder, arguments, 0, span)?;
        let second = argument_value(builder, arguments, 1, span)?;
        drop_owned(builder, output.local, span);
        let local = output.local;
        zip_generators(builder, first, second, output, span)?;
        builder.owned.push(local);
        return Ok(());
    }
    let Some((left, right)) = zip_shape(&checked) else {
        return Err("unsupported Body IR zip checked destination".into());
    };
    let first = argument_value(builder, arguments, 0, span)?;
    let second = argument_value(builder, arguments, 1, span)?;
    let first_type = operand_type(builder, &first)?;
    let second_type = operand_type(builder, &second)?;
    if list_shape(&first_type).is_none() || list_shape(&second_type).is_none() {
        return Err("unsupported Body IR zip non-list operands".into());
    }
    let (first_leaf, first_depth) = require_list_shape(&first_type, "unsupported Body IR zip left")?;
    let (second_leaf, second_depth) = require_list_shape(&second_type, "unsupported Body IR zip right")?;
    if list_element(&first_leaf, first_depth) != tuple_element_type(&left)
        || list_element(&second_leaf, second_depth) != tuple_element_type(&right)
    {
        return Err("unsupported Body IR zip checked operand type".into());
    }
    drop_owned(builder, output.local, span);
    let local = output.local;
    let result = PlanType::ZipIterator(left.clone(), right.clone());
    zip_call(
        builder,
        "zip_lists",
        &left,
        &right,
        vec![first_type, second_type],
        result,
        vec![first, second],
        output,
        span,
    );
    builder.owned.push(local);
    Ok(())
}

/// Materialize the checked builtin's zero-based tuple list from admitted list or text inputs.
fn lower_enumerate_builtin(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 1 {
        return Err("invalid Body IR enumerate arity".into());
    }
    let source_type = argument_type(builder, arguments, 0, span)?;
    let text_source = matches!(
        source_type,
        PlanType::String | PlanType::StringRef | PlanType::FrozenStr
    );
    let byte_source = source_type == PlanType::FrozenBytes
        || matches!(
            &source_type,
            PlanType::List(ListLeaf::U8, 1)
                | PlanType::ListRef(ListLeaf::U8, 1)
                | PlanType::ListMutRef(ListLeaf::U8, 1)
        );
    let element_type = if text_source {
        PlanType::String
    } else if byte_source {
        PlanType::Int
    } else {
        let (leaf, depth) = require_list_shape(&source_type, "unsupported Body IR enumerate non-list or text source")?;
        list_element(&leaf, depth)
    };
    let element = enumerated_element(&element_type)?;
    let result = PlanType::List(ListLeaf::Tuple(vec![TupleElement::Int, element]), 1);
    let output = call_destination(builder, destination, result.clone(), span)?;
    if place_type(&builder.locals, &output) != result {
        return Err("unsupported Body IR enumerate checked destination".into());
    }
    drop_owned(builder, output.local, span);
    let local = output.local;
    if text_source {
        let reference = text_argument(builder, arguments, 0, span)?;
        let path = "incan_native_runtime::enumerate_text";
        external_call(
            builder,
            path,
            vec![PlanType::StringRef],
            result,
            vec![reference],
            output,
            span,
        );
        builder.owned.push(local);
        return Ok(());
    }
    if byte_source {
        let reference = enumerated_byte_argument(builder, arguments, &source_type, span)?;
        let path = "incan_native_runtime::enumerate_bytes";
        let parameters = vec![PlanType::ListRef(ListLeaf::U8, 1)];
        external_call(builder, path, parameters, result, vec![reference], output, span);
        builder.owned.push(local);
        return Ok(());
    }
    let receiver = shared_receiver_place(builder, arguments, 0, span)?;
    let (leaf, depth) = require_list_shape(&source_type, "unsupported Body IR enumerate source layout")?;
    let reference = list_borrow(builder, receiver, false, span)?;
    let parameters = vec![PlanType::ListRef(leaf, depth)];
    list_call(
        builder,
        "enumerate_list",
        element_type,
        parameters,
        result,
        vec![reference],
        output,
        span,
    );
    Ok(())
}

/// Borrow owned byte storage or a literal, materializing only a frozen wrapper's immutable bytes.
fn enumerated_byte_argument(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    ty: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if *ty != PlanType::FrozenBytes {
        return borrowed_list_argument(builder, arguments, 0, false, span);
    }
    let source = argument_place(builder, arguments, 0, span)?;
    let snapshot = sequence_read_storage(builder, source, span)?;
    list_borrow(builder, snapshot, false, span)
}
