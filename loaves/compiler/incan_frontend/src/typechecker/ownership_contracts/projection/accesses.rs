//! Validate complete source access coverage without repairing or replacing the retained ownership graph.

use std::collections::{BTreeMap, BTreeSet};

use incan_lang::lang::surface::iterator_methods::{self, IteratorMethodId};
use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::{HirSourceSpan, IncanInputAccess, SemanticSourceTargetKind};

use super::{CheckedInputBody, flow};
use crate::ast::{
    AssertKind, AssignmentStmt, BindingKind, CallArg, Condition, Expr, FStringPart, ForStmt, ListEntry, Literal,
    MatchBody, Spanned, Statement, TupleUnpackStmt,
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
            .chain(
                self.graph
                    .closures
                    .values()
                    .flat_map(|closure| closure.reads.iter().map(|(_, index)| *index)),
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

    /// The retained capture record when the expression names a closure local.
    fn closure_callee(&self, callee: &Spanned<Expr>) -> Option<&flow::ClosureCaptures> {
        if !matches!(callee.node, Expr::Ident(_)) {
            return None;
        }
        let identity = self.proof.facts.resolved_identity(callee.span)?;
        self.proof
            .graph
            .closures
            .values()
            .find(|closure| &closure.local == identity)
    }

    /// Read every source a closure retains, proving that a direct call or call-bounded loan uses all captures.
    fn closure_capture_accesses(&mut self, span: crate::ast::Span, loans: &[flow::LoanId]) -> Result<(), &'static str> {
        for id in loans {
            let loan = self
                .proof
                .graph
                .loans
                .get(id.0)
                .ok_or("closure local has no captured loan")?;
            self.access(span, &loan.source, Some(*id), flow::Access::Read)?;
        }
        Ok(())
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

    /// Return whether the producer retained an unclaimed source access at this expression.
    fn has_access(&self, span: HirSourceSpan) -> bool {
        self.by_span
            .get(&span)
            .is_some_and(|indices| indices.iter().any(|index| !self.claimed.contains(index)))
    }

    /// Replay a source whose lazy result may borrow a place, carry existing dependencies or own a temporary.
    fn retained_source(&mut self, expression: &Spanned<Expr>, escaping: bool) -> Result<(), &'static str> {
        if let Some((place, through)) = self.place(expression)? {
            let moved = self.by_span.get(&expression.span).is_some_and(|indices| {
                indices.iter().any(|index| {
                    matches!(&self.proof.graph.nodes[*index].event,
                        flow::Event::Access {
                            place: actual,
                            access: flow::Access::Move,
                            through: actual_through,
                        } if actual == &place && *actual_through == through)
                })
            });
            if moved {
                self.place_evaluation(expression)?;
                return self.access(expression.span, &place, through, flow::Access::Move);
            }
            if self.has_access(expression.span) {
                return self.expression(expression, false);
            }
            return self.place_evaluation(expression);
        }
        self.expression(expression, escaping)
    }

    /// Claim every lazy-result dependency read retained at one canonically selected terminal call.
    fn terminal_dependencies(&mut self, span: HirSourceSpan) -> Result<(), &'static str> {
        let dependencies = self
            .by_span
            .get(&span)
            .into_iter()
            .flatten()
            .filter_map(|index| match &self.proof.graph.nodes[*index].event {
                flow::Event::Access {
                    place,
                    access: flow::Access::Read,
                    through: Some(loan),
                } => Some((place.clone(), *loan)),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (place, loan) in dependencies {
            self.access(span, &place, Some(loan), flow::Access::Read)?;
        }
        Ok(())
    }

    /// Replay the producer policy for one canonical lazy-builtin call without inventing an explicit invocation.
    fn retained_iterator_call(
        &mut self,
        receiver: &Spanned<Expr>,
        arguments: &[CallArg],
        method: IteratorMethodId,
        span: HirSourceSpan,
        escaping: bool,
    ) -> Result<(), &'static str> {
        self.retained_source(receiver, escaping)?;
        for (index, argument) in arguments.iter().enumerate() {
            let value = match argument {
                CallArg::Positional(value) | CallArg::Named(_, value) => value,
                _ => return Err("source access needs fixed iterator arguments"),
            };
            let carries_iterable = index == 0 && matches!(method, IteratorMethodId::Zip | IteratorMethodId::Chain);
            if carries_iterable {
                self.retained_source(value, false)?;
            } else if !matches!(
                self.proof.facts.expr_type(value.span),
                Some(crate::symbols::ResolvedType::Function(..))
            ) {
                self.expression(value, false)?;
            }
        }
        if iterator_methods::is_terminal(method) {
            self.terminal_dependencies(span)?;
        }
        Ok(())
    }

    /// Evaluate the index operands of one stored place without reading the place itself. Field and grouping
    /// projections add no runtime value evaluation, while every index expression runs once from left to right.
    fn place_evaluation(&mut self, expression: &Spanned<Expr>) -> Result<(), &'static str> {
        match &expression.node {
            Expr::Ident(_) | Expr::SelfExpr => Ok(()),
            Expr::Paren(base) | Expr::Field(base, _) => self.place_evaluation(base),
            Expr::Index(base, index) => {
                self.place_evaluation(base)?;
                self.expression(index, false)
            }
            Expr::MethodCall(..) if super::super::builtins::byte_slice_view(self.proof.facts, expression).is_some() => {
                self.place_evaluation(
                    super::super::builtins::byte_slice_view(self.proof.facts, expression)
                        .ok_or("byte slice view has no checked receiver")?,
                )
            }
            Expr::Call(..) | Expr::MethodCall(..) if self.place(expression)?.is_some() => {
                self.expression(expression, false)
            }
            _ => Err("source access place has an unsupported evaluation"),
        }
    }

    /// Match one stored-place read or move, including dependencies held by an owned aggregate local.
    fn stored_place_access(&mut self, expression: &Spanned<Expr>, consume: bool) -> Result<(), &'static str> {
        self.place_evaluation(expression)?;
        let (place, through) = self.place(expression)?.ok_or("source access has no stored place")?;
        if let flow::Root::Binding(root) = &place.root
            && let Some(loans) = self.proof.graph.dependent_locals.get(root)
        {
            let loans = loans.clone();
            self.closure_capture_accesses(expression.span, &loans)?;
        }
        let access = if consume
            && (through.is_some() || self.constrained.contains(&place.root))
            && !self.copy_value(expression)?
        {
            flow::Access::Move
        } else {
            flow::Access::Read
        };
        self.access(expression.span, &place, through, access)
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
            // A closure local passed on first uses the borrow it holds, then is read itself.
            if argument.access == IncanInputAccess::Inferred
                && let Some(closure) = self.closure_callee(value)
            {
                let loans: Vec<_> = closure.loans.iter().map(|(_, id)| *id).collect();
                self.closure_capture_accesses(value.span, &loans)?;
            }
            let source = self.place(value)?;
            match argument.access {
                IncanInputAccess::Owned => {
                    if let Some((place, _)) = source {
                        self.constrained.insert(place.root);
                    }
                    self.expression(value, true)?;
                }
                IncanInputAccess::Shared | IncanInputAccess::InferredShared | IncanInputAccess::Exclusive
                    if source.is_some() => {}
                IncanInputAccess::Shared | IncanInputAccess::InferredShared | IncanInputAccess::Exclusive => {
                    self.expression(value, true)?
                }
                IncanInputAccess::Inferred => self.expression(value, false)?,
            }
        }
        Ok(())
    }

    /// Visit source expressions according to the producer's actual evaluation policy. Borrowing a stored place
    /// evaluates its address, not its value; fresh borrowed arguments still evaluate their nested value normally.
    fn expression(&mut self, expression: &Spanned<Expr>, consume: bool) -> Result<(), &'static str> {
        match &expression.node {
            Expr::Ident(_) | Expr::SelfExpr | Expr::Field(_, _) | Expr::Index(_, _) => {
                self.stored_place_access(expression, consume)?
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
            Expr::Tuple(elements) => {
                for value in elements {
                    self.expression(value, false)?;
                }
            }
            Expr::ListComp(comprehension) => self.comprehension(&comprehension.clauses, &[&comprehension.expr])?,
            Expr::DictComp(comprehension) => {
                self.comprehension(&comprehension.clauses, &[&comprehension.key, &comprehension.value])?
            }
            Expr::Generator(generator) => self.generator_access(generator, consume)?,
            Expr::MethodCall(receiver, _, _, arguments) => {
                self.method_access(expression, receiver, arguments, consume)?
            }
            Expr::Call(callee, _, arguments) => self.call_access(expression, callee, arguments)?,
            // `await` suspends on its call, which is evaluated as written.
            Expr::Surface(surface) if super::super::builtins::awaited(surface).is_some() => {
                if let Some(awaited) = super::super::builtins::awaited(surface) {
                    self.expression(awaited, consume)?;
                }
            }
            _ => return Err("source access coverage requires the supported expression profile"),
        }
        Ok(())
    }

    /// Validate a generator's eager source; its later clauses execute only when a terminal reader advances it.
    fn generator_access(&mut self, generator: &crate::ast::GeneratorExpr, consume: bool) -> Result<(), &'static str> {
        let Some(crate::ast::ComprehensionClause::For { iter, .. }) = generator.clauses.first() else {
            return Err("source access requires a generator source");
        };
        self.retained_source(iter, consume)
    }

    /// Validate one method call according to its selected lazy, builtin-list, native or ordinary invocation policy.
    fn method_access(
        &mut self,
        expression: &Spanned<Expr>,
        receiver: &Spanned<Expr>,
        arguments: &[CallArg],
        consume: bool,
    ) -> Result<(), &'static str> {
        if let Some(iterator) = super::super::builtins::retained_iterator_method(self.proof.facts, expression.span) {
            return self.retained_iterator_call(receiver, arguments, iterator, expression.span, consume);
        }
        if let Some(writes) = super::super::builtins::list_method_writes(self.proof.facts, expression.span) {
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
            return self.access(expression.span, &place, through, access);
        }
        if self
            .proof
            .facts
            .resolved_identity(expression.span)
            .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Variant)
        {
            for argument in arguments {
                let CallArg::Positional(value) = argument else {
                    return Err("source access needs positional enum payloads");
                };
                self.expression(value, false)?;
            }
            return Ok(());
        }
        let invocation = self
            .proof
            .graph
            .invocations
            .get(&expression.span)
            .ok_or("source access method call has no checked invocation")?;
        if self.proof.facts.native_associated_call(expression.span) {
            return self.invocation_arguments(invocation, arguments);
        }
        if invocation.receiver_owned {
            self.expression(receiver, true)?;
            return self.invocation_arguments(invocation, arguments);
        }
        if invocation.receiver.is_none() {
            return Err("source access method receiver has no stored place");
        }
        self.fresh_method_receiver(receiver, invocation)?;
        self.invocation_arguments(invocation, arguments)
    }

    /// Evaluate a non-place method receiver through the temporary or chained binding loan selected by checking.
    fn fresh_method_receiver(
        &mut self,
        receiver: &Spanned<Expr>,
        invocation: &flow::Invocation,
    ) -> Result<(), &'static str> {
        if self.place(receiver)?.is_some() {
            return Ok(());
        }
        let loan = invocation
            .receiver
            .and_then(|loan| self.proof.graph.loans.get(loan.0))
            .ok_or("source access method receiver has no checked loan")?;
        match &loan.source.root {
            flow::Root::Temporary { .. } => self.expression(receiver, true),
            flow::Root::Binding(_) => {
                let inner = super::super::builtins::view_call(receiver);
                if !self.proof.graph.invocations.contains_key(&inner.span) && self.proof.carrier_loan(inner).is_none() {
                    return Err("source access method receiver has no stored place");
                }
                self.expression(receiver, false)
            }
        }
    }

    /// Validate closure calls from their capture record and every other call from its selected invocation or builtin
    /// operands.
    fn call_access(
        &mut self,
        expression: &Spanned<Expr>,
        callee: &Spanned<Expr>,
        arguments: &[CallArg],
    ) -> Result<(), &'static str> {
        if self.closure_callee(callee).is_some() {
            let loans: Vec<_> = self
                .closure_callee(callee)
                .into_iter()
                .flat_map(|closure| closure.loans.iter().map(|(_, id)| *id))
                .collect();
            for argument in arguments {
                let CallArg::Positional(value) = argument else {
                    return Err("source access closure call needs positional arguments");
                };
                self.expression(value, false)?;
            }
            return self.closure_capture_accesses(expression.span, &loans);
        }
        if let Some(invocation) = self.proof.graph.invocations.get(&expression.span) {
            return self.invocation_arguments(invocation, arguments);
        }
        for argument in arguments {
            let (CallArg::Positional(value) | CallArg::Named(_, value)) = argument else {
                return Err("source access needs fixed call arguments");
            };
            self.expression(value, false)?;
        }
        Ok(())
    }

    /// Unpacking a call that carries a view in one tuple element evaluates the call; the view's name takes a borrow
    /// keyed to it, as a view local does, and the other names are new owners of their elements.
    fn tuple_unpack(&mut self, unpack: &TupleUnpackStmt) -> Result<(), &'static str> {
        self.expression(&unpack.value, false)?;
        let call = self.proof.view_call_span(&unpack.value);
        for (name, &span) in unpack.names.iter().zip(&unpack.name_spans) {
            let identity = self
                .proof
                .facts
                .resolved_write_identity(span, name)
                .ok_or("source unpacking lacks a canonical target")?
                .clone();
            match self.proof.graph.local_loans.get(&identity) {
                Some(id) => {
                    let loan = self
                        .proof
                        .graph
                        .loans
                        .get(id.0)
                        .ok_or("unpacked view has no result loan")?;
                    if loan.span != call {
                        return Err("unpacked view differs from its initializer call");
                    }
                }
                None => {
                    self.known.insert(flow::Root::Binding(identity));
                }
            }
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
        // A closure binding reads or borrows its captures where it is built; those events are its retained record's.
        if let Expr::Closure(params, body) = &assignment.value.node {
            let closure = self
                .proof
                .graph
                .closures
                .get(&assignment.value.span)
                .ok_or("source closure binding has no checked captures")?;
            if closure.local != root {
                return Err("source closure binding differs from its checked local");
            }
            let free = crate::body_ir::free_vars::free_vars_in_closure_body(params, body);
            if closure
                .captures
                .iter()
                .any(|capture| !free.contains(&capture.declaration_name))
            {
                return Err("source closure capture is not read by its closure");
            }
            return Ok(());
        }
        // A new unannotated local whose loan is its initializer call's result aliases a returned view: the call is
        // evaluated, and the binding takes a borrow rather than a value. `?` takes a carried view out of its call.
        if mode.is_none()
            && root.declaration_span == assignment.name_span
            && let Some(loans) = self.proof.graph.dependent_locals.get(&root)
        {
            let sources = match &assignment.value.node {
                Expr::List(entries) => entries
                    .iter()
                    .filter_map(|entry| match entry {
                        ListEntry::Element(value) => Some(self.proof.view_call_span(value)),
                        ListEntry::Spread(_) => None,
                    })
                    .collect::<Vec<_>>(),
                _ => vec![self.proof.view_call_span(&assignment.value)],
            };
            if loans.iter().any(|id| {
                self.proof
                    .graph
                    .loans
                    .get(id.0)
                    .is_none_or(|loan| !sources.contains(&loan.span))
            }) {
                return Err("dependent local differs from its initializer call");
            }
            return self.expression(&assignment.value, false);
        }
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
                // Arm bindings own fresh payloads and record no access; guards read them before their arm's body.
                Statement::Expr(Spanned {
                    node: Expr::Match(scrutinee, arms),
                    ..
                }) => {
                    let consumes_stored_value =
                        !fresh_match_scrutinee(scrutinee) && self.proof.carrier_loan(scrutinee).is_none();
                    self.expression(scrutinee, consumes_stored_value)?;
                    let mut reaches = false;
                    for arm in arms {
                        if let Some(guard) = &arm.node.guard {
                            self.expression(guard, false)?;
                        }
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
                            None if !view.component.is_empty() => self.expression(value, true)?,
                            // A returned view borrows its source place: the graph holds a borrow, not a value access.
                            None => self.view_return(value)?,
                        },
                        (Some(value), None) => self.expression(value, true)?,
                        (None, _) => {}
                    }
                    return Ok(false);
                }
                Statement::Assignment(assignment) => self.assignment(assignment)?,
                Statement::TupleUnpack(unpack) => self.tuple_unpack(unpack)?,
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
    /// a carried view call of the same source is evaluated. A tuple's carrying element is a view return and its other
    /// elements are owned values, in order.
    fn carried_return(&mut self, value: &Spanned<Expr>, carried: usize) -> Result<(), &'static str> {
        if let Expr::Tuple(elements) = &value.node {
            for (index, element) in elements.iter().enumerate() {
                if index == carried {
                    self.view_return(element)?;
                } else {
                    self.expression(element, true)?;
                }
            }
            return Ok(());
        }
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
        self.for_iterable(&loop_.iter)?;
        self.block(&loop_.body)?;
        Ok(())
    }

    /// Validate an eager comprehension's nested source loops, filters and produced values in evaluation order.
    fn comprehension(
        &mut self,
        clauses: &[crate::ast::ComprehensionClause],
        output: &[&Spanned<Expr>],
    ) -> Result<(), &'static str> {
        for clause in clauses {
            match clause {
                crate::ast::ComprehensionClause::For { iter, .. } => self.for_iterable(iter)?,
                crate::ast::ComprehensionClause::If(condition) => self.expression(condition, false)?,
            }
        }
        for value in output {
            self.expression(value, false)?;
        }
        Ok(())
    }

    /// Validate one eager-loop iterable evaluation and its retained poll without inferring either access.
    fn for_iterable(&mut self, iter: &Spanned<Expr>) -> Result<(), &'static str> {
        let takes_items = self.proof.facts.for_loop_takes_items(iter.span);
        let iterable = if takes_items { None } else { self.place(iter)? };
        self.expression(iter, takes_items)?;
        if let Some((place, through)) = iterable {
            let loan = self
                .proof
                .graph
                .loans
                .iter()
                .position(|loan| {
                    loan.span == iter.span
                        && loan.source == place
                        && loan.parent == through
                        && loan.kind == flow::LoanKind::Shared
                })
                .ok_or("loop poll has no iteration loan")?;
            self.access(iter.span, &place, Some(flow::LoanId(loan)), flow::Access::Read)?;
        }
        Ok(())
    }
}

/// Calls, methods and literals produce fresh match inputs; only a stored place consumes its checked value route.
fn fresh_match_scrutinee(scrutinee: &Spanned<Expr>) -> bool {
    match &scrutinee.node {
        Expr::Call(..) | Expr::MethodCall(..) | Expr::Literal(_) => true,
        Expr::Paren(inner) => fresh_match_scrutinee(inner),
        _ => false,
    }
}
