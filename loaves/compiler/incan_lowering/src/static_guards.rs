//! Reads that run under a guarded static's storage lock (RFC 052) and let only their own result leave it: one field
//! copied out of a model static, or one read-only collection operation on a static's dictionary or set.
//!
//! A read never loads a snapshot of the whole stored value. Its operands are evaluated before the accessor takes the
//! shared guard, as every guarded accessor's are, and its result is an owned local of the reading frame, so a later
//! write of the static cannot change a value already read.

use incan_mir_plan_native::{
    Callee, CalleeKind, ExternalFunction, FieldProjection, Function, Local, Operand, Parameter, Place, PlanType,
    Projection, RvalueKind, SourceSpan, TerminatorKind, Unwind, assign as plan_assign, block as plan_block,
    place as plan_place,
};
use incan_semantics_core::body_ir::{ArgumentElement, Operand as SourceOperand, OwnershipFact, PlaceElem};

use crate::collections::{
    collection_arguments, collection_name, collection_reference, declare_collection_runtime, is_owned_collection,
    owned_runtime_operand, refuse_undisplayed_key,
};
use crate::computed_statics::{has_native_static_cell, initialize_module_statics, is_computed_static, static_owner};
use crate::decls::static_slot;
use crate::frame::{Builder, read_place, terminate_call};
use crate::lists::list_element;
use crate::models::field_path;
use crate::static_storage::{snapshot_owns, static_carrier};

/// How a guarded accessor takes one field's value out of the borrowed static.
pub enum FieldCopy {
    /// A plain value, copied out of its place.
    Copy,
    /// An owned value: the field is borrowed as `reference` and `callee` clones it.
    Clone { reference: PlanType, callee: CalleeKind },
}

/// Whether `ty` is a source enum whose declaration retains no Clone, so no field of it can be cloned out.
pub fn uncloned_enum(builder: &Builder, ty: &PlanType) -> bool {
    let PlanType::Enum(index, _) = ty else {
        return false;
    };
    !usize::try_from(*index)
        .ok()
        .and_then(|slot| builder.enums().get(slot))
        .is_some_and(|declaration| declaration.derives.iter().any(|derive| derive == "Clone"))
}

/// Select how a field of type `result` is copied under the guard, declaring the runtime clone it calls; None for a
/// carrier this route does not copy (an enum without Clone among them), which keeps the snapshot read.
pub fn field_copy(builder: &mut Builder, result: &PlanType, span: &SourceSpan) -> Result<Option<FieldCopy>, String> {
    if !snapshot_owns(result) {
        return Ok(Some(FieldCopy::Copy));
    }
    let (reference, callee) = match result {
        PlanType::String => {
            let path = "incan_native_runtime::clone_text";
            declare(builder, path, Vec::new(), vec![PlanType::StringRef], result, span);
            (PlanType::StringRef, CalleeKind::External(path.into()))
        }
        PlanType::Enum(index, name) => {
            if uncloned_enum(builder, result) {
                return Ok(None);
            }
            let reference = PlanType::EnumRef(*index, name.clone());
            let path = "core::clone::Clone::clone";
            declare(
                builder,
                path,
                vec![result.clone()],
                vec![reference.clone()],
                result,
                span,
            );
            (reference, CalleeKind::Instantiated(path.into(), vec![result.clone()]))
        }
        PlanType::Model(index, name) => (
            PlanType::ModelRef(*index, name.clone()),
            CalleeKind::CloneModel(*index, name.clone()),
        ),
        PlanType::List(leaf, depth) => {
            let reference = PlanType::ListRef(leaf.clone(), *depth);
            let path = "incan_native_runtime::clone_list";
            let element = list_element(leaf, *depth);
            declare(
                builder,
                path,
                vec![element.clone()],
                vec![reference.clone()],
                result,
                span,
            );
            (reference, CalleeKind::Instantiated(path.into(), vec![element]))
        }
        PlanType::Dict(..) | PlanType::Set(..) => {
            let reference = collection_reference(result, false);
            let path = format!("incan_native_runtime::{}", collection_name(result, "clone")?);
            let arguments = collection_arguments(result);
            declare(builder, &path, arguments.clone(), vec![reference.clone()], result, span);
            (reference, CalleeKind::Instantiated(path, arguments))
        }
        _ => return Ok(None),
    };
    Ok(Some(FieldCopy::Clone { reference, callee }))
}

