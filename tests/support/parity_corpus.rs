//! Schema, validation, and CI-summary emission for the #987 backend-cutover parity corpus.
//!
//! This module turns the #646 backend behavior inventory (`workspaces/docs-site/docs/contributing/reference/
//! backend_behavior_inventory.md`) into an executable shape. Each [`ParityCase`] is a stable, identified claim
//! about current compiler behavior, tagged with the inventory category and evidence lane that justifies it, plus
//! an explicit disposition for the 0.6 backend cutover (`#652`): preserved, intentionally migrated, or unsupported
//! with an owning issue.
//!
//! Two things this module deliberately refuses to do:
//!
//! - Treat generated Rust token shape as semantic authority. A case's [`ParityCase::evaluate`] function must compare
//!   frontend-observable, user-visible outcomes (typecheck acceptance/rejection, diagnostic presence, runtime helper
//!   results) even when its evidence lane is [`EvidenceLane::CodegenSnapshot`]; that lane may only assert that
//!   generation succeeds and stays syntactically valid Rust, never that a particular token layout is the contract.
//! - Default anything to green. [`ComparisonOutcome`] and [`ReceiptRef`] both carry explicit non-`Match`/ non-available
//!   states so that unavailable, skipped, or incompatible comparisons are visible in the emitted summary rather than
//!   silently counted as parity.
//!
//! ## Receipt-awareness (#986, landed via PR #1120)
//!
//! #987's own scope calls for "receipt-aware reference/replacement or shadow comparisons where both paths are
//! available." #986 landed [`incan::backend::selection`], so rows that actually execute a backend declare a real
//! [`BackendSelection`] before execution and finalize that exact run into a real
//! [`incan::backend::selection::BackendExecutionReceipt`] — the same three-call sequence
//! `src/cli/commands/build.rs` uses for an actual build. Compiler-behavior callbacks,
//! checked-graph generation, and artifact inspection are not backend executions and therefore carry distinct
//! observation evidence rather than fabricated receipts. The #988 Body-IR cases additionally carry a Body-IR
//! snapshot, ownership evidence, and runtime requirements.
//!
//! ## Source-observable comparison (#1146)
//!
//! A row may additionally declare the bounded comparison profile in [`incan::backend::shadow`]. That row is then
//! observed twice, independently: once by direct Body-IR execution, and once by building the emitted Rust with a
//! real native compiler and running the produced program as a separate process. Only such a row can reach
//! [`OverallState::Green`], and only when the comparison actually ran and both routes produced the same
//! source-level observable. Every other row stays non-green with the concrete reason no comparison was made —
//! generated Rust is never substituted as semantic proof.
//!
//! The legacy route is Oven-owned, so a row that declares a comparison needs a staged Oven capability. The
//! including test crate supplies it as a `shadow_capability` module (`tests/support/shadow_capability.rs`); when
//! nothing is staged, the row degrades to direct-execution-only evidence with that reason recorded — never to a
//! green result, and never losing the replacement execution that did happen.

use incan::backend::IrCodegen;
use incan::backend::replacement::{
    OwnershipReadProjection, ReplacementExecutionGraph, ReplacementValue, RuntimeRequirementProjection,
    TaskLifecycleProjection, execute_prevalidated_free_function, prepare_free_function_execution,
    prepare_free_function_execution_in_graph,
};
use incan::backend::selection::{
    BackendKind, BackendSelection, BackendSelectionError, FallbackPolicy, ShadowComparisonState, digest_output,
    finalize_receipt, resolve_execution, select_backend, unavailable_shadow_comparison,
};
use incan::backend::shadow::{LegacyExecutionAuthority, ShadowComparison, ShadowComparisonProfile};
use incan::cli::commands::compare_source_observable;
use incan::frontend::body_ir::{apply_body_ir_input_contract, build_body_ir_module_v0};
use incan::frontend::diagnostics::DIAGNOSTIC_SCHEMA_VERSION;
use incan::frontend::hir::build_hir_v0;
use incan::frontend::typechecker::{TypeCheckInfo, TypeChecker};
use incan::frontend::{ast, lexer, parser};
use incan_semantics_core::body_ir::{BodyIrModule, CallableTarget, Callee, Rvalue, Statement, StatementKind};
use incan_semantics_core::{
    CanonicalSymbolId, HirModule, SemanticSourceTargetKind, SymbolNamespace, encode_incan_symbol_identity,
};
use proc_macro2::{TokenStream, TokenTree};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::str::FromStr;

// ============================================================================
// Case schema
// ============================================================================

/// One of the seven behavior categories from the #646 inventory's `Categories` table.
///
/// The category names and meanings are the inventory's, not invented here — keep this enum in sync with
/// `workspaces/docs-site/docs/contributing/reference/backend_behavior_inventory.md` if that table changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BehaviorCategory {
    /// Documented or intentionally exposed source-level Incan semantics.
    SupportedLanguageContract,
    /// Behavior owned by `crates/incan_stdlib`, `crates/incan_core`, or `.incn` stdlib source.
    StdlibRuntimeBehavior,
    /// Behavior crossing `rust::` imports, rust-inspect metadata, or generated Cargo projects.
    ///
    /// No seed case uses this category yet: #987's own plan step 3 asks for a narrow source-only seed corpus
    /// before Rust-interop rows (step 4). Kept referenced here so the category taxonomy stays complete ahead of
    /// that growth — it requires an executable interop boundary and receipt-aware comparison.
    #[allow(dead_code)]
    RustInteropBehavior,
    /// Behavior visible mainly through generated Rust shape, manifests, or `target/incan/**` layout.
    GeneratedArtifactBehavior,
    /// Error/warning codes, spans, JSON schema facts, and text diagnostics.
    DiagnosticBehavior,
    /// Accepted only because a parser/typechecker/lowering path happens to allow it without a documented contract.
    AccidentalAcceptedBehavior,
    /// Preserved only because current users may rely on a workaround, or fixing it needs a larger migration.
    ///
    /// No seed case carries this category right now: `parity-987-0006` entered the corpus under it and #1117
    /// migrated it to [`BehaviorCategory::DiagnosticBehavior`]. Kept referenced so the taxonomy stays complete
    /// against the #646 inventory, which still defines the bucket.
    #[allow(dead_code)]
    BugCompatibleBehavior,
}

/// One of the seven evidence lanes from the #646 inventory's `Evidence lanes` table.
///
/// A behavior can be proven from more than one lane; [`ParityCase`] records the primary lane the case's
/// [`ParityCase::evaluate`] function actually exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceLane {
    /// `src/frontend/**` unit tests, diagnostics tests, parser snapshots — source acceptance/rejection.
    DirectParserTypechecker,
    /// `tests/codegen_snapshot_tests.rs`, `tests/snapshots/**` — current generated Rust shape.
    CodegenSnapshot,
    /// Integration tests, stdlib runtime tests, smoke tests — compiled/runtime behavior.
    GeneratedProjectRun,
    /// Typed source lowered to Body IR and executed directly by the bounded replacement profile.
    DirectReplacementBodyIr,
    /// Package consumer fixtures, facade/reexport tests, checked API metadata tests.
    ///
    /// RFC 120 conformance now uses this lane for a checked import/alias/re-export graph. Cross-package replacement
    /// execution remains explicitly unavailable under #989.
    PackageImportBoundary,
    /// Vocab desugarer tests, formatter/test-runner activation paths.
    ///
    /// No seed case uses this lane yet — deferred to plan step 4.
    #[allow(dead_code)]
    VocabTestBatch,
    /// IncQL or Hees.ai acceptance runs when the surface is exercised there.
    ///
    /// No seed case uses this lane yet — deferred to plan step 4.
    #[allow(dead_code)]
    DownstreamProof,
}

/// The cutover disposition for one case, per #987's own "Done when" contract.
///
/// This is deliberately restricted to the three states #987 names explicitly: preserved, intentional migration, or
/// unsupported. There is no fourth "undecided" variant — a case that has not been triaged yet must still pick
/// [`Disposition::Unsupported`] or [`Disposition::IntentionalMigration`] with a real owning issue and a migration
/// note that says what triage is still needed, rather than avoiding the decision.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Disposition {
    /// The behavior is a documented or evidenced contract that the 0.6 backend cutover must keep working.
    Preserved,
    /// The behavior will change deliberately during cutover; a real issue tracks the migration.
    ///
    /// Used by `parity-987-0006`, which #1117 migrated from a silent accept to an `INCAN-T0101` warning.
    IntentionalMigration {
        owning_issue: u32,
        migration_note: &'static str,
    },
    /// The behavior is not guaranteed to survive cutover as-is; a real issue tracks the decision.
    Unsupported {
        owning_issue: u32,
        migration_note: &'static str,
    },
}

impl Disposition {
    /// Return the disposition's serialized tag without allocating, for compact summary rows.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Disposition::Preserved => "preserved",
            Disposition::IntentionalMigration { .. } => "intentional_migration",
            Disposition::Unsupported { .. } => "unsupported",
        }
    }
}

