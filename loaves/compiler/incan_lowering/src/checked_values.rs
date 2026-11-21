//! Checked source locations, places and operands read into the native frame.

use incan_lang::numeric_strings::parse_float_string;
use incan_mir_plan_native::{
    Constant, Operand, OperandKind, Place, PlanType, Projection, RvalueKind, SourceSpan, assign as plan_assign,
    place as plan_place, source_span, tuple_element_type,
};
use incan_semantics_core::body_ir::{
    Constant as SourceConstant, Operand as SourceOperand, OwnershipFact, Place as SourcePlace, PlaceElem,
    TypedNumericConstant,
};
use incan_semantics_core::{HirSourceSpan, IncanType};

use crate::builtin_calls::{clone_enum_reference, clone_model_reference};
use crate::callable_values::{callable_object, cloned_closure};
use crate::carrier_operands::boundary_operand;
use crate::closures::callable_copy_type;
use crate::decimal_plan::decimal_literal;
use crate::element_places::is_element_place;
use crate::frame::{Builder, literal, place_type, read_place, temporary, track_rvalue_moves};
use crate::frozen_plan::{frozen_collection_literal, frozen_literal, frozen_observation_type};
use crate::layout_types::{native_source_type, shared_receiver_type};
use crate::list_values::element_read;
use crate::lists::{is_exclusive_borrow, is_list};
use crate::matches::enum_payload_read;
use crate::models::model_place;
use crate::newtype_constants::numeric_newtype_literal;
use crate::numerics::{numeric_carrier, numeric_constant};
use crate::options::{carrier_payload_value, clone_tuple_operand};
use crate::scalar_reads::scalar_storage_place;
use crate::statements::is_nominal_type;
use crate::static_storage::read_static;
use crate::text_plan::{clone_text, owned_literal};
use crate::tuples::{is_tuple_type, owned_carrier, tuple_has_owners};
use crate::unions::union_projection;

/// Map the checked byte offset to its line and byte column through borrowed UTF-8 line views.
pub fn location(span: &HirSourceSpan, source: &str, file: &str) -> SourceSpan {
    let offset = span.start as i64;
    let mut start: i64 = 0;
    let mut number: i64 = 1;
    for line in source.split('\n') {
        let width = line.len() as i64;
        if offset <= start + width {
            return source_span(file, number, offset - start + 1);
        }
        start += width + 1;
        number += 1;
    }
    source_span(file, number, 1)
}

/// Each line's starting byte offset, then the offset just past the last line, as `location` counts them.
pub fn line_starts(source: &str) -> Vec<i64> {
    let mut starts = vec![0];
    let mut start: i64 = 0;
    for line in source.split('\n') {
        start += line.len() as i64 + 1;
        starts.push(start);
    }
    starts
}

