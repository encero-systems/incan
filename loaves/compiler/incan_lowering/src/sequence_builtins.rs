//! Lower sequence builtins: sorting, repetition, concatenation, truth, powers, and text splitting and joining.

use incan_mir_plan_native::{
    BinaryOp, Constant, ListLeaf, Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign,
    tuple_element_type,
};
use incan_semantics_core::IncanInputAccess;
use incan_semantics_core::body_ir::{
    ArgumentElement, BuiltinFnId, Constant as SourceConstant, Operand as SourceOperand, Place as SourcePlace,
};

use crate::builtin_calls::{coerce_numeric_operand, validate_exact_float};
use crate::builtins::{emit_power_operator, list_call, scalar_builtin};
use crate::call_arguments::call_argument_operand;
use crate::checked_values::scalar_place;
use crate::collections::{borrow_collection, collection_call, collection_reference, is_collection};
use crate::element_places::{element_place_type, is_element_place};
use crate::frame::{Builder, external_call, literal, place_type, read_place, temporary, value_type};
use crate::list_iteration::shared_sequence_receiver;
use crate::list_methods::{argument_value, borrowed_list_argument, call_destination, shared_receiver_place};
use crate::list_values::{list_borrow, value_operand};
use crate::lists::{is_list, list_element, require_list_shape};
use crate::numerics::{integer_carrier, numeric_carrier, numeric_name};
use crate::operand_borrows::borrow_text_operand;
use crate::operators::scalar_operator_operand;
use crate::pending::untranslated;
use crate::text_ownership::drop_borrowed_text_temporaries;

/// Read an argument's retained carrier before selecting a borrow, avoiding an accidental owned read.
pub fn argument_type(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    index: usize,
    span: &SourceSpan,
) -> Result<PlanType, String> {
    let Some(argument) = arguments.get(index) else {
        return Err("invalid Body IR argument type arity".into());
    };
    match argument.as_one() {
        Some(value) => source_operand_type(builder, value, span),
        None => Err("unsupported Body IR argument type spread".into()),
    }
}

/// Read a retained carrier without materializing its value.
///
/// 1. Select an exact local, field or global carrier, or preserve a constant's ordinary typed conversion.
pub fn source_operand_type(
    builder: &mut Builder,
    value: &SourceOperand,
    span: &SourceSpan,
) -> Result<PlanType, String> {
    // 1. Select the exact place carrier or the constant's ordinary typed conversion.
    match value {
        SourceOperand::Place(read) => {
            if read.place.global().is_some() {
                return Err(untranslated("static_storage::computed_static_type"));
            }
            if is_element_place(builder, &read.place)? {
                return element_place_type(builder, &read.place);
            }
            let place = scalar_place(builder, &read.place, span)?;
            Ok(place_type(&builder.locals, &place))
        }
        SourceOperand::Constant(SourceConstant::Str(_)) => Ok(PlanType::String),
        SourceOperand::Constant(SourceConstant::FrozenStr(_)) => Ok(PlanType::FrozenStr),
        SourceOperand::Constant(_) => {
            let operand = value_operand(builder, value, span)?;
            value_type(builder, &operand)
        }
    }
}

/// Classify homogeneous list sorting and integer summation without projecting an indexed input as storage.
pub fn lower_sequence_builtin(
    builder: &mut Builder,
    builtin: BuiltinFnId,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 1 {
        return Err("unsupported Body IR sequence builtin arity".into());
    }
    let receiver = shared_sequence_receiver(builder, arguments, span, false)?;
    let (leaf, depth) = require_list_shape(
        &place_type(&builder.locals, &receiver),
        "unsupported Body IR sequence builtin type",
    )?;
    if depth != 1 && builtin == BuiltinFnId::Sum {
        return Err("unsupported Body IR nested sequence builtin".into());
    }
    if builtin == BuiltinFnId::Sorted && (leaf != ListLeaf::Float || depth != 1) {
        require_sort_order(builder, &list_element(&leaf, depth))?;
    }
    let reference = list_borrow(builder, receiver, false, span)?;
    let borrowed = PlanType::ListRef(leaf.clone(), depth);
    if builtin == BuiltinFnId::Sum {
        let name = match leaf {
            ListLeaf::Bool => "sum_bool",
            ListLeaf::Int => "sum_int",
            _ => return Err("unsupported Body IR builtin Sum element".into()),
        };
        let output = call_destination(builder, destination, PlanType::Int, span)?;
        external_call(
            builder,
            &format!("incan_native_runtime::{name}"),
            vec![borrowed],
            PlanType::Int,
            vec![reference],
            output,
            span,
        );
        return Ok(());
    }
    let result = PlanType::List(leaf.clone(), depth);
    let output = call_destination(builder, destination, result.clone(), span)?;
    if leaf == ListLeaf::Float && depth == 1 {
        let local = output.local;
        external_call(
            builder,
            "incan_native_runtime::sorted_float",
            vec![borrowed],
            result,
            vec![reference],
            output,
            span,
        );
        builder.owned.push(local);
    } else {
        list_call(
            builder,
            "sorted_list",
            list_element(&leaf, depth),
            vec![borrowed],
            result,
            vec![reference],
            output,
            span,
        );
    }
    Ok(())
}

