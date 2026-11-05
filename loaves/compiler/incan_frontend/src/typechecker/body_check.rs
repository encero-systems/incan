//! Declaration and callable-statement outputs and deferred decisions, before module settlement.
//!
//! Collected declaration inputs remain with G1. Declaration validation, defaults and initializers own outputs
//! independently from statement bodies. Each operation uses the same local writer and existing semantic rules;
//! settlement consumes those outputs without rechecking an expression or statement. Settled outputs store their
//! diagnostics as anchored semantic values, and the published diagnostics are rendered from those values.

use super::diagnostic_values::{AnchorFault, AnchoredDiagnostic, DiagnosticPositions};
use super::mut_arguments::{self, MutBodyLocal};
use super::ownership_contracts::CallableSource;
use super::type_info::{
    CAbiInteropArtifacts, CallArtifacts, ExpressionArtifacts, ProtocolArtifacts, ReferenceArtifacts,
    RustInteropArtifacts, TraitArtifacts, TypeCheckInfo,
};
use super::{TypeChecker, VisibilityPoint, YieldContext};
use crate::ast::{FunctionDecl, Program, Span, Spanned, Statement};
use crate::diagnostics::{CompileError, ErrorKind};
use crate::symbols::ResolvedType;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Semantic operation that owns one local output; declaration expressions never count as statement-body checks.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum LocalOwnerKind {
    #[default]
    Body,
    Declaration,
}

/// Owned local checked output. Open-value groups keep body-local indices until the compatibility join relocates
/// them; mutation and dictionary decisions remain pending for their existing module-order settlement owners.
#[derive(Debug, Default, Clone)]
pub(super) struct BodyLocal {
    /// Revision-local owner handle; semantic identity is supplied separately by the record adapter.
    owner: Span,
    /// Declaration/default output is settled independently from callable statement output.
    kind: LocalOwnerKind,
    order: BodyDiagnosticOrder,
    /// Body-owned lexical declarations retained independently of a later whole-symbol-table export.
    nested: BTreeMap<Span, incan_semantics_core::CanonicalSymbolId>,
    expressions: ExpressionArtifacts,
    references: ReferenceArtifacts,
    calls: CallArtifacts,
    protocols: ProtocolArtifacts,
    rust: RustInteropArtifacts,
    traits: TraitArtifacts,
    c_abi: CAbiInteropArtifacts,
    mutation: MutBodyLocal,
    dictionary: Vec<(usize, Span, String)>,
    iterator_sum_items: BTreeMap<Span, ResolvedType>,
    /// This owner's own entries in the active diagnostic writer, excluding nested owners. Settlement captures them
    /// as anchored values once; these invocation-local indices never enter a settled view.
    diagnostic_slots: Vec<usize>,
    warning_slots: Vec<usize>,
}

/// One source-order body activation against the current declaration/environment views and local scratch. Earlier
/// output is held outside the active writer, never cloned as a body input or recomputed by the scope.
pub(super) struct BodyCheckScope {
    enclosing: BodyLocal,
    symbol_start: usize,
    output_start: usize,
    order: BodyDiagnosticOrder,
    diagnostic_start: usize,
    warning_start: usize,
    child_start: usize,
    visibility: VisibilityPoint,
}

impl BodyCheckScope {
    /// Borrow G1's current visibility point before taking local tables. Declaration validation and prepared
    /// statement checking enter independently; collected tables and symbols remain their read authority.
    pub(super) fn enter(checker: &mut TypeChecker) -> Self {
        let visibility = checker.check_environment().visibility_point();
        let mut enclosing = BodyLocal::take(checker);
        enclosing.install_shared_inputs(checker);
        enclosing.mutation.enter_statement_frame(&mut checker.mut_params);
        #[cfg(test)]
        checker.body_enclosing_observations.push(enclosing.clone());
        Self {
            enclosing,
            symbol_start: checker.symbols.all_symbols().len(),
            output_start: checker.body_checks.len(),
            order: BodyDiagnosticOrder {
                mutation: checker.check_work.mutation_arguments,
                dictionary: checker.check_work.dictionary_lookups,
            },
            diagnostic_start: checker.errors.len(),
            warning_start: checker.warnings.len(),
            child_start: checker.diagnostic_owner_ranges.len(),
            visibility,
        }
    }

