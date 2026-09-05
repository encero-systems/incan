//! Backend-selection identity and execution receipt for the #652 replacement-backend cutover.
//!
//! The v0.6 programme tracked by #652 introduces a second compiler backend (the Body IR
//! "replacement" backend, tracked by #653) alongside the current Rust-source-emission backend
//! (`IrCodegen`, `src/backend/ir/`, referred to here as "legacy"). #988 supplies a deliberately partial direct
//! Body-IR profile; the selection boundary still declares and records which backend was intended and actually ran,
//! so a legacy result is never mistaken for replacement execution and an unsupported replacement source remains a
//! visible refusal rather than a silent legacy execution.
//!
//! This module owns two boundary types:
//!
//! - [`BackendSelection`]: a versioned, content-identified record of what was decided *before* execution — which
//!   backend, at what implementation revision, for what compatibility profile and source input, for what reason, and
//!   under what fallback policy.
//! - [`BackendExecutionReceipt`]: a versioned, content-identified record of what actually happened *after* execution —
//!   which backend actually ran, whether a declared fallback occurred, the optional shadow-comparison outcome, the
//!   diagnostic-contract version in force, and the produced output's identity.
//!
//! Both types are plain, IO-free data: this module does not invoke `IrCodegen` or any other
//! execution machinery. Callers (the CLI build path today) perform the actual dispatch and use
//! [`resolve_execution`] and [`finalize_receipt`] to turn real outcomes into these records. This
//! keeps the boundary testable without a full compilation pipeline and keeps it reusable by other
//! clients (Oven, `incan inspect`) that only need to read a versioned receipt.
//!
//! This is a different axis from Oven's legacy-Cargo-vs-direct-rustc *build* boundary
//! (`src/oven.rs`, `OvenCompatibilityKind`), which selects how an already-generated artifact is
//! compiled and never influences which compiler backend produced it.

use serde::{Deserialize, Serialize};

/// Current wire format for [`BackendSelection`] and [`BackendExecutionReceipt`].
///
/// Version `2` is the first version in which [`ShadowComparisonState::Matched`] and
/// [`ShadowComparisonState::Diverged`] are reachable, and both carry the comparison profile that produced them
/// (#1146). Version `1` could only ever record `NotRequested` or `Unavailable`, so no v1 receipt can contain a
/// comparison payload; the bump exists so a consumer that already deserializes `shadow_comparison` is told the
/// shape grew rather than silently meeting an unfamiliar variant body.
pub const BACKEND_SELECTION_SCHEMA_VERSION: u32 = 2;

/// Stable reference to the session-owned semantic module behind one backend execution.
///
/// The values are compiler data-model identities rather than source paths, so a persisted receipt can name the
/// checked semantic authority without leaking a machine-local path or exposing private HIR/Body-IR structures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticModuleProvenance {
    module_id: String,
    module_path: String,
    source_identity: String,
    semantic_snapshot_identity: String,
}

impl SemanticModuleProvenance {
    /// Build provenance for one session-owned semantic module and its deterministic source and snapshot identities.
    #[must_use]
    pub(crate) fn new(
        module_id: String,
        module_path: String,
        source_identity: String,
        semantic_snapshot_identity: String,
    ) -> Self {
        Self {
            module_id,
            module_path,
            source_identity,
            semantic_snapshot_identity,
        }
    }

    /// Return the source identity that must agree with the bound backend selection.
    pub(crate) fn source_identity(&self) -> &str {
        &self.source_identity
    }
}

/// Implementation revision of the current Rust-emission ("legacy") backend.
///
/// Independent of [`crate::version::INCAN_VERSION`]: increase it only when a change to the
/// Rust-emission pipeline can change generated output for a previously accepted program, so a
/// consumer keying reuse on this revision knows to invalidate.
pub const LEGACY_BACKEND_REVISION: u32 = 1;

/// Implementation revision of the partial Body-IR replacement backend.
///
/// #988 provides its first direct-execution profile. Bump this when that profile's observable execution or
/// receipt-bound semantic evidence changes for previously accepted source.
pub const REPLACEMENT_BACKEND_REVISION: u32 = 1;

/// Compiler backend that can produce or execute a build unit.
///
/// This is the codegen-execution axis tracked by #652/#986. See the module docs for how it
/// differs from Oven's build-compilation boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// The current Rust-source-emission pipeline (`IrCodegen`, `src/backend/ir/`).
    Legacy,
    /// The Body IR replacement backend tracked by #653. Its first executable #988 profile is intentionally partial;
    /// unsupported source is refused visibly instead of falling back to the legacy backend.
    Replacement,
}

