//! Source-profile eligibility is separate from a successfully constructed ownership graph. A complete graph may
//! describe a source shape whose native storage or cleanup is still deliberately unavailable.

use super::CallableSource;
use crate::ast::{BinaryOp, Condition, Expr, FunctionDecl, MethodDecl, Param, ParamKind, Spanned, Statement};
use crate::ast_walk::any_expr_in_body;
use crate::typechecker::{CheckedInputBody, TypeCheckInfo};
use incan_semantics_core::{CanonicalSymbolId, IncanInputAccess};

/// Direct synchronous fixed-arity source functions are the first executable frame boundary. Decorated, generic,
/// deferred and escaping signatures retain their existing stage refusals until their native boundary is proven.
pub(in crate::typechecker) fn direct_function_boundary(function: &FunctionDecl) -> bool {
    !function.is_async()
        && function.type_params.is_empty()
        && function.decorators.is_empty()
        && fixed_parameters(&function.params)
}

/// Concrete methods share the function frame boundary; the receiver is a borrowed slot of its own, checked with
/// the method's receiver mode.
pub(in crate::typechecker) fn direct_method_boundary(method: &MethodDecl) -> bool {
    method.body.is_some()
        && !method.is_async()
        && method.type_params.is_empty()
        && method.decorators.is_empty()
        && fixed_parameters(&method.params)
}

/// Every parameter is a required positional-or-named slot.
fn fixed_parameters(parameters: &[Spanned<Param>]) -> bool {
    parameters
        .iter()
        .all(|parameter| parameter.node.kind == ParamKind::Normal && parameter.node.default.is_none())
}

/// The frame boundary of either callable form.
fn callable_boundary(callable: CallableSource<'_>) -> bool {
    match callable {
        CallableSource::Function(function) => direct_function_boundary(function),
        CallableSource::Method(method) => direct_method_boundary(method),
    }
}

/// Recursive source shape only: full CFG/event coverage remains the authority for every retained operation.
fn input_statements_supported(body: &[Spanned<Statement>], conditionals: bool) -> bool {
    body.iter().all(|statement| match &statement.node {
        Statement::Pass
        | Statement::Assignment(_)
        | Statement::FieldAssignment(_)
        | Statement::CompoundAssignment(_)
        | Statement::Expr(_)
        | Statement::Return(_)
        | Statement::Assert(_) => true,
        Statement::If(branch) if conditionals => {
            matches!(branch.condition, Condition::Expr(_))
                && input_statements_supported(&branch.then_body, true)
                && branch
                    .elif_branches
                    .iter()
                    .all(|(_, body)| input_statements_supported(body, true))
                && branch
                    .else_body
                    .as_ref()
                    .is_none_or(|body| input_statements_supported(body, true))
        }
        // Loops share the conditional profile: each exit and back edge is a path of the same flow graph.
        Statement::While(loop_) if conditionals => {
            matches!(loop_.condition, Condition::Expr(_)) && input_statements_supported(&loop_.body, true)
        }
        Statement::Loop(loop_) if conditionals => input_statements_supported(&loop_.body, true),
        Statement::For(loop_) if conditionals => input_statements_supported(&loop_.body, true),
        Statement::Break(None) | Statement::Continue if conditionals => true,
        _ => false,
    })
}

/// Whether a body has control flow of its own, which only the conditional profile can execute.
fn has_control_flow(body: &[Spanned<Statement>]) -> bool {
    body.iter().any(|statement| {
        matches!(
            statement.node,
            Statement::If(_) | Statement::While(_) | Statement::Loop(_) | Statement::For(_)
        )
    })
}

