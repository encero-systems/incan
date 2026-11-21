//! Checked primitive, tuple, source-local model and carrier pattern tests for direct-route match dispatch.

use incan_lang::numeric_strings::parse_float_string;
use incan_mir_plan_native::{
    BinaryOp, Carrier, Constant, FieldProjection, Place, PlanType, Projection, RvalueKind, SourceSpan, TerminatorKind,
    assign as plan_assign, carrier_position,
};
use incan_semantics_core::body_ir::{Constant as SourceConstant, NominalPatternTarget, OwnershipFact, Pattern};

use crate::enums::pattern_variant;
use crate::frame::{Builder, appended_field, drop_owned, external_call, literal, place_type, read_place, temporary};
use crate::layout_types::{native_source_type, stored_field_type};
use crate::models::stored_steps;
use crate::numerics::numeric_constant;
use crate::options::{intrinsic_pattern, variant_condition};
use crate::text_plan::owned_literal;
use crate::tuples::{is_tuple_type, tuple_fields};
use crate::unions::union_pattern_slot;

/// Inspect a stored shared enum reference without moving its owner or losing a preceding payload projection.
pub fn enum_pattern_place(builder: &mut Builder, source: Place, span: &SourceSpan) -> Place {
    let ty = match place_type(&builder.locals, &source) {
        PlanType::EnumRef(owner, name) => PlanType::Enum(owner, name),
        PlanType::EnumMutRef(owner, name) => {
            if source.projection == Projection::Whole {
                let referent = PlanType::Enum(owner, name);
                return Place {
                    local: source.local,
                    projection: Projection::Deref(referent),
                    span: span.clone(),
                };
            }
            return source;
        }
        _ => return source,
    };
    let mut reference = source.clone();
    if source.projection != Projection::Whole {
        reference = temporary(builder, place_type(&builder.locals, &source), span);
        builder.emit(plan_assign(
            reference.clone(),
            RvalueKind::Use(read_place(source, false, span)),
            span,
        ));
    }
    Place {
        local: reference.local,
        projection: Projection::Deref(ty),
        span: span.clone(),
    }
}

/// Test retained literals and tuple components before any arm binding becomes initialized.
pub fn pattern_tests(
    builder: &mut Builder,
    source: &Place,
    pattern: &Pattern,
    selected: i64,
    rejected: i64,
    span: &SourceSpan,
) -> Result<(), String> {
    match pattern {
        Pattern::Wildcard | Pattern::Var(_) => builder.finish(TerminatorKind::Goto(selected), span),
        Pattern::Literal(value) => {
            // `None` names a carrier variant; every other literal compares a primitive value.
            if *value == SourceConstant::None {
                return carrier_tests(builder, source, pattern, selected, rejected, span);
            }
            let condition = literal_test(builder, source, value, span)?;
            builder.finish(
                TerminatorKind::SwitchBool(read_place(condition, false, span), rejected, selected),
                span,
            );
        }
        Pattern::Enum { .. }
        | Pattern::Result { .. }
        | Pattern::FieldlessEnumVariant(_)
        | Pattern::UnionMember { .. } => {
            return carrier_tests(builder, source, pattern, selected, rejected, span);
        }
        Pattern::Tuple(items) => {
            let ty = place_type(&builder.locals, source);
            if !is_tuple_type(&ty) {
                return Err("invalid Body IR tuple pattern carrier".into());
            }
            let fields = tuple_fields(source, &ty, span)?;
            if fields.len() != items.len() {
                return Err("invalid Body IR tuple pattern layout".into());
            }
            for (field, item) in fields.iter().zip(items) {
                let next_component = builder.new_block(span);
                pattern_tests(builder, field, item, next_component, rejected, span)?;
                builder.select(next_component);
            }
            builder.finish(TerminatorKind::Goto(selected), span);
        }
        Pattern::Nominal { target, fields } => {
            // Named fields are tested in written order, each against its own field of the borrowed scrutinee.
            for (name, field_pattern) in fields {
                let next_field = builder.new_block(span);
                let field = nominal_field(builder, source, target, name, span)?;
                pattern_tests(builder, &field, field_pattern, next_field, rejected, span)?;
                builder.select(next_field);
            }
            builder.finish(TerminatorKind::Goto(selected), span);
        }
        Pattern::Or(alternatives) => {
            // An alternation that binds nothing selects on its first matching alternative; each failure tries the
            // next.
            if pattern_binds(pattern) {
                return Err("unsupported Body IR binding alternation".into());
            }
            for alternative in alternatives {
                let next_alternative = builder.new_block(span);
                pattern_tests(builder, source, alternative, selected, next_alternative, span)?;
                builder.select(next_alternative);
            }
            builder.finish(TerminatorKind::Goto(rejected), span);
        }
        _ => return Err("unsupported Body IR structural match pattern".into()),
    }
    Ok(())
}

