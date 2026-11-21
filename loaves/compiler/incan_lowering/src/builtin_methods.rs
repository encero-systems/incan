//! Lower builtin members on text, integers, floats, and generators.

use incan_lang::lang::surface::bytes_methods::BytesMethodId;
use incan_lang::lang::surface::float_methods::FloatMethodId;
use incan_lang::lang::surface::frozen_bytes_methods::FrozenBytesMethodId;
use incan_lang::lang::surface::frozen_dict_methods::FrozenDictMethodId;
use incan_lang::lang::surface::frozen_list_methods::FrozenListMethodId;
use incan_lang::lang::surface::frozen_set_methods::FrozenSetMethodId;
use incan_lang::lang::surface::iterator_methods::IteratorMethodId;
use incan_lang::lang::surface::option_methods::OptionMethodId;
use incan_lang::lang::surface::string_methods::StringMethodId;
use incan_lang::lang::types::numerics::{IntegerHelperFamily, IntegerHelperOperation};
use incan_mir_plan_native::{
    BinaryOp, ListLeaf, Operand, Place, PlanType, Projection, RvalueKind, SourceSpan, assign as plan_assign,
};
use incan_semantics_core::body_ir::{
    ArgumentElement, MethodTarget, Operand as SourceOperand, OwnershipFact, Place as SourcePlace,
};

use crate::builtin_calls::{assign_text_result, coerce_numeric_operand};
use crate::checked_values::{scalar_operand, scalar_place};
use crate::element_places::is_element_place;
use crate::frame::{Builder, drop_owned, external_call, place_type, read_place, temporary, value_type};
use crate::generator_adapters::generator_adapter;
use crate::generator_consumers::{consumer_result, generator_consumer, is_generator_consumer};
use crate::hashed_collections::{collection_contains, dictionary_method, set_method};
use crate::list_iteration::{frozen_dictionary_contains, list_root_place};
use crate::list_methods::{
    argument_place, argument_value, borrowed_list_argument, call_destination, lower_bytes_as_slice,
    lower_iterator_method, lower_list_builtin, lower_list_iter, lower_list_method,
};
use crate::lists::require_list_shape;
use crate::numerics::{integer_carrier, numeric_carrier};
use crate::operand_borrows::borrow_text_operand;
use crate::options::replace_owned_destination;
use crate::pending::untranslated;
use crate::resize::emit_resize;
use crate::sequence_builtins::{argument_type, text_argument};
use crate::strings::{emit_text_helper, text_method};
use crate::text_ownership::drop_borrowed_text_temporaries;
use crate::text_primitives::encode_utf8;

/// Dispatch retained builtin registry members to text, numeric resize, or collection boundaries.
pub fn lower_builtin_method(
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if target.has_builtin_selection() {
        if !target.has_valid_builtin_selection() {
            return Err("unsupported Body IR builtin method operation without matching canonical provenance".into());
        }
        return lower_typed_builtin_method(builder, target, arguments, destination, span);
    }
    Err(format!(
        "unsupported Body IR Method {} without checker-selected builtin identity in {} at {}:{}:{}",
        target.name, builder.name, span.file, span.line, span.column
    ))
}