impl BackendKind {
    /// Whether this backend can actually execute a compilation today.
    ///
    /// Both backends have an executable implementation. The replacement backend still accepts only its declared
    /// #988 Body-IR profile, so callers must validate source support at the replacement boundary rather than using
    /// this capability bit as a claim of full language coverage.
    #[must_use]
    pub fn is_implemented(self) -> bool {
        matches!(self, BackendKind::Legacy | BackendKind::Replacement)
    }
}

/// Coverage a backend declares for the compilation it was selected for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityProfile {
    /// The backend declares complete coverage of the active release's language/runtime contract.
    /// The legacy backend is always `Full` today.
    Full,
    /// The backend declares only partial coverage. The replacement backend is `Partial` until
    /// #988 proves and expands its execution to the complete v0.6 contract.
    Partial,
}

/// Why a particular backend was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionReason {
    /// No explicit backend was requested; the compiler-owned default was declared. This is the
    /// "declared legacy capability selection" case: even the default path produces an explicit,
    /// recorded selection rather than an implicit, unrecorded one.
    Default,
    /// The caller explicitly requested this backend, for example `incan build --backend <kind>`.
    ExplicitRequest,
}

/// Declared policy for what happens if the selected backend cannot execute.
///
/// The policy is decided and recorded as part of [`BackendSelection`], before execution is
/// attempted, so a later fallback (or refusal) is always a declared outcome rather than a
/// runtime improvisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackPolicy {
    /// Never silently substitute another backend; an unavailable selection must fail visibly via
    /// [`BackendSelectionError::Refused`].
    Refuse,
    /// If the selected backend cannot execute, fall back to the named backend and record the
    /// substitution explicitly in the execution receipt's `fallback_outcome`.
    AllowTo(BackendKind),
}

/// Optional shadow-comparison outcome between the executed backend and the replacement backend.
///
/// Recorded explicitly whenever a shadow comparison was requested, even when the comparison
/// could not run, so an unavailable comparison is a visible non-green state rather than a
/// silently skipped one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowComparisonState {
    /// No shadow comparison was requested for this compilation.
    NotRequested,
    /// A shadow comparison was requested but could not run, with the concrete reason (for example, the active
    /// profile has no source-observable legacy/replacement comparator).
    Unavailable { reason: String },
    /// Both routes ran independently under one comparison profile and produced the same source-level observable.
    ///
    /// Two profile facts are recorded, and both are needed. `profile_kind` is the stable, human-meaningful kind
    /// of comparison that ran (for example `crate::backend::shadow::SHADOW_COMPARISON_PROFILE_ID`); a registry
    /// keyed on comparison capability links against it, and it survives every change to the compared source.
    /// `profile_identity` is the content identity of the exact instance — this source, this observed function,
    /// these arguments — so evidence names one comparison rather than a class of them. Recording only the hash
    /// would leave a consumer unable to say *what kind* of comparison it is looking at; recording only the kind
    /// would let two different comparisons claim the same evidence.
    ///
    /// `observable` is the compared value itself, not a summary of it, so a reader can see what agreement was
    /// claimed over without holding the comparison report.
    Matched {
        profile_kind: String,
        profile_identity: String,
        observable: String,
    },
    /// Both routes ran independently under one comparison profile and produced different source-level observables.
    ///
    /// Carries the same two profile facts as [`ShadowComparisonState::Matched`]. `detail` names both sides
    /// factually. This is a regression signal on the backend-selection axis, never a reason to fall back to the
    /// other backend.
    Diverged {
        profile_kind: String,
        profile_identity: String,
        detail: String,
    },
}

/// Produce the canonical shadow state for a request that could not run a source-observable comparison.
///
/// `reason` must name the concrete boundary that stopped the comparison, so an unavailable result stays a visible
/// non-green state with an actionable explanation rather than a generic "not supported". Callers pass
/// `shadow_requested` straight from the selection so the distinction between "nobody asked" and "asked and could
/// not run" is preserved: only the latter is a non-green outcome.
#[must_use]
pub fn unavailable_shadow_comparison(shadow_requested: bool, reason: &str) -> ShadowComparisonState {
    if shadow_requested {
        ShadowComparisonState::Unavailable {
            reason: reason.to_string(),
        }
    } else {
        ShadowComparisonState::NotRequested
    }
}

/// Whether the backend that actually executed differed from the one declared in the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackOutcome {
    /// The executed backend matches the selected backend; no fallback occurred.
    NotNeeded,
    /// The selected backend could not execute and [`FallbackPolicy::AllowTo`] authorized this
    /// substitution, which is recorded explicitly rather than left implicit.
    Declared { from: BackendKind, to: BackendKind },
}

