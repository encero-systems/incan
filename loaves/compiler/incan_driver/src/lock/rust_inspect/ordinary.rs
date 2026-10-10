//! Rust metadata preparation from original ordinary native producers, independent of SDK inventories.

use std::path::{Path, PathBuf};

use oven_rustc::native_loaf::{NativeLoafInspectionInputs, NativeLoafRequestObservation};
use oven_store::OvenBuildIntent;
use rust_inspect::{RetainedInspectionProject, RustMetadataError, RustWorkspace};

use crate::error::{CliError, CliResult};
use crate::lock::OvenRustInspectSourceAuthorityRequest;

/// Original complete request and source owners, kept inside the inspector's cached database.
struct OrdinaryInspectionProject {
    observation: NativeLoafRequestObservation,
    inputs: NativeLoafInspectionInputs,
    rustc: PathBuf,
    intent: OvenBuildIntent,
    manifest_dir: PathBuf,
}

impl RetainedInspectionProject for OrdinaryInspectionProject {
    /// Recheck the complete request and every original source/fact before projecting the bound compiler's sysroot.
    fn verified_project(&self) -> Result<serde_json::Value, RustMetadataError> {
        self.observation
            .verify_intent(&self.rustc, &self.intent)
            .map_err(|error| invalid(&self.manifest_dir, error))?;
        let graph = self
            .inputs
            .inspection_project()
            .map_err(|error| invalid(&self.manifest_dir, error))?;
        RustWorkspace::inspection_project_with_compiler(graph, &self.rustc)
    }
}

/// Prepare a source-only inspection database from one complete native request and exact declared query roots.
/// Macro execution remains unsupported here; source presence never becomes a grant to run a dynamic library.
pub(super) fn prepare(
    manifest_dir: &Path,
    target_dir: &Path,
    project_root: &Path,
    observation: &NativeLoafRequestObservation,
    rustc: &Path,
    authority: &OvenRustInspectSourceAuthorityRequest<'_>,
    queries: &[String],
    derives: &[String],
) -> CliResult<()> {
    if !derives.is_empty() {
        return Err(CliError::failure(
            "ordinary source inspection requires separate native macro authority",
        ));
    }
    let intent = OvenBuildIntent {
        target: authority.target.to_string(),
        toolchain: authority.toolchain.to_string(),
        profile: authority.profile.to_string(),
        features: authority.features.to_vec(),
    };
    let digest = observation.verify_intent(rustc, &intent).map_err(failure)?.to_string();
    let inputs = observation.graph().inspection_inputs().map_err(failure)?;
    let roots = observation
        .graph()
        .select_dependency_roots(authority.registry_dependencies, project_root, "target")
        .map_err(failure)?;
    for query in queries {
        let name = query.split("::").next().unwrap_or_default();
        if matches!(name, "std" | "core" | "alloc") {
            continue;
        }
        let root = roots
            .iter()
            .find(|root| root.alias.replace('-', "_") == name)
            .ok_or_else(|| {
                failure(format!(
                    "ordinary inspection query `{query}` has no declared native root"
                ))
            })?;
        let candidates = inputs
            .units()
            .iter()
            .filter(|(_, unit)| unit.crate_name() == name)
            .collect::<Vec<_>>();
        if candidates.len() != 1 || candidates[0].0 != &root.record_identity || candidates[0].1.is_proc_macro() {
            return Err(failure(format!(
                "ordinary inspection query `{query}` has an ambiguous, renamed or macro source binding"
            )));
        }
    }
    let project = OrdinaryInspectionProject {
        observation: observation.clone(),
        inputs,
        rustc: rustc.to_path_buf(),
        intent,
        manifest_dir: manifest_dir.to_path_buf(),
    };
    let graph = project.verified_project().map_err(failure)?;
    let sources = project
        .inputs
        .units()
        .values()
        .map(|unit| {
            let selected = unit.selected();
            let record = selected.record();
            let source_root = unit.source_root().map_err(failure)?;
            Ok(rust_inspect::OvenInspectionRegistrySource {
                package: record
                    .source
                    .loaf
                    .strip_prefix("crates-io/")
                    .unwrap_or(&record.source.loaf)
                    .to_string(),
                version: record.source.version.clone(),
                registry: "loaf+native".to_string(),
                checksum: record.source.archive_digest.clone(),
                features: record.source.features.clone(),
                source_digest: oven_store::digest_source_tree(&source_root).map_err(failure)?,
                source_root,
            })
        })
        .collect::<CliResult<Vec<_>>>()?;
    rust_inspect::write_sealed_oven_inspection_source_authority(manifest_dir, sources).map_err(failure)?;
    std::fs::write(
        manifest_dir.join(rust_inspect::OVEN_DIRECT_LOAF_PROJECT_FILE),
        serde_json::to_vec(&graph).map_err(failure)?,
    )
    .map_err(failure)?;
    std::fs::write(
        manifest_dir.join(rust_inspect::OVEN_LOAF_ONLY_INSPECTION_MARKER),
        b"1\n",
    )
    .map_err(failure)?;
    std::fs::write(manifest_dir.join(rust_inspect::OVEN_RETAINED_INSPECTION_MARKER), digest).map_err(failure)?;
    // Clear a prior projection's serialized grants; only the retained source-only loader can load this root.
    rust_inspect::write_oven_inspection_proc_macro_authority(manifest_dir, Vec::new()).map_err(failure)?;
    rust_inspect::RustMetadataCache::new()
        .prepare_retained_project(manifest_dir, target_dir, Box::new(project), &|_| {})
        .map(|_| ())
        .map_err(failure)
}

/// Preserve the generated projection as the diagnostic coordinate for a producer refusal.
fn invalid(path: &Path, error: impl std::fmt::Display) -> RustMetadataError {
    RustMetadataError::LoadWorkspace {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

/// Preserve native producer and inspector errors in the compiler command's error domain.
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

#[cfg(test)]
mod tests;