/// Dispatch one checker-selected builtin operation without reclassifying its canonical name.
///
/// Option and clone members clone through `derived_methods::lower_derived_clone`, which refuses by name until it is in
/// Rust.
fn lower_typed_builtin_method(
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if let Some(method) = target.builtin_str() {
        return lower_string_method(builder, target, method, false, arguments, destination, span);
    }
    if let Some(method) = target.builtin_frozen_str() {
        return lower_string_method(builder, target, method, true, arguments, destination, span);
    }
    match target.builtin_bytes() {
        Some(BytesMethodId::Decode) => {
            return lower_text_codec_method(builder, false, true, arguments, destination, span);
        }
        Some(BytesMethodId::AsSlice) => return lower_bytes_as_slice(builder, arguments, destination, span),
        None => {}
    }
    match target.builtin_frozen_bytes() {
        Some(FrozenBytesMethodId::Len) => return lower_list_builtin(builder, arguments, destination, span),
        Some(FrozenBytesMethodId::Decode) => {
            return lower_text_codec_method(builder, true, true, arguments, destination, span);
        }
        Some(FrozenBytesMethodId::IsEmpty) => {
            return Err("unsupported Body IR remaining builtin Method frozenbytes.is_empty".into());
        }
        None => {}
    }
    if let Some(method) = target.builtin_iterator().or_else(|| target.builtin_generator()) {
        return lower_iterator_method(builder, method, arguments, destination, span);
    }
    if let Some(method) = target.builtin_option() {
        if method == OptionMethodId::Clone {
            return Err(untranslated("derived_methods::lower_derived_clone"));
        }
        return refuse_remaining_builtin_method(target);
    }
    if target.builtin_result().is_some() {
        return refuse_remaining_builtin_method(target);
    }
    if target.builtin_clone().is_some() {
        return Err(untranslated("derived_methods::lower_derived_clone"));
    }
    if let Some(method) = target.builtin_float() {
        return lower_selected_float(builder, target, method, arguments, destination, span);
    }
    if let Some(family) = target.builtin_integer_helper_family() {
        let Some(operation) = target.builtin_integer_helper_operation() else {
            return Err("unsupported Body IR incomplete integer helper identity".into());
        };
        return lower_integer_helper(builder, target, family, operation, arguments, destination, span);
    }
    if let Some(policy) = target.builtin_numeric_resize() {
        if arguments.len() != 1 {
            return Err("invalid Body IR resize arity".into());
        }
        let receiver_type = argument_type(builder, arguments, 0, span)?;
        let Some(place) = destination else {
            return Err("unsupported Body IR resize without checked destination".into());
        };
        let output = scalar_place(builder, place, span)?;
        let operand = numeric_receiver(builder, arguments, span)?;
        return emit_resize(builder, policy, receiver_type, operand, output, span);
    }
    lower_collection_builtin_method(builder, target, arguments, destination, span)
}

/// Dispatch the list, dictionary, set and frozen collection members of the builtin registry.
fn lower_collection_builtin_method(
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if let Some(method) = target.builtin_list() {
        return lower_list_method(builder, method, arguments, destination, span);
    }
    if target.is_builtin_list_iter() {
        return lower_list_iter(builder, arguments, destination, span);
    }
    if let Some(method) = target.builtin_dict() {
        return dictionary_method(builder, method, arguments, destination, span);
    }
    if let Some(method) = target.builtin_set() {
        return set_method(builder, method, arguments, destination, span);
    }
    match target.builtin_frozen_list() {
        Some(FrozenListMethodId::Len) => return lower_list_builtin(builder, arguments, destination, span),
        Some(FrozenListMethodId::IsEmpty) => {
            return refuse_unsupported_frozen_collection_method(builder, "FrozenList.is_empty", arguments, span);
        }
        None => {}
    }
    match target.builtin_frozen_dict() {
        Some(FrozenDictMethodId::Len) => return lower_list_builtin(builder, arguments, destination, span),
        Some(FrozenDictMethodId::ContainsKey) => {
            return frozen_dictionary_contains(builder, arguments, destination, false, span);
        }
        Some(FrozenDictMethodId::IsEmpty) => {
            return refuse_unsupported_frozen_collection_method(builder, "FrozenDict.is_empty", arguments, span);
        }
        None => {}
    }
    match target.builtin_frozen_set() {
        Some(FrozenSetMethodId::Len) => return lower_list_builtin(builder, arguments, destination, span),
        Some(FrozenSetMethodId::Contains) => return collection_contains(builder, false, arguments, destination, span),
        Some(FrozenSetMethodId::IsEmpty) => {
            return refuse_unsupported_frozen_collection_method(builder, "FrozenSet.is_empty", arguments, span);
        }
        None => {}
    }
    Err("unsupported Body IR builtin method operation".into())
}

