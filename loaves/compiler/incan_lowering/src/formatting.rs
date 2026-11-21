//! Lower formatted strings and the display and debug operands they format.

use incan_mir_plan_native::{
    CalleeKind, Carrier, Constant, Operand, OperandKind, Place, PlanType, Projection, RvalueKind, SourceSpan,
    assign as plan_assign, carrier_position, is_carrier_index,
};
use incan_semantics_core::body_ir::{
    Constant as SourceConstant, FormatPart, FormatStyle, Operand as SourceOperand, OwnershipFact, Place as SourcePlace,
    Rvalue,
};

use crate::checked_values::{clone_nominal_operand, scalar_place};
use crate::collections::{is_collection, is_owned_collection};
use crate::element_places::{element_place_type, is_element_place};
use crate::enum_display::{borrowed_enum_debug, display_enum_operand, native_enum_owner, native_enum_text};
use crate::frame::{Builder, drop_owned, forget_owned, place_type, read_place, temporary, terminate_call, value_type};
use crate::list_values::{element_read, value_operand};
use crate::lists::{is_list, is_owned_list};
use crate::model_construction::lower_string_field_assignment;
use crate::model_values::{debug_model_operand, display_model_operand};
use crate::numerics::{has_sized_numeric, numeric_carrier, numeric_name};
use crate::option_tests::is_union_carrier;
use crate::options::{clone_tuple_operand, format_carrier};
use crate::text_plan::{
    clone_text, clone_text_reference, format_parts, owned_borrowed_text, owned_literal, owned_slice_text,
};
use crate::tuples::{display_tuple, is_tuple_type, tuple_debug_formatted};
use crate::validation_errors::{display_validation_error, validation_error_reference};
use crate::value_enum_methods::{select_value_enum_raw, value_enum_raw_type};

/// Lower an f-string, retaining literal boundaries and the checked display or nominal Debug style.
pub fn lower_format(
    builder: &mut Builder,
    destination: Place,
    retained: &[FormatPart],
    span: &SourceSpan,
) -> Result<(), String> {
    let mut parts = vec![String::new()];
    let mut values: Vec<Place> = Vec::new();
    for part in retained {
        append_format_part(builder, part, &mut parts, &mut values, span)?;
    }
    format_parts(builder, destination, &parts, &values, span);
    Ok(())
}

/// Add one f-string part: literal text extends the current text run, a value starts a new one.
///
/// `parts` always has one more entry than `values`: the text before, between and after the values.
fn append_format_part(
    builder: &mut Builder,
    part: &FormatPart,
    parts: &mut Vec<String>,
    values: &mut Vec<Place>,
    span: &SourceSpan,
) -> Result<(), String> {
    match part {
        FormatPart::Literal(text) => {
            if let Some(last) = parts.last_mut() {
                last.push_str(text);
            }
        }
        FormatPart::Expr { operand, style } => {
            let value = match style {
                FormatStyle::Display => display_operand(builder, operand, span)?,
                FormatStyle::Debug => debug_operand(builder, operand, span)?,
                FormatStyle::TypeParameterDisplay => display_value_operand(builder, operand, span, true)?,
            };
            values.push(value);
            parts.push(String::new());
        }
    }
    Ok(())
}

