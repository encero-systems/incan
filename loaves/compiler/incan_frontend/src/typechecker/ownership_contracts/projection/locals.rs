//! Project a checked local loan into its existing storage dependency without inventing a reference-valued owner.

use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, IncanInputAccess};

use super::{CheckedInputBody, CheckedInputSource, calls};
use crate::ast::{Expr, Spanned};
use crate::symbols::ResolvedType;

/// A local alias retains its declared value type and permissions while referring to another checked source place.
/// Its destination identity is a source binding, never an independent native owner or a drop obligation.
pub(crate) struct CheckedLocalInputLoan<'a> {
    /// Exact canonical local that declared the alias.
    pub destination: &'a CanonicalSymbolId,
    /// Logical referent type, independent of any native reference carrier.
    pub value_type: &'a ResolvedType,
    /// Shared or exclusive access established by the declaration and source loan together.
    pub access: IncanInputAccess,
    /// Whether the source binding separately permits writes through the loan.
    pub writable: bool,
    /// Canonical source storage, including each static stored-field identity.
    pub source: CheckedInputSource,
}

/// An owned declaration receives either an authenticated stored value or a fresh checked initializer. It creates
/// independent destination storage; the optional source describes the transferred value, never a retained loan.
pub(crate) struct CheckedOwnedLocal<'a> {
    /// Underlying checked value type, without the source annotation wrapper.
    pub value_type: &'a ResolvedType,
    /// Exact source owner and field path, or no owner for a fresh initializer.
    pub source: Option<CheckedInputSource>,
}

impl<'a> CheckedInputBody<'a> {
    /// Select the initializer retained by this exact declaration. The body proof has already authenticated its
    /// event index, Copy/Move agreement and canonical destination; no source-span search reconstructs the edge.
    pub(crate) fn owned_local(
        &self,
        destination: &CanonicalSymbolId,
        initializer: HirSourceSpan,
    ) -> Result<CheckedOwnedLocal<'a>, &'static str> {
        let retained = self
            .graph
            .owned_locals
            .get(destination)
            .ok_or("owned local has no retained initializer")?;
        let mode = self
            .facts
            .protocols
            .explicit_local_modes
            .get(destination)
            .ok_or("owned local has no checked annotation")?;
        if retained.initializer != initializer || mode.mode != OwnershipMode::Owned {
            return Err("owned initializer differs from its checked declaration");
        }
        Ok(CheckedOwnedLocal {
            value_type: &mode.value_type,
            source: retained.source.as_ref().map(calls::project_source).transpose()?,
        })
    }

    /// The loan of a local still holding a carried call's result, when an expression names one through
    /// parentheses: the local's loan is a carried call's result loan and its own type is that carrier. `?` and a
    /// `match` read it through that loan rather than consume it, and the view they take reborrows it, keyed to the
    /// same call; a view bound with `view = call()?` has the payload's type instead.
    pub(crate) fn carrier_loan(&self, expression: &Spanned<Expr>) -> Option<(super::flow::LoanId, &super::flow::Loan)> {
        let local = super::super::builtins::view_call(expression);
        if !matches!(local.node, Expr::Ident(_))
            || !self
                .facts
                .expr_type(local.span)
                .is_some_and(|ty| ty.is_result() || ty.is_option())
        {
            return None;
        }
        let id = *self.graph.local_loans.get(self.facts.resolved_identity(local.span)?)?;
        let loan = self.graph.loans.get(id.0)?;
        self.facts.selected_carried_view(loan.span)?;
        Some((id, loan))
    }

    /// The call a view's loan is keyed to: the expression's view call, or the carried call a local holds when the
    /// view is `?` on that local.
    pub(crate) fn view_call_span(&self, expression: &Spanned<Expr>) -> HirSourceSpan {
        let call = super::super::builtins::view_call(expression);
        self.carrier_loan(call).map_or(call.span, |(_, loan)| loan.span)
    }

