//! Conversion helpers between manifest-level [`super::TypeRef`] values and frontend semantic types.

use incan_core::lang::conventions;
use incan_core::lang::types::collections::{self, CollectionTypeId};
use incan_core::lang::types::numerics::{self, NumericTypeId};
use incan_core::lang::types::stringlike::{self, StringLikeId};

use super::TypeRef;
use crate::frontend::symbols::ResolvedType;

/// Convert a frontend semantic [`ResolvedType`] into the stable manifest-level [`TypeRef`] surface.
///
/// This keeps manifest type encoding in one place so producers write a deterministic type spelling regardless of where
/// the export originated.
pub(super) fn type_ref_from_resolved(ty: &ResolvedType) -> TypeRef {
    match ty {
        ResolvedType::Int => named_type_ref(numerics::as_str(NumericTypeId::Int)),
        ResolvedType::Float => named_type_ref(numerics::as_str(NumericTypeId::Float)),
        ResolvedType::Bool => named_type_ref(numerics::as_str(NumericTypeId::Bool)),
        ResolvedType::Str => named_type_ref(stringlike::as_str(StringLikeId::Str)),
        ResolvedType::Bytes => named_type_ref(stringlike::as_str(StringLikeId::Bytes)),
        // Keep existing surface spellings used by ResolvedType display for frozen string-like types.
        ResolvedType::FrozenStr => named_type_ref(ResolvedType::FrozenStr.to_string()),
        ResolvedType::FrozenBytes => named_type_ref(ResolvedType::FrozenBytes.to_string()),
        ResolvedType::FrozenList(inner) => TypeRef::Applied {
            name: collections::as_str(CollectionTypeId::FrozenList).to_string(),
            args: vec![type_ref_from_resolved(inner)],
        },
        ResolvedType::FrozenDict(key, value) => TypeRef::Applied {
            name: collections::as_str(CollectionTypeId::FrozenDict).to_string(),
            args: vec![type_ref_from_resolved(key), type_ref_from_resolved(value)],
        },
        ResolvedType::FrozenSet(inner) => TypeRef::Applied {
            name: collections::as_str(CollectionTypeId::FrozenSet).to_string(),
            args: vec![type_ref_from_resolved(inner)],
        },
        ResolvedType::Unit => named_type_ref(conventions::UNIT_TYPE_NAME),
        ResolvedType::Named(name) => named_type_ref(name.clone()),
        ResolvedType::Generic(name, args) => TypeRef::Applied {
            name: name.clone(),
            args: args.iter().map(type_ref_from_resolved).collect(),
        },
        ResolvedType::Function(params, return_type) => TypeRef::Function {
            params: params.iter().map(type_ref_from_resolved).collect(),
            return_type: Box::new(type_ref_from_resolved(return_type)),
        },
        ResolvedType::Tuple(elements) => TypeRef::Tuple {
            elements: elements.iter().map(type_ref_from_resolved).collect(),
        },
        ResolvedType::TypeVar(name) => TypeRef::TypeParam { name: name.clone() },
        ResolvedType::SelfType => TypeRef::SelfType,
        ResolvedType::Ref(inner) => TypeRef::Ref {
            inner: Box::new(type_ref_from_resolved(inner)),
        },
        // Mutable refs are compiler-internal today; manifests only preserve the borrowed inner type.
        ResolvedType::RefMut(inner) => TypeRef::Ref {
            inner: Box::new(type_ref_from_resolved(inner)),
        },
        ResolvedType::RustPath(_) => TypeRef::Unknown,
        ResolvedType::Unknown => TypeRef::Unknown,
    }
}

/// Convert a manifest-level [`TypeRef`] into frontend semantic [`ResolvedType`].
///
/// This keeps manifest type decoding in one place so both compiler consumers (typechecker, LSP, etc.) follow the same
/// mapping contract.
pub fn resolved_type_from_manifest_type_ref(ty: &TypeRef) -> ResolvedType {
    match ty {
        TypeRef::Named { name } => resolved_named_type_from_manifest(name),
        TypeRef::Applied { name, args } => {
            let resolved_args: Vec<ResolvedType> = args.iter().map(resolved_type_from_manifest_type_ref).collect();
            match collections::from_str(name.as_str()) {
                Some(CollectionTypeId::FrozenList) => ResolvedType::FrozenList(Box::new(
                    resolved_args.first().cloned().unwrap_or(ResolvedType::Unknown),
                )),
                Some(CollectionTypeId::FrozenSet) => ResolvedType::FrozenSet(Box::new(
                    resolved_args.first().cloned().unwrap_or(ResolvedType::Unknown),
                )),
                Some(CollectionTypeId::FrozenDict) => ResolvedType::FrozenDict(
                    Box::new(resolved_args.first().cloned().unwrap_or(ResolvedType::Unknown)),
                    Box::new(resolved_args.get(1).cloned().unwrap_or(ResolvedType::Unknown)),
                ),
                Some(collection_id) => {
                    ResolvedType::Generic(collections::as_str(collection_id).to_string(), resolved_args)
                }
                None => ResolvedType::Generic(name.clone(), resolved_args),
            }
        }
        TypeRef::Function { params, return_type } => ResolvedType::Function(
            params.iter().map(resolved_type_from_manifest_type_ref).collect(),
            Box::new(resolved_type_from_manifest_type_ref(return_type)),
        ),
        TypeRef::Tuple { elements } => {
            ResolvedType::Tuple(elements.iter().map(resolved_type_from_manifest_type_ref).collect())
        }
        TypeRef::TypeParam { name } => ResolvedType::TypeVar(name.clone()),
        TypeRef::SelfType => ResolvedType::SelfType,
        TypeRef::Ref { inner } => ResolvedType::Ref(Box::new(resolved_type_from_manifest_type_ref(inner))),
        TypeRef::Unknown => ResolvedType::Unknown,
    }
}

fn resolved_named_type_from_manifest(name: &str) -> ResolvedType {
    if let Some(id) = numerics::from_str(name) {
        return match id {
            NumericTypeId::Int => ResolvedType::Int,
            NumericTypeId::Float => ResolvedType::Float,
            NumericTypeId::Bool => ResolvedType::Bool,
        };
    }
    if let Some(id) = stringlike::from_str(name) {
        return match id {
            StringLikeId::Str => ResolvedType::Str,
            StringLikeId::Bytes => ResolvedType::Bytes,
            StringLikeId::FrozenStr => ResolvedType::FrozenStr,
            StringLikeId::FrozenBytes => ResolvedType::FrozenBytes,
            StringLikeId::FString => ResolvedType::Str,
        };
    }
    if let Some(id) = collections::from_str(name) {
        return ResolvedType::Named(collections::as_str(id).to_string());
    }
    if name == conventions::UNIT_TYPE_NAME || name == conventions::NONE_TYPE_NAME {
        return ResolvedType::Unit;
    }
    ResolvedType::Named(name.to_string())
}

fn named_type_ref(name: impl Into<String>) -> TypeRef {
    TypeRef::Named { name: name.into() }
}
