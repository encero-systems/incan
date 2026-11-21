//! Standard derive methods: Clone of any admitted carrier.

use incan_mir_plan_native::{
    CalleeKind, OperandKind, Place, PlanType, Projection, RvalueKind, SourceSpan, assign as plan_assign,
};
use incan_semantics_core::body_ir::{ArgumentElement, Operand as SourceOperand, Place as SourcePlace};

use crate::callable_calls::lower_callable_result;
use crate::checked_values::scalar_place;
use crate::decls::library_enum_item;
use crate::element_places::is_element_place;
use crate::frame::{Builder, drop_owned, forget_owned, place_type, read_place, temporary, track_rvalue_moves};
use crate::list_methods::materialized_receiver;
use crate::numerics::numeric_carrier;
use crate::options::{carrier_payload_value, clone_tuple_operand};
use crate::text_plan::clone_text;
use crate::tuples::is_tuple_type;

/// Clone a standard or inferred Clone receiver through its concrete native representation.
///
/// Scalars copy and owned strings clone through the runtime. Models and classes always derive Clone; a source enum's
/// checked derives must name it. Builtin aggregates and synthetic union, Option, and Result carriers use the same
/// native cloning path as a borrowed payload read. An indexed receiver follows the ordinary element read once before
/// its selected Clone call.
pub fn lower_derived_clone(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 1 {
        return Err("invalid Body IR Clone arguments".into());
    }
    let Some(value) = arguments[0].as_one() else {
        return Err("unsupported Body IR Clone spread receiver".into());
    };
    let SourceOperand::Place(read) = value else {
        return Err("unsupported Body IR Clone of a nonplace receiver".into());
    };
    let source = if is_element_place(builder, &read.place)? {
        materialized_receiver(builder, &read.place, span)?
    } else {
        scalar_place(builder, &read.place, span)?
    };
    clone_place(builder, source, destination, span)
}

/// Clone through the retained carrier and its owning implementation; compiled enums do not acquire local derives.
fn clone_place(
    builder: &mut Builder,
    source: Place,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let ty = place_type(&builder.locals, &source);
    let referent = match &ty {
        PlanType::StringRef => Some(PlanType::String),
        PlanType::TupleRef(elements) => Some(PlanType::Tuple(elements.clone())),
        PlanType::ListRef(leaf, depth) => Some(PlanType::List(leaf.clone(), *depth)),
        PlanType::SetRef(leaf) => Some(PlanType::Set(leaf.clone())),
        PlanType::DictRef(key, value) => Some(PlanType::Dict(key.clone(), value.clone())),
        PlanType::ModelRef(owner, name) => Some(PlanType::Model(*owner, name.clone())),
        PlanType::EnumRef(owner, name) => Some(PlanType::Enum(*owner, name.clone())),
        PlanType::ValidationErrorRef => Some(PlanType::ValidationError),
        _ => None,
    };
    if let Some(referent) = referent {
        return clone_referent(builder, source, referent, destination, span);
    }
    if let PlanType::ScalarRef(elements) = &ty
        && elements.len() == 1
    {
        let element = &elements[0];
        let copyable = numeric_carrier(element)
            || matches!(element, PlanType::Bool | PlanType::Unit | PlanType::FunctionPointer(_));
        if copyable {
            let reference = temporary(builder, ty.clone(), span);
            builder.emit(plan_assign(
                reference.clone(),
                RvalueKind::Use(read_place(source, false, span)),
                span,
            ));
            let pointer = Place {
                local: reference.local,
                projection: Projection::Deref(element.clone()),
                span: span.clone(),
            };
            return copy_clone_into(builder, pointer, element, destination, span);
        }
    }
    if numeric_carrier(&ty) || matches!(ty, PlanType::Bool | PlanType::Unit | PlanType::FrozenStr) {
        return copy_clone_into(builder, source, &ty, destination, span);
    }
    if ty == PlanType::String {
        return clone_string_into(builder, source, destination, span);
    }
    match &ty {
        PlanType::Model(index, name) => {
            let reference = PlanType::ModelRef(*index, name.clone());
            let callee = CalleeKind::CloneModel(*index, name.clone());
            clone_into(builder, source, reference, callee, ty.clone(), destination, span)
        }
        PlanType::Enum(index, name) => {
            if *index >= builder.enums().len() as i64 {
                return clone_builtin_place(builder, source, destination, span);
            }
            if *index < 0 {
                return Err("unsupported Body IR enum Clone without a derived Clone".into());
            }
            let declaration = &builder.enums()[*index as usize];
            if !library_enum_item(declaration).is_empty() {
                return clone_builtin_place(builder, source, destination, span);
            }
            if !declaration.derives.iter().any(|derive| derive == "Clone") {
                return Err("unsupported Body IR enum Clone without a derived Clone".into());
            }
            let reference = PlanType::EnumRef(*index, name.clone());
            let callee = CalleeKind::CloneEnum(*index, name.clone());
            clone_into(builder, source, reference, callee, ty.clone(), destination, span)
        }
        _ => clone_builtin_place(builder, source, destination, span),
    }
}

