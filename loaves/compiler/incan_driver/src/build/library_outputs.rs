//! What a prepared library project writes: its output path and receipts, its executable surfaces, manifest
//! artifacts, packaged-Loaf store root and desugarer artifact.

use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::build::backend_selection::default_backend_receipt_path;
use crate::build::source_authority::canonical_baked_project_lock_path;
use crate::build::{OVEN_PACKAGED_LIBRARY_LOAF_STORE_RELATIVE_PATH, PreparedLibraryProject};
use crate::build_report::artifact_report;
use crate::error::{CliError, CliResult};
use crate::project::validate_output_dir;
use incan_provider::dependency_resolver::ResolvedDependencies;
use incan_provider::vocab_extraction::PendingDesugarerArtifact;
use oven_model::manifest::{DependencySource, DependencySpec};

/// Return whether an internal library artifact build must avoid canonical workspace lock resolution.
///
/// Ordinary dependency artifacts are prepared before their parent can finish the canonical workspace lock, so they
/// must retain producer-local resolution. SDK artifacts are different: their publisher supplies an exact lock
/// override that remains part of preparation.
pub fn dependency_artifact_skips_canonical_lock(artifact_only: bool, sdk_provider_build: bool) -> bool {
    artifact_only && !sdk_provider_build
}

/// Return whether this library compilation needs a rust-inspect workspace to preserve Rust-call signatures.
///
/// An ordinary library build retains its complete ABI inspection contract. An artifact-only SDK provider may skip an
/// empty inspection workspace, but it must prepare one when its source imports Rust items: those signatures can carry
/// ownership facts such as `&impl AsFd` that code generation must preserve. This remains inside the explicit provider
/// publisher; normal Oven consumers never invoke this preparation path.
#[cfg(feature = "rust_inspect")]
pub fn library_rust_inspection_required(artifact_only: bool, metadata_query_paths: &[String]) -> bool {
    !artifact_only || !metadata_query_paths.is_empty()
}

/// Remove path dependencies that point back to the selected project's generated library crate.
///
/// A rooted workspace lock includes the root library as a dependency of its consumers. That aggregate dependency is
/// valid for the synthetic lock/preheat package, but the selected root library artifact cannot depend on its own
/// canonical `target/lib` crate after adopting the producer's Cargo package identity. This comparison deliberately
/// uses the project-owned artifact path rather than a command-specific output override.
pub fn remove_generated_library_self_dependencies(resolved: &mut ResolvedDependencies, project_root: &Path) {
    let artifact_root = project_root.join("target/lib");
    let canonical_artifact_root = fs::canonicalize(&artifact_root).unwrap_or(artifact_root);
    let points_to_generated_crate = |spec: &DependencySpec| match &spec.source {
        DependencySource::Path { path } => {
            fs::canonicalize(path).unwrap_or_else(|_| path.clone()) == canonical_artifact_root
        }
        DependencySource::Registry | DependencySource::Git { .. } => false,
    };
    resolved.dependencies.retain(|spec| !points_to_generated_crate(spec));
    resolved
        .dev_dependencies
        .retain(|spec| !points_to_generated_crate(spec));
}

/// Resolve the generated artifact root before a publication transaction or generator can mutate it.
pub fn library_output_path(project_root: &Path, output_dir: Option<&str>) -> CliResult<PathBuf> {
    match output_dir {
        Some(output) => {
            validate_output_dir(output)?;
            let output = PathBuf::from(output);
            Ok(if output.is_absolute() {
                output
            } else {
                project_root.join(output)
            })
        }
        None => Ok(project_root.join("target/lib")),
    }
}

/// Receipt pointers and portable lock state that a failed library build must restore alongside its artifact.
pub fn library_publication_receipts(project_root: &Path) -> CliResult<Vec<PathBuf>> {
    let receipt = oven_store::default_receipt_path(project_root);
    Ok(vec![
        receipt.clone(),
        receipt.with_file_name("library-debug-receipt.json"),
        receipt.with_file_name("library-release-receipt.json"),
        default_backend_receipt_path(project_root),
        project_root.join("oven.lock"),
        canonical_baked_project_lock_path(project_root)?,
    ])
}

/// Publish an immutable semantic sidecar before atomically selecting it through the accompanying manifest.
///
/// The digest filename means an interrupted rebuild leaves the previously selected representation intact. Removed
/// declarations and modules disappear from the new index; old immutable files are never searched or selected.
fn write_library_executable_surfaces(prepared: &mut PreparedLibraryProject) -> CliResult<()> {
    let path = incan_frontend::library_manifest::published_layout::executable_surface_path(
        &prepared.manifest_path,
        &prepared.library_manifest,
    )
    .ok_or_else(|| CliError::failure("prepared library has no valid executable artifact descriptor"))?;
    publish_library_file(&path, &prepared.executable_surface)?;
    prepared
        .report
        .artifacts
        .push(artifact_report("incan_executable_representation", &path));
    Ok(())
}