/// Map a byte offset as `location` does, by binary search over the frame's line starts.
pub fn indexed_location(starts: &[i64], span: &HirSourceSpan, file: &str) -> SourceSpan {
    let offset = span.start as i64;
    let mut low: usize = 0;
    let mut high: usize = starts.len() - 1;
    while low < high {
        let middle = (low + high + 1) / 2;
        if starts[middle] <= offset {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    // An offset past the last line takes the line after it, at its first column.
    if low == starts.len() - 1 {
        return source_span(file, low as i64 + 1, 1);
    }
    source_span(file, low as i64 + 1, offset - starts[low] + 1)
}

/// Map a span in the frame's source through its line starts; a frame without them scans its source.
pub fn builder_location(builder: &Builder, span: &HirSourceSpan) -> SourceSpan {
    if builder.line_starts.is_empty() {
        return location(span, &builder.source, &builder.file);
    }
    indexed_location(&builder.line_starts, span, &builder.file)
}

/// Map a local or proven model field into the native frame, which reserves local zero for return.
pub fn scalar_place(builder: &mut Builder, value: &SourcePlace, span: &SourceSpan) -> Result<Place, String> {
    validate_callback_place(builder, value)?;
    if let Some(projected) = union_projection(builder, value, span)? {
        return Ok(projected);
    }
    // The frame's slots start at its offset in the shared locals table; borrow the table rather than copy a suffix.
    let mut target = match model_place(builder.tables(), &builder.locals, value, span, builder.local_offset) {
        Ok(place) => place,
        Err(error) => return Err(format!("{error} in {} at {}:{}", builder.name, span.file, span.line)),
    };
    target.local += builder.local_offset;
    scalar_storage_place(&builder.locals, &builder.reference_values, target)
}

/// Preserve each scalar read's checked ownership fact rather than inferring it.
pub fn scalar_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Operand, String> {
    let constant = match value {
        SourceOperand::Place(read) => return place_read(builder, &read.place, read.fact, span),
        SourceOperand::Constant(constant) => constant,
    };
    match constant {
        SourceConstant::RustItem(path, ty) => {
            let carrier = builder_type(builder, ty)?;
            Ok(literal(Constant::RustItem(path.clone(), carrier), span))
        }
        SourceConstant::TypeToken(represented) => {
            let argument = builder_type(builder, represented)?;
            Ok(literal(Constant::TypeToken(vec![argument]), span))
        }
        SourceConstant::FrozenAggregate(ty, elements) => {
            let layout = builder_type(builder, ty)?;
            if is_tuple_type(&layout) {
                return retained_tuple_constant(builder, layout, elements, span);
            }
            frozen_collection_literal(builder, constant, span)
        }
        SourceConstant::NumericNewtype(_) => numeric_newtype_literal(builder, constant, span),
        SourceConstant::Str(text) => Ok(read_place(owned_literal(builder, text, span), true, span)),
        SourceConstant::FrozenStr(_) => frozen_literal(builder, constant, span),
        SourceConstant::TypedNumeric(value) if is_decimal_literal(value) => decimal_literal(builder, value, span),
        _ => scalar_constant(constant, span),
    }
}

/// Resolve a checked type against the frame's tables and source file.
fn builder_type(builder: &Builder, ty: &IncanType) -> Result<PlanType, String> {
    native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        ty,
        &builder.intrinsics,
        &builder.file,
    )
}

/// Retain the Rust enum import through a typed boundary; legacy drops pattern-only imports until dev.8.
fn is_decimal_literal(value: &TypedNumericConstant) -> bool {
    matches!(value, TypedNumericConstant::Decimal { .. })
}

/// Materialize a retained literal tuple at its checked field carriers, transferring owned fields into one cleanup
/// owner.
fn retained_tuple_constant(
    builder: &mut Builder,
    layout: PlanType,
    elements: &[SourceConstant],
    span: &SourceSpan,
) -> Result<Operand, String> {
    let PlanType::Tuple(fields) = &layout else {
        return Err("unsupported Body IR retained constant without tuple layout".into());
    };
    if fields.len() != elements.len() {
        return Err("invalid Body IR retained tuple constant arity".into());
    }
    let mut values = Vec::with_capacity(fields.len());
    for (field, element) in fields.iter().zip(elements) {
        let operand = SourceOperand::Constant(element.clone());
        values.push(boundary_operand(builder, &operand, &tuple_element_type(field), span)?);
    }
    let output = temporary(builder, layout.clone(), span);
    let operation = RvalueKind::Tuple(values);
    track_rvalue_moves(builder, &operation);
    builder.emit(plan_assign(output.clone(), operation, span));
    let owned = tuple_has_owners(&layout);
    if owned {
        builder.owned.push(output.local);
    }
    Ok(read_place(output, owned, span))
}

/// Turn a place read into a copy or move exactly as Body IR's ownership fact says.
pub fn place_read(
    builder: &mut Builder,
    value: &SourcePlace,
    fact: OwnershipFact,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if let Some(global) = value.global() {
        return read_static(builder, value, global, fact, span);
    }
    if let Some(reference) = reference_value_read(builder, value, fact, span)? {
        return Ok(reference);
    }
    // An element read materializes a copy, which would silently drop a write through a mutable borrow.
    if fact == OwnershipFact::MutBorrow && is_element_place(builder, value)? {
        return Err("unsupported Body IR mutable list element borrow".into());
    }
    if fact == OwnershipFact::Move
        && value.projection.is_empty()
        && let Some(local) = value.local_id()
    {
        let index = local.index() as i64 + 1 + builder.local_offset;
        if is_exclusive_borrow(&builder.locals[index as usize].ty) {
            return Ok(read_place(plan_place(index, span), true, span));
        }
    }
    if let Some(operand) = element_read(builder, value, span)? {
        return Ok(operand);
    }
    let source = scalar_place(builder, value, span)?;
    target_read(builder, source, fact, span)
}

/// Admit checked value reads of a logical reference. Copy, clone, borrow, and last-use transfer never authorize moving
/// its referent; unknown and mutable borrowing remain separate operations.
pub fn reference_read_fact(fact: OwnershipFact) -> bool {
    matches!(
        fact,
        OwnershipFact::Copy | OwnershipFact::Clone | OwnershipFact::Borrow | OwnershipFact::Move
    )
}

/// Read an explicitly reference-typed slot or structural tuple field as its reference, never its referent.
/// Shared references can copy; exclusive references transfer only under a checked Move. Borrowed storage for
/// source receivers is ineligible.
pub fn reference_value_read(
    builder: &mut Builder,
    value: &SourcePlace,
    fact: OwnershipFact,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    if !value.projection.is_empty() {
        for projection in &value.projection {
            match projection {
                PlaceElem::Field { structural: true, .. } => {}
                _ => return Ok(None),
            }
        }
        let target = scalar_place(builder, value, span)?;
        if let PlanType::StrRef
        | PlanType::ScalarRef(_)
        | PlanType::ModelRef(..)
        | PlanType::SliceRef(_)
        | PlanType::NativeArrayRef(..)
        | PlanType::StringRef
        | PlanType::EnumRef(..) = place_type(&builder.locals, &target)
        {
            if !reference_read_fact(fact) {
                return Err("unsupported Body IR projected reference without a checked read".into());
            }
            return Ok(Some(read_place(target, false, span)));
        }
        return Ok(None);
    }
    let Some(local) = value.local_id() else {
        return Ok(None);
    };
    let index = local.index() as i64 + 1 + builder.local_offset;
    if !builder.reference_values.contains(&index) {
        return Ok(None);
    }
    if !reference_read_fact(fact) {
        return Err("unsupported Body IR reference value without a checked copy or transfer".into());
    }
    if is_exclusive_borrow(&builder.locals[index as usize].ty) && fact != OwnershipFact::Move {
        return Err("unsupported Body IR exclusive reference value without a checked transfer".into());
    }
    Ok(Some(read_place(
        plan_place(index, span),
        fact == OwnershipFact::Move,
        span,
    )))
}

/// Limit field transfers to a direct tuple component whose remainder the frame can track.
fn is_direct_tuple_field(place: &Place) -> bool {
    matches!(place.projection, Projection::Field(..))
}

/// Read callable storage or apply checked ownership to ordinary fields and payloads through their actual carrier.
pub fn target_read(
    builder: &mut Builder,
    target: Place,
    fact: OwnershipFact,
    span: &SourceSpan,
) -> Result<Operand, String> {
    // Only the binding remembers a partial's supplied flags, so a partial is called or moved into a binding, never
    // read.
    if partial_flags(builder, target.local).is_some() {
        return Err("unsupported Body IR partial read outside a call or a binding".into());
    }
    let ty = place_type(&builder.locals, &target);
    if callable_copy_type(&ty) {
        return match fact {
            OwnershipFact::Unknown => Err("unsupported Body IR unknown callable ownership".into()),
            OwnershipFact::MutBorrow => Err("unsupported Body IR mutable callable borrow".into()),
            _ => Ok(read_place(target, false, span)),
        };
    }
    match &ty {
        PlanType::TupleRef(_) => match fact {
            OwnershipFact::Unknown => Err("unsupported Body IR unknown shared tuple ownership".into()),
            OwnershipFact::MutBorrow => Err("unsupported Body IR mutable shared tuple borrow".into()),
            _ => Ok(read_place(target, false, span)),
        },
        PlanType::Future(_) => {
            if fact != OwnershipFact::Move || target.projection != Projection::Whole {
                return Err("unsupported Body IR future read without whole ownership transfer".into());
            }
            Ok(read_place(target, true, span))
        }
        PlanType::ClosureRef(_) | PlanType::BorrowingClosureRef(_) => match fact {
            OwnershipFact::Unknown => Err("unsupported Body IR unknown callable-object ownership".into()),
            OwnershipFact::MutBorrow => Err("unsupported Body IR mutable callable-object borrow".into()),
            _ => Ok(read_place(target, false, span)),
        },
        PlanType::NativeClosure(..) => {
            if fact != OwnershipFact::Move || target.projection != Projection::Whole {
                return Err("unsupported Body IR native callback read without owned transfer".into());
            }
            Ok(read_place(target, true, span))
        }
        PlanType::Closure(signature) => closure_read(builder, target, &signature, fact, span),
        _ => owned_target_read(builder, target, fact, span),
    }
}

/// A narrowed payload stays owned by its carrier; reading its value snapshots only the selected environment.
fn closure_read(
    builder: &mut Builder,
    target: Place,
    signature: &[PlanType],
    fact: OwnershipFact,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if fact == OwnershipFact::Borrow {
        match target.projection {
            Projection::VariantField(..) => return target_read(builder, target, OwnershipFact::Clone, span),
            Projection::Field(..) | Projection::Fields(_) | Projection::DerefField(..) | Projection::DerefFields(_) => {
                return callable_object(builder, target, signature, span);
            }
            _ => {}
        }
    }
    match fact {
        OwnershipFact::Move => {
            if target.projection != Projection::Whole {
                return Err("unsupported Body IR partial closure move".into());
            }
            Ok(read_place(target, true, span))
        }
        OwnershipFact::Clone => cloned_closure(builder, target, signature, span),
        _ => {
            let site = format!("{}:{}:{} in {}", builder.file, span.line, span.column, builder.name);
            Err(format!(
                "unsupported Body IR closure read outside a call, argument, clone, or move at {site}; ownership {fact:?}; projection {:?}",
                target.projection
            ))
        }
    }
}

/// Apply checked ownership to ordinary storage after callable and producer representations have been handled.
fn owned_target_read(
    builder: &mut Builder,
    target: Place,
    fact: OwnershipFact,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let selected = place_type(&builder.locals, &target);
    let holder = builder.locals[target.local as usize].ty.clone();
    match fact {
        OwnershipFact::Copy => Ok(Operand {
            kind: OperandKind::Copy(target),
            span: span.clone(),
        }),
        OwnershipFact::Move => {
            let transferred_tuple_field = is_tuple_type(&holder) && is_direct_tuple_field(&target);
            let partial = target.projection != Projection::Whole && !transferred_tuple_field;
            if partial && (selected == PlanType::String || tuple_has_owners(&selected)) {
                return Err("unsupported Body IR partial model move".into());
            }
            // The whole tuple stays an owner, so moving an owned element out of it would drop that element twice.
            if partial && is_tuple_type(&holder) && owned_carrier(&selected) {
                return Err("unsupported Body IR partial tuple move".into());
            }
            // Preserve an immutable parameter as a shared operand. A typed owned boundary materializes it later, at
            // the escaping operation, while a read-only receiver consumes the reference directly.
            if is_nominal_type(&selected) && reads_through_reference(&target) {
                return shared_nominal_read(builder, target, span);
            }
            Ok(Operand {
                kind: OperandKind::Move(target),
                span: span.clone(),
            })
        }
        OwnershipFact::Clone => {
            if frozen_observation_type(&selected) {
                return Ok(read_place(target, false, span));
            }
            // A checked clone of an open parameter becomes Rust's copy-valued Clone on concrete scalar instances.
            // A range value's four scalar fields own nothing, so its clone is a copy, as legacy's is.
            if numeric_carrier(&selected)
                || matches!(
                    selected,
                    PlanType::Bool | PlanType::Unit | PlanType::Decimal | PlanType::RangeValue
                )
            {
                return Ok(Operand {
                    kind: OperandKind::Copy(target),
                    span: span.clone(),
                });
            }
            if is_tuple_type(&selected) {
                return clone_tuple_operand(builder, target, span);
            }
            if is_list(&selected) && is_tuple_type(&holder) {
                return carrier_payload_value(builder, target, span);
            }
            if selected == PlanType::String {
                let cloned = clone_text(builder, target, span)?;
                return Ok(read_place(cloned, true, span));
            }
            clone_nominal_operand(builder, target, span)
        }
        OwnershipFact::Borrow => borrowed_target_read(builder, target, selected, &holder, span),
        OwnershipFact::MutBorrow => Err("unsupported Body IR ownership MutBorrow".into()),
        OwnershipFact::Unknown => Err("unsupported Body IR ownership Unknown".into()),
    }
}

/// A checked projected read borrows its parent: materialize only the selected value, never the still-owned parent.
fn borrowed_target_read(
    builder: &mut Builder,
    target: Place,
    selected: PlanType,
    holder: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if frozen_observation_type(&selected) {
        return Ok(read_place(target, false, span));
    }
    if is_tuple_type(&selected) {
        return clone_tuple_operand(builder, target, span);
    }
    // A list held by a tuple element is materialized by cloning it, leaving the owning tuple whole.
    if is_list(&selected) && is_tuple_type(holder) {
        return carrier_payload_value(builder, target, span);
    }
    // Materialize only the selected enum or model through its real Clone implementation; copying or moving that field
    // would invalidate the still-owned parent. A model read through a shared receiver is cloned the same way, as legacy
    // clones `self` to return it.
    match &selected {
        PlanType::StringRef => return Ok(read_place(target, false, span)),
        PlanType::Enum(..) => match target.projection {
            Projection::Field(..) | Projection::VariantField(..) | Projection::Fields(_) => {
                return clone_nominal_operand(builder, target, span);
            }
            Projection::DerefField(..) | Projection::DerefFields(_) => {
                return shared_nominal_read(builder, target, span);
            }
            _ => {}
        },
        PlanType::Model(..) => match target.projection {
            Projection::Whole | Projection::VariantField(..) => {}
            _ => {
                if reads_through_reference(&target) {
                    return shared_nominal_read(builder, target, span);
                }
                return clone_nominal_operand(builder, target, span);
            }
        },
        PlanType::ModelRef(..) => return clone_nominal_operand(builder, target, span),
        _ => {}
    }
    if let Projection::VariantField(..) = target.projection {
        return enum_payload_read(builder, target, OwnershipFact::Borrow, span);
    }
    // A checked borrow of a value of an open type parameter, a field of a generic model say, reads a copy once the
    // instance makes it a concrete scalar, which owns nothing the borrow would protect.
    if numeric_carrier(&selected) || matches!(selected, PlanType::Bool | PlanType::Unit | PlanType::Decimal) {
        return Ok(Operand {
            kind: OperandKind::Copy(target),
            span: span.clone(),
        });
    }
    let site = format!("{}:{}:{} in {}", builder.file, span.line, span.column, builder.name);
    let detail = format!("projection {:?}; selected type {selected:?}", target.projection);
    Err(format!(
        "unsupported Body IR ownership Borrow outside formatting at {site}; {detail}"
    ))
}

/// Retain a whole or projected nominal reached through shared frame storage as an exact shared operand.
fn shared_nominal_read(builder: &mut Builder, target: Place, span: &SourceSpan) -> Result<Operand, String> {
    let selected = place_type(&builder.locals, &target);
    let reference_type = shared_receiver_type(selected.clone())?;
    let root = plan_place(target.local, span);
    if target.projection == Projection::Deref(selected) && place_type(&builder.locals, &root) == reference_type {
        return Ok(read_place(root, false, span));
    }
    let reference = temporary(builder, reference_type, span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(target), span));
    Ok(read_place(reference, false, span))
}

