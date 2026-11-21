//! Callable signature admission from checked frontend representation facts.

use incan_mir_plan_native::{ListLeaf, PlanType};
use incan_semantics_core::{IncanCallableParam, IncanCallableParamKind, IncanInputAccess, IncanType};

use crate::collections::{is_collection, is_owned_collection};
use crate::generators::{generator_type, is_generator_type};
use crate::layout_types::{
    callable_parameter_type, collection_leaf, context_free_type, mutable_receiver_type, shared_receiver_type,
};
use crate::lists::{is_list, parameter_storage_type};
use crate::native_type_identity::constructor_index;
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
