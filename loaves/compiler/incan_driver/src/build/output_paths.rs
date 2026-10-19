//! Where a project's outputs go: entrypoints, completed-output roots, and the files a completed output retains.

use std::path::{Path, PathBuf};
use std::{env, fs};

use crate::build::OvenProjectOutputBakeFile;
use crate::error::{CliError, CliResult};
use crate::project::{discover_effective_project_manifest, enforce_project_toolchain_constraint, resolve_project_root};

/// Return the native host target without asking an ambient Rust installation to decide whether a completed Loaf may
/// run. A completed project output is already linked; only a fresh compile needs to resolve `rustc`.
pub fn native_project_output_target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        _ => None,
    }
}

/// Normalize one entrypoint exactly as project preparation does, without discovering modules or constructing a compiler
/// session.
pub fn normalized_project_entrypoint(file_path: &str) -> CliResult<PathBuf> {
    if Path::new(file_path).is_absolute() {
        return Ok(PathBuf::from(file_path));
    }
    env::current_dir()
        .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))
        .map(|current_dir| current_dir.join(file_path))
}

/// Return a portable project-relative entrypoint path or decline the optional output fast path. Non-project invocations
/// remain supported by normal Oven preparation; only explicit project bakes may have published this form.
pub fn project_relative_entrypoint(project_root: &Path, entrypoint: &Path) -> Option<String> {
    entrypoint
        .strip_prefix(project_root)
        .ok()
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .filter(|relative| !relative.is_empty())
}

/// Discover the manifest-owning project root for a completed-output lookup.
///
/// This avoids assuming a conventional `src/` layout: a custom source root is still an exact project bake, while a
/// standalone file intentionally remains on the normal explicit-preparation path.
pub fn project_root_for_completed_output(entrypoint: &Path) -> CliResult<Option<PathBuf>> {
    project_root_for_completed_output_with_inputs(
        entrypoint,
        &mut super::retained_inputs::RetainedProjectInputs::default(),
    )
}

/// Find the nearest project without reparsing an already observed effective manifest or workspace.
///
/// Nested-command overrides keep their established discovery boundary. Ordinary requests probe manifest location,
/// then borrow the same guarded model used for source authority and canonical lock selection. Single-file requests
/// remain outside completed-project reuse.
pub(super) fn project_root_for_completed_output_with_inputs(
    entrypoint: &Path,
    inputs: &mut super::retained_inputs::RetainedProjectInputs,
) -> CliResult<Option<PathBuf>> {
    let inferred_root = resolve_project_root(entrypoint);
    if std::env::var_os(oven_model::manifest::INTERNAL_MANIFEST_OVERRIDE_ENV).is_some()
        || std::env::var_os(oven_model::manifest::INTERNAL_PROJECT_ROOT_OVERRIDE_ENV).is_some()
    {
        let Some(manifest) = discover_effective_project_manifest(&inferred_root)? else {
            return Ok(None);
        };
        enforce_project_toolchain_constraint(&manifest)?;
        return Ok(Some(manifest.project_root().to_path_buf()));
    }
    let Some(manifest_path) =
        oven_model::manifest::find_manifest(&inferred_root).map_err(|error| CliError::failure(error.to_string()))?
    else {
        return Ok(None);
    };
    let root = manifest_path
        .parent()
        .ok_or_else(|| CliError::failure("project manifest has no parent directory"))?;
    let project = inputs.project(root)?;
    enforce_project_toolchain_constraint(&project.manifest)?;
    Ok(Some(root.to_path_buf()))
}

/// Reject a store- or caller-relative path that could escape its declared root.
pub fn validated_project_output_relative_path(relative_path: &str, role: &str) -> CliResult<PathBuf> {
    let relative = Path::new(relative_path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(CliError::failure(format!(
            "selected Oven project-output Loaf has an unsafe {role} path"
        )));
    }
    Ok(relative.to_path_buf())
}