/// Require retained native ordering for new model and compound sorting shapes.
///
/// A model's Rust Ord derive or complete checked comparison body frame supplies executable authority. A source Ord
/// adoption alone does not prove its native dunders. Existing scalar and enum leaves retain their established profile.
/// Floating elements inside a compound value have no Rust Ord implementation, unlike the separately admitted flat
/// floating-list partial-order sort.
fn require_sort_order(builder: &Builder, element: &PlanType) -> Result<(), String> {
    match element {
        PlanType::Model(owner, _) => {
            let owner = *owner as usize;
            let ordered = owner < builder.nominals().len()
                && (builder.nominals()[owner].derives.iter().any(|name| name == "Ord")
                    || builder.program.comparison_functions[owner].len() == 5);
            if !ordered {
                return Err("unsupported Body IR Sorted nominal without a retained Ord derive".into());
            }
        }
        PlanType::Tuple(elements) => {
            for item in elements {
                require_sort_order(builder, &tuple_element_type(item))?;
            }
        }
        PlanType::Float => return Err("unsupported Body IR Sorted compound containing float".into()),
        PlanType::List(..) => {
            let (leaf, depth) = require_list_shape(element, "unsupported Body IR Sorted compound list")?;
            require_sort_order(builder, &list_element(&leaf, depth))?;
        }
        _ => {}
    }
    Ok(())
}

/// Clone one owned value `count` times into a new list through legacy's runtime helper.
///
/// The element type is the checked list the call assigns. The value moves into the helper, so a value read from a
/// binding arrives as the copy its ownership fact already made.
pub fn lower_list_repeat(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 2 {
        return Err("unsupported Body IR list_repeat arity".into());
    }
    if destination.is_none() {
        return Err("unsupported Body IR discarded list_repeat".into());
    }
    let output = call_destination(builder, destination, PlanType::Unit, span)?;
    let (leaf, depth) = require_list_shape(
        &place_type(&builder.locals, &output),
        "unsupported Body IR list_repeat type",
    )?;
    let element = list_element(&leaf, depth);
    let types = [element.clone(), PlanType::Int];
    let mut operands = Vec::with_capacity(2);
    for (argument, ty) in arguments.iter().zip(&types) {
        match argument.as_one() {
            Some(value) => operands.push(call_argument_operand(
                builder,
                value,
                ty,
                span,
                true,
                IncanInputAccess::Inferred,
            )?),
            None => return Err("unsupported Body IR list_repeat spread".into()),
        }
    }
    list_call(
        builder,
        "repeat_list",
        element.clone(),
        vec![element, PlanType::Int],
        PlanType::List(leaf, depth),
        operands,
        output,
        span,
    );
    Ok(())
}

/// Classify both input carriers without reading them, then borrow each value once for concatenation.
pub fn lower_list_concat(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 2 {
        return Err("unsupported Body IR list_concat arity".into());
    }
    let source_type = argument_type(builder, arguments, 0, span)?;
    let (leaf, depth) = require_list_shape(&source_type, "unsupported Body IR list_concat type")?;
    let other_type = argument_type(builder, arguments, 1, span)?;
    let (other_leaf, other_depth) = require_list_shape(&other_type, "unsupported Body IR list_concat type")?;
    if leaf != other_leaf || depth != other_depth {
        return Err("unsupported Body IR mixed list_concat type".into());
    }
    let operands = vec![
        borrowed_list_argument(builder, arguments, 0, false, span)?,
        borrowed_list_argument(builder, arguments, 1, false, span)?,
    ];
    let result = PlanType::List(leaf.clone(), depth);
    let output = call_destination(builder, destination, result.clone(), span)?;
    let borrowed = PlanType::ListRef(leaf.clone(), depth);
    list_call(
        builder,
        "concat_list",
        list_element(&leaf, depth),
        vec![borrowed.clone(), borrowed],
        result,
        operands,
        output,
        span,
    );
    Ok(())
}

