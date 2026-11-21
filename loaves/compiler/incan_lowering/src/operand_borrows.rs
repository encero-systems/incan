//! Operands lowered for their effects or as borrows: discarded values, text views and shared slices.

use incan_mir_plan_native::{
    CalleeKind, ExternalFunction, Operand, OperandKind, Place, PlanType, Projection, RvalueKind, SourceSpan,
    assign as plan_assign,
};
use incan_semantics_core::body_ir::{
    ArgumentElement, Constant as SourceConstant, Operand as SourceOperand, OwnershipFact, Place as SourcePlace,
    PlaceOperand,
};

use crate::array_views::borrow_array_slice;
use crate::builtin_calls::assign_text_result;
use crate::bytes_plan::bytes_literal;
use crate::checked_values::{reference_value_read, scalar_operand, scalar_place};
use crate::element_borrows::shared_element_place;
use crate::element_places::is_element_place;
use crate::formatting::{display_operand, display_plan_operand};
use crate::frame::{
    Builder, drop_owned, external_call, forget_owned, place_type, read_place, temporary, terminate_call, text_literal,
    value_type,
};
use crate::layout_types::{mutable_receiver_type, shared_receiver_type};
use crate::list_values::value_operand;
use crate::list_values::{checked_list_argument, has_slice_projection, list_borrow};
use crate::lists::is_list;
use crate::lists::{list_element, require_list_shape};
use crate::numerics::numeric_carrier;
use crate::pending::untranslated;
use crate::static_storage::read_static;
use crate::strings::{emit_text_helper, text_helper_arity};
use crate::text_ownership::drop_borrowed_text_temporaries;
use crate::text_plan::{owned_literal, owned_slice_text};

/// Evaluate discarded text through retained ownership facts and drop its materialized owner.
pub fn lower_discarded_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<(), String> {
    let is_text = match value {
        // A discarded string constant, such as a docstring, has no effect: materializing it would allocate and free
        // its text on every call.
        SourceOperand::Constant(SourceConstant::Str(_)) => return Ok(()),
        SourceOperand::Constant(_) => false,
        SourceOperand::Place(read) => {
            let place = scalar_place(builder, &read.place, span)?;
            place_type(&builder.locals, &place) == PlanType::String
        }
    };
    if is_text {
        let text = display_operand(builder, value, span)?;
        drop_owned(builder, text.local, span);
    } else {
        value_operand(builder, value, span)?;
    }
    Ok(())
}

/// Pass literal slice text directly or borrow owned text through String's own as_str method.
pub fn borrow_str_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Operand, String> {
    match value {
        SourceOperand::Constant(SourceConstant::Str(text)) => return Ok(text_literal(text, span)),
        SourceOperand::Place(read) => {
            let place = scalar_place(builder, &read.place, span)?;
            if place_type(&builder.locals, &place) == PlanType::StrRef {
                return Ok(read_place(place, false, span));
            }
        }
        _ => {}
    }
    let reference = borrow_text_operand(builder, value, span)?;
    let output = temporary(builder, PlanType::StrRef, span);
    external_call(
        builder,
        "alloc::string::String::as_str",
        vec![PlanType::StringRef],
        PlanType::StrRef,
        vec![reference],
        output.clone(),
        span,
    );
    Ok(read_place(output, false, span))
}

