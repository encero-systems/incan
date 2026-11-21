//! Callable signature admission from checked frontend representation facts.

use incan_mir_plan_native::{ListLeaf, PlanType};
use incan_semantics_core::body_ir::Body;
use incan_semantics_core::{IncanCallableParam, IncanCallableParamKind, IncanInputAccess, IncanType};

use crate::collections::{is_collection, is_owned_collection};
use crate::explicit_arguments::direct_view_access;
use crate::generators::{generator_type, is_generator_type};
use crate::layout_types::{
    callable_parameter_type, collection_leaf, context_free_type, mutable_receiver_type, shared_receiver_type,
};
use crate::lists::{is_list, parameter_storage_type};
use crate::native_type_identity::constructor_index;
use crate::numerics::numeric_carrier;
use crate::types::primitive_type;

/// The native signature: checked input order and loans, then the owned return contract.
pub fn function_pointer_type(parameters: &[IncanCallableParam], result: &IncanType) -> Result<PlanType, String> {
    let constructors = constructor_index(&[]);
    let mut signature = Vec::with_capacity(parameters.len() + 1);
    for parameter in parameters {
        let input = if parameter.kind == IncanCallableParamKind::Normal {
            callable_input(&parameter.ty)?
        } else {
            callable_parameter_type(&constructors, &[], &[], &[], parameter, &[])?
        };
        signature.push(packed_input(
            input,
            parameter.kind,
            parameter.is_mut,
            parameter.native_owned,
            parameter.input_access,
        )?);
    }
    signature.push(callable_result(result)?);
    Ok(PlanType::FunctionPointer(signature))
}

/// Explicit native input loans; returned values still follow the owned callable component contract.
fn callable_input(ty: &IncanType) -> Result<PlanType, String> {
    match ty {
        IncanType::Ref(_) | IncanType::RefMut(_) => context_free_type(ty),
        _ => callable_component(ty),
    }
}

/// A callable's result: any admitted component, or a lazy generator, such as a `flat_map` expansion.
fn callable_result(ty: &IncanType) -> Result<PlanType, String> {
    if let Some(Ok(result)) = generator_type(ty)
        && is_generator_type(&result)
    {
        return Ok(result);
    }
    callable_component(ty)
}

/// A callable's input as its declaration's frame receives it.
///
/// Checked access selects an owned value or a true shared/exclusive reference before ordinary defaults. Binding
/// mutability does not change that access. A checked native collection consumes its outer value; references inside
/// its elements keep their own ABI.
pub fn signature_input(
    ty: PlanType,
    mutable: bool,
    native_owned: bool,
    input_access: IncanInputAccess,
) -> Result<PlanType, String> {
    if input_access != IncanInputAccess::Inferred {
        if native_owned {
            return Err("invalid Body IR simultaneous native and explicit input ownership".into());
        }
        return match input_access {
            IncanInputAccess::Owned => Ok(ty),
            IncanInputAccess::Shared | IncanInputAccess::InferredShared => shared_receiver_type(ty),
            IncanInputAccess::Exclusive => mutable_receiver_type(ty),
            IncanInputAccess::Inferred => Err("invalid Body IR explicit input access".into()),
        };
    }
    if native_owned {
        if !is_owned_collection(&ty) {
            return Err("invalid Body IR owned native collection parameter".into());
        }
        return Ok(ty);
    }
    Ok(parameter_storage_type(&ty, mutable))
}

/// Project a checked callable element onto its parameter ABI.
pub fn semantic_input(
    ty: PlanType,
    kind: IncanCallableParamKind,
    mutable: bool,
    native_owned: bool,
    input_access: IncanInputAccess,
) -> Result<PlanType, String> {
    if kind == IncanCallableParamKind::Normal {
        return signature_input(ty, mutable, native_owned, input_access);
    }
    if input_access != IncanInputAccess::Inferred {
        return Err("unsupported Body IR explicit rest input access".into());
    }
    if mutable {
        return Err("unsupported Body IR mutable rest parameter".into());
    }
    match kind {
        IncanCallableParamKind::RestPositional => {
            if let PlanType::List(leaf, depth) = &ty {
                return Ok(PlanType::List(leaf.clone(), depth + 1));
            }
            Ok(PlanType::List(collection_leaf(&ty)?, 1))
        }
        IncanCallableParamKind::RestKeyword => Ok(PlanType::Dict(ListLeaf::Str, collection_leaf(&ty)?)),
        IncanCallableParamKind::Normal => Err("unsupported Body IR callable parameter role".into()),
    }
}

