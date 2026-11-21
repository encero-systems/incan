//! Construct models, enum variants and tuples from checked aggregates, with their retained field order and ownership.

use incan_mir_plan_native::{
    CalleeKind, ExternalFunction, Operand, Place, PlanType, RvalueKind, SourceSpan, TerminatorKind, TupleElement,
    Unwind, assign as plan_assign, tuple_element_type,
};
use incan_semantics_core::body_ir::{
    AggregateKind, ArgumentBinding, ArgumentElement, BodyIrModule, Constant as SourceConstant, NominalDeclaration,
    Operand as SourceOperand, Rvalue,
};

use crate::builtin_calls::coerce_numeric_operand;
use crate::carrier_operands::boundary_operand;
use crate::checked_values::{scalar_operand, scalar_place};
use crate::defaults::model_default_arguments;
use crate::element_places::is_element_place;
use crate::enums::variant_index;
use crate::explicit_arguments::reference_contract;
use crate::formatting::{display_operand, display_plan_operand, lower_format};
use crate::frame::{
    Builder, cleanup_exit, forget_owned, place_type, read_place, temporary, terminate_call, value_type,
};
use crate::layout_types::{native_source_type, stored_field_type};
use crate::list_values::value_operand;
use crate::models::constructor_index;
use crate::ranges::range_aggregate;
use crate::result_carriers::frozen_operand;
use crate::statements::{authenticated_component_operand, borrowed_payload, carrier_payload};
use crate::text_plan::owned_literal;
use crate::tuples::carrier_element;

/// Materialize a tuple or a proven plain-model constructor with its retained field order and ownership.
pub fn checked_aggregate(
    module: &BodyIrModule,
    builder: &mut Builder,
    kind: &AggregateKind,
    elements: &[ArgumentElement],
    expected: &PlanType,
    span: &SourceSpan,
) -> Result<RvalueKind, String> {
    match kind {
        AggregateKind::EnumVariant(target) => {
            let (owner, variant) = variant_index(builder.enums(), target, expected)?;
            let fields = builder.enums()[owner].variants[variant].fields.clone();
            if elements.len() != fields.len() {
                return Err("invalid Body IR enum constructor payload count".into());
            }
            let mut operands = Vec::with_capacity(elements.len());
            for (element, field_type) in elements.iter().zip(&fields) {
                let Some(value) = element.as_one() else {
                    return Err("unsupported Body IR spread enum payload".into());
                };
                let ty = native_source_type(
                    builder.constructors(),
                    builder.nominals(),
                    builder.enums(),
                    &builder.unions,
                    field_type,
                    &builder.intrinsics,
                    "",
                )?;
                if ty == PlanType::String {
                    let text = display_operand(builder, value, span)?;
                    operands.push(read_place(text, true, span));
                } else if ty == PlanType::FrozenStr {
                    operands.push(frozen_operand(builder, value, span)?);
                } else {
                    operands.push(boundary_operand(builder, value, &ty, span)?);
                }
            }
            Ok(RvalueKind::Enum(owner as i64, variant as i64, operands))
        }
        AggregateKind::Tuple => tuple_value(builder, elements, expected, span),
        AggregateKind::Constructor(target) => {
            let index = constructor_index(builder.nominals(), target, elements.len(), expected)?;
            match &target.binding {
                ArgumentBinding::Resolved { defaulted_slots, .. } => {
                    if defaulted_slots.is_empty() {
                        let operands = model_field_operands(builder, index, elements, span)?;
                        return Ok(RvalueKind::Model(index as i64, operands));
                    }
                }
                _ => return Err("unsupported Body IR unresolved model constructor".into()),
            }
            let operands = model_default_arguments(module, builder, index, &target.binding, elements, span)?;
            Ok(RvalueKind::Model(index as i64, operands))
        }
        AggregateKind::Range => range_value(builder, elements, expected, span),
        _ => Err("unsupported Body IR Aggregate outside plain models".into()),
    }
}

