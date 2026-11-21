//! Consume checker-selected list items through the standard concrete Vec iterator, without Clone or eager polling.

use incan_mir_plan_native::{
    BinaryOp, CalleeKind, Carrier, Constant, ExternalFunction, Operand, Place, PlanType, Projection, RvalueKind,
    SourceSpan, TerminatorKind, assign as plan_assign, carrier_index,
};

use crate::frame::{
    Builder, drop_owned, exhaust_loop, forget_owned, literal, place_type, read_place, temporary, terminate_call,
};
use crate::lists::list_element;

/// Identify owned state; the mutable polling view never enters the drop ledger.
pub fn is_owned_list_iterator(ty: &PlanType) -> bool {
    matches!(ty, PlanType::ListIntoIter(..))
}

/// Call the public native trait with its exact concrete Self and metadata-validated signature.
#[allow(clippy::too_many_arguments)] // One argument per part of the native call; the Incan helper takes the same eight.
fn iterator_call(
    builder: &mut Builder,
    path: &str,
    owner: PlanType,
    parameters: Vec<PlanType>,
    result: PlanType,
    arguments: Vec<Operand>,
    output: Place,
    span: &SourceSpan,
) {
    builder.externals.push(ExternalFunction {
        path: path.to_owned(),
        type_arguments: vec![owner.clone()],
        parameters,
        return_type: result,
        span: span.clone(),
    });
    terminate_call(
        builder,
        CalleeKind::Instantiated(path.to_owned(), vec![owner]),
        arguments,
        output,
        span,
    );
}

/// Move an owned checked list into IntoIter; a caller-owned reference remains refused rather than cloned.
pub fn initialize_owned_list_iterator(
    builder: &mut Builder,
    target: &Place,
    source: &Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let target_type = place_type(&builder.locals, target);
    let PlanType::ListIntoIter(leaf, depth) = &target_type else {
        return Err("unsupported Body IR consuming list iterator destination".into());
    };
    let source_type = PlanType::List(leaf.clone(), *depth);
    if source.projection != Projection::Whole || place_type(&builder.locals, source) != source_type {
        return Err("unsupported Body IR consuming iteration of a borrowed or projected list".into());
    }
    iterator_call(
        builder,
        "core::iter::IntoIterator::into_iter",
        source_type.clone(),
        vec![source_type],
        target_type.clone(),
        vec![read_place(source.clone(), true, span)],
        target.clone(),
        span,
    );
    builder.owned.push(target.local);
    Ok(())
}

/// Use the retained standard Option layout for Iterator::next's checked item type.
fn iterator_poll_type(builder: &Builder, element: &PlanType) -> Result<PlanType, String> {
    for (index, carrier) in builder.intrinsics.iter().enumerate() {
        if carrier.carrier == Carrier::Option
            && carrier.variants.len() == 2
            && carrier.variants[1].fields.len() == 1
            && carrier.variants[1].fields[0] == *element
        {
            return Ok(PlanType::Enum(carrier_index(index as i64), carrier.name.clone()));
        }
    }
    Err("unsupported Body IR consuming iteration without retained Option output".into())
}

/// Move one Some payload into the loop item, or take its existing exit on None; remaining items stay owned.
pub fn owned_list_next(
    builder: &mut Builder,
    receiver: &Place,
    target: &Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let PlanType::ListIntoIter(leaf, depth) = place_type(&builder.locals, receiver) else {
        return Err("unsupported Body IR consuming list poll receiver".into());
    };
    let element = list_element(&leaf, depth);
    if place_type(&builder.locals, target) != element {
        return Err("unsupported Body IR consuming iteration item layout".into());
    }
    let iterator_type = PlanType::ListIntoIter(leaf.clone(), depth);
    let reference_type = PlanType::ListIntoIterMutRef(leaf, depth);
    let reference = temporary(builder, reference_type.clone(), span);
    builder.emit(plan_assign(
        reference.clone(),
        RvalueKind::MutBorrow(receiver.clone()),
        span,
    ));
    let poll_type = iterator_poll_type(builder, &element)?;
    let poll = temporary(builder, poll_type.clone(), span);
    iterator_call(
        builder,
        "core::iter::Iterator::next",
        iterator_type,
        vec![reference_type],
        poll_type,
        vec![read_place(reference, true, span)],
        poll.clone(),
        span,
    );
    builder.owned.push(poll.local);
    let raw = temporary(builder, PlanType::EnumTag, span);
    let tag = temporary(builder, PlanType::Int, span);
    builder.emit(plan_assign(raw.clone(), RvalueKind::Discriminant(poll.clone()), span));
    builder.emit(plan_assign(
        tag.clone(),
        RvalueKind::TagToInt(read_place(raw, false, span)),
        span,
    ));
    let available = temporary(builder, PlanType::Bool, span);
    let one = literal(Constant::Int(1), span);
    builder.emit(plan_assign(
        available.clone(),
        RvalueKind::Binary(BinaryOp::Equal, read_place(tag, false, span), one),
        span,
    ));
    let exhausted = builder.new_block(span);
    let selected = builder.new_block(span);
    // The selected branch resumes from the owners live before the exhausted branch drops its poll.
    let owners = builder.owned.clone();
    builder.finish(
        TerminatorKind::SwitchBool(read_place(available, false, span), exhausted, selected),
        span,
    );
    builder.select(exhausted);
    drop_owned(builder, poll.local, span);
    if builder.loop_owners.last() != Some(&builder.owned) {
        return Err("unsupported Body IR consuming poll with initialized loop-local owners".into());
    }
    exhaust_loop(builder, span);
    builder.select(selected);
    builder.owned = owners;
    drop_owned(builder, target.local, span);
    let payload = Place {
        local: poll.local,
        projection: Projection::VariantField(1, 0, element),
        span: span.clone(),
    };
    builder.emit(plan_assign(
        target.clone(),
        RvalueKind::Use(read_place(payload, true, span)),
        span,
    ));
    // Some's payload has moved; its empty shell cannot be dropped. The iterator stays owned until the common loop exit.
    forget_owned(builder, poll.local);
    builder.owned.push(target.local);
    Ok(())
}
