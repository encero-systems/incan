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
//! available." #986 landed [`incan::backend::selection`], so [`evaluate_case`] declares a real
//! [`BackendSelection`], resolves it, and finalizes a real [`BackendExecutionReceipt`] for every case's source —
//! the same three-call sequence `src/cli/commands/build.rs` uses for an actual build. The #988 Body-IR cases
//! additionally execute the replacement backend and carry their own selection/execution receipt, Body-IR snapshot,
//! ownership evidence, and runtime requirements.
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

use incan::backend::replacement::{
    OwnershipReadProjection, ReplacementValue, RuntimeRequirementProjection, TaskLifecycleProjection,
    execute_prevalidated_free_function, prepare_free_function_execution,
};
use incan::backend::selection::{
    BackendExecutionReceipt, BackendKind, BackendSelection, BackendSelectionError, FallbackPolicy,
    ShadowComparisonState, digest_output, finalize_receipt, resolve_execution, select_backend,
    unavailable_shadow_comparison,
};
use incan::backend::shadow::{
    LegacyExecutionAuthority, ShadowComparison, ShadowComparisonProfile, compare_source_observable,
};
use incan::frontend::body_ir::{apply_body_ir_input_contract, build_body_ir_module_v0};
use incan::frontend::diagnostics::DIAGNOSTIC_SCHEMA_VERSION;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan_semantics_core::body_ir::BodyIrModule;
use std::collections::BTreeSet;

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

/// One of the six evidence lanes from the #646 inventory's `Evidence lanes` table.
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
    /// No seed case uses this lane yet — deferred to plan step 4 alongside `RustInteropBehavior`.
    #[allow(dead_code)]
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

/// The result of actually consulting the #986 backend-selection receipt for one case's source.
///
/// Every variant carries the real [`BackendExecutionReceipt::identity`] this case produced — never a guessed or
/// presence-checked value — so a consumer can independently re-verify which receipt a case's `overall_state` is
/// derived from.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum ReceiptRef {
    /// A real legacy receipt was produced, but its requested source-observable comparison is unavailable. This is
    /// the honest state for pre-#988 seed rows that do not yet own a direct replacement execution. See
    /// [`OverallState::NonGreenShadowUnavailable`].
    ShadowUnavailable { receipt_identity: String, reason: String },
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

/// Reason a row with no direct replacement execution cannot have a source-observable comparison.
const NO_REPLACEMENT_ROUTE_REASON: &str = "this row observes the legacy backend only; it has no direct replacement execution to compare against, so no \
     source-observable comparison was made";

/// Reason a direct-execution row that has not opted into the bounded #1146 profile stays non-green.
const NO_DECLARED_COMPARISON_REASON: &str = "this row executes the replacement backend directly but does not declare the bounded #1146 source-observable \
     comparison profile, so no legacy route was run to compare it against";

/// Declare, resolve, and finalize the legacy-side #986 receipt retained by the pre-#988 seed rows.
fn compute_legacy_receipt(source: &str) -> ReceiptRef {
    let source_identity = digest_output(&[source]);
    let selection = select_backend(
        BackendKind::Legacy,
        false,
        true,
        source_identity,
        FallbackPolicy::Refuse,
    );
    let executed = match resolve_execution(&selection, BackendKind::Legacy.is_implemented()) {
        Ok(backend) => backend,
        Err(error) => return receipt_ref_from_error(&error),
    };
    let shadow_comparison = shadow_comparison_for(&selection);
    let output_identity = digest_output(&[source]);
    let receipt: Result<BackendExecutionReceipt, BackendSelectionError> = finalize_receipt(
        &selection,
        executed,
        output_identity,
        shadow_comparison,
        DIAGNOSTIC_SCHEMA_VERSION,
    );
    match receipt {
        Ok(receipt) => receipt_ref_from_receipt(receipt),
        Err(error) => receipt_ref_from_error(&error),
    }
}

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
    let capability = crate::shadow_capability::legacy_capability().map_err(|error| error.reason)?;
    let workspace = tempfile::tempdir()
        .map_err(|error| format!("the legacy comparison route could not create a workspace: {error}"))?;
    let profile = ShadowComparisonProfile::new(source, plan.function, (plan.arguments)());
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

/// Decide the shadow-comparison state for a legacy-only row's selection.
///
/// Routed through the same [`unavailable_shadow_comparison`] helper `build.rs` uses, so an unavailable comparison
/// is recorded the one canonical way rather than hand-assembled here; only the reason is row-specific.
fn shadow_comparison_for(selection: &BackendSelection) -> ShadowComparisonState {
    unavailable_shadow_comparison(selection.shadow_requested, NO_REPLACEMENT_ROUTE_REASON)
}