/// Pre-execution declaration of which backend a compilation will use.
///
/// Built by [`select_backend`] before any codegen or execution starts. `identity` is a content
/// hash over every other field, so a later stage that only holds a serialized copy can call
/// [`BackendSelection::verify_identity`] instead of re-deriving trust from scratch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendSelection {
    /// Selection wire-schema version.
    pub schema_version: u32,
    /// Content-derived `sha256:` identity over every other field.
    pub identity: String,
    /// The backend declared for this compilation.
    pub selected_backend: BackendKind,
    /// Implementation revision of `selected_backend` at selection time.
    pub implementation_revision: u32,
    /// Compatibility coverage `selected_backend` declares for this compilation.
    pub compatibility_profile: CompatibilityProfile,
    /// Content-derived identity of the semantic/source input being compiled.
    pub source_identity: String,
    /// Why `selected_backend` was chosen.
    pub selection_reason: SelectionReason,
    /// What happens if `selected_backend` cannot execute.
    pub fallback_policy: FallbackPolicy,
    /// Whether a shadow comparison against the replacement backend was requested alongside
    /// `selected_backend`'s normal execution.
    pub shadow_requested: bool,
}

/// Post-execution record of how a declared [`BackendSelection`] was actually carried out.
///
/// Binds the backend that actually ran, any declared fallback, the shadow-comparison outcome,
/// the diagnostic-contract version in force, and the produced output's identity into one
/// versioned, content-identified record that Oven and other clients can consume without reading
/// private HIR or Body IR structures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendExecutionReceipt {
    /// Receipt wire-schema version.
    pub schema_version: u32,
    /// Content-derived `sha256:` identity over every other field, including the bound
    /// selection's own identity.
    pub identity: String,
    /// Compiler version that produced this receipt.
    pub compiler_version: String,
    /// The selection this receipt is bound to.
    pub selection: BackendSelection,
    /// The backend that actually executed. May differ from `selection.selected_backend` only
    /// when `fallback_outcome` is [`FallbackOutcome::Declared`].
    pub executed_backend: BackendKind,
    /// Shadow-comparison outcome, always present and explicit even when unavailable.
    pub shadow_comparison: ShadowComparisonState,
    /// Whether a declared fallback occurred.
    pub fallback_outcome: FallbackOutcome,
    /// Diagnostic-contract version in force when this receipt was produced.
    pub diagnostic_contract_version: u32,
    /// Content-derived identity of the produced output or artifact.
    pub output_identity: String,
    /// Session-owned semantic module used by this execution, when the backend consumed one directly.
    ///
    /// This remains absent for existing execution routes. Omitting it also preserves their historical receipt identity
    /// and schema-2 wire shape; a direct replacement execution binds it into the receipt identity below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_module: Option<SemanticModuleProvenance>,
}

/// Typed failure while selecting a backend or verifying a selection/receipt's content identity.
#[derive(Debug, thiserror::Error)]
pub enum BackendSelectionError {
    /// A persisted selection uses an unsupported schema version.
    #[error("unsupported backend-selection schema version {found}; expected {expected}")]
    UnsupportedSelectionSchema { found: u32, expected: u32 },
    /// A selection's claimed content identity did not match its immutable fields.
    #[error("backend-selection identity mismatch: expected {expected}, got {actual}")]
    SelectionIdentityMismatch { expected: String, actual: String },
    /// A persisted receipt uses an unsupported schema version.
    #[error("unsupported backend-execution-receipt schema version {found}; expected {expected}")]
    UnsupportedReceiptSchema { found: u32, expected: u32 },
    /// A receipt's claimed content identity did not match its immutable fields.
    #[error("backend-execution-receipt identity mismatch: expected {expected}, got {actual}")]
    ReceiptIdentityMismatch { expected: String, actual: String },
    /// The selected backend cannot execute and [`FallbackPolicy::Refuse`] applies, so the build
    /// must stop instead of silently substituting another backend.
    #[error(
        "backend `{backend:?}` was selected but cannot execute yet ({reason}); fallback policy is `refuse`, so the \
         build stops instead of silently running another backend"
    )]
    Refused {
        /// The backend that was selected but could not execute.
        backend: BackendKind,
        /// Concrete reason it could not execute.
        reason: String,
    },
    /// A caller attempted to record an executed backend that the selection did not authorize.
    #[error(
        "backend `{executed_backend:?}` cannot execute a `{selected_backend:?}` selection with fallback policy \
         `{fallback_policy:?}`"
    )]
    UndeclaredExecution {
        /// Backend declared by the selection.
        selected_backend: BackendKind,
        /// Backend the caller attempted to record as having executed.
        executed_backend: BackendKind,
        /// Policy that failed to authorize the recorded execution.
        fallback_policy: FallbackPolicy,
    },
    /// A persisted receipt's fallback outcome disagrees with its declared selection and execution.
    #[error("backend-execution receipt fallback outcome disagrees with its declared selection and executed backend")]
    ReceiptFallbackOutcomeMismatch,
}

