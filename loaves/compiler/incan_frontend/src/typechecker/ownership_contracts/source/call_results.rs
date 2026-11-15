//! Closed result shapes that source-call projection can classify without retained dependencies.

use super::Builder;
use crate::ast::Span;
use crate::symbols::ResolvedType;
use incan_lang::lang::builtins::BuiltinMethodId;
use incan_lang::lang::surface::{
    clone_methods::CloneMethodId, list_methods::ListMethodId, option_methods::OptionMethodId,
};
use incan_lang::lang::trait_bounds;
use incan_lang::lang::traits::{self as core_traits, TraitId};
use incan_lang::lang::types::collections::{self, CollectionTypeId};

impl Builder<'_> {
    /// Return whether semantic method resolution selected the standard `Clone.clone` slot for this call. A source
    /// method that merely uses the spelling `clone` does not gain independent-result semantics.
    pub(super) fn checked_clone_call(&self, span: Span) -> bool {
        let builtin = crate::symbols::canonical_builtin_member_identity(
            core_traits::as_str(TraitId::Clone),
            trait_bounds::rust::CLONE_METHOD,
        );
        if matches!(
            self.checker.type_info.resolved_builtin_method_call(span),
            Some(
                BuiltinMethodId::Clone(CloneMethodId::Clone)
                    | BuiltinMethodId::List(ListMethodId::Clone)
                    | BuiltinMethodId::Option(OptionMethodId::Clone)
            )
        ) || self.checker.type_info.traits.inferred_method_identities.get(&span) == Some(&builtin)
        {
            return true;
        }
        self.checker.type_info.resolved_method_call(span).is_some_and(|call| {
            call.method == trait_bounds::rust::CLONE_METHOD
                && matches!(
                    &call.dispatch,
                    crate::typechecker::ResolvedMethodDispatch::Trait { trait_name, .. }
                        if core_traits::from_str(trait_name) == Some(TraitId::Clone)
                )
        })
    }
}

/// This initial call profile admits only closed scalar/owned byte-string results and builtin `Result`/`Option`
/// carriers of them, with no possible retained source dependency. Other aggregates, references, native and generic
/// results await their checked dependency representation.
pub(in crate::typechecker::ownership_contracts) fn plain_result(ty: &ResolvedType) -> bool {
    match ty {
        ResolvedType::Unit
        | ResolvedType::Int
        | ResolvedType::Float
        | ResolvedType::Numeric(_)
        | ResolvedType::Bool
        | ResolvedType::Str
        | ResolvedType::Bytes
        | ResolvedType::FrozenStr
        | ResolvedType::FrozenBytes => true,
        ResolvedType::Generic(_, arguments) if ty.is_result() || ty.is_option() => {
            !arguments.is_empty() && arguments.iter().all(plain_result)
        }
        _ => false,
    }
}

/// Retain an unresolved scalar-carrier obligation without declaring any named leaf independent. Legacy checking
/// can represent an enclosing binder as Named; Body IR classifies it using the exact declaration's binder inventory.
/// A real nominal leaf stays nominal and refuses closed admission until its dependency proof is supported.
pub(in crate::typechecker::ownership_contracts) fn conditional_result(ty: &ResolvedType) -> bool {
    /// Check carrier leaves while retaining whether any unresolved binder prevents a final independence proof.
    fn leaves(ty: &ResolvedType, generic: &mut bool) -> bool {
        match ty {
            ResolvedType::TypeVar(_) | ResolvedType::Named(_) => {
                *generic = true;
                true
            }
            ResolvedType::Generic(_, arguments) if ty.is_result() || ty.is_option() => {
                !arguments.is_empty() && arguments.iter().all(|item| leaves(item, generic))
            }
            _ => plain_result(ty),
        }
    }
    let mut generic = false;
    (ty.is_result() || ty.is_option()) && leaves(ty, &mut generic) && generic
}

/// Printing reaches no nominal display hook for primitive values and for lists whose items print the same way.
pub(super) fn primitive_display(ty: &ResolvedType) -> bool {
    match ty {
        ResolvedType::Str
        | ResolvedType::FrozenStr
        | ResolvedType::Int
        | ResolvedType::Float
        | ResolvedType::Bool
        | ResolvedType::Unit => true,
        ResolvedType::Generic(name, arguments) => {
            collections::from_str(name) == Some(CollectionTypeId::List)
                && matches!(arguments.as_slice(), [item] if primitive_display(item))
        }
        _ => false,
    }
}
