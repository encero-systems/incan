//! Loop-item transfers and iterable invalidation projected through the canonical ownership graph (#1844).
//!
//! Ordinary checking retains only candidates whose item type cannot be copied or cloned. The graph then decides from
//! actual `Move` events whether the body hands one of those items on. A taking loop moves its iterable at the loop
//! header; the graph's existing transfer fixed point consequently owns reads, refills, captures and repeated paths.

use incan_lang::lang::builtins::{self, BuiltinFnId};
use incan_lang::lang::keywords::KeywordId;
use incan_lang::lang::surface::types::{self as surface_types, SurfaceTypeId};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SurfaceFeatureKey};

use super::TypeChecker;
use super::flow::{Access, Conflict, ConflictKind, Event, Graph, LoanId, Place, Root};
use crate::ast::{Expr, ForStmt, Pattern, Span, Spanned, SurfaceExprPayload};
use crate::diagnostics::{CompileError, errors};
use crate::symbols::{ResolvedType, SymbolKind, TypeInfo};
use crate::typechecker::helpers::collection_type_id;
use crate::typechecker::{TypeCheckInfo, check_stmt::TupleShape, check_stmt::classify_tuple_shape};

/// One checked loop whose body may transfer a non-copyable item.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct Candidate {
    pub iter_span: Span,
    pub loop_span: Span,
    pub body_end: usize,
    pub item_type: String,
    pub items: Vec<CanonicalSymbolId>,
    pub source: CandidateSource,
}

/// Where a candidate loop gets the values its pattern binds.
#[derive(Debug, Clone)]
pub(in crate::typechecker) enum CandidateSource {
    /// A fresh value or iterator already yields owned items.
    OwnValues,
    /// A direct list binding can be transferred if the body moves an item.
    List {
        name: String,
        place: Place,
        definition: Span,
        outer_loop: Option<Span>,
    },
    /// This source cannot surrender either its items or independent copies.
    Untakeable(Untakeable),
}

/// Owned diagnostic payload for an iterable that cannot be taken.
#[derive(Debug, Clone)]
pub(in crate::typechecker) enum Untakeable {
    MutParameter(String),
    InPlaceLoopVariable(String),
    Binding(String),
    Element,
    Field,
    CollectionMethod(String),
    Adapter(String),
    Other,
}

impl Untakeable {
    /// Borrow this retained payload in the diagnostics catalog's established shape.
    fn diagnostic(&self) -> errors::UntakeableIterable<'_> {
        match self {
            Self::MutParameter(name) => errors::UntakeableIterable::MutParameter(name),
            Self::InPlaceLoopVariable(name) => errors::UntakeableIterable::InPlaceLoopVariable(name),
            Self::Binding(name) => errors::UntakeableIterable::Binding(name),
            Self::Element => errors::UntakeableIterable::Element,
            Self::Field => errors::UntakeableIterable::Field,
            Self::CollectionMethod(method) => errors::UntakeableIterable::CollectionMethod(method),
            Self::Adapter(name) => errors::UntakeableIterable::Adapter(name),
            Self::Other => errors::UntakeableIterable::Other,
        }
    }
}

/// Candidate metadata retained by the graph builder, including its item roots and optional direct-list transfer.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct GraphLoop {
    pub candidate: Candidate,
    pub transfer: Option<TransferNodes>,
}

/// Nodes whose dormant transfer replaces a direct list iteration when its item is moved.
#[derive(Debug, Clone, Copy)]
pub(in crate::typechecker) struct TransferNodes {
    pub loan: LoanId,
    pub transfer: usize,
    pub poll: usize,
}

/// Stable T0119 payload associated with the graph node that moves a list into its iterator.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct TakingMove {
    pub list: String,
    pub item_type: String,
    pub loop_span: Span,
    pub body_end: usize,
    pub definition: Span,
}

