//! Validate complete source access coverage without repairing or replacing the retained ownership graph.

use std::collections::{BTreeMap, BTreeSet};

use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::{HirSourceSpan, IncanInputAccess};

use super::{CheckedInputBody, flow};
use crate::ast::{
    AssertKind, AssignmentStmt, BindingKind, CallArg, Condition, Expr, FStringPart, ForStmt, ListEntry, Literal,
    MatchBody, Spanned, Statement,
};
use crate::typechecker::ownership_contracts::CallableSource;

impl CheckedInputBody<'_> {
    /// Every stored source read and write must exist with its canonical place, access kind and loan route. The
    /// index below accelerates validation only: it never invents an event or supplies missing lowering authority.
    pub(super) fn validate_source_access_coverage(&self, function: CallableSource<'_>) -> Result<(), &'static str> {
        let mut coverage = SourceAccesses {
            proof: self,
            by_span: BTreeMap::new(),
            claimed: BTreeSet::new(),
            last: None,
            known: BTreeSet::new(),
            constrained: BTreeSet::new(),
        };
        for (index, node) in self.graph.nodes.iter().enumerate() {
            if matches!(node.event, flow::Event::Access { .. }) {
                coverage.by_span.entry(node.span).or_default().push(index);
            }
        }
        for index in self
            .graph
            .invocations
            .values()
            .flat_map(|call| call.final_accesses.iter().copied())
            .chain(
                self.graph
                    .operations
                    .values()
                    .flat_map(|operation| operation.operands.iter().map(|operand| operand.final_access)),
            )
        {
            if !coverage.claimed.insert(index) {
                return Err("synthetic input access fulfills more than one source role");
            }
        }
        let contract = self.facts.declarations.ownership_contracts.get(self.canonical_owner());
        for (slot, parameter) in function.params().iter().enumerate() {
            let identity = self
                .facts
                .resolved_write_identity(parameter.span, &parameter.node.name)
                .ok_or("source access parameter has no canonical declaration")?;
            let root = flow::Root::Binding(identity.clone());
            coverage.known.insert(root.clone());
            if crate::ownership_metadata::direct_input_access(contract, slot)? != IncanInputAccess::Inferred {
                coverage.constrained.insert(root);
            }
        }
        // The receiver's scope has closed; its root is the graph's receiver permission at the binding token.
        if let Some((binding, _)) = function.receiver() {
            let root = self
                .graph
                .permissions
                .keys()
                .find(|root| {
                    matches!(root, flow::Root::Binding(identity)
                        if identity.kind == incan_semantics_core::SemanticSourceTargetKind::Receiver
                            && identity.declaration_span == binding.span)
                })
                .ok_or("source access receiver has no canonical declaration")?;
            coverage.known.insert(root.clone());
        }
        coverage.block(function.body())?;
        if self
            .graph
            .nodes
            .iter()
            .enumerate()
            .any(|(index, node)| matches!(node.event, flow::Event::Access { .. }) && !coverage.claimed.contains(&index))
        {
            return Err("retained access has no matching checked source operation");
        }
        Ok(())
    }
}

/// Canonical source roles consume existing events in producer order; synthetic invocation/operation uses were
/// independently validated and are excluded. Constraint propagation follows the source producer's lexical order.
struct SourceAccesses<'a, 'facts> {
    proof: &'a CheckedInputBody<'facts>,
    by_span: BTreeMap<HirSourceSpan, Vec<usize>>,
    claimed: BTreeSet<usize>,
    last: Option<usize>,
    known: BTreeSet<flow::Root>,
    constrained: BTreeSet<flow::Root>,
}

