//! Checked generated field reflection over retained concrete storage, with legacy's typed and string contracts.

use incan_mir_plan_native::{
    CalleeKind, Carrier, FieldProjection, Operand, Place, PlanType, RvalueKind, SourceSpan, TerminatorKind,
    assign as plan_assign, tuple_element_type,
};
use incan_semantics_core::SemanticSourceTargetKind;
use incan_semantics_core::body_ir::{
    ArgumentElement, MethodTarget, NamedCallableTarget, Place as SourcePlace, ReflectionOperation,
};

use crate::builtins::list_call;
use crate::checked_values::scalar_place;
use crate::formatting::display_plan_value;
use crate::frame::{
    Builder, appended_field, drop_owned, external_call, place_type, read_place, temporary, terminate_call,
    track_rvalue_moves, value_type,
};
use crate::layout_types::native_source_type;
use crate::list_values::append_list_value;
use crate::lists::{list_element, list_shape};
use crate::model_values::nominal_place;
use crate::option_tests::is_union_carrier;
use crate::options::{
    assign_carrier, carrier_layout, carrier_payload_value, replace_owned_destination, variant_condition,
};
use crate::reflection_carriers::is_field_metadata_carrier;
use crate::sequence_builtins::text_argument;
use crate::text_plan::owned_literal;
use crate::unions::union_slot;