    /// Return the owned body result and restore its enclosing writer, preserving source diagnostic order. Pending
    /// decisions are recorded, not resolved, and the environment cannot advance during this activation.
    pub(super) fn finish(mut self, checker: &mut TypeChecker) -> BodyLocal {
        debug_assert_eq!(self.visibility, checker.check_environment().visibility_point());
        let mut local = BodyLocal::take(checker);
        local.nested = checker
            .symbols
            .nested_declaration_identities_from(self.symbol_start)
            .filter(|(span, _)| {
                !checker.body_checks[self.output_start..]
                    .iter()
                    .any(|child| child.local().nested.contains_key(span))
            })
            .map(|(span, identity)| (span, identity.clone()))
            .collect();
        local.order = self.order;
        let children = &checker.diagnostic_owner_ranges[self.child_start..];
        local.diagnostic_slots = (self.diagnostic_start..checker.errors.len())
            .filter(|index| !children.iter().any(|(errors, _)| errors.contains(index)))
            .collect();
        local.warning_slots = (self.warning_start..checker.warnings.len())
            .filter(|index| !children.iter().any(|(_, warnings)| warnings.contains(index)))
            .collect();
        checker.diagnostic_owner_ranges.push((
            self.diagnostic_start..checker.errors.len(),
            self.warning_start..checker.warnings.len(),
        ));
        local.mutation.return_statement_frame(&mut self.enclosing.mutation);
        local.restore_shared_inputs(&mut self.enclosing);
        self.enclosing.install(checker);
        #[cfg(test)]
        let _ = checker.body_enclosing_observations.pop();
        local
    }
}

mod artifact_merge;
mod baseline_compare;
mod body_locals;
mod checked_views;
mod declaration_owners;
pub(crate) mod records;

use artifact_merge::*;
#[cfg(test)]
mod tests;

/// A completed local output and its separately ordered settlement diagnostics. Publication consumes this value only
/// after the local checker and the effect producer have finished; it never enters `check_callable_body` again.
/// Body IR combines these local facts with separate declaration and BodyFacts inputs. Later module publication
/// projections, including native-newtype and empty-list facts, are not recreated by this view.
///
/// Every diagnostic the owner produced is stored as an [`AnchoredDiagnostic`]: local refusals and advisories, the
/// final mutation and dictionary decisions, and open-value refusals. Their ranges are structural anchors into the
/// checked syntax, and the current revision's [`DiagnosticPositions`] binds them when they are published.
#[derive(Debug, Clone)]
pub(crate) struct CheckedBodyView {
    local: Arc<BodyLocal>,
    effect_reads: super::mut_arguments::MutEffectReads,
    diagnostics: Vec<AnchoredDiagnostic>,
    warnings: Vec<AnchoredDiagnostic>,
    mutation: Vec<(usize, AnchoredDiagnostic)>,
    dictionary: Vec<(usize, AnchoredDiagnostic)>,
    open: Vec<AnchoredDiagnostic>,
}

/// Prepared local work or a previous final value whose local-check inputs the caller has already validated.
/// This adapter owns no cache or scheduler; the engine remains responsible for input applicability and positions.
#[derive(Debug, Clone)]
pub(super) enum BodyCheckInput {
    Local(BodyLocal),
    Reuse {
        checked: Arc<CheckedBodyView>,
        order: BodyDiagnosticOrder,
    },
}

/// Invocation-local traversal binding, separated from the reusable body's semantic decisions.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct BodyDiagnosticOrder {
    mutation: usize,
    dictionary: usize,
}

/// Invalid use of the native read adapter, rather than a new source-language diagnostic. The record scope separately
/// binds both revision-local handles to the same checked source revision and producer profile.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum CheckedBodyInputError {
    /// A statement view and a declaration/default view must keep their different ownership roles.
    #[error("native body inputs require statement and declaration owners")]
    WrongOwnerRole,
    /// The supplied declaration belongs to another revision-local lexical owner.
    #[error("native body inputs belong to different declaration owners")]
    DifferentOwner,
    /// Refused predecessor values cannot supply executable Body IR inputs.
    #[error("native body inputs require accepted settled predecessors")]
    RefusedOwner,
}

/// Native read-only settled owners retained for an explicit Body IR baseline activation. The compatibility module
/// projection is not a keyed fact source; retaining it must never be mistaken for record-backed production.
#[derive(Debug, Default, Clone)]
pub(crate) struct BodyIrBaselineInputs {
    declarations: BTreeMap<Span, Arc<CheckedBodyView>>,
    bodies: BTreeMap<Span, Arc<CheckedBodyView>>,
}
