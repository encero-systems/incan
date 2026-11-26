//! Deferred source-consumption decisions for direct collection comprehensions (#1983).
//!
//! This state is deliberately limited to the comprehension contract. Loop-item transfer, list invalidation and reuse
//! belong to the canonical ownership graph and must not acquire another binding or control-flow ledger here.

use incan_lang::lang::traits::TraitId;
use incan_lang::lang::types::collections::CollectionTypeId;

use crate::ast::{Expr, Span, Spanned};
use crate::symbols::{ResolvedType, SymbolId, SymbolKind};
use crate::typechecker::helpers::collection_type_id;

use super::TypeChecker;

/// Per-callable facts needed after all later binding reads are known.
#[derive(Debug, Default, Clone)]
pub(super) struct ComprehensionSources {
    reads: Vec<(SymbolId, Span)>,
    pending: Vec<PendingComprehension>,
}

/// One comprehension over a collection whose borrowed iteration plan copies elements.
#[derive(Debug, Clone)]
struct PendingComprehension {
    symbol: SymbolId,
    source_span: Span,
    collection_type: String,
    type_param: String,
    has_clone_bound: bool,
    can_consume_if_last: bool,
}

impl TypeChecker {
    /// Record a collection comprehension whose source-consumption decision Body IR lowers from (#1983).
    pub(super) fn plan_comprehension_collection_source(&mut self, iter_expr: &Spanned<Expr>, iter_ty: &ResolvedType) {
        let Some(type_param) = comprehension_copied_element_type(iter_ty)
            .and_then(|element| self.active_type_param_name(element))
            .map(str::to_owned)
        else {
            return;
        };
        let Expr::Ident(name) = &iter_expr.node else {
            return;
        };
        let Some(symbol) = self.symbols.lookup(name) else {
            return;
        };
        let is_immutable = self
            .symbols
            .get(symbol)
            .is_some_and(|definition| matches!(&definition.kind, SymbolKind::Variable(info) if !info.is_mutable));
        let can_consume_if_last =
            is_immutable && is_list_type(iter_ty) && self.loop_stack.is_empty() && self.closure_depth == 0;
        let has_clone_bound = self.active_type_param_satisfies_builtin_bound(&type_param, TraitId::Clone);
        self.comprehension_sources.pending.push(PendingComprehension {
            symbol,
            source_span: iter_expr.span,
            collection_type: iter_ty.to_string(),
            type_param,
            has_clone_bound,
            can_consume_if_last,
        });
    }

    /// Finalize comprehension source plans after the complete callable body has populated the binding-read ledger.
    pub(super) fn finalize_comprehension_collection_sources(&mut self) {
        let pending = std::mem::take(&mut self.comprehension_sources.pending);
        for comprehension in pending {
            let has_later_read =
                self.comprehension_sources.reads.iter().any(|(symbol, span)| {
                    *symbol == comprehension.symbol && span.start > comprehension.source_span.start
                });
            let consumed = comprehension.can_consume_if_last && !has_later_read;
            self.type_info
                .record_comprehension_source_consumption(comprehension.source_span, consumed);
            if !consumed && !comprehension.has_clone_bound {
                self.errors.push(
                    crate::diagnostics::errors::comprehension_element_copy_missing_clone_bound(
                        &comprehension.type_param,
                        &comprehension.collection_type,
                        comprehension.source_span,
                    ),
                );
            }
        }
    }

    /// Retain a binding read for a pending direct comprehension without participating in loop-item ownership.
    pub(super) fn observe_comprehension_source_read(&mut self, symbol: SymbolId, span: Span) {
        self.comprehension_sources.reads.push((symbol, span));
    }

    /// Start an isolated callable's comprehension decisions.
    pub(super) fn enter_comprehension_source_scope(&mut self) -> ComprehensionSources {
        std::mem::take(&mut self.comprehension_sources)
    }

    /// Restore the enclosing callable's comprehension decisions.
    pub(super) fn exit_comprehension_source_scope(&mut self, previous: ComprehensionSources) {
        self.comprehension_sources = previous;
    }
}

/// Return the direct element type a borrowed comprehension copies from this collection type.
fn comprehension_copied_element_type(ty: &ResolvedType) -> Option<&ResolvedType> {
    let ty = match ty {
        ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => inner.as_ref(),
        other => other,
    };
    match ty {
        ResolvedType::Generic(name, args)
            if matches!(
                collection_type_id(name),
                Some(CollectionTypeId::List | CollectionTypeId::Set | CollectionTypeId::Dict)
            ) =>
        {
            args.first()
        }
        _ => None,
    }
}

/// Return whether `ty` is an ordinary list, looking through a parameter reference wrapper.
fn is_list_type(ty: &ResolvedType) -> bool {
    match ty {
        ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => is_list_type(inner),
        ResolvedType::Generic(name, _) => collection_type_id(name) == Some(CollectionTypeId::List),
        _ => false,
    }
}
