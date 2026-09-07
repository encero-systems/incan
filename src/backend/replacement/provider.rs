//! Direct execution of one checked provider-service operation from an already-lowered plan (#1156).
//!
//! This is the replacement backend's first consumer of the three scoped upstream contracts the Slice 1 train
//! delivered, and it consumes all three rather than growing provider-local equivalents of them. The
//! [`ProviderOperationPlan`] says *which* operation was invoked and against *which* capability, and phrases the
//! authority question through its own [`ProviderOperationPlan::authority_request`]. The
//! [`AuthorityDecisionSource`] seam answers that question; nothing here inspects a grant set, interprets a mode, or
//! decides what a denial means. The [`OperationReceipt`] records what happened, including the denial case where
//! nothing happened at all; the backend's own execution receipt *references* it through [`ProviderReceiptLink`]
//! rather than copying its authority, redaction, or replay semantics.
//!
//! ## The selected first operation
//!
//! The vertical executes one fixture-controlled operation: a ledger provider's `charge(account, amount)`, requiring
//! the RFC 104 capability `host.ledger.charge`. It was selected because it exercises every path this issue asks for
//! while still producing a deterministic observable:
//!
//! - **Deterministic result.** A charge settles to a value computed purely from its recorded inputs, so an allowed
//!   invocation has one source-level observable with no clock, network, or ordering dependence.
//! - **Activation.** A ledger provider is either selected and locally backed by this compilation or it is not, so the
//!   plan's activation state is a real precondition rather than a flag invented for a test.
//! - **Authority.** Moving money is exactly what RFC 104 governs, so a governed denial of `host.ledger.charge` is a
//!   meaningful refusal instead of a contrived one.
//! - **Redaction.** The operation naturally records two attributes at different sensitivities — the amount is public,
//!   the account identifier is secret — so redaction classification is a property of the operation.
//! - **Failure.** A charge can be refused by the ledger itself after authority was granted, which is precisely the
//!   "allowed, then failed" distinction the receipt contract keeps separate from a denial.
//! - **Cleanup.** A charge opens a settlement handle that must be released whether it settled or failed.
//!
//! ## Boundaries this module exists to hold
//!
//! Nothing here branches on a provider module name, a call-site spelling, or an emitted Rust name. A host is asked
//! about an operation by [`CanonicalSymbolId`] and by nothing else, so a local call, an import, an alias, and a
//! re-export of one operation all reach the same host entry. The provider key the plan carries is provenance for
//! receipts and diagnostics only.
//!
//! Import is not authority. A plan exists because an operation was *admitted*, and admission is a lowering-time
//! question about activation and capability identity. The authority question is asked here, once, at the moment an
//! admitted operation is actually invoked — and a denial answers it before the host is reached by any call at all.

use std::{cell::RefCell, rc::Rc};

use incan_semantics_core::authority::AuthorityDecisionSource;
use incan_semantics_core::body_ir::ProviderOperationPlan;
use incan_semantics_core::receipts::{OperationReceipt, ReceiptAttribute, ReceiptStatus, ReplayClassification};
use incan_semantics_core::{
    AuthorityDecision, AuthorityMode, CanonicalSymbolId, HirSourceSpan, IncanType, SymbolOrigin,
};

use crate::backend::selection::{
    BackendKind, BackendSelection, FallbackPolicy, digest_output, finalize_receipt, resolve_execution, select_backend,
    unavailable_shadow_comparison,
};
use crate::frontend::diagnostics::DIAGNOSTIC_SCHEMA_VERSION;

use super::{ReplacementExecutionError, ReplacementValue, runtime_failure, unexecutable_provider_plan, unsupported};

/// Why every provider execution receipt this backend finalizes declares a non-green comparison.
///
/// Direct execution proves the replacement route ran; it proves nothing about agreement with the legacy route,
/// which cannot execute a provider operation at all today. #1146 owns the receipt-bound paired comparison that
/// could retire this reason, so recording it explicitly is what keeps an executed provider operation from being
/// mistaken for a compared one.
pub const PROVIDER_COMPARISON_UNAVAILABLE_REASON: &str = "a directly executed provider operation has no paired legacy observation to compare against; #1146 owns the \
     receipt-bound source-observable comparison that could make this row green";