/// Report whether a place reaches its value through a reference: a receiver, a `mut` parameter or an in-place view.
fn reads_through_reference(place: &Place) -> bool {
    matches!(
        place.projection,
        Projection::Deref(_) | Projection::DerefField(..) | Projection::DerefFields(_)
    )
}

/// Turn a scalar literal into a plan constant; other literal kinds are refused by name.
pub fn scalar_constant(constant: &SourceConstant, span: &SourceSpan) -> Result<Operand, String> {
    match constant {
        SourceConstant::Int(number) => Ok(literal(Constant::Int(*number), span)),
        SourceConstant::RustItem(..) => Err("Rust constant requires its checked carrier".into()),
        SourceConstant::Float(text) => {
            let value = parse_float_string(text).ok_or_else(|| format!("invalid Body IR float constant {text}"))?;
            Ok(literal(Constant::Float(value), span))
        }
        SourceConstant::Bool(flag) => Ok(literal(Constant::Bool(*flag), span)),
        SourceConstant::Unit => Ok(literal(Constant::Unit, span)),
        SourceConstant::TypedNumeric(value) => Ok(literal(numeric_constant(value)?, span)),
        SourceConstant::Str(_) => Err("unsupported Body IR Str constant".into()),
        SourceConstant::FrozenStr(_) => Err("Body IR FrozenStr constant requires its retained carrier".into()),
        SourceConstant::Bytes(_) => Err("unsupported Body IR Bytes constant".into()),
        SourceConstant::NumericNewtype(_) => Err("unsupported Body IR numeric newtype scalar constant".into()),
        SourceConstant::None => Err("unsupported Body IR None constant".into()),
        SourceConstant::TypeToken(_) => Err("type token constants require their checked represented type".into()),
        SourceConstant::FrozenAggregate(..) => Err("unsupported Body IR frozen aggregate scalar constant".into()),
    }
}