/// Consume a checked frame pack as owned storage.
pub fn packed_input(
    ty: PlanType,
    kind: IncanCallableParamKind,
    mutable: bool,
    native_owned: bool,
    input_access: IncanInputAccess,
) -> Result<PlanType, String> {
    if kind == IncanCallableParamKind::Normal {
        return signature_input(ty, mutable, native_owned, input_access);
    }
    if input_access != IncanInputAccess::Inferred {
        return Err("unsupported Body IR explicit rest input access".into());
    }
    if mutable {
        return Err("unsupported Body IR mutable rest parameter".into());
    }
    match &ty {
        PlanType::List(_, depth) => {
            if kind == IncanCallableParamKind::RestPositional && *depth > 0 {
                return Ok(ty);
            }
        }
        PlanType::Dict(key, _) => {
            if kind == IncanCallableParamKind::RestKeyword && *key == ListLeaf::Str {
                return Ok(ty);
            }
        }
        _ => {}
    }
    Err("unsupported Body IR checked rest pack representation".into())
}

/// Admit scalar, list, collection and recursively callable signature components; name every other type.
pub fn callable_component(ty: &IncanType) -> Result<PlanType, String> {
    match ty {
        IncanType::Primitive(primitive) => return primitive_type(primitive),
        IncanType::Function { params, return_type } => return function_pointer_type(params, return_type),
        _ => {}
    }
    if let Ok(component) = context_free_type(ty)
        && (is_list(&component) || is_collection(&component))
    {
        return Ok(component);
    }
    Err(format!("unsupported Body IR callable signature type {ty}"))
}

/// The return slot and explicit function annotations keep pointer storage; inferred item locals keep their type.
pub fn function_pointer_storage(body: &Body) -> Vec<i64> {
    let mut locals = Vec::new();
    match &body.callable_representation {
        Some(representation) => {
            if !representation.closure_holding_return {
                locals.push(0);
            }
            for local in &representation.function_pointer_locals {
                locals.push(local.index() as i64 + 1);
            }
        }
        None => locals.push(0),
    }
    for parameter in &body.params {
        if let IncanType::Function { .. } = parameter.ty {
            match &body.callable_representation {
                Some(representation) => {
                    if !representation.closure_holding_locals.contains(&parameter.local) {
                        locals.push(parameter.local.index() as i64 + 1);
                    }
                }
                None => locals.push(parameter.local.index() as i64 + 1),
            }
        }
    }
    locals
}

/// Refuse callable signatures whose closure-holding contract the checker did not record.
pub fn validate_callable_representation(body: &Body) -> Result<(), String> {
    if body.callable_representation.is_some() {
        return Ok(());
    }
    if let IncanType::Function { .. } = body.return_type {
        return Err(format!(
            "unsupported Body IR unproven callable return representation in {}",
            body.name
        ));
    }
    for parameter in &body.params {
        if let IncanType::Function { .. } = parameter.ty {
            return Err(format!(
                "unsupported Body IR unproven callable parameter representation in {}",
                body.name
            ));
        }
    }
    Ok(())
}

/// A selected generic value owns its environment; a call-only parameter owns one temporary lending envelope.
pub fn parameter_representation(body: &Body, local: usize, ty: PlanType) -> Result<PlanType, String> {
    if let PlanType::FunctionPointer(signature) = &ty
        && let Some(representation) = &body.callable_representation
    {
        if representation
            .closure_holding_locals
            .iter()
            .any(|held| held.index() == local)
        {
            return Ok(PlanType::Closure(signature.clone()));
        }
        if representation
            .closure_holding_parameters
            .iter()
            .any(|held| held.index() == local)
        {
            closure_arguments(signature)?;
            return Ok(PlanType::BorrowingClosure(signature.clone()));
        }
    }
    Ok(ty)
}

/// Retain an instance's owned generic aliases at their checked environment-carrying ABI.
pub fn local_representation(body: &Body, local: usize, ty: PlanType) -> PlanType {
    if let PlanType::FunctionPointer(signature) = &ty
        && let Some(representation) = &body.callable_representation
        && representation
            .closure_holding_locals
            .iter()
            .any(|held| held.index() == local)
    {
        return PlanType::Closure(signature.clone());
    }
    ty
}

/// A function whose one returned closure captures returns that closure's owned callable object. A direct returned
/// view is a shared or exclusive reference to its source place, with the view's access; a carried view's carrier is
/// an owned result.
pub fn return_representation(body: &Body, ty: PlanType) -> Result<PlanType, String> {
    if let Some(access) = direct_view_access(body) {
        return signature_input(ty, false, false, access);
    }
    if let PlanType::FunctionPointer(signature) = &ty
        && let Some(representation) = &body.callable_representation
        && representation.closure_holding_return
    {
        closure_arguments(signature)?;
        return Ok(PlanType::Closure(signature.clone()));
    }
    Ok(ty)
}

