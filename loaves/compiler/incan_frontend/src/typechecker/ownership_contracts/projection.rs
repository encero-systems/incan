//! Authenticate the source body's ownership proof before any native projection consumes it.

use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolOrigin};

use super::{InputCall, InputCallKind, InputFlow, flow};
use crate::typechecker::TypeCheckInfo;

mod accesses;
mod call_evaluation;
mod calls;
mod locals;
mod operations;
mod transfers;
pub(crate) use calls::{CheckedInputInvocation, CheckedInputSource};

/// A completed source proof belongs to one exact callable. The view never grants authority to a nested closure,
/// default evaluator or another declaration that happens to reuse the same source range or display name.
pub(crate) struct CheckedInputBody<'a> {
    owner: CanonicalSymbolId,
    pub(super) facts: &'a TypeCheckInfo,
    pub(super) graph: &'a flow::Graph,
}

impl CheckedInputBody<'_> {
    /// Retain the source proof's exact owner for the frame profile it publishes.
    pub(crate) fn canonical_owner(&self) -> &CanonicalSymbolId {
        &self.owner
    }

    /// Whether the invocation argument written at `span` passes a closure local to a parameter its callee only calls.
    pub(crate) fn is_closure_argument(&self, span: incan_semantics_core::HirSourceSpan) -> bool {
        self.graph
            .invocations
            .values()
            .flat_map(|invocation| &invocation.arguments)
            .any(|argument| argument.span == span && closure_argument(self.graph, argument))
    }

    /// Select the source checker's exact Copy decision for a checked expression in this body. Native storage may
    /// reject a carrier it cannot preserve, but cannot replace this semantic fact with its own ABI heuristic.
    pub(crate) fn value_is_copy(&self, span: incan_semantics_core::HirSourceSpan) -> Result<bool, &'static str> {
        let ty = self
            .facts
            .expr_type(span)
            .ok_or("input value has no checked source type")?;
        self.facts.checked_input_copy_value(self.canonical_owner(), ty)
    }
    /// Read a local's already authenticated request; bindings from other bodies are not part of this view.
    pub(crate) fn local_mode(&self, local: &CanonicalSymbolId) -> Option<OwnershipMode> {
        self.facts
            .protocols
            .explicit_local_modes
            .get(local)
            .filter(|mode| mode.owner == self.owner)
            .map(|mode| mode.mode)
    }
}

