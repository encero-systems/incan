//! Lower value-enum members: ordinal encodings, raw values, construction from values, and enum messages.

use incan_mir_plan_native::{
    BinaryOp, Carrier, Constant, Operand, Place, PlanType, RvalueKind, SourceSpan, TerminatorKind,
    assign as plan_assign,
};
use incan_semantics_core::SymbolOrigin;
use incan_semantics_core::body_ir::{
    ArgumentElement, BodyIrModule, Constant as SourceConstant, NamedCallableTarget, Operand as SourceOperand,
    Place as SourcePlace, Rvalue, ValueEnumBacking,
};

use crate::callable_calls::{checked_type_arguments, has_adapter_slot};
use crate::checked_values::scalar_place;
use crate::decls::library_enum_item;
use crate::formatting::lower_string_assignment;
use crate::frame::{
    Builder, drop_owned, external_call, literal, place_type, read_place, temporary, text_literal, track_rvalue_moves,
    value_type,
};
use crate::layout_types::native_source_type;
use crate::list_methods::{argument_place, argument_value, call_destination};
use crate::list_values::value_operand;
use crate::options::{assign_carrier, carrier_layout};
use crate::strings::emit_text_helper;
use crate::text_plan::owned_literal;
use crate::value_enums::validate_layout;

/// Keep the checked receiver's dereference and field projections before reading its native enum discriminant.
fn value_enum_receiver_place(
    builder: &mut Builder,
    argument: &ArgumentElement,
    span: &SourceSpan,
) -> Result<Place, String> {
    match argument.as_one() {
        Some(SourceOperand::Place(read)) => scalar_place(builder, &read.place, span),
        _ => Err("unsupported Body IR nonplace value enum receiver".into()),
    }
}

/// Select the retained raw value by native tag, preserving repeated reads and text ownership.
pub fn lower_value_enum_method(
    builder: &mut Builder,
    owner: usize,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 1 {
        return Err("unsupported Body IR value enum getter arguments".into());
    }
    let source = value_enum_receiver_place(builder, &arguments[0], span)?;
    lower_value_enum_getter_from(builder, owner, source, destination, span)
}

/// Write the raw value of the value enum `source` holds into the call's destination.
pub fn lower_value_enum_getter_from(
    builder: &mut Builder,
    owner: usize,
    source: Place,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let result_type = value_enum_raw_type(builder, owner);
    let output = call_destination(builder, destination, result_type.clone(), span)?;
    if place_type(&builder.locals, &output) != result_type || builder.owned.contains(&output.local) {
        return Err("unsupported Body IR value enum getter destination".into());
    }
    select_value_enum_raw(builder, owner, source, output, span)
}

/// Locate a source value enum by its retained layout identity, including a borrowed receiver.
///
/// 1. Read the enum's checked layout slot.
/// 2. Match its exact retained source identity.
pub fn ordinal_value_owner(builder: &Builder, ty: &PlanType) -> Option<usize> {
    // 1. Read the enum's checked layout slot.
    let index = match ty {
        PlanType::Enum(owner, _) | PlanType::EnumRef(owner, _) => *owner,
        _ => return None,
    };
    if index < 0 || index as usize >= builder.enums().len() {
        return None;
    }
    // 2. Match its exact retained source identity.
    let layout = &builder.enums()[index as usize];
    builder
        .value_enums()
        .iter()
        .position(|declaration| layout.canonical == declaration.canonical && library_enum_item(layout).is_empty())
}

