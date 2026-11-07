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
    FunctionDecl, IfStmt, MethodDecl, Param, Receiver, Span, Spanned, Statement, Type,
};
use crate::ownership_metadata::SourceSlot;
use crate::symbols::{ResolvedType, SymbolKind};
use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};

mod builder_places;
mod builder_statements;
mod calls;
mod closures;
mod comprehensions;
mod dictionaries;
mod matches;
mod operations;
mod retained_results;
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

    /// The declared result type as written.
    pub(in crate::typechecker) fn return_type(self) -> &'a Spanned<Type> {
        match self {
            Self::Function(function) => &function.return_type,
            Self::Method(method) => &method.return_type,
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
    /// Whether an ordinary iteration-only profile may evaluate fresh aggregate list elements. Explicit ownership
    /// profiles keep their established refusal until retained aggregate storage has a complete transfer proof.
    allow_fresh_aggregate_elements: bool,
    graph: Graph,
    locals: BTreeMap<CanonicalSymbolId, Permission>,
    aliases: BTreeMap<CanonicalSymbolId, Path>,
    /// Locals bound to a call that carries a view, not yet consumed. Each aliases the view's place under the call's
    /// result loan, which it holds until a `match` or `?` takes the view out; any other use of it is refused.
    carriers: BTreeMap<CanonicalSymbolId, matches::BoundCarrier>,
    /// Locals a `match` or `?` already took a carried view out of; any later use of them is refused.
    taken_carriers: BTreeSet<CanonicalSymbolId>,
    /// Locals bound to a closure, with every place and loan the closure holds. A closure local can only be called or
    /// lent to a parameter whose body only calls it.
    closures: BTreeMap<CanonicalSymbolId, Vec<Path>>,
    /// Calls an `await` suspends on directly. A call to an async callable anywhere else would leave a future holding
    /// its loans, which is not modeled.
    awaited: BTreeSet<Span>,
    constrained: BTreeSet<Root>,
    borrowed_inputs: BTreeSet<Root>,
    loops: Vec<LoopEdges>,
    /// The declared source dependency of a top-level owned result. The result remains value storage rather than a
    /// source alias, but it must carry a checked loan of this place.
    owned_source: Option<Place>,
    /// The declared returned view: whether it is exclusive, and the exact source place every return borrows.
    view: Option<(bool, Place)>,
    /// The `Result` or `Option` payload or the tuple element carrying the returned view, or `None` for a view returned
    /// whole.
    view_carried: Option<usize>,
    /// Whether the declared result is a tuple, so `view_carried` names one of its elements.
    view_in_tuple: bool,
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
    build_reporting_views(checker, callable, span, identity, false).0
}

/// Build the body's graph and also return the diagnostics for returns that do not borrow the declared view.
pub(super) fn build_reporting_views(
    checker: &TypeChecker,
    callable: CallableSource<'_>,
    span: Span,
    identity: &CanonicalSymbolId,
    allow_fresh_aggregate_elements: bool,
) -> (Result<Graph, Unavailable>, Vec<crate::diagnostics::CompileError>) {
    let mut view_errors = Vec::new();
    let graph = build_graph(
        checker,
        callable,
        span,
        identity,
        allow_fresh_aggregate_elements,
        &mut view_errors,
    );
    (graph, view_errors)
}