/// One already-evaluated input handed to a provider host, with the plan facts describing it.
///
/// The value comes from the surrounding call's operands, and everything else comes from the plan's own
/// [`incan_semantics_core::body_ir::ProviderOperationInput`]. Pairing them here is what lets a host record an
/// attribute against the argument that actually carried the value, rather than against the whole call.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderInputValue {
    /// Declared parameter slot this input supplies.
    pub slot: usize,
    /// Zero-based position among the call site's written arguments, which is also its evaluation order.
    pub written_position: usize,
    /// Checked type of the evaluated argument expression.
    pub ty: IncanType,
    /// Span of the argument expression itself.
    pub span: HirSourceSpan,
    /// The value direct execution evaluated for this argument.
    pub value: ReplacementValue,
}

/// One authorized invocation of a provider operation, described to its host.
///
/// The host is told the operation's canonical identity and nothing that would let it re-resolve source. The
/// provider key and module path travel as provenance so a host can label what it is doing, never as a dispatch key.
#[derive(Debug, Clone, Copy)]
pub struct ProviderInvocation<'plan, 'inputs> {
    /// Canonical identity of the operation being invoked.
    pub operation: &'plan CanonicalSymbolId,
    /// Canonical identity of the capability whose authority was granted for this invocation.
    pub capability: &'plan CanonicalSymbolId,
    /// Stable key of the catalog record the operation was admitted from, carried as provenance.
    pub provider_key: &'plan str,
    /// The already-evaluated inputs, in written source order.
    pub inputs: &'inputs [ProviderInputValue],
    /// Span of the invocation, which is where any failure is reported.
    pub call_span: HirSourceSpan,
}

/// What one provider host did when an authorized operation was invoked.
///
/// Both variants carry attributes and a replay classification because both are things that happened: an operation
/// that failed after acquiring a connection still recorded what it attempted, and RFC 104 asks the runtime not to
/// lose that. The host owns redaction — a value it declines to persist arrives as
/// [`ReceiptAttribute::redacted`] — because giving redaction a second owner in this backend would eventually mean
/// two answers about what was recorded.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderOperationOutcome {
    /// The operation ran to completion and produced a source-level value.
    Completed {
        /// The operation's source-level result.
        value: ReplacementValue,
        /// Attributes the host recorded, redacted or not.
        attributes: Vec<ReceiptAttribute>,
        /// How replayable this invocation is.
        replay: ReplayClassification,
    },
    /// Authority was granted, and the operation itself failed.
    Failed {
        /// Source-observable description of the failure.
        detail: String,
        /// Attributes the host recorded before failing.
        attributes: Vec<ReceiptAttribute>,
        /// How replayable this invocation is.
        replay: ReplayClassification,
    },
}

/// Whatever can execute admitted provider operations for a run.
///
/// This is the second seam this vertical consumes rather than owns, and it is deliberately shaped like the first:
/// a consumer holds `&dyn ProviderOperationHost` so a real provider runtime, a local stub, and a test double are
/// interchangeable without the executor knowing which it has.
///
/// [`Self::operation_kind`] doubles as the resolution question. A host that returns `None` does not execute the
/// operation, and the call is refused before authority is consulted and before any receipt exists. Implementors
/// must keep it a pure description: it is called on paths — including a governed denial — where the operation must
/// not run.
pub trait ProviderOperationHost {
    /// The publisher's own kind label for `operation`, or `None` when this host cannot execute it.
    ///
    /// The label is what a receipt records as its `operation_kind`, such as `ledger.charge`. It must not perform
    /// the operation, acquire resources, or observe anything outside the host's own catalog.
    fn operation_kind(&self, operation: &CanonicalSymbolId) -> Option<String>;

    /// Invoke one operation whose authority was already granted.
    ///
    /// Reached only after an [`AuthorityDecisionSource`] allowed the invocation. A denied operation never arrives
    /// here, which is the whole point of asking authority first.
    fn invoke(&self, invocation: &ProviderInvocation<'_, '_>) -> ProviderOperationOutcome;

    /// Release whatever [`Self::invoke`] acquired for one invocation.
    ///
    /// Called exactly once after every invocation that started, whether it completed or failed, and never for an
    /// operation that was denied or refused before it started. A host with nothing to release implements this as a
    /// no-op; a host that holds a connection, a transaction, or a settlement handle closes it here.
    fn release(&self, operation: &CanonicalSymbolId, call_span: HirSourceSpan);
}

