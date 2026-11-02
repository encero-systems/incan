//! Source control-flow projection for explicit inputs and selected calls. Unsupported expressions refuse this
//! profile; an empty graph is never substituted for missing call, capture, suspension or returned-dependency facts.

use std::collections::{BTreeMap, BTreeSet};

use super::TypeChecker;
use super::flow::{
    Access, Event, Graph, Loan, LoanId, LoanKind, Node, OperandDependency, OperationInputs, OwnedInitialization,
    Permission, Place, Projection, Root,
};
use crate::ast::{
    AssignmentStmt, BinaryOp, BindingKind, CompoundAssignmentStmt, Condition, Expr, FieldAssignmentStmt, ForStmt,
    FunctionDecl, MethodDecl, Param, Receiver, Span, Spanned, Statement,
};
use crate::ownership_metadata::SourceSlot;
use crate::symbols::{ResolvedType, SymbolKind};
use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};

mod calls;
mod matches;
mod operations;
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

impl<'a> CallableSource<'a> {
    /// Source declaration spelling; the canonical owner identity, not this name, selects the callable.
    pub(in crate::typechecker) fn name(self) -> &'a str {
        match self {
            Self::Function(function) => &function.name,
            Self::Method(method) => &method.name,
        }
    }

    /// Declared source parameters in slot order, excluding a method's receiver.
    pub(in crate::typechecker) fn params(self) -> &'a [Spanned<Param>] {
        match self {
            Self::Function(function) => &function.params,
            Self::Method(method) => &method.params,
        }
    }

    /// Statements of the body; an abstract method has none.
    pub(in crate::typechecker) fn body(self) -> &'a [Spanned<Statement>] {
        match self {
            Self::Function(function) => &function.body,
            Self::Method(method) => method.body.as_deref().unwrap_or_default(),
        }
    }

    /// A method's receiver binding token with its mode; functions have no receiver.
    pub(in crate::typechecker) fn receiver(self) -> Option<(&'a Spanned<crate::ast::Ident>, Receiver)> {
        match self {
            Self::Function(_) => None,
            Self::Method(method) => method.receiver_binding.as_ref().zip(method.receiver),
        }
    }
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
    borrowed_inputs: BTreeSet<Root>,
    loops: Vec<LoopEdges>,
    /// The declared returned view: whether it is exclusive, and the exact source place every return borrows.
    view: Option<(bool, Place)>,
    /// Whether the returned view is carried as a `Result` or `Option` payload rather than returned whole.
    view_carried: bool,
    /// Returns that do not borrow the declared view source. These are program errors, reported by the caller.
    view_errors: Vec<crate::diagnostics::CompileError>,
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
    build_reporting_views(checker, callable, span, identity).0
}

/// Build the body's graph and also return the diagnostics for returns that do not borrow the declared view.
pub(super) fn build_reporting_views(
    checker: &TypeChecker,
    callable: CallableSource<'_>,
    span: Span,
    identity: &CanonicalSymbolId,
) -> (Result<Graph, Unavailable>, Vec<crate::diagnostics::CompileError>) {
    let mut view_errors = Vec::new();
    let graph = build_graph(checker, callable, span, identity, &mut view_errors);
    (graph, view_errors)
}