/// Borrow structural nominal places through their retained Debug implementation before materializing other values.
fn debug_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Place, String> {
    if let Some(text) = native_enum_text(builder, value, true, span)? {
        return Ok(text);
    }
    if let SourceOperand::Place(read) = value {
        if is_element_place(builder, &read.place)? {
            // An indexed model, including an opaque native one, materializes once for its retained Debug call.
            if let PlanType::Model(..) = element_place_type(builder, &read.place)? {
                return debug_model_operand(builder, value, span);
            }
        } else if read.place.global().is_none() {
            let place = scalar_place(builder, &read.place, span)?;
            match place_type(&builder.locals, &place) {
                PlanType::Model(..) => return debug_model_operand(builder, value, span),
                PlanType::Enum(owner, _) => return debug_enum_operand(builder, place, owner, span),
                PlanType::ValidationError | PlanType::ValidationErrorRef => {
                    let reference = validation_error_reference(builder, place, span)?;
                    return debug_plan_operand(builder, reference, span);
                }
                PlanType::StringRef | PlanType::StrRef => {
                    return debug_plan_operand(builder, read_place(place, false, span), span);
                }
                _ => {}
            }
        }
    }
    let operand = value_operand(builder, value, span)?;
    let ty = value_type(builder, &operand)?;
    if let PlanType::Enum(owner, _) = &ty {
        let declared_debug = builder
            .enums()
            .get(*owner as usize)
            .is_some_and(|declaration| declaration.derives.iter().any(|derive| derive == "Debug"));
        if native_enum_owner(builder, *owner) || declared_debug {
            return debug_plan_operand(builder, operand, span);
        }
    }
    if has_sized_numeric(&ty)
        || matches!(
            ty,
            PlanType::Int
                | PlanType::Float
                | PlanType::Bool
                | PlanType::Unit
                | PlanType::String
                | PlanType::StringRef
                | PlanType::StrRef
        )
        || is_owned_list(&ty)
    {
        return debug_plan_operand(builder, operand, span);
    }
    Err("unsupported Body IR nonmodel FormatStyle.Debug".into())
}

/// Display a whole local tuple while leaving global statics and other carriers on their existing paths.
fn display_tuple_place(builder: &mut Builder, value: &SourcePlace, span: &SourceSpan) -> Result<Option<Place>, String> {
    if value.local_id().is_none() || !value.projection.is_empty() {
        return Ok(None);
    }
    let target = scalar_place(builder, value, span)?;
    let ty = place_type(&builder.locals, &target);
    if !is_tuple_type(&ty) {
        return Ok(None);
    }
    if tuple_debug_formatted(&ty) {
        // A list or carrier element has no flat formatter; Debug of a snapshot spells the whole tuple as legacy does.
        let snapshot = clone_tuple_operand(builder, target, span)?;
        return Ok(Some(debug_plan_operand(builder, snapshot, span)?));
    }
    Ok(Some(display_tuple(builder, target, span)?))
}

/// Produce an owned `String` holding a value's display text, honoring the value's ownership fact.
pub fn display_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Place, String> {
    display_value_operand(builder, value, span, false)
}

/// Produce an owned `String` holding a value's display text, honoring the value's ownership fact.
///
/// A value whose checked type is a type parameter displays as generic code displays it (see `display_plan_value`).
fn display_value_operand(
    builder: &mut Builder,
    value: &SourceOperand,
    span: &SourceSpan,
    type_parameter: bool,
) -> Result<Place, String> {
    if let Some(text) = display_model_operand(builder, value, span)? {
        return Ok(text);
    }
    if let Some(text) = display_enum_operand(builder, value, span)? {
        return Ok(text);
    }
    // Indexed collection reads come first: their places are not model places, so the carrier check below cannot map
    // them.
    if let SourceOperand::Place(read) = value {
        // Retained globals are scalar-only and have no frame-local slot for carrier inspection.
        if read.place.global().is_some() {
            let operand = value_operand(builder, value, span)?;
            return display_plan_value(builder, operand, span, type_parameter);
        }
        if let Some(item) = element_read(builder, &read.place, span)? {
            return display_plan_value(builder, item, span, type_parameter);
        }
        if let Some(text) = display_tuple_place(builder, &read.place, span)? {
            return Ok(text);
        }
        let place = scalar_place(builder, &read.place, span)?;
        let ty = place_type(&builder.locals, &place);
        if is_list(&ty) || is_collection(&ty) {
            let operand = value_operand(builder, value, span)?;
            return display_plan_value(builder, operand, span, type_parameter);
        }
        match &ty {
            PlanType::ValidationError | PlanType::ValidationErrorRef => {
                return display_validation_error(builder, place, span);
            }
            PlanType::Enum(owner, _) => {
                if *owner >= builder.enums().len() as i64
                    && builder.intrinsics[carrier_position(*owner) as usize].carrier != Carrier::Declared
                {
                    let result = format_carrier(builder, place.clone(), span)?;
                    if read.fact == OwnershipFact::Move {
                        drop_owned(builder, place.local, span);
                    }
                    return Ok(result);
                }
                let value_enum = value_enum_owner(builder, *owner);
                if value_enum >= 0 {
                    return display_value_enum(builder, value_enum as usize, place, span);
                }
            }
            _ => {}
        }
        // A cloned or borrowed string is copied into a new owned string; the original stays where it is.
        if read.fact == OwnershipFact::Clone || read.fact == OwnershipFact::Borrow {
            return clone_display_place(builder, &read.place, span, type_parameter);
        }
    }
    if let SourceOperand::Constant(constant) = value
        && let Some(text) = literal_text(constant)
    {
        return Ok(owned_literal(builder, text, span));
    }
    let operand = value_operand(builder, value, span)?;
    display_plan_value(builder, operand, span, type_parameter)
}