impl TypeCheckInfo {
    /// Authenticate complete source flow and the currently implemented storage profile together. An unavailable
    /// body cannot borrow permission from its callee's signature or a previous declaration's successful check.
    pub(crate) fn direct_input_admission<'a>(
        &'a self,
        owner: &CanonicalSymbolId,
        function: &FunctionDecl,
    ) -> Result<Option<CheckedInputBody<'a>>, &'static str> {
        self.input_projection(owner, CallableSource::Function(function), false)
    }

    /// Admit a concrete method body under the same straight-line profile; its receiver is checked as a borrowed
    /// slot with the method's receiver mode.
    pub(crate) fn method_input_admission<'a>(
        &'a self,
        owner: &CanonicalSymbolId,
        method: &MethodDecl,
    ) -> Result<Option<CheckedInputBody<'a>>, &'static str> {
        self.input_projection(owner, CallableSource::Method(method), false)
    }

    /// The executable profile of a function body: profile 1 for a straight-line body, else profile 2 when its
    /// statement-ifs project completely. The checker's execution gate and Body IR select the same profile.
    pub(crate) fn executable_function_input<'a>(
        &'a self,
        owner: &CanonicalSymbolId,
        function: &FunctionDecl,
    ) -> Result<Option<(u32, CheckedInputBody<'a>)>, &'static str> {
        self.executable_input(owner, CallableSource::Function(function))
    }

    /// The executable profile of a concrete method body, selected as for functions.
    pub(crate) fn executable_method_input<'a>(
        &'a self,
        owner: &CanonicalSymbolId,
        method: &MethodDecl,
    ) -> Result<Option<(u32, CheckedInputBody<'a>)>, &'static str> {
        self.executable_input(owner, CallableSource::Method(method))
    }

    /// A body whose straight-line projection is refused executes under profile 2 only if it has control flow of its
    /// own (statement-ifs or loops) and its conditional projection is complete. A body without control flow reports
    /// the straight-line refusal; one with control flow reports why its conditional projection is refused.
    fn executable_input<'a>(
        &'a self,
        owner: &CanonicalSymbolId,
        callable: CallableSource<'_>,
    ) -> Result<Option<(u32, CheckedInputBody<'a>)>, &'static str> {
        let reason = match self.input_projection(owner, callable, false) {
            Ok(proof) => return Ok(proof.map(|proof| (1, proof))),
            Err(reason) => reason,
        };
        if !has_control_flow(callable.body()) {
            return Err(reason);
        }
        match self.input_projection(owner, callable, true) {
            Ok(Some(proof)) => Ok(Some((2, proof))),
            Ok(None) => Err(reason),
            Err(conditional) => Err(conditional),
        }
    }

    /// Retain a fully authenticated conditional source projection, the version-2 profile of a body with
    /// statement-ifs. `executable_function_input` selects it when the straight-line profile is refused.
    pub(crate) fn conditional_input_projection<'a>(
        &'a self,
        owner: &CanonicalSymbolId,
        function: &FunctionDecl,
    ) -> Result<Option<CheckedInputBody<'a>>, &'static str> {
        self.input_projection(owner, CallableSource::Function(function), true)
    }

    /// Both representations require the same source/storage authority; only statement-if structure differs.
    /// Method calls are admitted only as checked invocations: a complete source graph carries one for each.
    fn input_projection<'a>(
        &'a self,
        owner: &CanonicalSymbolId,
        callable: CallableSource<'_>,
        conditionals: bool,
    ) -> Result<Option<CheckedInputBody<'a>>, &'static str> {
        let Some(proof) = self.checked_input_body(owner)? else {
            return Ok(None);
        };
        if !callable_boundary(callable) {
            return Err("explicit inputs require a synchronous fixed source-callable boundary");
        }
        proof.validate_local_storage()?;
        if !input_statements_supported(callable.body(), conditionals)
            || any_expr_in_body(callable.body(), |expression| {
                matches!(
                    expression,
                    Expr::Index(..) | Expr::Binary(_, BinaryOp::And | BinaryOp::Or, _)
                )
            })
        {
            return Err("explicit inputs require direct straight-line source access in this implementation stage");
        }
        proof.validate_direct_call_storage()?;
        proof.validate_assertion_transfers(callable.body())?;
        proof.validate_operation_coverage(callable)?;
        Ok(Some(proof))
    }
}

