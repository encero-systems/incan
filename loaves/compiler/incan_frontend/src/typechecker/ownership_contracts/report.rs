//! Source diagnostic projection of validated ownership conflicts. Related locations follow graph reachability,
//! including reborrow ancestry, rather than selecting a later textual occurrence of the same name.

use std::collections::{BTreeSet, VecDeque};

use incan_lang::lang::types::collections::{self, CollectionTypeId};

use super::flow::{Access, Analysis, ConflictKind, Event, Graph, LoanId, Place, Root};
use crate::ast::Span;
use crate::diagnostics::CompileError;
use crate::typechecker::TypeCheckInfo;

/// Render conflicts only from a successfully validated graph. The result retains structured locations for CLI,
/// editor and serialized diagnostic projections; native storage selection is not involved.
pub(super) fn conflicts(graph: &Graph, analysis: &Analysis, facts: &TypeCheckInfo) -> Vec<CompileError> {
    let mut projected: Vec<ProjectedConflict> = Vec::new();
    for conflict in &analysis.conflicts {
        let operation = &graph.nodes[conflict.node];
        if let Some(loan) = builtin_conflict_loan(graph, analysis, facts, conflict) {
            let error = builtin_result_conflict(graph, analysis, facts, conflict.node, loan);
            let priority = builtin_conflict_priority(&operation.event);
            let key = builtin_conflict_key(graph, analysis, facts, conflict.node, loan);
            if projected.iter().any(|current| {
                current.priority > priority
                    && current.error.span.start < error.span.start
                    && current.builtin.as_ref() == Some(&key)
            }) {
                continue;
            }
            if let Some(current) = projected
                .iter_mut()
                .find(|current| current.error.span == error.span && current.builtin.as_ref() == Some(&key))
            {
                if priority > current.priority {
                    current.error = error;
                    current.priority = priority;
                }
            } else {
                projected.push(ProjectedConflict {
                    error,
                    priority,
                    builtin: Some(key),
                });
            }
            continue;
        }
        let message = match conflict.kind {
            ConflictKind::Permission => "This operation requires access not granted by its ownership contract",
            ConflictKind::ActiveLoan(_) => "This operation conflicts with a live borrow of the same storage",
            ConflictKind::Moved(_) => "This storage may already have been moved on a path reaching this operation",
            ConflictKind::UnavailableLoan(_) => "This borrow has not been established on every path reaching its use",
        };
        let error = CompileError::type_error(message.into(), operation.span).with_stable_code("INCAN-T0126");
        let error = match conflict.kind {
            ConflictKind::Permission => permission_origin(error, graph, &operation.event),
            ConflictKind::Moved(origin) => error
                .with_related_span(graph.nodes[origin].span, "ownership transferred here")
                .with_hint("Use the value before transferring it, or reinitialize the moved place before using it"),
            ConflictKind::UnavailableLoan(loan) => error
                .with_related_span(graph.loans[loan.0].span, "this borrow is established here")
                .with_hint("Establish the borrow on every path that reaches its use"),
            ConflictKind::ActiveLoan(loan) => {
                let mut error = error.with_related_span(graph.loans[loan.0].span, "conflicting borrow starts here");
                if let Some(use_) = dependent_use(graph, analysis, conflict.node, loan) {
                    error = error.with_related_span(use_.span, "this reachable use keeps the borrow live");
                }
                error.with_hint("Finish the dependent use before accessing the overlapping place")
            }
        };
        projected.push(ProjectedConflict {
            error,
            priority: 0,
            builtin: None,
        });
    }
    projected.into_iter().map(|conflict| conflict.error).collect()
}

/// One emitted diagnostic and the lazy-reader identity used to collapse graph-level duplicates and cascades.
struct ProjectedConflict {
    error: CompileError,
    priority: u8,
    builtin: Option<BuiltinConflictKey>,
}

