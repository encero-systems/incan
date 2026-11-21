//! Builtin function calls and their scalar boundary checks.

use incan_mir_plan_native::{
    CalleeKind, Constant, ExternalFunction, Operand, Place, PlanType, RvalueKind, SourceSpan, TerminatorKind,
    assign as plan_assign,
};
use incan_semantics_core::body_ir::{
    ArgumentElement, BuiltinFnId, Constant as SourceConstant, HelperOp, Place as SourcePlace,
};

use crate::builtins::{emit_boolean_operator, scalar_builtin};
use crate::checked_values::scalar_operand;
use crate::checked_values::scalar_place;
use crate::decls::library_enum_item;
use crate::formatting::display_operand;
use crate::frame::{
    Builder, drop_owned, external_call, forget_owned, literal, place_type, read_place, temporary, terminate_call,
    value_type,
};
use crate::hashed_collections::collection_contains;
use crate::json_builtins::lower_model_json_builtin;
use crate::list_methods::{argument_value, call_destination, lower_list_builtin, lower_list_helper};
use crate::list_values::value_operand;
use crate::numerics::{float_conversion, integer_conversion};
use crate::numerics::{has_sized_numeric, numeric_carrier, numeric_name};
use crate::operand_borrows::{borrow_str_operand, lower_text_helper};
use crate::operators::scalar_operator_operand;
use crate::pending::untranslated;
use crate::sequence_builtins::{
    lower_join_helper, lower_list_concat, lower_list_repeat, lower_sequence_builtin, lower_split_helper,
    lower_truth_builtin, source_operand_type,
};
use crate::strings::text_helper;
use crate::text_plan::{format_parts, owned_literal};
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
        BuiltinFnId::Min | BuiltinFnId::Max => Err(untranslated("builtin_calls::lower_extrema_builtin")),
        BuiltinFnId::ReadFile | BuiltinFnId::WriteFile => Err(untranslated("builtin_calls::lower_file_builtin")),
        BuiltinFnId::Zip => Err(untranslated("builtin_calls::lower_zip_builtin")),
        BuiltinFnId::Enumerate => Err(untranslated("builtin_calls::lower_enumerate_builtin")),
        BuiltinFnId::Len => lower_list_builtin(builder, arguments, destination, span),
        BuiltinFnId::JsonStringify => lower_model_json_builtin(builder, arguments, destination, span),
        BuiltinFnId::Range => Err(untranslated("builtin_calls::lower_range_builtin")),
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
