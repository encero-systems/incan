//! Hashed collection layouts, primitive leaves and their runtime wrappers.

use incan_lang::lang::surface::dict_methods::DictMethodId;
use incan_mir_plan_native::{
    CalleeKind, ExternalFunction, ListLeaf, Operand, OperandKind, Place, PlanType, Projection, RvalueKind, SourceSpan,
    TupleElement, assign as plan_assign, list_leaf_type,
};
use incan_semantics_core::body_ir::{ArgumentElement, Operand as SourceOperand, OwnershipFact};
use incan_semantics_core::{IncanPrimitiveType, IncanType, NumericTypeId};

use crate::frame::{Builder, drop_owned, forget_owned, place_type, read_place, temporary, terminate_call, value_type};
use crate::models::runtime_storage_marker;
use crate::text_plan::clone_text_reference;
use crate::tuples::{generic_tuple_type, tuple_type};

/// Admit primitive leaves, owned byte buffers and flat scalar tuple keys; floating point keys lack Eq and Hash.
pub fn primitive_leaf(ty: &IncanType, hashed: bool) -> Result<ListLeaf, String> {
    if let Some(PlanType::Decimal) = runtime_storage_marker(ty) {
        return Ok(ListLeaf::Decimal);
    }
    if native_byte_buffer(ty) {
        return Ok(ListLeaf::List(vec![ListLeaf::U8], 1));
    }
    match ty {
        IncanType::Decimal { .. } => return Ok(ListLeaf::Decimal),
        IncanType::Primitive(primitive) => match primitive {
            IncanPrimitiveType::Int => return Ok(ListLeaf::Int),
            IncanPrimitiveType::Bool => return Ok(ListLeaf::Bool),
            IncanPrimitiveType::Str => return Ok(ListLeaf::Str),
            IncanPrimitiveType::FrozenStr => return Ok(ListLeaf::FrozenStr),
            IncanPrimitiveType::Bytes => return Ok(ListLeaf::List(vec![ListLeaf::U8], 1)),
            IncanPrimitiveType::Unit => return Ok(ListLeaf::Unit),
            IncanPrimitiveType::Float if !hashed => return Ok(ListLeaf::Float),
            _ => {}
        },
        IncanType::Tuple(elements) => {
            if hashed {
                return hashed_tuple_leaf(&tuple_type(elements)?);
            }
        }
        IncanType::Generic { .. } => {
            if hashed && let Ok(tuple_ty) = generic_tuple_type(ty) {
                return hashed_tuple_leaf(&tuple_ty);
            }
        }
        _ => {}
    }
    Err(format!("unsupported Body IR collection leaf {ty}"))
}

/// The byte buffer's native storage after a governed argument preserves `Vec[u8]` rather than bytes.
fn native_byte_buffer(ty: &IncanType) -> bool {
    match ty {
        IncanType::Generic { base, args } if base == "List" && args.len() == 1 => {
            matches!(
                args[0],
                IncanType::Primitive(IncanPrimitiveType::Numeric(NumericTypeId::U8))
            )
        }
        _ => false,
    }
}

/// Keep tuple keys flat and exclude floats before selecting the runtime's Eq and Hash bounds.
fn hashed_tuple_leaf(ty: &PlanType) -> Result<ListLeaf, String> {
    match ty {
        PlanType::Tuple(elements) => {
            if elements.contains(&TupleElement::Float) {
                return Err("unsupported Body IR floating-point tuple hash key".into());
            }
            Ok(ListLeaf::Tuple(elements.clone()))
        }
        _ => Err("unsupported Body IR hash tuple representation".into()),
    }
}

/// The exact admitted leaf layout without numeric type tags.
pub fn leaf_type(leaf: &ListLeaf) -> PlanType {
    list_leaf_type(leaf)
}

/// The owner representation behind any checked collection borrow.
pub fn collection_owned(ty: &PlanType) -> PlanType {
    match ty {
        PlanType::SetRef(leaf) | PlanType::SetMutRef(leaf) => PlanType::Set(leaf.clone()),
        PlanType::DictRef(key, value) | PlanType::DictMutRef(key, value) => PlanType::Dict(key.clone(), value.clone()),
        _ => ty.clone(),
    }
}

