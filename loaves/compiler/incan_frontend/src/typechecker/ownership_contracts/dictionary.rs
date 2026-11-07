//! Dictionary lookup decisions projected from canonical ownership graph loans and liveness.

use std::collections::BTreeSet;

use incan_lang::interop::RustItemKind;
use incan_lang::lang::builtins::BuiltinMethodId;
use incan_lang::lang::derives::DeriveId;
use incan_lang::lang::surface::dict_methods::DictMethodId;
use incan_lang::lang::surface::types as surface_types;
use incan_lang::lang::traits::{self, TraitId};
use incan_lang::lang::types::collections::{self, CollectionTypeId};

use super::TypeChecker;
use super::flow::{Access, Analysis, Conflict, ConflictKind, DictionaryLookup, Event, Graph, LoanId};
use crate::ast::{AssignmentStmt, Expr, Span, Spanned};
use crate::diagnostics::errors;
use crate::symbols::ResolvedType;
use crate::typechecker::IdentKind;
use crate::typechecker::derive_requirements::DeriveSupport;

impl TypeChecker {
    /// Retain the graph activation for one optional lookup and the stable refusal payload when its stored value cannot
    /// be copied. Settlement remains diagnostic ordering only; read-versus-copy is decided by graph liveness.
    pub(in crate::typechecker) fn note_dict_lookup_value(&mut self, call_span: Span, value_ty: &ResolvedType) {
        if self.value_type_cannot_be_copied(value_ty) {
            self.pending_uncopyable_dict_lookups.push((
                self.check_work.dictionary_lookups,
                call_span,
                value_ty.to_string(),
            ));
            self.check_work.dictionary_lookups += 1;
        }
    }

    /// Activate graph construction only when a pattern consumes a staged uncopyable lookup. Copyable values need no
    /// ownership decision; for uncopyable values the graph owns every later access decision. This hook retains no
    /// pattern names, source place, use count or copy decision.
    pub(in crate::typechecker) fn note_dictionary_pattern_lookup(&mut self, lookup: &Spanned<Expr>) {
        if !self
            .pending_uncopyable_dict_lookups
            .iter()
            .any(|(_, span, _)| *span == lookup.span)
        {
            return;
        }
        let Expr::MethodCall(_, _, _, arguments) = &lookup.node else {
            return;
        };
        if arguments.len() == 1
            && self.type_info.resolved_builtin_method_call(lookup.span)
                == Some(BuiltinMethodId::Dict(DictMethodId::Get))
            && let Some(identity) = self.current_body_identity().cloned()
        {
            self.dictionary_lookup_bodies.insert(identity);
        }
    }

    /// Refuse every lookup whose canonical graph did not prove an in-place observation of an uncopyable value.
    pub(in crate::typechecker) fn refuse_copying_dict_lookups_of_uncopyable_values(&mut self) {
        self.check_work.dictionary_settlements += 1;
        for (_, span, value) in std::mem::take(&mut self.pending_uncopyable_dict_lookups) {
            if !self.type_info.is_read_only_dict_lookup(span) {
                self.errors
                    .push(errors::kept_dict_lookup_value_cannot_be_copied(&value, span));
            }
        }
    }

    /// Remember a local bound directly to module static storage so graph lookup projection never treats it as an
    /// ordinary local owner.
    pub(in crate::typechecker) fn note_static_alias_binding(&mut self, assign: &AssignmentStmt) {
        if let Expr::Ident(static_name) = &assign.value.node
            && self.type_info.ident_kind(assign.value.span) == Some(IdentKind::Static)
            && let Some(id) = self.symbols.lookup(&assign.name)
        {
            self.static_alias_bindings.insert(id, static_name.clone());
        }
    }

    /// Whether an already-checked expression reads module static storage, directly or through its recorded alias.
    pub(in crate::typechecker) fn expr_reads_static_storage(&self, expr: &Spanned<Expr>) -> bool {
        match &expr.node {
            Expr::Ident(name) => {
                self.type_info.ident_kind(expr.span) == Some(IdentKind::Static)
                    || self
                        .symbols
                        .lookup(name)
                        .is_some_and(|id| self.static_alias_bindings.contains_key(&id))
            }
            Expr::Field(object, _) | Expr::Index(object, _) | Expr::Paren(object) => {
                self.expr_reads_static_storage(object)
            }
            _ => false,
        }
    }

