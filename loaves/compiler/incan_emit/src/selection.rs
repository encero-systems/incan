//! Backend-selection identity and execution receipt: which route produced a build's output.
//!
//! Incan has one execution route. Today it is the Rust-emission backend (`IrCodegen`, the `incan_ir` and `incan_emit`
//! crates), named `legacy` here; the direct route of #1337, which lowers Body IR inside the pinned `rustc`, replaces it
//! as that one route. A build still declares its backend before it runs and records the backend that ran, so a reader
//! of a persisted receipt can tell which route produced the output beside it, including after a completed-output reuse.
//!
//! This module owns two boundary types:
//!
//! - [`BackendSelection`]: a versioned, content-identified record of what was decided *before* execution — which
//!   backend, at what implementation revision, for what source input.
//! - [`BackendExecutionReceipt`]: a versioned, content-identified record of what happened *after* execution — the
//!   backend that ran, the diagnostic-contract version in force, and the produced output's identity.
//!
//! Both are plain data with no I/O: this module does not invoke `IrCodegen` or any other execution machinery. Callers
//! (the build path) use [`select_backend`] and [`finalize_receipt`] to turn a real outcome into these records, which
//! keeps the boundary testable without a compilation pipeline and readable by clients (Oven, `incan inspect`) that
//! only hold a serialized receipt.
//!
//! This is a different axis from Oven's legacy-Cargo-versus-direct-rustc *build* boundary (`oven_store`'s
//! `OvenCompatibilityKind`), which selects how an already generated artifact is compiled and never decides which
//! backend produced it.

use serde::{Deserialize, Serialize};

/// Current wire format for [`BackendSelection`] and [`BackendExecutionReceipt`].
///
/// Version `3` is the one-route shape: the selection names the backend, its revision and the source, and the receipt
/// adds the backend that ran, the diagnostic contract and the output. Version `2` also carried a selection reason, a
/// compatibility profile, a fallback policy and outcome and a shadow-comparison state, which existed only while a
/// second, partial backend could be requested; a version-2 receipt no longer verifies, so a completed output that
/// carries one is rebuilt rather than reused.
pub const BACKEND_SELECTION_SCHEMA_VERSION: u32 = 3;

/// Implementation revision of the current Rust-emission ("legacy") backend.
///
/// Independent of [`incan_lang::version::INCAN_VERSION`]: increase it only when a change to the Rust-emission pipeline
/// can change generated output for a previously accepted program, so a consumer keying reuse on this revision knows to
/// invalidate.
pub const LEGACY_BACKEND_REVISION: u32 = 1;

/// The compiler backend that produced a build's output.
///
/// This is the codegen-execution axis tracked by #652/#986. See the module docs for how it differs from Oven's
/// build-compilation boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// The Rust-source-emission pipeline (`IrCodegen`; the `incan_ir` and `incan_emit` crates).
    Legacy,
}

impl BackendKind {
    /// The implementation revision a selection of this backend records.
    #[must_use]
    pub fn implementation_revision(self) -> u32 {
        match self {
            BackendKind::Legacy => LEGACY_BACKEND_REVISION,
        }
    }
}

/// Pre-execution declaration of which backend a compilation uses.
///
/// Built by [`select_backend`] before code generation starts. `identity` is a content hash over every other field, so
/// a later stage that holds only a serialized copy can call [`BackendSelection::verify_identity`] instead of
/// re-deriving trust from scratch.
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
    /// Content-derived identity of the semantic/source input being compiled.
    pub source_identity: String,
}

/// Post-execution record of how a declared [`BackendSelection`] was carried out.
///
/// Binds the backend that ran, the diagnostic-contract version in force and the produced output's identity into one
/// versioned, content-identified record that Oven and other clients can consume without reading private HIR or Body IR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendExecutionReceipt {
    /// Receipt wire-schema version.
    pub schema_version: u32,
    /// Content-derived `sha256:` identity over every other field, including the bound selection's own identity.
    pub identity: String,
    /// Compiler version that produced this receipt.
    pub compiler_version: String,
    /// The selection this receipt is bound to.
    pub selection: BackendSelection,
    /// The backend that ran. It is always the selected backend: there is no other route to fall back to.
    pub executed_backend: BackendKind,
    /// Diagnostic-contract version in force when this receipt was produced.
    pub diagnostic_contract_version: u32,
    /// Content-derived identity of the produced output or artifact.
    pub output_identity: String,
}