/// The default flags recorded for a local that holds a closure whose parameters carry defaults.
pub fn partial_flags(builder: &Builder, local: i64) -> Option<&[bool]> {
    builder
        .partial_callables
        .iter()
        .find(|entry| entry.0 == local)
        .map(|entry| entry.1.as_slice())
}

/// Reject a lexical capture before any source read or write can enter an uninitialized callback frame.
pub fn validate_callback_place(builder: &Builder, value: &SourcePlace) -> Result<(), String> {
    if let Some(local) = value.local_id() {
        let index = local.index() as i64 + 1 + builder.local_offset;
        if builder.forbidden_reads.contains(&index) {
            return Err("unsupported Body IR Race callback capture".into());
        }
    }
    Ok(())
}

/// Materialize the checker's clone fact through a source nominal's real derived Clone implementation.
///
/// A shared model reference, such as a receiver, already is the borrow that Clone takes. An immutable scalar reference,
/// including a text reference, function pointer or static frozen wrapper copies the value through the ordinary
/// carrier-payload read.
pub fn clone_nominal_operand(builder: &mut Builder, value: Place, span: &SourceSpan) -> Result<Operand, String> {
    match place_type(&builder.locals, &value) {
        PlanType::FunctionPointer(_)
        | PlanType::ScalarRef(_)
        | PlanType::FrozenStr
        | PlanType::FrozenBytes
        | PlanType::StringRef
        | PlanType::ValidationError => carrier_payload_value(builder, value, span),
        PlanType::ValidationErrorRef => {
            let owned = Place {
                local: value.local,
                projection: Projection::Deref(PlanType::ValidationError),
                span: span.clone(),
            };
            carrier_payload_value(builder, owned, span)
        }
        PlanType::TupleRef(_) | PlanType::FrozenCollectionRef(..) => Ok(read_place(value, false, span)),
        PlanType::ModelRef(index, name) => Ok(clone_model_reference(
            builder,
            read_place(value, false, span),
            index,
            &name,
            span,
        )),
        PlanType::EnumRef(index, name) => Ok(clone_enum_reference(
            builder,
            read_place(value, false, span),
            index,
            &name,
            span,
        )),
        PlanType::Model(index, name) => {
            let reference = temporary(builder, PlanType::ModelRef(index, name.clone()), span);
            builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(value), span));
            Ok(clone_model_reference(
                builder,
                read_place(reference, false, span),
                index,
                &name,
                span,
            ))
        }
        PlanType::Enum(index, name) => {
            // SDK enums own their Clone implementation; the local derive callee cannot validate a foreign layout.
            let native = builder
                .enums()
                .get(index as usize)
                .is_some_and(|declaration| !crate::decls::library_enum_item(declaration).is_empty());
            if index >= 0 && native {
                return carrier_payload_value(builder, value, span);
            }
            let reference = temporary(builder, PlanType::EnumRef(index, name.clone()), span);
            builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(value), span));
            Ok(clone_enum_reference(
                builder,
                read_place(reference, false, span),
                index,
                &name,
                span,
            ))
        }
        ty => {
            let site = format!("{}:{}:{} in {}", builder.file, span.line, span.column, builder.name);
            Err(format!(
                "unsupported Body IR ownership Clone outside models or formatting at {site}; type {ty:?}; projection {:?}",
                value.projection
            ))
        }
    }
}