/// Lower one checker-selected runtime or frozen string operation.
///
/// A selected text helper is `operand_borrows::lower_text_helper`, which refuses by name until it is in Rust.
fn lower_string_method(
    builder: &mut Builder,
    target: &MethodTarget,
    method: StringMethodId,
    frozen: bool,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if method == StringMethodId::Encode {
        return lower_text_codec_method(builder, frozen, false, arguments, destination, span);
    }
    if method == StringMethodId::Slice {
        if frozen {
            return Err("unsupported Body IR frozen string slice operation".into());
        }
        return lower_string_slice(builder, arguments, destination, span);
    }
    let receiver_type = argument_type(builder, arguments, 0, span)?;
    match text_method(method, &receiver_type) {
        Some(_) => Err(untranslated("operand_borrows::lower_text_helper")),
        None => refuse_remaining_builtin_method(target),
    }
}

/// Preserve the existing non-list refusal for an unsupported frozen-collection member.
fn refuse_unsupported_frozen_collection_method(
    builder: &mut Builder,
    member: &str,
    arguments: &[ArgumentElement],
    span: &SourceSpan,
) -> Result<(), String> {
    let receiver = argument_type(builder, arguments, 0, span)?;
    let description = format!(
        "unsupported Body IR list Method {member} on non-list in {}",
        builder.name
    );
    let source_location = format!("{}:{}:{}", span.file, span.line, span.column);
    require_list_shape(&receiver, &format!("{description} at {source_location}"))?;
    Err(format!("unsupported Body IR list Method {member}"))
}

/// Lower wrapping arithmetic directly and call native checked and saturating u8 methods at their exact widths.
fn lower_integer_helper(
    builder: &mut Builder,
    target: &MethodTarget,
    family: IntegerHelperFamily,
    operation: IntegerHelperOperation,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let receiver = argument_type(builder, arguments, 0, span)?;
    if family == IntegerHelperFamily::Wrapping && operation != IntegerHelperOperation::Pow {
        return lower_wrapping_integer(builder, operation, receiver, arguments, destination, span);
    }
    if receiver != PlanType::U8 {
        return refuse_remaining_builtin_method(target);
    }
    if arguments.len() != 2 {
        return Err("unsupported Body IR integer helper arity".into());
    }
    let operand_type = if operation == IntegerHelperOperation::Pow {
        PlanType::U32
    } else {
        PlanType::U8
    };
    let mut result = PlanType::U8;
    if family == IntegerHelperFamily::Checked {
        let Some(place) = destination else {
            return Err("unsupported Body IR checked integer helper without destination".into());
        };
        let output = scalar_place(builder, place, span)?;
        result = place_type(&builder.locals, &output);
    }
    let output = call_destination(builder, destination, result.clone(), span)?;
    let value = argument_value(builder, arguments, 1, span)?;
    let operands = vec![
        numeric_receiver(builder, arguments, span)?,
        coerce_numeric_operand(builder, value, &operand_type, span)?,
    ];
    external_call(
        builder,
        &format!("u8::{}", integer_helper_name(family, operation)),
        vec![PlanType::U8, operand_type],
        result,
        operands,
        output,
        span,
    );
    Ok(())
}

