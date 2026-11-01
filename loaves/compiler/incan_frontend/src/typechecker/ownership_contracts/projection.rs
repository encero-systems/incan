//! Authenticate the source body's ownership proof before any native projection consumes it.

use std::collections::BTreeMap;

use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};

use super::{InputFlow, LocalMode, flow};
use crate::typechecker::TypeCheckInfo;

/// A completed source proof belongs to one exact callable. The view never grants authority to a nested closure,
/// default evaluator or another declaration that happens to reuse the same source range or display name.
pub(crate) struct CheckedInputBody<'a> {
    owner: CanonicalSymbolId,
    locals: &'a BTreeMap<CanonicalSymbolId, LocalMode>,
}

impl CheckedInputBody<'_> {
    /// Read a local's already authenticated request; bindings from other bodies are not part of this view.
    pub(crate) fn local_mode(&self, local: &CanonicalSymbolId) -> Option<OwnershipMode> {
        self.locals
            .get(local)
            .filter(|mode| mode.owner == self.owner)
            .map(|mode| mode.mode)
    }
}

impl TypeCheckInfo {
    /// Require the exact declaration, completed flow and matching canonical local facts before lowering uses an
    /// explicit input body. Ordinary bodies return no proof; a missing proof for an explicit obligation is an error.
    pub(crate) fn checked_input_body(
        &self,
        owner: &CanonicalSymbolId,
    ) -> Result<Option<CheckedInputBody<'_>>, &'static str> {
        // ---- Owning declaration and explicit obligations ----
        let required = self.declarations.ownership_contracts.contains_key(owner)
            || self.protocols.explicit_input_flow.contains_key(owner)
            || self
                .protocols
                .explicit_local_modes
                .values()
                .any(|mode| &mode.owner == owner)
            || self
                .protocols
                .explicit_input_calls
                .values()
                .any(|call| call.owner.as_ref() == Some(owner));
        if !required {
            return Ok(None);
        }
        let declaration = self
            .declarations
            .function_bindings_by_span
            .get(&owner.declaration_span)
            .into_iter()
            .chain(self.declarations.method_bindings_by_span.get(&owner.declaration_span))
            .any(|binding| binding.identity.as_ref() == Some(owner));
        if !declaration {
            return Err("explicit input proof has no matching callable declaration");
        }

        // ---- Complete source-flow evidence ----
        let Some(InputFlow::Checked { graph, analysis }) = self.protocols.explicit_input_flow.get(owner) else {
            return Err("explicit input body has no completed source flow proof");
        };
        if &graph.owner != owner {
            return Err("explicit input graph belongs to another callable");
        }
        let recomputed = flow::analyze(graph).map_err(|_| "explicit input body has an invalid source graph")?;
        if &recomputed != analysis || !analysis.conflicts.is_empty() {
            return Err("explicit input body has conflicting or stale source flow evidence");
        }

        // ---- Loan destinations and their source annotations ----
        for (local, loan) in &graph.local_loans {
            let mode = self
                .protocols
                .explicit_local_modes
                .get(local)
                .ok_or("local loan has no checked ownership annotation")?;
            if &mode.owner != owner || local.kind != SemanticSourceTargetKind::Local {
                return Err("local loan annotation belongs to another declaration");
            }
            let loan = graph
                .loans
                .get(loan.0)
                .ok_or("local loan destination has no source loan")?;
            let expected = match mode.mode {
                OwnershipMode::Borrowed => flow::LoanKind::Shared,
                OwnershipMode::BorrowedMut => flow::LoanKind::Exclusive,
                _ => return Err("local loan destination does not declare a borrow"),
            };
            if loan.kind != expected || loan.writable != mode.writable {
                return Err("local loan permissions disagree with its checked annotation");
            }
            if !matches!(loan.source.root, flow::Root::Binding(_)) {
                return Err("local loan cannot retain a bounded invocation temporary");
            }
        }

        // ---- Canonical value declarations and complete borrow coverage ----
        for (local, mode) in self
            .protocols
            .explicit_local_modes
            .iter()
            .filter(|(_, mode)| &mode.owner == owner)
        {
            if local.kind != SemanticSourceTargetKind::Local
                || self.resolved_write_identity(local.declaration_span, &local.declaration_name) != Some(local)
                || self.resolved_write_type(local.declaration_span, &local.declaration_name) != Some(&mode.value_type)
            {
                return Err("local ownership annotation has no matching checked value declaration");
            }
            match mode.mode {
                OwnershipMode::Borrowed | OwnershipMode::BorrowedMut if !graph.local_loans.contains_key(local) => {
                    return Err("local borrow annotation has no source loan destination");
                }
                OwnershipMode::Reference => return Err("returned local dependencies require a later ownership stage"),
                _ => {}
            }
        }
        Ok(Some(CheckedInputBody {
            owner: owner.clone(),
            locals: &self.protocols.explicit_local_modes,
        }))
    }
}