/// Typed failure while verifying a persisted selection or receipt.
#[derive(Debug, thiserror::Error)]
pub enum BackendSelectionError {
    /// A persisted selection uses an unsupported schema version.
    #[error("unsupported backend-selection schema version {found}; expected {expected}")]
    UnsupportedSelectionSchema { found: u32, expected: u32 },
    /// A selection's claimed content identity did not match its immutable fields.
    #[error("backend-selection identity mismatch: expected {expected}, got {actual}")]
    SelectionIdentityMismatch { expected: String, actual: String },
    /// A selection records an implementation revision its backend does not have.
    #[error("backend `{backend:?}` selection records revision {found}; this compiler's revision is {expected}")]
    ImplementationRevisionMismatch {
        /// The selected backend.
        backend: BackendKind,
        /// The revision the selection records.
        found: u32,
        /// The revision this compiler's backend has.
        expected: u32,
    },
    /// A persisted receipt uses an unsupported schema version.
    #[error("unsupported backend-execution-receipt schema version {found}; expected {expected}")]
    UnsupportedReceiptSchema { found: u32, expected: u32 },
    /// A receipt's claimed content identity did not match its immutable fields.
    #[error("backend-execution-receipt identity mismatch: expected {expected}, got {actual}")]
    ReceiptIdentityMismatch { expected: String, actual: String },
    /// A receipt records a backend that ran other than the one its selection declared.
    #[error("backend `{executed:?}` cannot have executed a `{selected:?}` selection")]
    ExecutedBackendMismatch {
        /// Backend declared by the selection.
        selected: BackendKind,
        /// Backend the receipt records as having run.
        executed: BackendKind,
    },
}

/// Declare the backend for one compilation, before anything executes.
#[must_use]
pub fn select_backend(backend: BackendKind, source_identity: impl Into<String>) -> BackendSelection {
    let implementation_revision = backend.implementation_revision();
    let source_identity = source_identity.into();
    let identity = selection_identity(backend, implementation_revision, &source_identity);
    BackendSelection {
        schema_version: BACKEND_SELECTION_SCHEMA_VERSION,
        identity,
        selected_backend: backend,
        implementation_revision,
        source_identity,
    }
}

/// Bind a real execution outcome to its declared selection, producing a versioned receipt.
///
/// The selected backend is the one that ran; `output_identity` is the identity of what it produced.
pub fn finalize_receipt(
    selection: &BackendSelection,
    output_identity: impl Into<String>,
    diagnostic_contract_version: u32,
) -> Result<BackendExecutionReceipt, BackendSelectionError> {
    selection.verify_identity()?;
    let output_identity = output_identity.into();
    let compiler_version = incan_lang::version::INCAN_VERSION.to_string();
    let executed_backend = selection.selected_backend;
    let identity = receipt_identity(
        &selection.identity,
        &compiler_version,
        executed_backend,
        diagnostic_contract_version,
        &output_identity,
    );
    Ok(BackendExecutionReceipt {
        schema_version: BACKEND_SELECTION_SCHEMA_VERSION,
        identity,
        compiler_version,
        selection: selection.clone(),
        executed_backend,
        diagnostic_contract_version,
        output_identity,
    })
}