/// Preserve builtin truthiness for admitted scalars, lists, and hashed collections.
pub fn lower_truth_builtin(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let output = call_destination(builder, destination, PlanType::Bool, span)?;
    if arguments.is_empty() {
        builder.emit(plan_assign(
            output,
            RvalueKind::Use(literal(Constant::Bool(false), span)),
            span,
        ));
        return Ok(());
    }
    if arguments.len() != 1 {
        return Err("unsupported Body IR Bool arity".into());
    }
    let ty = argument_type(builder, arguments, 0, span)?;
    if let Some((name, result)) = scalar_builtin(BuiltinFnId::Bool, &ty) {
        let operand = argument_value(builder, arguments, 0, span)?;
        let path = format!("incan_native_runtime::{name}");
        external_call(builder, &path, vec![ty], result, vec![operand], output, span);
        return Ok(());
    }
    if matches!(ty, PlanType::String | PlanType::StringRef | PlanType::FrozenStr) {
        return text_truth(builder, &arguments[0], output, span);
    }
    if is_collection(&ty) {
        let receiver = shared_receiver_place(builder, arguments, 0, span)?;
        let reference = borrow_collection(builder, &receiver, false, span)?;
        let length = temporary(builder, PlanType::Int, span);
        collection_call(
            builder,
            &ty,
            "length",
            vec![collection_reference(&ty, false)],
            PlanType::Int,
            vec![reference],
            length.clone(),
            span,
        )?;
        let nonempty = RvalueKind::Binary(
            BinaryOp::NotEqual,
            read_place(length, false, span),
            literal(Constant::Int(0), span),
        );
        builder.emit(plan_assign(output, nonempty, span));
        return Ok(());
    }
    if is_list(&ty) {
        let (leaf, depth) = require_list_shape(&ty, "unsupported Body IR Bool list")?;
        let reference = borrowed_list_argument(builder, arguments, 0, false, span)?;
        list_call(
            builder,
            "truth_list",
            list_element(&leaf, depth),
            vec![PlanType::ListRef(leaf, depth)],
            PlanType::Bool,
            vec![reference],
            output,
            span,
        );
        return Ok(());
    }
    Err("unsupported Body IR builtin Bool type".into())
}

/// Test text truthiness through a borrow, releasing any owner materialized only for that borrow.
fn text_truth(
    builder: &mut Builder,
    argument: &ArgumentElement,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let Some(value) = argument.as_one() else {
        return Err("unsupported Body IR Bool spread".into());
    };
    // Legacy's StaticStr constant falls through bool() to true, even for empty text.
    if let SourceOperand::Constant(SourceConstant::FrozenStr(_)) = value {
        builder.emit(plan_assign(
            output,
            RvalueKind::Use(literal(Constant::Bool(true), span)),
            span,
        ));
        return Ok(());
    }
    let incoming = builder.owned.clone();
    let reference = borrow_text_operand(builder, value, span)?;
    let result = output.local;
    external_call(
        builder,
        "incan_native_runtime::truth_text",
        vec![PlanType::StringRef],
        PlanType::Bool,
        vec![reference],
        output,
        span,
    );
    drop_borrowed_text_temporaries(builder, &incoming, result, span);
    Ok(())
}