/// Stable source facts that distinguish one retained reader of one place from another.
#[derive(PartialEq, Eq)]
struct BuiltinConflictKey {
    reader: Span,
    source: Place,
}

/// Prefer the ownership-changing event when one synthetic escape boundary also records a source read.
fn builtin_conflict_priority(event: &Event) -> u8 {
    match event {
        Event::Access {
            access: Access::Move, ..
        } => 4,
        Event::Access {
            access: Access::Replace,
            ..
        } => 3,
        Event::Access {
            access: Access::Write, ..
        } => 2,
        Event::Access {
            access: Access::Read, ..
        } => 1,
        Event::Borrow(_) | Event::Nothing => 0,
    }
}

/// Recover the canonical source and reader origin before wording projects either into a diagnostic.
fn builtin_conflict_key(
    graph: &Graph,
    analysis: &Analysis,
    facts: &TypeCheckInfo,
    node: usize,
    loan: LoanId,
) -> BuiltinConflictKey {
    let use_ = dependent_use(graph, analysis, node, loan);
    let reader = lazy_reader(graph, facts, loan, use_.map(|use_| use_.loan));
    BuiltinConflictKey {
        reader: reader.span,
        source: graph.loans[loan.0].source.clone(),
    }
}

/// Select the retained builtin loan responsible for this conflict, including a permission or moved conflict emitted
/// at the same synthetic escape boundary.
fn builtin_conflict_loan(
    graph: &Graph,
    analysis: &Analysis,
    facts: &TypeCheckInfo,
    conflict: &super::flow::Conflict,
) -> Option<LoanId> {
    let direct = match conflict.kind {
        ConflictKind::ActiveLoan(loan) | ConflictKind::UnavailableLoan(loan) => Some(loan),
        ConflictKind::Permission | ConflictKind::Moved(_) => match graph.nodes[conflict.node].event {
            Event::Access {
                through: Some(loan), ..
            } => Some(loan),
            _ => analysis.conflicts.iter().find_map(|candidate| {
                (candidate.node == conflict.node)
                    .then_some(candidate)
                    .and_then(|candidate| match candidate.kind {
                        ConflictKind::ActiveLoan(loan) => Some(loan),
                        _ => None,
                    })
            }),
        },
    }?;
    let mut loan = Some(direct);
    while let Some(id) = loan {
        if super::builtins::lazy_result_loan(graph, facts, id) {
            return Some(id);
        }
        loan = graph.loans.get(id.0)?.parent;
    }
    None
}

/// Explain a lazy builtin conflict using the source place, the conflicting action and the dependent later use.
fn builtin_result_conflict(
    graph: &Graph,
    analysis: &Analysis,
    facts: &TypeCheckInfo,
    node: usize,
    loan: LoanId,
) -> CompileError {
    let operation = &graph.nodes[node];
    let retained = &graph.loans[loan.0];
    let source = display_place(&retained.source);
    let (action, completed, ongoing) = match operation.event {
        Event::Borrow(_) => ("borrow", "borrowed", "borrowing"),
        Event::Access {
            access: Access::Read, ..
        } => ("read", "read", "reading"),
        Event::Access {
            access: Access::Write, ..
        } => ("change", "changed", "changing"),
        Event::Access {
            access: Access::Replace,
            ..
        } => ("replace", "replaced", "replacing"),
        Event::Access {
            access: Access::Move, ..
        } => ("move", "moved", "moving"),
        Event::Nothing => ("access", "accessed", "accessing"),
    };
    let use_ = dependent_use(graph, analysis, node, loan);
    let reader = lazy_reader(graph, facts, loan, use_.as_ref().map(|use_| use_.loan));
    let subject = reader.subject();
    if matches!(
        operation.event,
        Event::Access {
            access: Access::Move,
            ..
        }
    ) && reader.span == operation.span
        && use_.as_ref().is_some_and(|use_| use_.span == operation.span)
    {
        return CompileError::type_error(
            format!("cannot let {subject} escape while it is still reading `{source}`"),
            operation.span,
        )
        .with_stable_code("INCAN-T0001")
        .with_related_span(operation.span, format!("{subject} reads `{source}` and escapes here"))
        .with_hint(reader.escape_hint(&source));
    }
    let mut error = CompileError::type_error(
        format!("cannot {action} `{source}` while {subject} is still reading it"),
        operation.span,
    )
    .with_stable_code("INCAN-T0001")
    .with_related_span(reader.span, reader.origin_label(&source))
    .with_related_span(operation.span, format!("`{source}` is {completed} here"));
    if let Some(use_) = use_ {
        error = error.with_related_span(use_.span, reader.use_label());
    }
    error.with_hint(reader.hint(&source, ongoing))
}

