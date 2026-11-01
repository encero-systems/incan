//! Source control-flow projection for explicit inputs and selected calls. Unsupported expressions refuse this
//! profile; an empty graph is never substituted for missing call, capture, suspension or returned-dependency facts.

use std::collections::{BTreeMap, BTreeSet};

use super::TypeChecker;
use super::flow::{
    Access, Event, Graph, Loan, LoanId, LoanKind, Node, OwnedInitialization, Permission, Place, Projection, Root,
};
use crate::ast::{
    AssignmentStmt, BinaryOp, BindingKind, CompoundAssignmentStmt, Condition, Expr, FieldAssignmentStmt, FunctionDecl,
    MethodDecl, Param, Receiver, Span, Spanned, Statement,
};
use crate::symbols::{ResolvedType, SymbolKind};
use incan_lang::lang::derives::DeriveId;
use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};

mod calls;
mod values;

/// A source shape whose access behavior has not been projected. The anchor is the unsupported operation itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::typechecker) struct Unavailable {
    pub span: Span,
    pub reason: String,
}

/// The source callable whose already prepared declaration facts govern this body. Methods retain a distinct
/// receiver declaration; neither a synthetic function nor a name-based signature lookup replaces that identity.
#[derive(Clone, Copy)]
pub(in crate::typechecker) enum CallableSource<'a> {
    Function(&'a FunctionDecl),
    Method(&'a MethodDecl),
}

/// One enclosing loop owns its back-edge and the paths that leave it; nested loops cannot steal each other's exits.
struct LoopEdges {
    header: usize,
    breaks: Vec<usize>,
}

/// A local reference names its source place and access route; it never becomes a second owned storage root.
#[derive(Debug, Clone)]
struct Path {
    place: Place,
    through: Option<LoanId>,
}

/// Invocation-local projection state, borrowing immutable checked identities and types rather than resolving names.
struct Builder<'a> {
    checker: &'a TypeChecker,
    owner: CanonicalSymbolId,
    graph: Graph,
    locals: BTreeMap<CanonicalSymbolId, Permission>,
    aliases: BTreeMap<CanonicalSymbolId, Path>,
    constrained: BTreeSet<Root>,
    loops: Vec<LoopEdges>,
}

/// Project a checked callable body against its declaration and local contracts. Calls without selected access
/// facts, escaping dependencies and other incomplete shapes remain explicit unavailable results. The caller supplies
/// its checked owner directly; span-keyed inventories shared by multiple modules cannot select another owner.
pub(super) fn build(
    checker: &TypeChecker,
    callable: CallableSource<'_>,
    span: Span,
    identity: &CanonicalSymbolId,
) -> Result<Graph, Unavailable> {
    let (parameters, body, is_async, receiver, name, kind) = match callable {
        CallableSource::Function(function) => (
            &function.params,
            &function.body,
            function.is_async(),
            None,
            &function.name,
            SemanticSourceTargetKind::Function,
        ),
        CallableSource::Method(method) => (
            &method.params,
            method.body.as_ref().ok_or(Unavailable {
                span,
                reason: "abstract method has no body".into(),
            })?,
            method.is_async(),
            method.receiver_binding.as_ref().zip(method.receiver),
            &method.name,
            SemanticSourceTargetKind::Method,
        ),
    };
    if identity.declaration_span != span || identity.declaration_name != *name || identity.kind != kind {
        return Err(Unavailable {
            span,
            reason: "checked owner does not match the callable declaration".into(),
        });
    }
    let contract = checker.type_info.declarations.ownership_contracts.get(identity);
    if is_async
        || contract.is_some_and(|contract| {
            contract.result_source.is_some()
                || contract.modes.iter().any(|mode| {
                    mode.component.parameter.is_none()
                        || !mode.component.children.is_empty()
                        || mode.mode == OwnershipMode::Reference
                })
        })
    {
        return Err(Unavailable {
            span,
            reason: "escaping or nested ownership contract".into(),
        });
    }
    let mut builder = Builder {
        checker,
        owner: identity.clone(),
        locals: checker
            .symbols
            .all_symbols()
            .iter()
            .enumerate()
            .filter_map(|(index, symbol)| {
                let identity = checker.symbols.identity_of(index)?;
                let SymbolKind::Variable(variable) = &symbol.kind else {
                    return None;
                };
                (identity.kind == SemanticSourceTargetKind::Local).then(|| {
                    (
                        identity.clone(),
                        Permission {
                            exclusive: variable.is_mutable,
                            writable: variable.is_mutable,
                            transferable: true,
                        },
                    )
                })
            })
            .collect(),
        graph: Graph {
            owner: identity.clone(),
            permissions: BTreeMap::new(),
            loans: Vec::new(),
            local_loans: BTreeMap::new(),
            owned_locals: BTreeMap::new(),
            invocations: BTreeMap::new(),
            nodes: vec![Node {
                event: Event::Nothing,
                successors: Vec::new(),
                span,
            }],
            entry: 0,
        },
        constrained: BTreeSet::new(),
        aliases: BTreeMap::new(),
        loops: Vec::new(),
    };
    builder.inputs(parameters, identity)?;
    if let Some((binding, receiver)) = receiver {
        let root = builder.binding_root(&binding.node, binding.span, SemanticSourceTargetKind::Receiver)?;
        let writable = receiver == Receiver::Mutable;
        builder.graph.permissions.insert(
            Root::Binding(root),
            Permission {
                exclusive: writable,
                writable,
                transferable: false,
            },
        );
    }
    builder.block(body, vec![0])?;
    Ok(builder.graph)
}