/// The receipt or non-execution evidence reference for one corpus case.
///
/// Execution variants carry the real [`incan::backend::selection::BackendExecutionReceipt::identity`] produced by
/// the run. Identity-only conformance carries a recomputable evidence identity and an optional receipt only when
/// replacement actually ran; generated source and artifact inspection never borrow an execution-shaped identity.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum ReceiptRef {
    /// A callback evaluated its documented behavior without executing a selected backend.
    ///
    /// The evidence identity binds the stable row, its source input, and the exact [`ComparisonOutcome`] returned by
    /// the callback. It is deliberately not shaped like a backend receipt.
    BehaviorObserved {
        evidence_identity: String,
        comparison_reason: String,
    },
    /// A #988 replacement execution with its own #986 selection/execution receipt and Body-IR evidence.
    ///
    /// The comparison remains non-green because the requested source-observable legacy comparator is unavailable;
    /// this variant proves replacement execution without promoting it to parity.
    ReplacementExecuted {
        /// Identity of the pre-execution replacement selection.
        selection_identity: String,
        /// Identity of the finalized replacement execution receipt.
        receipt_identity: String,
        /// Identity of the direct Body-IR output bound into that receipt.
        output_identity: String,
        /// Deterministic snapshot of the Body IR the replacement executor consumed.
        body_snapshot: String,
        /// Canonical ownership facts observed during direct execution.
        ownership_reads: Vec<OwnershipReadProjection>,
        /// Canonical Body-IR runtime requirements observed by direct execution.
        runtime_requirements: Vec<RuntimeRequirementProjection>,
        /// Canonical direct-task lifecycle evidence observed by direct execution. Empty for non-async cases.
        task_lifecycle: Vec<TaskLifecycleProjection>,
        /// Concrete reason the intentionally requested semantic comparison is non-green.
        comparison_reason: String,
    },
    /// Compiler-identity conformance observed through the checked graph, HIR/Body IR consumers, and generated or
    /// recovered symbol projections, with replacement execution recorded when the selected row is source-local.
    ///
    /// This state is deliberately non-green: it proves that both compiler paths retained the selected identities,
    /// but it is not a two-route source-observable execution comparison. Package/import execution remains owned by
    /// #989 and is named explicitly in `comparison_reason` rather than silently treated as parity.
    IdentityConformanceObserved {
        /// Receipt for an actual direct replacement execution when this row has a source-local executable
        /// entrypoint. Generated source or recovered artifact inspection never fabricates an execution receipt.
        replacement_receipt_identity: Option<String>,
        /// Identity of the checked conformance evidence bundle.
        evidence_identity: String,
        /// Concrete reason this observation is not a paired source-observable comparison.
        comparison_reason: String,
    },
    /// Two independent executions of the same source under a #1146 comparison profile agreed.
    ///
    /// This is the only receipt state that may promote a row to [`OverallState::Green`]. It records both routes'
    /// receipt identities separately, because the two receipts are deliberately not interchangeable: they differ
    /// by selected and executed backend and by what each route actually produced.
    ShadowMatched {
        /// Stable kind of comparison profile that ran, for consumers keyed on comparison capability (#1153).
        profile_kind: String,
        /// Content identity of the exact comparison profile instance both routes were bound to.
        profile_identity: String,
        /// The source-level observable both routes produced.
        observable: String,
        /// Identity of the finalized legacy-route receipt.
        legacy_receipt_identity: String,
        /// Identity of the finalized replacement-route receipt.
        replacement_receipt_identity: String,
        /// Identity of the legacy process observation bound into the legacy receipt.
        legacy_output_identity: String,
        /// Identity of the direct Body-IR output bound into the replacement receipt.
        replacement_output_identity: String,
        /// The Oven receipt, build unit, and direct-rustc plan that authorized the legacy execution.
        ///
        /// Present so a green row names the build authority behind its legacy answer rather than leaving it an
        /// unattributed process result.
        legacy_authority: LegacyExecutionAuthority,
    },
    /// Two independent executions of the same source under a #1146 comparison profile disagreed.
    ///
    /// A genuine regression signal on the backend-selection axis itself, never a reason to prefer one route's
    /// answer over the other's.
    ShadowDiverged {
        /// Stable kind of comparison profile that ran, for consumers keyed on comparison capability (#1153).
        profile_kind: String,
        /// Content identity of the exact comparison profile instance both routes were bound to.
        profile_identity: String,
        /// Factual account of what each route observed.
        detail: String,
        /// Identity of the finalized legacy-route receipt.
        legacy_receipt_identity: String,
        /// Identity of the finalized replacement-route receipt.
        replacement_receipt_identity: String,
    },
    /// A route's selection or receipt evidence could not be produced or verified, so this row reports no
    /// comparison at all.
    ///
    /// Reached when the backend-selection API errors while declaring a selection, and when a comparison's routes
    /// executed but their receipts could not be finalized or verified. Both are defensive with today's fixed
    /// `FallbackPolicy::Refuse` inputs, and both must stay visible: the corpus must never treat unverifiable
    /// evidence as an available, green-eligible receipt, and must never answer it by re-running the row, which
    /// would report a different execution than the one that was observed.
    SelectionError { detail: String },
}

/// Serializable evidence produced by one [`IdentityConformancePlan`].
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct IdentityConformanceEvidence {
    /// Typed statement of the semantically valid binding/namespace/scope cells this row exercised.
    pub(crate) coverage_cells: Vec<IdentityCoverageCell>,
    /// Exact checked graph or native artifact subject this evidence describes.
    pub(crate) subject: IdentityConformanceSubject,
    /// Canonical equality/distinctness relations proven from checker-owned facts.
    pub(crate) checked_relations: Vec<String>,
    /// Declaration/import identities observed by declaration-level HIR.
    pub(crate) hir_consumers: Vec<String>,
    /// Canonical identities observed on typed Body-IR locals/call targets consumed by replacement.
    pub(crate) body_ir_consumers: Vec<String>,
    /// Exact `incan-v1` projections found in generated Rust or recovered from a pinned release artifact.
    pub(crate) legacy_projections: Vec<String>,
    /// Additional artifact observations, including generic specialization and non-Incan classification.
    pub(crate) artifact_observations: Vec<String>,
    /// Direct replacement output identity for source-local rows; absent only at an explicit unavailable boundary.
    pub(crate) replacement_output_identity: Option<String>,
    /// Owning issue for an unavailable replacement execution, when one exists.
    pub(crate) replacement_unavailable_issue: Option<u32>,
    /// Digest over every evidence field above, recomputable by summary consumers.
    pub(crate) evidence_identity: String,
}

/// The concrete subject whose compiler-carried identities one conformance row verifies.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum IdentityConformanceSubject {
    /// Multi-module checked source, bound to every module path and source input.
    SourceGraph { graph_identity: String },
    /// Pinned native fixture, separately binding its inputs, compiled content, and recovered observation.
    ReleaseArtifact {
        fixture_input_identity: String,
        artifact_content_identity: String,
        recovered_observation_identity: String,
    },
}

/// How the selected target entered the source scope exercised by one conformance cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IdentityBindingForm {
    Local,
    Import,
    Alias,
    ReExport,
}

impl IdentityBindingForm {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Import => "import",
            Self::Alias => "alias",
            Self::ReExport => "re_export",
        }
    }
}

/// RFC 120 namespace exercised by one semantically valid conformance cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IdentityNamespace {
    Lexical,
    Member,
    ModulePath,
}

impl IdentityNamespace {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Member => "member",
            Self::ModulePath => "module_path",
        }
    }
}

/// Source scope in which a binding or reference was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IdentityScope {
    Module,
    Owner,
    Function,
    Block,
}

impl IdentityScope {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Owner => "owner",
            Self::Function => "function",
            Self::Block => "block",
        }
    }
}

/// Carrier evidence for one semantically valid RFC 120 conformance cell.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct IdentityCoverageCell {
    pub(crate) binding: IdentityBindingForm,
    pub(crate) namespace: IdentityNamespace,
    pub(crate) scope: IdentityScope,
    /// Checker-owned identity observed for this cell. Always required.
    pub(crate) checked_identity: String,
    /// HIR carrier, required only for module-scope lexical and module-path bindings. HIR v0 does not contain
    /// owner-scope member declarations or executable references.
    pub(crate) hir_identity: Option<String>,
    /// Body-IR carrier, required for executable function/block references and absent at module scope.
    pub(crate) body_ir_identity: Option<String>,
    /// Generated `incan-v1` projection, required for linker-visible lexical/member source declarations and absent
    /// for module-path bindings, which do not themselves emit a callable/static symbol.
    pub(crate) emitted_projection: Option<String>,
}

/// Reason a direct-execution row that has not opted into the bounded #1146 profile stays non-green.
const NO_DECLARED_COMPARISON_REASON: &str = "this row executes the replacement backend directly but does not declare the bounded #1146 source-observable \
     comparison profile, so no legacy route was run to compare it against";

const CALLBACK_OBSERVATION_REASON: &str = "this row evaluated a compiler behavior callback but did not execute a selected backend, so it has no backend \
     execution receipt or two-route comparison";

/// The behavior result and #986 receipt produced by one direct replacement execution.
///
/// The two values are inseparable evidence: the value comes from the same selected, validated Body-IR execution
/// whose output identity is finalized into `receipt`.
struct ReplacementPlanEvidence {
    behavior_outcome: ComparisonOutcome,
    receipt: ReceiptRef,
}

/// Produce one #988 row's evidence, running the bounded #1146 comparison first when the row declares it.
///
/// A declared comparison that cannot run — no staged Oven capability, an unbuildable legacy program, a profile
/// the comparator refuses — degrades to direct-execution-only evidence carrying that concrete reason. The row
/// stays non-green either way; what changes is whether the reason is honest about *why*.
fn execute_replacement_plan(source: &str, plan: ReplacementExecutionPlan) -> ReplacementPlanEvidence {
    if !plan.shadow_comparison {
        return execute_direct_replacement_plan(source, plan, NO_DECLARED_COMPARISON_REASON.to_string());
    }
    match compare_replacement_plan(source, plan) {
        Ok(evidence) => evidence,
        Err(reason) => execute_direct_replacement_plan(source, plan, reason),
    }
}

/// Observe one row through both routes and bind the result to the receipts the comparison produced.
///
/// When both routes ran, this reports the comparison itself. When only the replacement route ran, it reports
/// *that* execution's own receipt and Body-IR evidence rather than re-running it: the comparison already
/// retained everything the row needs, and executing twice would make the reported receipt describe a different
/// run than the one the comparison observed.
///
/// `Err` is reserved for the one case where there is genuinely nothing to report — no staged capability, or a
/// comparison that retained no executed route at all — because the caller answers `Err` by running the row
/// directly. A route that executed but whose receipt could not be finalized must never take that path: it would
/// re-execute and publish a receipt describing a *different* run than the one that was observed. That case
/// reports [`ReceiptRef::SelectionError`] instead, which is non-green and names what happened.
fn compare_replacement_plan(source: &str, plan: ReplacementExecutionPlan) -> Result<ReplacementPlanEvidence, String> {
    let workspace = tempfile::tempdir()
        .map_err(|error| format!("the legacy comparison route could not create a workspace: {error}"))?;
    let profile = ShadowComparisonProfile::new(source, plan.function, (plan.arguments)());
    let capability = crate::shadow_capability::legacy_capability().map_err(|error| error.reason)?;
    let comparison = compare_source_observable(&profile, &capability, workspace.path());
    let (legacy, replacement) = match (&comparison.legacy, &comparison.replacement) {
        (Some(legacy), Some(replacement)) => (legacy, replacement),
        (_, Some(replacement)) => return retained_replacement_evidence(&comparison, replacement, plan),
        _ => return Err(unavailable_reason(&comparison)),
    };

    let behavior_outcome = comparison_behavior_outcome(&comparison, plan);
    let (legacy_receipt, replacement_receipt) = match (legacy.receipt(), replacement.receipt()) {
        (Ok(legacy_receipt), Ok(replacement_receipt)) => (legacy_receipt, replacement_receipt),
        (legacy_receipt, replacement_receipt) => {
            return Ok(unverifiable_comparison_evidence(
                behavior_outcome,
                [legacy_receipt.err(), replacement_receipt.err()],
            ));
        }
    };
    for receipt in [legacy_receipt, replacement_receipt] {
        if let Err(error) = receipt.verify_identity() {
            return Ok(unverifiable_comparison_evidence(
                behavior_outcome,
                [
                    Some(format!("a comparison receipt failed identity verification: {error}")),
                    None,
                ],
            ));
        }
    }
    let receipt = match &comparison.state {
        ShadowComparisonState::Matched {
            profile_kind,
            profile_identity,
            observable,
        } => ReceiptRef::ShadowMatched {
            profile_kind: profile_kind.clone(),
            profile_identity: profile_identity.clone(),
            observable: observable.clone(),
            legacy_receipt_identity: legacy_receipt.identity.clone(),
            replacement_receipt_identity: replacement_receipt.identity.clone(),
            legacy_output_identity: legacy_receipt.output_identity.clone(),
            replacement_output_identity: replacement_receipt.output_identity.clone(),
            legacy_authority: match comparison.legacy_authority.clone() {
                Some(authority) => authority,
                None => {
                    return Ok(unverifiable_comparison_evidence(
                        behavior_outcome,
                        [
                            Some("an executed legacy route recorded no Oven authority".to_string()),
                            None,
                        ],
                    ));
                }
            },
        },
        ShadowComparisonState::Diverged {
            profile_kind,
            profile_identity,
            detail,
        } => ReceiptRef::ShadowDiverged {
            profile_kind: profile_kind.clone(),
            profile_identity: profile_identity.clone(),
            detail: detail.clone(),
            legacy_receipt_identity: legacy_receipt.identity.clone(),
            replacement_receipt_identity: replacement_receipt.identity.clone(),
        },
        state => {
            return Ok(unverifiable_comparison_evidence(
                behavior_outcome,
                [
                    Some(format!(
                        "a comparison whose routes both executed must record agreement or divergence, got {state:?}"
                    )),
                    None,
                ],
            ));
        }
    };
    Ok(ReplacementPlanEvidence {
        behavior_outcome,
        receipt,
    })
}