/// Declare one external a guarded accessor calls.
fn declare(
    builder: &mut Builder,
    path: &str,
    type_arguments: Vec<PlanType>,
    parameters: Vec<PlanType>,
    result: &PlanType,
    span: &SourceSpan,
) {
    builder.externals.push(ExternalFunction {
        path: path.into(),
        type_arguments,
        parameters,
        return_type: result.clone(),
        span: span.clone(),
    });
}

/// The accessor that copies the field at `fields` of its borrowed `receiver` while the guard is held.
pub fn field_copier(
    name: String,
    receiver: PlanType,
    result: PlanType,
    fields: Vec<FieldProjection>,
    copy: FieldCopy,
    span: &SourceSpan,
) -> Function {
    let field = Place {
        local: 1,
        projection: Projection::DerefFields(fields),
        span: span.clone(),
    };
    let mut locals = vec![local(&result, span), local(&receiver, span)];
    let blocks = match copy {
        FieldCopy::Clone { reference, callee } => {
            locals.push(local(&reference, span));
            let borrowed = plan_assign(plan_place(2, span), RvalueKind::Borrow(field), span);
            let arguments = vec![read_place(plan_place(2, span), false, span)];
            vec![
                plan_block(vec![borrowed], call(callee, arguments, span), span),
                plan_block(Vec::new(), TerminatorKind::Return, span),
            ]
        }
        FieldCopy::Copy => {
            let copied = plan_assign(
                plan_place(0, span),
                RvalueKind::Use(read_place(field, false, span)),
                span,
            );
            vec![plan_block(vec![copied], TerminatorKind::Return, span)]
        }
    };
    Function::new(
        name,
        vec![parameter("__incan_receiver", &receiver, span)],
        result,
        locals,
        blocks,
        span.clone(),
    )
}

/// A read-only collection call's static receiver: the static's slot, the reference its accessor receives, the field
/// path from the stored value to the collection (empty when the static is the dictionary) and the collection's type.
pub struct GuardedCollection {
    slot: usize,
    root: PlanType,
    fields: Vec<FieldProjection>,
    pub collection: PlanType,
}

/// The guarded static a read-only collection call's receiver names: a dictionary static, or a dictionary or set field
/// of a model static. None for any other receiver, which keeps its ordinary read.
pub fn guarded_collection(
    builder: &Builder,
    arguments: &[ArgumentElement],
) -> Result<Option<GuardedCollection>, String> {
    let Some(SourceOperand::Place(read)) = arguments.first().and_then(ArgumentElement::as_one) else {
        return Ok(None);
    };
    if !matches!(
        read.fact,
        OwnershipFact::Borrow | OwnershipFact::Clone | OwnershipFact::Copy
    ) {
        return Ok(None);
    }
    let Some(global) = read.place.global() else {
        return Ok(None);
    };
    let slot = static_slot(&builder.program.statics, global)?;
    if !has_native_static_cell(&builder.program.statics[slot]) {
        return Ok(None);
    }
    let ty = static_carrier(builder, global)?;
    if read.place.projection.is_empty() {
        // The native storage cell admits a dictionary itself as an accessor's receiver, not a set.
        if !matches!(ty, PlanType::Dict(..)) {
            return Ok(None);
        }
        return Ok(Some(GuardedCollection {
            slot,
            root: collection_reference(&ty, false),
            fields: Vec::new(),
            collection: ty,
        }));
    }
    let PlanType::Model(index, name) = &ty else {
        return Ok(None);
    };
    let steps = &read.place.projection;
    if !steps.iter().all(|step| matches!(step, PlaceElem::Field { .. })) {
        return Ok(None);
    }
    let fields = field_path(builder.tables(), ty.clone(), steps, steps.len())?;
    let Some(last) = fields.last() else {
        return Ok(None);
    };
    if fields.iter().any(|field| field.variant >= 0) || !matches!(last.ty, PlanType::Dict(..) | PlanType::Set(..)) {
        return Ok(None);
    }
    Ok(Some(GuardedCollection {
        slot,
        root: PlanType::ModelRef(*index, name.clone()),
        collection: last.ty.clone(),
        fields,
    }))
}

/// One read-only runtime operation on a guarded collection: its name, the parameters after the collection reference,
/// its result, and the evaluated operands for those parameters.
pub struct GuardedOperation<'a> {
    pub operation: &'a str,
    pub parameters: Vec<PlanType>,
    pub result: PlanType,
    pub operands: Vec<Operand>,
}