impl Builder<'_> {
    /// Select a prepared local binding by its canonical identity and exact declaration token. During body checking
    /// the declaration output may not be joined yet, so only the still-active exact frame may supply that identity.
    fn binding_root(
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
    fn inputs(&mut self, parameters: &[Spanned<Param>], identity: &CanonicalSymbolId) -> Result<(), Unavailable> {
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
                self.constrained.insert(Root::Binding(root));
            }
        }
        Ok(())
    }

    /// Connect each current predecessor to one evaluated operation, preserving distinct branch paths.
    fn event(&mut self, incoming: Vec<usize>, event: Event, span: Span) -> Vec<usize> {
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
    fn connect(&mut self, incoming: &[usize], next: usize) {
        for previous in incoming {
            self.graph.nodes[*previous].successors.push(next);
        }
    }

    /// Register a checked local owner once. Parameters must arrive from the prepared declaration, not this fallback.
    fn root(&mut self, identity: &CanonicalSymbolId, span: Span) -> Result<Place, Unavailable> {
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
    fn place(&mut self, expression: &Spanned<Expr>) -> Result<Path, Unavailable> {
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
            _ => Err(Unavailable {
                span: expression.span,
                reason: "expression is not a stored place".into(),
            }),
        }
    }

    /// Index computations run before access, exactly once; projection lookup itself is not another read of the owner.
    fn evaluate_place(&mut self, expression: &Spanned<Expr>, incoming: Vec<usize>) -> Result<Vec<usize>, Unavailable> {
        match &expression.node {
            Expr::Ident(_) | Expr::SelfExpr => Ok(incoming),
            Expr::Paren(base) | Expr::Field(base, _) => self.evaluate_place(base, incoming),
            Expr::Index(base, index) => {
                let incoming = self.evaluate_place(base, incoming)?;
                self.expression(index, incoming, false)
            }
            _ => Err(Unavailable {
                span: expression.span,
                reason: "unsupported place evaluation".into(),
            }),
        }
    }

    /// Evaluate an expression in source order. A checked Copy read is not a transfer; ordinary unconstrained values
    /// retain their existing semantics until the separate ordinary-binding migration ships.
    fn expression(
        &mut self,
        expression: &Spanned<Expr>,
        incoming: Vec<usize>,
        consume: bool,
    ) -> Result<Vec<usize>, Unavailable> {
        if self
            .checker
            .type_info
            .calls
            .resolved_operator_calls
            .contains_key(&expression.span)
        {
            return Err(Unavailable {
                span: expression.span,
                reason: "operator needs selected callable access facts".into(),
            });
        }
        match &expression.node {
            Expr::Ident(_) | Expr::SelfExpr | Expr::Field(_, _) | Expr::Index(_, _) => {
                let place = self.place(expression)?;
                let incoming = self.evaluate_place(expression, incoming)?;
                let ty = self.checker.type_info.expr_type(expression.span).ok_or(Unavailable {
                    span: expression.span,
                    reason: "missing checked access type".into(),
                })?;
                let access = if consume
                    && (place.through.is_some() || self.constrained.contains(&place.place.root))
                    && !self.copy_value(ty)
                {
                    Access::Move
                } else {
                    Access::Read
                };
                Ok(self.event(
                    incoming,
                    Event::Access {
                        place: place.place,
                        access,
                        through: place.through,
                    },
                    expression.span,
                ))
            }
            Expr::Literal(_) => Ok(incoming),
            Expr::List(entries) => self.scalar_list(entries, incoming),
            Expr::FString(parts) => self.formatted_text(parts, incoming),
            Expr::Call(callee, _, arguments) => self.call(callee, arguments, expression.span, incoming),
            Expr::MethodCall(receiver, _, _, arguments) => {
                self.apply_call(Some(receiver), arguments, expression.span, incoming)
            }
            Expr::Paren(inner) | Expr::Unary(_, inner) => self.expression(inner, incoming, consume),
            Expr::Binary(left, operation, right) => {
                let incoming = self.expression(left, incoming, false)?;
                let mut outgoing = self.expression(right, incoming.clone(), false)?;
                if matches!(operation, BinaryOp::And | BinaryOp::Or) {
                    outgoing.extend(incoming);
                }
                Ok(outgoing)
            }
            _ => Err(Unavailable {
                span: expression.span,
                reason: "expression needs checked call or dependency access facts".into(),
            }),
        }
    }

    /// Use the checker's derive relation for transfer proofs rather than the legacy ABI adaptation heuristic.
    /// Shared references copy their address; exclusive references transfer access. Missing Copy evidence never
    /// authorizes duplication, including inside tuples and native carriers with incomplete trait metadata.
    fn copy_value(&self, ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::Ref(_) => true,
            ResolvedType::RefMut(_) => false,
            ResolvedType::Tuple(items) => items.iter().all(|item| self.copy_value(item)),
            _ => matches!(
                self.checker.derive_support(ty, DeriveId::Copy),
                super::super::derive_requirements::DeriveSupport::Supported
            ),
        }
    }

    /// Patterns need binding-specific dependency transfer and are unavailable until their checked projection exists.
    fn condition(&mut self, condition: &Condition, incoming: Vec<usize>) -> Result<Vec<usize>, Unavailable> {
        match condition {
            Condition::Expr(value) => self.expression(value, incoming, false),
            Condition::Let { .. } => Err(Unavailable {
                span: condition.span(),
                reason: "pattern condition dependency".into(),
            }),
        }
    }

    /// Sequence reachable statements. A return/break/continue ends its path; later statements are not successors.
    fn block(
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

    /// Distinguish stored sources from fresh computed values without swallowing missing checked projection facts.
    fn optional_place(&mut self, expression: &Spanned<Expr>) -> Result<Option<Path>, Unavailable> {
        match &expression.node {
            Expr::Ident(_) | Expr::SelfExpr | Expr::Field(_, _) | Expr::Index(_, _) => self.place(expression).map(Some),
            Expr::Paren(inner) => self.optional_place(inner),
            _ => Ok(None),
        }
    }

    /// Bind an explicit local loan to its exact source, or transfer an owned value. Unannotated declarations keep
    /// their existing behavior; reassignment through an already borrowed local writes through without retargeting.
    fn assignment(
        &mut self,
        assignment: &AssignmentStmt,
        span: Span,
        incoming: Vec<usize>,
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
            let access_node = if source.is_some() {
                let [node] = incoming.as_slice() else {
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
                source: source.as_ref().map(|source| source.place.clone()),
                access_node,
            };
            if self.graph.owned_locals.insert(root.clone(), retained).is_some() {
                return Err(Unavailable {
                    span: assignment.name_span,
                    reason: "owned local has more than one initializer".into(),
                });
            }
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

    /// Evaluate a stored field destination before its replacement value. Only canonical stored fields participate;
    /// computed setters remain unavailable. A primitive compound update already contains its checked read/operator
    /// expression; only static stored projections can reuse it without repeating an effectful destination.
    fn field_assignment(
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
    fn compound_assignment(
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

    /// Project explicit branches, loop back-edges and exits. Unsupported statement effects refuse the whole profile.
    fn statement(&mut self, statement: &Spanned<Statement>, incoming: Vec<usize>) -> Result<Vec<usize>, Unavailable> {
        match &statement.node {
            Statement::Pass => Ok(incoming),
            Statement::Expr(value) => self.expression(value, incoming, false),
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
                if let Some(value) = value {
                    self.expression(value, incoming, true)?;
                }
                Ok(Vec::new())
            }
            Statement::Assignment(assignment) => self.assignment(assignment, statement.span, incoming),
            Statement::FieldAssignment(assignment) => self.field_assignment(assignment, incoming),
            Statement::CompoundAssignment(assignment) => self.compound_assignment(assignment, statement.span, incoming),
            Statement::If(branch) => {
                let mut remaining = self.condition(&branch.condition, incoming)?;
                let mut outgoing = self.block(&branch.then_body, remaining.clone())?;
                for (condition, body) in &branch.elif_branches {
                    remaining = self.expression(condition, remaining, false)?;
                    outgoing.extend(self.block(body, remaining.clone())?);
                }
                outgoing.extend(match &branch.else_body {
                    Some(body) => self.block(body, remaining)?,
                    None => remaining,
                });
                Ok(outgoing)
            }
            Statement::While(loop_) => {
                let header = self.event(incoming, Event::Nothing, statement.span)[0];
                let condition = self.condition(&loop_.condition, vec![header])?;
                self.loops.push(LoopEdges {
                    header,
                    breaks: Vec::new(),
                });
                let body = self.block(&loop_.body, condition.clone())?;
                self.connect(&body, header);
                let loop_ = self.loops.pop().ok_or(Unavailable {
                    span: statement.span,
                    reason: "missing loop owner".into(),
                })?;
                Ok(condition.into_iter().chain(loop_.breaks).collect())
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