/// Materialize a tuple's elements at the destination's checked element carriers.
fn tuple_value(
    builder: &mut Builder,
    elements: &[ArgumentElement],
    expected: &PlanType,
    span: &SourceSpan,
) -> Result<RvalueKind, String> {
    let element_types: Vec<TupleElement> = match expected {
        PlanType::Tuple(types) => types.clone(),
        _ => Vec::new(),
    };
    let mut operands = Vec::with_capacity(elements.len());
    for (slot, element) in elements.iter().enumerate() {
        let Some(value) = element.as_one() else {
            return Err("unsupported Body IR spread tuple element".into());
        };
        let declared = element_types.get(slot);
        // A tuple carrying a view holds the view's loan in its reference element.
        if let Some(field) = declared
            && builder.carries_view
            && borrowed_payload(value)
            && reference_contract(&tuple_element_type(field)).is_ok()
        {
            operands.push(carrier_payload(builder, value, &tuple_element_type(field), span)?);
            continue;
        }
        // A list or carrier element takes the destination element's exact carrier, `None` included.
        if let Some(field) = declared
            && (carrier_element(field) || matches!(field, TupleElement::FunctionPointer(_)))
        {
            operands.push(boundary_operand(builder, value, &tuple_element_type(field), span)?);
            continue;
        }
        if let Some(TupleElement::FrozenStr) = declared {
            operands.push(frozen_operand(builder, value, span)?);
            continue;
        }
        let owns_text = declared.is_some_and(|field| tuple_element_type(field) == PlanType::String);
        match value {
            SourceOperand::Constant(SourceConstant::Str(text)) => {
                operands.push(read_place(owned_literal(builder, text, span), true, span));
            }
            SourceOperand::Constant(_) => {
                let operand = scalar_operand(builder, value, span)?;
                operands.push(owned_text(builder, operand, owns_text, span)?);
            }
            SourceOperand::Place(read) => {
                // A list or dictionary element is already a materialized copy the tuple can own.
                if is_element_place(builder, &read.place)? {
                    operands.push(value_operand(builder, value, span)?);
                    continue;
                }
                let target = scalar_place(builder, &read.place, span)?;
                if place_type(&builder.locals, &target) == PlanType::String {
                    operands.push(read_place(display_operand(builder, value, span)?, true, span));
                } else {
                    let operand = scalar_operand(builder, value, span)?;
                    operands.push(owned_text(builder, operand, owns_text, span)?);
                }
            }
        }
    }
    Ok(RvalueKind::Tuple(operands))
}

/// Turn frozen text into owned text for a tuple element the checker typed `str`: a `str` constant reads as frozen
/// text, and the tuple owns its text as a list element does. Any other operand is returned unchanged.
fn owned_text(builder: &mut Builder, operand: Operand, owns_text: bool, span: &SourceSpan) -> Result<Operand, String> {
    if owns_text && value_type(builder, &operand)? == PlanType::FrozenStr {
        return Ok(read_place(display_plan_operand(builder, operand, span)?, true, span));
    }
    Ok(operand)
}

/// Materialize canonical fields with their checked concrete types, preserving string and model ownership facts.
fn model_field_operands(
    builder: &mut Builder,
    index: usize,
    elements: &[ArgumentElement],
    span: &SourceSpan,
) -> Result<Vec<Operand>, String> {
    let mut operands = Vec::with_capacity(elements.len());
    for (slot, element) in elements.iter().enumerate() {
        match element.as_one() {
            Some(value) => operands.push(model_field_operand(builder, index, slot, value, span)?),
            None => return Err("unsupported Body IR spread model field".into()),
        }
    }
    Ok(operands)
}