/// Borrow owned text or reuse a checked shared String reference without transferring its owner or adding a reference
/// layer.
pub fn borrow_text_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Operand, String> {
    let read = match value {
        SourceOperand::Place(read) => read,
        SourceOperand::Constant(constant) => {
            let text = match constant {
                SourceConstant::Str(text) | SourceConstant::FrozenStr(text) => text,
                _ => return Err("unsupported Body IR non-string text constant".into()),
            };
            let owner = owned_literal(builder, text, span);
            return Ok(borrow_string(builder, owner, span));
        }
    };
    if let Some(global) = read.place.global() {
        let owner = read_static(builder, &read.place, global, read.fact, span)?;
        let OperandKind::Move(place) = owner.kind else {
            return Err("unsupported Body IR non-string static text operand".into());
        };
        let reference = temporary(builder, PlanType::StringRef, span);
        builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(place), span));
        return Ok(read_place(reference, false, span));
    }
    if is_element_place(builder, &read.place)? {
        return borrow_element_text(builder, value, read.fact, span);
    }
    let place = scalar_place(builder, &read.place, span)?;
    match read.fact {
        OwnershipFact::MutBorrow => return Err("unsupported Body IR mutable String borrow".into()),
        OwnershipFact::Unknown => return Err("unsupported Body IR unknown String ownership".into()),
        _ => {}
    }
    match place_type(&builder.locals, &place) {
        PlanType::StringRef => Ok(read_place(place, false, span)),
        PlanType::StrRef => {
            let owner = owned_slice_text(builder, read_place(place, false, span), span);
            builder.owned.push(owner.local);
            Ok(borrow_string(builder, owner, span))
        }
        PlanType::FrozenStr => {
            let owner = display_plan_operand(builder, read_place(place, false, span), span)?;
            Ok(borrow_string(builder, owner, span))
        }
        PlanType::String => Ok(borrow_string(builder, place, span)),
        ty => Err(unsupported_text_operand(builder, &ty, span)),
    }
}

/// Identify the checked text boundary's actual carrier and source site when it cannot be borrowed as text.
fn unsupported_text_operand(builder: &Builder, ty: &PlanType, span: &SourceSpan) -> String {
    let site = format!("{}:{}:{} in {}", builder.file, span.line, span.column, builder.name);
    format!("unsupported Body IR non-string text operand at {site}; type {ty:?}")
}

/// Borrow indexed text in place for a shared read; owned reads retain their independent materialization.
fn borrow_element_text(
    builder: &mut Builder,
    value: &SourceOperand,
    fact: OwnershipFact,
    span: &SourceSpan,
) -> Result<Operand, String> {
    match fact {
        OwnershipFact::MutBorrow => return Err("unsupported Body IR mutable String borrow".into()),
        OwnershipFact::Unknown => return Err("unsupported Body IR unknown String ownership".into()),
        _ => {}
    }
    if fact == OwnershipFact::Borrow
        && let SourceOperand::Place(read) = value
        && let Some(owner) = shared_element_place(builder, &read.place, span)?
        && place_type(&builder.locals, &owner) == PlanType::String
    {
        return Ok(borrow_string(builder, owner, span));
    }
    let operand = value_operand(builder, value, span)?;
    if value_type(builder, &operand)? == PlanType::FrozenStr {
        // Match direct frozen text: copy its static wrapper, then provide the owned text view this helper expects.
        let owner = display_plan_operand(builder, operand, span)?;
        return Ok(borrow_string(builder, owner, span));
    }
    if let OperandKind::Move(owner) = &operand.kind {
        let ty = place_type(&builder.locals, owner);
        if ty != PlanType::String {
            return Err(unsupported_text_operand(builder, &ty, span));
        }
        return Ok(borrow_string(builder, owner.clone(), span));
    }
    let ty = value_type(builder, &operand)?;
    Err(unsupported_text_operand(builder, &ty, span))
}

/// A shared reference to one owned string place.
fn borrow_string(builder: &mut Builder, owner: incan_mir_plan_native::Place, span: &SourceSpan) -> Operand {
    let reference = temporary(builder, PlanType::StringRef, span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(owner), span));
    read_place(reference, false, span)
}