    /// The access of an unannotated local bound to a returned view, when its loan is exactly its initializer
    /// call's result loan: the initializer itself, or the call `?` takes a carried view out of. Annotated locals and
    /// ordinary bindings return `None`.
    pub(crate) fn view_local(
        &self,
        destination: &CanonicalSymbolId,
        initializer: &Spanned<Expr>,
    ) -> Result<Option<IncanInputAccess>, &'static str> {
        // A closure local holds its closure value; its loan is the closure's captured borrow, not a view.
        if self.facts.protocols.explicit_local_modes.contains_key(destination)
            || self
                .graph
                .closures
                .values()
                .any(|closure| &closure.local == destination)
        {
            return Ok(None);
        }
        let Some(id) = self.graph.local_loans.get(destination) else {
            return Ok(None);
        };
        let loan = self
            .graph
            .loans
            .get(id.0)
            .ok_or("returned view local has no result loan")?;
        if loan.span != self.view_call_span(initializer) {
            return Err("returned view local differs from its initializer call");
        }
        Ok(Some(match loan.kind {
            super::flow::LoanKind::Exclusive => IncanInputAccess::Exclusive,
            super::flow::LoanKind::Shared => IncanInputAccess::Shared,
        }))
    }

    /// The declarations the closure at `span` captures, as this body's ownership graph records them.
    pub(crate) fn closure_captures(&self, span: HirSourceSpan) -> Option<&'a [CanonicalSymbolId]> {
        self.graph
            .closures
            .get(&span)
            .map(|closure| closure.captures.as_slice())
    }

    /// Whether this exact closure retains `capture` through a checked loan rather than a Copy read.
    pub(crate) fn closure_borrows_capture(&self, span: HirSourceSpan, capture: &CanonicalSymbolId) -> bool {
        self.graph
            .closures
            .get(&span)
            .is_some_and(|closure| closure.loans.iter().any(|(identity, _)| identity == capture))
    }

    /// The checked referent type of a local alias: an explicit borrowed annotation's underlying value or the closed
    /// binding type of an unannotated returned view.
    pub(crate) fn local_value_type(&self, local: &CanonicalSymbolId) -> Option<&'a ResolvedType> {
        self.facts
            .protocols
            .explicit_local_modes
            .get(local)
            .filter(|mode| mode.owner == self.owner)
            .map(|mode| &mode.value_type)
            .or_else(|| self.facts.expressions.body_ir_binding_types.get(local))
    }

    /// Select the exact declaration and initializer whose loan this body already authenticated. A matching name,
    /// another initializer span or an index lacking evaluated-storage identity cannot authorize an alias.
    pub(crate) fn local_loan(
        &self,
        destination: &CanonicalSymbolId,
        initializer: HirSourceSpan,
    ) -> Result<CheckedLocalInputLoan<'a>, &'static str> {
        let (local, id) = self
            .graph
            .local_loans
            .get_key_value(destination)
            .ok_or("local alias has no checked destination loan")?;
        let mode = self
            .facts
            .protocols
            .explicit_local_modes
            .get(local)
            .ok_or("local alias has no checked annotation")?;
        let loan = self.graph.loans.get(id.0).ok_or("local alias has no source loan")?;
        if loan.span != initializer {
            return Err("local alias initializer differs from its checked source loan");
        }
        let access = match mode.mode {
            OwnershipMode::Borrowed => IncanInputAccess::Shared,
            OwnershipMode::BorrowedMut => IncanInputAccess::Exclusive,
            _ => return Err("local alias does not declare shared or exclusive access"),
        };
        Ok(CheckedLocalInputLoan {
            destination: local,
            value_type: &mode.value_type,
            access,
            writable: mode.writable,
            source: calls::project_source(&loan.source)?,
        })
    }
}

impl super::TypeCheckInfo {
    /// Preserve the obligation even when body proof is unavailable, so lowering cannot treat an explicit alias as
    /// an ordinary copied local merely because its completed ownership evidence is missing.
    pub(crate) fn has_local_input_annotation(&self, destination: &CanonicalSymbolId) -> bool {
        self.protocols.explicit_local_modes.contains_key(destination)
    }
}