/// Call one read-only operation on a guarded static collection, writing its result to `destination`. The operands
/// are already evaluated; the accessor borrows the collection under the shared guard and only the result leaves it.
pub fn guarded_collection_call(
    builder: &mut Builder,
    receiver: &GuardedCollection,
    request: GuardedOperation<'_>,
    destination: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let GuardedOperation {
        operation,
        parameters,
        result,
        operands,
    } = request;
    if parameters.len() != operands.len() {
        return Err("unsupported Body IR guarded collection arity".into());
    }
    refuse_undisplayed_key(&receiver.collection, operation)?;
    let mut admitted = Vec::with_capacity(operands.len());
    for (operand, parameter) in operands.into_iter().zip(&parameters) {
        match owned_runtime_operand(builder, operand, parameter, span)? {
            Some(operand) => admitted.push(operand),
            None => return Err("unsupported Body IR collection runtime argument coercion".into()),
        }
    }
    let reference = collection_reference(&receiver.collection, false);
    let mut declared = vec![reference.clone()];
    declared.extend(parameters.iter().cloned());
    let callee = declare_collection_runtime(builder, &receiver.collection, operation, declared, result.clone(), span)?;
    let name = format!(
        "{}__incan_static_call_{}_{}",
        builder.name,
        receiver.slot,
        builder.synthesized.len()
    );
    let accessor = collection_accessor(&name, receiver, reference, callee, &parameters, &result, span);
    builder.synthesized.push(accessor);
    let declaration = &builder.program.statics[receiver.slot];
    if is_computed_static(declaration) {
        let owner_module = static_owner(declaration);
        initialize_module_statics(builder, &owner_module, span);
    }
    let local = destination.local;
    terminate_call(
        builder,
        CalleeKind::StaticMethod(receiver.slot as i64, name),
        admitted,
        destination,
        span,
    );
    if result == PlanType::String || is_owned_collection(&result) {
        builder.owned.push(local);
    }
    Ok(())
}

/// The accessor of one guarded collection operation: it borrows the collection out of its receiver (or receives the
/// dictionary itself) and calls the runtime operation with the forwarded operands.
fn collection_accessor(
    name: &str,
    receiver: &GuardedCollection,
    reference: PlanType,
    callee: CalleeKind,
    inputs: &[PlanType],
    result: &PlanType,
    span: &SourceSpan,
) -> Function {
    let mut parameters = vec![parameter("__incan_receiver", &receiver.root, span)];
    let mut locals = vec![local(result, span), local(&receiver.root, span)];
    for (index, input) in inputs.iter().enumerate() {
        parameters.push(parameter(&format!("__incan_argument_{index}"), input, span));
        locals.push(local(input, span));
    }
    let mut statements = Vec::new();
    let collection = if receiver.fields.is_empty() {
        plan_place(1, span)
    } else {
        let borrowed = plan_place(locals.len() as i64, span);
        locals.push(local(&reference, span));
        let field = Place {
            local: 1,
            projection: Projection::DerefFields(receiver.fields.clone()),
            span: span.clone(),
        };
        statements.push(plan_assign(borrowed.clone(), RvalueKind::Borrow(field), span));
        borrowed
    };
    let mut arguments = vec![read_place(collection, false, span)];
    for (index, input) in inputs.iter().enumerate() {
        arguments.push(read_place(
            plan_place(index as i64 + 2, span),
            snapshot_owns(input),
            span,
        ));
    }
    Function::new(
        name.to_string(),
        parameters,
        result.clone(),
        locals,
        vec![
            plan_block(statements, call(callee, arguments, span), span),
            plan_block(Vec::new(), TerminatorKind::Return, span),
        ],
        span.clone(),
    )
}

/// An accessor's single call, writing its result to the return slot and continuing at the return block.
fn call(callee: CalleeKind, arguments: Vec<Operand>, span: &SourceSpan) -> TerminatorKind {
    TerminatorKind::Call(
        Callee {
            kind: callee,
            span: span.clone(),
        },
        arguments,
        plan_place(0, span),
        1,
        Unwind::Continue,
    )
}

fn local(ty: &PlanType, span: &SourceSpan) -> Local {
    Local {
        ty: ty.clone(),
        span: span.clone(),
    }
}

fn parameter(name: &str, ty: &PlanType, span: &SourceSpan) -> Parameter {
    Parameter {
        name: name.into(),
        ty: ty.clone(),
        span: span.clone(),
    }
}