impl TypeChecker {
    /// Retain a possible loop-item transfer after the pattern bindings have their canonical identities.
    ///
    /// No body walk occurs here. The candidate activates the one ownership graph; actual item `Move` events decide
    /// whether lowering consumes the iterable and whether T0119 applies.
    pub(in crate::typechecker) fn prepare_loop_item_candidate(
        &mut self,
        loop_: &ForStmt,
        item_type: &ResolvedType,
        loop_span: Span,
    ) -> Vec<CanonicalSymbolId> {
        let mut bindings = Vec::new();
        collect_pattern_bindings(&loop_.pattern, item_type, &mut bindings);
        let items = bindings
            .into_iter()
            .filter(|(_, ty)| self.value_is_not_cloneable(ty))
            .filter_map(|(pattern, _)| match &pattern.node {
                Pattern::Binding(name) => self.type_info.resolved_write_identity(pattern.span, name).cloned(),
                _ => None,
            })
            .collect::<Vec<_>>();
        if items.is_empty() {
            return Vec::new();
        }
        let source = self.loop_item_source(&loop_.iter);
        let item_type = item_type_display(item_type);
        let body_end = loop_
            .body
            .last()
            .map_or(loop_.iter.span.end, |statement| statement.span.end);
        let candidate = Candidate {
            iter_span: loop_.iter.span,
            loop_span,
            body_end,
            item_type,
            items: items.clone(),
            source,
        };
        self.loop_item_candidates.insert(loop_.iter.span, candidate);
        if let Some(owner) = self.current_body_identity().cloned() {
            self.loop_item_bodies.insert(owner);
        }
        for item in &items {
            self.active_loop_items.insert(item.clone(), loop_.iter.span);
        }
        items
    }

    /// Leave the lexical loop-item scope after its body has been checked.
    pub(in crate::typechecker) fn leave_loop_item_candidate(&mut self, items: Vec<CanonicalSymbolId>) {
        for item in items {
            self.active_loop_items.remove(&item);
        }
    }

    /// Whether a value can be neither copied nor cloned according to the compiler-owned type registry.
    fn value_is_not_cloneable(&self, ty: &ResolvedType) -> bool {
        if let Some(target) = self.rusttype_native_target(ty) {
            return self.value_is_not_cloneable(&target);
        }
        match ty {
            ResolvedType::RustPath(path) => {
                surface_types::from_runtime_rust_path(path).is_some_and(surface_types::is_not_cloneable)
            }
            ResolvedType::Tuple(items) => items.iter().any(|item| self.value_is_not_cloneable(item)),
            ResolvedType::Generic(name, _)
                if surface_types::from_runtime_rust_path(name).is_some_and(surface_types::is_not_cloneable) =>
            {
                true
            }
            ResolvedType::Generic(name, arguments)
                if collection_type_id(name).is_some()
                    || matches!(
                        self.surface_type_in_scope(name),
                        Some(SurfaceTypeId::Vec | SurfaceTypeId::HashMap)
                    ) =>
            {
                arguments.iter().any(|argument| self.value_is_not_cloneable(argument))
            }
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => self
                .surface_type_in_scope(name)
                .is_some_and(surface_types::is_not_cloneable),
            _ => false,
        }
    }

    /// Resolve a compiler-known surface type only when source has not rebound its spelling.
    fn surface_type_in_scope(&self, name: &str) -> Option<SurfaceTypeId> {
        match self.lookup_symbol(name).map(|symbol| &symbol.kind) {
            None | Some(SymbolKind::Type(TypeInfo::Builtin)) => surface_types::from_str(name),
            Some(_) => None,
        }
    }

    /// Classify a candidate iterable from checked identities and registry keys, never from a later diagnostic guess.
    fn loop_item_source(&self, iter: &Spanned<Expr>) -> CandidateSource {
        let iterates_own_values = self.type_info.protocol_iteration(iter.span).is_some()
            || self.type_info.expr_type(iter.span).is_some_and(|ty| {
                matches!(ty, ResolvedType::Generic(name, _)
                    if collection_type_id(name) == Some(CollectionTypeId::Generator))
            });
        if iterates_own_values {
            return CandidateSource::OwnValues;
        }
        let iter = unparenthesized(iter);
        match &iter.node {
            Expr::Ident(name) => self.named_loop_item_source(name, iter.span),
            Expr::List(_) | Expr::ListComp(_) => CandidateSource::OwnValues,
            Expr::Try(inner) => self.loop_item_source(inner),
            Expr::Surface(surface)
                if matches!(
                    (&surface.key, &surface.payload),
                    (
                        SurfaceFeatureKey::SoftKeyword(KeywordId::Await),
                        SurfaceExprPayload::PrefixUnary(_)
                    )
                ) =>
            {
                CandidateSource::OwnValues
            }
            Expr::Call(callee, _, _) => match &callee.node {
                Expr::Ident(name)
                    if matches!(
                        builtins::from_str(name),
                        Some(BuiltinFnId::Enumerate | BuiltinFnId::Zip)
                    ) && !self.has_non_builtin_call_root_binding(name) =>
                {
                    CandidateSource::Untakeable(Untakeable::Adapter(name.clone()))
                }
                _ => CandidateSource::OwnValues,
            },
            Expr::MethodCall(receiver, method, _, _) => {
                let builtin_collection = self.type_info.expr_type(receiver.span).is_some_and(|ty| match ty {
                    ResolvedType::Generic(name, _) => collection_type_id(name).is_some(),
                    ResolvedType::FrozenList(_) | ResolvedType::FrozenSet(_) | ResolvedType::FrozenDict(_, _) => true,
                    _ => false,
                });
                if builtin_collection {
                    CandidateSource::Untakeable(Untakeable::CollectionMethod(method.clone()))
                } else {
                    CandidateSource::OwnValues
                }
            }
            Expr::Index(_, _) => CandidateSource::Untakeable(Untakeable::Element),
            Expr::Field(_, _) => CandidateSource::Untakeable(Untakeable::Field),
            _ => CandidateSource::Untakeable(Untakeable::Other),
        }
    }

