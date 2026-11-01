//! Source-profile eligibility is separate from a successfully constructed ownership graph. A complete graph may
//! describe a source shape whose native storage or cleanup is still deliberately unavailable.

use crate::ast::{BinaryOp, Expr, FunctionDecl, ParamKind, Statement};
use crate::ast_walk::any_expr_in_body;
use crate::typechecker::{CheckedInputBody, TypeCheckInfo};
use incan_semantics_core::{CanonicalSymbolId, IncanInputAccess};

/// Direct synchronous fixed-arity source functions are the first executable frame boundary. Decorated, generic,
/// deferred and escaping signatures retain their existing stage refusals until their native boundary is proven.
pub(in crate::typechecker) fn direct_function_boundary(function: &FunctionDecl) -> bool {
    !function.is_async()
        && function.type_params.is_empty()
        && function.decorators.is_empty()
        && function.result_source.is_none()
        && function
            .params
            .iter()
            .all(|parameter| parameter.node.kind == ParamKind::Normal && parameter.node.default.is_none())
}

impl TypeCheckInfo {
    /// Authenticate complete source flow and the currently implemented storage profile together. An unavailable
    /// body cannot borrow permission from its callee's signature or a previous declaration's successful check.
    pub(crate) fn direct_input_admission<'a>(
        &'a self,
        owner: &CanonicalSymbolId,
        function: &FunctionDecl,
    ) -> Result<Option<CheckedInputBody<'a>>, &'static str> {
        let Some(proof) = self.checked_input_body(owner)? else {
            return Ok(None);
        };
        if !direct_function_boundary(function) {
            return Err("explicit inputs require a synchronous fixed source-function boundary");
        }
        proof.validate_local_storage()?;
        if function.body.iter().any(|statement| {
            !matches!(
                statement.node,
                Statement::Pass
                    | Statement::Assignment(_)
                    | Statement::FieldAssignment(_)
                    | Statement::CompoundAssignment(_)
                    | Statement::Expr(_)
                    | Statement::Return(_)
                    | Statement::Assert(_)
            )
        }) || any_expr_in_body(&function.body, |expression| {
            matches!(
                expression,
                Expr::MethodCall(..) | Expr::Index(..) | Expr::Binary(_, BinaryOp::And | BinaryOp::Or, _)
            )
        }) {
            return Err("explicit inputs require direct straight-line source access in this implementation stage");
        }
        proof.validate_direct_call_storage()?;
        Ok(Some(proof))
    }
}

impl CheckedInputBody<'_> {
    /// Only checked Copy referents with static owner/field places have local alias storage in this stage. An owned
    /// local is a different transfer operation; non-Copy alias reads need no-repair consumer facts of their own.
    fn validate_local_storage(&self) -> Result<(), &'static str> {
        for (local, mode) in self
            .facts
            .protocols
            .explicit_local_modes
            .iter()
            .filter(|(_, mode)| &mode.owner == self.canonical_owner())
        {
            if !matches!(
                mode.mode,
                incan_lang::lang::types::ownership::OwnershipMode::Borrowed
                    | incan_lang::lang::types::ownership::OwnershipMode::BorrowedMut
            ) {
                return Err("explicit owned or returned local needs its own storage projection");
            }
            let loan = self
                .graph
                .local_loans
                .get(local)
                .and_then(|id| self.graph.loans.get(id.0))
                .ok_or("explicit local alias has no checked source loan")?;
            let retained = self.local_loan(local, loan.span)?;
            if !crate::typechecker::semantic_type_from_resolved(retained.value_type)
                .abi_v0_facts()
                .ownership
                .is_trivially_copy()
            {
                return Err("non-Copy local alias requires checked value-consumption projection");
            }
        }
        Ok(())
    }

    /// Require the exact carriers consumed by the first direct-call materializer. A richer source graph is useful
    /// for diagnostics but does not authorize an indexed adaptation, method frame or implicit non-Copy repair.
    fn validate_direct_call_storage(&self) -> Result<(), &'static str> {
        for (span, invocation) in &self.graph.invocations {
            if invocation.receiver.is_some() {
                return Err("explicit method inputs require their checked receiver materialization");
            }
            let call = self
                .facts
                .protocols
                .explicit_input_calls
                .get(span)
                .ok_or("missing explicit call facts")?;
            for argument in &invocation.arguments {
                let parameter = call
                    .parameters
                    .get(argument.slot)
                    .ok_or("missing explicit call parameter")?;
                let copy = crate::typechecker::semantic_type_from_resolved(&parameter.ty)
                    .abi_v0_facts()
                    .ownership
                    .is_trivially_copy();
                if argument.access == IncanInputAccess::Inferred && !copy && argument.source.is_some() {
                    return Err("ordinary non-Copy input requires its inferred access proof");
                }
                if argument.access == IncanInputAccess::Owned
                    && !copy
                    && argument
                        .source
                        .as_ref()
                        .is_some_and(|source| !source.projection.is_empty())
                {
                    return Err("explicit projected transfer requires checked drop obligations");
                }
            }
        }
        Ok(())
    }
}
