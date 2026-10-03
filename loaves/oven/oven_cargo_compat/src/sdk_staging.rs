//! Staging the sealed SDK provider tree and copying regular directory trees into publisher staging.
//!
//! The compiler-suite publisher copies an already prepared, read-only SDK inventory into its private staging,
//! rebases the component runtime paths it carries, refreshes the digests of what it staged, and turns directories
//! into the materialized-file records a store publication declares. The publisher that calls them lives in
//! `legacy_cargo.rs`.

use std::fs;
use std::path::{Path, PathBuf};

use super::{
    OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH, OvenArtifactMaterializedFile, OvenLegacyCargoError, verified_regular_file,
};
use oven_store::OvenProviderHooks;

/// Copy an SDK inventory with its compiler-owned runtime path dependencies made self-contained.
///
/// SDK component crates intentionally use path dependencies while they are prepared.  A compiler-suite entry is an
/// immutable package boundary, so preserving those original relative paths would either read an unrelated checkout
/// or fail after the publisher's worktree disappears.  The publisher therefore copies only the runtime crate source
/// needed by component Cargo manifests, gives that closure its own minimal workspace, and rewrites only the four
/// compiler-owned dependency paths.  Component-to-component paths remain relative to the copied provider tree.
pub fn stage_self_contained_sdk_provider_tree(
    prepared_root: &Path,
    staging_root: &Path,
    provider_hooks: &dyn OvenProviderHooks,
) -> Result<PathBuf, OvenLegacyCargoError> {
    let provider_root = staging_root.join("providers");
    copy_regular_directory_tree(prepared_root, &provider_root, "SDK provider inventory")?;
    stage_sdk_runtime_crates(&provider_root)?;
    rebase_sdk_component_runtime_paths(&provider_root)?;
    provider_hooks.refresh_staged_sdk_provider_digests(&provider_root)?;
    Ok(provider_root)
}

/// Copy the minimal compiler runtime source closure used by installed SDK component Cargo manifests.
pub fn stage_sdk_runtime_crates(provider_root: &Path) -> Result<(), OvenLegacyCargoError> {
    let source_root = oven_model::toolchain_layout::development_root();
    let runtime_root = provider_root.join("runtime");
    // An inventory may itself have been recovered from an older compiler-suite entry. Its runtime closure was
    // immutable when materialized, and is not an input authority for this publisher. Discard it in staging before
    // rebuilding from this compiler's checked source so a read-only prior `Cargo.lock` neither blocks publication
    // nor silently chooses an older compiler runtime.
    if runtime_root.exists() {
        fs::remove_dir_all(&runtime_root).map_err(|source| OvenLegacyCargoError::Io {
            path: runtime_root.clone(),
            source,
        })?;
    }
    fs::create_dir_all(runtime_root.join("crates")).map_err(|source| OvenLegacyCargoError::Io {
        path: runtime_root.join("crates"),
        source,
    })?;

    // Preserve the active compiler's workspace package metadata while making only the runtime crates workspace
    // members.  Copying the root Cargo.toml verbatim would name compiler crates that are deliberately not retained
    // in this tiny provider closure.
    let source_workspace_manifest = source_root.join("Cargo.toml");
    let source_workspace_text =
        fs::read_to_string(&source_workspace_manifest).map_err(|source| OvenLegacyCargoError::Io {
            path: source_workspace_manifest.clone(),
            source,
        })?;
    let mut workspace_document =
        toml::from_str::<toml::Value>(&source_workspace_text).map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "compiler runtime workspace Cargo.toml",
            message: error.to_string(),
        })?;
    let root_table = workspace_document
        .as_table_mut()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler runtime workspace Cargo.toml",
            message: "must be a TOML table".to_string(),
        })?;
    root_table.retain(|key, _| key == "workspace");
    let workspace = root_table
        .get_mut("workspace")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler runtime workspace Cargo.toml",
            message: "must declare [workspace]".to_string(),
        })?;
    workspace.insert(
        "members".to_string(),
        toml::Value::Array(
            oven_model::toolchain_layout::SDK_RUNTIME_CRATES
                .into_iter()
                .map(|crate_name| toml::Value::String(format!("crates/{crate_name}")))
                .collect(),
        ),
    );
    workspace.remove("exclude");
    // The checkout names every workspace crate once, in `[workspace.dependencies]`, and the copied members inherit
    // their entries from it. Those paths describe the checkout, not the staged layout, where every runtime crate
    // sits under `crates/`: the runtime crates' entries point at the copies (a ring version requirement beside the
    // path survives), and the path entries of crates the runtime does not ship are dropped.
    if let Some(dependencies) = workspace.get_mut("dependencies").and_then(toml::Value::as_table_mut) {
        dependencies.retain(|crate_name, dependency| {
            oven_model::toolchain_layout::SDK_RUNTIME_CRATES.contains(&crate_name)
                || dependency
                    .as_table()
                    .is_none_or(|dependency| !dependency.contains_key("path"))
        });
        for crate_name in oven_model::toolchain_layout::SDK_RUNTIME_CRATES {
            if let Some(dependency) = dependencies.get_mut(crate_name).and_then(toml::Value::as_table_mut)
                && dependency.contains_key("path")
            {
                dependency.insert("path".to_string(), toml::Value::String(format!("crates/{crate_name}")));
            }
        }
    }
    let runtime_workspace_manifest = runtime_root.join("Cargo.toml");
    fs::write(
        &runtime_workspace_manifest,
        toml::to_string_pretty(&workspace_document).map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "compiler runtime workspace Cargo.toml",
            message: error.to_string(),
        })?,
    )
    .map_err(|source| OvenLegacyCargoError::Io {
        path: runtime_workspace_manifest,
        source,
    })?;
    for file_name in ["Cargo.lock", "README.md"] {
        let source = source_root.join(file_name);
        if source.is_file() {
            let destination = runtime_root.join(file_name);
            fs::copy(&source, &destination).map_err(|source_error| OvenLegacyCargoError::Io {
                path: source.clone(),
                source: source_error,
            })?;
        }
    }

    for crate_name in oven_model::toolchain_layout::SDK_RUNTIME_CRATES {
        let source_crate = oven_model::toolchain_layout::support_crate_dir_in(&source_root, crate_name);
        let destination_crate = runtime_root.join("crates").join(crate_name);
        let source_manifest = source_crate.join("Cargo.toml");
        if !source_manifest.is_file() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler runtime source closure",
                message: format!("is missing {}", source_manifest.display()),
            });
        }
        fs::create_dir_all(&destination_crate).map_err(|source| OvenLegacyCargoError::Io {
            path: destination_crate.clone(),
            source,
        })?;
        let destination_manifest = destination_crate.join("Cargo.toml");
        fs::copy(&source_manifest, &destination_manifest).map_err(|source| OvenLegacyCargoError::Io {
            path: source_manifest,
            source,
        })?;
        copy_regular_directory_tree(
            &source_crate.join("src"),
            &destination_crate.join("src"),
            "compiler runtime source closure",
        )?;
    }
    Ok(())
}