/// Retain the declaring module and ordered raw-value schema for an authenticated source OrdinalKey slot.
pub fn lower_value_enum_ordinal_encoding(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &NamedCallableTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<bool, String> {
    let Some(identity) = &target.canonical else {
        return Ok(false);
    };
    if identity.declaration_name != "ordinal_encoding" || !has_adapter_slot(module, identity) {
        return Ok(false);
    }
    let Some(receiver) = &target.receiver_type else {
        return Ok(false);
    };
    let closed = checked_type_arguments(builder, std::slice::from_ref(receiver))?;
    let ty = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &closed[0],
        &builder.intrinsics,
        "",
    )?;
    let Some(owner) = ordinal_value_owner(builder, &ty) else {
        return Ok(false);
    };
    let owner_identity = &builder.value_enums()[owner].canonical;
    let overridden = module.trait_implementations.iter().any(|implementation| {
        implementation.owner == *owner_identity
            && Some(&implementation.method) == target.canonical.as_ref()
            && implementation.implementation != implementation.method
    });
    if overridden {
        return Ok(false);
    }
    if !arguments.is_empty() {
        return Err("unsupported Body IR value enum ordinal encoding arguments".into());
    }
    let schema = value_enum_ordinal_schema(builder, owner)?;
    let output = call_destination(builder, destination, PlanType::String, span)?;
    let value = Rvalue::Use(SourceOperand::Constant(SourceConstant::Str(schema)));
    lower_string_assignment(builder, output, &value, span)?;
    Ok(true)
}

/// Encode the canonical declaring module and each raw value in source order, with UTF-8 byte lengths for text.
fn value_enum_ordinal_schema(builder: &Builder, owner: usize) -> Result<String, String> {
    let declaration = &builder.value_enums()[owner];
    let SymbolOrigin::Module(path) = &declaration.canonical.origin else {
        return Err("unsupported Body IR source value enum ordinal module identity".into());
    };
    let backing = if declaration.backing == ValueEnumBacking::Str {
        "str"
    } else {
        "int"
    };
    let mut segments: Vec<String> = path.iter().map(|segment| segment.to_string()).collect();
    segments.push(declaration.name.to_string());
    let mut schema = format!("value-enum:{backing}:{}:", segments.join("."));
    for variant in &declaration.variants {
        match &variant.raw_value {
            SourceConstant::Str(value) => schema += &format!("{}:{value};", value.len()),
            SourceConstant::Int(value) => schema += &format!("{value};"),
            _ => return Err("unsupported Body IR value enum ordinal raw carrier".into()),
        }
    }
    Ok(schema + ":v1")
}

/// Return the checked scalar backing of a value enum.
///
/// 1. Read its retained text or integer backing.
pub fn value_enum_raw_type(builder: &Builder, owner: usize) -> PlanType {
    // 1. Read its retained scalar backing.
    match builder.value_enums()[owner].backing {
        ValueEnumBacking::Str => PlanType::String,
        ValueEnumBacking::Int => PlanType::Int,
    }
}

/// Write the raw value of the variant source holds into output, selected by its native tag.
///
/// 1. Allocate a branch per checked variant.
/// 2. Materialize that variant's retained scalar value.
/// 3. Join the branches and retain owned text for cleanup.
pub fn select_value_enum_raw(
    builder: &mut Builder,
    owner: usize,
    source: Place,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    // 1. Allocate a branch per checked variant.
    let raw_values: Vec<SourceConstant> = builder.value_enums()[owner]
        .variants
        .iter()
        .map(|variant| variant.raw_value.clone())
        .collect();
    let result_type = value_enum_raw_type(builder, owner);
    let join = builder.new_block(span);
    let owners = builder.owned.clone();
    let branches = tag_branches(builder, source, raw_values.len(), span);
    // 2. Materialize that variant's retained scalar value.
    for (branch, raw_value) in branches.into_iter().zip(raw_values) {
        builder.select(branch);
        let operand = SourceOperand::Constant(raw_value);
        if result_type == PlanType::String {
            lower_string_assignment(builder, output.clone(), &Rvalue::Use(operand), span)?;
        } else {
            let value = value_operand(builder, &operand, span)?;
            builder.emit(plan_assign(output.clone(), RvalueKind::Use(value), span));
        }
        builder.finish(TerminatorKind::Goto(join), span);
        builder.owned = owners.clone();
    }
    // 3. Join the branches and retain owned text for cleanup.
    builder.select(join);
    if result_type == PlanType::String {
        builder.owned.push(output.local);
    }
    Ok(())
}