/// Select the variant a source enum, union, `Option` or `Result` pattern names; a wildcard names none (-1).
pub fn carrier_pattern_slot(builder: &Builder, owner: i64, pattern: &Pattern) -> Result<i64, String> {
    if owner < builder.enums().len() as i64 {
        return pattern_variant(&builder.enums()[owner as usize], pattern);
    }
    let declaration = &builder.intrinsics[carrier_position(owner) as usize];
    if declaration.carrier == Carrier::Declared {
        return union_pattern_slot(builder, owner, pattern);
    }
    intrinsic_pattern(declaration, pattern)
}

/// The declared payload types of one variant of a retained source enum or standard carrier.
pub fn carrier_payload_types(builder: &Builder, owner: i64, slot: i64) -> Result<Vec<PlanType>, String> {
    if owner < builder.enums().len() as i64 {
        let mut types = Vec::new();
        for ty in &builder.enums()[owner as usize].variants[slot as usize].fields {
            types.push(native_source_type(
                builder.constructors(),
                builder.nominals(),
                builder.enums(),
                &builder.unions,
                ty,
                &builder.intrinsics,
                "",
            )?);
        }
        return Ok(types);
    }
    Ok(
        builder.intrinsics[carrier_position(owner) as usize].variants[slot as usize]
            .fields
            .clone(),
    )
}

/// Report whether a carrier pattern tests more than its variant: a literal or a nested pattern in a payload, or an
/// alternation.
pub fn has_refutable_payload(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Or(_) => true,
        Pattern::Enum { fields, .. } | Pattern::Result { fields, .. } | Pattern::UnionMember { fields, .. } => fields
            .iter()
            .any(|field| !matches!(field, Pattern::Wildcard | Pattern::Var(_))),
        _ => false,
    }
}

/// Test a carrier pattern's variant, then each payload pattern against its projected payload, in order.
fn carrier_tests(
    builder: &mut Builder,
    source: &Place,
    pattern: &Pattern,
    selected: i64,
    rejected: i64,
    span: &SourceSpan,
) -> Result<(), String> {
    let carrier = enum_pattern_place(builder, source.clone(), span);
    let owner = match place_type(&builder.locals, &carrier) {
        PlanType::Enum(index, _) => index,
        other => return Err(format!("invalid Body IR carrier pattern scrutinee {other:?}")),
    };
    let slot = carrier_pattern_slot(builder, owner, pattern)?;
    if slot < 0 {
        builder.finish(TerminatorKind::Goto(selected), span);
        return Ok(());
    }
    let matched = builder.new_block(span);
    let condition = variant_condition(builder, &carrier, slot, span);
    builder.finish(TerminatorKind::SwitchBool(condition, rejected, matched), span);
    builder.select(matched);
    match pattern {
        Pattern::Enum { fields, .. } | Pattern::Result { fields, .. } | Pattern::UnionMember { fields, .. } => {
            payload_tests(builder, &carrier, owner, slot, fields, selected, rejected, span)
        }
        _ => {
            builder.finish(TerminatorKind::Goto(selected), span);
            Ok(())
        }
    }
}

/// Test each payload pattern of a selected variant against its payload, rejecting at the first mismatch.
#[allow(clippy::too_many_arguments)]
fn payload_tests(
    builder: &mut Builder,
    source: &Place,
    owner: i64,
    slot: i64,
    fields: &[Pattern],
    selected: i64,
    rejected: i64,
    span: &SourceSpan,
) -> Result<(), String> {
    let types = carrier_payload_types(builder, owner, slot)?;
    if types.len() != fields.len() {
        return Err("invalid Body IR carrier pattern payload layout".into());
    }
    for (index, (field, ty)) in fields.iter().zip(types).enumerate() {
        let mut projection = FieldProjection::new(index as i64, ty);
        projection.variant = slot;
        let payload = appended_field(source, projection, span)?;
        let next_payload = builder.new_block(span);
        pattern_tests(builder, &payload, field, next_payload, rejected, span)?;
        builder.select(next_payload);
    }
    builder.finish(TerminatorKind::Goto(selected), span);
    Ok(())
}

/// Report whether any position of a pattern binds a name.
pub fn pattern_binds(pattern: &Pattern) -> bool {
    !bound_in_place(pattern).is_empty()
}

/// Report whether a pattern binds a name its arm changes in place, as an exclusive borrow of the scrutinee.
pub fn binds_in_place(pattern: &Pattern) -> bool {
    bound_in_place(pattern).contains(&true)
}