/// Fold one single-route receipt's shadow-comparison outcome into the [`ReceiptRef`] a [`CaseReport`] carries.
///
/// A comparison outcome is a claim about *two* routes, so it can only be assembled from both routes' receipts by
/// [`compare_replacement_plan`]. Seeing one here would mean a single-route receipt had been handed a verdict it
/// cannot support, which is recorded as a selection error rather than passed through as evidence.
fn receipt_ref_from_receipt(receipt: BackendExecutionReceipt) -> ReceiptRef {
    match receipt.shadow_comparison {
        ShadowComparisonState::Unavailable { reason } => ReceiptRef::ShadowUnavailable {
            receipt_identity: receipt.identity,
            reason,
        },
        ShadowComparisonState::NotRequested => ReceiptRef::ShadowUnavailable {
            receipt_identity: receipt.identity,
            reason: "no shadow comparison was requested for this case".to_string(),
        },
        state => ReceiptRef::SelectionError {
            detail: format!(
                "a single-route receipt cannot carry the two-route comparison outcome {state:?}; only a real \
                 comparison over both routes may record one"
            ),
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
    /// The case's own Incan source, used to derive a real #986 backend-selection receipt via [`evaluate_case`].
    /// Red-state fixtures that never reach [`evaluate_case`] may use a trivial placeholder since it is unused.
    pub(crate) source: &'static str,
    /// Executes the legacy/non-replacement comparison for this case. Direct replacement plans instead derive their
    /// outcome from the single selected execution that also produces their receipt. Must not panic on an expected
    /// non-green result — return [`ComparisonOutcome::Mismatch`]/`Skipped`/`Incompatible` instead.
    pub(crate) evaluate: Option<fn() -> ComparisonOutcome>,
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

/// The combined result of running one case and folding in its receipt-availability state.
///
/// `overall_state` is the field a CI consumer should read first: it is the only field that already accounts for
/// both axes (observed behavior and receipt availability) so a consumer cannot accidentally read `behavior_outcome`
/// alone and report green while the receipt-aware comparison this corpus promises is still unavailable.
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
    /// Behavior matched and a real receipt was produced, but no source-observable comparison ran for this row.
    NonGreenShadowUnavailable,
    /// Behavior matched, but the two routes observed different results — a regression signal, not a gap.
    NonGreenShadowDiverged,
    /// The case's own behavior evaluation did not match (mismatch, skip, or incompatible) — a real signal.
    NonGreenBehavior,
}

/// Evaluate one case: run its behavior probe, consult a real #986 receipt for its source, and fold both into a
/// [`CaseReport`].
pub(crate) fn evaluate_case(case: &ParityCase) -> CaseReport {
    let (behavior_outcome, receipt) = match case.replacement_execution {
        Some(plan) => {
            let evidence = execute_replacement_plan(case.source, plan);
            (evidence.behavior_outcome, evidence.receipt)
        }
        None => match case.evaluate {
            Some(evaluate) => (evaluate(), compute_legacy_receipt(case.source)),
            None => (
                ComparisonOutcome::Incompatible {
                    reason: "corpus case has neither a legacy behavior probe nor a replacement execution plan"
                        .to_string(),
                },
                ReceiptRef::SelectionError {
                    detail: "corpus case has neither a legacy behavior probe nor a replacement execution plan"
                        .to_string(),
                },
            ),
        },
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
        overall_state,
    }
}

/// A CI-readable summary of one corpus run, shaped for #655 (compatibility report) to consume.
///
/// Serializes to a stable-keyed JSON object. `receipt_schema_available` is a top-level flag (not just inferable
/// from per-case state) so a consumer can immediately see whether #986 has landed without scanning every case.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct CorpusSummary {
    pub(crate) schema_version: u32,
    pub(crate) total_cases: usize,
    pub(crate) green: usize,
    pub(crate) non_green_shadow_unavailable: usize,
    pub(crate) non_green_shadow_diverged: usize,
    pub(crate) non_green_behavior: usize,
    pub(crate) receipt_schema_available: bool,
    /// Whether at least one row proved its result through two independent executions of the same source (#1146).
    ///
    /// A top-level flag rather than something a consumer must infer from per-case state, matching
    /// `receipt_schema_available`: it reports whether the comparison contract is exercised at all, not whether
    /// every row exercises it.
    pub(crate) source_observable_comparison_available: bool,
    pub(crate) cases: Vec<CaseReport>,
}

/// The current CI-summary schema version. Version `3` added `ReceiptRef::ReplacementExecuted`, which binds direct
/// Body-IR cases (initially #988's profile and subsequently #1123's lazy-generator case) to their own
/// selection/execution identities and canonical body, ownership, and runtime evidence. Version `4` makes
/// `OverallState::Green` reachable through the bounded #1146 source-observable comparison, adds the
/// `non_green_shadow_diverged` count and the `source_observable_comparison_available` flag, and gives
/// `ReceiptRef::ShadowMatched`/`ShadowDiverged` both routes' receipt identities. Version `5` adds the canonical
/// direct-task lifecycle projection. Bump again whenever
/// `CorpusSummary`'s or `CaseReport`'s field shape changes in a way a consumer (including #655) would need to
/// notice.
pub(crate) const SCHEMA_VERSION: u32 = 5;

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
    CorpusSummary {
        schema_version: SCHEMA_VERSION,
        total_cases: reports.len(),
        green,
        non_green_shadow_unavailable,
        non_green_shadow_diverged,
        non_green_behavior,
        // #986 landed: every case above consulted a real `BackendExecutionReceipt`, not a placeholder. This is
        // `true` regardless of how many cases reach `Green`, because it reports whether the receipt contract
        // itself is available, not whether every comparison succeeded.
        receipt_schema_available: true,
        source_observable_comparison_available,
        cases: reports,
    }
}
