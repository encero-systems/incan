//! Const-eval helpers shared across the typechecker.
use crate::ast::Span;
use crate::diagnostics::{CompileError, errors};
use crate::symbols::ResolvedType;

use super::{collection_type_id, frozen_bytes_ty, frozen_str_ty, tuple_generic_ty};
use incan_lang::lang::types::collections::CollectionTypeId;

/// Check whether a type is acceptable for indexing/slicing integer positions.
///
/// Accepts `int` and `Unknown` (for inference / error recovery).
pub fn is_intlike_for_index(ty: &ResolvedType) -> bool {
    matches!(ty, ResolvedType::Int | ResolvedType::Unknown)
}

/// Map a const annotation type to its frozen equivalent.
///
/// `const` implies deep immutability: common containers are rewritten to their frozen wrappers.
pub fn freeze_const_type(ty: ResolvedType) -> ResolvedType {
    match ty {
        ResolvedType::Str => frozen_str_ty(),
        ResolvedType::FrozenStr => ResolvedType::FrozenStr,
        ResolvedType::Bytes => frozen_bytes_ty(),
        ResolvedType::FrozenBytes => ResolvedType::FrozenBytes,
        ResolvedType::Generic(name, args) => match collection_type_id(name.as_str()) {
            Some(CollectionTypeId::List) => {
                ResolvedType::FrozenList(Box::new(args.first().cloned().unwrap_or(ResolvedType::Unknown)))
            }
            Some(CollectionTypeId::Dict) => ResolvedType::FrozenDict(
                Box::new(args.first().cloned().unwrap_or(ResolvedType::Unknown)),
                Box::new(args.get(1).cloned().unwrap_or(ResolvedType::Unknown)),
            ),
            Some(CollectionTypeId::Set) => {
                ResolvedType::FrozenSet(Box::new(args.first().cloned().unwrap_or(ResolvedType::Unknown)))
            }
            Some(CollectionTypeId::FrozenList) => {
                ResolvedType::FrozenList(Box::new(args.first().cloned().unwrap_or(ResolvedType::Unknown)))
            }
            Some(CollectionTypeId::FrozenDict) => ResolvedType::FrozenDict(
                Box::new(args.first().cloned().unwrap_or(ResolvedType::Unknown)),
                Box::new(args.get(1).cloned().unwrap_or(ResolvedType::Unknown)),
            ),
            Some(CollectionTypeId::FrozenSet) => {
                ResolvedType::FrozenSet(Box::new(args.first().cloned().unwrap_or(ResolvedType::Unknown)))
            }
            // Tuples stay tuples; their element types may already be frozen.
            Some(CollectionTypeId::Tuple) => tuple_generic_ty(args),
            _ => ResolvedType::Generic(name, args),
        },
        ResolvedType::FrozenList(_) | ResolvedType::FrozenDict(_, _) | ResolvedType::FrozenSet(_) => ty,
        other => other,
    }
}

/// Validate that a condition type is compatible with `bool`.
pub fn ensure_bool_condition(
    cond_ty: &ResolvedType,
    span: Span,
    is_compatible: bool,
    errors_out: &mut Vec<CompileError>,
) {
    if !is_compatible {
        errors_out.push(errors::type_mismatch("bool", &cond_ty.to_string(), span));
    }
}