/// Bind exponentiation to the checked result carrier, as legacy's `powf` and `pow` do.
pub fn lower_power_operator(
    builder: &mut Builder,
    destination: Place,
    left: &SourceOperand,
    right: &SourceOperand,
    span: &SourceSpan,
) -> Result<(), String> {
    let mut operands = vec![
        scalar_operator_operand(builder, left, span)?,
        scalar_operator_operand(builder, right, span)?,
    ];
    let target = place_type(&builder.locals, &destination);
    // A float result is a retained checker fact: legacy converts every numeric operand to that carrier before `powf`,
    // so an `int` exponent and an `f32` base both widen here.
    if matches!(target, PlanType::Float | PlanType::F32 | PlanType::F64) {
        let mut converted = Vec::with_capacity(2);
        for operand in operands {
            let source = value_type(builder, &operand)?;
            if source != target && !numeric_carrier(&source) {
                return Err("unsupported Body IR mixed Pow operands".into());
            }
            converted.push(coerce_numeric_operand(builder, operand, &target, span)?);
        }
        operands = converted;
    } else if target != PlanType::Int && integer_carrier(&target) {
        return lower_exact_integer_power(builder, destination, operands, &target, span);
    }
    emit_power_operator(builder, destination.clone(), operands, span)?;
    validate_exact_float(builder, &destination, span);
    Ok(())
}

/// Raise an exact-width integer base through the runtime power for its carrier.
fn lower_exact_integer_power(
    builder: &mut Builder,
    destination: Place,
    operands: Vec<Operand>,
    target: &PlanType,
    span: &SourceSpan,
) -> Result<(), String> {
    // The checked result keeps the base's carrier. Legacy calls that carrier's `pow` with the exponent cast to `u32`;
    // the runtime helper takes an `int` exponent and truncates it the same way.
    let mut operands = operands.into_iter();
    let (Some(base), Some(exponent)) = (operands.next(), operands.next()) else {
        return Err("unsupported Body IR mixed Pow operands".into());
    };
    if value_type(builder, &base)? != *target {
        return Err("unsupported Body IR mixed Pow operands".into());
    }
    let exponent = coerce_numeric_operand(builder, exponent, &PlanType::Int, span)?;
    external_call(
        builder,
        &format!("incan_native_runtime::power_{}", numeric_name(target)),
        vec![target.clone(), PlanType::Int],
        target.clone(),
        vec![base, exponent],
        destination,
        span,
    );
    Ok(())
}

/// Borrow one text argument without consuming a caller-owned string.
pub fn text_argument(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    index: usize,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let Some(argument) = arguments.get(index) else {
        return Err("invalid Body IR text argument arity".into());
    };
    match argument.as_one() {
        Some(value) => borrow_text_operand(builder, value, span),
        None => Err("unsupported Body IR text argument spread".into()),
    }
}

/// Select explicit and omitted separators without admitting general Option lowering.
pub fn lower_split_helper(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.is_empty() || arguments.len() > 2 {
        return Err("invalid Body IR str_split arity".into());
    }
    let omitted = arguments.len() == 1
        || matches!(
            arguments[1].as_one(),
            Some(SourceOperand::Constant(SourceConstant::None))
        );
    let mut name = "split_default_text";
    let mut parameters = vec![PlanType::StringRef];
    let mut operands = vec![text_argument(builder, arguments, 0, span)?];
    if !omitted {
        name = "split_text";
        parameters.push(PlanType::StringRef);
        operands.push(text_argument(builder, arguments, 1, span)?);
    }
    let result = PlanType::List(ListLeaf::Str, 1);
    let output = call_destination(builder, destination, result.clone(), span)?;
    let local = output.local;
    external_call(
        builder,
        &format!("incan_native_runtime::{name}"),
        parameters,
        result,
        operands,
        output,
        span,
    );
    builder.owned.push(local);
    Ok(())
}

/// Join an admitted flat string list, materializing a projected element or slice once before borrowing it.
pub fn lower_join_helper(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 2 {
        return Err("invalid Body IR str_join arity".into());
    }
    let values = shared_receiver_place(builder, arguments, 1, span)?;
    let (leaf, depth) = require_list_shape(
        &place_type(&builder.locals, &values),
        "unsupported Body IR str_join input",
    )?;
    if leaf != ListLeaf::Str || depth != 1 {
        return Err("unsupported Body IR str_join element".into());
    }
    let operands = vec![
        text_argument(builder, arguments, 0, span)?,
        list_borrow(builder, values, false, span)?,
    ];
    let output = call_destination(builder, destination, PlanType::String, span)?;
    external_call(
        builder,
        "incan_native_runtime::join_text",
        vec![PlanType::StringRef, PlanType::ListRef(ListLeaf::Str, 1)],
        PlanType::String,
        operands,
        output,
        span,
    );
    Ok(())
}