/// Validate a closure's retained input and result storage, including shared native model references.
///
/// 1. Validate scalar/text inputs, owned nominals, shared model and collection views, and exclusive `mut` borrows.
/// 2. Validate the result's scalar or owned transfer carrier.
/// 3. Return input types in their checked call order.
pub fn closure_arguments(signature: &[PlanType]) -> Result<Vec<PlanType>, String> {
    // 1. Validate each retained input representation.
    let Some((result, inputs)) = signature.split_last() else {
        return Err("invalid Body IR callable signature without return type".into());
    };
    for input in inputs {
        if !closure_value_type(input) && !owned_nominal_input(input) {
            match input {
                PlanType::ListRef(..)
                | PlanType::SetRef(_)
                | PlanType::DictRef(..)
                | PlanType::ListMutRef(..)
                | PlanType::SetMutRef(_)
                | PlanType::DictMutRef(..)
                | PlanType::StringMutRef
                | PlanType::Model(..)
                | PlanType::ModelMutRef(..)
                | PlanType::ModelRef(..)
                | PlanType::EnumMutRef(..) => {}
                _ => {
                    return Err(
                        "unsupported Body IR closure input outside scalars, text, models, collection views and borrows"
                            .into(),
                    );
                }
            }
        }
    }
    // 2. Validate the result's owned transfer representation.
    if !closure_value_type(result)
        && !is_owned_collection(result)
        && !is_generator_type(result)
        && !owned_nominal_result(result)
    {
        return Err(
            "unsupported Body IR closure result outside scalars, owned text, collections and retained nominal values"
                .into(),
        );
    }
    // 3. Keep the checked input order.
    Ok(inputs.to_vec())
}

/// Report whether a callable input is an owned model or enum, which the call moves into the callable.
fn owned_nominal_input(ty: &PlanType) -> bool {
    matches!(ty, PlanType::Model(..) | PlanType::Enum(..))
}

/// Admit a retained model, newtype or enum through the owned return path; borrowed and unretained layouts stay refused.
fn owned_nominal_result(ty: &PlanType) -> bool {
    matches!(ty, PlanType::Model(..) | PlanType::Enum(..))
}

/// Report whether a value crosses a boxed closure's call boundary by value: a scalar of any numeric carrier, or text.
pub fn closure_value_type(ty: &PlanType) -> bool {
    numeric_carrier(ty) || matches!(ty, PlanType::Bool | PlanType::Unit | PlanType::String)
}

/// Admit captures according to the retained reusable or consuming environment contract.
///
/// Reusable environments copy scalars and clone owned text, collection, model, and enum snapshots for every call.
/// A reusable environment can also hold one shared reference, copied into every call, while checking keeps its source
/// borrowed. Checking forbids changes to captured values and aliases of their storage.
/// A proven consuming environment instead transfers its canonical zip into the lifted body once.
pub fn closure_capture_type(ty: &PlanType, consumes_environment: bool) -> bool {
    match ty {
        PlanType::ZipIterator(..) => consumes_environment,
        _ => {
            owned_capture(ty)
                || (closure_value_type(ty) && *ty != PlanType::Unit)
                || (!consumes_environment && reference_capture(ty))
        }
    }
}

/// Select a shared reference to borrowed text, a collection, a model, an enum, a tuple or a scalar.
pub fn reference_capture(ty: &PlanType) -> bool {
    matches!(
        ty,
        PlanType::StringRef
            | PlanType::StrRef
            | PlanType::ListRef(..)
            | PlanType::SetRef(_)
            | PlanType::DictRef(..)
            | PlanType::ModelRef(..)
            | PlanType::EnumRef(..)
            | PlanType::TupleRef(_)
            | PlanType::ScalarRef(_)
    )
}

/// Select owned text, collection, callable, model, enum and zip snapshots for construction, validation and invocation.
///
/// Reusable calls clone these snapshots.
pub fn owned_capture(ty: &PlanType) -> bool {
    matches!(
        ty,
        PlanType::String
            | PlanType::List(..)
            | PlanType::Set(_)
            | PlanType::Dict(..)
            | PlanType::Model(..)
            | PlanType::Enum(..)
            | PlanType::Closure(_)
            | PlanType::NativeClosure(..)
            | PlanType::ZipIterator(..)
    )
}

/// Function items and function pointers have no environment owner and are native Copy values.
pub fn callable_copy_type(ty: &PlanType) -> bool {
    matches!(ty, PlanType::FunctionPointer(_) | PlanType::FunctionItem(..))
}