/// One source-level lazy reader, named when the retained value itself has a local binding.
struct LazyReader {
    name: Option<String>,
    kind: &'static str,
    span: Span,
}

impl LazyReader {
    /// Name the value whose retained read conflicts with the requested operation.
    fn subject(&self) -> String {
        match &self.name {
            Some(name) => format!("`{name}`"),
            None if self.kind == "generator expression" => "the generator expression".into(),
            None => "the iterator made here".into(),
        }
    }

    /// Explain where this reader began retaining the source place.
    fn origin_label(&self, source: &str) -> String {
        match &self.name {
            Some(name) => format!("`{name}` reads `{source}` from here"),
            None if self.kind == "generator expression" => {
                format!("the generator expression here reads `{source}`")
            }
            None => format!("the iterator made here reads `{source}`"),
        }
    }

    /// Explain the path-sensitive use that keeps this reader live.
    fn use_label(&self) -> String {
        match &self.name {
            Some(name) => format!("`{name}` is used again here"),
            None => format!("the {} is used again here", self.kind),
        }
    }

    /// Suggest ending the dependent use or making the list source explicit and independent.
    fn hint(&self, source: &str, ongoing: &str) -> String {
        let reader = match &self.name {
            Some(name) => format!("`{name}`"),
            None => format!("the {}", self.kind),
        };
        if self.kind == "generator expression" {
            return format!(
                "finish using {reader} before {ongoing} `{source}`, or iterate over `{source}.clone()` to use a copy"
            );
        }
        format!(
            "finish using {reader} before {ongoing} `{source}`, or write `{source}.clone().iter()` to iterate over a copy"
        )
    }

    /// Suggest transferring owned storage or cloning the source when the lazy reader itself escapes.
    fn escape_hint(&self, source: &str) -> String {
        if self.kind == "generator expression" {
            return format!(
                "move an owned list into the escaping generator, or iterate over `{source}.clone()` to give it an independent copy"
            );
        }
        format!(
            "move an owned list into the escaping iterator, or write `{source}.clone().iter()` to give it an independent copy"
        )
    }
}

/// Recover the reader's user-facing identity from the dependency used after the conflict, then its lazy origin.
fn lazy_reader(graph: &Graph, facts: &TypeCheckInfo, conflict: LoanId, used: Option<LoanId>) -> LazyReader {
    let selected = used.unwrap_or(conflict);
    let name = graph.dependent_locals.iter().find_map(|(local, dependencies)| {
        (dependencies.contains(&selected) && lazy_local(facts, local)).then(|| local.declaration_name.clone())
    });
    let (kind, span) = lazy_origin(graph, facts, selected)
        .or_else(|| lazy_origin(graph, facts, conflict))
        .unwrap_or(("iterator", graph.loans[conflict.0].span));
    LazyReader { name, kind, span }
}

/// Return whether a dependent local itself is the lazy reader rather than an aggregate carrying one.
fn lazy_local(facts: &TypeCheckInfo, local: &incan_semantics_core::CanonicalSymbolId) -> bool {
    matches!(facts.resolved_write_type(local.declaration_span, &local.declaration_name),
        Some(crate::symbols::ResolvedType::Generic(name, _))
            if collections::from_str(name) == Some(CollectionTypeId::Generator)
                || incan_lang::lang::traits::from_qualified_str(name)
                    == Some(incan_lang::lang::traits::TraitId::Iterator))
}

