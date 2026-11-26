//! Checked flat-tuple zip polling and indexed builtin enumeration layouts.

use incan_mir_plan_native::{
    CalleeKind, ExternalFunction, ListLeaf, Operand, Place, PlanType, Projection, RvalueKind, SourceSpan,
    TerminatorKind, TupleElement, assign as plan_assign, tuple_element_type,
};
use incan_semantics_core::IncanType;

use crate::builtins::list_call;
use crate::frame::{Builder, drop_owned, exhaust_loop, read_place, temporary, terminate_call};
use crate::tuples::tuple_element;

/// Recognize a zip owner or its retained poll reference.
pub fn zip_shape(ty: &PlanType) -> Option<(TupleElement, TupleElement)> {
    match ty {
        PlanType::ZipIterator(left, right)
        | PlanType::ZipIteratorRef(left, right)
        | PlanType::ZipIteratorMutRef(left, right) => Some((left.clone(), right.clone())),
        _ => None,
    }
}

/// Borrow a snapshot or reborrow its exclusive loop handle without duplicating reference ownership.
pub fn zip_reference(
    builder: &mut Builder,
    source: Place,
    mutable: bool,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let ty = builder.locals[source.local as usize].ty.clone();
    let Some((left, right)) = zip_shape(&ty) else {
        return Err("unsupported Body IR zip reference receiver".into());
    };
    let mut root = source;
    let owner = PlanType::ZipIterator(left.clone(), right.clone());
    if ty != owner {
        if mutable && ty == PlanType::ZipIteratorRef(left.clone(), right.clone()) {
            return Err("unsupported Body IR mutable zip through shared reference".into());
        }
        root.projection = Projection::Deref(owner);
    }
    let (reference_type, operation) = if mutable {
        (PlanType::ZipIteratorMutRef(left, right), RvalueKind::MutBorrow(root))
    } else {
        (PlanType::ZipIteratorRef(left, right), RvalueKind::Borrow(root))
    };
    let reference = temporary(builder, reference_type, span);
    builder.emit(plan_assign(reference.clone(), operation, span));
    Ok(read_place(reference, mutable, span))
}

/// Keep the exact two runtime type arguments and signature beside each canonical call.
#[allow(clippy::too_many_arguments)] // The Incan helper's nine parts of one runtime call.
pub fn zip_call(
    builder: &mut Builder,
    name: &str,
    left: &TupleElement,
    right: &TupleElement,
    parameters: Vec<PlanType>,
    result: PlanType,
    arguments: Vec<Operand>,
    output: Place,
    span: &SourceSpan,
) {
    let types = vec![tuple_element_type(left), tuple_element_type(right)];
    let path = format!("incan_native_runtime::{name}");
    builder.externals.push(ExternalFunction {
        path: path.clone(),
        type_arguments: types.clone(),
        parameters,
        return_type: result,
        span: span.clone(),
    });
    terminate_call(builder, CalleeKind::Instantiated(path, types), arguments, output, span);
}

/// Snapshot both lists and their cursors for a checked value-preserving read.
pub fn clone_zip(builder: &mut Builder, source: Place, span: &SourceSpan) -> Result<Operand, String> {
    let Some((left, right)) = zip_shape(&builder.locals[source.local as usize].ty) else {
        return Err("unsupported Body IR zip clone receiver".into());
    };
    let reference = zip_reference(builder, source, false, span)?;
    let ty = PlanType::ZipIterator(left.clone(), right.clone());
    let output = temporary(builder, ty.clone(), span);
    zip_call(
        builder,
        "clone_zip",
        &left,
        &right,
        vec![PlanType::ZipIteratorRef(left.clone(), right.clone())],
        ty,
        vec![reference],
        output.clone(),
        span,
    );
    builder.owned.push(output.local);
    Ok(read_place(output, true, span))
}

/// Borrow the checked consumed zip state for one normalized loop without inventing another value read.
pub fn initialize_zip_iterator(
    builder: &mut Builder,
    destination: Place,
    source: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let reference = zip_reference(builder, source, true, span)?;
    builder.emit(plan_assign(destination, RvalueKind::Use(reference), span));
    Ok(())
}

/// Poll one pair and branch to the loop exit on exhaustion, disposing of the temporary buffer on both edges.
pub fn zip_next(builder: &mut Builder, receiver: Place, output: &Place, span: &SourceSpan) -> Result<(), String> {
    if builder.loops.is_empty() {
        return Err("unsupported Body IR zip polling outside loop".into());
    }
    let Some((left, right)) = zip_shape(&builder.locals[receiver.local as usize].ty) else {
        return Err("unsupported Body IR zip poll receiver".into());
    };
    let element = PlanType::Tuple(vec![left.clone(), right.clone()]);
    if builder.locals[output.local as usize].ty != element {
        return Err("unsupported Body IR zip IterNext destination".into());
    }
    let leaf = ListLeaf::Tuple(vec![left.clone(), right.clone()]);
    let storage_type = PlanType::List(leaf.clone(), 1);
    let buffer_type = PlanType::ListMutRef(leaf, 1);
    let storage = temporary(builder, storage_type.clone(), span);
    list_call(
        builder,
        "empty_list",
        element.clone(),
        Vec::new(),
        storage_type,
        Vec::new(),
        storage.clone(),
        span,
    );
    let reference = temporary(builder, buffer_type.clone(), span);
    builder.emit(plan_assign(
        reference.clone(),
        RvalueKind::MutBorrow(storage.clone()),
        span,
    ));
    let source = zip_reference(builder, receiver, true, span)?;
    let available = temporary(builder, PlanType::Bool, span);
    zip_call(
        builder,
        "poll_zip",
        &left,
        &right,
        vec![
            PlanType::ZipIteratorMutRef(left.clone(), right.clone()),
            buffer_type.clone(),
        ],
        PlanType::Bool,
        vec![source, read_place(reference, true, span)],
        available.clone(),
        span,
    );
    let exhausted = builder.new_block(span);
    let selected = builder.new_block(span);
    // The selected branch resumes from the owners live before the exhausted branch drops the poll buffer.
    let owners = builder.owned.clone();
    builder.finish(
        TerminatorKind::SwitchBool(read_place(available, false, span), exhausted, selected),
        span,
    );
    builder.select(exhausted);
    drop_owned(builder, storage.local, span);
    exhaust_loop(builder, span);
    builder.select(selected);
    builder.owned = owners;
    drop_owned(builder, output.local, span);
    let final_reference = temporary(builder, buffer_type.clone(), span);
    builder.emit(plan_assign(
        final_reference.clone(),
        RvalueKind::MutBorrow(storage.clone()),
        span,
    ));
    list_call(
        builder,
        "pop_list",
        element.clone(),
        vec![buffer_type],
        element,
        vec![read_place(final_reference, true, span)],
        output.clone(),
        span,
    );
    drop_owned(builder, storage.local, span);
    Ok(())
}

/// Retain the checked item's full layout before constructing the builtin's indexed tuple, including nested lists,
/// tuples and carriers.
pub fn enumerated_element(ty: &PlanType) -> Result<TupleElement, String> {
    tuple_element(ty, &IncanType::Unknown)
}
