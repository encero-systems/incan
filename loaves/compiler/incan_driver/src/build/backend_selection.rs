//! Choosing and recording the code-generation backend for one build.

use std::fs;
use std::path::{Path, PathBuf};

use crate::backend::selection::{BackendKind, BackendSelection, finalize_receipt, select_backend};
use crate::build::rust_extern::module_source_identity;
use crate::error::{CliError, CliResult};
use incan_frontend::{ParsedModule, diagnostics};

/// Declare the backend for one build over its collected modules, before codegen runs (#986).
///
/// The one route is the legacy Rust-emission backend until the direct route replaces it; the selection binds it to the
/// content identity of the modules being compiled, at both the executable (`prepare_oven_project`) and library
/// (`prepare_library_project`) call sites.
pub fn select_build_backend(modules: &[ParsedModule]) -> BackendSelection {
    select_backend(BackendKind::Legacy, module_source_identity(modules))
}

/// Bind a real output identity to a build's backend selection (#986), with the diagnostic contract in force.
pub fn finalize_backend_receipt(
    selection: &BackendSelection,
    output_identity: String,
) -> CliResult<crate::backend::selection::BackendExecutionReceipt> {
    finalize_receipt(selection, output_identity, diagnostics::DIAGNOSTIC_SCHEMA_VERSION)
        .map_err(|error| CliError::failure(error.to_string()))
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