/// Emit the checked integer member as native modular arithmetic without invoking a runtime wrapper recursively.
fn lower_wrapping_integer(
    builder: &mut Builder,
    method: IntegerHelperOperation,
    receiver: PlanType,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if !integer_carrier(&receiver) {
        return Err("unsupported Body IR wrapping helper on a non-integer receiver".into());
    }
    let binary = match method {
        IntegerHelperOperation::Add => BinaryOp::Add,
        IntegerHelperOperation::Sub => BinaryOp::Subtract,
        IntegerHelperOperation::Mul => BinaryOp::Multiply,
        IntegerHelperOperation::Pow => return Err("unsupported Body IR direct wrapping pow".into()),
    };
    if arguments.len() != 2 {
        return Err("unsupported Body IR wrapping integer arity".into());
    }
    let output = call_destination(builder, destination, receiver.clone(), span)?;
    let left = numeric_receiver(builder, arguments, span)?;
    let value = argument_value(builder, arguments, 1, span)?;
    let right = coerce_numeric_operand(builder, value, &receiver, span)?;
    if receiver == PlanType::Int {
        let pair = temporary(builder, PlanType::CheckedInt, span);
        builder.emit(plan_assign(pair.clone(), RvalueKind::Binary(binary, left, right), span));
        let wrapped = Place {
            local: pair.local,
            projection: Projection::Value,
            span: span.clone(),
        };
        builder.emit(plan_assign(
            output,
            RvalueKind::Use(read_place(wrapped, false, span)),
            span,
        ));
        return Ok(());
    }
    builder.emit(plan_assign(output, RvalueKind::Binary(binary, left, right), span));
    Ok(())
}

/// Choose the native primitive spelling after typed integer-helper dispatch.
fn integer_helper_name(family: IntegerHelperFamily, operation: IntegerHelperOperation) -> String {
    let prefix = match family {
        IntegerHelperFamily::Checked => "checked",
        IntegerHelperFamily::Wrapping => "wrapping",
        IntegerHelperFamily::Saturating => "saturating",
    };
    let suffix = match operation {
        IntegerHelperOperation::Add => "add",
        IntegerHelperOperation::Sub => "sub",
        IntegerHelperOperation::Mul => "mul",
        IntegerHelperOperation::Pow => "pow",
    };
    format!("{prefix}_{suffix}")
}

/// Consume the checked primitive slice member with its borrowed receiver and two signed int endpoints.
fn lower_string_slice(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 3 {
        return Err("unsupported Body IR string slice argument count".into());
    }
    let incoming = builder.owned.clone();
    let mut operands = Vec::with_capacity(3);
    let Some(receiver) = arguments[0].as_one() else {
        return Err("unsupported Body IR string slice receiver spread".into());
    };
    operands.push(borrow_text_operand(builder, receiver, span)?);
    for argument in &arguments[1..3] {
        let Some(value) = argument.as_one() else {
            return Err("unsupported Body IR string slice bound spread".into());
        };
        let operand = scalar_operand(builder, value, span)?;
        if value_type(builder, &operand)? != PlanType::Int {
            return Err("unsupported Body IR non-int string slice bound".into());
        }
        operands.push(operand);
    }
    let text = emit_text_helper(
        builder,
        "slice_text_bounds",
        PlanType::String,
        vec![PlanType::StringRef, PlanType::Int, PlanType::Int],
        operands,
        span,
    );
    drop_borrowed_text_temporaries(builder, &incoming, text.local, span);
    assign_text_result(builder, text, destination, span)
}