impl BackendSelection {
    /// Recompute this selection's content identity and confirm it matches `self.identity`.
    ///
    /// A later stage that holds only a serialized `BackendSelection` (for example, one persisted alongside an Oven
    /// cache entry) must call this before trusting it, the same way `oven_store::OvenReceipt::verify_identity` guards a
    /// persisted Oven receipt. A selection made by another revision of the backend is refused too: its output was not
    /// produced by the backend this compiler runs.
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
            &self.source_identity,
        );
        if actual != self.identity {
            return Err(BackendSelectionError::SelectionIdentityMismatch {
                expected: self.identity.clone(),
                actual,
            });
        }
        let expected = self.selected_backend.implementation_revision();
        if self.implementation_revision != expected {
            return Err(BackendSelectionError::ImplementationRevisionMismatch {
                backend: self.selected_backend,
                found: self.implementation_revision,
                expected,
            });
        }
        Ok(())
    }
}

impl BackendExecutionReceipt {
    /// Recompute this receipt's content identity (and its bound selection's identity) and confirm both match their
    /// recorded values.
    pub fn verify_identity(&self) -> Result<(), BackendSelectionError> {
        self.selection.verify_identity()?;
        if self.schema_version != BACKEND_SELECTION_SCHEMA_VERSION {
            return Err(BackendSelectionError::UnsupportedReceiptSchema {
                found: self.schema_version,
                expected: BACKEND_SELECTION_SCHEMA_VERSION,
            });
        }
        let actual = receipt_identity(
            &self.selection.identity,
            &self.compiler_version,
            self.executed_backend,
            self.diagnostic_contract_version,
            &self.output_identity,
        );
        if actual != self.identity {
            return Err(BackendSelectionError::ReceiptIdentityMismatch {
                expected: self.identity.clone(),
                actual,
            });
        }
        if self.executed_backend != self.selection.selected_backend {
            return Err(BackendSelectionError::ExecutedBackendMismatch {
                selected: self.selection.selected_backend,
                executed: self.executed_backend,
            });
        }
        Ok(())
    }
}

/// Digest the fields that make up a [`BackendSelection`]'s content identity.
fn selection_identity(selected_backend: BackendKind, implementation_revision: u32, source_identity: &str) -> String {
    digest_content(&format!(
        "{selected_backend:?}\n{implementation_revision}\n{source_identity}\n"
    ))
}

/// Digest the fields that make up a [`BackendExecutionReceipt`]'s content identity.
///
/// Takes the bound selection's own `identity` rather than its full field set, so tampering with any selection field is
/// caught by [`BackendSelection::verify_identity`] and any tampering with the receipt's own fields (including swapping
/// in a different, validly identified selection) is caught here.
fn receipt_identity(
    selection_identity: &str,
    compiler_version: &str,
    executed_backend: BackendKind,
    diagnostic_contract_version: u32,
    output_identity: &str,
) -> String {
    digest_content(&format!(
        "{selection_identity}\n{compiler_version}\n{executed_backend:?}\n{diagnostic_contract_version}\n{output_identity}\n"
    ))
}

/// Render a `sha256:`-prefixed hex digest of `content`.
///
/// Delegates to [`oven_model::digest::digest_bytes`] rather than hashing independently, so
/// `BackendSelection`/`BackendExecutionReceipt` identities share exactly one hashing implementation with `OvenReceipt`
/// identities instead of two that could drift apart.
fn digest_content(content: &str) -> String {
    oven_model::digest::digest_bytes(content.as_bytes())
}