/// Branch on the native tag of the enum `source` holds, returning one block per variant in variant order.
///
/// Each returned block runs only for its variant; the caller fills it and ends it with a jump to its own join.
fn tag_branches(builder: &mut Builder, source: Place, count: usize, span: &SourceSpan) -> Vec<i64> {
    let raw_tag = temporary(builder, PlanType::EnumTag, span);
    let tag = temporary(builder, PlanType::Int, span);
    builder.emit(plan_assign(raw_tag.clone(), RvalueKind::Discriminant(source), span));
    builder.emit(plan_assign(
        tag.clone(),
        RvalueKind::TagToInt(read_place(raw_tag, false, span)),
        span,
    ));
    let mut branches = Vec::with_capacity(count);
    for slot in 0..count {
        if slot + 1 == count {
            branches.push(builder.current as i64);
            continue;
        }
        let selected = builder.new_block(span);
        let next = builder.new_block(span);
        let condition = temporary(builder, PlanType::Bool, span);
        let compared = RvalueKind::Binary(
            BinaryOp::Equal,
            read_place(tag.clone(), false, span),
            literal(Constant::Int(slot as i64), span),
        );
        builder.emit(plan_assign(condition.clone(), compared, span));
        builder.finish(
            TerminatorKind::SwitchBool(read_place(condition, false, span), next, selected),
            span,
        );
        branches.push(selected);
        builder.select(next);
    }
    branches
}

/// Select the variant whose raw value equals the argument, as the generated `from_value()` does: `Some` of the first
/// variant in declaration order that matches, or `None` when none does.
pub fn lower_value_enum_from_value(
    builder: &mut Builder,
    owner: usize,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 1 {
        return Err("unsupported Body IR value enum from_value arguments".into());
    }
    let layout = value_enum_layout(builder, owner)?;
    let raw_type = value_enum_raw_type(builder, owner);
    let value = argument_value(builder, arguments, 0, span)?;
    if value_type(builder, &value)? != raw_type {
        return Err("unsupported Body IR value enum from_value argument".into());
    }
    let raw = temporary(builder, raw_type.clone(), span);
    track_rvalue_moves(builder, &RvalueKind::Use(value.clone()));
    builder.emit(plan_assign(raw.clone(), RvalueKind::Use(value), span));
    if raw_type == PlanType::String {
        builder.owned.push(raw.local);
    }
    let output = call_destination(builder, destination, PlanType::Unit, span)?;
    let (carrier, declaration) = carrier_layout(builder, &place_type(&builder.locals, &output))?;
    let element = PlanType::Enum(layout as i64, builder.enums()[layout].name.to_string());
    if declaration.carrier != Carrier::Option
        || declaration.variants[1].fields != [element.clone()]
        || builder.owned.contains(&output.local)
    {
        return Err("unsupported Body IR value enum from_value destination".into());
    }
    let join = builder.new_block(span);
    let owners = builder.owned.clone();
    let raw_values: Vec<SourceConstant> = builder.value_enums()[owner]
        .variants
        .iter()
        .map(|variant| variant.raw_value.clone())
        .collect();
    for (slot, raw_value) in raw_values.into_iter().enumerate() {
        let condition = raw_value_matches(builder, &raw, raw_value, span)?;
        let matched = builder.new_block(span);
        let next = builder.new_block(span);
        builder.finish(TerminatorKind::SwitchBool(condition, next, matched), span);
        builder.select(matched);
        let variant = temporary(builder, element.clone(), span);
        builder.emit(plan_assign(
            variant.clone(),
            RvalueKind::Enum(layout as i64, slot as i64, Vec::new()),
            span,
        ));
        assign_carrier(
            builder,
            output.clone(),
            carrier,
            1,
            vec![read_place(variant, true, span)],
            span,
        );
        builder.finish(TerminatorKind::Goto(join), span);
        builder.owned = owners.clone();
        builder.select(next);
    }
    assign_carrier(builder, output.clone(), carrier, 0, Vec::new(), span);
    builder.finish(TerminatorKind::Goto(join), span);
    builder.owned = owners;
    builder.select(join);
    builder.owned.push(output.local);
    drop_owned(builder, raw.local, span);
    Ok(())
}