/// Read literal bytes for text observation without discarding the constant's retained storage identity.
fn literal_text(value: &SourceConstant) -> Option<&str> {
    match value {
        SourceConstant::Str(text) | SourceConstant::FrozenStr(text) => Some(text),
        _ => None,
    }
}

/// Find the value enum whose retained layout is source enum `index`, or -1 for any other enum.
fn value_enum_owner(builder: &Builder, index: i64) -> i64 {
    let Some(declaration) = builder.enums().get(index as usize) else {
        return -1;
    };
    builder
        .value_enums()
        .iter()
        .position(|value_enum| declaration.canonical == value_enum.canonical)
        .map_or(-1, |owner| owner as i64)
}

/// Display a value enum as its raw value, as legacy's Display does: the text itself, or the integer's digits.
fn display_value_enum(builder: &mut Builder, owner: usize, source: Place, span: &SourceSpan) -> Result<Place, String> {
    let ty = value_enum_raw_type(builder, owner);
    let raw = temporary(builder, ty.clone(), span);
    select_value_enum_raw(builder, owner, source, raw.clone(), span)?;
    if ty == PlanType::String {
        return Ok(raw);
    }
    display_plan_operand(builder, read_place(raw, false, span), span)
}

/// Format a materialized scalar without re-reading its source ownership fact.
pub fn display_plan_operand(builder: &mut Builder, operand: Operand, span: &SourceSpan) -> Result<Place, String> {
    display_plan_value(builder, operand, span, false)
}

