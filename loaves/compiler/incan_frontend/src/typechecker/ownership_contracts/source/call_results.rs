//! Call result shapes for source-call projection: closed results with no retained dependency, and results that
//! carry their one carrying input's dependencies through a type argument.

use super::super::{InputCall, InputCallKind};
use super::{Builder, CanonicalSymbolId, Root, Unavailable};
use crate::ast::{Expr, Span, Spanned};
use crate::ownership_metadata::CheckedOwnershipContract;
use crate::symbols::ResolvedType;
use incan_lang::lang::builtins::BuiltinMethodId;
use incan_lang::lang::surface::{
    clone_methods::CloneMethodId, list_methods::ListMethodId, option_methods::OptionMethodId,
};
use incan_lang::lang::trait_bounds;
use incan_lang::lang::traits::{self as core_traits, TraitId};
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_lang::lang::types::ownership::OwnershipMode;

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

    /// Whether a temporary argument is a fresh value: no returned or native view, and a type that can hold no borrow.
    pub(super) fn fresh_argument(&mut self, expression: &Spanned<Expr>) -> Result<bool, Unavailable> {
        if self.view_source(expression)?.is_some()
            || self.view_rooted(expression)
            || self
                .native_result_relation(expression)?
                .is_some_and(|relation| !relation.sources.is_empty())
        {
            return Ok(false);
        }
        Ok(self
            .checker
            .type_info
            .expr_type(expression.span)
            .is_some_and(|ty| plain_result(ty) || self.checker.type_holds_no_borrow(ty)))
    }

    /// Whether a source call's result holds a borrow only through the type arguments it is instantiated with: the
    /// declaration publishes no result dependency, yet the instantiated result type can hold a borrow, and every
    /// borrow it can hold is a substituted view, native reference or open type parameter. A declared view result
    /// needs a `from` clause (INCAN-T0124), so such a borrow can only be one the call's inputs carry (#2972). A
    /// callable, a trait object or a native type with lifetimes could capture a borrow of its own and is not carried.
    pub(super) fn carried_result(
        &self,
        call: &InputCall,
        contract: Option<&CheckedOwnershipContract>,
        result: &ResolvedType,
    ) -> bool {
        matches!(call.kind, InputCallKind::Explicit | InputCallKind::Ordinary)
            && contract.is_none_or(|contract| {
                contract.result_source.is_none() && contract.modes.iter().all(|mode| mode.component.parameter.is_some())
            })
            && !plain_result(result)
            && !self.checker.type_holds_no_borrow(result)
            && self.borrows_only_in_substituted_leaves(result)
    }

    /// Whether every borrow a value of `ty` can hold sits in a leaf a type argument supplied: a view or a native
    /// reference, or a type parameter still open. A generic nominal or builtin carrier qualifies when its own layout,
    /// with each type argument replaced by unit, holds no borrow and each argument qualifies in turn.
    fn borrows_only_in_substituted_leaves(&self, ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::Ref(_) | ResolvedType::RefMut(_) | ResolvedType::TypeVar(_) => true,
            ResolvedType::Generic(name, arguments) if let Some(mode) = self.checker.explicit_ownership_mode(name) => {
                mode != OwnershipMode::Owned
                    || arguments
                        .iter()
                        .all(|argument| self.borrows_only_in_substituted_leaves(argument))
            }
            ResolvedType::Generic(name, arguments) => {
                let opaque = ResolvedType::Generic(name.clone(), vec![ResolvedType::Unit; arguments.len()]);
                self.checker.type_holds_no_borrow(&opaque)
                    && arguments
                        .iter()
                        .all(|argument| self.borrows_only_in_substituted_leaves(argument))
            }
            ResolvedType::Tuple(items) => items.iter().all(|item| self.borrows_only_in_substituted_leaves(item)),
            ResolvedType::FrozenList(item) | ResolvedType::FrozenSet(item) => {
                self.borrows_only_in_substituted_leaves(item)
            }
            ResolvedType::FrozenDict(key, value) => {
                self.borrows_only_in_substituted_leaves(key) && self.borrows_only_in_substituted_leaves(value)
            }
            _ => self.checker.type_holds_no_borrow(ty),
        }
    }

    /// Whether this expression is a call, possibly taken with `?`, whose result is carried as `carried_result` says.
    pub(super) fn carried_call(&self, expression: &Spanned<Expr>) -> bool {
        let call = super::super::builtins::view_call(expression);
        if !matches!(call.node, Expr::Call(..) | Expr::MethodCall(..)) {
            return false;
        }
        let facts = &self.checker.type_info;
        facts
            .protocols
            .explicit_input_calls
            .get(&call.span)
            .is_some_and(|input| {
                facts.selected_input_contract(input).is_ok_and(|contract| {
                    facts
                        .expr_type(call.span)
                        .is_some_and(|result| self.carried_result(input, contract, result))
                })
            })
    }

    /// Record the dependencies a carried result retains: those of the one input whose value holds any, its receiver
    /// or an argument. A result retaining the dependencies of more than one input would name more than one source,
    /// which one result cannot publish (RFC 164). A new local binding takes them; any other position is refused once
    /// the body is complete.
    pub(super) fn retain_carried_result(&mut self, span: Span) -> Result<(), Unavailable> {
        let invocation = self.graph.invocations.get(&span).ok_or(Unavailable {
            span,
            reason: "carried result has no checked invocation".into(),
        })?;
        let dependencies_of = |root: Option<&Root>| match root {
            Some(Root::Binding(root)) => self.graph.dependent_locals.get(root).cloned().unwrap_or_default(),
            _ => Vec::new(),
        };
        let receiver = if invocation.receiver_dependencies.is_empty() {
            let source = invocation
                .receiver
                .and_then(|loan| self.graph.loans.get(loan.0))
                .map(|loan| &loan.source)
                .or(invocation.receiver_source.as_ref());
            dependencies_of(source.map(|place| &place.root))
        } else {
            invocation.receiver_dependencies.clone()
        };
        let mut carrying = std::iter::once(receiver)
            .chain(
                invocation
                    .arguments
                    .iter()
                    .map(|argument| dependencies_of(argument.source.as_ref().map(|place| &place.root))),
            )
            .filter(|dependencies| !dependencies.is_empty());
        let Some(dependencies) = carrying.next() else {
            return Ok(());
        };
        if carrying.next().is_some() {
            return Err(Unavailable {
                span,
                reason: "a carried result depends on more than one input".into(),
            });
        }
        self.carried_results.insert(span, dependencies);
        Ok(())
    }

    /// Bind a carried call result to a new local: the local holds a child of each dependency its carrying input
    /// retains, so every later use of the local keeps those sources valid. A result with no dependency in this body is
    /// an ordinary value.
    pub(super) fn bind_carried_result(
        &mut self,
        value: &Spanned<Expr>,
        root: &CanonicalSymbolId,
        name_span: Span,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let mut incoming = self.expression(value, incoming, true)?;
        self.root(root, name_span)?;
        let span = super::super::builtins::view_call(value).span;
        let Some(parents) = self.carried_results.remove(&span) else {
            return Ok(incoming);
        };
        let mut loans = Vec::with_capacity(parents.len());
        for parent in parents {
            let (next, _, loan) = self.carry_dependency(parent, span, incoming)?;
            incoming = next;
            loans.push(loan);
        }
        if self.graph.dependent_locals.insert(root.clone(), loans).is_some() {
            return Err(Unavailable {
                span: name_span,
                reason: "carried result has more than one dependency destination".into(),
            });
        }
        Ok(incoming)
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