/// Find the retained source enum layout of value enum `owner`.
fn value_enum_layout(builder: &Builder, owner: usize) -> Result<usize, String> {
    let declaration = &builder.value_enums()[owner];
    for (index, layout) in builder.enums().iter().enumerate() {
        if layout.canonical == declaration.canonical {
            validate_layout(layout, declaration)?;
            return Ok(index);
        }
    }
    Err("unsupported Body IR unretained value enum layout".into())
}

/// Compare a value enum's raw argument with one variant's raw value: integers natively, text through the runtime.
fn raw_value_matches(
    builder: &mut Builder,
    raw: &Place,
    value: SourceConstant,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let condition = temporary(builder, PlanType::Bool, span);
    if place_type(&builder.locals, raw) == PlanType::Int {
        let operand = value_operand(builder, &SourceOperand::Constant(value), span)?;
        let compared = RvalueKind::Binary(BinaryOp::Equal, read_place(raw.clone(), false, span), operand);
        builder.emit(plan_assign(condition.clone(), compared, span));
        return Ok(read_place(condition, false, span));
    }
    let SourceConstant::Str(spelling) = value else {
        return Err("unsupported Body IR value enum raw value".into());
    };
    let text = owned_literal(builder, &spelling, span);
    let left = temporary(builder, PlanType::StringRef, span);
    builder.emit(plan_assign(left.clone(), RvalueKind::Borrow(raw.clone()), span));
    let right = temporary(builder, PlanType::StringRef, span);
    builder.emit(plan_assign(right.clone(), RvalueKind::Borrow(text.clone()), span));
    let operands = vec![read_place(left, false, span), read_place(right, false, span)];
    let parameters = vec![PlanType::StringRef, PlanType::StringRef];
    let result = emit_text_helper(builder, "equal_text", PlanType::Bool, parameters, operands, span);
    drop_owned(builder, text.local, span);
    Ok(read_place(result, false, span))
}

/// Spell the name of the variant a source enum holds, as the generated `message()` does.
pub fn lower_enum_message(
    builder: &mut Builder,
    index: usize,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 1 {
        return Err("unsupported Body IR enum message arguments".into());
    }
    let source = argument_place(builder, arguments, 0, span)?;
    let output = call_destination(builder, destination, PlanType::String, span)?;
    if place_type(&builder.locals, &output) != PlanType::String || builder.owned.contains(&output.local) {
        return Err("unsupported Body IR enum message destination".into());
    }
    let names: Vec<String> = builder.enums()[index]
        .variants
        .iter()
        .map(|variant| variant.name.to_string())
        .collect();
    let join = builder.new_block(span);
    let owners = builder.owned.clone();
    let branches = tag_branches(builder, source, names.len(), span);
    for (branch, name) in branches.into_iter().zip(names) {
        builder.select(branch);
        let texts = vec![text_literal(&name, span), text_literal("", span)];
        external_call(
            builder,
            "incan_std_primitives::strings::str_concat",
            vec![PlanType::StrRef, PlanType::StrRef],
            PlanType::String,
            texts,
            output.clone(),
            span,
        );
        builder.finish(TerminatorKind::Goto(join), span);
        builder.owned = owners.clone();
    }
    builder.select(join);
    builder.owned.push(output.local);
    Ok(())
}