/// Bind the retained string-helper identity to shared operands and transfer its owned text or list result.
pub fn lower_text_helper(
    builder: &mut Builder,
    name: &str,
    result: PlanType,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() as i64 != text_helper_arity(name) {
        return Err("invalid Body IR string helper arity".into());
    }
    let incoming = builder.owned.clone();
    let mut parameters = Vec::with_capacity(arguments.len());
    let mut operands = Vec::with_capacity(arguments.len());
    for argument in arguments {
        parameters.push(PlanType::StringRef);
        match argument.as_one() {
            Some(value) => operands.push(borrow_text_operand(builder, value, span)?),
            None => return Err("unsupported Body IR spread string helper argument".into()),
        }
    }
    let text = emit_text_helper(builder, name, result.clone(), parameters, operands, span);
    // Literal arguments belong to this call, including when its source branch is skipped elsewhere.
    drop_borrowed_text_temporaries(builder, &incoming, text.local, span);
    match destination {
        Some(place) => {
            let output = scalar_place(builder, place, span)?;
            forget_owned(builder, text.local);
            drop_owned(builder, output.local, span);
            let owns_result = result == PlanType::String || is_list(&result);
            let local = output.local;
            builder.emit(plan_assign(
                output,
                RvalueKind::Use(read_place(text, owns_result, span)),
                span,
            ));
            if owns_result {
                builder.owned.push(local);
            }
        }
        None => drop_owned(builder, text.local, span),
    }
    Ok(())
}

/// Lend exact vector storage through its native slice conversion.
///
/// Nested element layouts and checked exclusive-access authority remain unchanged. A sliced sequence, `raw[start:end]`,
/// is the fresh owner slicing makes, lent as that owner's slice; an exclusive loan of one is refused, since a change to
/// it could never reach the sliced sequence (#3036).
pub fn borrow_slice_operand(
    builder: &mut Builder,
    value: &SourceOperand,
    elements: &[PlanType],
    mutable: bool,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if elements.len() != 1 {
        return Err("invalid Body IR native slice element arity".into());
    }
    let reference_type = if mutable {
        PlanType::SliceMutRef(elements.to_vec())
    } else {
        PlanType::SliceRef(elements.to_vec())
    };
    if let SourceOperand::Place(read) = value {
        if has_slice_projection(&read.place) {
            if mutable {
                return Err("unsupported Body IR mutable borrow through a list slice".into());
            }
            let sliced = checked_list_argument(builder, value, false, span)?;
            return list_slice_view(builder, sliced, elements, false, span);
        }
        let place = scalar_place(builder, &read.place, span)?;
        let actual = place_type(&builder.locals, &place);
        if let Some(view) = borrow_array_slice(builder, &place, elements, mutable, &read.fact, span)? {
            return Ok(view);
        }
        if actual == PlanType::SliceRef(elements.to_vec()) && !mutable {
            return Ok(read_place(place, false, span));
        }
        if actual == PlanType::SliceMutRef(elements.to_vec()) {
            if mutable && read.fact != OwnershipFact::MutBorrow {
                return Err("unsupported Body IR mutable slice without exclusive authority".into());
            }
            let slice = PlanType::NativeSlice(elements.to_vec());
            let referent = Place {
                local: place.local,
                projection: Projection::Deref(slice),
                span: span.clone(),
            };
            let output = temporary(builder, reference_type, span);
            let operation = if mutable {
                RvalueKind::MutBorrow(referent)
            } else {
                RvalueKind::Borrow(referent)
            };
            builder.emit(plan_assign(output.clone(), operation, span));
            return Ok(read_place(output, mutable, span));
        }
    }
    let reference = match value {
        SourceOperand::Constant(constant) => {
            if mutable {
                return Err("unsupported Body IR mutable slice literal".into());
            }
            let owner = bytes_literal(builder, constant, span)?;
            list_borrow(builder, owner, false, span)?
        }
        SourceOperand::Place(_) => checked_list_argument(builder, value, mutable, span)?,
    };
    list_slice_view(builder, reference, elements, mutable, span)
}