/// Report the replacement execution a partially unavailable comparison already performed.
///
/// This is the corpus side of #1146's partial-evidence contract: the legacy route did not run, so no comparison
/// verdict exists, but the replacement route really executed and its receipt must be the one reported. Every
/// branch here reports what was observed; none hands the row back for a second execution, which would publish
/// evidence describing a different run than the one the comparison made.
fn retained_replacement_evidence(
    comparison: &ShadowComparison,
    replacement: &incan::backend::shadow::RouteEvidence,
    plan: ReplacementExecutionPlan,
) -> Result<ReplacementPlanEvidence, String> {
    let behavior_outcome = comparison_behavior_outcome(comparison, plan);
    // A retained route that has no usable receipt is reported as an explicit non-green error, never handed back
    // for re-execution: rerunning would replace observed evidence with a second, different run.
    let replacement_receipt = match replacement.receipt() {
        Ok(receipt) => receipt,
        Err(error) => return Ok(unverifiable_comparison_evidence(behavior_outcome, [Some(error), None])),
    };
    if let Err(error) = replacement_receipt.verify_identity() {
        return Ok(unverifiable_comparison_evidence(
            behavior_outcome,
            [
                Some(format!(
                    "a retained replacement receipt failed identity verification: {error}"
                )),
                None,
            ],
        ));
    }
    // A replacement route that stopped on a classified runtime failure executed, but produced no Body IR result
    // to project. Re-running the row would observe that same failure a second time and report *that* run, so the
    // observed outcome is kept and the missing projection is stated instead.
    let Some(execution) = comparison.replacement_execution.as_ref() else {
        return Ok(unverifiable_comparison_evidence(
            behavior_outcome,
            [
                Some(format!(
                    "the replacement route executed and observed {} rather than returning a value, so there is no \
                     Body-IR projection to report",
                    replacement.observation.observable.describe()
                )),
                None,
            ],
        ));
    };
    Ok(ReplacementPlanEvidence {
        behavior_outcome,
        receipt: ReceiptRef::ReplacementExecuted {
            selection_identity: replacement_receipt.selection.identity.clone(),
            receipt_identity: replacement_receipt.identity.clone(),
            output_identity: replacement_receipt.output_identity.clone(),
            body_snapshot: execution.body_snapshot.clone(),
            ownership_reads: execution.ownership_evidence(),
            runtime_requirements: execution.runtime_requirement_evidence(),
            task_lifecycle: execution.task_lifecycle_evidence(),
            comparison_reason: unavailable_reason(comparison),
        },
    })
}

/// Report a comparison whose routes executed but whose evidence cannot be verified.
///
/// This keeps the observed behavior outcome — the routes really did run — while refusing to publish a receipt
/// reference the corpus could mistake for verified parity. The result is always non-green, and the caller must
/// not respond by re-running the row: a second execution would describe a different run than the one observed.
fn unverifiable_comparison_evidence(
    behavior_outcome: ComparisonOutcome,
    problems: [Option<String>; 2],
) -> ReplacementPlanEvidence {
    let detail = problems.into_iter().flatten().collect::<Vec<_>>().join("; ");
    ReplacementPlanEvidence {
        behavior_outcome,
        receipt: ReceiptRef::SelectionError {
            detail: format!(
                "the comparison's routes executed but their evidence could not be verified, so this row reports \
                 no comparison: {detail}"
            ),
        },
    }
}

/// Read the recorded reason a comparison did not run.
fn unavailable_reason(comparison: &ShadowComparison) -> String {
    match &comparison.state {
        ShadowComparisonState::Unavailable { reason } => reason.clone(),
        state => format!("the comparison produced no receipts while recording {state:?}"),
    }
}

/// Confirm the compared row still produces the value its case documents.
///
/// The comparison proves the two routes agree; this proves they agree on the *expected* answer, so a shared
/// regression in both routes cannot pass as parity.
fn comparison_behavior_outcome(comparison: &ShadowComparison, plan: ReplacementExecutionPlan) -> ComparisonOutcome {
    let expected = (plan.expected)();
    match &comparison.replacement_execution {
        Some(execution) if execution.value == expected => ComparisonOutcome::Match,
        Some(execution) => ComparisonOutcome::Mismatch {
            detail: format!(
                "replacement `{}` returned {:?}, expected {:?}",
                plan.function, execution.value, expected
            ),
        },
        None => ComparisonOutcome::Mismatch {
            detail: format!(
                "replacement `{}` did not complete normally, so it cannot have produced {expected:?}",
                plan.function
            ),
        },
    }
}

/// Execute one #988 plan once, through #986 selection, and bind the observed Body-IR result to its receipt.
///
/// `comparison_reason` states why this row carries no source-observable comparison, so its non-green state is
/// explained rather than merely asserted.
fn execute_direct_replacement_plan(
    source: &str,
    plan: ReplacementExecutionPlan,
    comparison_reason: String,
) -> ReplacementPlanEvidence {
    let arguments = (plan.arguments)();
    let expected = (plan.expected)();
    let selection = select_backend(
        BackendKind::Replacement,
        true,
        true,
        digest_output(&[source]),
        FallbackPolicy::Refuse,
    );
    let body_ir = match lower_replacement_case(source) {
        Ok(body_ir) => body_ir,
        Err(detail) => return replacement_profile_refusal(&selection, detail),
    };
    let execution_plan = match prepare_free_function_execution(&body_ir, plan.function, &arguments) {
        Ok(execution_plan) => execution_plan,
        Err(error) => return replacement_profile_refusal(&selection, error.to_string()),
    };
    let executed = match resolve_execution(&selection, true) {
        Ok(backend) => backend,
        Err(error) => {
            return ReplacementPlanEvidence {
                behavior_outcome: ComparisonOutcome::Incompatible {
                    reason: format!("replacement corpus selection failure: {error}"),
                },
                receipt: receipt_ref_from_error(&error),
            };
        }
    };
    let execution = match execute_prevalidated_free_function(execution_plan) {
        Ok(execution) => execution,
        Err(error) => {
            return ReplacementPlanEvidence {
                behavior_outcome: ComparisonOutcome::Mismatch {
                    detail: format!("replacement corpus execution failure: {error}"),
                },
                receipt: ReceiptRef::SelectionError {
                    detail: format!("replacement corpus execution failure: {error}"),
                },
            };
        }
    };
    let behavior_outcome = if execution.value == expected {
        ComparisonOutcome::Match
    } else {
        ComparisonOutcome::Mismatch {
            detail: format!(
                "replacement `{}` returned {:?}, expected {:?}",
                plan.function, execution.value, expected
            ),
        }
    };
    let shadow_comparison = unavailable_shadow_comparison(selection.shadow_requested, &comparison_reason);
    if !matches!(shadow_comparison, ShadowComparisonState::Unavailable { .. }) {
        return ReplacementPlanEvidence {
            behavior_outcome,
            receipt: ReceiptRef::SelectionError {
                detail: format!(
                    "replacement corpus expected an unavailable shadow comparison, got {shadow_comparison:?}"
                ),
            },
        };
    }
    let receipt = match finalize_receipt(
        &selection,
        executed,
        execution.output_identity.clone(),
        shadow_comparison,
        DIAGNOSTIC_SCHEMA_VERSION,
    ) {
        Ok(receipt) => receipt,
        Err(error) => {
            return ReplacementPlanEvidence {
                behavior_outcome,
                receipt: receipt_ref_from_error(&error),
            };
        }
    };
    if let Err(error) = receipt.verify_identity() {
        return ReplacementPlanEvidence {
            behavior_outcome,
            receipt: receipt_ref_from_error(&error),
        };
    }
    let output_identity = execution.output_identity.clone();
    let body_snapshot = execution.body_snapshot.clone();
    let ownership_reads = execution.ownership_evidence();
    let runtime_requirements = execution.runtime_requirement_evidence();
    let task_lifecycle = execution.task_lifecycle_evidence();
    ReplacementPlanEvidence {
        behavior_outcome,
        receipt: ReceiptRef::ReplacementExecuted {
            selection_identity: receipt.selection.identity,
            receipt_identity: receipt.identity,
            output_identity,
            body_snapshot,
            ownership_reads,
            runtime_requirements,
            task_lifecycle,
            comparison_reason,
        },
    }
}

/// Refuse a replacement corpus profile through the canonical #986 selection boundary.
///
/// A Body-IR lowering or profile error must not execute directly, and must not silently turn into legacy behavior.
/// Resolving the declared selection with availability set to `false` preserves that refusal as receipt evidence.
fn replacement_profile_refusal(selection: &BackendSelection, detail: String) -> ReplacementPlanEvidence {
    match resolve_execution(selection, false) {
        Ok(executed) => ReplacementPlanEvidence {
            behavior_outcome: ComparisonOutcome::Incompatible {
                reason: format!("replacement corpus profile refusal: {detail}"),
            },
            receipt: ReceiptRef::SelectionError {
                detail: format!(
                    "replacement corpus profile refusal was incorrectly resolved to `{executed:?}`: {detail}"
                ),
            },
        },
        Err(error) => ReplacementPlanEvidence {
            behavior_outcome: ComparisonOutcome::Incompatible {
                reason: format!("replacement corpus profile refusal: {detail}"),
            },
            receipt: ReceiptRef::SelectionError {
                detail: format!("replacement corpus profile refusal: {detail}; selection refusal: {error}"),
            },
        },
    }
}