/// Clone a selected shared receiver's referent, including a stored reference projected from a carrier.
///
/// The standard Clone selection returns the referent's owned type. Copying its shared reference would instead change
/// the result ABI and retain the caller's storage. Materialize the reference once before dereferencing it.
fn clone_referent(
    builder: &mut Builder,
    source: Place,
    ty: PlanType,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let reference = temporary(builder, place_type(&builder.locals, &source), span);
    builder.emit(plan_assign(
        reference.clone(),
        RvalueKind::Use(read_place(source, false, span)),
        span,
    ));
    let referent = Place {
        local: reference.local,
        projection: Projection::Deref(ty),
        span: span.clone(),
    };
    clone_place(builder, referent, destination, span)
}

/// Copy an admitted scalar or frozen string without consuming the receiver; a discarded copy has no effect.
fn copy_clone_into(
    builder: &mut Builder,
    source: Place,
    ty: &PlanType,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if let Some(place) = destination {
        let output = scalar_place(builder, place, span)?;
        if place_type(&builder.locals, &output) != *ty {
            return Err("unsupported Body IR Clone destination carrier mismatch".into());
        }
        builder.emit(plan_assign(
            output,
            RvalueKind::Use(read_place(source, false, span)),
            span,
        ));
    }
    Ok(())
}

/// Borrow the source, replace a whole owned text destination after cloning, or immediately drop a discarded clone.
fn clone_string_into(
    builder: &mut Builder,
    source: Place,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    match destination {
        Some(place) => {
            let output = scalar_place(builder, place, span)?;
            if output.projection != Projection::Whole || place_type(&builder.locals, &output) != PlanType::String {
                return Err("unsupported Body IR Clone projected or nontext destination".into());
            }
            let cloned = clone_text(builder, source, span)?;
            drop_owned(builder, output.local, span);
            forget_owned(builder, cloned.local);
            let local = output.local;
            builder.emit(plan_assign(
                output,
                RvalueKind::Use(read_place(cloned, true, span)),
                span,
            ));
            builder.owned.push(local);
        }
        None => {
            let cloned = clone_text(builder, source, span)?;
            drop_owned(builder, cloned.local, span);
        }
    }
    Ok(())
}

/// Borrow the receiver and write the owned copy to the call's destination, replacing what it held.
#[allow(clippy::too_many_arguments)]
fn clone_into(
    builder: &mut Builder,
    source: Place,
    reference_type: PlanType,
    callee: CalleeKind,
    ty: PlanType,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let reference = temporary(builder, reference_type, span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(source), span));
    let output = match destination {
        Some(place) => scalar_place(builder, place, span)?,
        None => temporary(builder, ty.clone(), span),
    };
    lower_callable_result(
        builder,
        callee,
        vec![read_place(reference, false, span)],
        output,
        ty,
        span,
    )
}

/// Copy builtin scalars and clone the owned storage of carriers through the existing borrowed payload operation.
fn clone_builtin_place(
    builder: &mut Builder,
    source: Place,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let value = if is_tuple_type(&place_type(&builder.locals, &source)) {
        clone_tuple_operand(builder, source, span)?
    } else {
        carrier_payload_value(builder, source, span)?
    };
    if let Some(place) = destination {
        let output = scalar_place(builder, place, span)?;
        if output.projection != Projection::Whole {
            return Err("unsupported Body IR projected builtin Clone destination".into());
        }
        let owned = matches!(value.kind, OperandKind::Move(_));
        if owned {
            drop_owned(builder, output.local, span);
        }
        track_rvalue_moves(builder, &RvalueKind::Use(value.clone()));
        let local = output.local;
        builder.emit(plan_assign(output, RvalueKind::Use(value), span));
        if owned {
            builder.owned.push(local);
        }
    }
    Ok(())
}