/// Admit only a retained generated operation and an eligible checked nominal field policy.
pub fn lower_reflection(
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<bool, String> {
    if lower_field_metadata(builder, target, arguments, destination, span)? {
        return Ok(true);
    }
    let (string, items) = match &target.operation {
        None | Some(ReflectionOperation::Builtin(_)) => return Ok(false),
        Some(ReflectionOperation::Metadata) => {
            return Err("unsupported Body IR field metadata without a retained declaration".into());
        }
        Some(ReflectionOperation::GenericMetadata(_)) => {
            return Err("unsupported Body IR generic field metadata without a retained declaration".into());
        }
        Some(ReflectionOperation::TypedValue) => (false, false),
        Some(ReflectionOperation::TypedItems) => (false, true),
        Some(ReflectionOperation::StringValue) => (true, false),
        Some(ReflectionOperation::StringItems) => (true, true),
    };
    if arguments.len() != 1 + usize::from(!items) || !target.type_args.is_empty() {
        return Err("unsupported Body IR reflection argument contract".into());
    }
    if target.canonical.is_none() {
        return Err("unsupported Body IR reflection without generated identity".into());
    }
    let Some(value) = arguments[0].as_one() else {
        return Err("unsupported Body IR reflection receiver spread".into());
    };
    let Some(receiver) = nominal_place(builder, value, span)? else {
        return Err("unsupported Body IR reflection receiver storage".into());
    };
    let PlanType::Model(owner, _) = place_type(&builder.locals, &receiver) else {
        return Err("unsupported Body IR reflection receiver carrier".into());
    };
    let declaration = &builder.nominals()[owner as usize];
    let Some(slots) = declaration.reflection_fields.clone() else {
        return Err("unsupported Body IR reflection field policy".into());
    };
    if !declaration.type_parameters.is_empty() {
        return Err("unsupported Body IR reflection over generic fields".into());
    }
    // Indexed reads copy Rust's primitive slot; a borrowed loop item cannot be cast by legacy.
    let fields: Vec<i64> = slots.iter().map(|slot| *slot as i64).collect();
    let reflected = Reflected {
        owner,
        receiver,
        fields,
        string,
    };
    validate_reflection_fields(builder, &reflected, target, span)?;
    let Some(place) = destination else {
        return Err("unsupported Body IR discarded reflection result".into());
    };
    let output = scalar_place(builder, place, span)?;
    if items {
        reflect_items(builder, &reflected, output, span)?;
    } else {
        reflect_value(builder, &reflected, arguments, output, span)?;
    }
    Ok(true)
}

/// The reflected model, its receiver storage, its reflected field slots and whether values are stringified.
struct Reflected {
    owner: i64,
    receiver: Place,
    fields: Vec<i64>,
    string: bool,
}

/// Use checked generated ownership before substitution and the concrete declaration's retained public metadata.
pub fn lower_field_metadata(
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<bool, String> {
    let generic = match &target.operation {
        Some(ReflectionOperation::Metadata) => false,
        Some(ReflectionOperation::GenericMetadata(_)) => {
            if !target.has_valid_generic_metadata() {
                return Err("unsupported Body IR generic field metadata identity".into());
            }
            true
        }
        _ => return Ok(false),
    };
    if arguments.len() != 1 || !target.type_args.is_empty() {
        return Err("unsupported Body IR field metadata arguments".into());
    }
    let Some(value) = arguments[0].as_one() else {
        return Err("unsupported Body IR field metadata receiver spread".into());
    };
    let Some(receiver) = nominal_place(builder, value, span)? else {
        return Err("unsupported Body IR field metadata receiver".into());
    };
    let PlanType::Model(owner, _) = place_type(&builder.locals, &receiver) else {
        return Err("unsupported Body IR field metadata receiver carrier".into());
    };
    let Some(identity) = &target.canonical else {
        return Err("unsupported Body IR field metadata identity".into());
    };
    let declaration = &builder.nominals()[owner as usize];
    if generic {
        // The generated-operation fact carries the checked binder authority; the substituted receiver must retain its
        // own reflection capability and metadata.
        if !declaration.derives.iter().any(|derive| derive == "FieldInfo") {
            return Err("unsupported Body IR generic field metadata capability".into());
        }
    } else if identity.origin != declaration.canonical.origin
        || identity.declaration_span != declaration.canonical.declaration_span
        || identity.scope_discriminant != declaration.canonical.scope_discriminant
        || identity.declaration_name != "__fields__"
    {
        return Err("unsupported Body IR field metadata owner mismatch".into());
    }
    emit_field_metadata(builder, owner, destination, span)
}

/// Use the checked static owner and generated member identity; authored methods have their own independent spans.
pub fn lower_static_field_metadata(
    builder: &mut Builder,
    target: &NamedCallableTarget,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<bool, String> {
    let Some(identity) = &target.canonical else {
        return Ok(false);
    };
    if identity.kind != SemanticSourceTargetKind::Method || identity.declaration_name != "__fields__" {
        return Ok(false);
    }
    let Some(receiver) = &target.receiver_type else {
        return Ok(false);
    };
    let ty = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        receiver,
        &builder.intrinsics,
        "",
    )?;
    let PlanType::Model(owner, _) = ty else {
        return Err("unsupported Body IR static field metadata owner".into());
    };
    let declaration = &builder.nominals()[owner as usize].canonical;
    if identity.origin != declaration.origin
        || identity.declaration_span != declaration.declaration_span
        || identity.scope_discriminant != declaration.scope_discriminant
    {
        return Ok(false);
    }
    if !arguments.is_empty() || !target.type_args.is_empty() {
        return Err("unsupported Body IR static field metadata arguments".into());
    }
    emit_field_metadata(builder, owner, destination, span)
}

/// Share immutable declaration records using the checked Incan primitive carrier.
pub fn emit_field_metadata(
    builder: &mut Builder,
    owner: i64,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<bool, String> {
    if builder.nominals()[owner as usize].reflection_metadata.is_none() {
        return Err("unsupported Body IR field metadata declaration".into());
    }
    let Some(info) = builder.nominals().iter().position(is_field_metadata_carrier) else {
        return Err("the standard library field metadata layout is unsupported".into());
    };
    let name = builder.nominals()[info].name.to_string();
    let ty = PlanType::FrozenCollection("FrozenList".into(), vec![PlanType::Model(info as i64, name)]);
    let mut output = temporary(builder, ty.clone(), span);
    if let Some(place) = destination {
        output = scalar_place(builder, place, span)?;
        if place_type(&builder.locals, &output) != ty {
            return Err("unsupported Body IR field metadata output carrier".into());
        }
    }
    terminate_call(
        builder,
        CalleeKind::FieldMetadata(owner, info as i64),
        Vec::new(),
        output,
        span,
    );
    Ok(true)
}

/// Project only an in-range retained source field using its canonical declared type.
fn reflected_field(
    builder: &Builder,
    owner: i64,
    source: &Place,
    slot: i64,
    span: &SourceSpan,
) -> Result<Place, String> {
    let declaration = &builder.nominals()[owner as usize];
    if slot < 0 || slot as usize >= declaration.fields.len() {
        return Err("unsupported Body IR reflection field slot".into());
    }
    let ty = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &declaration.field_types[slot as usize],
        &builder.intrinsics,
        "",
    )?;
    appended_field(source, FieldProjection::new(slot, ty), span)
}

/// Admit supported scalar Display carriers; other scalar variants retain a named frontier.
fn scalar_reflection_type(ty: &PlanType) -> bool {
    matches!(ty, PlanType::String | PlanType::Int | PlanType::Float | PlanType::Bool)
}

/// Prove generated ownership and the whole generic capability before lowering a single lookup.
fn validate_reflection_fields(
    builder: &Builder,
    reflected: &Reflected,
    target: &MethodTarget,
    span: &SourceSpan,
) -> Result<(), String> {
    let declaration = &builder.nominals()[reflected.owner as usize];
    if !reflected.string {
        let Some(identity) = &target.canonical else {
            return Err("unsupported Body IR reflection generated owner".into());
        };
        if identity.origin != declaration.canonical.origin
            || identity.declaration_span != declaration.canonical.declaration_span
            || identity.scope_discriminant != declaration.canonical.scope_discriminant
        {
            return Err("unsupported Body IR reflection generated owner mismatch".into());
        }
        return Ok(());
    }
    if !declaration.derives.iter().any(|derive| derive == "FieldInfo") {
        return Err("unsupported Body IR generic reflection without field metadata".into());
    }
    for slot in &reflected.fields {
        let field = reflected_field(builder, reflected.owner, &reflected.receiver, *slot, span)?;
        let ty = place_type(&builder.locals, &field);
        if scalar_reflection_type(&ty) {
            continue;
        }
        let (_, layout) = carrier_layout(builder, &ty)?;
        if layout.carrier != Carrier::Option
            || layout.variants[1].fields.len() != 1
            || !scalar_reflection_type(&layout.variants[1].fields[0])
        {
            return Err("unsupported Body IR generic reflection field capability".into());
        }
    }
    Ok(())
}

/// Use Rust Display for an eligible scalar, cloning owned text before transferring it to the formatter.
fn scalar_string(builder: &mut Builder, field: Place, span: &SourceSpan) -> Result<Operand, String> {
    let ty = place_type(&builder.locals, &field);
    if !scalar_reflection_type(&ty) {
        return Err("unsupported Body IR generic field reflection scalar".into());
    }
    let value = carrier_payload_value(builder, field, span)?;
    let output = display_plan_value(builder, value, span, true)?;
    Ok(read_place(output, true, span))
}

/// Stringify optional scalar fields as their contained Display value or literal None, never Option Debug.
fn string_field(builder: &mut Builder, field: Place, span: &SourceSpan) -> Result<Operand, String> {
    let ty = place_type(&builder.locals, &field);
    if !matches!(ty, PlanType::Enum(..)) {
        return scalar_string(builder, field, span);
    }
    let (_, layout) = carrier_layout(builder, &ty)?;
    if layout.carrier != Carrier::Option || layout.variants[1].fields.len() != 1 {
        return Err("unsupported Body IR generic field reflection optional carrier".into());
    }
    let output = temporary(builder, PlanType::String, span);
    let selected = builder.new_block(span);
    let missing = builder.new_block(span);
    let join = builder.new_block(span);
    let owners = builder.owned.clone();
    let condition = variant_condition(builder, &field, 1, span);
    builder.finish(TerminatorKind::SwitchBool(condition, missing, selected), span);
    builder.select(selected);
    let mut projection = FieldProjection::new(0, layout.variants[1].fields[0].clone());
    projection.variant = 1;
    let payload = appended_field(&field, projection, span)?;
    let value = scalar_string(builder, payload, span)?;
    replace_owned_destination(builder, output.clone(), RvalueKind::Use(value), span);
    builder.finish(TerminatorKind::Goto(join), span);
    builder.select(missing);
    builder.owned = owners;
    let text = owned_literal(builder, "None", span);
    replace_owned_destination(
        builder,
        output.clone(),
        RvalueKind::Use(read_place(text, true, span)),
        span,
    );
    let retained = builder.owned.clone();
    builder.finish(TerminatorKind::Goto(join), span);
    builder.select(join);
    builder.owned = retained;
    Ok(read_place(output, true, span))
}

/// Clone a typed field and wrap its exact union member, or stringify under the checked generic contract.
fn field_value(
    builder: &mut Builder,
    field: Place,
    string: bool,
    expected: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if string {
        if *expected != PlanType::String {
            return Err("unsupported Body IR reflection string result contract".into());
        }
        return string_field(builder, field, span);
    }
    let value = carrier_payload_value(builder, field, span)?;
    let member = value_type(builder, &value)?;
    if member == *expected {
        return Ok(value);
    }
    let PlanType::Enum(owner, _) = expected else {
        return Err("unsupported Body IR reflection field result contract".into());
    };
    if !is_union_carrier(builder, *owner) {
        return Err("unsupported Body IR reflection typed overlay".into());
    }
    let slot = union_slot(&builder.unions, *owner, &member, &builder.intrinsics)?;
    let output = temporary(builder, expected.clone(), span);
    replace_owned_destination(
        builder,
        output.clone(),
        RvalueKind::Enum(*owner, slot, vec![value]),
        span,
    );
    Ok(read_place(output, true, span))
}

/// Build owned name/value tuples in retained source order, transferring tuple owners into the list.
fn reflect_items(builder: &mut Builder, reflected: &Reflected, output: Place, span: &SourceSpan) -> Result<(), String> {
    let ty = place_type(&builder.locals, &output);
    let Some((leaf, depth)) = list_shape(&ty) else {
        return Err("unsupported Body IR reflection items list".into());
    };
    let element = list_element(&leaf, depth);
    let PlanType::Tuple(elements) = &element else {
        return Err("unsupported Body IR reflection items element".into());
    };
    if elements.len() != 2 {
        return Err("unsupported Body IR reflection items tuple".into());
    }
    let expected = tuple_element_type(&elements[1]);
    list_call(
        builder,
        "empty_list",
        element.clone(),
        Vec::new(),
        ty,
        Vec::new(),
        output.clone(),
        span,
    );
    for slot in &reflected.fields {
        let field = reflected_field(builder, reflected.owner, &reflected.receiver, *slot, span)?;
        let value = field_value(builder, field, reflected.string, &expected, span)?;
        let field_name = builder.nominals()[reflected.owner as usize].fields[*slot as usize].to_string();
        let name = owned_literal(builder, &field_name, span);
        let pair = temporary(builder, element.clone(), span);
        let operation = RvalueKind::Tuple(vec![read_place(name, true, span), value]);
        track_rvalue_moves(builder, &operation);
        builder.emit(plan_assign(pair.clone(), operation, span));
        builder.owned.push(pair.local);
        append_list_value(builder, &output, read_place(pair, true, span), &leaf, depth, span)?;
    }
    Ok(())
}

/// Compare names and aliases once, clone only a matched field, and construct checked Some or None results.
fn reflect_value(
    builder: &mut Builder,
    reflected: &Reflected,
    arguments: &[ArgumentElement],
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let (index, layout) = carrier_layout(builder, &place_type(&builder.locals, &output))?;
    if layout.carrier != Carrier::Option || layout.variants[1].fields.len() != 1 {
        return Err("unsupported Body IR reflection lookup result".into());
    }
    let expected = layout.variants[1].fields[0].clone();
    let key = text_argument(builder, arguments, 1, span)?;
    let join = builder.new_block(span);
    let owners = builder.owned.clone();
    for slot in &reflected.fields {
        for name in field_keys(builder, reflected.owner, *slot) {
            let yes = builder.new_block(span);
            let no = builder.new_block(span);
            let condition = temporary(builder, PlanType::Bool, span);
            let text = owned_literal(builder, &name, span);
            let reference = temporary(builder, PlanType::StringRef, span);
            builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(text.clone()), span));
            external_call(
                builder,
                "incan_native_runtime::equal_text",
                vec![PlanType::StringRef, PlanType::StringRef],
                PlanType::Bool,
                vec![key.clone(), read_place(reference, false, span)],
                condition.clone(),
                span,
            );
            drop_owned(builder, text.local, span);
            builder.finish(
                TerminatorKind::SwitchBool(read_place(condition, false, span), no, yes),
                span,
            );
            builder.select(yes);
            let field = reflected_field(builder, reflected.owner, &reflected.receiver, *slot, span)?;
            let value = field_value(builder, field, reflected.string, &expected, span)?;
            assign_carrier(builder, output.clone(), index, 1, vec![value], span);
            builder.finish(TerminatorKind::Goto(join), span);
            builder.select(no);
            builder.owned = owners.clone();
        }
    }
    assign_carrier(builder, output, index, 0, Vec::new(), span);
    let retained = builder.owned.clone();
    builder.finish(TerminatorKind::Goto(join), span);
    builder.select(join);
    builder.owned = retained;
    Ok(())
}

/// A field's lookup names: its declared name, then its alias when it has a distinct one.
fn field_keys(builder: &Builder, owner: i64, slot: i64) -> Vec<String> {
    let declaration = &builder.nominals()[owner as usize];
    let mut keys = vec![declaration.fields[slot as usize].to_string()];
    if declaration.field_aliases.len() == declaration.fields.len()
        && let Some(alias) = &declaration.field_aliases[slot as usize]
        && *alias != keys[0]
    {
        keys.push(alias.to_string());
    }
    keys
}
