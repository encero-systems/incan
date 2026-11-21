//! Builtin registry methods: text, numeric and collection members the checker selected by identity.

use incan_lang::lang::surface::bytes_methods::BytesMethodId;
use incan_lang::lang::surface::float_methods::FloatMethodId;
use incan_lang::lang::surface::frozen_bytes_methods::FrozenBytesMethodId;
use incan_lang::lang::surface::frozen_dict_methods::FrozenDictMethodId;
use incan_lang::lang::surface::frozen_list_methods::FrozenListMethodId;
use incan_lang::lang::surface::frozen_set_methods::FrozenSetMethodId;
use incan_lang::lang::surface::option_methods::OptionMethodId;
use incan_lang::lang::surface::string_methods::StringMethodId;
use incan_mir_plan_native::{Operand, PlanType, SourceSpan};
use incan_semantics_core::body_ir::{
    ArgumentElement, MethodTarget, Operand as SourceOperand, OwnershipFact, Place as SourcePlace,
};

use crate::builtin_calls::coerce_numeric_operand;
use crate::checked_values::scalar_place;
use crate::derived_methods::lower_derived_clone;
use crate::element_places::is_element_place;
use crate::frame::{Builder, external_call, place_type, read_place};
use crate::hashed_collections::{collection_contains, dictionary_method, set_method};
use crate::list_iteration::frozen_dictionary_contains;
use crate::list_methods::{
    argument_value, call_destination, lower_bytes_as_slice, lower_iterator_method, lower_list_builtin, lower_list_iter,
    lower_list_method,
};
use crate::numerics::numeric_carrier;
use crate::operand_borrows::lower_text_helper;
use crate::pending::untranslated;
use crate::resize::emit_resize;
use crate::sequence_builtins::argument_type;
use crate::strings::text_method;

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
        Some(BytesMethodId::Decode) => return Err(untranslated("builtin_methods::lower_text_codec_method")),
        Some(BytesMethodId::AsSlice) => return lower_bytes_as_slice(builder, arguments, destination, span),
        Some(_) | None => {}
    }
    match target.builtin_frozen_bytes() {
        Some(FrozenBytesMethodId::Len) => return lower_list_builtin(builder, arguments, destination, span),
        Some(FrozenBytesMethodId::Decode) => return Err(untranslated("builtin_methods::lower_text_codec_method")),
        Some(FrozenBytesMethodId::IsEmpty) => {
            return Err("unsupported Body IR remaining builtin Method frozenbytes.is_empty".into());
        }
        Some(_) | None => {}
    }
    if let Some(method) = target.builtin_iterator().or_else(|| target.builtin_generator()) {
        return lower_iterator_method(builder, method, arguments, destination, span);
    }
    match target.builtin_option() {
        Some(OptionMethodId::Clone) => return lower_derived_clone(builder, arguments, destination, span),
        Some(_) => return refuse_remaining_builtin_method(target),
        None => {}
    }
    if target.builtin_result().is_some() {
        return refuse_remaining_builtin_method(target);
    }
    if target.builtin_clone().is_some() {
        return lower_derived_clone(builder, arguments, destination, span);
    }
    if let Some(method) = target.builtin_float() {
        return lower_selected_float(builder, target, method, arguments, destination, span);
    }
    if target.builtin_integer_helper_family().is_some() {
        if target.builtin_integer_helper_operation().is_none() {
            return Err("unsupported Body IR incomplete integer helper identity".into());
        }
        return Err(untranslated("builtin_methods::lower_integer_helper"));
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
    lower_collection_method(builder, target, arguments, destination, span)
}

/// Dispatch list, dictionary, set and frozen collection members to the collection lowering.
fn lower_collection_method(
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
            return Err(untranslated(
                "builtin_methods::refuse_unsupported_frozen_collection_method",
            ));
        }
        Some(_) | None => {}
    }
    match target.builtin_frozen_dict() {
        Some(FrozenDictMethodId::Len) => return lower_list_builtin(builder, arguments, destination, span),
        Some(FrozenDictMethodId::ContainsKey) => {
            return frozen_dictionary_contains(builder, arguments, destination, false, span);
        }
        Some(FrozenDictMethodId::IsEmpty) => {
            return Err(untranslated(
                "builtin_methods::refuse_unsupported_frozen_collection_method",
            ));
        }
        Some(_) | None => {}
    }
    match target.builtin_frozen_set() {
        Some(FrozenSetMethodId::Len) => return lower_list_builtin(builder, arguments, destination, span),
        Some(FrozenSetMethodId::Contains) => return collection_contains(builder, false, arguments, destination, span),
        Some(FrozenSetMethodId::IsEmpty) => {
            return Err(untranslated(
                "builtin_methods::refuse_unsupported_frozen_collection_method",
            ));
        }
        Some(_) | None => {}
    }
    Err("unsupported Body IR builtin method operation".into())
}

/// Lower one checker-selected runtime or frozen string operation.
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
        return Err(untranslated("builtin_methods::lower_text_codec_method"));
    }
    if method == StringMethodId::Slice {
        if frozen {
            return Err("unsupported Body IR frozen string slice operation".into());
        }
        return Err(untranslated("builtin_methods::lower_string_slice"));
    }
    let receiver_type = argument_type(builder, arguments, 0, span)?;
    match text_method(method, &receiver_type) {
        Some((name, result)) => lower_text_helper(builder, name, result, arguments, destination, span),
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
        return lower_float_square_root(builder, &receiver, arguments, destination, span);
    }
    match float_method(method, &receiver) {
        Some((name, parameters, result)) => {
            lower_float_method(builder, &name, parameters, result, arguments, destination, span)
        }
        None => refuse_remaining_builtin_method(target),
    }
}

/// Call native square root at the checker's float result type, widening an f32 receiver before evaluation.
fn lower_float_square_root(
    builder: &mut Builder,
    receiver: &PlanType,
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
    let narrow = *receiver == PlanType::F32;
    match method {
        FloatMethodId::Powf if !narrow => Some((
            "f64::powf".into(),
            vec![receiver.clone(), receiver.clone()],
            receiver.clone(),
        )),
        FloatMethodId::Abs if !narrow => Some(("f64::abs".into(), vec![receiver.clone()], PlanType::Float)),
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
    if let Some(SourceOperand::Place(read)) = arguments[0].as_one() {
        // A list or dictionary element has no storage to copy from; its materialized value is read below.
        if read.fact == OwnershipFact::Borrow && !is_element_place(builder, &read.place)? {
            let place = scalar_place(builder, &read.place, span)?;
            if !numeric_carrier(&place_type(&builder.locals, &place)) {
                return Err("invalid Body IR numeric receiver outside numeric storage".into());
            }
            return Ok(read_place(place, false, span));
        }
    }
    argument_value(builder, arguments, 0, span)
}