/// Materialize one checked source operand into its exact canonical model field slot.
pub fn model_field_operand(
    builder: &mut Builder,
    index: usize,
    slot: usize,
    value: &SourceOperand,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let program = builder.program.clone();
    let declaration = &program.nominals[index];
    let field_type = stored_field_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        declaration,
        slot,
        &builder.intrinsics,
    )?;
    // A generic carrier selected as this body's returned view stores the authenticated source loan in its
    // reference-shaped field, just as a tuple or builtin result carrier stores its selected payload.
    if builder.carries_view && reference_contract(&field_type).is_ok() {
        return authenticated_component_operand(builder, value, &field_type, span);
    }
    if field_type == PlanType::String {
        let text = display_operand(builder, value, span)?;
        return Ok(read_place(text, true, span));
    }
    if let PlanType::Boxed(referent) = &field_type
        && let Some(inner) = referent.first()
    {
        let operand = boundary_operand(builder, value, inner, span)?;
        let stored = coerce_numeric_operand(builder, operand, inner, span)?;
        return Ok(boxed_operand(builder, stored, inner.clone(), span));
    }
    let operand = model_field_boundary(builder, value, declaration, slot, &field_type, span)?;
    // Generic inference has already selected this field's carrier; retain its widening just as a call slot does.
    coerce_numeric_operand(builder, operand, &field_type, span)
}

/// Add the closed field identity to a failed value-boundary diagnostic without changing materialization.
fn model_field_boundary(
    builder: &mut Builder,
    value: &SourceOperand,
    declaration: &NominalDeclaration,
    slot: usize,
    field_type: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    boundary_operand(builder, value, field_type, span).map_err(|reason| {
        format!(
            "{reason}; constructing {}.{} as {field_type:?}",
            declaration.name, declaration.fields[slot]
        )
    })
}

/// Move a checked field value onto the heap for a boxed class field, as `Box::new` does.
fn boxed_operand(builder: &mut Builder, value: Operand, referent: PlanType, span: &SourceSpan) -> Operand {
    let path = "std::boxed::Box::new";
    let boxed = PlanType::Boxed(vec![referent.clone()]);
    builder.externals.push(ExternalFunction {
        path: path.to_string(),
        type_arguments: vec![referent.clone()],
        parameters: vec![referent.clone()],
        return_type: boxed.clone(),
        span: span.clone(),
    });
    let destination = temporary(builder, boxed, span);
    terminate_call(
        builder,
        CalleeKind::Instantiated(path.to_string(), vec![referent]),
        vec![value],
        destination.clone(),
        span,
    );
    builder.owned.push(destination.local);
    read_place(destination, true, span)
}

/// Replace a projected string after evaluating its new value, retaining the containing owner or borrowed slot.
pub fn lower_string_field_assignment(
    builder: &mut Builder,
    destination: Place,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    // The text temporary is reserved before a displayed value selects its own storage, as the reference reserves it.
    let mut replacement = temporary(builder, PlanType::String, span);
    match value {
        Rvalue::Use(operand) => replacement = display_operand(builder, operand, span)?,
        Rvalue::Format(parts) => lower_format(builder, replacement.clone(), parts, span)?,
        _ => return Err("unsupported Body IR string field Rvalue outside formatting".into()),
    }
    let continuation = builder.new_block(span);
    let cleanup = cleanup_exit(builder, span);
    builder.finish(
        TerminatorKind::Drop(destination.clone(), continuation, Unwind::Cleanup(cleanup)),
        span,
    );
    builder.current = continuation as usize;
    forget_owned(builder, replacement.local);
    builder.emit(plan_assign(
        destination,
        RvalueKind::Use(read_place(replacement, true, span)),
        span,
    ));
    Ok(())
}

/// Copy a source range's checked scalar fields, in Body IR's declared order, into its native layout.
fn range_value(
    builder: &mut Builder,
    elements: &[ArgumentElement],
    expected: &PlanType,
    span: &SourceSpan,
) -> Result<RvalueKind, String> {
    let mut fields = Vec::with_capacity(elements.len());
    let mut types = Vec::with_capacity(elements.len());
    for element in elements {
        let Some(value) = element.as_one() else {
            return Err("unsupported Body IR spread range field".into());
        };
        let field = scalar_operand(builder, value, span)?;
        types.push(value_type(builder, &field)?);
        fields.push(field);
    }
    range_aggregate(expected, fields, &types)
}