/// Call the exact borrowed UTF-8 runtime boundary after the frontend fills all checked codec slots.
///
/// A decode reads its Result carrier through `options::carrier_layout`, which refuses by name until it is in Rust.
fn lower_text_codec_method(
    builder: &mut Builder,
    frozen: bool,
    decoding: bool,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let arity = if decoding { 3 } else { 2 };
    if arguments.len() != arity {
        return Err("unsupported Body IR incomplete builtin codec arguments".into());
    }
    let incoming = builder.owned.clone();
    let receiver_type = argument_type(builder, arguments, 0, span)?;
    let mut operands = Vec::with_capacity(arity);
    let text = matches!(receiver_type, PlanType::String | PlanType::StringRef);
    if !decoding && !frozen && text {
        operands.push(text_argument(builder, arguments, 0, span)?);
    } else if !decoding && frozen && receiver_type == PlanType::FrozenStr {
        operands.push(argument_value(builder, arguments, 0, span)?);
    } else if decoding && !frozen {
        let (leaf, depth) = require_list_shape(&receiver_type, "unsupported Body IR nonbytes codec receiver")?;
        if leaf != ListLeaf::U8 || depth != 1 {
            return Err("unsupported Body IR nonbytes codec receiver".into());
        }
        operands.push(borrowed_list_argument(builder, arguments, 0, false, span)?);
    } else if decoding && frozen && receiver_type == PlanType::FrozenBytes {
        operands.push(argument_value(builder, arguments, 0, span)?);
    } else {
        return Err("unsupported Body IR builtin codec identity with a different receiver".into());
    }
    for index in 1..arity {
        operands.push(text_argument(builder, arguments, index, span)?);
    }
    if decoding {
        let Some(place) = destination else {
            return Err("unsupported Body IR discarded codec result without a checked carrier".into());
        };
        list_root_place(builder, place, span)?;
        return Err(untranslated("options::carrier_layout"));
    }
    let result = PlanType::List(ListLeaf::U8, 1);
    let output = call_destination(builder, destination, result.clone(), span)?;
    if place_type(&builder.locals, &output) != result {
        return Err("unsupported Body IR encode result carrier".into());
    }
    let local = output.local;
    lower_primitive_encode(builder, frozen, operands, output, span);
    drop_borrowed_text_temporaries(builder, &incoming, local, span);
    Ok(())
}

/// Preserve the codec's label error in its owner, then transfer primitive-owned bytes to the checked result.
fn lower_primitive_encode(
    builder: &mut Builder,
    frozen: bool,
    operands: Vec<Operand>,
    output: Place,
    span: &SourceSpan,
) {
    let mut operands = operands.into_iter();
    let (Some(value), Some(label)) = (operands.next(), operands.next()) else {
        return;
    };
    let validated = temporary(builder, PlanType::Unit, span);
    external_call(
        builder,
        "incan_native_runtime::validate_encoder_label",
        vec![PlanType::StringRef],
        PlanType::Unit,
        vec![label],
        validated,
        span,
    );
    let encoded = encode_utf8(builder, value, frozen, span);
    replace_owned_destination(builder, output, RvalueKind::Use(read_place(encoded, true, span)), span);
}

/// Lower one checker-selected float operation without reclassifying its canonical name.
fn lower_selected_float(
    builder: &mut Builder,
    target: &MethodTarget,
    method: FloatMethodId,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let receiver = argument_type(builder, arguments, 0, span)?;
    if method == FloatMethodId::Powf && receiver == PlanType::F32 {
        return Err(
            "unsupported Body IR f32 powf: its checked float result differs from the carrier's f32 powf".into(),
        );
    }
    if method == FloatMethodId::Sqrt {
        return lower_float_square_root(builder, receiver, arguments, destination, span);
    }
    match float_method(method, &receiver) {
        Some((name, parameters, result)) => {
            lower_float_method(builder, &name, parameters, result, arguments, destination, span)
        }
        None => refuse_remaining_builtin_method(target),
    }
}

/// Preserve the established lowering refusal after typed dispatch identifies an unsupported operation.
fn refuse_remaining_builtin_method(target: &MethodTarget) -> Result<(), String> {
    match target.builtin_label() {
        Some(member) => Err(format!("unsupported Body IR remaining builtin Method {member}")),
        None => Err("unsupported Body IR builtin method operation".into()),
    }
}

/// Call native square root at the checker's float result type, widening an f32 receiver before evaluation.
fn lower_float_square_root(
    builder: &mut Builder,
    receiver: PlanType,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 1 {
        return Err("invalid Body IR square-root arity".into());
    }
    if !matches!(receiver, PlanType::F32 | PlanType::Float | PlanType::F64) {
        return Err("invalid Body IR square-root receiver".into());
    }
    let output = call_destination(builder, destination, PlanType::Float, span)?;
    let operand = numeric_receiver(builder, arguments, span)?;
    let widened = coerce_numeric_operand(builder, operand, &PlanType::Float, span)?;
    external_call(
        builder,
        "std::primitive::f64::sqrt",
        vec![PlanType::Float],
        PlanType::Float,
        vec![widened],
        output,
        span,
    );
    Ok(())
}