/// One transition in a provider operation's direct-execution lifecycle.
///
/// Recorded as report-only evidence and bound into the enclosing execution's output identity, in the same shape
/// direct task frames already use. The vocabulary is closed on purpose: a reader must be able to tell "the host
/// was never reached" from "the host ran and then released" without interpreting free text.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProviderLifecycleProjection {
    /// Stable per-execution invocation identity in source execution order.
    pub invocation_id: usize,
    /// Stable lifecycle transition label.
    pub event: &'static str,
    /// Original source span that caused the transition.
    pub span_start: usize,
    /// Original source span that caused the transition.
    pub span_end: usize,
}

/// Canonical, machine-readable rendering of one backend provider-execution receipt.
///
/// `operation_receipt_sequence_id` is a *reference* into the RFC 104 receipt log this runtime holds, not a restated
/// copy of that receipt. A consumer that wants the authority decision, the recorded attributes, or the replay
/// classification reads the operation receipt itself, so those facts keep exactly one owner.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProviderExecutionProjection {
    /// Identity of the pre-invocation backend selection.
    pub selection_identity: String,
    /// Identity of the finalized backend execution receipt.
    pub receipt_identity: String,
    /// Sequence id of the referenced RFC 104 operation receipt.
    pub operation_receipt_sequence_id: u64,
    /// Stable label for what this backend execution did.
    pub outcome: &'static str,
    /// The explicit, non-green comparison state this execution recorded.
    pub comparison_reason: String,
}

/// One backend selection/execution receipt for a directly executed provider operation.
///
/// This is the backend's own record of having selected the replacement backend and run something with it. It is
/// deliberately a separate artifact from the operation receipt: the operation receipt answers "what did this
/// capability-aware operation do", and this answers "which backend executed it, and what comparison state did that
/// The backend's link from an execution record to the RFC 104 operation receipt it corresponds to.
///
/// [`incan_semantics_core::receipts`] deliberately exports no standalone cross-run reference: a sequence number
/// alone means nothing outside the run that produced it, so linkage belongs to whichever producer owns both ends.
/// This is the replacement backend's end of that link, and it carries the sequence id only — never a copy of the
/// receipt's authority, redaction, or replay facts, which would give those two owners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderReceiptLink {
    /// The referenced receipt's sequence id within this run.
    pub sequence_id: u64,
}

/// execution have". The link between them is [`Self::operation_receipt`] and only that.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderExecutionRecord {
    /// The pre-invocation selection this execution was bound to.
    pub selection: BackendSelection,
    /// The finalized backend execution receipt.
    pub receipt: crate::backend::selection::BackendExecutionReceipt,
    /// Reference to the RFC 104 operation receipt this execution's authority and redaction semantics live on.
    pub operation_receipt: ProviderReceiptLink,
    /// Stable label for what this backend execution did.
    pub outcome: &'static str,
}

impl ProviderExecutionRecord {
    /// Project this record into the stable evidence shape identities and reports share.
    #[must_use]
    pub fn projection(&self) -> ProviderExecutionProjection {
        ProviderExecutionProjection {
            selection_identity: self.selection.identity.clone(),
            receipt_identity: self.receipt.identity.clone(),
            operation_receipt_sequence_id: self.operation_receipt.sequence_id,
            outcome: self.outcome,
            comparison_reason: PROVIDER_COMPARISON_UNAVAILABLE_REASON.to_string(),
        }
    }
}

/// One provider lifecycle transition observed during direct execution.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderLifecycleEvent {
    invocation_id: usize,
    event: &'static str,
    span: HirSourceSpan,
}

/// Everything one run accumulated while executing provider operations.
///
/// Held behind a [`RefCell`] because the executor reaches the runtime through a shared handle that nested callable
/// frames also hold: receipts must be sequenced across the whole run, not per frame, or two frames would each
/// think they emitted receipt `#0`.
#[derive(Debug, Default)]
struct ProviderRuntimeState {
    next_sequence_id: u64,
    next_invocation_id: usize,
    receipts: Vec<OperationReceipt>,
    executions: Vec<ProviderExecutionRecord>,
    lifecycle: Vec<ProviderLifecycleEvent>,
}