/// Format a materialized scalar, as generic code formats it when its checked type is a type parameter, retaining a
/// native enum's own Display implementation. A retained shared string reference produces owned display text through
/// the ordinary text materialization boundary.
///
/// A list, set or dictionary displays through its structure, as legacy's Debug spells it. A shared scalar reference
/// retains its pointer once, then copies its numeric or boolean referent into the ordinary scalar formatter; a shared
/// frozen or static text reference displays through its native Display.
pub fn display_plan_value(
    builder: &mut Builder,
    operand: Operand,
    span: &SourceSpan,
    type_parameter: bool,
) -> Result<Place, String> {
    let ty = value_type(builder, &operand)?;
    match &ty {
        PlanType::Enum(owner, _) => {
            if native_enum_owner(builder, *owner) {
                return native_format_text(builder, operand, false, span);
            }
        }
        PlanType::FrozenCollection(..) => return native_format_text(builder, operand, true, span),
        PlanType::ScalarRef(elements) => {
            if elements.len() != 1 {
                return Err("invalid Body IR scalar display reference arity".into());
            }
            if type_parameter {
                return native_format_text(builder, operand, false, span);
            }
            let referent = elements[0].clone();
            // Frozen text displays as its ordinary counterpart, through the borrowed carrier's native Display.
            if referent == PlanType::StaticText || referent == PlanType::FrozenStr {
                return native_format_text(builder, operand, false, span);
            }
            if !numeric_carrier(&referent)
                && !matches!(referent, PlanType::Bool | PlanType::StaticBytes | PlanType::FrozenBytes)
            {
                return Err("unsupported Body IR referenced display type".into());
            }
            let reference = temporary(builder, ty.clone(), span);
            builder.emit(plan_assign(reference.clone(), RvalueKind::Use(operand), span));
            let value = Place {
                local: reference.local,
                projection: Projection::Deref(referent),
                span: span.clone(),
            };
            return display_plan_value(builder, read_place(value, false, span), span, type_parameter);
        }
        _ => {}
    }
    // Generic code uses the argument's native Display, preserving generic float spelling and avoiding a cycle
    // through sized numeric runtime formatters whose implementations use this same generic display operation.
    if type_parameter && (ty == PlanType::Float || has_sized_numeric(&ty)) {
        return native_format_text(builder, operand, false, span);
    }
    if is_owned_list(&ty) || is_owned_collection(&ty) {
        return native_format_text(builder, operand, true, span);
    }
    match ty {
        PlanType::StringRef => return clone_text_reference(builder, operand, span),
        PlanType::String => return display_string_operand(builder, operand, span),
        PlanType::StringMutRef => return exclusive_text_display(builder, operand, span),
        PlanType::StrRef => return Ok(owned_slice_text(builder, operand, span)),
        PlanType::StaticText => return native_format_text(builder, operand, false, span),
        _ => {}
    }
    let name = if has_sized_numeric(&ty) {
        format!("format_{}", numeric_name(&ty))
    } else {
        match ty {
            PlanType::Decimal => "format_decimal",
            PlanType::FrozenStr => "format_frozen",
            PlanType::Int => "format_int",
            PlanType::Float => "format_float",
            PlanType::Bool => "format_bool",
            _ => return Err("unsupported Body IR display type".into()),
        }
        .to_string()
    };
    let result = temporary(builder, PlanType::String, span);
    crate::frame::external_call(
        builder,
        &format!("incan_native_runtime::{name}"),
        vec![ty.clone()],
        PlanType::String,
        vec![operand],
        result.clone(),
        span,
    );
    Ok(result)
}

/// Format an exclusive String reference through a shared reborrow of its referent.
fn exclusive_text_display(builder: &mut Builder, operand: Operand, span: &SourceSpan) -> Result<Place, String> {
    let reference = match &operand.kind {
        OperandKind::Move(place) | OperandKind::Copy(place) => place.clone(),
        _ => return Err("unsupported Body IR exclusive text display without a place".into()),
    };
    let referent = Place {
        local: reference.local,
        projection: Projection::Deref(PlanType::String),
        span: span.clone(),
    };
    let shared = temporary(builder, PlanType::StringRef, span);
    builder.emit(plan_assign(shared.clone(), RvalueKind::Borrow(referent), span));
    Ok(owned_borrowed_text(builder, read_place(shared, false, span), span))
}

/// Invoke legacy Debug on an admitted carrier after its read ownership has been materialized.
fn debug_plan_operand(builder: &mut Builder, operand: Operand, span: &SourceSpan) -> Result<Place, String> {
    native_format_text(builder, operand, true, span)
}

/// Format an exact owned or borrowed carrier with native Debug or Display, without a runtime cycle.
pub fn native_format_text(
    builder: &mut Builder,
    operand: Operand,
    debug: bool,
    span: &SourceSpan,
) -> Result<Place, String> {
    let ty = value_type(builder, &operand)?;
    let result = temporary(builder, PlanType::String, span);
    terminate_call(
        builder,
        CalleeKind::FormatValue(ty, debug),
        vec![operand],
        result.clone(),
        span,
    );
    builder.owned.push(result.local);
    Ok(result)
}