    /// Classify a direct list binding, retaining a nested loop-item parent rather than an in-place-name set.
    fn named_loop_item_source(&self, name: &str, span: Span) -> CandidateSource {
        let Some(identity) = self.type_info.resolved_identity(span).cloned() else {
            return CandidateSource::Untakeable(Untakeable::Binding(name.into()));
        };
        let place = Place {
            root: Root::Binding(identity.clone()),
            projection: Vec::new(),
        };
        let outer_loop = self.active_loop_items.get(&identity).copied();
        if outer_loop.is_some() {
            return CandidateSource::List {
                name: name.into(),
                place,
                definition: identity.declaration_span,
                outer_loop,
            };
        }
        let eligible = match identity.kind {
            SemanticSourceTargetKind::Local => true,
            SemanticSourceTargetKind::Parameter => !self
                .type_info
                .declarations
                .mut_param_marker(identity.declaration_span, &identity.declaration_name)
                .unwrap_or(false),
            _ => false,
        };
        if eligible {
            return CandidateSource::List {
                name: name.into(),
                place,
                definition: identity.declaration_span,
                outer_loop: None,
            };
        }
        let reason = if identity.kind == SemanticSourceTargetKind::Parameter {
            Untakeable::MutParameter(name.into())
        } else {
            Untakeable::Binding(name.into())
        };
        CandidateSource::Untakeable(reason)
    }
}

/// Settle loop candidates from actual item moves, then materialize direct-list transfers before graph analysis.
pub(super) fn settle(graph: &mut Graph, facts: &mut TypeCheckInfo) -> Vec<CompileError> {
    let mut errors = Vec::new();
    let mut loops = graph.item_taking_loops.values().cloned().collect::<Vec<_>>();
    loops.sort_by_key(|loop_| std::cmp::Reverse(loop_.candidate.loop_span.start));
    for loop_ in loops {
        let moved = graph.nodes.iter().any(|node| match &node.event {
            Event::Access {
                place:
                    Place {
                        root: Root::Binding(root),
                        projection,
                    },
                access: Access::Move,
                ..
            } => projection.is_empty() && loop_.candidate.items.contains(root),
            _ => false,
        });
        if !moved {
            continue;
        }
        match &loop_.candidate.source {
            CandidateSource::OwnValues => {}
            CandidateSource::Untakeable(source) => errors.push(errors::loop_cannot_take_items(
                source.diagnostic(),
                &loop_.candidate.item_type,
                loop_.candidate.iter_span,
            )),
            CandidateSource::List { name, definition, .. } => {
                let Some(transfer) = loop_.transfer else {
                    continue;
                };
                let Some(loan) = graph.loans.get(transfer.loan.0) else {
                    continue;
                };
                graph.nodes[transfer.transfer].event = Event::Access {
                    place: loan.source.clone(),
                    access: Access::Move,
                    through: None,
                };
                graph.nodes[transfer.poll].event = Event::Nothing;
                graph.direct_iteration_loans.remove(&transfer.loan);
                graph.taking_moves.insert(
                    transfer.transfer,
                    TakingMove {
                        list: name.clone(),
                        item_type: loop_.candidate.item_type.clone(),
                        loop_span: loop_.candidate.iter_span,
                        body_end: loop_.candidate.body_end,
                        definition: *definition,
                    },
                );
                facts.record_for_loop_takes_items(loop_.candidate.iter_span);
            }
        }
    }
    errors
}