/// The authority source, provider host, and receipt log one direct execution runs against.
///
/// The caller builds this and keeps it, which is deliberate: a denial and a provider failure both stop the
/// enclosing execution with an error, and their receipts must still be readable afterwards. Putting the receipt log
/// in the runtime rather than in the successful-execution result is what makes a refused run as reportable as a
/// successful one.
pub struct ProviderRuntime {
    authority: Rc<dyn AuthorityDecisionSource>,
    host: Rc<dyn ProviderOperationHost>,
    state: RefCell<ProviderRuntimeState>,
}

impl std::fmt::Debug for ProviderRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderRuntime")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl ProviderRuntime {
    /// Build a runtime that decides authority through `authority` and executes operations through `host`.
    #[must_use]
    pub fn new(authority: Rc<dyn AuthorityDecisionSource>, host: Rc<dyn ProviderOperationHost>) -> Rc<Self> {
        Rc::new(Self {
            authority,
            host,
            state: RefCell::new(ProviderRuntimeState::default()),
        })
    }

    /// Every RFC 104 operation receipt this run emitted, in emission order.
    #[must_use]
    pub fn operation_receipts(&self) -> Vec<OperationReceipt> {
        self.state.borrow().receipts.clone()
    }

    /// Every backend selection/execution receipt this run finalized, in execution order.
    #[must_use]
    pub fn provider_executions(&self) -> Vec<ProviderExecutionRecord> {
        self.state.borrow().executions.clone()
    }

    /// Every provider lifecycle transition this run observed, in execution order.
    #[must_use]
    pub fn lifecycle_evidence(&self) -> Vec<ProviderLifecycleProjection> {
        self.state
            .borrow()
            .lifecycle
            .iter()
            .map(|event| ProviderLifecycleProjection {
                invocation_id: event.invocation_id,
                event: event.event,
                span_start: event.span.start,
                span_end: event.span.end,
            })
            .collect()
    }

    /// Whether this runtime's host can execute the operation named by `operation`.
    ///
    /// Consulted by the pre-execution profile gate so an unresolved operation refuses before anything runs, rather
    /// than part-way through the enclosing body.
    pub(super) fn resolves(&self, operation: &CanonicalSymbolId) -> bool {
        self.host.operation_kind(operation).is_some()
    }

    /// Execute one admitted provider operation whose inputs direct execution has already evaluated.
    ///
    /// The order of the steps is the contract, not an implementation detail. Admission is re-checked before anything
    /// else, because a plan that reached here claiming an inactive provider means an earlier gate was bypassed.
    /// Resolution comes next, so an operation no host executes is refused while it is still true that nothing ran.
    /// Authority is decided *after* those and *before* the host is invoked, which is what makes a denial structural:
    /// the only call path that reaches [`ProviderOperationHost::invoke`] runs through an allowed decision.
    pub(super) fn execute(
        &self,
        plan: &ProviderOperationPlan,
        inputs: Vec<ProviderInputValue>,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let span = plan.call_span;
        let declared = plan.operation.declaration_name.as_str();

        // ---- Fail-closed admission: an unexecutable plan must never reach a host ----
        // The rule lives in `unexecutable_provider_plan` and is consulted rather than restated, so the pre-execution
        // gate and this last-line check cannot drift into disagreeing about which plans are executable.
        if let Some(description) = unexecutable_provider_plan(plan, inputs.len()) {
            return Err(unsupported(description, span));
        }
        let Some(operation_kind) = self.host.operation_kind(&plan.operation) else {
            return Err(unsupported(
                format!("provider operation `{declared}` that no provider host in this run executes"),
                span,
            ));
        };

        // ---- Authority: asked once, at invocation, through the plan's own request ----
        let decision = self.authority.decide(&plan.authority_request());
        if !decision.is_allowed() {
            return Err(self.record_denial(plan, decision, operation_kind));
        }

        // ---- Invocation, then cleanup, then policy-selected reporting ----
        let invocation_id = self.next_invocation_id();
        self.record_lifecycle(invocation_id, "invoked", span);
        let outcome = self.host.invoke(&ProviderInvocation {
            operation: &plan.operation,
            capability: &plan.required_capability,
            provider_key: plan.provider.provider_key.as_str(),
            inputs: &inputs,
            call_span: span,
        });
        let event = match &outcome {
            ProviderOperationOutcome::Completed { .. } => "completed",
            ProviderOperationOutcome::Failed { .. } => "failed",
        };
        self.record_lifecycle(invocation_id, event, span);

        // Cleanup is unconditional for an invocation that started. A failed charge still holds the settlement
        // handle it opened, and leaking it is exactly the outcome a lifecycle contract exists to prevent.
        self.host.release(&plan.operation, span);
        self.record_lifecycle(invocation_id, "released", span);

        self.record_outcome(plan, decision, operation_kind, outcome)
    }

    /// Record a governed denial and return the diagnostic that reports it at the call site.
    ///
    /// A denial is never a success, so this returns the error itself rather than a `Result`: every path out of it is
    /// a refusal, and the only question is whether the refusal is the denial or a failure to record it honestly.
    /// Returning the error to the caller rather than raising it here keeps `return Err(...)` the single exit from
    /// the denial branch, so no later statement can continue past a refusal.
    fn record_denial(
        &self,
        plan: &ProviderOperationPlan,
        decision: AuthorityDecision,
        operation_kind: String,
    ) -> ReplacementExecutionError {
        let span = plan.call_span;
        let reason = denial_reason_text(&decision);
        let invocation_id = self.next_invocation_id();
        let sequence_id = self.next_sequence_id();
        // `denied` validates on construction and refuses to turn an allowed or non-governed decision into a
        // durable governed-denial claim, so a receipt that exists is one the contract already accepted.
        let receipt = match OperationReceipt::denied(sequence_id, plan.operation.clone(), decision, operation_kind) {
            Ok(receipt) => receipt,
            Err(violation) => return Self::receipt_contract_error(violation, span),
        };
        if let Err(error) = self.record_receipt(receipt, span) {
            return error;
        }
        self.record_lifecycle(invocation_id, "denied", span);
        let reference = ProviderReceiptLink { sequence_id };
        if let Err(error) = self.finalize_execution(plan, reference, "denied", span) {
            return error;
        }
        ReplacementExecutionError::ProviderAuthorityDenied {
            operation: plan.operation.declaration_name.clone(),
            reason,
            receipt_sequence_id: sequence_id,
            span,
            span_start: span.start,
            span_end: span.end,
        }
    }

    /// Record the receipt for an invocation that actually ran, and surface its source-level result.
    ///
    /// The status is derived from what the host reported and from whether it withheld any attribute value, which is
    /// the one classification this backend makes. It is a reading of the host's own redaction decision, never a
    /// second redaction policy: a withheld value stays withheld, and the status simply stops claiming that a
    /// receipt with withheld values recorded everything.
    fn record_outcome(
        &self,
        plan: &ProviderOperationPlan,
        decision: AuthorityDecision,
        operation_kind: String,
        outcome: ProviderOperationOutcome,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let span = plan.call_span;
        if matches!(decision.mode, AuthorityMode::Permissive) {
            return Self::return_unreported_outcome(plan, outcome);
        }

        let (status, attributes, replay, failure) = match outcome {
            ProviderOperationOutcome::Completed {
                value,
                attributes,
                replay,
            } => {
                // `Allowed` is a governed-only outcome: it claims authority was enforced and granted. An observed
                // run never enforced anything, so its truthful success status is `Observed` -- the receipt contract
                // rejects the stronger claim rather than letting a run overstate what happened.
                let status = if attributes.iter().any(ReceiptAttribute::is_redacted) {
                    ReceiptStatus::Redacted
                } else if matches!(decision.mode, AuthorityMode::Governed) {
                    ReceiptStatus::Allowed
                } else {
                    ReceiptStatus::Observed
                };
                (status, attributes, replay, Ok(value))
            }
            ProviderOperationOutcome::Failed {
                detail,
                attributes,
                replay,
            } => (ReceiptStatus::Failed, attributes, replay, Err(detail)),
        };

        let sequence_id = self.next_sequence_id();
        // The capability and the use-site span are derived from the decision rather than repeated here, so the two
        // cannot drift apart in a stored receipt.
        let receipt = OperationReceipt::new(
            sequence_id,
            plan.operation.clone(),
            operation_kind,
            status,
            decision,
            None,
            attributes,
            replay,
        )
        .map_err(|violation| Self::receipt_contract_error(violation, span))?;
        self.record_receipt(receipt, span)?;
        let reference = ProviderReceiptLink { sequence_id };
        let label = match status {
            ReceiptStatus::Redacted => "redacted",
            ReceiptStatus::Observed => "observed",
            ReceiptStatus::Failed => "failed",
            _ => "allowed",
        };
        self.finalize_execution(plan, reference, label, span)?;

        match failure {
            Ok(value) => Ok(value),
            Err(detail) => Err(ReplacementExecutionError::ProviderOperationFailed {
                operation: plan.operation.declaration_name.clone(),
                detail,
                receipt_sequence_id: Some(sequence_id),
                span,
                span_start: span.start,
                span_end: span.end,
            }),
        }
    }

    /// Return a permissive invocation result without retaining authority or backend execution evidence.
    ///
    /// RFC 104 defines permissive as the explicit reporting-disabled escape hatch. The operation still executes and
    /// still releases its host resources, but it must not construct an `Observed` operation receipt or a backend
    /// execution record that would claim to reference one.
    fn return_unreported_outcome(
        plan: &ProviderOperationPlan,
        outcome: ProviderOperationOutcome,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        match outcome {
            ProviderOperationOutcome::Completed { value, .. } => Ok(value),
            ProviderOperationOutcome::Failed { detail, .. } => {
                Err(ReplacementExecutionError::ProviderOperationFailed {
                    operation: plan.operation.declaration_name.clone(),
                    detail,
                    receipt_sequence_id: None,
                    span: plan.call_span,
                    span_start: plan.call_span.start,
                    span_end: plan.call_span.end,
                })
            }
        }
    }

    /// Check one receipt against its own contract before it is retained as evidence.
    ///
    /// A receipt that contradicts itself is a runtime failure of the publisher, not a fact to store: RFC 104's
    /// whole premise is that a receipt is believed by a reader with no way to re-derive the truth.
    fn record_receipt(&self, receipt: OperationReceipt, _span: HirSourceSpan) -> Result<(), ReplacementExecutionError> {
        // No validation here: `OperationReceipt`'s constructors validate and refuse to build a receipt that
        // contradicts itself, so anything reaching this point is already a receipt the contract accepted.
        self.state.borrow_mut().receipts.push(receipt);
        Ok(())
    }

    /// Turn a refused receipt construction into a runtime failure at the operation's own span.
    ///
    /// A contract violation here is a defect in this backend rather than in the program being run: it means the
    /// executor tried to record a claim the receipt contract rejects, such as a denial over an allowing decision.
    fn receipt_contract_error(
        violation: incan_semantics_core::receipts::ReceiptContractViolation,
        span: HirSourceSpan,
    ) -> ReplacementExecutionError {
        runtime_failure(
            format!("provider operation receipt contradicts itself: {violation}"),
            span,
        )
    }

    /// Declare, resolve, and finalize the backend receipt for one provider execution.
    ///
    /// Uses the ordinary #986 selection API rather than a provider-local receipt shape, and always records an
    /// explicitly unavailable shadow comparison: a direct execution with no paired legacy observation must be
    /// visibly non-green rather than silently uncompared.
    fn finalize_execution(
        &self,
        plan: &ProviderOperationPlan,
        operation_receipt: ProviderReceiptLink,
        outcome: &'static str,
        span: HirSourceSpan,
    ) -> Result<(), ReplacementExecutionError> {
        let selection = select_backend(
            BackendKind::Replacement,
            true,
            true,
            provider_source_identity(plan),
            FallbackPolicy::Refuse,
        );
        let executed = resolve_execution(&selection, BackendKind::Replacement.is_implemented())
            .map_err(|error| runtime_failure(format!("provider execution selection failed: {error}"), span))?;
        let output_identity = digest_output(&[
            selection.source_identity.as_str(),
            outcome,
            operation_receipt.sequence_id.to_string().as_str(),
        ]);
        let receipt = finalize_receipt(
            &selection,
            executed,
            output_identity,
            unavailable_shadow_comparison(selection.shadow_requested, PROVIDER_COMPARISON_UNAVAILABLE_REASON),
            DIAGNOSTIC_SCHEMA_VERSION,
        )
        .map_err(|error| {
            runtime_failure(
                format!("provider execution receipt could not be finalized: {error}"),
                span,
            )
        })?;
        self.state.borrow_mut().executions.push(ProviderExecutionRecord {
            selection,
            receipt,
            operation_receipt,
            outcome,
        });
        Ok(())
    }

    /// Allocate the next receipt sequence id for this run.
    fn next_sequence_id(&self) -> u64 {
        let mut state = self.state.borrow_mut();
        let sequence_id = state.next_sequence_id;
        state.next_sequence_id += 1;
        sequence_id
    }

    /// Allocate the next lifecycle invocation id for this run.
    fn next_invocation_id(&self) -> usize {
        let mut state = self.state.borrow_mut();
        let invocation_id = state.next_invocation_id;
        state.next_invocation_id += 1;
        invocation_id
    }

    /// Append one lifecycle transition in execution order.
    fn record_lifecycle(&self, invocation_id: usize, event: &'static str, span: HirSourceSpan) {
        self.state.borrow_mut().lifecycle.push(ProviderLifecycleEvent {
            invocation_id,
            event,
            span,
        });
    }
}