/// Materialize checked text literals, retain moved strings and clone copied strings for display.
fn display_string_operand(builder: &mut Builder, operand: Operand, span: &SourceSpan) -> Result<Place, String> {
    match operand.kind {
        OperandKind::Literal(Constant::Text(text)) => Ok(owned_literal(builder, &text, span)),
        OperandKind::Move(place) => Ok(place),
        OperandKind::Copy(place) => clone_text(builder, place, span),
        _ => Err("unsupported Body IR string display ownership".into()),
    }
}

/// Materialize borrowed text for display; native str slices, copyable decimal and frozen carriers keep their checked
/// representation.
///
/// A scalar a type parameter's checked clone or borrow fact reads, once its instance or closed binding makes it
/// concrete, displays from a copy, in the type-parameter display style when the value's checked type is one.
fn clone_display_place(
    builder: &mut Builder,
    value: &SourcePlace,
    span: &SourceSpan,
    type_parameter: bool,
) -> Result<Place, String> {
    let place = scalar_place(builder, value, span)?;
    let ty = place_type(&builder.locals, &place);
    if matches!(
        ty,
        PlanType::Decimal
            | PlanType::FrozenStr
            | PlanType::StringRef
            | PlanType::StrRef
            | PlanType::Bool
            | PlanType::Unit
    ) || numeric_carrier(&ty)
    {
        return display_plan_value(builder, read_place(place, false, span), span, type_parameter);
    }
    if ty != PlanType::String {
        let site = format!("{}:{}:{} in {}", builder.file, span.line, span.column, builder.name);
        return Err(format!(
            "unsupported Body IR nonscalar display borrow at {site}; type {ty:?}; projection {:?}",
            place.projection
        ));
    }
    clone_text(builder, place, span)
}

/// Evaluate a replacement before dropping its old value, preserving retained text ownership.
pub fn lower_string_assignment(
    builder: &mut Builder,
    destination: Place,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    if destination.projection != Projection::Whole {
        return lower_string_field_assignment(builder, destination, value, span);
    }
    let text = match value {
        Rvalue::Format(parts) => {
            let text = temporary(builder, PlanType::String, span);
            lower_format(builder, text.clone(), parts, span)?;
            text
        }
        Rvalue::Use(operand) => display_operand(builder, operand, span)?,
        _ => return Err("unsupported Body IR string Rvalue outside formatting".into()),
    };
    forget_owned(builder, text.local);
    drop_owned(builder, destination.local, span);
    builder.emit(plan_assign(
        destination.clone(),
        RvalueKind::Use(read_place(text, true, span)),
        span,
    ));
    builder.owned.push(destination.local);
    Ok(())
}

/// Borrow a source enum's checked Debug implementation; unions and text Options retain their existing rendering.
fn debug_enum_operand(builder: &mut Builder, place: Place, owner: i64, span: &SourceSpan) -> Result<Place, String> {
    if let Some(declaration) = usize::try_from(owner).ok().and_then(|slot| builder.enums().get(slot))
        && declaration.derives.iter().any(|derive| derive == "Debug")
    {
        let name = declaration.name.clone();
        return borrowed_enum_debug(builder, place, owner, &name, span);
    }
    if is_union_carrier(builder, owner) {
        let copied = clone_nominal_operand(builder, place, span)?;
        return debug_plan_operand(builder, copied, span);
    }
    if is_carrier_index(owner)
        && let Some(declaration) = usize::try_from(carrier_position(owner))
            .ok()
            .and_then(|slot| builder.intrinsics.get(slot))
        && matches!(declaration.carrier, Carrier::Option | Carrier::Result)
    {
        let name = declaration.name.clone();
        return borrowed_enum_debug(builder, place, owner, &name, span);
    }
    Err("unsupported Body IR nonmodel FormatStyle.Debug".into())
}