/// Resolve one regular output file and its path relative to its physical artifact root.
///
/// Resolve ancestor aliases on both sides before checking containment, while refusing a symlink at the file itself.
pub(super) fn canonical_output_file_relative_to_root(
    project_root: &Path,
    source_path: &Path,
) -> CliResult<(PathBuf, String)> {
    let metadata = fs::symlink_metadata(source_path).map_err(|error| {
        CliError::failure(format!(
            "cannot retain generated Oven project output {}: {error}",
            source_path.display()
        ))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CliError::failure(format!(
            "completed Oven project output must be a regular non-symlink file: {}",
            source_path.display()
        )));
    }
    let canonical_root = fs::canonicalize(project_root).map_err(|error| {
        CliError::failure(format!(
            "cannot resolve Oven project root {}: {error}",
            project_root.display()
        ))
    })?;
    let canonical_source = fs::canonicalize(source_path).map_err(|error| {
        CliError::failure(format!(
            "cannot resolve generated Oven project output {}: {error}",
            source_path.display()
        ))
    })?;
    let caller_relative_path = canonical_source
        .strip_prefix(&canonical_root)
        .map_err(|_| {
            CliError::failure(format!(
                "completed Oven project output {} escaped project root {}",
                source_path.display(),
                project_root.display()
            ))
        })?
        .to_string_lossy()
        .replace('\\', "/");
    let _ = validated_project_output_relative_path(&caller_relative_path, "caller output")?;
    Ok((canonical_source, caller_relative_path))
}

/// Add one regular file physically below the project root to the completed-Loaf publication set.
pub(crate) fn append_project_output_bake_file(
    project_root: &Path,
    source_path: &Path,
    output_relative_path: String,
    files: &mut Vec<OvenProjectOutputBakeFile>,
) -> CliResult<()> {
    let (canonical_source, caller_relative_path) = canonical_output_file_relative_to_root(project_root, source_path)?;
    let output_relative_path = validated_project_output_relative_path(&output_relative_path, "stored output")?
        .to_string_lossy()
        .replace('\\', "/");
    files.push(OvenProjectOutputBakeFile {
        source_path: canonical_source,
        caller_relative_path,
        output_relative_path,
    });
    Ok(())
}

/// Recursively collect generated source files in stable order without treating unrelated worktree output or a mutable
/// inspection cache as completed output.
pub(crate) fn append_project_output_tree(
    project_root: &Path,
    source_root: &Path,
    output_prefix: &str,
    files: &mut Vec<OvenProjectOutputBakeFile>,
) -> CliResult<()> {
    let mut entries = fs::read_dir(source_root)
        .map_err(|error| {
            CliError::failure(format!(
                "failed to read generated Oven source {}: {error}",
                source_root.display()
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            CliError::failure(format!(
                "failed to enumerate generated Oven source {}: {error}",
                source_root.display()
            ))
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            CliError::failure(format!(
                "failed to inspect generated Oven source {}: {error}",
                path.display()
            ))
        })?;
        if file_type.is_dir() {
            let child_prefix = format!(
                "{}/{}",
                output_prefix.trim_end_matches('/'),
                entry.file_name().to_string_lossy()
            );
            append_project_output_tree(project_root, &path, &child_prefix, files)?;
        } else if file_type.is_file() {
            let output_relative_path = format!(
                "{}/{}",
                output_prefix.trim_end_matches('/'),
                entry.file_name().to_string_lossy()
            );
            append_project_output_bake_file(project_root, &path, output_relative_path, files)?;
        } else {
            return Err(CliError::failure(format!(
                "generated Oven source contains a non-regular entry: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn project_output_accepts_symlink_equivalent_roots() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let project = temporary.path().join("project");
        fs::create_dir_all(project.join("target"))?;
        fs::write(project.join("target/result"), "native artifact")?;
        let canonical_project = fs::canonicalize(&project)?;
        let alias = temporary.path().join("alias");
        std::os::unix::fs::symlink(&canonical_project, &alias)?;
        for (root, source) in [
            (alias.clone(), canonical_project.join("target/result")),
            (canonical_project.clone(), alias.join("target/result")),
        ] {
            let mut files = Vec::new();
            append_project_output_bake_file(&root, &source, "artifact/result".into(), &mut files)?;
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].caller_relative_path, "target/result");
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn project_output_rejects_physical_escape_and_symlink_files() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let project = temporary.path().join("project");
        let outside = temporary.path().join("outside");
        fs::create_dir_all(&project)?;
        fs::create_dir_all(&outside)?;
        fs::write(outside.join("result"), "outside artifact")?;
        std::os::unix::fs::symlink(&outside, project.join("escape"))?;
        std::os::unix::fs::symlink(outside.join("result"), project.join("linked-file"))?;
        for source in [
            outside.join("result"),
            project.join("escape/result"),
            project.join("linked-file"),
        ] {
            let mut files = Vec::new();
            assert!(append_project_output_bake_file(&project, &source, "artifact/result".into(), &mut files).is_err());
            assert!(files.is_empty(), "a refused file must not enter the publication set");
        }
        Ok(())
    }
}