/// Declare a backend selection for one compilation, before anything executes.
///
/// `explicit` distinguishes an operator-provided request (for example `incan build --backend
/// <kind>`) from the compiler-owned default, and is the sole input to `selection_reason`.
/// `shadow_requested` records whether a shadow comparison against the replacement backend was
/// also asked for. This function never fails: it only declares intent. Availability is checked
/// separately by [`resolve_execution`], right before dispatch.
#[must_use]
pub fn select_backend(
    requested: BackendKind,
    explicit: bool,
    shadow_requested: bool,
    source_identity: impl Into<String>,
    fallback_policy: FallbackPolicy,
) -> BackendSelection {
    let implementation_revision = match requested {
        BackendKind::Legacy => LEGACY_BACKEND_REVISION,
        BackendKind::Replacement => REPLACEMENT_BACKEND_REVISION,
    };
    let compatibility_profile = match requested {
        BackendKind::Legacy => CompatibilityProfile::Full,
        BackendKind::Replacement => CompatibilityProfile::Partial,
    };
    let selection_reason = if explicit {
        SelectionReason::ExplicitRequest
    } else {
        SelectionReason::Default
    };
    let source_identity = source_identity.into();
    let identity = selection_identity(
        requested,
        implementation_revision,
        compatibility_profile,
        &source_identity,
        selection_reason,
        fallback_policy,
        shadow_requested,
    );
    BackendSelection {
        schema_version: BACKEND_SELECTION_SCHEMA_VERSION,
        identity,
        selected_backend: requested,
        implementation_revision,
        compatibility_profile,
        source_identity,
        selection_reason,
        fallback_policy,
        shadow_requested,
    }
}

/// Decide which backend a caller must actually invoke for a declared selection.
///
/// `selected_available` reports whether `selection.selected_backend` can execute right now (in
/// practice, [`BackendKind::is_implemented`] on that backend). Returns the backend to invoke, or
/// [`BackendSelectionError::Refused`] when the selection is unavailable and
/// [`FallbackPolicy::Refuse`] applies, or when it is unavailable and the declared
/// [`FallbackPolicy::AllowTo`] target is *itself* unimplemented — a fallback to a backend that
/// cannot execute either must still refuse, not silently report the unavailable target as
/// executed. No receipt is produced on refusal: an unavailable selection must stop the build
/// rather than emit a receipt that could be mistaken for a successful replacement-backend result.
pub fn resolve_execution(
    selection: &BackendSelection,
    selected_available: bool,
) -> Result<BackendKind, BackendSelectionError> {
    if selected_available {
        return Ok(selection.selected_backend);
    }
    match selection.fallback_policy {
        FallbackPolicy::Refuse => Err(BackendSelectionError::Refused {
            backend: selection.selected_backend,
            reason: format!(
                "{:?} backend is not available for this compilation",
                selection.selected_backend
            ),
        }),
        FallbackPolicy::AllowTo(target) if target.is_implemented() => Ok(target),
        FallbackPolicy::AllowTo(target) => Err(BackendSelectionError::Refused {
            backend: target,
            reason: format!(
                "declared fallback target {target:?} is not available either; {:?} backend is also unavailable",
                selection.selected_backend
            ),
        }),
    }
}

/// Bind a real execution outcome to its declared selection, producing a versioned receipt.
///
/// `executed_backend` must be the value [`resolve_execution`] returned (or
/// `selection.selected_backend` itself, when it executed directly). This function validates that
/// the declared fallback policy authorizes any mismatch before creating a receipt, so callers
/// cannot turn an undeclared backend substitution into an identity-valid provenance record.
pub fn finalize_receipt(
    selection: &BackendSelection,
    executed_backend: BackendKind,
    output_identity: impl Into<String>,
    shadow_comparison: ShadowComparisonState,
    diagnostic_contract_version: u32,
) -> Result<BackendExecutionReceipt, BackendSelectionError> {
    finalize_receipt_with_semantic_module(
        selection,
        executed_backend,
        output_identity,
        shadow_comparison,
        diagnostic_contract_version,
        None,
    )
}

/// Bind a real execution outcome to its declared selection, optionally retaining its checked semantic-module authority.
///
/// The optional provenance exists for direct semantic consumers such as the replacement backend. Existing routes pass
/// `None`, retaining their historical receipt payload and identity exactly.
pub(crate) fn finalize_receipt_with_semantic_module(
    selection: &BackendSelection,
    executed_backend: BackendKind,
    output_identity: impl Into<String>,
    shadow_comparison: ShadowComparisonState,
    diagnostic_contract_version: u32,
    semantic_module: Option<SemanticModuleProvenance>,
) -> Result<BackendExecutionReceipt, BackendSelectionError> {
    selection.verify_identity()?;
    let output_identity = output_identity.into();
    let fallback_outcome = fallback_outcome_for_execution(selection, executed_backend)?;
    let compiler_version = crate::version::INCAN_VERSION.to_string();
    let identity = receipt_identity(ReceiptIdentityInputs {
        selection_identity: &selection.identity,
        compiler_version: &compiler_version,
        executed_backend,
        shadow_comparison: &shadow_comparison,
        fallback_outcome,
        diagnostic_contract_version,
        output_identity: &output_identity,
        semantic_module: semantic_module.as_ref(),
    });
    Ok(BackendExecutionReceipt {
        schema_version: BACKEND_SELECTION_SCHEMA_VERSION,
        identity,
        compiler_version,
        selection: selection.clone(),
        executed_backend,
        shadow_comparison,
        fallback_outcome,
        diagnostic_contract_version,
        output_identity,
        semantic_module,
    })
}