/// The hashed collection representations, independently of lists.
pub fn is_collection(ty: &PlanType) -> bool {
    matches!(collection_owned(ty), PlanType::Set(_) | PlanType::Dict(..))
}

/// Keep source parameter storage caller-owned with its checked exclusivity.
pub fn collection_reference(ty: &PlanType, mutable: bool) -> PlanType {
    match collection_owned(ty) {
        PlanType::Set(leaf) => {
            if mutable {
                PlanType::SetMutRef(leaf)
            } else {
                PlanType::SetRef(leaf)
            }
        }
        PlanType::Dict(key, value) => {
            if mutable {
                PlanType::DictMutRef(key, value)
            } else {
                PlanType::DictRef(key, value)
            }
        }
        _ => ty.clone(),
    }
}

/// Exactly the runtime's generic type arguments, including dictionary values.
pub fn collection_arguments(ty: &PlanType) -> Vec<PlanType> {
    match collection_owned(ty) {
        PlanType::Set(leaf) => vec![leaf_type(&leaf)],
        PlanType::Dict(key, value) => vec![leaf_type(&key), leaf_type(&value)],
        _ => Vec::new(),
    }
}

/// A typed runtime wrapper, only after collection layout admission.
pub fn collection_name(ty: &PlanType, operation: &str) -> Result<String, String> {
    match collection_owned(ty) {
        PlanType::Set(_) => Ok(format!("{operation}_set")),
        PlanType::Dict(..) => Ok(format!("{operation}_dict")),
        _ => Err("unsupported Body IR noncollection runtime operation".into()),
    }
}

/// Owned lists, sets and dictionaries belong in the frame's drop ledger.
pub fn is_owned_collection(ty: &PlanType) -> bool {
    match ty {
        PlanType::List(..) => true,
        _ => is_collection(ty) && collection_owned(ty) == *ty,
    }
}

/// Retain hashed runtime signatures with exactly their checked generic instantiation.
#[allow(clippy::too_many_arguments)] // The Incan helper's eight parts of one runtime call.
pub fn collection_call(
    builder: &mut Builder,
    ty: &PlanType,
    operation: &str,
    parameters: Vec<PlanType>,
    result: PlanType,
    operands: Vec<Operand>,
    destination: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    if parameters.len() != operands.len() {
        return Err("unsupported Body IR collection runtime arity".into());
    }
    refuse_undisplayed_key(ty, operation)?;
    let mut admitted = Vec::with_capacity(operands.len());
    for (operand, parameter) in operands.into_iter().zip(&parameters) {
        match owned_runtime_operand(builder, operand, parameter, span)? {
            Some(operand) => admitted.push(operand),
            None => return Err("unsupported Body IR collection runtime argument coercion".into()),
        }
    }
    let owned_result = result == PlanType::String || is_owned_collection(&result);
    let local = destination.local;
    let callee = declare_collection_runtime(builder, ty, operation, parameters, result, span)?;
    terminate_call(builder, callee, admitted, destination, span);
    if owned_result {
        builder.owned.push(local);
    }
    Ok(())
}

/// Refuse a keyed dictionary lookup whose missing-key diagnostic cannot display its key.
pub fn refuse_undisplayed_key(ty: &PlanType, operation: &str) -> Result<(), String> {
    let kind = undisplayed_key(ty, operation);
    if kind.is_empty() {
        return Ok(());
    }
    Err(format!(
        "unsupported Body IR keyed lookup in a dictionary with {kind} keys"
    ))
}

/// Declare one hashed runtime operation with exactly its checked generic instantiation, and name it as a callee.
pub fn declare_collection_runtime(
    builder: &mut Builder,
    ty: &PlanType,
    operation: &str,
    parameters: Vec<PlanType>,
    result: PlanType,
    span: &SourceSpan,
) -> Result<CalleeKind, String> {
    let arguments = collection_arguments(ty);
    let path = format!("incan_native_runtime::{}", collection_name(ty, operation)?);
    builder.externals.push(ExternalFunction {
        path: path.clone(),
        type_arguments: arguments.clone(),
        parameters,
        return_type: result,
        span: span.clone(),
    });
    Ok(CalleeKind::Instantiated(path, arguments))
}