impl TypeCheckInfo {
    /// Authenticate an ordinary header affirmatively, not by absence of explicit metadata. Exact canonical
    /// identity and unchanged parameters preserve declaration ABI. A generic binder that does not change these
    /// parameters is not itself an access change; parameter substitution requires separate retained evidence.
    pub(in crate::typechecker) fn selected_input_contract(
        &self,
        call: &InputCall,
    ) -> Result<Option<&crate::ownership_metadata::CheckedOwnershipContract>, &'static str> {
        let contract = self.declarations.ownership_contracts.get(&call.callee);
        match call.kind {
            // A native call's contract is the one its selected Rust signature gives it.
            InputCallKind::Native => self
                .protocols
                .native_input_contracts
                .get(&call.callee)
                .map(Some)
                .ok_or("native invocation has no contract from its signature"),
            InputCallKind::Explicit => {
                let contract = contract.ok_or("invocation has no canonical ownership contract")?;
                // A returned view or independent owned result is an explicit obligation even when every input slot
                // is ordinary.
                if contract.result_source.is_none()
                    && !crate::ownership_metadata::owned_result(Some(contract))?.is_some()
                    && !contract.modes.iter().any(|mode| {
                        mode.component
                            .parameter
                            .is_some_and(|slot| slot < call.parameters.len())
                    })
                {
                    return Err("invocation has no canonical explicit input slot");
                }
                Ok(Some(contract))
            }
            InputCallKind::Ordinary => {
                if !matches!(
                    call.callee.origin,
                    SymbolOrigin::Module(_) | SymbolOrigin::Package { .. }
                ) {
                    return Err("ordinary invocation requires a canonical source declaration");
                }
                if contract.is_some() || call.owner.is_none() {
                    return Err("ordinary invocation cannot replace an explicit contract");
                }
                let binding = match call.callee.kind {
                    SemanticSourceTargetKind::Function if call.receiver.is_none() => {
                        let mut binding = None;
                        for candidate in self
                            .declarations
                            .function_bindings_by_span
                            .values()
                            .chain(self.declarations.function_bindings.values())
                            .filter(|candidate| candidate.identity.as_ref() == Some(&call.callee))
                        {
                            if binding.is_some_and(|previous| previous != candidate) {
                                return Err("ordinary invocation has conflicting canonical headers");
                            }
                            binding = Some(candidate);
                        }
                        binding.ok_or("ordinary invocation has no canonical source header")?
                    }
                    SemanticSourceTargetKind::Method if call.receiver.is_some() => self
                        .declarations
                        .method_bindings_by_span
                        .get(&call.callee.declaration_span)
                        .filter(|binding| binding.identity.as_ref() == Some(&call.callee))
                        .ok_or("ordinary method invocation has no canonical source header")?,
                    _ => return Err("ordinary invocation syntax differs from its canonical declaration"),
                };
                if binding.is_async || !ordinary_header_matches(&binding.params, &call.parameters) {
                    return Err("ordinary invocation differs from its synchronous concrete header");
                }
                Ok(None)
            }
            InputCallKind::Indirect => {
                if call.callee.kind != SemanticSourceTargetKind::Parameter
                    || contract.is_some()
                    || call.receiver.is_some()
                    || call.owner.is_none()
                {
                    return Err("indirect invocation requires a checked callable binding");
                }
                Ok(None)
            }
        }
    }

    /// A local without an ownership annotation may alias only a returned-view call's result: its loan is that call's
    /// result loan, keyed by the retained invocation, with the callee's declared access over existing storage.
    fn validate_view_local(
        &self,
        graph: &flow::Graph,
        local: &CanonicalSymbolId,
        id: flow::LoanId,
    ) -> Result<(), &'static str> {
        if local.kind != SemanticSourceTargetKind::Local {
            return Err("local loan has no checked ownership annotation");
        }
        let loan = graph.loans.get(id.0).ok_or("returned view local has no result loan")?;
        let call = self
            .protocols
            .explicit_input_calls
            .get(&loan.span)
            .filter(|_| graph.invocations.contains_key(&loan.span))
            .ok_or("local loan has no checked ownership annotation")?;
        let view = crate::ownership_metadata::returned_view(self.selected_input_contract(call)?)?
            .ok_or("returned view local names a call without a returned view")?;
        let expected = if view.access == incan_semantics_core::IncanInputAccess::Exclusive {
            flow::LoanKind::Exclusive
        } else {
            flow::LoanKind::Shared
        };
        if loan.kind != expected || !matches!(loan.source.root, flow::Root::Binding(_)) {
            return Err("returned view local differs from its callee's declared view");
        }
        Ok(())
    }

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
        self.validate_input_invocations(owner, graph)?;

        // ---- Loan destinations and their source annotations ----
        for (local, loan) in &graph.local_loans {
            let Some(mode) = self.protocols.explicit_local_modes.get(local) else {
                self.validate_view_local(graph, local, *loan)?;
                continue;
            };
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

        // ---- Exact owned initializer relationships ----
        for (local, initializer) in &graph.owned_locals {
            let mode = self
                .protocols
                .explicit_local_modes
                .get(local)
                .ok_or("owned initializer has no checked annotation")?;
            if &mode.owner != owner
                || mode.mode != OwnershipMode::Owned
                || self.expr_type(initializer.initializer) != Some(&mode.value_type)
            {
                return Err("owned initializer differs from its checked owner or value type");
            }
            if let Some(index) = initializer.access_node {
                let node = graph
                    .nodes
                    .get(index)
                    .ok_or("owned initializer has no checked source event")?;
                let flow::Event::Access { access, .. } = node.event else {
                    return Err("owned initializer has no read or transfer event");
                };
                let copy = self.checked_input_copy_value(owner, &mode.value_type)?;
                if (copy && access != flow::Access::Read) || (!copy && access != flow::Access::Move) {
                    return Err("owned initializer access differs from its checked Copy or transfer contract");
                }
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
                OwnershipMode::Owned if !graph.owned_locals.contains_key(local) => {
                    return Err("owned local annotation has no retained initializer");
                }
                OwnershipMode::Reference => return Err("returned local dependencies require a later ownership stage"),
                _ => {}
            }
        }
        Ok(Some(CheckedInputBody {
            owner: owner.clone(),
            facts: self,
            graph,
        }))
    }

    /// Bind invocation loans to the selected canonical source signature. A graph's access tag cannot authorize a
    /// different callee, slot, receiver, or a contract unavailable to this checked body.
    fn validate_input_invocations(&self, owner: &CanonicalSymbolId, graph: &flow::Graph) -> Result<(), &'static str> {
        let mut marker_roles = std::collections::BTreeSet::new();
        for (span, invocation) in &graph.invocations {
            let call = self
                .protocols
                .explicit_input_calls
                .get(span)
                .ok_or("invocation has no selected call facts")?;
            if call.owner.as_ref() != Some(owner)
                || call.callee != invocation.callee
                || call.receiver.is_some() != invocation.receiver_evaluation.is_some()
            {
                return Err("invocation differs from its checked caller or signature");
            }
            let mut supplied = std::collections::BTreeSet::new();
            for argument in &invocation.arguments {
                if argument.slot >= call.parameters.len() || !supplied.insert(argument.slot) {
                    return Err("invocation differs from its checked caller or signature");
                }
            }
            if call
                .parameters
                .iter()
                .enumerate()
                .any(|(slot, parameter)| !supplied.contains(&slot) && !parameter.has_default)
            {
                return Err("invocation omits a required selected parameter");
            }
            let contract = self.selected_input_contract(call)?;
            if call.kind == InputCallKind::Indirect {
                for (slot, parameter) in call.parameters.iter().enumerate() {
                    // A closure local passed to a function-typed parameter: checking admits a capturing closure only
                    // where its callee calls it and keeps it no longer.
                    let closure = matches!(parameter.ty, crate::symbols::ResolvedType::Function(..))
                        && invocation
                            .arguments
                            .iter()
                            .find(|argument| argument.slot == slot)
                            .is_some_and(|argument| closure_argument(graph, argument));
                    if parameter.is_mut || !(closure || self.checked_input_copy_value(owner, &parameter.ty)?) {
                        return Err("indirect invocation needs checked immutable Copy inputs");
                    }
                }
            }
            for index in std::iter::once(invocation.completion_node)
                .chain(
                    invocation
                        .receiver_evaluation
                        .iter()
                        .flat_map(|evaluation| [evaluation.start_node, evaluation.end_node]),
                )
                .chain(
                    invocation
                        .arguments
                        .iter()
                        .flat_map(|argument| [argument.evaluation.start_node, argument.evaluation.end_node]),
                )
            {
                if !marker_roles.insert(index) {
                    return Err("invocation marker fulfills more than one evaluation role");
                }
            }
            call_evaluation::validate_invocation_events(
                self,
                graph,
                *span,
                invocation,
                &call.parameters,
                call.kind != InputCallKind::Indirect && call.receiver.is_none(),
            )?;
            for argument in &invocation.arguments {
                let access = crate::ownership_metadata::direct_input_access(contract, argument.slot)?;
                if argument.access != access {
                    return Err("invocation argument access differs from the selected parameter");
                }
                if let Some(id) = argument.loan {
                    let parameter = call
                        .parameters
                        .get(argument.slot)
                        .ok_or("invocation parameter is absent")?;
                    let loan = graph.loans.get(id.0).ok_or("invocation argument loan is absent")?;
                    let writable = access == incan_semantics_core::IncanInputAccess::Exclusive && parameter.is_mut;
                    if writable != loan.writable {
                        return Err("invocation argument write permission differs from the selected parameter");
                    }
                }
            }
            if let Some(id) = invocation.receiver {
                let loan = graph.loans.get(id.0).ok_or("invocation receiver loan is absent")?;
                let mutable = call.receiver == Some(crate::ast::Receiver::Mutable);
                let expected = if mutable {
                    flow::LoanKind::Exclusive
                } else {
                    flow::LoanKind::Shared
                };
                if loan.kind != expected || loan.writable != mutable {
                    return Err("invocation receiver access differs from the selected method");
                }
            } else if invocation.receiver_owned {
                if !self.native_value_receiver(*span) {
                    return Err("owned receiver is not selected by a native value signature");
                }
            } else if call.receiver.is_some() {
                return Err("invocation receiver has no access proof");
            }
        }
        Ok(())
    }
}

