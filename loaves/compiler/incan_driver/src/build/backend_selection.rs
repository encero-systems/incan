//! Choosing and recording the code-generation backend for one build.

use std::fs;
use std::path::{Path, PathBuf};

use crate::backend::selection::{
    BackendKind, BackendSelection, FallbackOutcome, ShadowComparisonState, finalize_receipt, resolve_execution,
    select_backend, unavailable_shadow_comparison,
};
use crate::build::BackendSelectionOptions;
use crate::build::rust_extern::module_source_identity;
use crate::error::{CliError, CliResult};
use incan_frontend::{ParsedModule, diagnostics};

/// Why a shadow comparison is unavailable: the Body IR interpreter it compared against is removed (#1337).
const SHADOW_COMPARISON_REMOVED_REASON: &str =
    "the compiler has one execution route; the Body IR interpreter a shadow comparison ran against is removed";

/// Shadow-comparison state for one build's backend execution receipt.
///
/// No build can request a comparison any more, so this is `NotRequested`; the receipt keeps the field until its
/// schema collapses to the one route.
pub fn backend_shadow_comparison(selection: &BackendSelection) -> ShadowComparisonState {
    unavailable_shadow_comparison(selection.shadow_requested, SHADOW_COMPARISON_REMOVED_REASON)
}

/// Declare and resolve a backend selection for one build, before codegen runs (#986).
///
/// Combines [`select_backend`] and [`resolve_execution`] — identical at both the executable (`prepare_oven_project`)
/// and library (`prepare_library_project`) call sites — into the one step callers actually need: a declared selection
/// plus the backend they must invoke, or a visible refusal.
pub fn select_and_resolve_backend(
    backend_options: &BackendSelectionOptions,
    modules: &[ParsedModule],
) -> CliResult<(BackendSelection, BackendKind)> {
    let selection = select_backend(
        backend_options.requested,
        backend_options.explicit,
        backend_options.shadow,
        module_source_identity(modules),
        backend_options.fallback_policy,
    );
    let executed = resolve_execution(&selection, selection.selected_backend.is_implemented())
        .map_err(|error| CliError::failure(error.to_string()))?;
    Ok((selection, executed))
}

/// Bind a real output identity to a resolved backend selection and surface any declared fallback (#986). Combines
/// [`finalize_receipt`], [`backend_shadow_comparison`], and [`report_backend_fallback`] — identical at both build-path
/// call sites — into one step.
pub fn finalize_backend_receipt(
    selection: &BackendSelection,
    executed: BackendKind,
    output_identity: String,
) -> CliResult<crate::backend::selection::BackendExecutionReceipt> {
    let receipt = finalize_receipt(
        selection,
        executed,
        output_identity,
        backend_shadow_comparison(selection),
        diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
    )
    .map_err(|error| CliError::failure(error.to_string()))?;
    report_backend_fallback(&receipt);
    Ok(receipt)
}

/// Surface a declared backend fallback on stderr, visible even when `--report` is not requested.
fn report_backend_fallback(receipt: &crate::backend::selection::BackendExecutionReceipt) {
    if let FallbackOutcome::Declared { from, to } = receipt.fallback_outcome {
        eprintln!(
            "⚠ backend fallback: `{from:?}` was selected but is not available; executed `{to:?}` instead (declared, not silent)"
        );
    }
}

/// Compiler-owned, project-relative destination for a build's backend-selection execution receipt.
///
/// Kept separate from `oven_store::DEFAULT_RECEIPT_RELATIVE_PATH`: the Oven receipt is the build-unit/native-plan
/// boundary, while this receipt is the backend-selection/execution boundary (#986). Oven and other clients consume this
/// without reading private HIR/Body IR.
const DEFAULT_BACKEND_RECEIPT_RELATIVE_PATH: &str = ".incan/backend/receipt.json";

/// Return the compiler-owned project-relative destination for a backend-selection execution receipt.
pub fn default_backend_receipt_path(project_root: &Path) -> PathBuf {
    project_root.join(DEFAULT_BACKEND_RECEIPT_RELATIVE_PATH)
}

/// Publish a backend-selection execution receipt through a same-directory staged file and atomic replacement, mirroring
/// `oven_store::write_receipt`'s durability guarantee for its own receipt.
pub fn write_backend_receipt(
    receipt: &crate::backend::selection::BackendExecutionReceipt,
    path: &Path,
) -> CliResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| CliError::failure(format!("invalid backend-selection receipt path {}", path.display())))?;
    fs::create_dir_all(parent)
        .map_err(|error| CliError::failure(format!("failed to create {}: {error}", parent.display())))?;
    let payload = serde_json::to_vec_pretty(receipt)
        .map_err(|error| CliError::failure(format!("failed to serialize backend-selection receipt: {error}")))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CliError::failure(format!("invalid backend-selection receipt path {}", path.display())))?;
    let staged_path = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));
    let result = oven_store::write_receipt_staged(&payload, &staged_path, path, parent);
    if result.is_err() && staged_path.exists() {
        let _ = fs::remove_file(&staged_path);
    }
    result.map_err(|error| {
        CliError::failure(format!(
            "failed to publish backend-selection receipt {}: {error}",
            path.display()
        ))
    })
}
