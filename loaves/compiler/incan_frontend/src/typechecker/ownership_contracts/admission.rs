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
        if self
            .protocols
            .explicit_local_modes
            .values()
            .any(|mode| &mode.owner == owner)
        {
            return Err("explicit local ownership still requires source-place storage projection");
        }
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