/// Fold a backend-selection API error into a [`ReceiptRef`] rather than panicking or silently treating it as
/// available.
fn receipt_ref_from_error(error: &BackendSelectionError) -> ReceiptRef {
    ReceiptRef::SelectionError {
        detail: error.to_string(),
    }
}

/// The outcome of actually running a case's [`ParityCase::evaluate`] function.
///
/// [`ComparisonOutcome::Match`] is the only state that counts as green. Every other variant is an explicit
/// non-green state so that a missing, skipped, or incompatible comparison is visible in the emitted summary
/// instead of being silently rolled up as passing parity.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum ComparisonOutcome {
    /// The observed behavior matched the case's documented expectation.
    Match,
    /// The observed behavior diverged from the case's documented expectation — a real regression signal.
    Mismatch { detail: String },
    /// The comparison was not run for a stated reason (for example, the required backend path does not exist yet).
    ///
    /// No seed case produces this yet — every seed `evaluate` function can run today. Reserved for a future source
    /// profile whose required execution boundary is unavailable, so that case can report "not run" honestly instead
    /// of being omitted from the corpus entirely.
    #[allow(dead_code)]
    Skipped { reason: String },
    /// The two sides of the comparison are not comparable (for example, mismatched build profiles).
    Incompatible { reason: String },
}

impl ComparisonOutcome {
    /// Whether this outcome counts as green for the behavior-verification axis.
    ///
    /// This only covers whether the case's own `evaluate` function observed the expected behavior; it says
    /// nothing about receipt-aware reference/replacement comparison, which is tracked separately via
    /// [`ReceiptRef`] and folded into [`CaseReport::overall_state`].
    pub(crate) fn is_green(&self) -> bool {
        matches!(self, ComparisonOutcome::Match)
    }
}

/// Recompute the non-execution evidence identity for one callback observation.
pub(crate) fn behavior_observation_identity(
    case_id: &str,
    evidence: &str,
    source: &str,
    outcome: &ComparisonOutcome,
) -> String {
    let (state, detail) = match outcome {
        ComparisonOutcome::Match => ("match", ""),
        ComparisonOutcome::Mismatch { detail } => ("mismatch", detail.as_str()),
        ComparisonOutcome::Skipped { reason } => ("skipped", reason.as_str()),
        ComparisonOutcome::Incompatible { reason } => ("incompatible", reason.as_str()),
    };
    digest_output(&[case_id, evidence, source, state, detail])
}

/// One executable parity corpus case.
///
/// `evaluate` is a function pointer rather than a pre-computed result: the corpus must be executable, not just
/// metadata, so each case proves its own claim by actually lexing/parsing/typechecking/generating against the
/// current compiler at test-run time.
#[derive(Clone)]
pub(crate) struct ParityCase {
    /// Stable case identity. Once assigned, an ID must never be reused for a different case — renumbering breaks
    /// the "stable case ID" contract #987 and #655 both depend on. Delete and re-add rather than renumber.
    pub(crate) id: &'static str,
    /// Short human-readable title for CI summaries and review.
    pub(crate) title: &'static str,
    pub(crate) category: BehaviorCategory,
    pub(crate) lane: EvidenceLane,
    /// Repo-relative pointer to the primary evidence for this case (fixture path, test name, or doc anchor).
    pub(crate) evidence: &'static str,
    pub(crate) disposition: Disposition,
    /// The case's own Incan source. Direct backend plans bind it into their selection; callbacks and first-class
    /// conformance plans report their own typed observations without pretending this descriptive field was output.
    /// Red-state fixtures that never reach [`evaluate_case`] may use a trivial placeholder since it is unused.
    pub(crate) source: &'static str,
    /// Executes the legacy/non-replacement comparison for this case. Direct replacement plans instead derive their
    /// outcome from the single selected execution that also produces their receipt. Must not panic on an expected
    /// non-green result — return [`ComparisonOutcome::Mismatch`]/`Skipped`/`Incompatible` instead.
    pub(crate) evaluate: Option<fn() -> ComparisonOutcome>,
    /// Optional RFC 120 identity-conformance evaluation. Mutually exclusive with `evaluate` and
    /// `replacement_execution`; validation rejects a row that tries to claim more than one execution shape.
    pub(crate) identity_conformance: Option<IdentityConformancePlan>,
    /// Optional direct replacement execution that owns this case's #988 proof bundle.
    pub(crate) replacement_execution: Option<ReplacementExecutionPlan>,
}

/// A parameterized direct replacement execution bound to one stable #987 corpus case.
///
/// This intentionally names a function plus concrete values rather than a generated-Rust entrypoint. The source is
/// typechecked and lowered to Body IR in-process, then the replacement executor consumes that Body IR directly.
#[derive(Clone, Copy)]
pub(crate) struct ReplacementExecutionPlan {
    /// Source-level free function executed by the replacement profile.
    pub(crate) function: &'static str,
    /// Concrete typed values passed to `function` in source parameter order.
    pub(crate) arguments: fn() -> Vec<ReplacementValue>,
    /// The source-observable value the direct execution must produce.
    pub(crate) expected: fn() -> ReplacementValue,
    /// Whether this row also runs the bounded #1146 source-observable comparison against the legacy backend.
    ///
    /// Opt-in per row rather than implied by the lane: a row without a proven two-route comparison must stay
    /// non-green, and silently comparing every direct-execution row would make that distinction invisible.
    pub(crate) shadow_comparison: bool,
}

/// One source module in a compiler-checked identity graph.
#[derive(Clone, Copy)]
pub(crate) struct IdentitySourceModule {
    /// Stable flattened dependency key used by the current frontend import API.
    pub(crate) name: &'static str,
    /// Canonical source module path used to mint declaration identities.
    pub(crate) path: &'static [&'static str],
    /// Exact module source.
    pub(crate) source: &'static str,
    /// Other entries in the plan that this module may import.
    pub(crate) dependencies: &'static [&'static str],
}

/// Checked carrier state made available to an identity-plan verifier.
pub(crate) struct CheckedIdentityModule {
    pub(crate) name: &'static str,
    pub(crate) path: Vec<String>,
    pub(crate) source: &'static str,
    pub(crate) type_info: TypeCheckInfo,
    pub(crate) hir: HirModule,
    pub(crate) body_ir: BodyIrModule,
    pub(crate) emitted_rust: String,
}

/// A complete multi-module compilation observed before target-specific execution.
pub(crate) struct CheckedIdentityGraph {
    pub(crate) modules: Vec<CheckedIdentityModule>,
    pub(crate) source_graph_identity: String,
}

impl CheckedIdentityGraph {
    /// Resolve a checked module by its stable graph name.
    pub(crate) fn module(&self, name: &str) -> Result<&CheckedIdentityModule, String> {
        self.modules
            .iter()
            .find(|module| module.name == name)
            .ok_or_else(|| format!("identity graph has no module `{name}`"))
    }

    /// Read the outermost checker-owned identity recorded at or inside the selected source fragment.
    ///
    /// Conformance rows use full call expressions as stable source anchors even though the checker records the
    /// callable reference (and, for member calls, its receiver) rather than assigning an identity to the call node.
    /// Selecting the widest checked reference inside that anchor preserves the compiler-owned fact without
    /// reconstructing an identity from source spelling.
    pub(crate) fn resolved_identity(
        &self,
        module: &str,
        needle: &str,
        occurrence: usize,
    ) -> Result<CanonicalSymbolId, String> {
        let module = self.module(module)?;
        let span = nth_source_span(module.source, needle, occurrence)?;
        if let Some(identity) = module.type_info.resolved_identity(span) {
            return Ok(identity.clone());
        }

        let candidates = module
            .type_info
            .references
            .resolved_identities
            .iter()
            .filter(|((start, end), _)| *start >= span.start && *end <= span.end)
            .map(|(&(start, end), identity)| (end.saturating_sub(start), start, end, identity));
        let Some((widest, _, _, _)) = candidates.clone().max_by_key(|candidate| candidate.0) else {
            return Err(format!(
                "`{}` has no checked identity for occurrence {occurrence} of `{needle}`",
                module.name
            ));
        };
        let widest_candidates = candidates.filter(|candidate| candidate.0 == widest).collect::<Vec<_>>();
        if widest_candidates.len() != 1 {
            return Err(format!(
                "`{}` has ambiguous outermost checked identities inside occurrence {occurrence} of `{needle}`: {:?}",
                module.name,
                widest_candidates
                    .iter()
                    .map(|(_, start, end, identity)| (*start, *end, identity.render_compact()))
                    .collect::<Vec<_>>()
            ));
        }
        Ok(widest_candidates[0].3.clone())
    }

    /// Resolve one unique local declaration identity by semantic name/kind/namespace.
    pub(crate) fn declaration_identity(
        &self,
        module: &str,
        declaration_name: &str,
        kind: SemanticSourceTargetKind,
        namespace: SymbolNamespace,
    ) -> Result<CanonicalSymbolId, String> {
        let module = self.module(module)?;
        let mut matches = module
            .type_info
            .declarations
            .declaration_identities
            .values()
            .chain(module.type_info.declarations.member_declaration_identities.values())
            .filter(|identity| {
                identity.declaration_name == declaration_name
                    && identity.kind == kind
                    && identity.namespace == namespace
            });
        let identity = matches
            .next()
            .cloned()
            .ok_or_else(|| format!("`{}` has no canonical `{declaration_name}` declaration", module.name))?;
        if matches.next().is_some() {
            return Err(format!(
                "`{}` has multiple canonical `{declaration_name}` declarations; the plan must select by span",
                module.name
            ));
        }
        Ok(identity)
    }

    /// Select one declaration whose checker-owned declaration span contains an exact authored source anchor.
    pub(crate) fn declaration_identity_at_source_anchor(
        &self,
        module: &str,
        anchor: &str,
        occurrence: usize,
        kind: SemanticSourceTargetKind,
        namespace: SymbolNamespace,
    ) -> Result<CanonicalSymbolId, String> {
        let module = self.module(module)?;
        let anchor_span = nth_source_span(module.source, anchor, occurrence)?;
        let mut identities = module
            .type_info
            .declarations
            .declaration_identities
            .values()
            .chain(module.type_info.declarations.member_declaration_identities.values())
            .filter(|identity| {
                identity.kind == kind
                    && identity.namespace == namespace
                    && identity.declaration_span.start <= anchor_span.start
                    && identity.declaration_span.end >= anchor_span.end
            })
            .cloned();
        let identity = identities.next().ok_or_else(|| {
            format!(
                "`{}` has no {kind:?}/{namespace:?} declaration containing occurrence {occurrence} of `{anchor}`",
                module.name
            )
        })?;
        if identities.next().is_some() {
            return Err(format!(
                "`{}` has multiple {kind:?}/{namespace:?} declarations containing occurrence {occurrence} of `{anchor}`",
                module.name
            ));
        }
        Ok(identity)
    }

