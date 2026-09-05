//! Pre-execution reachability and provider-host availability across retained Body-IR computations.
//!
//! This pass validates the structural execution profile of every reachable same-module body and checks provider-host
//! availability, but not provider authority. It never runs a default, polls a generator or task, or invokes a
//! provider. Named calls use the same exact declaration resolver as runtime dispatch; unrelated module bodies and
//! imported targets are not an excuse to infer new execution support.

use std::collections::BTreeSet;

use incan_semantics_core::body_ir::{
    Body, BodyIrModule, CallableParam, CallableParamDefault, CallableTarget, Callee, NamedCallableTarget, Rvalue,
    Statement, StatementKind,
};
use incan_semantics_core::{CompilerNodeId, HirSourceSpan};

use super::{
    ReplacementExecutionError, named_callable_body, provider::ProviderRuntime, unsupported,
    validate_argument_binding_profile, validate_direct_body_profile,
};

/// Refuse unsupported body profiles and missing hosts across the selected reachable computation.
///
/// Preflight is conservative: defaults, untaken branches and unpolled frames are inspected without being executed. A
/// worklist breaks recursive call cycles without omitting the rest of a body.
pub(super) fn validate(
    module: &BodyIrModule,
    reachable: &[BodyIrModule],
    entry: &Body,
    providers: Option<&ProviderRuntime>,
) -> Result<(), ReplacementExecutionError> {
    let mut preflight = ExecutionPreflight {
        module,
        reachable,
        providers,
        pending: vec![(module, entry)],
        visited: BTreeSet::new(),
    };
    while let Some((owner, body)) = preflight.pending.pop() {
        if preflight.visited.insert(&body.direct_call_id) {
            // A refusal raised here carries a span measured in `owner`, which is not always the entrypoint's module
            // once a call can leave it. Recording the module keeps the reported location and the reported span
            // describing the same file.
            let owner_id = owner.module_id.path();
            validate_direct_body_profile(body).map_err(|error| error.measured_in_module(owner_id))?;
            preflight
                .parameters(&body.params)
                .map_err(|error| error.measured_in_module(owner_id))?;
            preflight
                .statements(&body.block.stmts)
                .map_err(|error| error.measured_in_module(owner_id))?;
        }
    }
    Ok(())
}

/// Borrowed traversal state; visited declaration identities bound recursion without storing runtime evidence.
struct ExecutionPreflight<'module, 'runtime> {
    module: &'module BodyIrModule,
    /// Modules other than the entry's that a resolved call may reach.
    ///
    /// Preflight has to follow a call across a module edge for the same reason it follows one inside a module: an
    /// admitted profile is proved before any program effect runs, and a body left unvisited is a body whose refusals
    /// would surface part-way through execution instead.
    reachable: &'module [BodyIrModule],
    providers: Option<&'runtime ProviderRuntime>,
    pending: Vec<(&'module BodyIrModule, &'module Body)>,
    visited: BTreeSet<&'module CompilerNodeId>,
}