/// Select the checked native float member; these primitives must not reenter their runtime wrappers.
fn float_method(method: FloatMethodId, receiver: &PlanType) -> Option<(String, Vec<PlanType>, PlanType)> {
    let width = match receiver {
        PlanType::F32 => "f32",
        PlanType::Float | PlanType::F64 => "f64",
        _ => return None,
    };
    let wide = *receiver != PlanType::F32;
    match method {
        FloatMethodId::Powf if wide => Some((
            "f64::powf".into(),
            vec![receiver.clone(), receiver.clone()],
            receiver.clone(),
        )),
        FloatMethodId::Abs if wide => Some(("f64::abs".into(), vec![receiver.clone()], PlanType::Float)),
        FloatMethodId::IsFinite => Some((format!("{width}::is_finite"), vec![receiver.clone()], PlanType::Bool)),
        FloatMethodId::IsNan => Some((format!("{width}::is_nan"), vec![receiver.clone()], PlanType::Bool)),
        _ => None,
    }
}

/// Call the native float member with the copied receiver and each argument at its checked carrier.
fn lower_float_method(
    builder: &mut Builder,
    name: &str,
    parameters: Vec<PlanType>,
    result: PlanType,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != parameters.len() {
        return Err("unsupported Body IR float Method arity".into());
    }
    let mut operands = vec![numeric_receiver(builder, arguments, span)?];
    for (index, parameter) in parameters.iter().enumerate().skip(1) {
        let argument = argument_value(builder, arguments, index, span)?;
        operands.push(coerce_numeric_operand(builder, argument, parameter, span)?);
    }
    let output = call_destination(builder, destination, result.clone(), span)?;
    external_call(builder, name, parameters, result, operands, output, span);
    Ok(())
}

/// Read a canonical numeric method's shared receiver without consuming its Copy storage.
fn numeric_receiver(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    span: &SourceSpan,
) -> Result<Operand, String> {
    // A list or dictionary element has no storage to copy from; its materialized value is read below.
    if let Some(SourceOperand::Place(read)) = arguments[0].as_one()
        && read.fact == OwnershipFact::Borrow
        && !is_element_place(builder, &read.place)?
    {
        let place = scalar_place(builder, &read.place, span)?;
        if !numeric_carrier(&place_type(&builder.locals, &place)) {
            return Err("invalid Body IR numeric receiver outside numeric storage".into());
        }
        return Ok(read_place(place, false, span));
    }
    argument_value(builder, arguments, 0, span)
}

/// Move the owned generator receiver into one runtime adapter or consumer, as legacy's member consumes its receiver.
///
/// An adapter's result is another owned generator; a consumer drains the receiver into its checked destination.
pub fn lower_generator_member(
    builder: &mut Builder,
    method: IteratorMethodId,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    receiver_type: PlanType,
    span: &SourceSpan,
) -> Result<(), String> {
    let receiver = argument_place(builder, arguments, 0, span)?;
    if receiver.projection != Projection::Whole || place_type(&builder.locals, &receiver) != receiver_type {
        return Err("unsupported Body IR generator member receiver".into());
    }
    let mut operands = Vec::with_capacity(arguments.len().saturating_sub(1));
    for index in 1..arguments.len() {
        operands.push(argument_value(builder, arguments, index, span)?);
    }
    let source = read_place(receiver, true, span);
    if is_generator_consumer(method) {
        let output = call_destination(builder, destination, consumer_result(method, &receiver_type), span)?;
        drop_owned(builder, output.local, span);
        return generator_consumer(builder, method, source, operands, output, span);
    }
    let output = call_destination(builder, destination, receiver_type, span)?;
    drop_owned(builder, output.local, span);
    let local = output.local;
    generator_adapter(builder, method, source, operands, output, span)?;
    builder.owned.push(local);
    Ok(())
}
