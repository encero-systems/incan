//! Source graph builder assignments, compound assignments, loops and statements.

use super::*;

impl Builder<'_> {
    /// Bind an explicit local loan to its exact source, or transfer an owned value. Unannotated declarations keep
    /// their existing behavior; reassignment through an already borrowed local writes through without retargeting.
    pub(super) fn assignment(
        &mut self,
        assignment: &AssignmentStmt,
        span: Span,
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let root = self
            .checker
            .type_info
            .resolved_write_identity(assignment.name_span, &assignment.name)
            .ok_or(Unavailable {
                span: assignment.name_span,
                reason: "missing checked assignment target".into(),
            })?
            .clone();
        let mode = (root.declaration_span == assignment.name_span)
            .then(|| self.checker.type_info.protocols.explicit_local_modes.get(&root))
            .flatten();
        if mode.is_none()
            && root.declaration_span == assignment.name_span
            && root.kind == SemanticSourceTargetKind::Local
        {
            let (next, bound) = self.bind_unannotated_local(assignment, &root, incoming)?;
            incoming = next;
            if bound {
                return Ok(incoming);
            }
        }
        let source = self.optional_place(&assignment.value)?;
        if let Some(mode) = mode {
            if root.kind != SemanticSourceTargetKind::Local || mode.owner != self.owner {
                return Err(Unavailable {
                    span: mode.annotation,
                    reason: "local ownership annotation does not belong to its checked declaration".into(),
                });
            }
            if mode.mode == OwnershipMode::Reference {
                return Err(Unavailable {
                    span: mode.annotation,
                    reason: "returned-reference local dependency".into(),
                });
            }
            if self.checker.type_info.assignment_binding_type(span) != Some(&mode.value_type) {
                return Err(Unavailable {
                    span: mode.annotation,
                    reason: "local mode and checked value type disagree".into(),
                });
            }
            if mode.mode != OwnershipMode::Owned {
                let source = source.ok_or(Unavailable {
                    span: assignment.value.span,
                    reason: "local borrow requires a checked owner place".into(),
                })?;
                let incoming = self.evaluate_place(&assignment.value, incoming)?;
                let loan = LoanId(self.graph.loans.len());
                self.graph.loans.push(Loan {
                    source: source.place.clone(),
                    kind: if mode.mode == OwnershipMode::Borrowed {
                        LoanKind::Shared
                    } else {
                        LoanKind::Exclusive
                    },
                    writable: mode.writable,
                    parent: source.through,
                    span: assignment.value.span,
                });
                if self.graph.local_loans.insert(root.clone(), loan).is_some() {
                    return Err(Unavailable {
                        span: mode.annotation,
                        reason: "local declaration has more than one ownership destination".into(),
                    });
                }
                self.aliases.insert(
                    root,
                    Path {
                        place: source.place,
                        through: Some(loan),
                    },
                );
                return Ok(self.event(incoming, Event::Borrow(loan), assignment.value.span));
            }
            if let Some(source) = &source {
                self.constrained.insert(source.place.root.clone());
            }
        }
        let incoming = self.expression(&assignment.value, incoming, true)?;
        if mode.is_some_and(|mode| mode.mode == OwnershipMode::Owned) {
            self.retain_owned_initializer(assignment, &root, source.as_ref(), &incoming)?;
        }
        if let Some(path) = self.aliases.get(&root).cloned() {
            return Ok(self.event(
                incoming,
                Event::Access {
                    place: path.place,
                    access: Access::Write,
                    through: path.through,
                },
                assignment.name_span,
            ));
        }
        let previously_known = self.graph.permissions.contains_key(&Root::Binding(root.clone()));
        let place = self.root(&root, assignment.name_span)?;
        if mode.is_some()
            || source.is_some_and(|source| source.through.is_some() || self.constrained.contains(&source.place.root))
        {
            self.constrained.insert(Root::Binding(root));
        }
        if !previously_known && !matches!(assignment.binding, BindingKind::Reassign) {
            return Ok(incoming);
        }
        Ok(self.event(
            incoming,
            Event::Access {
                place,
                access: Access::Replace,
                through: None,
            },
            assignment.name_span,
        ))
    }

    /// Bind a closure capture, direct returned view or carried view to a fresh unannotated local. The boolean
    /// distinguishes a completed binding from an ordinary assignment while returning the source-flow frontier
    /// without cloning it.
    fn bind_unannotated_local(
        &mut self,
        assignment: &AssignmentStmt,
        root: &CanonicalSymbolId,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, bool), Unavailable> {
        // A new local initialized by a closure holds the closure's captured borrow, if it has one.
        if matches!(assignment.value.node, Expr::Closure(..)) {
            let incoming = self.bind_closure(root.clone(), &assignment.value, incoming)?;
            return Ok((incoming, true));
        }
        // A lazy builtin result owns its runtime cursor while retaining ordinary graph loans to every stored source.
        let lazy_result = matches!(self.checker.type_info.expr_type(assignment.value.span),
            Some(ResolvedType::Generic(name, _))
                if incan_lang::lang::types::collections::from_str(name)
                    == Some(incan_lang::lang::types::collections::CollectionTypeId::Generator))
            || self
                .checker
                .type_info
                .expr_type(assignment.value.span)
                .is_some_and(|ty| ty.iterator_item_type().is_some());
        if lazy_result && let Some(result) = self.retained_result(&assignment.value, incoming.clone(), false)? {
            if result.loans.is_empty() {
                return Ok((result.incoming, false));
            }
            if self.graph.dependent_locals.insert(root.clone(), result.loans).is_some() {
                return Err(Unavailable {
                    span: assignment.name_span,
                    reason: "lazy result has more than one dependency destination".into(),
                });
            }
            return Ok((result.incoming, true));
        }
        if let Some(result) = self.retained_carrier(&assignment.value, incoming.clone(), false)? {
            if result.loans.is_empty() {
                return Ok((result.incoming, false));
            }
            if self.graph.dependent_locals.insert(root.clone(), result.loans).is_some() {
                return Err(Unavailable {
                    span: assignment.name_span,
                    reason: "lazy carrier has more than one dependency destination".into(),
                });
            }
            self.graph.dependent_values.insert(assignment.value.span);
            return Ok((result.incoming, true));
        }
        // A user-defined or nested aggregate stores its physical reference while retaining the substituted source
        // loan on the aggregate local. Unlike a builtin carried view, ordinary field access reads that stored value.
        if let Some(relation) = self
            .call_relation(&assignment.value)?
            .filter(|relation| !relation.component.is_empty() && relation.carried.is_none())
        {
            let (incoming, loans) = self.evaluate_aggregate_view_call(&assignment.value, relation, incoming)?;
            if self.graph.dependent_locals.insert(root.clone(), loans).is_some() {
                return Err(Unavailable {
                    span: assignment.name_span,
                    reason: "aggregate view has more than one dependency destination".into(),
                });
            }
            return Ok((incoming, true));
        }
        // A new local initialized by a returned view aliases its substituted place under the call's result loan.
        if let Some(view) = self.view_source(&assignment.value)? {
            let (incoming, path) = self.evaluate_view_source(&assignment.value, view, incoming)?;
            let loan = path.through.ok_or(Unavailable {
                span: assignment.value.span,
                reason: "returned view has no result loan".into(),
            })?;
            self.retain_local_loan(assignment, root, loan, path)?;
            return Ok((incoming, true));
        }
        // A native borrowed result keeps its physical Rust value while retaining every caller-owned input named by
        // the checked signature. Unlike a source returned view, it is never replaced by a source-place alias.
        if let Some(relation) = self.native_result_relation(&assignment.value)?
            && !relation.sources.is_empty()
        {
            let (incoming, loans) = self.evaluate_native_result(&assignment.value, &relation, incoming)?;
            if self.graph.dependent_locals.insert(root.clone(), loans).is_some() {
                return Err(Unavailable {
                    span: assignment.name_span,
                    reason: "native borrowed result local has more than one dependency destination".into(),
                });
            }
            return Ok((incoming, true));
        }
        // A top-level owned result keeps its declared source dependency while remaining independent value storage.
        if let Some(relation) = self.owned_relation(&assignment.value)? {
            let (incoming, path) = self.evaluate_view_call(&assignment.value, relation, incoming)?;
            let loan = path.through.ok_or(Unavailable {
                span: assignment.value.span,
                reason: "owned result has no retained source loan".into(),
            })?;
            if self.graph.dependent_locals.insert(root.clone(), vec![loan]).is_some() {
                return Err(Unavailable {
                    span: assignment.name_span,
                    reason: "owned result local has more than one dependency destination".into(),
                });
            }
            return Ok((incoming, true));
        }
        if let Expr::List(entries) = &assignment.value.node
            && let Some((incoming, loans)) = self.dependent_list(entries, &incoming)?
        {
            if self.graph.dependent_locals.insert(root.clone(), loans).is_some() {
                return Err(Unavailable {
                    span: assignment.name_span,
                    reason: "reference-carrying list has more than one dependency destination".into(),
                });
            }
            self.graph.dependent_values.insert(assignment.value.span);
            return Ok((incoming, true));
        }
        // A Result or Option local keeps the source alias until `?` or a match consumes it; a tuple local keeps the
        // whole carrier through its last use without making its borrowed element directly nameable.
        let Some(relation) = self.call_relation(&assignment.value)? else {
            return Ok((incoming, false));
        };
        let Some(payload) = relation.carried else {
            return Ok((incoming, false));
        };
        let tuple = self.tuple_carrier(&assignment.value);
        let (incoming, path) = self.evaluate_view_call(&assignment.value, relation, incoming)?;
        let loan = path.through.ok_or(Unavailable {
            span: assignment.value.span,
            reason: "carried view has no result loan".into(),
        })?;
        self.retain_local_loan(assignment, root, loan, path)?;
        if !tuple {
            let loop_depth = self.loops.len();
            self.carriers
                .insert(root.clone(), matches::BoundCarrier { payload, loop_depth });
        }
        Ok((incoming, true))
    }

    /// Retain one result loan and its substituted source alias for a fresh unannotated local.
    fn retain_local_loan(
        &mut self,
        assignment: &AssignmentStmt,
        root: &CanonicalSymbolId,
        loan: LoanId,
        path: Path,
    ) -> Result<(), Unavailable> {
        if self.graph.local_loans.insert(root.clone(), loan).is_some() {
            return Err(Unavailable {
                span: assignment.name_span,
                reason: "local declaration has more than one ownership destination".into(),
            });
        }
        self.aliases.insert(root.clone(), path);
        Ok(())
    }

    /// Retain the exact final source access for an explicit Owned initializer, or no access for a fresh value.
    fn retain_owned_initializer(
        &mut self,
        assignment: &AssignmentStmt,
        root: &CanonicalSymbolId,
        source: Option<&Path>,
        incoming: &[usize],
    ) -> Result<(), Unavailable> {
        let access_node = if source.is_some() {
            let [node] = incoming else {
                return Err(Unavailable {
                    span: assignment.value.span,
                    reason: "owned initializer needs one final stored access".into(),
                });
            };
            Some(*node)
        } else {
            None
        };
        let retained = OwnedInitialization {
            initializer: assignment.value.span,
            source: source.map(|source| source.place.clone()),
            access_node,
        };
        if self.graph.owned_locals.insert(root.clone(), retained).is_some() {
            return Err(Unavailable {
                span: assignment.name_span,
                reason: "owned local has more than one initializer".into(),
            });
        }
        Ok(())
    }

    /// Evaluate a stored field destination before its replacement value. Only canonical stored fields participate;
    /// computed setters remain unavailable. A primitive compound update already contains its checked read/operator
    /// expression; only static stored projections can reuse it without repeating an effectful destination.
    pub(super) fn field_assignment(
        &mut self,
        assignment: &FieldAssignmentStmt,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let identity = self
            .checker
            .type_info
            .resolved_identity(assignment.target_span)
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Field)
            .ok_or(Unavailable {
                span: assignment.target_span,
                reason: "field write lacks a checked stored identity".into(),
            })?
            .clone();
        let mut path = self.place(&assignment.object)?;
        path.place.projection.push(Projection::Field(identity));
        if assignment.compound_op.is_some()
            && (path
                .place
                .projection
                .iter()
                .any(|projection| matches!(projection, Projection::Element(_)))
                || !matches!(
                    self.checker.type_info.expr_type(assignment.target_span),
                    Some(ResolvedType::Int | ResolvedType::Float | ResolvedType::Numeric(_) | ResolvedType::Bool)
                ))
        {
            return Err(Unavailable {
                span: assignment.target_span,
                reason: "compound field destination needs checked scalar single-evaluation facts".into(),
            });
        }
        let incoming = self.evaluate_place(&assignment.object, incoming)?;
        let incoming = self.expression(&assignment.value, incoming, true)?;
        Ok(self.event(
            incoming,
            Event::Access {
                place: path.place,
                access: if path.through.is_some() {
                    Access::Write
                } else {
                    Access::Replace
                },
                through: path.through,
            },
            assignment.target_span,
        ))
    }

    /// Primitive scalar compound assignment reads its selected target before the right operand, then writes the
    /// same place. Overloaded operators remain calls whose access contract cannot be inferred from their spelling.
    pub(super) fn compound_assignment(
        &mut self,
        assignment: &CompoundAssignmentStmt,
        span: Span,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let info = &self.checker.type_info;
        if info.resolved_operator_call(span).is_some()
            || !matches!(
                info.resolved_write_type(assignment.name_span, &assignment.name),
                Some(ResolvedType::Int | ResolvedType::Float | ResolvedType::Numeric(_) | ResolvedType::Bool)
            )
        {
            return Err(Unavailable {
                span,
                reason: "compound operator needs selected callable access facts".into(),
            });
        }
        let identity = info
            .resolved_write_identity(assignment.name_span, &assignment.name)
            .ok_or(Unavailable {
                span: assignment.name_span,
                reason: "compound target lacks a checked identity".into(),
            })?
            .clone();
        let path = match self.aliases.get(&identity) {
            Some(path) => path.clone(),
            None => Path {
                place: self.root(&identity, assignment.name_span)?,
                through: None,
            },
        };
        let incoming = self.event(
            incoming,
            Event::Access {
                place: path.place.clone(),
                access: Access::Read,
                through: path.through,
            },
            assignment.name_span,
        );
        let incoming = self.expression(&assignment.value, incoming, false)?;
        Ok(self.event(
            incoming,
            Event::Access {
                place: path.place,
                access: Access::Write,
                through: path.through,
            },
            assignment.name_span,
        ))
    }

    /// Build the common header and poll edges of a statement loop or eager comprehension. A stored iterable remains
    /// shared-borrowed through every poll, while an iterator result reuses the loans retained by that result.
    pub(super) fn for_header(
        &mut self,
        _pattern: &Spanned<crate::ast::Pattern>,
        iter: &Spanned<Expr>,
        span: Span,
        incoming: Vec<usize>,
    ) -> Result<(usize, Vec<usize>), Unavailable> {
        if self.checker.type_info.protocols.iterations.contains_key(&iter.span)
            || self.checker.type_info.protocols.iterations.contains_key(&span)
        {
            return Err(Unavailable {
                span: iter.span,
                reason: "protocol iteration needs checked callable access facts".into(),
            });
        }
        let candidate = self.checker.loop_item_candidates.get(&iter.span).cloned();
        let takes_items = self.checker.type_info.for_loop_takes_items(iter.span);
        let iterable = if takes_items { None } else { self.optional_place(iter)? };
        let retained = if takes_items || self.uses_ordinary_iteration_access(iter.span) {
            None
        } else {
            self.retained_result(iter, incoming.clone(), false)?
        };
        let mut evaluated = match &retained {
            Some(result) => result.incoming.clone(),
            None => self.expression(iter, incoming, takes_items)?,
        };
        let iteration = match iterable {
            Some(path) => {
                let place = path.place.clone();
                let loan = self.call_loan(path, false, false, iter.span);
                self.graph.direct_iteration_loans.insert(loan);
                evaluated = self.event(evaluated, Event::Borrow(loan), iter.span);
                Some((loan, place))
            }
            None => None,
        };
        let transfer = self.event(evaluated, Event::Nothing, iter.span)[0];
        let header = self.event(vec![transfer], Event::Nothing, span)[0];
        // The poll reads the iterable through its loan; exhaustion leaves the loop from here.
        let mut poll = match &iteration {
            Some((loan, place)) => self.event(
                vec![header],
                Event::Access {
                    place: place.clone(),
                    access: Access::Read,
                    through: Some(*loan),
                },
                iter.span,
            ),
            None => vec![header],
        };
        if let Some(result) = retained {
            for loan in result.loans {
                let source = self
                    .graph
                    .loans
                    .get(loan.0)
                    .map(|loan| loan.source.clone())
                    .ok_or(Unavailable {
                        span: iter.span,
                        reason: "iterator loop has no retained source loan".into(),
                    })?;
                poll = self.event(
                    poll,
                    Event::Access {
                        place: source,
                        access: Access::Read,
                        through: Some(loan),
                    },
                    iter.span,
                );
            }
        }
        if let Some(candidate) = candidate {
            let item_roots = candidate.items.clone();
            for item in &candidate.items {
                let root = Root::Binding(item.clone());
                // The loop scope has closed before the callable graph is projected, so its ephemeral item bindings
                // are no longer in the symbol table's local inventory. The header creates each item as a fresh,
                // transferable owner; register that permission from the retained checked identity.
                self.graph.permissions.insert(
                    root.clone(),
                    Permission {
                        exclusive: false,
                        // The source binding remains immutable to authored assignments; the graph needs write
                        // permission only for the loop header's producer-owned replacement on every poll.
                        writable: true,
                        transferable: true,
                    },
                );
                self.constrained.insert(root);
            }
            let transfer = iteration.map(|(loan, _)| super::super::loop_items::TransferNodes {
                loan,
                transfer,
                poll: poll[0],
            });
            self.graph
                .item_taking_loops
                .insert(iter.span, super::super::loop_items::GraphLoop { candidate, transfer });
            for item in item_roots {
                poll = self.event(
                    poll,
                    Event::Access {
                        place: Place {
                            root: Root::Binding(item),
                            projection: Vec::new(),
                        },
                        access: Access::Replace,
                        through: None,
                    },
                    span,
                );
            }
        }
        Ok((header, poll))
    }

    /// Iterate an evaluated iterable. A stored iterable stays shared-borrowed while the loop runs: each poll at the
    /// header reads through that loan, so the back edge keeps it live across the body and an edit of the iterable
    /// there conflicts. A loop that takes the list's items consumes it instead. The item is a fresh local declared at
    /// each poll; it is an owned value, never a view of the iterable.
    pub(super) fn for_loop(
        &mut self,
        loop_: &ForStmt,
        span: Span,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let (header, poll) = self.for_header(&loop_.pattern, &loop_.iter, span, incoming)?;
        self.loops.push(LoopEdges {
            header,
            breaks: Vec::new(),
        });
        let body = self.block(&loop_.body, poll.clone())?;
        self.connect(&body, header);
        let edges = self.loops.pop().ok_or(Unavailable {
            span,
            reason: "missing loop owner".into(),
        })?;
        Ok(poll.into_iter().chain(edges.breaks).collect())
    }

    /// Project explicit branches, loop back-edges and exits. Unsupported statement effects refuse the whole profile.
    pub(super) fn statement(
        &mut self,
        statement: &Spanned<Statement>,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        match &statement.node {
            Statement::Pass => Ok(incoming),
            Statement::Expr(Spanned {
                node: Expr::Match(scrutinee, arms),
                ..
            }) => self.match_statement(scrutinee, arms, incoming),
            // A discarded returned view still borrows its source for the call; its result loan is never used. A
            // discarded call that carries a view drops its carrier the same way, with its view and its error.
            Statement::Expr(value) => match self.view_source(value)? {
                Some(view) => Ok(self.evaluate_view_source(value, view, incoming)?.0),
                None => match self.call_relation(value)?.filter(|relation| relation.carried.is_some()) {
                    Some(relation) => Ok(self.evaluate_view_call(value, relation, incoming)?.0),
                    None => self.expression(value, incoming, false),
                },
            },
            Statement::Assert(assertion) => {
                let crate::ast::AssertKind::Condition(condition) = &assertion.kind else {
                    return Err(Unavailable {
                        span: statement.span,
                        reason: "pattern assertion needs checked binding access".into(),
                    });
                };
                if self.checker.type_info.expr_type(condition.span) != Some(&ResolvedType::Bool) {
                    return Err(Unavailable {
                        span: condition.span,
                        reason: "assertion truthiness needs checked callable access".into(),
                    });
                }
                let incoming = self.expression(condition, incoming, false)?;
                if let Some(message) = &assertion.message {
                    if !matches!(
                        self.checker.type_info.expr_type(message.span),
                        Some(ResolvedType::Str | ResolvedType::FrozenStr)
                    ) {
                        return Err(Unavailable {
                            span: message.span,
                            reason: "assertion message needs checked display access".into(),
                        });
                    }
                    // The failure message is evaluated only on the terminating failure path. Its reads remain in
                    // the graph, but its transfers do not flow into statements on the successful path.
                    self.expression(message, incoming.clone(), false)?;
                }
                Ok(incoming)
            }
            Statement::Return(value) => {
                match (value, self.view.clone(), self.owned_source.clone()) {
                    (Some(value), Some((exclusive, source)), None)
                        if !self.view_component.is_empty() && self.view_carried.is_none() =>
                    {
                        self.aggregate_return(value, &self.view_component.clone(), exclusive, &source, incoming)?;
                    }
                    (Some(value), Some((exclusive, source)), None) => match self.view_carried {
                        Some(payload) => self.carried_return(value, payload, exclusive, &source, incoming)?,
                        None => {
                            self.view_return(value, exclusive, &source, incoming)?;
                        }
                    },
                    (None, Some(_), None) => self.view_errors.push(view_error(
                        "A returned view must return its declared source place",
                        statement.span,
                    )),
                    (Some(value), None, Some(source)) => {
                        self.owned_return(value, &source, incoming)?;
                    }
                    (None, None, Some(_)) => {
                        return Err(Unavailable {
                            span: statement.span,
                            reason: "owned result dependency requires a returned value".into(),
                        });
                    }
                    (Some(value), None, None) => {
                        if let Some(result) = self.retained_result(value, incoming.clone(), true)? {
                            self.escape_retained_result(result, value.span)?;
                        } else if let Some(result) = self.retained_carrier(value, incoming.clone(), true)? {
                            self.escape_retained_result(result, value.span)?;
                        } else if let Some(result) = self.stored_result(value, incoming.clone())? {
                            self.escape_retained_result(result, value.span)?;
                        } else {
                            self.expression(value, incoming, true)?;
                        }
                    }
                    (None, None, None) => {}
                    (_, Some(_), Some(_)) => {
                        return Err(Unavailable {
                            span: statement.span,
                            reason: "result cannot be both a returned view and owned dependency".into(),
                        });
                    }
                }
                Ok(Vec::new())
            }
            Statement::Assignment(assignment) => self.assignment(assignment, statement.span, incoming),
            Statement::TupleUnpack(unpack) => self.tuple_unpack(unpack, incoming),
            Statement::FieldAssignment(assignment) => self.field_assignment(assignment, incoming),
            Statement::CompoundAssignment(assignment) => self.compound_assignment(assignment, statement.span, incoming),
            Statement::If(branch) => self.if_statement(branch, incoming),
            Statement::While(loop_) => {
                let header = self.event(incoming, Event::Nothing, statement.span)[0];
                let (condition, exit, dictionary_bindings) = match &loop_.condition {
                    Condition::Let { pattern, value } => {
                        match self.dictionary_condition(pattern, value, vec![header])? {
                            Some((matched, unmatched, bindings)) => (matched, unmatched, bindings),
                            None => (self.condition(&loop_.condition, vec![header])?, Vec::new(), Vec::new()),
                        }
                    }
                    Condition::Expr(_) => {
                        let evaluated = self.condition(&loop_.condition, vec![header])?;
                        (evaluated.clone(), evaluated, Vec::new())
                    }
                };
                self.loops.push(LoopEdges {
                    header,
                    breaks: Vec::new(),
                });
                let body = self.block(&loop_.body, condition.clone())?;
                self.leave_dictionary_bindings(&dictionary_bindings);
                self.connect(&body, header);
                let loop_ = self.loops.pop().ok_or(Unavailable {
                    span: statement.span,
                    reason: "missing loop owner".into(),
                })?;
                Ok(exit.into_iter().chain(loop_.breaks).collect())
            }
            Statement::Loop(loop_) => {
                let header = self.event(incoming, Event::Nothing, statement.span)[0];
                self.loops.push(LoopEdges {
                    header,
                    breaks: Vec::new(),
                });
                let body = self.block(&loop_.body, vec![header])?;
                self.connect(&body, header);
                Ok(self
                    .loops
                    .pop()
                    .ok_or(Unavailable {
                        span: statement.span,
                        reason: "missing loop owner".into(),
                    })?
                    .breaks)
            }
            Statement::For(loop_) => self.for_loop(loop_, statement.span, incoming),
            Statement::Break(None) => {
                self.loops
                    .last_mut()
                    .ok_or(Unavailable {
                        span: statement.span,
                        reason: "break without loop".into(),
                    })?
                    .breaks
                    .extend(incoming);
                Ok(Vec::new())
            }
            Statement::Break(Some(value)) => {
                let outgoing = self.expression(value, incoming, true)?;
                self.loops
                    .last_mut()
                    .ok_or(Unavailable {
                        span: statement.span,
                        reason: "break without loop".into(),
                    })?
                    .breaks
                    .extend(outgoing);
                Ok(Vec::new())
            }
            Statement::Continue => {
                let header = self
                    .loops
                    .last()
                    .ok_or(Unavailable {
                        span: statement.span,
                        reason: "continue without loop".into(),
                    })?
                    .header;
                self.connect(&incoming, header);
                Ok(Vec::new())
            }
            _ => Err(Unavailable {
                span: statement.span,
                reason: "statement needs additional checked dependency facts".into(),
            }),
        }
    }
}
