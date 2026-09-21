//! Choosing and recording the code-generation backend for one build, and refusing replacement profiles the
//! selected backend cannot honor.

use std::fs;
use std::path::{Path, PathBuf};

use crate::backend::replacement::ReplacementExecutionError;
use crate::backend::selection::{
    BackendKind, BackendSelection, FallbackOutcome, ShadowComparisonState, finalize_receipt, resolve_execution,
    select_backend, unavailable_shadow_comparison,
};
use crate::backend::shadow::PROGRAM_ENTRYPOINT_UNAVAILABLE_REASON;
use crate::build::BackendSelectionOptions;
use crate::build::replacement::ReplacementModuleInputs;
use crate::build::rust_extern::module_source_identity;
use crate::error::{CliError, CliResult};
use incan_frontend::{ParsedModule, diagnostics};

/// Shadow-comparison state for one build's backend execution receipt.
///
/// #1146 implements a real source-observable comparison, but only for the bounded profile in
/// `crate::backend::shadow`: one module, one named free function that is not the program entrypoint, and concrete
/// scalar arguments. Every build path observes the module's `main` instead, whose return value the produced
/// process does not expose, so a requested comparison stays explicitly `Unavailable` with that reason rather than
/// silently `NotRequested` or inferred from generated Rust.
pub fn backend_shadow_comparison(selection: &BackendSelection) -> ShadowComparisonState {
    unavailable_shadow_comparison(selection.shadow_requested, PROGRAM_ENTRYPOINT_UNAVAILABLE_REASON)
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

/// Stable schema marker for the direct Body-IR replacement execution report.
///
/// This is distinct from the Oven build-report schema because this path has no generated Rust, artifacts, or Oven
/// plan to report. Consumers must inspect its backend receipt and direct-execution evidence rather than treating it
/// as a partial legacy build report.
pub const REPLACEMENT_EXECUTION_REPORT_SCHEMA_VERSION: &str = "incan.replacement_execution.v1";

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

/// Convert a typed replacement refusal into the CLI's stable source-location presentation.
///
/// `CliError` predates typed frontend diagnostics and carries display text only, so this adapter retains the
/// replacement diagnostic code, entrypoint path, and original Body-IR span rather than discarding them at the CLI
/// boundary.
pub fn replacement_profile_cli_error(error: ReplacementExecutionError, entrypoint: &Path) -> CliError {
    match error.primary_span() {
        Some(span) => CliError::failure(format!(
            "{}: {error}\nprimary Incan source location: {}:{}..{}",
            error.diagnostic_code(),
            entrypoint.display(),
            span.start,
            span.end
        )),
        None => CliError::failure(format!("{}: {error}", error.diagnostic_code())),
    }
}

/// Return the file a refusal's span was measured in, falling back to the executed entrypoint.
///
/// A refusal raised while walking a module the entrypoint merely reaches carries that module's identity. Resolving it
/// back to a file keeps the reported location and the reported span describing the same source.
pub fn replacement_refusal_source<'a>(
    error: &ReplacementExecutionError,
    entrypoint: &'a Path,
    reachable: &'a [ReplacementModuleInputs],
) -> &'a Path {
    let Some(module_id) = error.measured_module() else {
        return entrypoint;
    };
    reachable
        .iter()
        .find(|module| incan_semantics_core::module_identity_for_path(&module.module_path) == module_id)
        .map_or(entrypoint, |module| module.file_path.as_path())
}

/// Refuse one unsupported replacement profile through the canonical #986 selection boundary.
///
/// The resolver must reject the availability claim before the profile diagnostic reaches the CLI. If a future
/// fallback policy resolves it anyway, that is a separate, visible failure rather than implicit legacy execution.
pub fn refuse_replacement_profile<T>(
    selection: &BackendSelection,
    error: ReplacementExecutionError,
    entrypoint: &Path,
) -> CliResult<T> {
    match resolve_execution(selection, false) {
        Err(_) => Err(replacement_profile_cli_error(error, entrypoint)),
        Ok(executed) => Err(CliError::failure(format!(
            "{}: replacement source-profile refusal cannot execute `{executed:?}` because this CLI exposes no receipt-bound fallback path",
            error.diagnostic_code()
        ))),
    }
}

/// Resolve an available direct replacement selection through the canonical #986 boundary.
pub fn resolve_available_replacement_execution(selection: &BackendSelection) -> CliResult<BackendKind> {
    match resolve_execution(selection, true) {
        Ok(BackendKind::Replacement) => Ok(BackendKind::Replacement),
        Ok(executed) => Err(CliError::failure(format!(
            "replacement profile selection resolved unexpected backend `{executed:?}`"
        ))),
        Err(error) => Err(CliError::failure(error.to_string())),
    }
}