    /// Whether a stored dictionary value lacks the copy operation required by an owned lookup result.
    pub(in crate::typechecker) fn value_type_cannot_be_copied(&self, ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::RustPath(path) => self.rust_type_proven_not_clone(path),
            ResolvedType::TypeVar(_) => false,
            ResolvedType::Generic(name, args)
                if matches!(
                    collections::from_str(name),
                    Some(
                        CollectionTypeId::List
                            | CollectionTypeId::Dict
                            | CollectionTypeId::Set
                            | CollectionTypeId::Tuple
                            | CollectionTypeId::Option
                            | CollectionTypeId::Result
                    )
                ) =>
            {
                args.iter().any(|arg| self.value_type_cannot_be_copied(arg))
            }
            ResolvedType::Tuple(args) => args.iter().any(|arg| self.value_type_cannot_be_copied(arg)),
            ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => self.value_type_cannot_be_copied(inner),
            ResolvedType::FrozenDict(key, value) => {
                self.value_type_cannot_be_copied(key) || self.value_type_cannot_be_copied(value)
            }
            ResolvedType::Named(name) if surface_types::from_str(name).is_some() => {
                matches!(self.derive_support(ty, DeriveId::Clone), DeriveSupport::Missing(_))
            }
            ResolvedType::CallSiteInfer => false,
            other => !self.is_clone_type(other),
        }
    }

    /// Whether complete inspected metadata proves that a native value has no `Clone` implementation.
    fn rust_type_proven_not_clone(&self, path: &str) -> bool {
        let base = path.split('<').next().unwrap_or(path).trim();
        let Some(metadata) = self.rust_item_metadata_for_path(base) else {
            return false;
        };
        let RustItemKind::Type(info) = &metadata.kind else {
            return false;
        };
        if !info.metadata_completeness.has_trait_impls() {
            return false;
        }
        let is_clone =
            |path: &str| traits::rust_paths(TraitId::Clone).contains(&path) || path == traits::as_str(TraitId::Clone);
        !(info
            .implemented_traits
            .iter()
            .any(|implemented| is_clone(&implemented.path))
            || info
                .expanded_derive_traits
                .iter()
                .any(|implemented| is_clone(&implemented.path)))
    }
}

/// Return the optional lookups whose pattern bindings remain in-place observations on every graph path.
pub(super) fn read_only_lookups(graph: &Graph, analysis: &Analysis) -> BTreeSet<crate::ast::Span> {
    graph
        .dictionary_lookups
        .iter()
        .filter_map(|(span, lookup)| lookup_is_read_only(graph, analysis, lookup).then_some(*span))
        .collect()
}

/// Whether a conflict is the graph form of a dictionary lookup choosing its existing copy path.
pub(super) fn owns_conflict(graph: &Graph, analysis: &Analysis, conflict: &Conflict) -> bool {
    graph.dictionary_lookups.values().any(|lookup| {
        lookup.loans.iter().any(|loan| {
            conflict_uses_loan(graph, conflict, *loan)
                || access_touches_live_lookup(graph, analysis, conflict.node, lookup, *loan)
        })
    })
}

/// A lookup remains borrowed when its bindings only read through their loan and no independent access touches the
/// dictionary entry while that loan or one of its reborrows is live.
fn lookup_is_read_only(graph: &Graph, analysis: &Analysis, lookup: &DictionaryLookup) -> bool {
    lookup.loans.iter().all(|loan| {
        !analysis
            .conflicts
            .iter()
            .any(|conflict| conflict_uses_loan(graph, conflict, *loan))
            && graph.nodes.iter().enumerate().all(|(node, event)| {
                let Event::Access { place, access, through } = &event.event else {
                    return true;
                };
                if through.is_some_and(|used| descends_from(graph, used, *loan)) {
                    return *access == Access::Read;
                }
                !analysis.live_before[node]
                    .iter()
                    .any(|live| descends_from(graph, *live, *loan))
                    || !lookup.source.overlaps(place)
            })
    })
}

/// Associate permission and active-loan conflicts with the dictionary loan that caused them.
fn conflict_uses_loan(graph: &Graph, conflict: &Conflict, loan: LoanId) -> bool {
    match conflict.kind {
        ConflictKind::ActiveLoan(active) | ConflictKind::UnavailableLoan(active) => descends_from(graph, active, loan),
        ConflictKind::Permission | ConflictKind::Moved(_) => matches!(
            graph.nodes.get(conflict.node).map(|node| &node.event),
            Some(Event::Access { through: Some(used), .. }) if descends_from(graph, *used, loan)
        ),
    }
}

/// Test an independent access at one conflict node against one live lookup loan.
fn access_touches_live_lookup(
    graph: &Graph,
    analysis: &Analysis,
    node: usize,
    lookup: &DictionaryLookup,
    loan: LoanId,
) -> bool {
    let Some(Event::Access { place, through, .. }) = graph.nodes.get(node).map(|node| &node.event) else {
        return false;
    };
    through.is_none_or(|used| !descends_from(graph, used, loan))
        && analysis
            .live_before
            .get(node)
            .is_some_and(|live| live.iter().any(|live| descends_from(graph, *live, loan)))
        && lookup.source.overlaps(place)
}

/// Follow reborrow parents to the lookup loan without deriving ancestry from spans.
fn descends_from(graph: &Graph, mut loan: LoanId, ancestor: LoanId) -> bool {
    loop {
        if loan == ancestor {
            return true;
        }
        let Some(parent) = graph.loans.get(loan.0).and_then(|loan| loan.parent) else {
            return false;
        };
        loan = parent;
    }
}
