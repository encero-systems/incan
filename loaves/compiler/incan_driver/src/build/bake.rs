//! A project's targets and its completed outputs: the targets an explicit bake prepares, the reusable completed
//! executable output a command may select before any frontend work, and publishing a prepared output.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::backend::selection::BackendExecutionReceipt;
use crate::build::output_materialization::{
    completed_output_default_backend_receipt, select_default_project_output_with_inputs,
};
use crate::build::output_paths::{
    normalized_project_entrypoint, project_root_for_completed_output_with_inputs,
    validated_project_output_relative_path,
};
use crate::build::output_selection::remember_project_output;
use crate::build::publication::{project_output_payload_for_bake, publish_project_output_loaf};
use crate::build::source_authority::project_bake_receipt_path;
use crate::build::{
    BuildCommandOptions, CompletedOutputPolicy, OvenBakeProjectTarget, OvenProjectOutputBakeRequest,
    OvenStoredProjectOutput,
};
use crate::error::{CliError, CliResult};
use crate::oven_store::open_default_oven_store;
use crate::project::{discover_effective_project_manifest, enforce_project_toolchain_constraint};
use oven_model::manifest::ProjectManifest;

/// Publish a prepared backend result through the shared completed-output payload, Store and exact locator.
///
/// Callers must re-observe their complete source and invocation authority before entering this boundary. Both
/// ordinary Rust emission and the pinned native driver use the same publication and materialization contracts.
pub fn publish_prepared_project_output(
    request: OvenProjectOutputBakeRequest<'_>,
    preparation_identity: &str,
) -> CliResult<OvenStoredProjectOutput> {
    let _invocation =
        tracing::trace_span!(target: "incan::invocation", "incan_driver::publish_prepared_output").entered();
    let receipt_path = project_bake_receipt_path(
        request.project_root,
        request.target,
        request.entrypoint,
        request.profile,
    )?;
    let receipt = request.receipt;
    let files = request.files.clone();
    let mut payload = project_output_payload_for_bake(request)?;
    payload.preparation_identity_digest = Some(preparation_identity.to_string());
    let output = publish_project_output_loaf(&open_default_oven_store()?, receipt, &payload, &files)?;
    remember_project_output(&receipt_path, &output.identity);
    Ok(output)
}

/// Select the requested profile's default sealed output without entering frontend or report reconstruction.
pub fn select_default_executable_project_output(
    file_path: &str,
    output_dir: Option<&String>,
    options: &BuildCommandOptions,
    authority: &mut crate::build::ProjectSourceAuthorityDigester,
) -> CliResult<Option<(PathBuf, OvenStoredProjectOutput, BackendExecutionReceipt)>> {
    let _invocation =
        tracing::trace_span!(target: "incan::invocation", "incan_driver::select_executable_output", input = file_path)
            .entered();
    if output_dir.is_some() {
        return Ok(None);
    }
    let completed_output_policy = CompletedOutputPolicy {
        lock_policy: &options.lock_policy,
        package_features: &options.package_features,
    };
    let Some(selected) = select_default_project_output_with_inputs(
        file_path,
        &completed_output_policy,
        OvenBakeProjectTarget::Executable,
        options.executable_profile.as_str(),
        authority,
    )?
    else {
        return Ok(None);
    };
    let project_root = project_root_for_completed_output_with_inputs(
        &normalized_project_entrypoint(file_path)?,
        &mut authority.retained_inputs,
    )?
    .ok_or_else(|| CliError::failure("selected Oven project-output Loaf has no manifest-backed project root"))?;
    let Some(backend_receipt) = completed_output_default_backend_receipt(&selected) else {
        return Ok(None);
    };
    super::output_materialization::warn_for_completed_output_lock_fingerprint_drift_with_current(
        &project_root,
        [&selected],
        || authority.lock_fingerprint(&project_root),
    )?;
    Ok(Some((project_root, selected, backend_receipt)))
}

/// Resolve and validate every distinct executable entrypoint selected by one effective manifest.
///
/// This is shared by target discovery and source-authority hashing so an executable cannot be baked without the same
/// path also participating in freshness checks.
pub fn discover_oven_executable_entrypoints(manifest: &ProjectManifest) -> CliResult<BTreeMap<String, PathBuf>> {
    let mut executable_paths = BTreeMap::new();
    if let Some(project) = manifest.project.as_ref() {
        let mut scripts = project.scripts.iter().collect::<Vec<_>>();
        scripts.sort_by(|left, right| left.0.cmp(right.0));
        for (name, configured_path) in scripts {
            let relative = validated_project_output_relative_path(configured_path, "declared script")?;
            let entrypoint = manifest.project_root().join(&relative);
            let metadata = fs::symlink_metadata(&entrypoint).map_err(|error| {
                CliError::failure(format!(
                    "declared Oven project script `{name}` must resolve to a regular file at {}: {error}",
                    entrypoint.display()
                ))
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(CliError::failure(format!(
                    "declared Oven project script `{name}` must resolve to a regular file at {}",
                    entrypoint.display()
                )));
            }
            executable_paths.insert(relative.to_string_lossy().replace('\\', "/"), entrypoint);
        }
    }
    let conventional_main = manifest
        .project_root()
        .join(OvenBakeProjectTarget::Executable.source_relative_path());
    match fs::symlink_metadata(&conventional_main) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(CliError::failure(format!(
                "Oven project executable target must be a regular file: {}",
                conventional_main.display()
            )));
        }
        Ok(_) => {
            executable_paths.insert(
                OvenBakeProjectTarget::Executable.source_relative_path().to_string(),
                conventional_main,
            );
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CliError::failure(format!(
                "failed to inspect Oven project executable target {}: {error}",
                conventional_main.display()
            )));
        }
    }
    Ok(executable_paths)
}

/// Resolve every manifest-backed target that an explicit project bake must prepare.
///
/// Declared scripts are first-class executable targets rather than aliases for `src/main.incn`. The conventional main
/// remains an implicit fallback when present, and exact duplicate paths are collapsed so one authored entrypoint is
/// never compiled twice merely because it has more than one script name.
pub fn discover_oven_bake_project_targets(project_root: &Path) -> CliResult<Vec<(OvenBakeProjectTarget, PathBuf)>> {
    let Some(manifest) = discover_effective_project_manifest(project_root)? else {
        return Err(CliError::failure(format!(
            "`incan oven bake --project` requires a loaf.toml project at {}",
            project_root.display()
        )));
    };
    enforce_project_toolchain_constraint(&manifest)?;

    let mut targets = Vec::new();
    let library = manifest
        .project_root()
        .join(OvenBakeProjectTarget::Library.source_relative_path());
    match fs::symlink_metadata(&library) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(CliError::failure(format!(
                "Oven project library target must be a regular file: {}",
                library.display()
            )));
        }
        Ok(_) => targets.push((OvenBakeProjectTarget::Library, library)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CliError::failure(format!(
                "failed to inspect Oven project library target {}: {error}",
                library.display()
            )));
        }
    }

    targets.extend(
        discover_oven_executable_entrypoints(&manifest)?
            .into_values()
            .map(|entrypoint| (OvenBakeProjectTarget::Executable, entrypoint)),
    );
    if targets.is_empty() {
        return Err(CliError::failure(format!(
            "`incan oven bake --project` requires {}, {}, or a declared [project.scripts] entry below {}",
            OvenBakeProjectTarget::Library.source_relative_path(),
            OvenBakeProjectTarget::Executable.source_relative_path(),
            manifest.project_root().display()
        )));
    }
    Ok(targets)
}