/// Derive the only fallback outcome a declared selection permits for one execution.
///
/// This is shared by receipt finalization and persisted-receipt verification so a receipt that
/// has a self-consistent content hash still cannot claim an execution the selection never
/// authorized.
fn fallback_outcome_for_execution(
    selection: &BackendSelection,
    executed_backend: BackendKind,
) -> Result<FallbackOutcome, BackendSelectionError> {
    if executed_backend == selection.selected_backend {
        return Ok(FallbackOutcome::NotNeeded);
    }
    match selection.fallback_policy {
        FallbackPolicy::AllowTo(target) if target == executed_backend => Ok(FallbackOutcome::Declared {
            from: selection.selected_backend,
            to: executed_backend,
        }),
        fallback_policy => Err(BackendSelectionError::UndeclaredExecution {
            selected_backend: selection.selected_backend,
            executed_backend,
            fallback_policy,
        }),
    }
}

impl BackendSelection {
    /// Recompute this selection's content identity and confirm it matches `self.identity`.
    ///
    /// A later stage that only holds a serialized `BackendSelection` (for example, one persisted
    /// alongside an Oven cache entry) must call this before trusting it, the same way
    /// [`crate::oven::OvenReceipt::verify_identity`] guards a persisted Oven receipt.
    pub fn verify_identity(&self) -> Result<(), BackendSelectionError> {
        if self.schema_version != BACKEND_SELECTION_SCHEMA_VERSION {
            return Err(BackendSelectionError::UnsupportedSelectionSchema {
                found: self.schema_version,
                expected: BACKEND_SELECTION_SCHEMA_VERSION,
            });
        }
        let actual = selection_identity(
            self.selected_backend,
            self.implementation_revision,
            self.compatibility_profile,
            &self.source_identity,
            self.selection_reason,
            self.fallback_policy,
            self.shadow_requested,
        );
        if actual != self.identity {
            return Err(BackendSelectionError::SelectionIdentityMismatch {
                expected: self.identity.clone(),
                actual,
            });
        }
        Ok(())
    }
}

impl BackendExecutionReceipt {
    /// Recompute this receipt's content identity (and its bound selection's identity) and
    /// confirm both match their recorded values.
    pub fn verify_identity(&self) -> Result<(), BackendSelectionError> {
        self.selection.verify_identity()?;
        if self.schema_version != BACKEND_SELECTION_SCHEMA_VERSION {
            return Err(BackendSelectionError::UnsupportedReceiptSchema {
                found: self.schema_version,
                expected: BACKEND_SELECTION_SCHEMA_VERSION,
            });
        }
        let actual = receipt_identity(ReceiptIdentityInputs {
            selection_identity: &self.selection.identity,
            compiler_version: &self.compiler_version,
            executed_backend: self.executed_backend,
            shadow_comparison: &self.shadow_comparison,
            fallback_outcome: self.fallback_outcome,
            diagnostic_contract_version: self.diagnostic_contract_version,
            output_identity: &self.output_identity,
            semantic_module: self.semantic_module.as_ref(),
        });
        if actual != self.identity {
            return Err(BackendSelectionError::ReceiptIdentityMismatch {
                expected: self.identity.clone(),
                actual,
            });
        }
        if self.fallback_outcome != fallback_outcome_for_execution(&self.selection, self.executed_backend)? {
            return Err(BackendSelectionError::ReceiptFallbackOutcomeMismatch);
        }
        Ok(())
    }
}

/// Digest the fields that make up a [`BackendSelection`]'s content identity.
fn selection_identity(
    selected_backend: BackendKind,
    implementation_revision: u32,
    compatibility_profile: CompatibilityProfile,
    source_identity: &str,
    selection_reason: SelectionReason,
    fallback_policy: FallbackPolicy,
    shadow_requested: bool,
) -> String {
    digest_content(&format!(
        "{selected_backend:?}\n{implementation_revision}\n{compatibility_profile:?}\n{source_identity}\n\
         {selection_reason:?}\n{fallback_policy:?}\n{shadow_requested}\n"
    ))
}