/// Project one callable into its ownership graph while retaining returned-view diagnostics separately from structural
/// unavailability. Async callables use the same source relation: suspension changes the physical carrier, not which
/// declared input place the result views.
fn build_graph(
    checker: &TypeChecker,
    callable: CallableSource<'_>,
    span: Span,
    identity: &CanonicalSymbolId,
    allow_fresh_aggregate_elements: bool,
    view_errors: &mut Vec<crate::diagnostics::CompileError>,
) -> Result<Graph, Unavailable> {
    let (parameters, body, receiver, name, kind) = match callable {
        CallableSource::Function(function) => (
            &function.params,
            &function.body,
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
    let owned_source = crate::ownership_metadata::owned_result_source(contract)
        .map_err(|reason| Unavailable {
            span,
            reason: reason.into(),
        })?
        .map(|(_, source)| source.clone());
    if contract.is_some_and(|contract| {
        contract.modes.iter().any(|mode| {
            mode.component.parameter.is_some()
                && (!mode.component.children.is_empty() || mode.mode == OwnershipMode::Reference)
        })
    }) {
        return Err(Unavailable {
            span,
            reason: "escaping or nested ownership contract".into(),
        });
    }
    let mut builder = Builder {
        checker,
        owner: identity.clone(),
        allow_fresh_aggregate_elements,
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
            direct_iteration_loans: BTreeSet::new(),
            item_taking_loops: BTreeMap::new(),
            taking_moves: BTreeMap::new(),
            dictionary_lookups: BTreeMap::new(),
            local_loans: BTreeMap::new(),
            dependent_locals: BTreeMap::new(),
            dependent_values: BTreeSet::new(),
            owned_locals: BTreeMap::new(),
            invocations: BTreeMap::new(),
            operations: BTreeMap::new(),
            closures: BTreeMap::new(),
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
        carriers: BTreeMap::new(),
        taken_carriers: BTreeSet::new(),
        closures: BTreeMap::new(),
        awaited: BTreeSet::new(),
        loops: Vec::new(),
        owned_source: None,
        view: None,
        view_carried: None,
        view_in_tuple: false,
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
            SourceSlot::Receiver => receiver_root.clone().ok_or(Unavailable {
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
        builder.view_in_tuple = super::super::written_tuple_elements(&callable.return_type().node).is_some();
    }
    if let Some(source) = owned_source {
        let root = match source.slot {
            SourceSlot::Receiver => receiver_root.clone().ok_or(Unavailable {
                span,
                reason: "owned result dependency names an absent receiver".into(),
            })?,
            SourceSlot::Parameter(slot) => {
                let parameter = parameters.get(slot).ok_or(Unavailable {
                    span,
                    reason: "owned result dependency names an absent parameter".into(),
                })?;
                builder.binding_root(
                    &parameter.node.name,
                    parameter.span,
                    SemanticSourceTargetKind::Parameter,
                )?
            }
        };
        builder.owned_source = Some(Place {
            root: Root::Binding(root),
            projection: source.fields.iter().cloned().map(Projection::Field).collect(),
        });
    }
    let result = builder.block(body, vec![0]);
    view_errors.append(&mut builder.view_errors);
    result?;
    Ok(builder.graph)
}

impl Builder<'_> {
    /// Evaluate an expression in source order. A checked Copy read is not a transfer; ordinary unconstrained values
    /// retain their existing semantics until the separate ordinary-binding migration ships.
    fn expression(
        &mut self,
        expression: &Spanned<Expr>,
        incoming: Vec<usize>,
        consume: bool,
    ) -> Result<Vec<usize>, Unavailable> {
        if let Some(result) = self.retained_result(expression, incoming.clone(), false)? {
            return Ok(result.incoming);
        }
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
        if matches!(expression.node, Expr::Field(_, _) | Expr::Index(_, _))
            && let Some((incoming, path)) = self.projected_view_place(expression, incoming.clone())?
        {
            let ty = self.checker.type_info.expr_type(expression.span).ok_or(Unavailable {
                span: expression.span,
                reason: "missing checked projected-view type".into(),
            })?;
            let access = if consume && !self.copy_value(ty) {
                Access::Move
            } else {
                Access::Read
            };
            return Ok(self.event(
                incoming,
                Event::Access {
                    place: path.place,
                    access,
                    through: path.through,
                },
                expression.span,
            ));
        }
        match &expression.node {
            Expr::Ident(_) | Expr::SelfExpr | Expr::Field(_, _) | Expr::Index(_, _) => {
                let place = self.place(expression)?;
                let mut incoming = self.evaluate_place(expression, incoming)?;
                if let Root::Binding(root) = &place.place.root
                    && let Some(loans) = self.graph.dependent_locals.get(root).cloned()
                {
                    for id in loans {
                        let loan = self.graph.loans.get(id.0).ok_or(Unavailable {
                            span: expression.span,
                            reason: "dependent value has no retained source loan".into(),
                        })?;
                        incoming = self.event(
                            incoming,
                            Event::Access {
                                place: loan.source.clone(),
                                access: Access::Read,
                                through: Some(id),
                            },
                            expression.span,
                        );
                    }
                }
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
            Expr::List(entries)
                if !consume
                    || !entries.iter().any(|entry| match entry {
                        crate::ast::ListEntry::Element(value) | crate::ast::ListEntry::Spread(value) => {
                            self.is_loop_item_value(value)
                        }
                    }) =>
            {
                self.scalar_list(entries, incoming)
            }
            Expr::FString(parts) => self.formatted_text(parts, expression.span, incoming),
            // A returned view, including one `?` takes out of its carrier, is a place, not a value: it may be bound,
            // used as a receiver or returned as a view.
            Expr::Call(..) | Expr::MethodCall(..) | Expr::Try(..) if self.view_source(expression)?.is_some() => {
                Err(Unavailable {
                    span: expression.span,
                    reason: "a returned view needs a local binding, a receiver or a returned view position".into(),
                })
            }
            // A carried view stays in its carrier until `?` or a match takes it out; the carrier holds a loan.
            Expr::Call(..) | Expr::MethodCall(..)
                if self
                    .call_relation(expression)?
                    .is_some_and(|relation| relation.carried.is_some()) =>
            {
                Err(Unavailable {
                    span: expression.span,
                    reason: "a carried view needs `?`, a match, unpacking or a returned carried view position".into(),
                })
            }
            Expr::Call(callee, _, arguments) => self.call(callee, arguments, expression.span, incoming),
            Expr::MethodCall(receiver, _, _, arguments) => {
                match super::builtins::list_method_writes(&self.checker.type_info, expression.span) {
                    Some(writes) => self.list_method(receiver, writes, arguments, expression.span, incoming),
                    None if self.uses_ordinary_iteration_access(expression.span) => {
                        self.ordinary_iteration_call(Some(receiver), arguments, incoming)
                    }
                    None => self.apply_call(Some(receiver), arguments, expression.span, incoming),
                }
            }
            Expr::Paren(inner) | Expr::Unary(_, inner) => self.expression(inner, incoming, consume),
            // `?` consumes its carrier and continues with the success payload; the error path leaves the body as a
            // `return` does, carrying only the carrier's own error value. Only a body without a returned view, or one
            // whose view is carried in the success payload, has that exit; an error that carries the view must be the
            // declared source's.
            Expr::Try(inner) if self.view.is_none() || self.view_carried == Some(0) => {
                self.expression(inner, incoming, true)
            }
            Expr::Try(_) if self.view_carried == Some(1) => Err(error_view_try(expression.span)),
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
            Expr::ListComp(comprehension) => self.list_comprehension(comprehension, expression.span, incoming),
            Expr::DictComp(comprehension) => self.dict_comprehension(comprehension, expression.span, incoming),
            // `await` on a call suspends this body until the call's future completes, and the call's loans end there as
            // a synchronous call's do. Awaiting a stored future needs task facts of its own.
            Expr::Surface(surface) if super::builtins::awaited(surface).is_some() => {
                let awaited = super::builtins::awaited(surface).ok_or(Unavailable {
                    span: expression.span,
                    reason: "await has no awaited expression".into(),
                })?;
                let call = super::builtins::view_call(awaited);
                if matches!(call.node, Expr::Call(..) | Expr::MethodCall(..)) {
                    self.awaited.insert(call.span);
                } else if !self.is_loop_item_value(awaited) {
                    return Err(Unavailable {
                        span: awaited.span,
                        reason: "await on a stored future needs checked task facts".into(),
                    });
                }
                self.expression(awaited, incoming, true)
            }
            Expr::Yield(Some(value)) if self.is_loop_item_value(value) => self.expression(value, incoming, true),
            Expr::Yield(None) => Ok(incoming),
            Expr::Tuple(items) | Expr::Set(items) if items.iter().any(|item| self.is_loop_item_value(item)) => {
                let mut outgoing = incoming;
                for item in items {
                    outgoing = self.expression(item, outgoing, true)?;
                }
                Ok(outgoing)
            }
            Expr::List(entries) => {
                let mut outgoing = incoming;
                for entry in entries {
                    let value = match entry {
                        crate::ast::ListEntry::Element(value) | crate::ast::ListEntry::Spread(value) => value,
                    };
                    outgoing = self.expression(value, outgoing, true)?;
                }
                Ok(outgoing)
            }
            Expr::Dict(entries)
                if entries.iter().any(|entry| match entry {
                    crate::ast::DictEntry::Pair(_, value) | crate::ast::DictEntry::Spread(value) => {
                        self.is_loop_item_value(value)
                    }
                }) =>
            {
                let mut outgoing = incoming;
                for entry in entries {
                    match entry {
                        crate::ast::DictEntry::Pair(key, value) => {
                            outgoing = self.expression(key, outgoing, false)?;
                            outgoing = self.expression(value, outgoing, true)?;
                        }
                        crate::ast::DictEntry::Spread(value) => {
                            outgoing = self.expression(value, outgoing, true)?;
                        }
                    }
                }
                Ok(outgoing)
            }
            Expr::Closure(..) => Err(Unavailable {
                span: expression.span,
                reason: "a closure needs a local binding before it is called".into(),
            }),
            _ => Err(Unavailable {
                span: expression.span,
                reason: "expression needs checked call or dependency access facts".into(),
            }),
        }
    }

    /// Whether this expression names one of the non-copyable loop-item roots retained for graph settlement.
    fn is_loop_item_value(&self, expression: &Spanned<Expr>) -> bool {
        match &expression.node {
            Expr::Paren(inner) => self.is_loop_item_value(inner),
            Expr::Ident(_) => self
                .checker
                .type_info
                .resolved_identity(expression.span)
                .is_some_and(|identity| {
                    self.checker
                        .loop_item_candidates
                        .values()
                        .any(|candidate| candidate.items.contains(identity))
                }),
            _ => false,
        }
    }
}

/// A local whose carried view a `match` or `?` already took out; it holds nothing more to use.
fn taken_carrier(span: Span) -> Unavailable {
    Unavailable {
        span,
        reason: "the view was already taken out of this carried result".into(),
    }
}

/// `?` in a body whose error carries its view: the exit would return the carrier's own error, not the source place.
fn error_view_try(span: Span) -> Unavailable {
    Unavailable {
        span,
        reason: "`?` in a body whose error carries a view would return an error other than its source place".into(),
    }
}

/// A returned-view relationship error, reported at the offending return.
fn view_error(message: &str, span: Span) -> crate::diagnostics::CompileError {
    crate::diagnostics::CompileError::type_error(message.to_owned(), span).with_stable_code("INCAN-T0124")
}