/// Use the artifact publisher's staged-write durability for one same-directory atomic file replacement.
fn publish_library_file(path: &Path, bytes: &[u8]) -> CliResult<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| CliError::failure("library artifact has no parent"))?;
    fs::create_dir_all(parent).map_err(|error| CliError::failure(error.to_string()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CliError::failure("library artifact has no file name"))?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| CliError::failure(error.to_string()))?
        .as_nanos();
    let staged = parent.join(format!(".{name}.publish-{}-{nonce}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&staged)
        .map_err(|error| CliError::failure(format!("failed to stage {}: {error}", path.display())))?;
    let result = (|| -> io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&staged, path)?;
        fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result.map_err(|error| CliError::failure(format!("failed to publish {}: {error}", path.display())))
}

/// Write the `.incnlib` manifest and build-report artifact paths for a prepared library project.
pub fn write_library_manifest_artifacts(prepared: &mut PreparedLibraryProject) -> CliResult<()> {
    let manifest = prepared
        .library_manifest
        .to_json_string()
        .map_err(|error| CliError::failure(format!("failed to encode library manifest: {error}")))?;
    write_library_executable_surfaces(prepared)?;
    publish_library_file(&prepared.manifest_path, manifest.as_bytes())?;

    prepared
        .report
        .artifacts
        .push(artifact_report("incan_library_manifest", &prepared.manifest_path));
    prepared.report.artifacts.push(artifact_report(
        "generated_cargo_manifest",
        &prepared.generator.cargo_manifest_path(),
    ));
    Ok(())
}

/// Return the package-owned bounded Oven store that carries this public library's project Loafs.
pub fn packaged_library_loaf_store_root(artifact_root: &Path) -> PathBuf {
    artifact_root.join(OVEN_PACKAGED_LIBRARY_LOAF_STORE_RELATIVE_PATH)
}

/// Copy a pending desugarer artifact into its declared path beneath the output directory.
pub fn package_desugarer_artifact(out_dir: &Path, artifact: Option<&PendingDesugarerArtifact>) -> CliResult<()> {
    let Some(artifact) = artifact else {
        return Ok(());
    };

    let destination = out_dir.join(&artifact.metadata.relative_path);
    let destination_parent = destination.parent().ok_or_else(|| {
        CliError::failure(format!(
            "invalid desugarer artifact destination path: {}",
            destination.display()
        ))
    })?;

    fs::create_dir_all(destination_parent).map_err(|err| {
        CliError::failure(format!(
            "failed to create desugarer artifact directory {}: {err}",
            destination_parent.display()
        ))
    })?;
    fs::copy(&artifact.source_path, &destination).map_err(|err| {
        CliError::failure(format!(
            "failed to package vocab desugarer artifact {} -> {}: {err}",
            artifact.source_path.display(),
            destination.display()
        ))
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    use incan_provider::dependency_resolver::ResolvedDependencies;
    use oven_model::manifest::{DependencySource, DependencySpec};

    #[test]
    fn dependency_artifact_only_build_skips_canonical_lock_issue908() {
        assert!(dependency_artifact_skips_canonical_lock(true, false));
        assert!(!dependency_artifact_skips_canonical_lock(true, true));
        assert!(!dependency_artifact_skips_canonical_lock(false, false));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn artifact_only_provider_preserves_required_rust_call_metadata() {
        assert!(library_rust_inspection_required(
            true,
            &["rustix::fs::flock".to_string()]
        ));
        assert!(!library_rust_inspection_required(true, &[]));
        assert!(library_rust_inspection_required(false, &[]));
    }

    #[test]
    fn rooted_library_removes_selected_project_self_dependency_issue909() -> Result<(), Box<dyn std::error::Error>> {
        let project_root = tempfile::tempdir()?;
        let artifact_root = project_root.path().join("target/lib");
        let external_root = project_root.path().join("external/artifact");
        fs::create_dir_all(&artifact_root)?;
        fs::create_dir_all(&external_root)?;
        let path_dependency = |crate_name: &str, path: PathBuf| DependencySpec {
            crate_name: crate_name.to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path { path },
            optional: false,
            package: None,
        };
        let mut resolved = ResolvedDependencies {
            dependencies: vec![
                path_dependency("root_lib", artifact_root.clone()),
                path_dependency("external", external_root),
            ],
            dev_dependencies: vec![path_dependency("root_lib_dev_alias", artifact_root)],
        };

        remove_generated_library_self_dependencies(&mut resolved, project_root.path());

        assert_eq!(resolved.dependencies.len(), 1);
        assert_eq!(resolved.dependencies[0].crate_name, "external");
        assert!(resolved.dev_dependencies.is_empty());
        Ok(())
    }
}