    /// Read the declaration-level HIR identity carried by one unique named binding.
    pub(crate) fn hir_identity(&self, module: &str, binding_name: &str) -> Result<CanonicalSymbolId, String> {
        let module = self.module(module)?;
        let mut matches = module
            .hir
            .declarations
            .iter()
            .filter(|declaration| declaration.name.as_deref() == Some(binding_name));
        let declaration = matches
            .next()
            .ok_or_else(|| format!("`{}` has no HIR binding `{binding_name}`", module.name))?;
        if matches.next().is_some() {
            return Err(format!(
                "`{}` has multiple HIR bindings `{binding_name}`; the plan must select an unambiguous binding",
                module.name
            ));
        }
        declaration.canonical.clone().ok_or_else(|| {
            format!(
                "HIR binding `{binding_name}` in `{}` has no canonical identity",
                module.name
            )
        })
    }

    /// Return every declaration identity carried by the replacement-facing statements in one lowered body.
    pub(crate) fn body_consumer_identities(
        &self,
        module: &str,
        body_name: &str,
    ) -> Result<Vec<CanonicalSymbolId>, String> {
        let module = self.module(module)?;
        let body = module
            .body_ir
            .bodies
            .iter()
            .find(|body| body.name == body_name)
            .ok_or_else(|| format!("`{}` has no Body-IR body `{body_name}`", module.name))?;
        let mut identities = body
            .locals
            .iter()
            .filter_map(|local| local.identity.clone())
            .collect::<Vec<_>>();
        collect_statement_consumer_identities(&body.block.stmts, &mut identities);
        Ok(identities)
    }

    /// Return all canonical identities carried by same-spelled locals in one lowered body.
    pub(crate) fn body_local_identities(
        &self,
        module: &str,
        body_name: &str,
        local_name: &str,
    ) -> Result<Vec<CanonicalSymbolId>, String> {
        let module = self.module(module)?;
        let body = module
            .body_ir
            .bodies
            .iter()
            .find(|body| body.name == body_name)
            .ok_or_else(|| format!("`{}` has no Body-IR body `{body_name}`", module.name))?;
        body.locals
            .iter()
            .filter(|local| local.name.as_deref() == Some(local_name))
            .map(|local| {
                local
                    .identity
                    .clone()
                    .ok_or_else(|| format!("Body-IR local `{local_name}` has no canonical identity"))
            })
            .collect()
    }

    /// Require one exact canonical projection in the selected module's generated Rust.
    pub(crate) fn require_emitted_projection(
        &self,
        module: &str,
        identity: &CanonicalSymbolId,
    ) -> Result<String, String> {
        let module = self.module(module)?;
        let projection = encode_incan_symbol_identity(identity);
        let matched = exact_rust_identifier(&module.emitted_rust, &projection).map_err(|error| {
            let available = module
                .emitted_rust
                .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
                .filter(|token| token.starts_with("__incan_v1_"))
                .take(8)
                .collect::<Vec<_>>();
            format!(
                "generated Rust for `{}` could not prove canonical projection `{projection}`: {error}; available projections: {available:?}",
                module.name,
            )
        })?;
        Ok(format!("{} identifier {matched}", module.name))
    }
}

/// Return one exact Rust identifier token, excluding substrings, comments, and string literals.
pub(crate) fn exact_rust_identifier(source: &str, expected: &str) -> Result<String, String> {
    fn find_in_stream(stream: TokenStream, expected: &str) -> Option<String> {
        for token in stream {
            match token {
                TokenTree::Ident(identifier) => {
                    let identifier = identifier.to_string();
                    if identifier == expected {
                        return Some(identifier);
                    }
                }
                TokenTree::Group(group) => {
                    if let Some(identifier) = find_in_stream(group.stream(), expected) {
                        return Some(identifier);
                    }
                }
                TokenTree::Punct(_) | TokenTree::Literal(_) => {}
            }
        }
        None
    }

    let tokens = TokenStream::from_str(source).map_err(|error| format!("Rust tokenization failed: {error}"))?;
    find_in_stream(tokens, expected).ok_or_else(|| format!("no exact Rust identifier token `{expected}` was emitted"))
}

/// Successful assertions supplied by a source-graph or artifact verifier.
#[derive(Debug, Clone)]
pub(crate) struct IdentityAssertions {
    pub(crate) coverage_cells: Vec<IdentityCoverageCell>,
    pub(crate) checked_relations: Vec<String>,
    pub(crate) hir_consumers: Vec<String>,
    pub(crate) body_ir_consumers: Vec<String>,
    pub(crate) legacy_projections: Vec<String>,
    pub(crate) artifact_observations: Vec<String>,
}

/// Verified carriers plus the actual native-artifact subject that produced them.
#[derive(Debug, Clone)]
pub(crate) struct ReleaseArtifactAssertions {
    pub(crate) assertions: IdentityAssertions,
    pub(crate) fixture_input_identity: String,
    pub(crate) artifact_content_identity: String,
    pub(crate) recovered_observation_identity: String,
}

