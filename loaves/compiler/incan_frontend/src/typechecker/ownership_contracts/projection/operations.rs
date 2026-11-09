//! Source-backed coverage for operation-duration dependencies in the admitted straight-line profile.

use std::collections::BTreeSet;

use incan_lang::lang::builtins::BuiltinFnId;
use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, IncanInputAccess, SemanticSourceTargetKind};

use super::{CheckedInputBody, flow};
use crate::ast::{AssertKind, CallArg, Condition, Expr, FStringPart, ListEntry, MatchBody, Spanned, Statement};
use crate::typechecker::ownership_contracts::CallableSource;

mod paths;

/// Separate source operations from selected calls while sharing one reachable AST traversal.
#[derive(Default)]
struct SourceCoverage {
    operations: BTreeSet<HirSourceSpan>,
    invocations: BTreeSet<HirSourceSpan>,
}

impl CheckedInputBody<'_> {
    /// Select the retained operand relationship after graph validation. The enclosing source admission separately
    /// proves complete AST coverage, so this lookup cannot grant execution to an orphan or missing body proof.
    pub(crate) fn operation_operand_source(
        &self,
        operand: HirSourceSpan,
    ) -> Result<Option<super::CheckedInputSource>, &'static str> {
        let mut matches = self
            .graph
            .operations
            .values()
            .flat_map(|operation| &operation.operands)
            .filter(|dependency| dependency.operand == operand);
        let Some(dependency) = matches.next() else {
            return Ok(None);
        };
        if matches.next().is_some() {
            return Err("operand belongs to multiple operation dependencies");
        }
        if self
            .facts
            .expr_type(operand)
            .is_some_and(|ty| ty.tuple_elements().is_some())
            && self
                .facts
                .resolved_identity(operand)
                .and_then(|identity| self.graph.local_loans.get(identity))
                .and_then(|loan| self.graph.loans.get(loan.0))
                .is_some_and(|loan| self.facts.selected_carried_view(loan.span).is_some())
        {
            return Ok(Some(super::CheckedInputSource::CarriedLocal));
        }
        super::calls::project_source(&dependency.source).map(Some)
    }

    /// Require every admitted borrowed operand's operation record, including after restoring persisted checker
    /// facts. Recomputed liveness over a graph with removed consumption reads cannot substitute for this coverage.
    pub(in crate::typechecker::ownership_contracts) fn validate_operation_coverage(
        &self,
        function: CallableSource<'_>,
    ) -> Result<(), &'static str> {
        let mut seen = SourceCoverage::default();
        let parameters = self.borrowed_operation_parameters(function)?;
        let execution = paths::execution_order(self.graph, function)?;
        self.visit_operation_statements(function.body(), &parameters, &execution, &mut seen)?;
        if seen.operations.len() != self.graph.operations.len() {
            return Err("operation dependency has no matching source operation");
        }
        if seen.invocations.len() != self.graph.invocations.len() {
            return Err("invocation dependency has no matching source call");
        }
        self.validate_source_access_coverage(function)?;
        Ok(())
    }

    /// Validate each reachable source region, including conditional evidence that remains non-executable until its
    /// cleanup profile is supported. The result says whether the block can reach its following statement; this
    /// traversal never grants the outer admission boundary permission to widen.
    fn visit_operation_statements(
        &self,
        statements: &[Spanned<Statement>],
        parameters: &BTreeSet<CanonicalSymbolId>,
        execution: &[usize],
        seen: &mut SourceCoverage,
    ) -> Result<bool, &'static str> {
        for statement in statements {
            match &statement.node {
                Statement::Pass => {}
                Statement::Assignment(value) => {
                    self.visit_operation_expression(&value.value, parameters, execution, seen)?
                }
                Statement::TupleUnpack(value) => {
                    self.visit_operation_expression(&value.value, parameters, execution, seen)?
                }
                Statement::CompoundAssignment(value) => {
                    self.visit_operation_expression(&value.value, parameters, execution, seen)?
                }
                Statement::FieldAssignment(value) => {
                    self.visit_operation_expression(&value.object, parameters, execution, seen)?;
                    self.visit_operation_expression(&value.value, parameters, execution, seen)?;
                }
                // Every arm starts from the evaluated scrutinee; the match reaches what follows when one arm does.
                Statement::Expr(Spanned {
                    node: Expr::Match(scrutinee, arms),
                    ..
                }) => {
                    self.visit_operation_expression(scrutinee, parameters, execution, seen)?;
                    let mut reaches = false;
                    for arm in arms {
                        if let Some(guard) = &arm.node.guard {
                            self.visit_operation_expression(guard, parameters, execution, seen)?;
                        }
                        reaches |= match &arm.node.body {
                            MatchBody::Expr(value) => {
                                self.visit_operation_expression(value, parameters, execution, seen)?;
                                true
                            }
                            MatchBody::Block(body) => {
                                self.visit_operation_statements(body, parameters, execution, seen)?
                            }
                        };
                    }
                    if !reaches {
                        return Ok(false);
                    }
                }
                Statement::Expr(value) => self.visit_operation_expression(value, parameters, execution, seen)?,
                Statement::Return(value) => {
                    if let Some(value) = value {
                        self.visit_operation_expression(value, parameters, execution, seen)?;
                    }
                    return Ok(false);
                }
                Statement::Assert(value) => {
                    let AssertKind::Condition(condition) = &value.kind else {
                        return Err("operation coverage requires a checked ordinary assertion");
                    };
                    self.visit_operation_expression(condition, parameters, execution, seen)?;
                    if let Some(message) = &value.message {
                        self.visit_operation_expression(message, parameters, execution, seen)?;
                    }
                }
                Statement::If(branch) => {
                    let Condition::Expr(condition) = &branch.condition else {
                        return Err("operation coverage requires a checked expression condition");
                    };
                    self.visit_operation_expression(condition, parameters, execution, seen)?;
                    let mut reaches =
                        self.visit_operation_statements(&branch.then_body, parameters, execution, seen)?;
                    for (condition, body) in &branch.elif_branches {
                        self.visit_operation_expression(condition, parameters, execution, seen)?;
                        reaches |= self.visit_operation_statements(body, parameters, execution, seen)?;
                    }
                    reaches |= match &branch.else_body {
                        Some(body) => self.visit_operation_statements(body, parameters, execution, seen)?,
                        None => true,
                    };
                    if !reaches {
                        return Ok(false);
                    }
                }
                // A `while` or `for` loop can always leave through its condition or exhausted poll; a `loop` only
                // through a `break` of its own.
                Statement::While(loop_) => {
                    let Condition::Expr(condition) = &loop_.condition else {
                        return Err("operation coverage requires a checked expression condition");
                    };
                    self.visit_operation_expression(condition, parameters, execution, seen)?;
                    self.visit_operation_statements(&loop_.body, parameters, execution, seen)?;
                }
                Statement::For(loop_) => {
                    self.visit_operation_expression(&loop_.iter, parameters, execution, seen)?;
                    self.visit_operation_statements(&loop_.body, parameters, execution, seen)?;
                }
                Statement::Loop(loop_) => {
                    self.visit_operation_statements(&loop_.body, parameters, execution, seen)?;
                    if !breaks_out(&loop_.body) {
                        return Ok(false);
                    }
                }
                Statement::Break(None) | Statement::Continue => return Ok(false),
                _ => return Err("operation coverage requires the supported statement profile"),
            }
        }
        Ok(true)
    }

    /// Source parameter slots and canonical binding tokens select borrowed inputs; a graph permission bit or a
    /// parameter's display name alone cannot authorize removing an owned snapshot.
    fn borrowed_operation_parameters(
        &self,
        function: CallableSource<'_>,
    ) -> Result<BTreeSet<CanonicalSymbolId>, &'static str> {
        let contract = self.facts.declarations.ownership_contracts.get(self.canonical_owner());
        let mut parameters = BTreeSet::new();
        for (slot, parameter) in function.params().iter().enumerate() {
            if matches!(
                crate::ownership_metadata::direct_input_access(contract, slot)?,
                IncanInputAccess::Shared | IncanInputAccess::Exclusive
            ) {
                let identity = self
                    .facts
                    .resolved_write_identity(parameter.span, &parameter.node.name)
                    .filter(|identity| identity.kind == SemanticSourceTargetKind::Parameter)
                    .ok_or("borrowed operation parameter has no canonical binding")?;
                parameters.insert(identity.clone());
            }
        }
        Ok(parameters)
    }

    /// Follow the supported expression forms and their operand order. This is an admission validator: extending
    /// source-flow coverage does not automatically grant execution to another expression or evaluation policy.
    fn visit_operation_expression(
        &self,
        expression: &Spanned<Expr>,
        parameters: &BTreeSet<CanonicalSymbolId>,
        execution: &[usize],
        seen: &mut SourceCoverage,
    ) -> Result<(), &'static str> {
        match &expression.node {
            Expr::Ident(_) | Expr::SelfExpr | Expr::Literal(_) => {}
            // `?` evaluates its carrier; the propagating path leaves the body without a further operation.
            Expr::Paren(inner) | Expr::Unary(_, inner) | Expr::Field(inner, _) | Expr::Try(inner) => {
                self.visit_operation_expression(inner, parameters, execution, seen)?;
            }
            Expr::Index(base, index) => {
                self.visit_operation_expression(base, parameters, execution, seen)?;
                self.visit_operation_expression(index, parameters, execution, seen)?;
            }
            Expr::Binary(left, _, right) => {
                self.visit_operation_expression(left, parameters, execution, seen)?;
                self.visit_operation_expression(right, parameters, execution, seen)?;
                self.require_operation(
                    expression.span,
                    &[left.as_ref(), right.as_ref()],
                    parameters,
                    execution,
                    seen,
                )?;
            }
            Expr::FString(parts) => {
                let operands = parts
                    .iter()
                    .filter_map(|part| match part {
                        FStringPart::Expr { expr, .. } => Some(expr),
                        FStringPart::Literal(_) => None,
                    })
                    .collect::<Vec<_>>();
                for operand in &operands {
                    self.visit_operation_expression(operand, parameters, execution, seen)?;
                }
                self.require_operation(expression.span, &operands, parameters, execution, seen)?;
            }
            Expr::Call(callee, _, arguments) => {
                self.visit_call_operation(expression, callee, arguments, parameters, execution, seen)?
            }
            Expr::MethodCall(receiver, _, _, arguments) => {
                self.visit_method_operation(expression, receiver, arguments, parameters, execution, seen)?
            }
            Expr::List(entries) => {
                for entry in entries {
                    let ListEntry::Element(value) = entry else {
                        return Err("operation coverage cannot infer list-spread access");
                    };
                    self.visit_operation_expression(value, parameters, execution, seen)?;
                }
            }
            Expr::Tuple(elements) => {
                for element in elements {
                    self.visit_operation_expression(element, parameters, execution, seen)?;
                }
            }
            Expr::ListComp(comprehension) => {
                self.visit_comprehension(
                    &comprehension.clauses,
                    &[&comprehension.expr],
                    parameters,
                    execution,
                    seen,
                )?;
            }
            Expr::DictComp(comprehension) => {
                self.visit_comprehension(
                    &comprehension.clauses,
                    &[&comprehension.key, &comprehension.value],
                    parameters,
                    execution,
                    seen,
                )?;
            }
            // A generator evaluates only its first iterable when it is created. Its produced value, filters and
            // later clauses run when a terminal reader advances it and are represented by the retained result.
            Expr::Generator(generator) => {
                let Some(crate::ast::ComprehensionClause::For { iter, .. }) = generator.clauses.first() else {
                    return Err("operation coverage requires a generator source");
                };
                self.visit_operation_expression(iter, parameters, execution, seen)?;
            }
            // A bound closure's body runs in its own frame when the closure is called; building it reads or borrows
            // only its captures, which its retained closure record covers.
            Expr::Closure(..) if self.graph.closures.contains_key(&expression.span) => {}
            // `await` suspends on its call; the call is the operation.
            Expr::Surface(surface) if super::super::builtins::awaited(surface).is_some() => {
                if let Some(awaited) = super::super::builtins::awaited(surface) {
                    self.visit_operation_expression(awaited, parameters, execution, seen)?;
                }
            }
            _ => return Err("operation coverage requires an admitted expression"),
        }
        Ok(())
    }

    /// Validate one free call from its selected builtin operands or explicit invocation record.
    fn visit_call_operation(
        &self,
        expression: &Spanned<Expr>,
        callee: &Spanned<Expr>,
        arguments: &[CallArg],
        parameters: &BTreeSet<CanonicalSymbolId>,
        execution: &[usize],
        seen: &mut SourceCoverage,
    ) -> Result<(), &'static str> {
        self.visit_operation_expression(callee, parameters, execution, seen)?;
        let builtin = self.facts.resolved_builtin_call(expression.span);
        let mut operands = Vec::new();
        for (index, argument) in arguments.iter().enumerate() {
            let (CallArg::Positional(value) | CallArg::Named(_, value)) = argument else {
                return Err("operation coverage needs checked fixed call arguments");
            };
            if builtin == Some(BuiltinFnId::IsInstance) && index == 1 {
                continue;
            }
            self.visit_operation_expression(value, parameters, execution, seen)?;
            operands.push(value);
        }
        if builtin.is_some() {
            return self.require_operation(expression.span, &operands, parameters, execution, seen);
        }
        if self.facts.protocols.explicit_input_calls.contains_key(&expression.span) {
            self.record_invocation(expression.span, arguments, seen)?;
        }
        Ok(())
    }

    /// Validate one method call using its selected lazy, builtin-list, view or ordinary invocation boundary.
    fn visit_method_operation(
        &self,
        expression: &Spanned<Expr>,
        receiver: &Spanned<Expr>,
        arguments: &[CallArg],
        parameters: &BTreeSet<CanonicalSymbolId>,
        execution: &[usize],
        seen: &mut SourceCoverage,
    ) -> Result<(), &'static str> {
        self.visit_operation_expression(receiver, parameters, execution, seen)?;
        let retained = super::super::builtins::retained_iterator_method(self.facts, expression.span).is_some();
        let list = super::super::builtins::list_method_writes(self.facts, expression.span).is_some();
        let view = super::super::builtins::byte_slice_view(self.facts, expression).is_some();
        if view {
            return Ok(());
        }
        for argument in arguments {
            let value = match argument {
                CallArg::Positional(value) => value,
                CallArg::Named(_, value) if !list => value,
                _ if list => return Err("operation coverage needs positional list method arguments"),
                _ => return Err("operation coverage needs checked fixed call arguments"),
            };
            self.visit_operation_expression(value, parameters, execution, seen)?;
        }
        if retained || list {
            return Ok(());
        }
        if !self.facts.native_associated_call(expression.span) {
            self.authenticate_method_receiver(expression, receiver)?;
        }
        self.record_invocation(expression.span, arguments, seen)
    }

    /// Prove that a checked borrowed receiver names the written place or its authenticated chained temporary.
    fn authenticate_method_receiver(
        &self,
        expression: &Spanned<Expr>,
        receiver: &Spanned<Expr>,
    ) -> Result<(), &'static str> {
        let invocation = self
            .graph
            .invocations
            .get(&expression.span)
            .ok_or("method call has no checked invocation")?;
        if invocation.receiver_owned {
            return Ok(());
        }
        let loan = invocation
            .receiver
            .and_then(|loan| self.graph.loans.get(loan.0))
            .ok_or("method call has no checked receiver loan")?;
        let inner = super::super::builtins::view_call(receiver);
        let authenticated = match self.call_place(receiver)? {
            Some((source, _)) => source == loan.source,
            None => match &loan.source.root {
                flow::Root::Temporary {
                    owner,
                    expression: actual_expression,
                    invocation,
                } => {
                    owner == self.canonical_owner()
                        && *actual_expression == receiver.span
                        && *invocation == expression.span
                        && loan.source.projection.is_empty()
                }
                flow::Root::Binding(_) => loan
                    .parent
                    .and_then(|parent| self.graph.loans.get(parent.0))
                    .is_some_and(|parent| {
                        parent.span == self.view_call_span(receiver)
                            && parent.source == loan.source
                            && (self.graph.invocations.contains_key(&inner.span)
                                || self
                                    .carrier_loan(inner)
                                    .is_some_and(|(carrier, _)| parent.parent == Some(carrier)))
                    }),
            },
        };
        authenticated
            .then_some(())
            .ok_or("method receiver differs from its checked source loan")
    }

    /// Validate an invocation's checked arguments once and retain its unique source evaluation marker.
    fn record_invocation(
        &self,
        span: HirSourceSpan,
        arguments: &[CallArg],
        seen: &mut SourceCoverage,
    ) -> Result<(), &'static str> {
        self.validate_call_expression(span, arguments)?;
        if !seen.invocations.insert(span) {
            return Err("source call has duplicate retained evaluation");
        }
        Ok(())
    }

    /// Validate eager comprehension sources, filters and produced values in their source evaluation order.
    fn visit_comprehension(
        &self,
        clauses: &[crate::ast::ComprehensionClause],
        output: &[&Spanned<Expr>],
        parameters: &BTreeSet<CanonicalSymbolId>,
        execution: &[usize],
        seen: &mut SourceCoverage,
    ) -> Result<(), &'static str> {
        for clause in clauses {
            let value = match clause {
                crate::ast::ComprehensionClause::For { iter, .. } => iter,
                crate::ast::ComprehensionClause::If(condition) => condition,
            };
            self.visit_operation_expression(value, parameters, execution, seen)?;
        }
        for value in output {
            self.visit_operation_expression(value, parameters, execution, seen)?;
        }
        Ok(())
    }

    /// Authenticate exact operand coverage against checked alias identities. A marker at another span, reordered
    /// operands, or an otherwise well-formed read through another loan cannot fulfill this operation's contract.
    fn require_operation(
        &self,
        operation: HirSourceSpan,
        operands: &[&Spanned<Expr>],
        parameters: &BTreeSet<CanonicalSymbolId>,
        execution: &[usize],
        seen: &mut SourceCoverage,
    ) -> Result<(), &'static str> {
        let mut required = Vec::new();
        for operand in operands {
            let ty = self
                .facts
                .expr_type(operand.span)
                .ok_or("operation operand has no checked type")?;
            if !self.facts.checked_input_copy_value(self.canonical_owner(), ty)?
                && let Some((source, loan)) = self.operation_borrowed_place(operand, parameters)?
            {
                required.push((operand.span, source, loan));
            }
        }
        let retained = self.graph.operations.get(&operation);
        if required.is_empty() {
            if retained.is_some() {
                return Err("operation retains a dependency absent from its source operands");
            }
            return Ok(());
        }
        let retained = retained.ok_or("borrowed operation has no retained consumption dependency")?;
        if !seen.operations.insert(operation) || retained.operands.len() != required.len() {
            return Err("operation dependencies do not match source operand coverage");
        }
        for (index, node) in self.graph.nodes.iter().enumerate() {
            if node.span.expansion == operation.expansion
                && operation.start <= node.span.start
                && node.span.end <= operation.end
                && index != retained.completion_node
                && !retained
                    .operands
                    .iter()
                    .any(|dependency| dependency.final_access == index)
                && execution[index] >= execution[retained.completion_node]
            {
                return Err("operation execution consumes operands before their evaluation completes");
            }
        }
        for (actual, (operand, source, loan)) in retained.operands.iter().zip(required) {
            let observed = self
                .graph
                .loans
                .get(actual.loan.0)
                .ok_or("operation read loan is absent")?;
            if actual.operand != operand
                || actual.source != source
                || observed.source != source
                || observed.parent != loan
                || observed.kind != flow::LoanKind::Shared
                || observed.writable
                || observed.span != operand
            {
                return Err("operation dependency differs from its checked operand source");
            }
            let origin = self
                .graph
                .nodes
                .iter()
                .position(|node| matches!(node.event, flow::Event::Borrow(id) if id == actual.loan))
                .ok_or("operation read loan has no creation event")?;
            let evaluated = origin
                .checked_sub(1)
                .and_then(|index| self.graph.nodes.get(index))
                .ok_or("operation read loan has no evaluated operand")?;
            if execution[origin] >= execution[retained.completion_node]
                || evaluated.successors.as_slice() != [origin]
                || self.graph.nodes[origin].span != operand
                || evaluated.span.expansion != operand.expansion
                || evaluated.span.start < operand.start
                || evaluated.span.end > operand.end
                || !matches!(&evaluated.event, flow::Event::Access { place, access: flow::Access::Read, through }
                    if place == &source && *through == loan)
            {
                return Err("operation read loan was not created at its evaluated operand");
            }
            let position = operands
                .iter()
                .position(|expression| expression.span == operand)
                .ok_or("operation dependency has no source operand position")?;
            for later in &operands[position + 1..] {
                if self.graph.nodes.iter().enumerate().any(|(index, node)| {
                    execution[index] <= execution[origin]
                        && node.span.expansion == later.span.expansion
                        && later.span.start <= node.span.start
                        && node.span.end <= later.span.end
                }) {
                    return Err("operation read loan starts after a later operand was evaluated");
                }
            }
        }
        Ok(())
    }

    /// Resolve stored aliases and declared borrowed parameters through canonical identities, preserving field and
    /// indexed projections. Fresh expression results retain no upstream storage dependency after their own operation
    /// completes.
    fn operation_borrowed_place(
        &self,
        expression: &Spanned<Expr>,
        parameters: &BTreeSet<CanonicalSymbolId>,
    ) -> Result<Option<(flow::Place, Option<flow::LoanId>)>, &'static str> {
        match &expression.node {
            Expr::Ident(_) => {
                let Some(identity) = self.facts.resolved_identity(expression.span) else {
                    return Ok(None);
                };
                if let Some(id) = self.graph.local_loans.get(identity) {
                    let loan = self.graph.loans.get(id.0).ok_or("operation alias has no source loan")?;
                    return Ok(Some((loan.source.clone(), Some(*id))));
                }
                Ok(parameters.contains(identity).then(|| {
                    (
                        flow::Place {
                            root: flow::Root::Binding(identity.clone()),
                            projection: Vec::new(),
                        },
                        None,
                    )
                }))
            }
            Expr::Paren(inner) => self.operation_borrowed_place(inner, parameters),
            Expr::Index(base, index) => {
                let Some((mut place, loan)) = self.operation_borrowed_place(base, parameters)? else {
                    return Ok(None);
                };
                place.projection.push(flow::Projection::Element(index.span));
                Ok(Some((place, loan)))
            }
            Expr::Field(base, _) => {
                let Some((mut place, loan)) = self.operation_borrowed_place(base, parameters)? else {
                    return Ok(None);
                };
                let field = self
                    .facts
                    .resolved_identity(expression.span)
                    .filter(|field| field.kind == SemanticSourceTargetKind::Field)
                    .ok_or("operation alias projection lacks its canonical field")?;
                if self.facts.computed_property_access(expression.span).is_some() {
                    return Err("operation alias cannot infer computed property access");
                }
                place.projection.push(flow::Projection::Field(field.clone()));
                Ok(Some((place, loan)))
            }
            _ => Ok(None),
        }
    }
}

/// Whether a `loop` body can leave the loop: a `break` of its own, directly or in a branch. A nested loop's `break`
/// leaves only that inner loop.
pub(super) fn breaks_out(body: &[Spanned<Statement>]) -> bool {
    body.iter().any(|statement| match &statement.node {
        Statement::Break(_) => true,
        Statement::If(branch) => {
            breaks_out(&branch.then_body)
                || branch.elif_branches.iter().any(|(_, body)| breaks_out(body))
                || branch.else_body.as_deref().is_some_and(breaks_out)
        }
        _ => false,
    })
}