/// For every name a pattern binds, in source order, whether the name borrows its part of the scrutinee exclusively.
fn bound_in_place(pattern: &Pattern) -> Vec<bool> {
    let mut bound = Vec::new();
    match pattern {
        Pattern::Var(binding) => bound.push(binding.fact == OwnershipFact::MutBorrow),
        Pattern::Tuple(items) | Pattern::Or(items) => {
            for item in items {
                bound.extend(bound_in_place(item));
            }
        }
        Pattern::Enum { fields, .. } | Pattern::Result { fields, .. } | Pattern::UnionMember { fields, .. } => {
            for field in fields {
                bound.extend(bound_in_place(field));
            }
        }
        Pattern::Nominal { fields, .. } | Pattern::Struct { fields, .. } => {
            for (_, field_pattern) in fields {
                bound.extend(bound_in_place(field_pattern));
            }
        }
        _ => {}
    }
    bound
}

/// Project one canonical field of a model scrutinee once the pattern proves it names the scrutinee's declaration.
pub fn nominal_field(
    builder: &Builder,
    source: &Place,
    target: &NominalPatternTarget,
    name: &str,
    span: &SourceSpan,
) -> Result<Place, String> {
    let PlanType::Model(owner, _) = place_type(&builder.locals, source) else {
        return Err("invalid Body IR nominal pattern carrier".into());
    };
    let declaration = &builder.nominals()[owner as usize];
    if target.direct_declaration_id != declaration.direct_declaration_id || target.canonical != declaration.canonical {
        return Err("invalid Body IR nominal pattern target".into());
    }
    for (slot, field_name) in declaration.fields.iter().enumerate() {
        if field_name != name {
            continue;
        }
        let ty = stored_field_type(
            builder.constructors(),
            builder.nominals(),
            builder.enums(),
            &builder.unions,
            declaration,
            slot,
            &builder.intrinsics,
        )?;
        let mut field = source.clone();
        for step in stored_steps(FieldProjection::new(slot as i64, ty)) {
            field = appended_field(&field, step, span)?;
        }
        return Ok(field);
    }
    Err("invalid Body IR nominal pattern field".into())
}

/// Compare an exact primitive literal carrier, borrowing strings through the existing equality runtime.
fn literal_test(
    builder: &mut Builder,
    source: &Place,
    value: &SourceConstant,
    span: &SourceSpan,
) -> Result<Place, String> {
    let ty = place_type(&builder.locals, source);
    if let SourceConstant::Str(text) = value {
        if ty != PlanType::String {
            return Err("invalid Body IR string pattern carrier".into());
        }
        return Ok(text_literal_test(builder, source, text, span));
    }
    let (constant, expected) = match value {
        SourceConstant::Int(number) => (Constant::Int(*number), PlanType::Int),
        SourceConstant::Float(text) => {
            let number = parse_float_string(text).ok_or_else(|| format!("invalid Body IR float constant {text}"))?;
            (Constant::Float(number), PlanType::Float)
        }
        SourceConstant::Bool(flag) => (Constant::Bool(*flag), PlanType::Bool),
        SourceConstant::TypedNumeric(number) => {
            let constant = numeric_constant(number)?;
            let Constant::Numeric(_, carrier) = &constant else {
                return Err("unsupported Body IR nonprimitive numeric literal pattern".into());
            };
            let carrier = carrier.clone();
            (constant, carrier)
        }
        SourceConstant::Unit => (Constant::Unit, PlanType::Unit),
        _ => return Err("unsupported Body IR nonprimitive literal pattern".into()),
    };
    if ty != expected {
        return Err("invalid Body IR literal pattern carrier".into());
    }
    let result = temporary(builder, PlanType::Bool, span);
    let operation = if ty == PlanType::Unit {
        RvalueKind::Use(literal(Constant::Bool(true), span))
    } else {
        RvalueKind::Binary(
            BinaryOp::Equal,
            read_place(source.clone(), false, span),
            literal(constant, span),
        )
    };
    builder.emit(plan_assign(result.clone(), operation, span));
    Ok(result)
}

/// Borrow both strings for equality and keep the materialized literal in the frame's owner ledger.
fn text_literal_test(builder: &mut Builder, source: &Place, text: &str, span: &SourceSpan) -> Place {
    let value = owned_literal(builder, text, span);
    builder.owned.push(value.local);
    let source_reference = temporary(builder, PlanType::StringRef, span);
    let value_reference = temporary(builder, PlanType::StringRef, span);
    builder.emit(plan_assign(
        source_reference.clone(),
        RvalueKind::Borrow(source.clone()),
        span,
    ));
    builder.emit(plan_assign(
        value_reference.clone(),
        RvalueKind::Borrow(value.clone()),
        span,
    ));
    let result = temporary(builder, PlanType::Bool, span);
    external_call(
        builder,
        "incan_native_runtime::equal_text",
        vec![PlanType::StringRef, PlanType::StringRef],
        PlanType::Bool,
        vec![
            read_place(source_reference, false, span),
            read_place(value_reference, false, span),
        ],
        result.clone(),
        span,
    );
    // Release the literal before branching: a rejected earlier tuple field never initializes later literals.
    drop_owned(builder, value.local, span);
    result
}