/// Authenticate an instantiated ordinary call against its selected declaration without comparing open type
/// variables to their closed call-site types. Expression checking already proved those type substitutions; this
/// boundary owns only the parameter slots and passing shape consumed by the source-flow graph.
fn ordinary_header_matches(
    declared: &[crate::symbols::CallableParam],
    selected: &[crate::symbols::CallableParam],
) -> bool {
    declared.len() == selected.len()
        && declared.iter().zip(selected).all(|(declared, selected)| {
            declared.name == selected.name
                && declared.kind == selected.kind
                && declared.has_default == selected.has_default
                && declared.is_mut == selected.is_mut
                && declared.is_partial_preset == selected.is_partial_preset
        })
}

/// Whether an invocation argument passes a closure local: its source read is of the closure local itself.
pub(super) fn closure_argument(graph: &flow::Graph, argument: &flow::InvocationArgument) -> bool {
    let Some(flow::Event::Access {
        place, through: None, ..
    }) = argument
        .evaluation
        .source_node
        .and_then(|index| graph.nodes.get(index))
        .map(|node| &node.event)
    else {
        return false;
    };
    place.projection.is_empty()
        && graph
            .closures
            .values()
            .any(|closure| matches!(&place.root, flow::Root::Binding(local) if *local == closure.local))
}