/// Project the historical T0119 diagnostic from a transfer conflict owned by a taking-loop move.
pub(super) fn conflict(graph: &Graph, conflict: &Conflict) -> Option<CompileError> {
    match conflict.kind {
        ConflictKind::Moved(origin) => {
            let taking = graph.taking_moves.get(&origin)?;
            let operation = graph.nodes.get(conflict.node)?;
            let (reuse, related) = if conflict.node == origin {
                (errors::TakenListReuse::RepeatedLoop, taking.definition)
            } else if operation.span.start < taking.loop_span.start {
                (errors::TakenListReuse::ReadBeforeRefill, taking.loop_span)
            } else if operation.span.end <= taking.body_end {
                (errors::TakenListReuse::InsideLoop, taking.loop_span)
            } else {
                (errors::TakenListReuse::AfterLoop, taking.loop_span)
            };
            Some(errors::taken_list_used_again(
                &taking.list,
                &taking.item_type,
                reuse,
                operation.span,
                related,
            ))
        }
        ConflictKind::ActiveLoan(loan) => {
            let taking = graph.taking_moves.get(&conflict.node)?;
            let captured = graph
                .closures
                .values()
                .any(|closure| closure.loans.iter().any(|(_, candidate)| *candidate == loan));
            captured.then(|| {
                errors::taken_list_used_again(
                    &taking.list,
                    &taking.item_type,
                    errors::TakenListReuse::CapturedBeforeLoop,
                    graph.loans[loan.0].span,
                    taking.loop_span,
                )
            })
        }
        ConflictKind::Permission | ConflictKind::UnavailableLoan(_) => None,
    }
}

/// Return whether this conflict arises from the list transfer introduced by a taking loop.
pub(super) fn owns_conflict(graph: &Graph, conflict: &Conflict) -> bool {
    graph.taking_moves.contains_key(&conflict.node)
        || matches!(conflict.kind, ConflictKind::Moved(origin) if graph.taking_moves.contains_key(&origin))
}

/// Collect binding patterns with the corresponding checked element type.
fn collect_pattern_bindings<'a>(
    pattern: &'a Spanned<Pattern>,
    item_type: &ResolvedType,
    bindings: &mut Vec<(&'a Spanned<Pattern>, ResolvedType)>,
) {
    match &pattern.node {
        Pattern::Binding(_) => bindings.push((pattern, item_type.clone())),
        Pattern::Tuple(items) => {
            let element_types = match classify_tuple_shape(item_type) {
                TupleShape::Tuple(types) if types.len() == items.len() => types,
                _ => vec![ResolvedType::Unknown; items.len()],
            };
            for (item, element_type) in items.iter().zip(&element_types) {
                collect_pattern_bindings(item, element_type, bindings);
            }
        }
        Pattern::Group(inner) => collect_pattern_bindings(inner, item_type, bindings),
        _ => {}
    }
}

/// Return a checked expression without source parentheses, retaining the innermost expression's identity span.
fn unparenthesized(expression: &Spanned<Expr>) -> &Spanned<Expr> {
    match &expression.node {
        Expr::Paren(inner) => unparenthesized(inner),
        _ => expression,
    }
}

/// Render a non-copyable item type with the source-facing compiler registry spelling.
fn item_type_display(ty: &ResolvedType) -> String {
    match ty {
        ResolvedType::RustPath(path) => surface_types::from_runtime_rust_path(path)
            .map_or_else(|| ty.to_string(), |id| surface_types::as_str(id).to_string()),
        ResolvedType::Generic(name, arguments)
            if arguments
                .iter()
                .any(|argument| matches!(argument, ResolvedType::Unknown)) =>
        {
            name.clone()
        }
        ResolvedType::Generic(name, arguments) => {
            let arguments = arguments.iter().map(item_type_display).collect::<Vec<_>>();
            format!("{name}[{}]", arguments.join(", "))
        }
        ResolvedType::Tuple(items) => {
            let items = items.iter().map(item_type_display).collect::<Vec<_>>();
            format!("({})", items.join(", "))
        }
        other => other.to_string(),
    }
}