/// Immutable fields that determine a backend execution receipt's content identity.
///
/// Keeping these fields together avoids a fragile positional argument list at the receipt boundary while making the
/// optional semantic-module authority part of the same identity contract as the selection and execution evidence.
struct ReceiptIdentityInputs<'receipt> {
    selection_identity: &'receipt str,
    compiler_version: &'receipt str,
    executed_backend: BackendKind,
    shadow_comparison: &'receipt ShadowComparisonState,
    fallback_outcome: FallbackOutcome,
    diagnostic_contract_version: u32,
    output_identity: &'receipt str,
    semantic_module: Option<&'receipt SemanticModuleProvenance>,
}

/// Digest the fields that make up a [`BackendExecutionReceipt`]'s content identity.
///
/// Takes the bound selection's own `identity` rather than its full field set, so tampering with any selection field is
/// caught by [`BackendSelection::verify_identity`] and any tampering with the receipt's own fields (including swapping
/// in a different, validly-identified selection) is caught here.
fn receipt_identity(inputs: ReceiptIdentityInputs<'_>) -> String {
    let mut content = format!(
        "{}\n{}\n{:?}\n{:?}\n{:?}\n{}\n{}\n",
        inputs.selection_identity,
        inputs.compiler_version,
        inputs.executed_backend,
        inputs.shadow_comparison,
        inputs.fallback_outcome,
        inputs.diagnostic_contract_version,
        inputs.output_identity,
    );
    if let Some(semantic_module) = inputs.semantic_module {
        content.push_str(&format!(
            "semantic-module\n{}\n{}\n{}\n{}\n",
            semantic_module.module_id,
            semantic_module.module_path,
            semantic_module.source_identity,
            semantic_module.semantic_snapshot_identity,
        ));
    }
    digest_content(&content)
}

/// Render a `sha256:`-prefixed hex digest of `content`.
///
/// Delegates to [`crate::oven::digest_bytes`] rather than hashing independently, so
/// `BackendSelection`/`BackendExecutionReceipt` identities share exactly one hashing
/// implementation with `OvenReceipt` identities instead of two that could drift apart.
fn digest_content(content: &str) -> String {
    crate::oven::digest_bytes(content.as_bytes())
}

