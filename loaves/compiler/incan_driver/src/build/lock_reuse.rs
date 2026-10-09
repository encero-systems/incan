//! Transactional lock refresh after an explicit completed-output probe misses.

use std::path::Path;

use crate::build::output_publication::OutputPublication;
use crate::build::source_authority::canonical_baked_project_lock_path;
use crate::error::{CliError, CliResult};
use incan_provider::FeatureSelection;
use oven_model::lock::{IncanLock, acquire_publication_lock};
use oven_model::manifest::ProjectManifest;

/// Retry completed-output selection under checked provider identities, restoring the prior lock on a miss or error.
///
/// Call only after the ordinary warm probe misses. Resolution cannot compile or prepare providers; unavailable
/// checked inputs retain the normal miss/fallback behavior. The canonical publication guard spans resolution,
/// atomic lock replacement, output selection, and rollback. Successful selection is the only commit boundary.
pub(super) fn try_reuse_with_resolved_provider_lock<T>(
    manifest: &ProjectManifest,
    entry_file: &Path,
    features: &FeatureSelection,
    reuse: impl FnOnce() -> CliResult<Option<T>>,
) -> CliResult<Option<T>> {
    let path = canonical_baked_project_lock_path(manifest.project_root())?;
    if !path.is_file() {
        return Ok(None);
    }
    let guard = acquire_publication_lock(&path).map_err(|error| CliError::failure(error.to_string()))?;
    let current = IncanLock::load(&path).map_err(|error| CliError::failure(error.to_string()))?;
    let candidate = match crate::lock::reuse::resolved_provider_refresh(manifest, entry_file, features, &current) {
        Ok(Some(candidate)) => candidate,
        result => {
            if std::env::var_os("INCAN_OVEN_TRACE_REUSE").is_some() {
                match result {
                    Err(error) => eprintln!("Oven reuse provider lock refresh unavailable: {error}"),
                    _ => eprintln!("Oven reuse provider lock refresh: no digest-only candidate"),
                }
            }
            return Ok(None);
        }
    };
    let owner = path
        .parent()
        .ok_or_else(|| CliError::failure("canonical lock has no owner"))?;
    let publication = OutputPublication::begin(owner, Vec::new(), vec![path.clone()])?;
    let result = (|| {
        candidate
            .write_while_locked(&path, &guard)
            .map_err(|error| CliError::failure(error.to_string()))?;
        reuse()
    })();
    publication.finish_reuse(result)
}
