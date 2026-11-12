//! Builtin collection methods an explicit body may call on a checked place, with the receiver access each needs, and
//! the builtin carrier forms that hold or reveal a carried view.

use incan_lang::lang::builtins::BuiltinMethodId;
use incan_lang::lang::surface::bytes_methods::BytesMethodId;
use incan_lang::lang::surface::constructors::{self, ConstructorId};
use incan_lang::lang::surface::iterator_methods::IteratorMethodId;
use incan_lang::lang::surface::option_methods::OptionMethodId;
use incan_lang::lang::surface::set_methods::SetMethodId;
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

use crate::ast::{CallArg, Expr, Pattern, PatternArg, Span, Spanned, SurfaceExpr, SurfaceExprPayload};
use crate::symbols::ResolvedType;
use crate::typechecker::TypeCheckInfo;
use incan_lang::lang::keywords::KeywordId;
use incan_semantics_core::SurfaceFeatureKey;

use super::flow::{Graph, LoanId};

/// Return the lazy iterator member authenticated for one checked call span.
///
/// List, opaque-iterator and concrete-generator members are selected only by their typed builtin operation.
pub(super) fn retained_iterator_method(facts: &TypeCheckInfo, span: Span) -> Option<IteratorMethodId> {
    match facts.resolved_builtin_method_call(span)? {
        BuiltinMethodId::ListIter => Some(IteratorMethodId::Iter),
        BuiltinMethodId::Iterator(method) | BuiltinMethodId::Generator(method) => Some(method),
        _ => None,
    }
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

/// Whether a method call is a supported builtin place operation, and whether it changes its receiver in place:
/// `Some(true)` for a write, `Some(false)` for a read. A trait or source method selected for the call is never a
/// builtin. Registry identities, rather than method spellings, keep same-named source methods outside this boundary.
pub(super) fn builtin_place_method_writes(facts: &TypeCheckInfo, span: Span) -> Option<bool> {
    match facts.resolved_builtin_method_call(span)? {
        operation @ BuiltinMethodId::List(_) => Some(operation.changes_receiver()),
        operation @ BuiltinMethodId::Set(SetMethodId::Add | SetMethodId::Contains) => {
            Some(operation.changes_receiver())
        }
        operation @ BuiltinMethodId::Option(OptionMethodId::IsSome | OptionMethodId::IsNone) => {
            Some(operation.changes_receiver())
        }
        _ => None,
    }
}

/// Return the receiver of the compiler-selected `bytes.as_slice()` view. The retained Body IR result fact proves this
/// is the builtin native slice projection rather than a source method with the same spelling.
pub(super) fn byte_slice_view<'a>(facts: &TypeCheckInfo, expression: &'a Spanned<Expr>) -> Option<&'a Spanned<Expr>> {
    let Expr::MethodCall(receiver, _, _, arguments) = &expression.node else {
        return None;
    };
    (facts.resolved_builtin_method_call(expression.span) == Some(BuiltinMethodId::Bytes(BytesMethodId::AsSlice))
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