/// Rebase the component manifests' compiler-owned dependencies to the sealed runtime source closure.
pub fn rebase_sdk_component_runtime_paths(provider_root: &Path) -> Result<(), OvenLegacyCargoError> {
    let components_root = provider_root.join("components");
    let components = fs::read_dir(&components_root)
        .map_err(|source| OvenLegacyCargoError::Io {
            path: components_root.clone(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenLegacyCargoError::Io {
            path: components_root.clone(),
            source,
        })?;
    for component in components {
        let metadata = component.metadata().map_err(|source| OvenLegacyCargoError::Io {
            path: component.path(),
            source,
        })?;
        if !metadata.is_dir() {
            continue;
        }
        let manifest_path = component.path().join("Cargo.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest_text = fs::read_to_string(&manifest_path).map_err(|source| OvenLegacyCargoError::Io {
            path: manifest_path.clone(),
            source,
        })?;
        let mut manifest =
            toml::from_str::<toml::Value>(&manifest_text).map_err(|error| OvenLegacyCargoError::InvalidInput {
                field: "SDK component Cargo.toml",
                message: format!("{}: {error}", manifest_path.display()),
            })?;
        let Some(dependencies) = manifest.get_mut("dependencies").and_then(toml::Value::as_table_mut) else {
            continue;
        };
        let mut changed = false;
        for crate_name in oven_model::toolchain_layout::SDK_RUNTIME_CRATES {
            let Some(dependency) = dependencies.get_mut(crate_name).and_then(toml::Value::as_table_mut) else {
                continue;
            };
            if dependency.contains_key("path") {
                dependency.insert(
                    "path".to_string(),
                    toml::Value::String(format!("../../runtime/crates/{crate_name}")),
                );
                changed = true;
            }
        }
        if changed {
            // Prepared SDK artifacts may have been installed read-only.  This publisher-owned staging copy is the
            // sole place where its path metadata is rebased, before the final immutable store copy is made.
            make_publisher_staging_file_writable(&manifest_path)?;
            fs::write(
                &manifest_path,
                toml::to_string_pretty(&manifest).map_err(|error| OvenLegacyCargoError::InvalidInput {
                    field: "SDK component Cargo.toml",
                    message: format!("{}: {error}", manifest_path.display()),
                })?,
            )
            .map_err(|source| OvenLegacyCargoError::Io {
                path: manifest_path,
                source,
            })?;
        }
    }
    Ok(())
}

/// Mark one copied SDK file writable before adjusting its integrity metadata in publisher-owned staging.
pub fn make_publisher_staging_file_writable(path: &Path) -> Result<(), OvenLegacyCargoError> {
    let mut permissions = fs::metadata(path)
        .map_err(|source| OvenLegacyCargoError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        // The copied staging file may be read-only, but it must retain its existing group/other bits. Clearing
        // `readonly` would make it world-writable on Unix; grant the publisher owner write access only.
        permissions.set_mode(permissions.mode() | 0o200);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Copy a directory while making symlinked or special source inputs fail closed.
pub fn copy_regular_directory_tree(
    source_root: &Path,
    destination_root: &Path,
    field: &'static str,
) -> Result<(), OvenLegacyCargoError> {
    let metadata = fs::symlink_metadata(source_root).map_err(|source| OvenLegacyCargoError::Io {
        path: source_root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field,
            message: format!("expected a real directory at {}", source_root.display()),
        });
    }
    fs::create_dir_all(destination_root).map_err(|source| OvenLegacyCargoError::Io {
        path: destination_root.to_path_buf(),
        source,
    })?;
    let mut entries = fs::read_dir(source_root)
        .map_err(|source| OvenLegacyCargoError::Io {
            path: source_root.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenLegacyCargoError::Io {
            path: source_root.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source = entry.path();
        let destination = destination_root.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source).map_err(|source_error| OvenLegacyCargoError::Io {
            path: source.clone(),
            source: source_error,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field,
                message: format!("refuses symlinked publisher input {}", source.display()),
            });
        }
        if metadata.is_dir() {
            copy_regular_directory_tree(&source, &destination, field)?;
        } else if metadata.is_file() {
            fs::copy(&source, &destination).map_err(|source_error| OvenLegacyCargoError::Io {
                path: source,
                source: source_error,
            })?;
        } else {
            return Err(OvenLegacyCargoError::InvalidInput {
                field,
                message: format!("refuses non-regular publisher input {}", source.display()),
            });
        }
    }
    Ok(())
}

/// Enumerate one publisher-owned directory as immutable files below a safe artifact prefix.
///
/// The returned files are ordered by their complete portable relative path. Sorting each directory before recursion
/// is insufficient: a root file such as `src.rs` sorts before `src/lib.rs` even though traversal enters `src/` first.
pub fn materialized_files_from_directory(
    root: &Path,
    prefix: &str,
    field: &'static str,
) -> Result<Vec<OvenArtifactMaterializedFile>, OvenLegacyCargoError> {
    let metadata = fs::symlink_metadata(root).map_err(|source| OvenLegacyCargoError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field,
            message: format!("expected a real directory at {}", root.display()),
        });
    }
    let mut files = Vec::new();
    collect_materialized_directory_files(root, root, prefix, field, &mut files)?;
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

/// Retain the exact dependency graph required to inspect sealed registry sources.
///
/// A project extension can share executable artifacts with its compiler Loaf, but its registry-source catalog still
/// needs the exact checked graph that selected those source trees. The lock is deliberately outside the direct-Rustc
/// plan: it is inspection authority, not a linker input. It nevertheless crosses the immutable-store boundary with
/// the same digest verification as every other materialized file.
pub fn materialize_sealed_registry_lock(
    staging: &Path,
    has_registry_sources: bool,
    materialized_files: &mut Vec<OvenArtifactMaterializedFile>,
) -> Result<(), OvenLegacyCargoError> {
    if !has_registry_sources {
        return Ok(());
    }
    let source_path = verified_regular_file(
        &staging.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH),
        "sealed registry Cargo.lock",
    )?;
    let relative_path = OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH.to_string();
    if materialized_files
        .iter()
        .any(|file| file.relative_path == relative_path)
    {
        return Err(OvenLegacyCargoError::Plan(
            "sealed registry Cargo.lock duplicates a direct artifact path".to_string(),
        ));
    }
    materialized_files.push(OvenArtifactMaterializedFile {
        source_path,
        relative_path,
    });
    Ok(())
}