/// Digest one or more content fragments into a single content-derived identity.
///
/// Exposed for callers that need to turn real source or generated-output text into a `source_identity` or
/// `output_identity` without duplicating the hashing scheme this module uses internally. `parts` must be presented in a
/// stable, caller-chosen order (for example, sorted by module path) so the identity does not depend on incidental
/// iteration order, such as `HashMap` iteration over multi-file codegen output.
#[must_use]
pub fn digest_output(parts: &[&str]) -> String {
    digest_content(&parts.join("\u{1}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), BackendSelectionError>;

    /// A selection records the backend, its revision and the source, and verifies.
    #[test]
    fn a_selection_records_the_backend_its_revision_and_the_source() -> TestResult {
        let selection = select_backend(BackendKind::Legacy, "sha256:source");
        assert_eq!(selection.schema_version, BACKEND_SELECTION_SCHEMA_VERSION);
        assert_eq!(selection.selected_backend, BackendKind::Legacy);
        assert_eq!(selection.implementation_revision, LEGACY_BACKEND_REVISION);
        assert_eq!(selection.source_identity, "sha256:source");
        selection.verify_identity()
    }

    /// A receipt names the selected backend as the one that ran, and verifies.
    #[test]
    fn a_receipt_names_the_selected_backend_as_the_one_that_ran() -> TestResult {
        let selection = select_backend(BackendKind::Legacy, "sha256:source");
        let receipt = finalize_receipt(&selection, "sha256:output", 2)?;
        assert_eq!(receipt.executed_backend, BackendKind::Legacy);
        assert_eq!(receipt.output_identity, "sha256:output");
        assert_eq!(receipt.diagnostic_contract_version, 2);
        assert_eq!(receipt.selection, selection);
        receipt.verify_identity()
    }

    /// The receipt's wire shape is the one-route shape: no fallback, shadow or selection-reason fields.
    #[test]
    fn the_receipt_wire_shape_has_no_second_route_fields() -> Result<(), Box<dyn std::error::Error>> {
        let selection = select_backend(BackendKind::Legacy, "sha256:source");
        let receipt = finalize_receipt(&selection, "sha256:output", 2)?;
        let json = serde_json::to_value(&receipt)?;
        for absent in ["fallback_outcome", "shadow_comparison", "semantic_module"] {
            assert!(json.get(absent).is_none(), "{absent} must not be serialized");
        }
        for absent in [
            "selection_reason",
            "fallback_policy",
            "shadow_requested",
            "compatibility_profile",
        ] {
            assert!(
                json["selection"].get(absent).is_none(),
                "{absent} must not be serialized"
            );
        }
        assert_eq!(json["executed_backend"], "legacy");
        Ok(())
    }

    /// Changing any recorded field after the fact is detected.
    #[test]
    fn tampering_with_a_receipt_or_its_selection_is_detected() -> TestResult {
        let selection = select_backend(BackendKind::Legacy, "sha256:source");
        let receipt = finalize_receipt(&selection, "sha256:output", 2)?;

        let mut output = receipt.clone();
        output.output_identity = "sha256:other".to_string();
        assert!(matches!(
            output.verify_identity(),
            Err(BackendSelectionError::ReceiptIdentityMismatch { .. })
        ));

        let mut source = receipt.clone();
        source.selection.source_identity = "sha256:other".to_string();
        assert!(matches!(
            source.verify_identity(),
            Err(BackendSelectionError::SelectionIdentityMismatch { .. })
        ));

        let mut schema = receipt;
        schema.schema_version = 2;
        assert!(matches!(
            schema.verify_identity(),
            Err(BackendSelectionError::UnsupportedReceiptSchema { found: 2, .. })
        ));
        Ok(())
    }

    /// A selection recorded by another revision of the backend does not verify, even when its identity is consistent.
    #[test]
    fn a_selection_from_another_backend_revision_is_refused() {
        let implementation_revision = LEGACY_BACKEND_REVISION + 1;
        let selection = BackendSelection {
            schema_version: BACKEND_SELECTION_SCHEMA_VERSION,
            identity: selection_identity(BackendKind::Legacy, implementation_revision, "sha256:source"),
            selected_backend: BackendKind::Legacy,
            implementation_revision,
            source_identity: "sha256:source".to_string(),
        };
        assert!(matches!(
            selection.verify_identity(),
            Err(BackendSelectionError::ImplementationRevisionMismatch { .. })
        ));
    }

    /// Output identities are deterministic over their parts and their order.
    #[test]
    fn output_identity_is_deterministic_over_its_parts() {
        assert_eq!(digest_output(&["a", "b"]), digest_output(&["a", "b"]));
        assert_ne!(digest_output(&["a", "b"]), digest_output(&["b", "a"]));
        assert!(digest_output(&["a"]).starts_with("sha256:"));
    }
}