impl<'module> ExecutionPreflight<'module, '_> {
    /// Resolve one named callee to the body preflight must also validate.
    ///
    /// A same-module call keeps the existing route: `direct_call_id` is a span identity that exists only for a
    /// declaration physically present in this module, so its presence already proves locality and
    /// `named_callable_body` applies the checks it always did.
    ///
    /// An imported callee has no such span identity and resolves on the canonical identity the typechecker selected,
    /// never on its spelling. Preflight must follow it: refusing here because the callee is elsewhere would make an
    /// imported body's refusals surface part-way through execution, which is the property this pass exists to
    /// prevent.
    fn resolve_callee(
        &self,
        target: &NamedCallableTarget,
        span: HirSourceSpan,
    ) -> Result<(&'module BodyIrModule, &'module Body), ReplacementExecutionError> {
        if target.direct_call_id.is_some() {
            return named_callable_body(self.module, target, span).map(|body| (self.module, body));
        }
        let canonical = target.canonical.as_ref().ok_or_else(|| {
            unsupported(
                format!(
                    "named callable `{}` without a canonical declaration target",
                    target.name
                ),
                span,
            )
        })?;
        let mut resolved = self
            .reachable
            .iter()
            .filter_map(|module| module.body_for_canonical_target(canonical).map(|body| (module, body)));
        let resolved_body = resolved.next().ok_or_else(|| {
            unsupported(
                format!(
                    "named callable `{}` resolves to a declaration outside this execution graph",
                    target.name
                ),
                span,
            )
        })?;
        if resolved.next().is_some() {
            return Err(unsupported(
                format!(
                    "named callable `{}` resolves to more than one module in this execution graph",
                    target.name
                ),
                span,
            ));
        }
        Ok(resolved_body)
    }

    /// Inspect retained source defaults without evaluating them or substituting partial presets.
    fn parameters(&mut self, params: &'module [CallableParam]) -> Result<(), ReplacementExecutionError> {
        for parameter in params {
            if let CallableParamDefault::Source(computation) = &parameter.default {
                self.statements(&computation.stmts)?;
            }
        }
        Ok(())
    }

    /// Visit every statement-owned computation, retaining the provider plan's own diagnostic span.
    fn statements(&mut self, statements: &'module [Statement]) -> Result<(), ReplacementExecutionError> {
        for statement in statements {
            match &statement.kind {
                StatementKind::Call {
                    callee: Callee::ProviderOperation(plan),
                    ..
                } => {
                    if !self.providers.is_some_and(|runtime| runtime.resolves(&plan.operation)) {
                        return Err(unsupported(
                            format!(
                                "provider operation `{}` that no provider host in this run executes",
                                plan.operation.declaration_name
                            ),
                            plan.call_span,
                        ));
                    }
                }
                StatementKind::Call {
                    callee: Callee::Function(CallableTarget::Named(target)),
                    args,
                    ..
                } if target.builtin.is_none()
                    && (target.direct_call_id.is_some() || target.canonical.is_some())
                    && args.iter().all(|argument| argument.as_one().is_some())
                    && validate_argument_binding_profile(&target.binding) =>
                {
                    // Invalid bindings and spreads belong to the caller's structural gate, not the target's host.
                    self.pending.push(self.resolve_callee(target, statement.span)?);
                }
                StatementKind::Assign { rvalue, .. } => self.rvalue(rvalue)?,
                StatementKind::If {
                    then_block, else_block, ..
                } => {
                    self.statements(&then_block.stmts)?;
                    if let Some(else_block) = else_block {
                        self.statements(&else_block.stmts)?;
                    }
                }
                StatementKind::Loop { body } => self.statements(&body.stmts)?,
                StatementKind::Race { arms, .. } => {
                    for arm in arms {
                        self.statements(&arm.body.stmts)?;
                    }
                }
                StatementKind::Call { .. }
                | StatementKind::Drop { .. }
                | StatementKind::Break { .. }
                | StatementKind::Continue
                | StatementKind::Return { .. }
                | StatementKind::Await { .. }
                | StatementKind::Yield { .. }
                | StatementKind::Assert { .. }
                | StatementKind::Expr { .. }
                | StatementKind::TryPropagate { .. }
                | StatementKind::IterNext { .. }
                | StatementKind::Unsupported { .. } => {}
            }
        }
        Ok(())
    }

    /// Inspect deferred rvalues without confusing construction with execution.
    fn rvalue(&mut self, rvalue: &'module Rvalue) -> Result<(), ReplacementExecutionError> {
        match rvalue {
            Rvalue::Closure { params, body, .. } => {
                self.parameters(params)?;
                self.statements(&body.stmts)?;
            }
            Rvalue::Generator { body, .. } => self.statements(&body.stmts)?,
            Rvalue::Match { arms, .. } => {
                for arm in arms {
                    self.statements(&arm.guard_stmts)?;
                    self.statements(&arm.body_stmts)?;
                }
            }
            Rvalue::Use(_)
            | Rvalue::UnaryOp(..)
            | Rvalue::BinaryOp(..)
            | Rvalue::IsInstance { .. }
            | Rvalue::Aggregate(..)
            | Rvalue::Dict(_)
            | Rvalue::ValueEnumVariant(_)
            | Rvalue::FieldlessEnumVariant(_)
            | Rvalue::ResultVariant(_)
            | Rvalue::Format(_) => {}
        }
        Ok(())
    }
}