/// Digest one or more content fragments into a single content-derived identity.
///
/// Exposed for callers that need to turn real source or generated-output text into a
/// `source_identity` or `output_identity` without duplicating the hashing scheme this module
/// uses internally. `parts` must be presented in a stable, caller-chosen order (for example,
/// sorted by module path) so the identity does not depend on incidental iteration order, such as
/// `HashMap` iteration over multi-file codegen output.
#[must_use]
pub fn digest_output(parts: &[&str]) -> String {
    digest_content(&parts.join("\u{1}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refuse_selection(requested: BackendKind, explicit: bool) -> BackendSelection {
        select_backend(requested, explicit, false, "sha256:source", FallbackPolicy::Refuse)
    }

    #[test]
    fn declared_legacy_capability_selection_is_explicit() -> Result<(), BackendSelectionError> {
        let selection = refuse_selection(BackendKind::Legacy, false);
        assert_eq!(selection.selected_backend, BackendKind::Legacy);
        assert_eq!(selection.selection_reason, SelectionReason::Default);
        assert_eq!(selection.compatibility_profile, CompatibilityProfile::Full);
        assert_eq!(selection.implementation_revision, LEGACY_BACKEND_REVISION);
        selection.verify_identity()?;

        let executed = resolve_execution(&selection, true)?;
        assert_eq!(executed, BackendKind::Legacy);

        let receipt = finalize_receipt(
            &selection,
            executed,
            "sha256:output",
            ShadowComparisonState::NotRequested,
            1,
        )?;
        assert_eq!(receipt.fallback_outcome, FallbackOutcome::NotNeeded);
        assert_eq!(receipt.executed_backend, BackendKind::Legacy);
        receipt.verify_identity()?;
        Ok(())
    }

    #[test]
    fn replacement_selection_refuses_visibly_when_unavailable() {
        let selection = refuse_selection(BackendKind::Replacement, true);
        assert_eq!(selection.selection_reason, SelectionReason::ExplicitRequest);
        assert_eq!(selection.compatibility_profile, CompatibilityProfile::Partial);

        let Err(error) = resolve_execution(&selection, false) else {
            panic!("an unavailable replacement source profile must be refused");
        };
        match error {
            BackendSelectionError::Refused { backend, .. } => assert_eq!(backend, BackendKind::Replacement),
            other => panic!("expected Refused, got {other:?}"),
        }
    }

    #[test]
    fn fallback_refusal_never_produces_a_receipt() {
        // `resolve_execution` returns `Err` for a refused fallback, and `finalize_receipt` rejects
        // any undeclared substitution independently. A refused build cannot be mistaken for a
        // green replacement-backend result.
        let selection = refuse_selection(BackendKind::Replacement, true);
        assert!(resolve_execution(&selection, false).is_err());
    }

    #[test]
    fn declared_fallback_is_recorded_explicitly() -> Result<(), BackendSelectionError> {
        let selection = select_backend(
            BackendKind::Replacement,
            true,
            false,
            "sha256:source",
            FallbackPolicy::AllowTo(BackendKind::Legacy),
        );
        let executed = resolve_execution(&selection, false)?;
        assert_eq!(executed, BackendKind::Legacy);

        let receipt = finalize_receipt(
            &selection,
            executed,
            "sha256:output",
            ShadowComparisonState::NotRequested,
            1,
        )?;
        assert_eq!(
            receipt.fallback_outcome,
            FallbackOutcome::Declared {
                from: BackendKind::Replacement,
                to: BackendKind::Legacy,
            }
        );
        receipt.verify_identity()?;
        Ok(())
    }

    #[test]
    fn declared_fallback_to_the_available_replacement_target_resolves_explicitly() -> Result<(), BackendSelectionError>
    {
        // #988 makes the replacement implementation available for its declared partial profile. A caller that has
        // independently found the initially selected execution unavailable may therefore resolve an explicitly
        // declared replacement target, but source-profile validation still belongs to the replacement executor.
        let selection = select_backend(
            BackendKind::Replacement,
            true,
            false,
            "sha256:source",
            FallbackPolicy::AllowTo(BackendKind::Replacement),
        );
        let resolved = resolve_execution(&selection, false)?;
        assert_eq!(resolved, BackendKind::Replacement);
        Ok(())
    }

    #[test]
    fn shadow_comparison_unavailability_is_recorded_not_skipped() -> Result<(), BackendSelectionError> {
        let selection = select_backend(BackendKind::Legacy, true, true, "sha256:source", FallbackPolicy::Refuse);
        assert!(selection.shadow_requested);

        let executed = resolve_execution(&selection, true)?;
        let shadow_comparison = unavailable_shadow_comparison(selection.shadow_requested, "no comparator staged");
        let receipt = finalize_receipt(&selection, executed, "sha256:output", shadow_comparison.clone(), 1)?;
        assert_eq!(receipt.shadow_comparison, shadow_comparison);
        receipt.verify_identity()?;
        Ok(())
    }

    #[test]
    fn an_unrequested_shadow_comparison_stays_distinct_from_an_unavailable_one() {
        assert_eq!(
            unavailable_shadow_comparison(false, "no comparator staged"),
            ShadowComparisonState::NotRequested
        );
    }

    #[test]
    fn a_recorded_comparison_outcome_is_covered_by_the_receipt_identity() -> Result<(), BackendSelectionError> {
        let selection = select_backend(
            BackendKind::Replacement,
            true,
            true,
            "sha256:source",
            FallbackPolicy::Refuse,
        );
        let executed = resolve_execution(&selection, true)?;
        let matched = ShadowComparisonState::Matched {
            profile_kind: "incan.shadow_comparison.example.v0".to_string(),
            profile_identity: "sha256:profile".to_string(),
            observable: "completed(42)".to_string(),
        };
        let mut receipt = finalize_receipt(&selection, executed, "sha256:output", matched.clone(), 1)?;
        receipt.verify_identity()?;
        assert_eq!(receipt.shadow_comparison, matched);

        // Rewriting an agreed comparison into a divergence claim (or the reverse) without recomputing the
        // identity is exactly the tampering a receipt-bound comparison has to make visible.
        receipt.shadow_comparison = ShadowComparisonState::Diverged {
            profile_kind: "incan.shadow_comparison.example.v0".to_string(),
            profile_identity: "sha256:profile".to_string(),
            detail: "invented".to_string(),
        };
        let Err(error) = receipt.verify_identity() else {
            panic!("a rewritten shadow-comparison outcome must be detected");
        };
        assert!(matches!(error, BackendSelectionError::ReceiptIdentityMismatch { .. }));
        Ok(())
    }

    #[test]
    fn semantic_module_provenance_is_bound_to_receipt_identity() -> Result<(), Box<dyn std::error::Error>> {
        let selection = select_backend(
            BackendKind::Replacement,
            true,
            false,
            "sha256:source",
            FallbackPolicy::Refuse,
        );
        let executed = resolve_execution(&selection, true)?;
        let legacy_receipt = finalize_receipt(
            &selection,
            executed,
            "sha256:output",
            ShadowComparisonState::NotRequested,
            1,
        )?;
        let legacy_payload = serde_json::to_value(&legacy_receipt)?;
        assert!(
            legacy_payload
                .as_object()
                .is_some_and(|receipt| !receipt.contains_key("semantic_module")),
            "existing execution receipts must retain their historical wire shape"
        );

        let mut receipt = finalize_receipt_with_semantic_module(
            &selection,
            executed,
            "sha256:output",
            ShadowComparisonState::NotRequested,
            1,
            Some(SemanticModuleProvenance::new(
                "module:main".to_string(),
                "main".to_string(),
                "sha256:source".to_string(),
                "sha256:semantic-snapshot".to_string(),
            )),
        )?;
        receipt.verify_identity()?;

        let semantic_module = receipt
            .semantic_module
            .as_mut()
            .ok_or("expected direct replacement receipt provenance")?;
        semantic_module.semantic_snapshot_identity = "sha256:tampered-snapshot".to_string();
        let Err(error) = receipt.verify_identity() else {
            return Err("semantic-module receipt tampering must be detected".into());
        };
        assert!(matches!(error, BackendSelectionError::ReceiptIdentityMismatch { .. }));
        Ok(())
    }

    #[test]
    fn mismatched_receipt_identity_is_detected() -> Result<(), BackendSelectionError> {
        let selection = refuse_selection(BackendKind::Legacy, false);
        let mut receipt = finalize_receipt(
            &selection,
            BackendKind::Legacy,
            "sha256:output",
            ShadowComparisonState::NotRequested,
            1,
        )?;
        receipt.verify_identity()?;

        // Tamper with the recorded output identity without recomputing `receipt.identity`, the
        // same class of divergence a stale or hand-edited receipt on disk would exhibit.
        receipt.output_identity = "sha256:tampered".to_string();
        let Err(error) = receipt.verify_identity() else {
            panic!("tampered output identity must be detected");
        };
        assert!(matches!(error, BackendSelectionError::ReceiptIdentityMismatch { .. }));
        Ok(())
    }

    #[test]
    fn mismatched_selection_identity_is_detected() {
        let mut selection = refuse_selection(BackendKind::Legacy, false);
        selection.source_identity = "sha256:different-source".to_string();
        let Err(error) = selection.verify_identity() else {
            panic!("tampered source identity must be detected");
        };
        assert!(matches!(error, BackendSelectionError::SelectionIdentityMismatch { .. }));
    }

    #[test]
    fn receipt_finalization_rejects_an_undeclared_execution() {
        let refusal = refuse_selection(BackendKind::Legacy, false);
        let Err(error) = finalize_receipt(
            &refusal,
            BackendKind::Replacement,
            "sha256:output",
            ShadowComparisonState::NotRequested,
            1,
        ) else {
            panic!("a refusal policy must not record a different executed backend");
        };
        assert!(matches!(
            error,
            BackendSelectionError::UndeclaredExecution {
                selected_backend: BackendKind::Legacy,
                executed_backend: BackendKind::Replacement,
                fallback_policy: FallbackPolicy::Refuse,
            }
        ));

        let declared_legacy = select_backend(
            BackendKind::Legacy,
            true,
            false,
            "sha256:source",
            FallbackPolicy::AllowTo(BackendKind::Legacy),
        );
        let Err(error) = finalize_receipt(
            &declared_legacy,
            BackendKind::Replacement,
            "sha256:output",
            ShadowComparisonState::NotRequested,
            1,
        ) else {
            panic!("a fallback policy must name the only backend it authorizes");
        };
        assert!(matches!(
            error,
            BackendSelectionError::UndeclaredExecution {
                selected_backend: BackendKind::Legacy,
                executed_backend: BackendKind::Replacement,
                fallback_policy: FallbackPolicy::AllowTo(BackendKind::Legacy),
            }
        ));
    }

    #[test]
    fn receipt_verification_rejects_a_self_consistent_undeclared_execution() -> Result<(), BackendSelectionError> {
        let selection = refuse_selection(BackendKind::Legacy, false);
        let mut receipt = finalize_receipt(
            &selection,
            BackendKind::Legacy,
            "sha256:output",
            ShadowComparisonState::NotRequested,
            1,
        )?;
        receipt.executed_backend = BackendKind::Replacement;
        receipt.fallback_outcome = FallbackOutcome::Declared {
            from: BackendKind::Legacy,
            to: BackendKind::Replacement,
        };
        receipt.identity = receipt_identity(ReceiptIdentityInputs {
            selection_identity: &receipt.selection.identity,
            compiler_version: &receipt.compiler_version,
            executed_backend: receipt.executed_backend,
            shadow_comparison: &receipt.shadow_comparison,
            fallback_outcome: receipt.fallback_outcome,
            diagnostic_contract_version: receipt.diagnostic_contract_version,
            output_identity: &receipt.output_identity,
            semantic_module: receipt.semantic_module.as_ref(),
        });

        let Err(error) = receipt.verify_identity() else {
            panic!("identity verification must reject an undeclared execution even with a matching hash");
        };
        assert!(matches!(error, BackendSelectionError::UndeclaredExecution { .. }));
        Ok(())
    }
}
