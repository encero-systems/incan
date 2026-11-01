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

impl<'a> CheckedInputBody<'a> {
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
