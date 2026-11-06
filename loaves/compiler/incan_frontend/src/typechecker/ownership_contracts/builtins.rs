//! Builtin collection methods an explicit body may call on a checked place, with the receiver access each needs, and
//! the builtin carrier forms that hold or reveal a carried view.

use incan_lang::lang::builtins::BuiltinMethodId;
use incan_lang::lang::surface::constructors::{self, ConstructorId};
use incan_lang::lang::surface::iterator_methods::{self, IteratorMethodId};
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

use crate::ast::{CallArg, Expr, Pattern, PatternArg, Span, Spanned, SurfaceExpr, SurfaceExprPayload};
use crate::symbols::ResolvedType;
use crate::typechecker::TypeCheckInfo;
use incan_lang::lang::keywords::KeywordId;
use incan_semantics_core::SurfaceFeatureKey;

use super::flow::{Graph, LoanId};

/// Return the registry identity of a deferred iterator or generator member.
///
/// Collection families with a closed [`BuiltinMethodId`] are selected through the typed checker fact instead. This
/// identity-backed path remains for iterator and generator operations whose family has not yet joined that handoff.
pub(super) fn retained_iterator_member(identity: &CanonicalSymbolId) -> Option<IteratorMethodId> {
    if identity.origin != SymbolOrigin::Builtin || identity.namespace != incan_semantics_core::SymbolNamespace::Member {
        return None;
    }
    let method = iterator_methods::from_str(identity.declaration_name.rsplit_once('.')?.1)?;
    let owner = identity.declaration_name.split_once('.')?.0;
    let generator = collections::as_str(CollectionTypeId::Generator);
    let iterator = incan_lang::lang::traits::as_str(incan_lang::lang::traits::TraitId::Iterator);
    (owner == generator || owner == iterator).then_some(method)
}

/// Return the lazy iterator member authenticated for one checked call span.
///
/// A list iterator is selected only by its typed builtin operation. Iterator and generator adapters retain their
/// existing canonical-identity path until those families gain their own typed operation handoff.
pub(super) fn retained_iterator_method(facts: &TypeCheckInfo, span: Span) -> Option<IteratorMethodId> {
    if facts.resolved_builtin_method_call(span) == Some(BuiltinMethodId::ListIter) {
        return Some(IteratorMethodId::Iter);
    }
    retained_iterator_member(facts.resolved_identity(span)?)
}

/// Return whether a loan descends from a checked lazy-result dependency. The source expression's canonical builtin
/// identity authenticates iterator calls; a generator expression is authenticated by its already checked result type.
pub(super) fn lazy_result_loan(graph: &Graph, facts: &TypeCheckInfo, loan: LoanId) -> bool {
    let mut loan = Some(loan);
    while let Some(id) = loan {
        let Some(retained) = graph.loans.get(id.0) else {
            return false;
        };
        if retained_iterator_method(facts, retained.span).is_some()
            || matches!(facts.expr_type(retained.span),
                Some(ResolvedType::Generic(name, _))
                    if collections::from_str(name) == Some(CollectionTypeId::Generator))
        {
            return true;
        }
        loan = retained.parent;
    }
    false
}

/// The builtin `Ok`, `Err` or `Some` constructor a callee names. A same-spelled source callable is never a builtin
/// carrier. This is the one definition the source flow and both admission walkers share.
pub(super) fn carrier_constructor(facts: &TypeCheckInfo, callee: &Spanned<Expr>) -> Option<ConstructorId> {
    let Expr::Ident(name) = &callee.node else {
        return None;
    };
    let id = constructors::from_str(name)
        .filter(|id| matches!(id, ConstructorId::Ok | ConstructorId::Err | ConstructorId::Some))?;
    facts
        .resolved_identity(callee.span)
        .is_none_or(|identity| identity.origin == SymbolOrigin::Builtin && identity.declaration_name == *name)
        .then_some(id)
}

/// The builtin carrier construction a call applies, with its arguments.
pub(super) fn carrier_construction<'a>(
    facts: &TypeCheckInfo,
    value: &'a Spanned<Expr>,
) -> Option<(ConstructorId, &'a [CallArg])> {
    let Expr::Call(callee, _, arguments) = &value.node else {
        return None;
    };
    carrier_constructor(facts, callee).map(|id| (id, arguments.as_slice()))
}

/// The call whose returned view an expression produces: the expression itself, or the call `await` or `?` takes a
/// view out of, through parentheses.
pub(super) fn view_call(expression: &Spanned<Expr>) -> &Spanned<Expr> {
    match &expression.node {
        Expr::Paren(inner) | Expr::Try(inner) => view_call(inner),
        Expr::Surface(surface) if let Some(awaited) = awaited(surface) => view_call(awaited),
        _ => expression,
    }
}

/// Whether a carrier variant holds the view of a result that carries it in `payload`: `Ok` or `Some` for payload `0`,
/// `Err` for payload `1`. Every other variant holds no view.
pub(crate) fn variant_carries_view(variant: ConstructorId, payload: usize) -> bool {
    match payload {
        0 => matches!(variant, ConstructorId::Ok | ConstructorId::Some),
        1 => variant == ConstructorId::Err,
        _ => false,
    }
}

/// The name the arm of the variant holding the view binds, with its span: `Ok(name)` or `Some(name)` for payload `0`,
/// `Err(name)` for payload `1`. In a match on a carried view's call, that arm receives the view; the checker's source
/// flow and Body IR both select it here.
pub(crate) fn carried_view_binding(pattern: &Spanned<Pattern>, payload: usize) -> Option<(&str, Span)> {
    let Pattern::Constructor(name, arguments) = &pattern.node else {
        return None;
    };
    if !constructors::from_str(&name.node).is_some_and(|variant| variant_carries_view(variant, payload)) {
        return None;
    }
    let [PatternArg::Positional(payload)] = arguments.as_slice() else {
        return None;
    };
    match &payload.node {
        Pattern::Binding(binding) => Some((binding.as_str(), payload.span)),
        _ => None,
    }
}

/// Whether a method call is a builtin `list` method, and whether it changes its receiver in place: `Some(true)` for a
/// write, `Some(false)` for a read. A trait or source method selected for the call is never a builtin.
pub(super) fn list_method_writes(facts: &TypeCheckInfo, span: Span) -> Option<bool> {
    match facts.resolved_builtin_method_call(span)? {
        operation @ BuiltinMethodId::List(_) => Some(operation.changes_receiver()),
        _ => None,
    }
}

/// Return the receiver of the compiler-selected `bytes.as_slice()` view. The retained Body IR result fact proves this
/// is the builtin native slice projection rather than a source method with the same spelling.
pub(super) fn byte_slice_view<'a>(facts: &TypeCheckInfo, expression: &'a Spanned<Expr>) -> Option<&'a Spanned<Expr>> {
    let Expr::MethodCall(receiver, method, _, arguments) = &expression.node else {
        return None;
    };
    (method == "as_slice"
        && arguments.is_empty()
        && facts.expr_type(receiver.span) == Some(&ResolvedType::Bytes)
        && facts.calls.body_ir_call_result_types.contains_key(&expression.span))
    .then_some(receiver)
}

/// The expression an `await` suspends on, when a surface expression is `await expression`.
pub(super) fn awaited(surface: &SurfaceExpr) -> Option<&Spanned<Expr>> {
    match (&surface.key, &surface.payload) {
        (SurfaceFeatureKey::SoftKeyword(KeywordId::Await), SurfaceExprPayload::PrefixUnary(awaited)) => Some(awaited),
        _ => None,
    }
}