/// Recursively retain regular provider files in deterministic path order while rejecting symlink indirection.
pub fn collect_materialized_directory_files(
    root: &Path,
    directory: &Path,
    prefix: &str,
    field: &'static str,
    files: &mut Vec<OvenArtifactMaterializedFile>,
) -> Result<(), OvenLegacyCargoError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|source| OvenLegacyCargoError::Io {
            path: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenLegacyCargoError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenLegacyCargoError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field,
                message: format!("refuses symlinked publisher input {}", path.display()),
            });
        }
        if metadata.is_dir() {
            collect_materialized_directory_files(root, &path, prefix, field, files)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field,
                message: format!("refuses non-regular publisher input {}", path.display()),
            });
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| OvenLegacyCargoError::InvalidInput {
                field,
                message: format!("cannot make {} relative to {}", path.display(), root.display()),
            })?;
        let components = relative
            .components()
            .map(|component| match component {
                std::path::Component::Normal(component) => {
                    component
                        .to_str()
                        .map(str::to_owned)
                        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                            field,
                            message: format!("path is not UTF-8: {}", path.display()),
                        })
                }
                _ => Err(OvenLegacyCargoError::InvalidInput {
                    field,
                    message: format!("path is not a safe relative file: {}", path.display()),
                }),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if components.is_empty() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field,
                message: format!("provider root file has no relative path: {}", path.display()),
            });
        }
        files.push(OvenArtifactMaterializedFile {
            source_path: path,
            relative_path: format!("{prefix}/{}", components.join("/")),
        });
    }
    Ok(())
}
