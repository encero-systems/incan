//! Source graph builder places, roots, events, branches, blocks and returned views.

use super::*;

impl Builder<'_> {
    /// Select a prepared local binding by its canonical identity and exact declaration token. During body checking
    /// the declaration output may not be joined yet, so only the still-active exact frame may supply that identity.
    pub(super) fn binding_root(
        &self,
        name: &str,
        span: Span,
        kind: SemanticSourceTargetKind,
    ) -> Result<CanonicalSymbolId, Unavailable> {
        self.checker
            .type_info
            .resolved_write_identity(span, name)
            .or_else(|| {
                let selected = self.checker.symbols.lookup(name)?;
                let identity = self.checker.symbols.identity_of(selected)?;
                (identity.kind == kind && identity.declaration_span == span).then_some(identity)
            })
            .cloned()
            .ok_or(Unavailable {
                span,
                reason: "missing prepared parameter identity".into(),
            })
    }

    /// Parameter slot contracts describe permissions independently of body use or the spelling of a shadowed mode.
    pub(super) fn inputs(
        &mut self,
        parameters: &[Spanned<Param>],
        identity: &CanonicalSymbolId,
    ) -> Result<(), Unavailable> {
        for (slot, parameter) in parameters.iter().enumerate() {
            let root = self.binding_root(
                &parameter.node.name,
                parameter.span,
                SemanticSourceTargetKind::Parameter,
            )?;
            let mode = self.checker.checked_input_contract(identity, slot);
            let writable = parameter.node.is_mut && mode.is_none_or(|mode| mode.mode != OwnershipMode::Borrowed);
            let exclusive = mode.map_or(writable, |mode| {
                mode.mode == OwnershipMode::BorrowedMut || (mode.mode == OwnershipMode::Owned && writable)
            });
            self.graph.permissions.insert(
                Root::Binding(root.clone()),
                Permission {
                    exclusive,
                    writable,
                    transferable: mode.is_none_or(|mode| mode.mode == OwnershipMode::Owned),
                },
            );
            if mode.is_some() {
                if mode.is_some_and(|mode| matches!(mode.mode, OwnershipMode::Borrowed | OwnershipMode::BorrowedMut)) {
                    self.borrowed_inputs.insert(Root::Binding(root.clone()));
                }
                self.constrained.insert(Root::Binding(root));
            }
        }
        Ok(())
    }

    /// Connect each current predecessor to one evaluated operation, preserving distinct branch paths.
    pub(super) fn event(&mut self, incoming: Vec<usize>, event: Event, span: Span) -> Vec<usize> {
        let next = self.graph.nodes.len();
        self.graph.nodes.push(Node {
            event,
            successors: Vec::new(),
            span,
        });
        self.connect(&incoming, next);
        vec![next]
    }

    /// Join already evaluated paths without introducing an operation or a new owner identity.
    pub(super) fn connect(&mut self, incoming: &[usize], next: usize) {
        for previous in incoming {
            self.graph.nodes[*previous].successors.push(next);
        }
    }

    /// Register a checked local owner once. Parameters must arrive from the prepared declaration, not this fallback.
    pub(super) fn root(&mut self, identity: &CanonicalSymbolId, span: Span) -> Result<Place, Unavailable> {
        let root = Root::Binding(identity.clone());
        if !self.graph.permissions.contains_key(&root) {
            if identity.kind != SemanticSourceTargetKind::Local {
                return Err(Unavailable {
                    span,
                    reason: "place is not a prepared parameter or checked local".into(),
                });
            }
            let permission = self.locals.get(identity).copied().ok_or(Unavailable {
                span,
                reason: "missing checked local permissions".into(),
            })?;
            self.graph.permissions.insert(root.clone(), permission);
        }
        Ok(Place {
            root,
            projection: Vec::new(),
        })
    }

    /// Read only checked storage projections. Properties and native accessors are calls, even when spelled as fields.
    pub(super) fn place(&mut self, expression: &Spanned<Expr>) -> Result<Path, Unavailable> {
        match &expression.node {
            Expr::Ident(_) | Expr::SelfExpr => {
                let identity = self
                    .checker
                    .type_info
                    .resolved_identity(expression.span)
                    .ok_or(Unavailable {
                        span: expression.span,
                        reason: "missing checked place identity".into(),
                    })?
                    .clone();
                if self.carriers.contains_key(&identity) {
                    return Err(Unavailable {
                        span: expression.span,
                        reason: "a bound carried view needs a match or `?` to take the view out".into(),
                    });
                }
                if self.taken_carriers.contains(&identity) {
                    return Err(taken_carrier(expression.span));
                }
                if self.closures.contains_key(&identity) {
                    return Err(Unavailable {
                        span: expression.span,
                        reason: "a closure local that is returned, stored or \
                                 passed"
                            .into(),
                    });
                }
                match self.aliases.get(&identity) {
                    Some(path) => Ok(path.clone()),
                    None => Ok(Path {
                        place: self.root(&identity, expression.span)?,
                        through: None,
                    }),
                }
            }
            Expr::Paren(inner) => self.place(inner),
            Expr::Field(base, _) => {
                if self
                    .checker
                    .type_info
                    .computed_property_access(expression.span)
                    .is_some()
                {
                    return Err(Unavailable {
                        span: expression.span,
                        reason: "computed property access".into(),
                    });
                }
                let identity = self
                    .checker
                    .type_info
                    .resolved_identity(expression.span)
                    .filter(|identity| identity.kind == SemanticSourceTargetKind::Field)
                    .ok_or(Unavailable {
                        span: expression.span,
                        reason: "field lacks a checked stored identity".into(),
                    })?
                    .clone();
                let mut place = self.place(base)?;
                place.place.projection.push(Projection::Field(identity));
                Ok(place)
            }
            Expr::Index(base, index) => {
                let mut place = self.place(base)?;
                place.place.projection.push(Projection::Element(index.span));
                Ok(place)
            }
            Expr::MethodCall(..) => {
                match super::super::builtins::byte_slice_view(&self.checker.type_info, expression) {
                    Some(receiver) => self.place(receiver),
                    None => Err(Unavailable {
                        span: expression.span,
                        reason: "expression is not a stored place".into(),
                    }),
                }
            }
            _ => Err(Unavailable {
                span: expression.span,
                reason: "expression is not a stored place".into(),
            }),
        }
    }

    /// Index computations run before access, exactly once; projection lookup itself is not another read of the owner.
    pub(super) fn evaluate_place(
        &mut self,
        expression: &Spanned<Expr>,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        match &expression.node {
            Expr::Ident(_) | Expr::SelfExpr => Ok(incoming),
            Expr::Paren(base) | Expr::Field(base, _) => self.evaluate_place(base, incoming),
            Expr::Index(base, index) => {
                let incoming = self.evaluate_place(base, incoming)?;
                self.expression(index, incoming, false)
            }
            Expr::MethodCall(..) => {
                match super::super::builtins::byte_slice_view(&self.checker.type_info, expression) {
                    Some(receiver) => self.evaluate_place(receiver, incoming),
                    None => Err(Unavailable {
                        span: expression.span,
                        reason: "unsupported place evaluation".into(),
                    }),
                }
            }
            _ => Err(Unavailable {
                span: expression.span,
                reason: "unsupported place evaluation".into(),
            }),
        }
    }

    /// Evaluate a stored projection whose root is a checked returned-view call. The call creates the retained loan;
    /// field and element projections then narrow the access without pretending the call expression is stored.
    pub(super) fn projected_view_place(
        &mut self,
        expression: &Spanned<Expr>,
        incoming: Vec<usize>,
    ) -> Result<Option<(Vec<usize>, Path)>, Unavailable> {
        match &expression.node {
            Expr::Paren(inner) => self.projected_view_place(inner, incoming),
            Expr::Field(base, _) => {
                let Some((incoming, mut path)) = self.projected_view_place(base, incoming)? else {
                    return Ok(None);
                };
                if self
                    .checker
                    .type_info
                    .computed_property_access(expression.span)
                    .is_some()
                {
                    return Err(Unavailable {
                        span: expression.span,
                        reason: "computed property access".into(),
                    });
                }
                let identity = self
                    .checker
                    .type_info
                    .resolved_identity(expression.span)
                    .filter(|identity| identity.kind == SemanticSourceTargetKind::Field)
                    .ok_or(Unavailable {
                        span: expression.span,
                        reason: "field lacks a checked stored identity".into(),
                    })?
                    .clone();
                path.place.projection.push(Projection::Field(identity));
                Ok(Some((incoming, path)))
            }
            Expr::Index(base, index) => {
                let Some((incoming, mut path)) = self.projected_view_place(base, incoming)? else {
                    return Ok(None);
                };
                let incoming = self.expression(index, incoming, false)?;
                path.place.projection.push(Projection::Element(index.span));
                Ok(Some((incoming, path)))
            }
            _ => {
                let Some(view) = self.view_source(expression)? else {
                    return Ok(None);
                };
                self.evaluate_view_source(expression, view, incoming)
                    .map(|result| Some(result))
            }
        }
    }

    /// Use the checker's derive relation for transfer proofs rather than the legacy ABI adaptation heuristic.
    /// Shared references copy their address; exclusive references transfer access. Missing Copy evidence never
    /// authorizes duplication, including inside tuples and native carriers with incomplete trait metadata.
    pub(super) fn copy_value(&self, ty: &ResolvedType) -> bool {
        self.checker.input_copy_value(ty)
    }

    /// A statement `if` evaluates each condition on the path where the previous ones were false, and its branches,
    /// including an absent `else`, join after it. Each branch starts from the carriers as that path left them.
    pub(super) fn if_statement(&mut self, branch: &IfStmt, incoming: Vec<usize>) -> Result<Vec<usize>, Unavailable> {
        let (then_input, mut remaining, dictionary_bindings) = match &branch.condition {
            Condition::Let { pattern, value } => match self.dictionary_condition(pattern, value, incoming.clone())? {
                Some((matched, unmatched, bindings)) => (matched, unmatched, bindings),
                None => (self.condition(&branch.condition, incoming)?, Vec::new(), Vec::new()),
            },
            Condition::Expr(_) => {
                let evaluated = self.condition(&branch.condition, incoming)?;
                (evaluated.clone(), evaluated, Vec::new())
            }
        };
        let mut fallthrough = self.carrier_state();
        let mut ends = Vec::new();
        let mut outgoing = self.block(&branch.then_body, then_input)?;
        self.leave_dictionary_bindings(&dictionary_bindings);
        if !outgoing.is_empty() {
            ends.push(self.carrier_state());
        }
        for (condition, body) in &branch.elif_branches {
            self.restore_carrier_state(&fallthrough);
            remaining = self.expression(condition, remaining, false)?;
            fallthrough = self.carrier_state();
            let continuing = self.block(body, remaining.clone())?;
            if !continuing.is_empty() {
                ends.push(self.carrier_state());
            }
            outgoing.extend(continuing);
        }
        self.restore_carrier_state(&fallthrough);
        let continuing = match &branch.else_body {
            Some(body) => self.block(body, remaining)?,
            None => remaining,
        };
        if !continuing.is_empty() {
            ends.push(self.carrier_state());
        }
        outgoing.extend(continuing);
        self.join_carrier_states(&fallthrough, &ends);
        Ok(outgoing)
    }

    /// Patterns need binding-specific dependency transfer and are unavailable until their checked projection exists.
    pub(super) fn condition(&mut self, condition: &Condition, incoming: Vec<usize>) -> Result<Vec<usize>, Unavailable> {
        match condition {
            Condition::Expr(value) => self.expression(value, incoming, false),
            Condition::Let { .. } => Err(Unavailable {
                span: condition.span(),
                reason: "pattern condition dependency".into(),
            }),
        }
    }

    /// Sequence reachable statements. A return/break/continue ends its path; later statements are not successors.
    pub(super) fn block(
        &mut self,
        statements: &[Spanned<Statement>],
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        for statement in statements {
            if incoming.is_empty() {
                break;
            }
            incoming = self.statement(statement, incoming)?;
        }
        Ok(incoming)
    }

    /// A returned view borrows exactly its declared source place, with the view's access; the borrow checks the
    /// source permission, so a shared input cannot back an exclusive view. Any other value is a program error.
    /// Returns the nodes after the borrow, for the elements of a tuple evaluated after it.
    pub(in crate::typechecker::ownership_contracts) fn view_return(
        &mut self,
        value: &Spanned<Expr>,
        exclusive: bool,
        source: &Place,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        // A call returning a view of exactly the declared source satisfies the relation through its result loan.
        if let Some(view) = self.view_source(value)? {
            let (incoming, path) = self.evaluate_view_source(value, view, incoming)?;
            if path.place != *source {
                self.view_errors.push(view_error(
                    "A returned view must return its declared source place; this call returns a view of a \
                     different place",
                    value.span,
                ));
                return Ok(incoming);
            }
            let loan = self.call_loan(path, exclusive, exclusive, value.span);
            return Ok(self.event(incoming, Event::Borrow(loan), value.span));
        }
        match self.optional_place(value)? {
            Some(path) if path.place == *source => {
                let incoming = self.evaluate_place(value, incoming)?;
                let loan = self.call_loan(path, exclusive, exclusive, value.span);
                Ok(self.event(incoming, Event::Borrow(loan), value.span))
            }
            Some(_) => {
                self.view_errors.push(view_error(
                    "A returned view must return its declared source place; this returns a different place",
                    value.span,
                ));
                self.expression(value, incoming, true)
            }
            None => {
                self.view_errors.push(view_error(
                    "A returned view cannot borrow a value created in this body; return its declared source place",
                    value.span,
                ));
                self.expression(value, incoming, true)
            }
        }
    }

    /// Prove a top-level owned return against its declared source dependency. Native lifetime-bearing values and
    /// another source-dependent owned call must retain that exact source. An otherwise independent computation may
    /// still publish the conservative declared dependency after its ordinary value evaluation completes.
    pub(in crate::typechecker::ownership_contracts) fn owned_return(
        &mut self,
        value: &Spanned<Expr>,
        source: &Place,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        if let Some(relation) = self.native_result_relation(value)?
            && !relation.sources.is_empty()
        {
            let (incoming, loans) = self.evaluate_native_result(value, &relation, incoming)?;
            if loans.is_empty()
                || loans
                    .iter()
                    .any(|loan| self.graph.loans.get(loan.0).is_none_or(|loan| loan.source != *source))
            {
                return Err(Unavailable {
                    span: value.span,
                    reason: "owned result does not retain exactly its declared source".into(),
                });
            }
            return Ok(incoming);
        }
        if let Some(relation) = self.owned_relation(value)? {
            let (incoming, path) = self.evaluate_view_call(value, relation, incoming)?;
            if path.place != *source {
                return Err(Unavailable {
                    span: value.span,
                    reason: "owned result call retains a different source".into(),
                });
            }
            return Ok(incoming);
        }
        self.expression(value, incoming, true)
    }

    /// Distinguish stored sources from fresh computed values without swallowing missing checked projection facts.
    pub(super) fn optional_place(&mut self, expression: &Spanned<Expr>) -> Result<Option<Path>, Unavailable> {
        match &expression.node {
            Expr::Ident(_) | Expr::SelfExpr | Expr::Field(_, _) | Expr::Index(_, _) => self.place(expression).map(Some),
            Expr::Paren(inner) => self.optional_place(inner),
            Expr::MethodCall(..)
                if super::super::builtins::byte_slice_view(&self.checker.type_info, expression).is_some() =>
            {
                self.place(expression).map(Some)
            }
            _ => Ok(None),
        }
    }
}