impl CheckedInputBody<'_> {
    /// An assertion message executes only on failure. Moving a stored owner there needs conditional cleanup even
    /// though the surrounding source contains no `if`; fresh failure-local values and Copy inputs need no such join.
    fn validate_assertion_transfers(&self, body: &[Spanned<Statement>]) -> Result<(), &'static str> {
        for statement in body {
            if let Statement::If(branch) = &statement.node {
                self.validate_assertion_transfers(&branch.then_body)?;
                for (_, body) in &branch.elif_branches {
                    self.validate_assertion_transfers(body)?;
                }
                if let Some(body) = &branch.else_body {
                    self.validate_assertion_transfers(body)?;
                }
            }
            let Statement::Assert(assertion) = &statement.node else {
                continue;
            };
            let Some(message) = &assertion.message else {
                continue;
            };
            for (span, invocation) in &self.graph.invocations {
                if span.expansion != message.span.expansion
                    || span.start < message.span.start
                    || span.end > message.span.end
                {
                    continue;
                }
                let call = self
                    .facts
                    .protocols
                    .explicit_input_calls
                    .get(span)
                    .ok_or("assertion call has no checked inputs")?;
                for argument in &invocation.arguments {
                    if argument.access != IncanInputAccess::Owned
                        || !argument
                            .source
                            .as_ref()
                            .is_some_and(|source| matches!(source.root, super::flow::Root::Binding(_)))
                    {
                        continue;
                    }
                    let parameter = call
                        .parameters
                        .get(argument.slot)
                        .ok_or("assertion input has no checked slot")?;
                    if !self
                        .facts
                        .checked_input_copy_value(self.canonical_owner(), &parameter.ty)?
                    {
                        return Err("explicit stored transfer in an assertion message requires conditional cleanup");
                    }
                }
            }
        }
        Ok(())
    }

    /// Local aliases preserve existing storage. Owned locals use their exact initializer relationship; a whole
    /// non-Copy owner may transfer, while projected transfers still require partial-drop obligations.
    fn validate_local_storage(&self) -> Result<(), &'static str> {
        for (local, mode) in self
            .facts
            .protocols
            .explicit_local_modes
            .iter()
            .filter(|(_, mode)| &mode.owner == self.canonical_owner())
        {
            if mode.mode == incan_lang::lang::types::ownership::OwnershipMode::Owned {
                let initializer = self
                    .graph
                    .owned_locals
                    .get(local)
                    .ok_or("owned local has no checked initializer")?;
                let retained = self.owned_local(local, initializer.initializer)?;
                let copy = crate::typechecker::semantic_type_from_resolved(retained.value_type)
                    .abi_v0_facts()
                    .ownership
                    .is_trivially_copy();
                if initializer.source.is_some()
                    && copy
                        != self
                            .facts
                            .checked_input_copy_value(self.canonical_owner(), retained.value_type)?
                {
                    return Err("checked Copy semantics require a compatible native input carrier");
                }
                if !copy
                    && initializer
                        .source
                        .as_ref()
                        .is_some_and(|source| !source.projection.is_empty())
                {
                    return Err("explicit projected transfer requires checked drop obligations");
                }
                continue;
            }
            if !matches!(
                mode.mode,
                incan_lang::lang::types::ownership::OwnershipMode::Borrowed
                    | incan_lang::lang::types::ownership::OwnershipMode::BorrowedMut
            ) {
                return Err("explicit returned local needs its own storage projection");
            }
            let loan = self
                .graph
                .local_loans
                .get(local)
                .and_then(|id| self.graph.loans.get(id.0))
                .ok_or("explicit local alias has no checked source loan")?;
            self.local_loan(local, loan.span)?;
        }
        Ok(())
    }

    /// Require the exact carriers consumed by the first direct-call materializer. A richer source graph is useful
    /// for diagnostics but does not authorize an indexed adaptation or implicit non-Copy repair. A method receiver
    /// is borrowed from existing owner storage; a temporary receiver needs its own bounded owner.
    fn validate_direct_call_storage(&self) -> Result<(), &'static str> {
        for (span, invocation) in &self.graph.invocations {
            if let Some(receiver) = invocation.receiver {
                let loan = self
                    .graph
                    .loans
                    .get(receiver.0)
                    .ok_or("explicit method receiver has no checked loan")?;
                if !matches!(loan.source.root, super::flow::Root::Binding(_)) {
                    return Err("explicit method receiver requires existing owner storage");
                }
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
                if argument.source.is_some()
                    && matches!(argument.access, IncanInputAccess::Owned | IncanInputAccess::Inferred)
                    && copy
                        != self
                            .facts
                            .checked_input_copy_value(self.canonical_owner(), &parameter.ty)?
                {
                    return Err("checked Copy semantics require a compatible native input carrier");
                }
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