/// Replacement-side state required by one source-graph identity row.
#[derive(Clone, Copy)]
pub(crate) enum IdentityReplacementPlan {
    /// Execute one source-local entrypoint after the verifier has proven its carriers.
    Direct {
        module: &'static str,
        function: &'static str,
        arguments: fn() -> Vec<ReplacementValue>,
        expected: fn() -> ReplacementValue,
    },
    /// Execute every declared entrypoint against a replacement graph spanning the checked modules.
    ///
    /// This is the cross-module form of [`IdentityReplacementPlan::Direct`]. A row uses it when its entrypoints
    /// call across the module boundary rather than staying inside one module's Body IR, which is exactly the
    /// surface #1260 and #1261 made executable: the callee's identity is resolved through the graph rather than
    /// through the caller module's own bodies.
    Graph {
        root_module: &'static str,
        entrypoints: &'static [IdentityGraphEntrypoint],
        deferred: &'static [IdentityGraphDeferral],
    },
    /// The checked graph is executable through its compiler carriers, but replacement package execution is not.
    Unavailable { owning_issue: u32, reason: &'static str },
}

/// One entrypoint a graph replacement plan executes, with the integer result the checked route must produce.
///
/// Entrypoints are scalar and argument-free on purpose. The claim under test is that a call reaching another
/// module resolves to one declaration and returns that declaration's result; parameter passing and richer value
/// shapes are already covered by the single-module rows and would only add noise here.
#[derive(Clone, Copy)]
pub(crate) struct IdentityGraphEntrypoint {
    /// Free function in the plan's root module.
    pub(crate) function: &'static str,
    /// Value the entrypoint must return.
    pub(crate) expected: i64,
}

/// One entrypoint a graph replacement plan cannot execute yet, bound to the issue that owns closing the gap.
///
/// The runner requires the refusal to be real: it prepares the entrypoint and fails the row if preparation
/// succeeds. A gap that closes without anyone noticing would otherwise leave a stale deferral behind, which is the
/// same "unavailable counted as fine" failure the corpus exists to prevent.
#[derive(Clone, Copy)]
pub(crate) struct IdentityGraphDeferral {
    /// Free function in the plan's root module.
    pub(crate) function: &'static str,
    /// Issue that owns making this entrypoint executable.
    pub(crate) owning_issue: u32,
}

/// Data-driven multi-module identity-conformance evaluation.
#[derive(Clone, Copy)]
pub(crate) struct SourceIdentityConformancePlan {
    pub(crate) modules: &'static [IdentitySourceModule],
    pub(crate) root_module: &'static str,
    pub(crate) verify: fn(&CheckedIdentityGraph) -> Result<IdentityAssertions, String>,
    pub(crate) replacement: IdentityReplacementPlan,
    pub(crate) comparison_reason: &'static str,
}

/// First-class RFC 120 conformance work owned by one corpus row.
#[derive(Clone, Copy)]
pub(crate) enum IdentityConformancePlan {
    /// Compile a checked source graph through HIR, Body IR, legacy projection, and optional direct replacement.
    SourceGraph(SourceIdentityConformancePlan),
    /// Execute the pinned native artifact decoder and return its exact recovered evidence.
    ReleaseArtifact {
        verify: fn() -> Result<ReleaseArtifactAssertions, String>,
        comparison_reason: &'static str,
    },
}

/// Build the complete checked graph that a source identity row verifies.
fn check_identity_source_graph(plan: SourceIdentityConformancePlan) -> Result<CheckedIdentityGraph, String> {
    if plan.modules.is_empty() {
        return Err("identity source graph must contain at least one module".to_string());
    }
    let parsed = plan
        .modules
        .iter()
        .map(|module| {
            let tokens = lexer::lex(module.source)
                .map_err(|errors| format!("identity module `{}` lex failure: {errors:?}", module.name))?;
            let program = parser::parse(&tokens)
                .map_err(|errors| format!("identity module `{}` parse failure: {errors:?}", module.name))?;
            let source_path = format!("{}.incn", module.path.join("/"));
            let program = apply_body_ir_input_contract(program, std::path::Path::new(&source_path))
                .map_err(|errors| format!("identity module `{}` input-contract failure: {errors:?}", module.name))?;
            Ok((*module, program))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let root_index = parsed
        .iter()
        .position(|(module, _)| module.name == plan.root_module)
        .ok_or_else(|| format!("identity graph has no declared root `{}`", plan.root_module))?;

    let mut checked_modules = Vec::with_capacity(parsed.len());
    for (module, program) in &parsed {
        let dependencies = module
            .dependencies
            .iter()
            .map(|dependency_name| {
                parsed
                    .iter()
                    .find(|(candidate, _)| candidate.name == *dependency_name)
                    .map(|(candidate, dependency)| (candidate.name, dependency))
                    .ok_or_else(|| {
                        format!(
                            "identity module `{}` names unknown dependency `{dependency_name}`",
                            module.name
                        )
                    })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let module_path = module
            .path
            .iter()
            .map(|segment| (*segment).to_string())
            .collect::<Vec<_>>();
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        for dependency_name in module.dependencies {
            let dependency = plan
                .modules
                .iter()
                .find(|candidate| candidate.name == *dependency_name)
                .ok_or_else(|| format!("identity graph lost dependency `{dependency_name}`"))?;
            checker.register_dependency_module_path_segments(
                dependency.name,
                dependency.path.iter().map(|segment| (*segment).to_string()).collect(),
            );
        }
        checker
            .check_with_imports(program, &dependencies)
            .map_err(|errors| format!("identity module `{}` typecheck failure: {errors:?}", module.name))?;
        let type_info = checker.type_info().clone();
        checked_modules.push(CheckedIdentityModule {
            name: module.name,
            path: module_path.clone(),
            source: module.source,
            hir: build_hir_v0(program, &module_path, &type_info),
            body_ir: build_body_ir_module_v0(program, &module_path, &type_info),
            type_info,
            emitted_rust: String::new(),
        });
    }

    let (root_spec, root_program) = &parsed[root_index];
    let mut codegen = IrCodegen::new();
    codegen.set_root_source_module_name(Some(root_spec.path.join(".")));
    codegen.set_externally_reachable_items(
        root_program
            .declarations
            .iter()
            .filter_map(|declaration| match &declaration.node {
                ast::Declaration::Function(function) => Some(function.name.clone()),
                _ => None,
            })
            .collect::<HashSet<_>>(),
    );
    codegen.set_externally_reachable_items_by_module(
        parsed
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != root_index)
            .map(|(_, (module, program))| {
                let path = module
                    .path
                    .iter()
                    .map(|segment| (*segment).to_string())
                    .collect::<Vec<_>>();
                let functions = program
                    .declarations
                    .iter()
                    .filter_map(|declaration| match &declaration.node {
                        ast::Declaration::Function(function) => Some(function.name.clone()),
                        _ => None,
                    })
                    .collect::<HashSet<_>>();
                (path, functions)
            })
            .collect::<HashMap<_, _>>(),
    );
    let mut dependency_paths = Vec::new();
    for (module, program) in &parsed {
        if module.name == root_spec.name {
            continue;
        }
        let path = module
            .path
            .iter()
            .map(|segment| (*segment).to_string())
            .collect::<Vec<_>>();
        codegen.add_module_with_path_segments(module.name, program, path.clone());
        dependency_paths.push(path);
    }
    let (root_rust, dependency_rust) = codegen
        .try_generate_multi_file_nested(root_program, &dependency_paths)
        .map_err(|error| format!("identity source graph legacy emission failed: {error:?}"))?;
    for module in &mut checked_modules {
        module.emitted_rust = if module.name == root_spec.name {
            root_rust.clone()
        } else {
            dependency_rust
                .get(&module.path)
                .cloned()
                .ok_or_else(|| format!("legacy emission omitted identity module `{}`", module.name))?
        };
    }

    let mut graph_parts = Vec::new();
    for module in plan.modules {
        graph_parts.push(module.name);
        graph_parts.extend(module.path.iter().copied());
        graph_parts.push(module.source);
    }
    Ok(CheckedIdentityGraph {
        modules: checked_modules,
        source_graph_identity: digest_output(&graph_parts),
    })
}

/// Find one source span without letting a verifier silently select the wrong occurrence.
fn nth_source_span(source: &str, needle: &str, occurrence: usize) -> Result<ast::Span, String> {
    source
        .match_indices(needle)
        .nth(occurrence)
        .map(|(start, value)| ast::Span::new(start, start + value.len()))
        .ok_or_else(|| format!("source has no occurrence {occurrence} of `{needle}`"))
}

fn collect_statement_consumer_identities(statements: &[Statement], identities: &mut Vec<CanonicalSymbolId>) {
    for statement in statements {
        match &statement.kind {
            StatementKind::Assign { rvalue, .. } => match rvalue {
                Rvalue::Aggregate(incan_semantics_core::body_ir::AggregateKind::Constructor(target), _) => {
                    identities.extend(target.canonical.iter().cloned());
                }
                Rvalue::ValueEnumVariant(target) => {
                    identities.push(target.enum_canonical.clone());
                    identities.push(target.variant_canonical.clone());
                }
                Rvalue::FieldlessEnumVariant(target) => {
                    identities.push(target.enum_canonical.clone());
                    identities.push(target.variant_canonical.clone());
                }
                Rvalue::Match { arms, .. } => {
                    for arm in arms {
                        collect_statement_consumer_identities(&arm.guard_stmts, identities);
                        collect_statement_consumer_identities(&arm.body_stmts, identities);
                    }
                }
                _ => {}
            },
            StatementKind::Call { callee, .. } => match callee {
                Callee::Function(CallableTarget::Named(target)) => identities.extend(target.canonical.iter().cloned()),
                Callee::Method(target) => identities.extend(target.canonical.iter().cloned()),
                Callee::ProviderOperation(plan) => identities.push(plan.operation.clone()),
                Callee::Function(CallableTarget::Local(_)) | Callee::Helper(_) => {}
            },
            StatementKind::If {
                then_block, else_block, ..
            } => {
                collect_statement_consumer_identities(&then_block.stmts, identities);
                if let Some(else_block) = else_block {
                    collect_statement_consumer_identities(&else_block.stmts, identities);
                }
            }
            StatementKind::Loop { body } => collect_statement_consumer_identities(&body.stmts, identities),
            StatementKind::Race { arms, .. } => {
                for arm in arms {
                    collect_statement_consumer_identities(&arm.body.stmts, identities);
                }
            }
            _ => {}
        }
    }
}

/// Typecheck and lower source into the Body IR that replacement selection validates before execution.
///
/// The corpus is a caller of [`build_body_ir_module_v0`] like any other, so it owes that boundary the same
/// desugared, feature-projected program the CLI path owes it (#1166). Applying the contract here rather than
/// duplicating its two steps is the point: a corpus that lowered raw parse output would be measuring a program the
/// real pipeline never produces, and would go green on a divergence instead of surfacing it.
fn lower_replacement_case(source: &str) -> Result<BodyIrModule, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("replacement corpus lex failure: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("replacement corpus parse failure: {errors:?}"))?;
    let program = apply_body_ir_input_contract(program, std::path::Path::new("parity_987_replacement.incn"))
        .map_err(|errors| format!("replacement corpus input-contract failure: {errors:?}"))?;
    let module_path = vec!["parity_987_replacement".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("replacement corpus typecheck failure: {errors:?}"))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// The behavior, receipt, and inspectable identity evidence produced by one conformance plan.
struct IdentityPlanEvidence {
    behavior_outcome: ComparisonOutcome,
    receipt: ReceiptRef,
    evidence: Option<IdentityConformanceEvidence>,
}

/// Execute one first-class identity conformance plan through every carrier it declares.
fn execute_identity_conformance_plan(plan: IdentityConformancePlan) -> IdentityPlanEvidence {
    match try_execute_identity_conformance_plan(plan) {
        Ok((behavior_outcome, receipt, evidence)) => IdentityPlanEvidence {
            behavior_outcome,
            receipt,
            evidence: Some(evidence),
        },
        Err(detail) => IdentityPlanEvidence {
            behavior_outcome: ComparisonOutcome::Mismatch {
                detail: format!("identity conformance failed: {detail}"),
            },
            receipt: ReceiptRef::SelectionError { detail },
            evidence: None,
        },
    }
}

fn try_execute_identity_conformance_plan(
    plan: IdentityConformancePlan,
) -> Result<(ComparisonOutcome, ReceiptRef, IdentityConformanceEvidence), String> {
    let (
        assertions,
        subject,
        replacement_output_identity,
        replacement_receipt_identity,
        replacement_unavailable_issue,
        behavior_outcome,
        reason,
    ) = match plan {
        IdentityConformancePlan::SourceGraph(plan) => {
            let graph = check_identity_source_graph(plan)?;
            let assertions = (plan.verify)(&graph)?;
            validate_identity_assertions(&assertions, false)?;
            let (replacement_output_identity, replacement_receipt_identity, unavailable_issue, behavior_outcome) =
                match plan.replacement {
                    IdentityReplacementPlan::Direct {
                        module,
                        function,
                        arguments,
                        expected,
                    } => {
                        let arguments = arguments();
                        let module = graph.module(module)?;
                        let selection = select_backend(
                            BackendKind::Replacement,
                            true,
                            true,
                            graph.source_graph_identity.clone(),
                            FallbackPolicy::Refuse,
                        );
                        let execution_plan = prepare_free_function_execution(&module.body_ir, function, &arguments)
                            .map_err(|error| format!("identity replacement execution refused: {error}"))?;
                        let executed_backend = resolve_execution(&selection, true)
                            .map_err(|error| format!("identity replacement selection failed: {error}"))?;
                        let execution = execute_prevalidated_free_function(execution_plan)
                            .map_err(|error| format!("identity replacement execution failed: {error}"))?;
                        let expected = expected();
                        let shadow = unavailable_shadow_comparison(selection.shadow_requested, plan.comparison_reason);
                        let receipt = finalize_receipt(
                            &selection,
                            executed_backend,
                            execution.output_identity.clone(),
                            shadow,
                            DIAGNOSTIC_SCHEMA_VERSION,
                        )
                        .map_err(|error| format!("identity replacement receipt failed: {error}"))?;
                        receipt
                            .verify_identity()
                            .map_err(|error| format!("identity replacement receipt verification failed: {error}"))?;
                        let behavior_outcome = if execution.value == expected {
                            ComparisonOutcome::Match
                        } else {
                            ComparisonOutcome::Mismatch {
                                detail: format!(
                                    "identity replacement `{function}` returned {:?}, expected {expected:?}",
                                    execution.value
                                ),
                            }
                        };
                        (
                            Some(execution.output_identity),
                            Some(receipt.identity),
                            None,
                            behavior_outcome,
                        )
                    }
                    IdentityReplacementPlan::Graph {
                        root_module,
                        entrypoints,
                        deferred,
                    } => {
                        if entrypoints.is_empty() {
                            return Err("a graph replacement plan must declare at least one entrypoint".to_string());
                        }
                        let root = graph.module(root_module)?;
                        let selection = select_backend(
                            BackendKind::Replacement,
                            true,
                            true,
                            graph.source_graph_identity.clone(),
                            FallbackPolicy::Refuse,
                        );
                        let executed_backend = resolve_execution(&selection, true)
                            .map_err(|error| format!("identity replacement selection failed: {error}"))?;

                        // Each entrypoint is executed against a graph rebuilt over the same modules. Building it per
                        // entrypoint rather than once keeps every execution independent, so one row cannot observe
                        // state another left behind -- the property a cross-module claim most needs to be free of.
                        let mut per_entrypoint_identities: Vec<String> = Vec::new();
                        let mut behavior_outcome = ComparisonOutcome::Match;
                        for entrypoint in entrypoints {
                            let reachable = graph
                                .modules
                                .iter()
                                .filter(|module| module.name != root_module)
                                .map(|module| &module.body_ir);
                            let execution_graph = ReplacementExecutionGraph::new(&root.body_ir, reachable)
                                .map_err(|error| format!("identity replacement graph assembly failed: {error}"))?;
                            let execution_plan = prepare_free_function_execution_in_graph(
                                execution_graph,
                                entrypoint.function,
                                &[],
                                None,
                            )
                            .map_err(|error| {
                                format!(
                                    "identity replacement execution refused `{}`: {error}",
                                    entrypoint.function
                                )
                            })?;
                            let execution = execute_prevalidated_free_function(execution_plan).map_err(|error| {
                                format!(
                                    "identity replacement execution failed `{}`: {error}",
                                    entrypoint.function
                                )
                            })?;
                            if execution.value != ReplacementValue::Int(entrypoint.expected) {
                                behavior_outcome = ComparisonOutcome::Mismatch {
                                    detail: format!(
                                        "identity replacement `{}` returned {:?}, expected Int({})",
                                        entrypoint.function, execution.value, entrypoint.expected
                                    ),
                                };
                                break;
                            }
                            per_entrypoint_identities.push(execution.output_identity);
                        }

                        // Prove every deferral is still real. A cell that starts executing must fail this row so it
                        // gets promoted, rather than sitting in the deferred list claiming a gap that closed.
                        for deferral in deferred {
                            if deferral.owning_issue == 0 {
                                return Err(format!(
                                    "deferred entrypoint `{}` must name the issue that owns closing the gap",
                                    deferral.function
                                ));
                            }
                            let reachable = graph
                                .modules
                                .iter()
                                .filter(|module| module.name != root_module)
                                .map(|module| &module.body_ir);
                            let execution_graph = ReplacementExecutionGraph::new(&root.body_ir, reachable)
                                .map_err(|error| format!("identity replacement graph assembly failed: {error}"))?;
                            if prepare_free_function_execution_in_graph(execution_graph, deferral.function, &[], None)
                                .is_ok()
                            {
                                return Err(format!(
                                    "deferred entrypoint `{}` now prepares successfully; #{} closed, so promote it \
                                     into the executed entrypoints",
                                    deferral.function, deferral.owning_issue
                                ));
                            }
                        }

                        // The row's output identity covers every entrypoint it executed, so dropping one changes it.
                        // A single entrypoint's identity would let the others silently disappear.
                        let borrowed: Vec<&str> = per_entrypoint_identities.iter().map(String::as_str).collect();
                        let output_identity = digest_output(&borrowed);
                        let shadow = unavailable_shadow_comparison(selection.shadow_requested, plan.comparison_reason);
                        let receipt = finalize_receipt(
                            &selection,
                            executed_backend,
                            output_identity.clone(),
                            shadow,
                            DIAGNOSTIC_SCHEMA_VERSION,
                        )
                        .map_err(|error| format!("identity replacement receipt failed: {error}"))?;
                        receipt
                            .verify_identity()
                            .map_err(|error| format!("identity replacement receipt verification failed: {error}"))?;
                        (Some(output_identity), Some(receipt.identity), None, behavior_outcome)
                    }
                    IdentityReplacementPlan::Unavailable { owning_issue, reason } => {
                        if owning_issue == 0 || !reason.contains(&format!("#{owning_issue}")) {
                            return Err(
                                "identity replacement unavailability must name a non-zero owning issue in its reason"
                                    .to_string(),
                            );
                        }
                        (None, None, Some(owning_issue), ComparisonOutcome::Match)
                    }
                };
            (
                assertions,
                IdentityConformanceSubject::SourceGraph {
                    graph_identity: graph.source_graph_identity,
                },
                replacement_output_identity,
                replacement_receipt_identity,
                unavailable_issue,
                behavior_outcome,
                plan.comparison_reason,
            )
        }
        IdentityConformancePlan::ReleaseArtifact {
            verify,
            comparison_reason,
        } => {
            let artifact = verify()?;
            validate_identity_assertions(&artifact.assertions, true)?;
            for (name, identity) in [
                ("fixture input", artifact.fixture_input_identity.as_str()),
                ("compiled artifact content", artifact.artifact_content_identity.as_str()),
                (
                    "recovered artifact observation",
                    artifact.recovered_observation_identity.as_str(),
                ),
            ] {
                if !is_sha256_identity(identity) {
                    return Err(format!(
                        "release-artifact conformance has no exact SHA-256 {name} identity: `{identity}`"
                    ));
                }
            }
            (
                artifact.assertions,
                IdentityConformanceSubject::ReleaseArtifact {
                    fixture_input_identity: artifact.fixture_input_identity,
                    artifact_content_identity: artifact.artifact_content_identity,
                    recovered_observation_identity: artifact.recovered_observation_identity,
                },
                None,
                None,
                None,
                ComparisonOutcome::Match,
                comparison_reason,
            )
        }
    };

    let mut evidence = IdentityConformanceEvidence {
        coverage_cells: assertions.coverage_cells,
        subject,
        checked_relations: assertions.checked_relations,
        hir_consumers: assertions.hir_consumers,
        body_ir_consumers: assertions.body_ir_consumers,
        legacy_projections: assertions.legacy_projections,
        artifact_observations: assertions.artifact_observations,
        replacement_output_identity,
        replacement_unavailable_issue,
        evidence_identity: String::new(),
    };
    evidence.evidence_identity = identity_conformance_evidence_identity(&evidence);
    let receipt = ReceiptRef::IdentityConformanceObserved {
        replacement_receipt_identity,
        evidence_identity: evidence.evidence_identity.clone(),
        comparison_reason: reason.to_string(),
    };
    Ok((behavior_outcome, receipt, evidence))
}

fn is_sha256_identity(identity: &str) -> bool {
    identity.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

/// Prevent a source-graph row from degrading into a frontend-only callback.
fn validate_identity_assertions(assertions: &IdentityAssertions, artifact: bool) -> Result<(), String> {
    if artifact {
        if assertions.legacy_projections.is_empty() || assertions.artifact_observations.is_empty() {
            return Err(
                "release-artifact identity evidence must include recovered projections and classification".to_string(),
            );
        }
        return Ok(());
    }
    validate_identity_coverage(&assertions.coverage_cells)?;
    for (name, values) in [
        ("checked equality/distinctness", &assertions.checked_relations),
        ("HIR consumer", &assertions.hir_consumers),
        ("Body-IR/replacement consumer", &assertions.body_ir_consumers),
        ("legacy emitted projection", &assertions.legacy_projections),
    ] {
        if values.is_empty() {
            return Err(format!("identity source-graph row has no {name} evidence"));
        }
    }
    Ok(())
}

pub(crate) fn validate_identity_coverage(cells: &[IdentityCoverageCell]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for cell in cells {
        let key = (cell.binding, cell.namespace, cell.scope);
        if !seen.insert(key) {
            return Err(format!("identity coverage repeats the cell {key:?}"));
        }
        let valid_scope = match cell.namespace {
            IdentityNamespace::Lexical => matches!(
                cell.scope,
                IdentityScope::Module | IdentityScope::Function | IdentityScope::Block
            ),
            IdentityNamespace::Member => matches!(
                cell.scope,
                IdentityScope::Owner | IdentityScope::Function | IdentityScope::Block
            ),
            IdentityNamespace::ModulePath => cell.scope == IdentityScope::Module,
        };
        if !valid_scope {
            return Err(format!(
                "identity coverage cell {key:?} is not a semantically valid namespace/scope combination"
            ));
        }
        if cell.checked_identity.is_empty() {
            return Err(format!("identity coverage cell {key:?} has no checked identity"));
        }
        let hir_required = cell.scope == IdentityScope::Module;
        let body_ir_required = matches!(cell.scope, IdentityScope::Function | IdentityScope::Block);
        let projection_required = cell.namespace != IdentityNamespace::ModulePath;
        for (carrier, required, value) in [
            ("HIR", hir_required, cell.hir_identity.as_deref()),
            ("Body IR", body_ir_required, cell.body_ir_identity.as_deref()),
            (
                "emitted projection",
                projection_required,
                cell.emitted_projection.as_deref(),
            ),
        ] {
            if required != value.is_some_and(|value| !value.is_empty()) {
                return Err(format!(
                    "identity coverage cell {key:?} has invalid {carrier} carrier presence: required={required}, value={value:?}"
                ));
            }
        }
    }
    Ok(())
}

/// Recompute the content identity of every serialized conformance-evidence field.
pub(crate) fn identity_conformance_evidence_identity(evidence: &IdentityConformanceEvidence) -> String {
    let mut parts = match &evidence.subject {
        IdentityConformanceSubject::SourceGraph { graph_identity } => {
            vec![format!("subject=source_graph|graph={graph_identity}")]
        }
        IdentityConformanceSubject::ReleaseArtifact {
            fixture_input_identity,
            artifact_content_identity,
            recovered_observation_identity,
        } => vec![format!(
            "subject=release_artifact|fixture={fixture_input_identity}|artifact={artifact_content_identity}|observation={recovered_observation_identity}"
        )],
    };
    parts.extend(evidence.coverage_cells.iter().map(|cell| {
        format!(
            "cell={}/{}/{}|checked={}|hir={}|body={}|projection={}",
            cell.binding.as_str(),
            cell.namespace.as_str(),
            cell.scope.as_str(),
            cell.checked_identity,
            cell.hir_identity.as_deref().unwrap_or("<none>"),
            cell.body_ir_identity.as_deref().unwrap_or("<none>"),
            cell.emitted_projection.as_deref().unwrap_or("<none>")
        )
    }));
    parts.extend(
        evidence
            .checked_relations
            .iter()
            .map(|value| format!("checked={value}")),
    );
    parts.extend(evidence.hir_consumers.iter().map(|value| format!("hir={value}")));
    parts.extend(evidence.body_ir_consumers.iter().map(|value| format!("body={value}")));
    parts.extend(
        evidence
            .legacy_projections
            .iter()
            .map(|value| format!("projection={value}")),
    );
    parts.extend(
        evidence
            .artifact_observations
            .iter()
            .map(|value| format!("artifact={value}")),
    );
    parts.push(format!(
        "replacement_output={}",
        evidence.replacement_output_identity.as_deref().unwrap_or("<none>")
    ));
    parts.push(format!(
        "replacement_issue={}",
        evidence
            .replacement_unavailable_issue
            .map_or_else(|| "<none>".to_string(), |issue| issue.to_string())
    ));
    let part_refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
    digest_output(&part_refs)
}

// ============================================================================
// Validation (proves the schema surfaces gaps rather than defaulting to green)
// ============================================================================

/// One structural problem found in a corpus by [`validate_corpus`].
///
/// This is separate from [`ComparisonOutcome`]: a validation violation means the case itself is malformed (missing
/// metadata, duplicate ID, a disposition without an owning issue), not that its evaluated behavior diverged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CorpusViolation {
    pub(crate) case_id: String,
    pub(crate) problem: String,
}

/// Validate structural invariants of a candidate corpus without evaluating any case.
///
/// Checks stable-ID uniqueness/non-emptiness, required text fields, and that every non-`Preserved` disposition
/// carries a real (non-zero) owning issue and a non-empty migration note. This is what proves the schema surfaces
/// gaps instead of defaulting to green — see the red-state tests in `tests/parity_corpus_tests.rs`.
pub(crate) fn validate_corpus(cases: &[ParityCase]) -> Vec<CorpusViolation> {
    let mut violations = Vec::new();
    let mut seen_ids: BTreeSet<&'static str> = BTreeSet::new();

    for case in cases {
        if case.id.trim().is_empty() {
            violations.push(CorpusViolation {
                case_id: "<empty>".to_string(),
                problem: "case id must not be empty".to_string(),
            });
        } else if !seen_ids.insert(case.id) {
            violations.push(CorpusViolation {
                case_id: case.id.to_string(),
                problem: "duplicate case id".to_string(),
            });
        }

        if case.title.trim().is_empty() {
            violations.push(CorpusViolation {
                case_id: case.id.to_string(),
                problem: "title must not be empty".to_string(),
            });
        }

        if case.evidence.trim().is_empty() {
            violations.push(CorpusViolation {
                case_id: case.id.to_string(),
                problem: "evidence pointer must not be empty".to_string(),
            });
        }

        let execution_shapes = usize::from(case.evaluate.is_some())
            + usize::from(case.identity_conformance.is_some())
            + usize::from(case.replacement_execution.is_some());
        if execution_shapes != 1 {
            violations.push(CorpusViolation {
                case_id: case.id.to_string(),
                problem: format!("case must declare exactly one execution shape, found {execution_shapes}"),
            });
        }

        match &case.disposition {
            Disposition::Preserved => {}
            Disposition::IntentionalMigration {
                owning_issue,
                migration_note,
            }
            | Disposition::Unsupported {
                owning_issue,
                migration_note,
            } => {
                if *owning_issue == 0 {
                    violations.push(CorpusViolation {
                        case_id: case.id.to_string(),
                        problem: "non-preserved disposition must name a real (non-zero) owning issue".to_string(),
                    });
                }
                if migration_note.trim().is_empty() {
                    violations.push(CorpusViolation {
                        case_id: case.id.to_string(),
                        problem: "non-preserved disposition must carry a migration note".to_string(),
                    });
                }
            }
        }
    }

    violations
}

// ============================================================================
// Evaluation and CI-readable summary
// ============================================================================

/// The combined result of running one case and folding in its execution/comparison-evidence state.
///
/// `overall_state` is the field a CI consumer should read first: it is the only field that already accounts for
/// both axes (observed behavior and paired-comparison availability) so a consumer cannot accidentally read
/// `behavior_outcome` alone and report green while the comparison this corpus promises is still unavailable.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct CaseReport {
    pub(crate) id: &'static str,
    pub(crate) title: &'static str,
    pub(crate) category: BehaviorCategory,
    pub(crate) lane: EvidenceLane,
    pub(crate) evidence: &'static str,
    pub(crate) disposition_kind: &'static str,
    pub(crate) behavior_outcome: ComparisonOutcome,
    pub(crate) receipt: ReceiptRef,
    /// Present only for first-class RFC 120 identity rows; never inferred from the receipt label.
    pub(crate) identity_conformance: Option<IdentityConformanceEvidence>,
    pub(crate) overall_state: OverallState,
}

/// The final, honest per-case state a CI consumer or #655 should read.
///
/// `Green` requires two independent things: a [`ComparisonOutcome::Match`] behavior outcome *and* a
/// source-observable comparison (#1146) that actually ran and agreed. A row that only executes one backend — even
/// with a valid receipt — cannot reach it, because a single route cannot demonstrate parity with the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OverallState {
    /// Behavior matched, and two independent executions of the same source agreed on the compared observable.
    Green,
    /// Behavior or conformance evidence matched, but no source-observable comparison ran for this row. A real
    /// execution receipt is present only when a backend actually executed; checked-graph and artifact observations
    /// deliberately carry none.
    NonGreenShadowUnavailable,
    /// Behavior matched, but the two routes observed different results — a regression signal, not a gap.
    NonGreenShadowDiverged,
    /// The case's own behavior evaluation did not match (mismatch, skip, or incompatible) — a real signal.
    NonGreenBehavior,
}

/// Evaluate one case and fold its behavior plus available evidence into a [`CaseReport`]. Execution rows consult a
/// real #986 receipt; callback, checked-graph, and artifact-only observations do not fabricate one.
pub(crate) fn evaluate_case(case: &ParityCase) -> CaseReport {
    let (behavior_outcome, receipt, identity_conformance) = if let Some(plan) = case.identity_conformance {
        let result = execute_identity_conformance_plan(plan);
        (result.behavior_outcome, result.receipt, result.evidence)
    } else {
        match case.replacement_execution {
            Some(plan) => {
                let evidence = execute_replacement_plan(case.source, plan);
                (evidence.behavior_outcome, evidence.receipt, None)
            }
            None => match case.evaluate {
                Some(evaluate) => {
                    let outcome = evaluate();
                    let evidence_identity =
                        behavior_observation_identity(case.id, case.evidence, case.source, &outcome);
                    (
                        outcome,
                        ReceiptRef::BehaviorObserved {
                            evidence_identity,
                            comparison_reason: CALLBACK_OBSERVATION_REASON.to_string(),
                        },
                        None,
                    )
                }
                None => (
                    ComparisonOutcome::Incompatible {
                        reason: "corpus case has neither a legacy behavior probe nor a replacement execution plan"
                            .to_string(),
                    },
                    ReceiptRef::SelectionError {
                        detail: "corpus case has neither a legacy behavior probe nor a replacement execution plan"
                            .to_string(),
                    },
                    None,
                ),
            },
        }
    };
    // A failed behavior probe outranks the comparison axis: a row whose documented behavior drifted is a
    // regression regardless of whether the two routes happened to agree on the drifted answer.
    let overall_state = if !behavior_outcome.is_green() {
        OverallState::NonGreenBehavior
    } else {
        match receipt {
            ReceiptRef::ShadowMatched { .. } => OverallState::Green,
            ReceiptRef::ShadowDiverged { .. } => OverallState::NonGreenShadowDiverged,
            _ => OverallState::NonGreenShadowUnavailable,
        }
    };
    CaseReport {
        id: case.id,
        title: case.title,
        category: case.category,
        lane: case.lane,
        evidence: case.evidence,
        disposition_kind: case.disposition.kind(),
        behavior_outcome,
        receipt,
        identity_conformance,
        overall_state,
    }
}

/// A CI-readable summary of one corpus run, shaped for #655 (compatibility report) to consume.
///
/// Serializes to a stable-keyed JSON object. `execution_receipt_schema_available` states that #986's schema can be
/// emitted, while `cases_with_execution_receipts` separately states how many rows actually executed a backend.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct CorpusSummary {
    pub(crate) schema_version: u32,
    pub(crate) total_cases: usize,
    pub(crate) green: usize,
    pub(crate) non_green_shadow_unavailable: usize,
    pub(crate) non_green_shadow_diverged: usize,
    pub(crate) non_green_behavior: usize,
    pub(crate) execution_receipt_schema_available: bool,
    pub(crate) cases_with_execution_receipts: usize,
    /// Whether at least one row proved its result through two independent executions of the same source (#1146).
    ///
    /// A top-level flag rather than something a consumer must infer from per-case state: it reports whether the
    /// comparison contract is exercised at all, not whether every row exercises it.
    pub(crate) source_observable_comparison_available: bool,
    pub(crate) cases: Vec<CaseReport>,
}

/// The current CI-summary schema version. Version `3` added `ReceiptRef::ReplacementExecuted`, which binds direct
/// Body-IR cases (initially #988's profile and subsequently #1123's lazy-generator case) to their own
/// selection/execution identities and canonical body, ownership, and runtime evidence. Version `4` makes
/// `OverallState::Green` reachable through the bounded #1146 source-observable comparison, adds the
/// `non_green_shadow_diverged` count and the `source_observable_comparison_available` flag, and gives
/// `ReceiptRef::ShadowMatched`/`ShadowDiverged` both routes' receipt identities. Version `5` adds the canonical
/// direct-task lifecycle projection. Version `6` adds first-class RFC 120 identity-conformance evidence. Version
/// `7` distinguishes callback observations from execution receipts, types the conformance subject, and reports the
/// count of rows that actually produced execution receipts. Bump again whenever `CorpusSummary`'s or
/// `CaseReport`'s field shape changes in a way a consumer (including #655) would need to notice.
pub(crate) const SCHEMA_VERSION: u32 = 7;

/// Evaluate every case in the corpus and assemble the CI-readable summary.
///
/// This does not itself assert anything; callers in `tests/parity_corpus_tests.rs` are responsible for turning
/// `non_green_behavior > 0` (an unexpected regression, as opposed to a case whose disposition already expects a
/// non-green mismatch) into a test failure.
pub(crate) fn summarize(cases: &[ParityCase]) -> CorpusSummary {
    // Source compilation needs the same stack provision as the CLI, not the smaller Rust test-thread default.
    let cases = cases.to_vec();
    let reports: Vec<CaseReport> =
        incan::compiler_stack::run_on_compiler_stack(move || cases.iter().map(evaluate_case).collect());
    let green = reports
        .iter()
        .filter(|r| r.overall_state == OverallState::Green)
        .count();
    let non_green_shadow_unavailable = reports
        .iter()
        .filter(|r| r.overall_state == OverallState::NonGreenShadowUnavailable)
        .count();
    let non_green_shadow_diverged = reports
        .iter()
        .filter(|r| r.overall_state == OverallState::NonGreenShadowDiverged)
        .count();
    let non_green_behavior = reports
        .iter()
        .filter(|r| r.overall_state == OverallState::NonGreenBehavior)
        .count();
    let source_observable_comparison_available = reports.iter().any(|r| {
        matches!(
            r.receipt,
            ReceiptRef::ShadowMatched { .. } | ReceiptRef::ShadowDiverged { .. }
        )
    });
    let cases_with_execution_receipts = reports
        .iter()
        .filter(|report| match &report.receipt {
            ReceiptRef::ReplacementExecuted { .. }
            | ReceiptRef::ShadowMatched { .. }
            | ReceiptRef::ShadowDiverged { .. } => true,
            ReceiptRef::IdentityConformanceObserved {
                replacement_receipt_identity,
                ..
            } => replacement_receipt_identity.is_some(),
            ReceiptRef::BehaviorObserved { .. } | ReceiptRef::SelectionError { .. } => false,
        })
        .count();
    CorpusSummary {
        schema_version: SCHEMA_VERSION,
        total_cases: reports.len(),
        green,
        non_green_shadow_unavailable,
        non_green_shadow_diverged,
        non_green_behavior,
        // #986 landed. Availability of the schema is intentionally separate from the count of rows that actually
        // executed a backend and therefore produced one.
        execution_receipt_schema_available: true,
        cases_with_execution_receipts,
        source_observable_comparison_available,
        cases: reports,
    }
}