/// Convert an already selected vector loan to its exact slice view without reading or copying its elements.
pub fn list_slice_view(
    builder: &mut Builder,
    reference: Operand,
    elements: &[PlanType],
    mutable: bool,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if elements.len() != 1 {
        return Err("invalid Body IR native slice element arity".into());
    }
    let (reference_type, method) = if mutable {
        (PlanType::SliceMutRef(elements.to_vec()), "core::convert::AsMut::as_mut")
    } else {
        (PlanType::SliceRef(elements.to_vec()), "core::convert::AsRef::as_ref")
    };
    let parameter = value_type(builder, &reference)?;
    let (element, depth) = require_list_shape(&parameter, "unsupported Body IR native slice owner")?;
    if depth < 1 || list_element(&element, depth) != elements[0] {
        return Err("unsupported Body IR native slice element mismatch".into());
    }
    let output = temporary(builder, reference_type.clone(), span);
    let arguments = vec![PlanType::List(element, depth), PlanType::NativeSlice(elements.to_vec())];
    builder.externals.push(ExternalFunction {
        path: method.to_string(),
        type_arguments: arguments.clone(),
        parameters: vec![parameter],
        return_type: reference_type,
        span: span.clone(),
    });
    terminate_call(
        builder,
        CalleeKind::Instantiated(method.to_string(), arguments),
        vec![reference],
        output.clone(),
        span,
    );
    Ok(read_place(output, mutable, span))
}

/// Whether `referent` is the shared nominal reference to a value of type `actual`.
fn lends_shared_referent(actual: &PlanType, referent: &PlanType) -> bool {
    matches!(actual, PlanType::Model(..) | PlanType::Enum(..))
        && shared_receiver_type(actual.clone()).is_ok_and(|reference| reference == *referent)
}

/// Borrow checked scalar or reference-value storage in place; only shared literal loans may create an owner.
pub fn borrow_scalar_operand(
    builder: &mut Builder,
    value: &SourceOperand,
    elements: &[PlanType],
    mutable: bool,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if elements.len() != 1 {
        return Err("invalid Body IR scalar reference arity".into());
    }
    let referent = &elements[0];
    let owner = match value {
        SourceOperand::Place(read) => {
            if mutable && read.fact != OwnershipFact::MutBorrow {
                return Err("unsupported Body IR scalar reference without an exclusive borrow fact".into());
            }
            let mut owner = scalar_borrow_owner(builder, read, referent, mutable, span)?;
            // A shared reference read through its referent, as a receiver is, is lent as a fresh loan of that referent:
            // `Read for &UnixStream` takes `&mut &UnixStream`, and the loan's own identity is unobservable.
            if mutable && lends_shared_referent(&place_type(&builder.locals, &owner), referent) {
                let loan = temporary(builder, referent.clone(), span);
                builder.emit(plan_assign(loan.clone(), RvalueKind::Borrow(owner), span));
                owner = loan;
            }
            let actual = place_type(&builder.locals, &owner);
            if let PlanType::NativeTraitObject(..) = referent
                && actual != *referent
            {
                return borrow_as_trait_object(builder, owner, &actual, referent, mutable, span);
            }
            // An exact reference-valued referent needs another loan; an additional existing loan is reborrowed.
            if actual != *referent {
                match &actual {
                    PlanType::ScalarRef(inner) => {
                        if mutable || inner.as_slice() != elements {
                            return Err("unsupported Body IR scalar reborrow contract".into());
                        }
                        return Ok(read_place(owner, false, span));
                    }
                    PlanType::ScalarMutRef(inner) => {
                        if inner.as_slice() != elements {
                            return Err("unsupported Body IR scalar reborrow referent".into());
                        }
                        owner = Place {
                            local: owner.local,
                            projection: Projection::Deref(referent.clone()),
                            span: span.clone(),
                        };
                    }
                    _ => {
                        return Err(format!(
                            "unsupported Body IR scalar borrow of another type: {actual:?} for a {referent:?} referent"
                        ));
                    }
                }
            }
            owner
        }
        SourceOperand::Constant(_) => {
            if mutable {
                return Err("unsupported Body IR mutable scalar constant borrow".into());
            }
            let operand = scalar_operand(builder, value, span)?;
            if value_type(builder, &operand)? != *referent {
                return Err("unsupported Body IR scalar literal borrow of another type".into());
            }
            let owner = temporary(builder, referent.clone(), span);
            builder.emit(plan_assign(owner.clone(), RvalueKind::Use(operand), span));
            owner
        }
    };
    let (reference, operation) = if mutable {
        (PlanType::ScalarMutRef(elements.to_vec()), RvalueKind::MutBorrow(owner))
    } else {
        (PlanType::ScalarRef(elements.to_vec()), RvalueKind::Borrow(owner))
    };
    let output = temporary(builder, reference, span);
    builder.emit(plan_assign(output.clone(), operation, span));
    Ok(read_place(output, mutable, span))
}

