//! Lowering for calls: argument planning and binding, callable identity, nominal construction, method calls.

use super::args::*;
use super::primitives::*;
use super::*;
use incan_lang::lang::builtins::{BuiltinFnId, BuiltinMethodId};
use incan_lang::lang::surface::collection_helpers::{self, BuiltinCollectionHelperId};
use incan_lang::lang::surface::list_methods::ListMethodId;
use incan_lang::lang::surface::result_methods::ResultMethodId;
use incan_semantics_core::SymbolOrigin;

mod argument_binding;
mod builtin_calls;
mod construction;
mod method_calls;
mod named_calls;
mod variants_and_intrinsics;

/// Borrow one written call's arguments and carry its already converted source range through lowering helpers.
struct CallLoweringSite<'ast> {
    type_args: &'ast [ast::Spanned<ast::Type>],
    args: &'ast [ast::CallArg],
    span: ast::Span,
    scope: bir::ScopeId,
    hir_span: HirSourceSpan,
}

/// Own the selected identity and bound arguments after declaration-slot binding has consumed its parameter plan.
struct BoundNamedCall {
    name: String,
    canonical: Option<CanonicalSymbolId>,
    direct_call_id: Option<CompilerNodeId>,
    builtin: Option<BuiltinFnId>,
    type_args: Vec<IncanType>,
    binding: bir::ArgumentBinding,
    arguments: Vec<bir::ArgumentElement>,
    is_async: bool,
}

/// Own the selected method identity and slots while retaining native/provider return contracts for post-binding
/// refinement.
struct MethodCallBinding {
    canonical: Option<CanonicalSymbolId>,
    declared: Option<Vec<DeclaredSlot>>,
    native_member: Option<bir::LoafCallable>,
    retained_method: Option<super::published_methods::PublishedMethod>,
}

/// Put a fully bound two-slot call's single-value arguments in declared slot order.
///
/// `None` unless each slot is supplied exactly once by one written value and nothing is left to a default.
fn slot_ordered_pair(elements: Vec<bir::ArgumentElement>, binding: &bir::ArgumentBinding) -> Option<[bir::Operand; 2]> {
    let bir::ArgumentBinding::Resolved {
        arguments,
        defaulted_slots,
    } = binding
    else {
        return None;
    };
    if !defaulted_slots.is_empty() || arguments.len() != 2 || elements.len() != 2 {
        return None;
    }
    let mut ordered: [Option<bir::Operand>; 2] = [None, None];
    for (element, bound) in elements.into_iter().zip(arguments) {
        let bir::ArgumentElement::One(operand) = element else {
            return None;
        };
        let slot = ordered.get_mut(bound.slot)?;
        if slot.replace(operand).is_some() {
            return None;
        }
    }
    let [Some(first), Some(second)] = ordered else {
        return None;
    };
    Some([first, second])
}

/// Read receiver exclusivity from the checker-selected builtin operation.
fn collection_method_changes_receiver(operation: BuiltinMethodId) -> bool {
    operation.changes_receiver()
}

/// The type a `Type.member(..)` call is made on, when its receiver identifier names a type rather than a value.
///
/// A nominal type in the ordinary namespace (model, class, newtype, rusttype, enum, alias or trait) is that type; a
/// generic parameter (`T.default()` under a `Default` bound) is the type variable, so a trait's associated function
/// called on it keeps the type that selects the implementation. Anything else is a value, and `None`.
fn named_static_receiver_type(name: &str, identity: &CanonicalSymbolId) -> Option<IncanType> {
    if identity.kind == SemanticSourceTargetKind::GenericBinder {
        return Some(IncanType::TypeVar(name.to_owned()));
    }
    (identity.namespace == incan_semantics_core::SymbolNamespace::OrdinaryLexical
        && matches!(
            identity.kind,
            SemanticSourceTargetKind::Model
                | SemanticSourceTargetKind::Class
                | SemanticSourceTargetKind::Newtype
                | SemanticSourceTargetKind::Rusttype
                | SemanticSourceTargetKind::Enum
                | SemanticSourceTargetKind::TypeAlias
                | SemanticSourceTargetKind::Trait
        ))
    .then(|| IncanType::Named(name.to_owned()))
}

/// Keep written nominal arguments in source order; spreads have no checked field-slot binding and remain refused.
fn nominal_argument_expressions<'args>(
    name: &str,
    args: &'args [ast::CallArg],
) -> Result<Vec<&'args ast::Spanned<ast::Expr>>, String> {
    args.iter()
        .map(|argument| match argument {
            ast::CallArg::Positional(value) | ast::CallArg::Named(_, value) => Ok(value),
            ast::CallArg::PositionalUnpack(_) => {
                Err(format!("construction of `{name}` with a positional argument spread"))
            }
            ast::CallArg::KeywordUnpack(_) => Err(format!("construction of `{name}` with a keyword argument spread")),
        })
        .collect()
}

/// Project a compiler-owned builtin id only from its retained RFC 120 identity.
///
/// The call-site spelling is deliberately absent from this function. A same-spelled source declaration, import, or
/// alias cannot become a builtin by textual resemblance, while a canonical builtin alias retains the registry's
/// declaration name and therefore still projects to the same [`BuiltinFnId`].
fn canonical_builtin(identity: &CanonicalSymbolId) -> Option<incan_lang::lang::builtins::BuiltinFnId> {
    (identity.origin == incan_semantics_core::SymbolOrigin::Builtin
        && identity.namespace == incan_semantics_core::SymbolNamespace::OrdinaryLexical
        && identity.kind == SemanticSourceTargetKind::Builtin
        && identity.scope_discriminant.is_none()
        && identity.declaration_span == HirSourceSpan::new(0, 0))
    .then(|| incan_lang::lang::builtins::from_str(&identity.declaration_name))
    .flatten()
}

/// Retain the sole operand frozen legacy passes to canonical `enumerate`, including a written positional spread.
///
/// Legacy's builtin emission directly enumerates the outer operand instead of unpacking its elements into arguments.
/// Only the checker's canonical builtin identity authorizes this profile; user callables keep ordinary spread binding.
fn legacy_enumerate_argument(builtin: Option<BuiltinFnId>, args: &[ast::CallArg]) -> Option<Vec<ast::CallArg>> {
    if builtin != Some(BuiltinFnId::Enumerate) {
        return None;
    }
    match args {
        [ast::CallArg::PositionalUnpack(source)] => Some(vec![ast::CallArg::Positional(source.clone())]),
        _ => None,
    }
}