impl SourceAccesses<'_, '_> {
    /// Match the full checked identity and access role, refusing ambiguity, omission and reordering. The span is
    /// only an index into candidates; it is not authority for a place or a loan.
    fn access(
        &mut self,
        span: HirSourceSpan,
        place: &flow::Place,
        through: Option<flow::LoanId>,
        expected: flow::Access,
    ) -> Result<(), &'static str> {
        let mut candidates = self.by_span.get(&span).into_iter().flatten().copied().filter(|index| {
            !self.claimed.contains(index)
                && matches!(&self.proof.graph.nodes[*index].event,
                flow::Event::Access { place: actual, access, through: actual_through }
                    if actual == place && *access == expected && *actual_through == through)
        });
        let index = candidates
            .next()
            .ok_or("checked source access has no exact retained event")?;
        if candidates.next().is_some() || self.last.is_some_and(|previous| previous >= index) {
            return Err("checked source accesses are ambiguous or reordered");
        }
        self.last = Some(index);
        self.claimed.insert(index);
        Ok(())
    }

    /// Resolve the source place from checked declarations, preserving a local alias's original storage route.
    fn place(
        &mut self,
        expression: &Spanned<Expr>,
    ) -> Result<Option<(flow::Place, Option<flow::LoanId>)>, &'static str> {
        let place = self.proof.call_place(expression)?;
        if let Some((place, _)) = &place {
            self.known.insert(place.root.clone());
        }
        Ok(place)
    }

    /// Copy semantics use the same checked value projection as explicit argument and initializer validation.
    fn copy_value(&self, expression: &Spanned<Expr>) -> Result<bool, &'static str> {
        let ty = self
            .proof
            .facts
            .expr_type(expression.span)
            .ok_or("source access has no checked value type")?;
        self.proof
            .facts
            .checked_input_copy_value(self.proof.canonical_owner(), ty)
    }

    /// Visit a checked invocation's arguments in its retained order. A borrowed stored place evaluates its address;
    /// an owned argument constrains its root and is consumed.
    fn invocation_arguments(
        &mut self,
        invocation: &flow::Invocation,
        arguments: &[CallArg],
    ) -> Result<(), &'static str> {
        for argument in &invocation.arguments {
            let value = match arguments.get(argument.written_position) {
                Some(CallArg::Positional(value) | CallArg::Named(_, value)) => value,
                _ => return Err("source access call has no fixed written argument"),
            };
            let source = self.place(value)?;
            match argument.access {
                IncanInputAccess::Owned => {
                    if let Some((place, _)) = source {
                        self.constrained.insert(place.root);
                    }
                    self.expression(value, true)?;
                }
                IncanInputAccess::Shared | IncanInputAccess::Exclusive if source.is_some() => {}
                IncanInputAccess::Shared | IncanInputAccess::Exclusive => self.expression(value, true)?,
                IncanInputAccess::Inferred => self.expression(value, false)?,
            }
        }
        Ok(())
    }

    /// Visit source expressions according to the producer's actual evaluation policy. Borrowing a stored place
    /// evaluates its address, not its value; fresh borrowed arguments still evaluate their nested value normally.
    fn expression(&mut self, expression: &Spanned<Expr>, consume: bool) -> Result<(), &'static str> {
        match &expression.node {
            Expr::Ident(_) | Expr::SelfExpr | Expr::Field(_, _) => {
                let (place, through) = self.place(expression)?.ok_or("source access has no stored place")?;
                let access = if consume
                    && (through.is_some() || self.constrained.contains(&place.root))
                    && !self.copy_value(expression)?
                {
                    flow::Access::Move
                } else {
                    flow::Access::Read
                };
                self.access(expression.span, &place, through, access)?;
            }
            Expr::Literal(_) => {}
            Expr::Paren(inner) | Expr::Unary(_, inner) => self.expression(inner, consume)?,
            // `?` consumes its carrier, as the source flow evaluates it; a local still holding a carried call is read
            // through its loan instead, as a match on it is.
            Expr::Try(inner) => self.expression(inner, self.proof.carrier_loan(inner).is_none())?,
            Expr::Binary(left, _, right) => {
                self.expression(left, false)?;
                self.expression(right, false)?;
            }
            Expr::FString(parts) => {
                for part in parts {
                    if let FStringPart::Expr { expr, .. } = part {
                        self.expression(expr, false)?;
                    }
                }
            }
            Expr::List(entries) => {
                for entry in entries {
                    let ListEntry::Element(value) = entry else {
                        return Err("source access cannot infer spread evaluation");
                    };
                    self.expression(value, false)?;
                }
            }
            // A builtin list method evaluates its arguments, then reads or writes its receiver place in place.
            Expr::MethodCall(receiver, method, _, arguments)
                if super::super::builtins::list_method_writes(self.proof.facts, receiver, method, expression.span)
                    .is_some() =>
            {
                let writes =
                    super::super::builtins::list_method_writes(self.proof.facts, receiver, method, expression.span)
                        == Some(true);
                let (place, through) = self
                    .place(receiver)?
                    .ok_or("source list method has no stored receiver")?;
                for argument in arguments {
                    let CallArg::Positional(value) = argument else {
                        return Err("source access needs positional list method arguments");
                    };
                    self.expression(value, false)?;
                }
                let access = if writes {
                    flow::Access::Write
                } else {
                    flow::Access::Read
                };
                self.access(expression.span, &place, through, access)?;
            }
            Expr::MethodCall(receiver, _, _, arguments) => {
                // A checked receiver is borrowed in place: evaluating it reads its address, not its value.
                let invocation = self
                    .proof
                    .graph
                    .invocations
                    .get(&expression.span)
                    .ok_or("source access method call has no checked invocation")?;
                if invocation.receiver.is_none() {
                    return Err("source access method receiver has no stored place");
                }
                // A chained receiver is an inner checked view call, whose own arguments are evaluated first, or `?` on
                // a local still holding one.
                if self.place(receiver)?.is_none() {
                    let inner = super::super::builtins::view_call(receiver);
                    if !self.proof.graph.invocations.contains_key(&inner.span)
                        && self.proof.carrier_loan(inner).is_none()
                    {
                        return Err("source access method receiver has no stored place");
                    }
                    self.expression(receiver, false)?;
                }
                self.invocation_arguments(invocation, arguments)?;
            }
            Expr::Call(_, _, arguments) => {
                if let Some(invocation) = self.proof.graph.invocations.get(&expression.span) {
                    self.invocation_arguments(invocation, arguments)?;
                } else {
                    // The preceding operation/call coverage accepts only checked builtin observers and complete
                    // constructors on this route; neither evaluates its callee identifier as a stored value.
                    for argument in arguments {
                        let (CallArg::Positional(value) | CallArg::Named(_, value)) = argument else {
                            return Err("source access needs fixed call arguments");
                        };
                        self.expression(value, false)?;
                    }
                }
            }
            _ => return Err("source access coverage requires the supported expression profile"),
        }
        Ok(())
    }

    /// Validate a declaration or whole binding replacement only after its actual initializer has been evaluated.
    fn assignment(&mut self, assignment: &AssignmentStmt) -> Result<(), &'static str> {
        let root = self
            .proof
            .facts
            .resolved_write_identity(assignment.name_span, &assignment.name)
            .ok_or("source assignment lacks a canonical target")?
            .clone();
        let mode = (root.declaration_span == assignment.name_span)
            .then(|| self.proof.facts.protocols.explicit_local_modes.get(&root))
            .flatten();
        // A new unannotated local whose loan is its initializer call's result aliases a returned view: the call is
        // evaluated, and the binding takes a borrow rather than a value. `?` takes a carried view out of its call.
        if mode.is_none()
            && root.declaration_span == assignment.name_span
            && let Some(id) = self.proof.graph.local_loans.get(&root)
        {
            let loan = self
                .proof
                .graph
                .loans
                .get(id.0)
                .ok_or("returned view local has no result loan")?;
            if loan.span != self.proof.view_call_span(&assignment.value) {
                return Err("returned view local differs from its initializer call");
            }
            return self.expression(&assignment.value, false);
        }
        let source = self.place(&assignment.value)?;
        if mode.is_some_and(|mode| matches!(mode.mode, OwnershipMode::Borrowed | OwnershipMode::BorrowedMut)) {
            let id = self
                .proof
                .graph
                .local_loans
                .get(&root)
                .ok_or("source alias lacks its retained loan")?;
            let loan = self.proof.graph.loans.get(id.0).ok_or("source alias loan is absent")?;
            if source
                .as_ref()
                .is_none_or(|(source, through)| source != &loan.source || *through != loan.parent)
            {
                return Err("source alias initializer differs from its checked storage");
            }
            return Ok(());
        }
        if mode.is_some_and(|mode| mode.mode == OwnershipMode::Owned)
            && let Some((source, _)) = &source
        {
            self.constrained.insert(source.root.clone());
        }
        self.expression(&assignment.value, true)?;
        if let Some(id) = self.proof.graph.local_loans.get(&root) {
            let loan = self
                .proof
                .graph
                .loans
                .get(id.0)
                .ok_or("source assignment alias loan is absent")?;
            return self.access(assignment.name_span, &loan.source, Some(*id), flow::Access::Write);
        }
        let binding = flow::Root::Binding(root);
        let known = self.known.contains(&binding);
        self.known.insert(binding.clone());
        if mode.is_some()
            || source.is_some_and(|(source, through)| through.is_some() || self.constrained.contains(&source.root))
        {
            self.constrained.insert(binding.clone());
        }
        if known || matches!(assignment.binding, BindingKind::Reassign) {
            self.access(
                assignment.name_span,
                &flow::Place {
                    root: binding,
                    projection: Vec::new(),
                },
                None,
                flow::Access::Replace,
            )?;
        }
        Ok(())
    }

    /// Traverse reachable source regions, retaining failure-message termination and continuing branch arms.
    fn block(&mut self, statements: &[Spanned<Statement>]) -> Result<bool, &'static str> {
        for statement in statements {
            match &statement.node {
                Statement::Pass => {}
                // Arm bindings own fresh payloads and record no access; each arm starts after the scrutinee.
                Statement::Expr(Spanned {
                    node: Expr::Match(scrutinee, arms),
                    ..
                }) => {
                    self.expression(scrutinee, false)?;
                    let mut reaches = false;
                    for arm in arms {
                        reaches |= match &arm.node.body {
                            MatchBody::Expr(value) => {
                                self.expression(value, false)?;
                                true
                            }
                            MatchBody::Block(body) => self.block(body)?,
                        };
                    }
                    if !reaches {
                        return Ok(false);
                    }
                }
                Statement::Expr(value) => self.expression(value, false)?,
                Statement::Return(value) => {
                    let view = crate::ownership_metadata::returned_view(
                        self.proof
                            .facts
                            .declarations
                            .ownership_contracts
                            .get(self.proof.canonical_owner()),
                    )?;
                    match (value, view) {
                        (Some(value), Some(view)) => match view.carried {
                            Some(payload) => self.carried_return(value, payload)?,
                            // A returned view borrows its source place: the graph holds a borrow, not a value access.
                            None => self.view_return(value)?,
                        },
                        (Some(value), None) => self.expression(value, true)?,
                        (None, _) => {}
                    }
                    return Ok(false);
                }
                Statement::Assignment(assignment) => self.assignment(assignment)?,
                Statement::CompoundAssignment(assignment) => {
                    let identity = self
                        .proof
                        .facts
                        .resolved_write_identity(assignment.name_span, &assignment.name)
                        .ok_or("compound target has no canonical identity")?;
                    let (place, through) = if let Some(id) = self.proof.graph.local_loans.get(identity) {
                        let loan = self.proof.graph.loans.get(id.0).ok_or("compound target has no loan")?;
                        (loan.source.clone(), Some(*id))
                    } else {
                        (
                            flow::Place {
                                root: flow::Root::Binding(identity.clone()),
                                projection: Vec::new(),
                            },
                            None,
                        )
                    };
                    self.known.insert(place.root.clone());
                    self.access(assignment.name_span, &place, through, flow::Access::Read)?;
                    self.expression(&assignment.value, false)?;
                    self.access(assignment.name_span, &place, through, flow::Access::Write)?;
                }
                Statement::FieldAssignment(assignment) => {
                    let (mut place, through) = self
                        .place(&assignment.object)?
                        .ok_or("field write has no source owner")?;
                    let field = self
                        .proof
                        .facts
                        .resolved_identity(assignment.target_span)
                        .ok_or("field write has no canonical field")?;
                    place.projection.push(flow::Projection::Field(field.clone()));
                    self.expression(&assignment.value, true)?;
                    self.access(
                        assignment.target_span,
                        &place,
                        through,
                        if through.is_some() {
                            flow::Access::Write
                        } else {
                            flow::Access::Replace
                        },
                    )?;
                }
                Statement::Assert(assertion) => {
                    let AssertKind::Condition(condition) = &assertion.kind else {
                        return Err("source access requires ordinary assertions");
                    };
                    self.expression(condition, false)?;
                    if let Some(message) = &assertion.message {
                        self.expression(message, false)?;
                    }
                }
                Statement::If(branch) => {
                    let Condition::Expr(condition) = &branch.condition else {
                        return Err("source access requires expression conditions");
                    };
                    self.expression(condition, false)?;
                    let mut reaches = self.block(&branch.then_body)?;
                    for (condition, body) in &branch.elif_branches {
                        self.expression(condition, false)?;
                        reaches |= self.block(body)?;
                    }
                    reaches |= match &branch.else_body {
                        Some(body) => self.block(body)?,
                        None => true,
                    };
                    if !reaches {
                        return Ok(false);
                    }
                }
                Statement::While(loop_) => {
                    let Condition::Expr(condition) = &loop_.condition else {
                        return Err("source access requires expression conditions");
                    };
                    self.expression(condition, false)?;
                    self.block(&loop_.body)?;
                }
                Statement::For(loop_) => self.for_loop(loop_)?,
                Statement::Loop(loop_) => {
                    self.block(&loop_.body)?;
                    if !super::operations::breaks_out(&loop_.body) {
                        return Ok(false);
                    }
                }
                Statement::Break(None) | Statement::Continue => return Ok(false),
                _ => return Err("source access coverage requires the supported statement profile"),
            }
        }
        Ok(true)
    }

    /// A returned view borrows its source place, recorded as a borrow rather than an access; a chained view return
    /// evaluates its checked inner call.
    fn view_return(&mut self, value: &Spanned<Expr>) -> Result<(), &'static str> {
        if self.place(value)?.is_none() {
            if !self.proof.graph.invocations.contains_key(&value.span) {
                return Err("returned view has no stored source place");
            }
            self.expression(value, false)?;
        }
        Ok(())
    }

    /// A carried view's return: the variant holding the view (`Ok` or `Some`, or `Err` for a view carried in the
    /// error) of the declared place is a view return, another variant is an owned value, `None` evaluates nothing, and
    /// a carried view call of the same source is evaluated.
    fn carried_return(&mut self, value: &Spanned<Expr>, carried: usize) -> Result<(), &'static str> {
        match super::super::builtins::carrier_construction(self.proof.facts, value) {
            Some((variant, [CallArg::Positional(payload)]))
                if super::super::builtins::variant_carries_view(variant, carried) =>
            {
                self.view_return(payload)
            }
            Some(_) => self.expression(value, true),
            _ if matches!(value.node, Expr::Literal(Literal::None)) => Ok(()),
            _ => {
                if !self.proof.graph.invocations.contains_key(&value.span) {
                    return Err("returned carried view has no checked carried call");
                }
                self.expression(value, false)
            }
        }
    }

    /// A loop evaluates its iterable once; a loop that takes the list's items consumes it. Each poll of a stored
    /// iterable reads it through the loop's own shared loan, created over that exact place at the iterable.
    fn for_loop(&mut self, loop_: &ForStmt) -> Result<(), &'static str> {
        let takes_items = self.proof.facts.for_loop_takes_items(loop_.iter.span);
        let iterable = if takes_items { None } else { self.place(&loop_.iter)? };
        self.expression(&loop_.iter, takes_items)?;
        if let Some((place, through)) = iterable {
            let loan = self
                .proof
                .graph
                .loans
                .iter()
                .position(|loan| {
                    loan.span == loop_.iter.span
                        && loan.source == place
                        && loan.parent == through
                        && loan.kind == flow::LoanKind::Shared
                })
                .ok_or("loop poll has no iteration loan")?;
            self.access(loop_.iter.span, &place, Some(flow::LoanId(loan)), flow::Access::Read)?;
        }
        self.block(&loop_.body)?;
        Ok(())
    }
}