fn build_graph(
    checker: &TypeChecker,
    callable: CallableSource<'_>,
    span: Span,
    identity: &CanonicalSymbolId,
    view_errors: &mut Vec<crate::diagnostics::CompileError>,
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
    // A direct returned view is checked below; aggregate dependencies and nested modes stay unavailable.
    let view = crate::ownership_metadata::returned_view(contract).map_err(|reason| Unavailable {
        span,
        reason: reason.into(),
    })?;
    if is_async
        || contract.is_some_and(|contract| {
            contract.modes.iter().any(|mode| {
                mode.component.parameter.is_some()
                    && (!mode.component.children.is_empty() || mode.mode == OwnershipMode::Reference)
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
            operations: BTreeMap::new(),
            nodes: vec![Node {
                event: Event::Nothing,
                successors: Vec::new(),
                span,
            }],
            entry: 0,
        },
        constrained: BTreeSet::new(),
        borrowed_inputs: BTreeSet::new(),
        aliases: BTreeMap::new(),
        loops: Vec::new(),
        view: None,
        view_carried: false,
        view_errors: Vec::new(),
    };
    builder.inputs(parameters, identity)?;
    let mut receiver_root = None;
    if let Some((binding, receiver)) = receiver {
        let root = builder.binding_root(&binding.node, binding.span, SemanticSourceTargetKind::Receiver)?;
        let writable = receiver == Receiver::Mutable;
        builder.graph.permissions.insert(
            Root::Binding(root.clone()),
            Permission {
                exclusive: writable,
                writable,
                transferable: false,
            },
        );
        receiver_root = Some(root);
    }
    if let Some(view) = view {
        let root = match view.source.slot {
            SourceSlot::Receiver => receiver_root.ok_or(Unavailable {
                span,
                reason: "returned view names an absent receiver".into(),
            })?,
            SourceSlot::Parameter(slot) => {
                let parameter = parameters.get(slot).ok_or(Unavailable {
                    span,
                    reason: "returned view names an absent parameter".into(),
                })?;
                builder.binding_root(
                    &parameter.node.name,
                    parameter.span,
                    SemanticSourceTargetKind::Parameter,
                )?
            }
        };
        let place = Place {
            root: Root::Binding(root),
            projection: view.source.fields.iter().cloned().map(Projection::Field).collect(),
        };
        builder.view = Some((view.access == incan_semantics_core::IncanInputAccess::Exclusive, place));
        builder.view_carried = view.carried;
    }
    let result = builder.block(body, vec![0]);
    view_errors.append(&mut builder.view_errors);
    result?;
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
                if mode.is_some_and(|mode| matches!(mode.mode, OwnershipMode::Borrowed | OwnershipMode::BorrowedMut)) {
                    self.borrowed_inputs.insert(Root::Binding(root.clone()));
                }
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
            Expr::FString(parts) => self.formatted_text(parts, expression.span, incoming),
            // A returned view, including one `?` takes out of its carrier, is a place, not a value: it may be bound,
            // used as a receiver or returned as a view.
            Expr::Call(..) | Expr::MethodCall(..) | Expr::Try(..) if self.view_relation(expression)?.is_some() => {
                Err(Unavailable {
                    span: expression.span,
                    reason: "a returned view needs a local binding, a receiver or a returned view position".into(),
                })
            }
            // A carried view stays in its carrier until `?` or a match takes it out; the carrier holds a loan.
            Expr::Call(..) | Expr::MethodCall(..)
                if self.call_relation(expression)?.is_some_and(|relation| relation.carried) =>
            {
                Err(Unavailable {
                    span: expression.span,
                    reason: "a carried view needs `?` or a returned carried view position".into(),
                })
            }
            Expr::Call(callee, _, arguments) => self.call(callee, arguments, expression.span, incoming),
            Expr::MethodCall(receiver, method, _, arguments) => {
                match super::builtins::list_method_writes(&self.checker.type_info, receiver, method, expression.span) {
                    Some(writes) => self.list_method(receiver, writes, arguments, expression.span, incoming),
                    None => self.apply_call(Some(receiver), arguments, expression.span, incoming),
                }
            }
            Expr::Paren(inner) | Expr::Unary(_, inner) => self.expression(inner, incoming, consume),
            // `?` consumes its carrier and continues with the success payload; the error path leaves the body as a
            // `return` does, carrying only the carrier's own error value. Only a body without a returned view, or one
            // whose view is itself carried, has that exit.
            Expr::Try(inner) if self.view.is_none() || self.view_carried => self.expression(inner, incoming, true),
            Expr::Binary(left, operation, right) => {
                let incoming = self.expression(left, incoming, false)?;
                let (incoming, left_dependency) = self.retain_operation_operand(left, incoming)?;
                let mut outgoing = self.expression(right, incoming.clone(), false)?;
                let (evaluated, right_dependency) = self.retain_operation_operand(right, outgoing)?;
                outgoing = evaluated;
                if matches!(operation, BinaryOp::And | BinaryOp::Or) {
                    outgoing.extend(incoming);
                }
                self.finish_operation(
                    expression.span,
                    left_dependency.into_iter().chain(right_dependency).collect(),
                    outgoing,
                )
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
        self.checker.input_copy_value(ty)
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

    /// A returned view borrows exactly its declared source place, with the view's access; the borrow checks the
    /// source permission, so a shared input cannot back an exclusive view. Any other value is a program error.
    fn view_return(
        &mut self,
        value: &Spanned<Expr>,
        exclusive: bool,
        source: &Place,
        incoming: Vec<usize>,
    ) -> Result<(), Unavailable> {
        // A call returning a view of exactly the declared source satisfies the relation through its result loan.
        if let Some(relation) = self.view_relation(value)? {
            let (incoming, path) = self.evaluate_view_call(value, relation, incoming)?;
            if path.place == *source {
                let loan = self.call_loan(path, exclusive, exclusive, value.span);
                self.event(incoming, Event::Borrow(loan), value.span);
            } else {
                self.view_errors.push(view_error(
                    "A returned view must return its declared source place; this call returns a view of a \
                     different place",
                    value.span,
                ));
            }
            return Ok(());
        }
        match self.optional_place(value)? {
            Some(path) if path.place == *source => {
                let incoming = self.evaluate_place(value, incoming)?;
                let loan = self.call_loan(path, exclusive, exclusive, value.span);
                self.event(incoming, Event::Borrow(loan), value.span);
            }
            Some(_) => {
                self.view_errors.push(view_error(
                    "A returned view must return its declared source place; this returns a different place",
                    value.span,
                ));
                self.expression(value, incoming, true)?;
            }
            None => {
                self.view_errors.push(view_error(
                    "A returned view cannot borrow a value created in this body; return its declared source place",
                    value.span,
                ));
                self.expression(value, incoming, true)?;
            }
        }
        Ok(())
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
        // A new local initialized by a returned view aliases its substituted place under the call's result loan.
        if mode.is_none()
            && root.declaration_span == assignment.name_span
            && root.kind == SemanticSourceTargetKind::Local
            && let Some(relation) = self.view_relation(&assignment.value)?
        {
            let (incoming, path) = self.evaluate_view_call(&assignment.value, relation, incoming)?;
            let loan = path.through.ok_or(Unavailable {
                span: assignment.value.span,
                reason: "returned view has no result loan".into(),
            })?;
            if self.graph.local_loans.insert(root.clone(), loan).is_some() {
                return Err(Unavailable {
                    span: assignment.name_span,
                    reason: "local declaration has more than one ownership destination".into(),
                });
            }
            self.aliases.insert(root, path);
            return Ok(incoming);
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

    /// Iterate an evaluated iterable. A stored iterable stays shared-borrowed while the loop runs: each poll at the
    /// header reads through that loan, so the back edge keeps it live across the body and an edit of the iterable
    /// there conflicts. A loop that takes the list's items consumes it instead. The item is a fresh local declared at
    /// each poll; it is an owned value, never a view of the iterable.
    fn for_loop(&mut self, loop_: &ForStmt, span: Span, incoming: Vec<usize>) -> Result<Vec<usize>, Unavailable> {
        let crate::ast::Pattern::Binding(name) = &loop_.pattern.node else {
            return Err(Unavailable {
                span: loop_.pattern.span,
                reason: "loop pattern needs checked binding access".into(),
            });
        };
        if self
            .checker
            .type_info
            .protocols
            .iterations
            .contains_key(&loop_.iter.span)
            || self.checker.type_info.protocols.iterations.contains_key(&span)
        {
            return Err(Unavailable {
                span: loop_.iter.span,
                reason: "protocol iteration needs checked callable access facts".into(),
            });
        }
        let item = self
            .checker
            .type_info
            .resolved_write_identity(loop_.pattern.span, name)
            .cloned()
            .ok_or(Unavailable {
                span: loop_.pattern.span,
                reason: "missing checked loop item".into(),
            })?;
        let takes_items = self.checker.type_info.for_loop_takes_items(loop_.iter.span);
        let iterable = if takes_items {
            None
        } else {
            self.optional_place(&loop_.iter)?
        };
        let mut evaluated = self.expression(&loop_.iter, incoming, takes_items)?;
        let iteration = match iterable {
            Some(path) => {
                let place = path.place.clone();
                let loan = self.call_loan(path, false, false, loop_.iter.span);
                evaluated = self.event(evaluated, Event::Borrow(loan), loop_.iter.span);
                Some((loan, place))
            }
            None => None,
        };
        let header = self.event(evaluated, Event::Nothing, span)[0];
        // The poll reads the iterable through its loan; exhaustion leaves the loop from here.
        let poll = match iteration {
            Some((loan, place)) => self.event(
                vec![header],
                Event::Access {
                    place,
                    access: Access::Read,
                    through: Some(loan),
                },
                loop_.iter.span,
            ),
            None => vec![header],
        };
        // Each poll declares the item afresh, as an initializer does: its root is registered, with no write event.
        self.root(&item, loop_.pattern.span)?;
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
    fn statement(&mut self, statement: &Spanned<Statement>, incoming: Vec<usize>) -> Result<Vec<usize>, Unavailable> {
        match &statement.node {
            Statement::Pass => Ok(incoming),
            Statement::Expr(Spanned {
                node: Expr::Match(scrutinee, arms),
                ..
            }) => self.match_statement(scrutinee, arms, incoming),
            // A discarded returned view still borrows its source for the call; its result loan is never used.
            Statement::Expr(value) => match self.view_relation(value)? {
                Some(relation) => Ok(self.evaluate_view_call(value, relation, incoming)?.0),
                None => self.expression(value, incoming, false),
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
                match (value, self.view.clone()) {
                    (Some(value), Some((exclusive, source))) if self.view_carried => {
                        self.carried_return(value, exclusive, &source, incoming)?
                    }
                    (Some(value), Some((exclusive, source))) => {
                        self.view_return(value, exclusive, &source, incoming)?
                    }
                    (None, Some(_)) => self.view_errors.push(view_error(
                        "A returned view must return its declared source place",
                        statement.span,
                    )),
                    (Some(value), None) => {
                        self.expression(value, incoming, true)?;
                    }
                    (None, None) => {}
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

/// A returned-view relationship error, reported at the offending return.
fn view_error(message: &str, span: Span) -> crate::diagnostics::CompileError {
    crate::diagnostics::CompileError::type_error(message.to_owned(), span).with_stable_code("INCAN-T0124")
}
