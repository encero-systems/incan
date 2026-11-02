//! Project a checked local loan into its existing storage dependency without inventing a reference-valued owner.

use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, IncanInputAccess};

use super::{CheckedInputBody, CheckedInputSource, calls};
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

    /// The access of an unannotated local bound to a returned view, when its loan is exactly its initializer
    /// call's result loan. Annotated locals and ordinary bindings return `None`.
    pub(crate) fn view_local(
        &self,
        destination: &CanonicalSymbolId,
        initializer: HirSourceSpan,
    ) -> Result<Option<IncanInputAccess>, &'static str> {
        if self.facts.protocols.explicit_local_modes.contains_key(destination) {
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
        if loan.span != initializer {
            return Err("returned view local differs from its initializer call");
        }
        Ok(Some(match loan.kind {
            super::flow::LoanKind::Exclusive => IncanInputAccess::Exclusive,
            super::flow::LoanKind::Shared => IncanInputAccess::Shared,
        }))
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
