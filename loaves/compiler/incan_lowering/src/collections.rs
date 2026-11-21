//! Hashed collection layouts, primitive leaves and their runtime wrappers.

use incan_mir_plan_native::{ListLeaf, PlanType, TupleElement, list_leaf_type};
use incan_semantics_core::{IncanPrimitiveType, IncanType, NumericTypeId};

use crate::models::runtime_storage_marker;
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