/// Pass one evaluated value at its runtime carrier, or nothing when the carriers differ. Text an inferred-shared
/// parameter lends reaches an owned runtime slot, such as a dictionary default or a stored entry, as its shared
/// reference; that runtime call is the owned use, so it receives an independent copy.
pub fn owned_runtime_operand(
    builder: &mut Builder,
    operand: Operand,
    parameter: &PlanType,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    let actual = value_type(builder, &operand)?;
    if actual == *parameter {
        return Ok(Some(operand));
    }
    if actual == PlanType::StringRef && *parameter == PlanType::String {
        return Ok(Some(read_place(
            clone_text_reference(builder, operand, span)?,
            true,
            span,
        )));
    }
    Ok(None)
}

/// Name the key kind, `model` or `enum`, when a runtime dictionary operation renders its key with Display, which
/// neither implements; otherwise nothing.
///
/// Legacy's keyed lookups share that bound, so such a lookup on a model- or enum-keyed dictionary does not build there
/// either.
fn undisplayed_key(ty: &PlanType, operation: &str) -> &'static str {
    if !["get", "borrow_mut", "optional", "values", "default", "extend"].contains(&operation) {
        return "";
    }
    match collection_owned(ty) {
        PlanType::Dict(ListLeaf::Model(..), _) => "model",
        PlanType::Dict(ListLeaf::Enum(..), _) => "enum",
        _ => "",
    }
}

/// Borrow owners, a model's collection field among them, and reborrow parameters, never copying an exclusive reference.
pub fn borrow_collection(
    builder: &mut Builder,
    place: &Place,
    mutable: bool,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let ty = place_type(&builder.locals, place);
    let mut source = place.clone();
    let owned = collection_owned(&ty);
    if owned != ty {
        if mutable && ty != collection_reference(&ty, true) {
            return Err("unsupported Body IR mutable access through shared collection parameter".into());
        }
        source.projection = Projection::Deref(owned);
    }
    let reference = temporary(builder, collection_reference(&ty, mutable), span);
    let operation = if mutable {
        RvalueKind::MutBorrow(source)
    } else {
        RvalueKind::Borrow(source)
    };
    builder.emit(plan_assign(reference.clone(), operation, span));
    Ok(read_place(reference, mutable, span))
}

/// Insert already evaluated operands through a short exclusive borrow of the selected collection storage.
pub fn insert_collection(
    builder: &mut Builder,
    receiver: &Place,
    values: Vec<Operand>,
    span: &SourceSpan,
) -> Result<(), String> {
    let ty = place_type(&builder.locals, receiver);
    let types = collection_arguments(&ty);
    if values.len() != types.len() {
        return Err("unsupported Body IR collection insert arity".into());
    }
    let mut admitted = Vec::with_capacity(values.len());
    for (value, parameter) in values.into_iter().zip(&types) {
        match owned_runtime_operand(builder, value, parameter, span)? {
            Some(operand) => admitted.push(operand),
            None => return Err("unsupported Body IR collection element coercion".into()),
        }
    }
    let reference = borrow_collection(builder, receiver, true, span)?;
    let mut parameters = vec![collection_reference(&ty, true)];
    parameters.extend(types);
    let mut operands = vec![reference];
    operands.extend(admitted);
    let unit = temporary(builder, PlanType::Unit, span);
    collection_call(builder, &ty, "insert", parameters, PlanType::Unit, operands, unit, span)
}

/// Allocate a typed owner for a checked literal without overwriting a live destination.
pub fn begin_collection(builder: &mut Builder, destination: &Place, span: &SourceSpan) -> Result<Place, String> {
    let ty = builder.locals[destination.local as usize].ty.clone();
    if ty != collection_owned(&ty) {
        return Err("unsupported Body IR whole borrowed collection assignment".into());
    }
    let output = temporary(builder, ty.clone(), span);
    collection_call(
        builder,
        &ty,
        "empty",
        Vec::new(),
        ty.clone(),
        Vec::new(),
        output.clone(),
        span,
    )?;
    Ok(output)
}

/// Replace a collection binding only after its complete value is initialized.
pub fn finish_collection(builder: &mut Builder, destination: &Place, output: Place, span: &SourceSpan) {
    drop_owned(builder, destination.local, span);
    forget_owned(builder, output.local);
    builder.emit(plan_assign(
        destination.clone(),
        RvalueKind::Use(read_place(output, true, span)),
        span,
    ));
    builder.owned.push(destination.local);
}

