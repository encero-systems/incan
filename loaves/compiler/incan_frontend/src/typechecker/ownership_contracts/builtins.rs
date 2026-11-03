//! Builtin collection methods an explicit body may call on a checked place, with the receiver access each needs, and
//! the builtin carrier forms that hold or reveal a carried view.

use incan_lang::lang::surface::constructors::{self, ConstructorId};
use incan_lang::lang::surface::list_methods::{self, ListMethodId};
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::SymbolOrigin;

use crate::ast::{CallArg, Expr, Pattern, PatternArg, Span, Spanned, SurfaceExpr, SurfaceExprPayload};
use crate::symbols::ResolvedType;
use crate::typechecker::TypeCheckInfo;
use incan_lang::lang::keywords::KeywordId;
use incan_semantics_core::SurfaceFeatureKey;

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

/// The call whose returned view an expression produces: the expression itself, or the call `?` takes a carried view
/// out of, through parentheses.
pub(super) fn view_call(expression: &Spanned<Expr>) -> &Spanned<Expr> {
    match &expression.node {
        Expr::Paren(inner) | Expr::Try(inner) => view_call(inner),
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
pub(super) fn list_method_writes(
    facts: &TypeCheckInfo,
    receiver: &Spanned<Expr>,
    method: &str,
    span: Span,
) -> Option<bool> {
    let list = matches!(
        facts.expr_type(receiver.span),
        Some(ResolvedType::Generic(name, arguments))
            if collections::from_str(name) == Some(CollectionTypeId::List) && arguments.len() == 1
    );
    if !list || facts.resolved_method_call(span).is_some() {
        return None;
    }
    match list_methods::from_str(method)? {
        ListMethodId::Append
        | ListMethodId::Extend
        | ListMethodId::Pop
        | ListMethodId::Swap
        | ListMethodId::Reserve
        | ListMethodId::ReserveExact
        | ListMethodId::Remove => Some(true),
        ListMethodId::Contains | ListMethodId::Count | ListMethodId::Index | ListMethodId::Clone => Some(false),
    }
}

/// The expression an `await` suspends on, when a surface expression is `await expression`.
pub(super) fn awaited(surface: &SurfaceExpr) -> Option<&Spanned<Expr>> {
    match (&surface.key, &surface.payload) {
        (SurfaceFeatureKey::SoftKeyword(KeywordId::Await), SurfaceExprPayload::PrefixUnary(awaited)) => Some(awaited),
        _ => None,
    }
}