/// Find the nearest iterator call or generator expression in a retained loan's ancestry.
fn lazy_origin(graph: &Graph, facts: &TypeCheckInfo, start: LoanId) -> Option<(&'static str, Span)> {
    let mut current = Some(start);
    while let Some(id) = current {
        let loan = graph.loans.get(id.0)?;
        if super::builtins::retained_iterator_method(facts, loan.span).is_some() {
            return Some(("iterator", loan.span));
        }
        if matches!(facts.expr_type(loan.span),
            Some(crate::symbols::ResolvedType::Generic(name, _))
                if collections::from_str(name) == Some(CollectionTypeId::Generator))
        {
            return Some(("generator expression", loan.span));
        }
        current = loan.parent;
    }
    None
}

/// Render a canonical source place without reconstructing it from source spelling.
fn display_place(place: &super::flow::Place) -> String {
    let mut value = match &place.root {
        Root::Binding(identity) => identity.declaration_name.clone(),
        Root::Temporary { .. } => "temporary".into(),
    };
    for projection in &place.projection {
        match projection {
            super::flow::Projection::Field(identity) => {
                value.push('.');
                value.push_str(&identity.declaration_name);
            }
            super::flow::Projection::Element(_) => value.push_str("[item]"),
        }
    }
    value
}

/// Explain the exact source grant used for permission checking. A reborrow derives access from its parent rather
/// than upgrading to its original owner's permissions; local mutability cannot grant shared storage writes.
fn permission_origin(error: CompileError, graph: &Graph, event: &Event) -> CompileError {
    let (place, through, request) = match event {
        Event::Borrow(id) => {
            let loan = &graph.loans[id.0];
            (
                &loan.source,
                loan.parent,
                "Borrowing does not upgrade the source's access permission",
            )
        }
        Event::Access { place, access, through } => (
            place,
            *through,
            match access {
                Access::Read => "Preserve the source's declared access permission",
                Access::Write | Access::Replace => "Writing requires a writable owner or a writable exclusive borrow",
                Access::Move => "A borrowed place does not grant ownership transfer",
            },
        ),
        Event::Nothing => return error,
    };
    let error = match through {
        Some(loan) => error.with_related_span(graph.loans[loan.0].span, "access is granted by this borrow"),
        None => match &place.root {
            Root::Binding(identity) => {
                error.with_related_declaration(identity.clone(), "access is granted by this declaration")
            }
            Root::Temporary { expression, .. } => {
                error.with_related_span(*expression, "temporary storage is created here")
            }
        },
    };
    error.with_hint(request)
}

/// Find one reachable dependent access that witnesses a conflicting loan's liveness. Traversal is restricted to
/// live paths and terminates across loop back-edges; reborrow accesses also keep their ancestors live.
fn dependent_use(graph: &Graph, analysis: &Analysis, conflict: usize, loan: LoanId) -> Option<DependentUse> {
    let mut pending: VecDeque<_> = graph.nodes[conflict].successors.iter().copied().collect();
    let mut visited = BTreeSet::new();
    while let Some(index) = pending.pop_front() {
        if !visited.insert(index) || !analysis.live_before[index].contains(&loan) {
            continue;
        }
        let node = &graph.nodes[index];
        if let Event::Access {
            through: Some(used), ..
        } = node.event
        {
            let mut ancestor = Some(used);
            while let Some(id) = ancestor {
                if id == loan {
                    return Some(DependentUse {
                        span: node.span,
                        loan: used,
                    });
                }
                ancestor = graph.loans[id.0].parent;
            }
        }
        pending.extend(node.successors.iter().copied());
    }
    None
}

/// One later access and the exact retained loan through which it reads.
struct DependentUse {
    span: Span,
    loan: LoanId,
}
