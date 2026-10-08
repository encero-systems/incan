//! Pre-execution provider-host availability across retained Body-IR computations.
//!
//! This pass checks availability, not authority. It never runs a default, polls a generator or task, or invokes a
//! provider. Named calls use the same exact declaration resolver as runtime dispatch; unrelated module bodies and
//! imported targets are not an excuse to infer new execution support.

use std::collections::BTreeSet;

use incan_semantics_core::CompilerNodeId;
use incan_semantics_core::body_ir::{
    Body, BodyIrModule, CallableParam, CallableParamDefault, CallableTarget, Callee, Rvalue, Statement, StatementKind,
};

use super::{
    ReplacementExecutionError, named_callable_body, provider::ProviderRuntime, unsupported,
    validate_argument_binding_profile,
};

/// Refuse missing hosts in the selected body's nested computations and reachable same-module callees.
///
/// Availability is conservative, like structural admission: defaults, untaken branches and unpolled frames are
/// inspected without being executed. A worklist breaks recursive call cycles without omitting the rest of a body.
pub(super) fn validate(
    module: &BodyIrModule,
    entry: &Body,
    providers: Option<&ProviderRuntime>,
) -> Result<(), ReplacementExecutionError> {
    let mut preflight = ProviderHostPreflight {
        module,
        providers,
        pending: vec![entry],
        visited: BTreeSet::new(),
    };
    while let Some(body) = preflight.pending.pop() {
        if preflight.visited.insert(&body.direct_call_id) {
            preflight.parameters(&body.params)?;
            preflight.statements(&body.block.stmts)?;
        }
    }
    Ok(())
}

/// Borrowed traversal state; visited declaration identities bound recursion without storing runtime evidence.
struct ProviderHostPreflight<'module, 'runtime> {
    module: &'module BodyIrModule,
    providers: Option<&'runtime ProviderRuntime>,
    pending: Vec<&'module Body>,
    visited: BTreeSet<&'module CompilerNodeId>,
}

impl<'module> ProviderHostPreflight<'module, '_> {
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
                    && target.direct_call_id.is_some()
                    && args.iter().all(|argument| argument.as_one().is_some())
                    && validate_argument_binding_profile(&target.binding) =>
                {
                    // Invalid bindings and spreads belong to the caller's structural gate, not the target's host.
                    self.pending
                        .push(named_callable_body(self.module, target, statement.span)?);
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