/// Render a denial as the text a source-owned diagnostic reports.
///
/// The suggested grant comes from the decision's own provenance, which the plan phrased through
/// [`ProviderOperationPlan::authority_request`], so the remedy a user is offered is the one RFC 104 named rather
/// than a spelling this backend invented.
fn denial_reason_text(decision: &AuthorityDecision) -> String {
    let reason = decision
        .denial_reason()
        .map_or("denied", incan_semantics_core::AuthorityDenialReason::as_str);
    format!(
        "{} authority for `{}` was {reason}; grant `{}` to permit it",
        decision.mode.as_str(),
        decision.capability.declaration_name,
        decision.provenance.suggested_grant,
    )
}

/// Derive the content identity of the plan one provider execution was selected for.
///
/// This is an identity digest component, never a user-facing spelling and never a dispatch key: it exists so two
/// invocations of different operations, or of one operation at different call sites, cannot claim the same
/// selection. The canonical identity's own parts are digested rather than rendered into a dotted path, precisely so
/// nothing here can be mistaken for a second spelling of the operation.
fn provider_source_identity(plan: &ProviderOperationPlan) -> String {
    let inputs = plan
        .inputs
        .iter()
        .map(|input| {
            format!(
                "slot={};written={};ty={};span={}..{}",
                input.slot, input.written_position, input.ty, input.span.start, input.span.end
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    digest_output(&[
        canonical_identity_component(&plan.operation).as_str(),
        canonical_identity_component(&plan.required_capability).as_str(),
        plan.provider.provider_key.as_str(),
        plan.provider.state.as_str(),
        inputs.as_str(),
        format!("{}..{}", plan.call_span.start, plan.call_span.end).as_str(),
    ])
}

/// Render one canonical identity's own parts as a deterministic digest component.
fn canonical_identity_component(symbol: &CanonicalSymbolId) -> String {
    let origin = match &symbol.origin {
        SymbolOrigin::Module(path) => format!("module:{}", path.join(".")),
        SymbolOrigin::RustCrate(path) => format!("rust_crate:{}", path.join(".")),
        SymbolOrigin::Package { library, module_path } => {
            format!("package:{library}:{}", module_path.join("."))
        }
        SymbolOrigin::Builtin => "builtin".to_string(),
    };
    format!(
        "{origin};name={};kind={:?};span={}..{}",
        symbol.declaration_name, symbol.kind, symbol.declaration_span.start, symbol.declaration_span.end
    )
}

/// Render provider executions as one deterministic output-identity component.
///
/// Empty when a run executed no provider operation, which keeps a scalar execution's evidence exactly as wide as
/// what it actually observed.
pub(super) fn canonical_provider_execution_summary(records: &[ProviderExecutionRecord]) -> String {
    records
        .iter()
        .map(|record| {
            let projection = record.projection();
            format!(
                "selection={};receipt={};operation_receipt={};outcome={}",
                projection.selection_identity,
                projection.receipt_identity,
                projection.operation_receipt_sequence_id,
                projection.outcome,
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}

#[cfg(test)]
mod tests;