/// Lend a concrete value as a Rust trait object, keeping the value in place for the loan.
///
/// The loan borrows the value as its own type first, then converts that reference to the object reference. rustc
/// proves the value implements every bound of the object before it builds the conversion.
fn borrow_as_trait_object(
    builder: &mut Builder,
    owner: Place,
    actual: &PlanType,
    referent: &PlanType,
    mutable: bool,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let (source, target, operation) = if mutable {
        (
            mutable_receiver_type(actual.clone())?,
            PlanType::ScalarMutRef(vec![referent.clone()]),
            RvalueKind::MutBorrow(owner),
        )
    } else {
        (
            shared_receiver_type(actual.clone())?,
            PlanType::ScalarRef(vec![referent.clone()]),
            RvalueKind::Borrow(owner),
        )
    };
    let loan = temporary(builder, source.clone(), span);
    builder.emit(plan_assign(loan.clone(), operation, span));
    let output = temporary(builder, target.clone(), span);
    let unsized_loan = RvalueKind::UnsizeTraitObject(read_place(loan, mutable, span), source, target);
    builder.emit(plan_assign(output.clone(), unsized_loan, span));
    Ok(read_place(output, mutable, span))
}

/// Retain a checked shared reference as the borrowed value when the native parameter expects that reference.
fn scalar_borrow_owner(
    builder: &mut Builder,
    read: &PlaceOperand,
    referent: &PlanType,
    mutable: bool,
    span: &SourceSpan,
) -> Result<Place, String> {
    if !mutable && let Some(value) = reference_value_read(builder, &read.place, read.fact, span)? {
        match value.kind {
            OperandKind::Copy(place) | OperandKind::Move(place) => {
                if place_type(&builder.locals, &place) == *referent {
                    return Ok(place);
                }
            }
            _ => return Err("invalid Body IR reference read without a place".into()),
        }
    }
    scalar_place(builder, &read.place, span)
}

/// Render the receiver's Display form, as a derived or builtin Display's `__str__` does.
///
/// A number or a bool is copied, though the method call borrows it; any other receiver, a model with a checked derived
/// Display included, displays as `str()` displays it.
pub fn lower_display_str(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 1 {
        return Err("invalid Body IR Display __str__ arguments".into());
    }
    let Some(value) = arguments[0].as_one() else {
        return Err("unsupported Body IR Display __str__ spread receiver".into());
    };
    let text = display_receiver(builder, value, span)?;
    assign_text_result(builder, text, destination, span)
}

/// Select the display rendering for one borrowed receiver.
fn display_receiver(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Place, String> {
    if let SourceOperand::Place(read) = value {
        let place = scalar_place(builder, &read.place, span)?;
        let ty = place_type(&builder.locals, &place);
        if numeric_carrier(&ty) || ty == PlanType::Bool {
            let copied = Operand {
                kind: OperandKind::Copy(place),
                span: span.clone(),
            };
            return display_plan_operand(builder, copied, span);
        }
    }
    display_operand(builder, value, span)
}