/// Transfer an already materialized owned collection, preserving the drop ledger.
pub fn transfer_collection(
    builder: &mut Builder,
    destination: &Place,
    argument: Operand,
    span: &SourceSpan,
) -> Result<(), String> {
    let ty = &builder.locals[destination.local as usize].ty;
    if *ty != collection_owned(ty) {
        return Err("unsupported Body IR whole borrowed collection assignment".into());
    }
    match &argument.kind {
        OperandKind::Move(place) => forget_owned(builder, place.local),
        _ => return Err("unsupported Body IR collection transfer without owned value".into()),
    }
    drop_owned(builder, destination.local, span);
    builder.emit(plan_assign(destination.clone(), RvalueKind::Use(argument), span));
    builder.owned.push(destination.local);
    Ok(())
}

/// Project one typed dictionary operation onto its native runtime signature.
pub fn dictionary_method_signature(
    ty: &PlanType,
    method: DictMethodId,
    arity: i64,
) -> Result<(String, Vec<PlanType>, PlanType), String> {
    let PlanType::Dict(key, value) = collection_owned(ty) else {
        return Err(format!(
            "unsupported Body IR dictionary Method {method:?} on non-dictionary"
        ));
    };
    if method == DictMethodId::Get {
        if arity != 3 {
            return Err(format!("unsupported Body IR dictionary method arity {method:?}"));
        }
        return Ok(("default".into(), collection_arguments(ty), leaf_type(&value)));
    }
    if arity != 1 {
        return Err(format!("unsupported Body IR dictionary method arity {method:?}"));
    }
    match method {
        DictMethodId::Keys => Ok(("keys".into(), Vec::new(), PlanType::List(key, 1))),
        DictMethodId::Values => Ok(("values".into(), Vec::new(), PlanType::List(value, 1))),
        _ => Err(format!("unsupported Body IR dictionary Method {method:?}")),
    }
}

/// Require the frontend's exclusive-access decision before mutating a method receiver.
pub fn require_mutable_receiver(arguments: &[ArgumentElement]) -> Result<(), String> {
    let Some(first) = arguments.first() else {
        return Err("unsupported Body IR collection method without receiver".into());
    };
    if let Some(SourceOperand::Place(read)) = first.as_one()
        && read.fact == OwnershipFact::MutBorrow
    {
        return Ok(());
    }
    Err("unsupported Body IR collection receiver without mutable borrow fact".into())
}

/// Splice one checked owner into a set constructor or dictionary literal, then consume that owner.
pub fn extend_collection(
    builder: &mut Builder,
    destination: &Place,
    source: &Operand,
    span: &SourceSpan,
) -> Result<(), String> {
    let OperandKind::Move(source_place) = &source.kind else {
        return Err("unsupported Body IR collection spread without owned source".into());
    };
    let ty = builder.locals[destination.local as usize].ty.clone();
    let source_type = builder.locals[source_place.local as usize].ty.clone();
    let mut operation = "extend";
    let mut source_reference_type = collection_reference(&source_type, false);
    if source_type != collection_owned(&ty) {
        let PlanType::Set(leaf) = &ty else {
            return Err("unsupported Body IR dictionary spread source".into());
        };
        if source_type != PlanType::List(leaf.clone(), 1) {
            return Err("unsupported Body IR set constructor source".into());
        }
        operation = "extend_list";
        source_reference_type = PlanType::ListRef(leaf.clone(), 1);
    }
    let reference = temporary(builder, source_reference_type.clone(), span);
    builder.emit(plan_assign(
        reference.clone(),
        RvalueKind::Borrow(source_place.clone()),
        span,
    ));
    let receiver = borrow_collection(builder, destination, true, span)?;
    let output = temporary(builder, PlanType::Unit, span);
    collection_call(
        builder,
        &ty,
        operation,
        vec![collection_reference(&ty, true), source_reference_type],
        PlanType::Unit,
        vec![receiver, read_place(reference, false, span)],
        output,
        span,
    )?;
    drop_owned(builder, source_place.local, span);
    Ok(())
}
