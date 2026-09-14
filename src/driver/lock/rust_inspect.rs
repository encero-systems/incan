//! Preparing the rust-inspect workspace a lock resolution or a typecheck needs, under Oven's inspection authority
//! where one is installed.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(feature = "rust_inspect")]
use std::sync::Arc;

use crate::dependency_resolver::resolve_reachable_dependencies;
use crate::driver::cargo_policy::cargo_command_flags;
use crate::driver::error::{CliError, CliResult};
use crate::driver::lock::registry_sources::{
    acquire_explicit_project_inspection_sources, install_required_oven_registry_lock,
};
use crate::driver::lock::resolution::resolve_lock_context;
use crate::driver::lock::{
    LockResolutionRequest, OvenRustInspectSourceAuthorityRequest, PreparedRustInspectTypecheckWorkspace,
    PreparedRustInspectWorkspace, RustInspectTypecheckRequest, RustInspectWorkspaceRequest,
};
use crate::driver::modules::{build_source_map, collect_rust_dependency_uses, format_dependency_error};
#[cfg(feature = "rust_inspect")]
use crate::driver::rust_inspect_workspace::collect_rust_inspect_derive_probe_paths;
#[cfg(feature = "rust_inspect")]
use crate::driver::rust_inspect_workspace::collect_rust_inspect_query_paths;
#[cfg(feature = "rust_inspect")]
use crate::driver::rust_inspect_workspace::ensure_rust_inspect_workspace_with_cargo_package_name;
#[cfg(feature = "rust_inspect")]
use crate::driver::rust_inspect_workspace::mark_oven_cargo_bootstrap_rust_inspection;
#[cfg(feature = "rust_inspect")]
use crate::driver::rust_inspect_workspace::mark_oven_direct_rust_inspection;
#[cfg(feature = "rust_inspect")]
use crate::driver::rust_inspect_workspace::prewarm_rust_inspect_workspace;
use crate::generated_cache::resolve_generated_cargo_target;
#[cfg(feature = "rust_inspect")]
use crate::oven::OvenGeneratedProjectRequest;
#[cfg(feature = "rust_inspect")]
use crate::oven::legacy_cargo::OVEN_LEGACY_CARGO_INSPECTION_AUTHORITY_ENV;
#[cfg(feature = "rust_inspect")]
use crate::oven::loaf::resolve_compiler_owned_loaf_for_registry_dependencies;
#[cfg(feature = "rust_inspect")]
use crate::oven::loaf::resolve_toolchain_loaf_for_registry_sources;
#[cfg(feature = "rust_inspect")]
use crate::oven::receipt_generated_project;
#[cfg(feature = "rust_inspect")]
use crate::oven::rustc::OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH;
#[cfg(feature = "rust_inspect")]
use crate::oven::rustc::resolve_active_rustc;
#[cfg(feature = "rust_inspect")]
use crate::oven::rustc::rustc_host_target;
#[cfg(feature = "rust_inspect")]
use crate::oven::rustc::rustc_identity;
use crate::provider::requirements::{collect_project_requirements, merge_project_requirement_dependencies};

/// Prepare and prewarm the generated Rust workspace used for rust-inspect metadata queries.
#[cfg(feature = "rust_inspect")]
pub(crate) fn prepare_rust_inspect_workspace(
    request: RustInspectWorkspaceRequest<'_>,
) -> CliResult<Option<PreparedRustInspectWorkspace>> {
    let RustInspectWorkspaceRequest {
        project_root,
        project_name,
        cargo_package_name,
        rust_edition,
        resolved,
        project_requirements,
        lock_payload,
        cargo_lock_projection_root,
        clear_cargo_lock,
        cargo_policy_flags,
        cargo_target_dir,
        rust_inspect_query_paths,
        rust_derive_probe_paths,
        prepare_when_empty,
        direct_oven_inspection,
        force_direct_prewarm,
        oven_source_authority,
        prepared_project_source_authorities,
        explicit_oven_bake,
    } = request;
    if rust_inspect_query_paths.is_empty() && rust_derive_probe_paths.is_empty() && !prepare_when_empty {
        return Ok(None);
    }

    let rust_inspect_manifest_dir = ensure_rust_inspect_workspace_with_cargo_package_name(
        project_root,
        project_name,
        cargo_package_name,
        rust_edition,
        resolved,
        project_requirements,
        lock_payload,
        cargo_lock_projection_root,
        clear_cargo_lock,
        cargo_target_dir,
        &cargo_policy_flags,
        rust_derive_probe_paths,
    )?;
    let mut source_loaf = None;
    let mut project_source_authorities = None;
    if direct_oven_inspection {
        if std::env::var_os(crate::oven::loaf::OVEN_LOAF_ENV).is_some_and(|value| value == "1") {
            let source = std::env::var_os(OVEN_LEGACY_CARGO_INSPECTION_AUTHORITY_ENV)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .ok_or_else(|| {
                    CliError::failure("explicit Loaf baker did not supply its locked Rust inspection source authority")
                })?;
            let destination =
                rust_inspect_manifest_dir.join(crate::rust_inspect::OVEN_DIRECT_INSPECTION_AUTHORITY_FILE);
            fs::copy(&source, &destination).map_err(|error| {
                CliError::failure(format!(
                    "failed to install explicit baker Rust inspection authority from {}: {error}",
                    source.display()
                ))
            })?;
        } else if let Some(authority_request) = oven_source_authority {
            let mut receipt_request = OvenGeneratedProjectRequest::new(
                project_root,
                project_name,
                authority_request.project_version,
                authority_request.target,
                authority_request.toolchain,
                authority_request.profile,
                authority_request.features.to_vec(),
            )
            .with_generated_source("generated-root", rust_inspect_manifest_dir.join("src/main.rs"));
            for (name, value) in authority_request.build_unit_inputs {
                receipt_request = receipt_request.with_build_unit_input(name, value);
            }
            let receipt = receipt_generated_project(&receipt_request).map_err(|error| {
                CliError::failure(format!("failed to receipt Oven Rust inspection source: {error}"))
            })?;
            let command_authority_available = prepared_project_source_authorities.is_some();
            let command_authority_installed = if let Some(prepared) = prepared_project_source_authorities.as_ref() {
                let installed = prepared
                    .install_for_dependencies(&rust_inspect_manifest_dir, authority_request.registry_dependencies)?;
                if installed {
                    project_source_authorities = Some(Arc::clone(prepared));
                }
                installed
            } else {
                false
            };
            if normal_inspection_requires_installed_project_authority(
                project_root,
                explicit_oven_bake,
                command_authority_available,
                command_authority_installed,
            ) {
                let detail = if command_authority_available {
                    "the source-current project inspection authority could not authorize this inspection batch"
                } else {
                    "no source-current project inspection authority is available"
                };
                return Err(CliError::failure(format!(
                    "Oven Alpha {detail}; rerun `incan oven bake --project .`"
                )));
            }
            if command_authority_installed {
                // The command-local context owns every output, entry, and base-Loaf lease through this workspace.
            } else if explicit_oven_bake {
                if let Some(selected) =
                    resolve_toolchain_loaf_for_registry_sources(&receipt, authority_request.registry_dependencies)
                        .map_err(|error| CliError::failure(error.to_string()))?
                {
                    install_oven_inspection_source_authority(
                        &rust_inspect_manifest_dir,
                        &selected.artifacts.registry_sources,
                        &selected.artifact_root,
                        None,
                        None,
                    )?;
                    install_required_oven_registry_lock(
                        !selected.artifacts.registry_sources.is_empty(),
                        &selected.artifact_root,
                        &rust_inspect_manifest_dir.join("Cargo.lock"),
                    )?;
                    source_loaf = Some(selected);
                } else {
                    let release_loaf = resolve_compiler_owned_loaf_for_registry_dependencies(&receipt, &[])
                        .map_err(|error| CliError::failure(error.to_string()))?;
                    let release_registry_lock = release_loaf
                        .as_ref()
                        .map(|loaf| loaf.artifact_root.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH));
                    acquire_explicit_project_inspection_sources(
                        &rust_inspect_manifest_dir,
                        authority_request.features,
                        authority_request.registry_dependencies,
                        release_registry_lock.as_deref(),
                    )?;
                    source_loaf = release_loaf;
                }
            } else if let Some(selected) =
                resolve_toolchain_loaf_for_registry_sources(&receipt, authority_request.registry_dependencies)
                    .map_err(|error| CliError::failure(error.to_string()))?
            {
                install_oven_inspection_source_authority(
                    &rust_inspect_manifest_dir,
                    &selected.artifacts.registry_sources,
                    &selected.artifact_root,
                    None,
                    None,
                )?;
                install_required_oven_registry_lock(
                    !selected.artifacts.registry_sources.is_empty(),
                    &selected.artifact_root,
                    &rust_inspect_manifest_dir.join("Cargo.lock"),
                )?;
                source_loaf = Some(selected);
            } else {
                return Err(CliError::failure(
                    "Oven Alpha has no receipt-compatible Loaf containing the requested Rust inspection sources",
                ));
            }
        }
        if explicit_oven_bake {
            mark_oven_cargo_bootstrap_rust_inspection(&rust_inspect_manifest_dir)?;
        } else {
            mark_oven_direct_rust_inspection(&rust_inspect_manifest_dir)?;
        }
    }
    prewarm_rust_inspect_workspace(
        &rust_inspect_manifest_dir,
        cargo_target_dir,
        rust_inspect_query_paths,
        force_direct_prewarm,
    )?;
    Ok(Some(PreparedRustInspectWorkspace {
        manifest_dir: rust_inspect_manifest_dir,
        _source_loaf: source_loaf,
        _project_source_authorities: project_source_authorities,
    }))
}

/// Return whether a normal direct-inspection consumer must refuse generic release-source selection.
///
/// A prepared command authority is direct evidence that the caller belongs to a manifest-backed project, including
/// projects with a custom source root or scripts outside `src`. Conventional source paths retain the earlier defensive
/// check for callers that have not yet propagated that authority. Explicit baking and true standalone files may still
/// select release-owned inspection sources.
#[cfg(feature = "rust_inspect")]
fn normal_inspection_requires_installed_project_authority(
    project_root: &Path,
    explicit_oven_bake: bool,
    command_authority_available: bool,
    command_authority_installed: bool,
) -> bool {
    let conventional_project =
        project_root.join("src/lib.incn").is_file() || project_root.join("src/main.incn").is_file();
    !explicit_oven_bake && !command_authority_installed && (command_authority_available || conventional_project)
}

/// Install one sealed registry-source catalog into a direct Oven inspection workspace.
#[cfg(feature = "rust_inspect")]
fn install_oven_inspection_source_authority(
    manifest_dir: &Path,
    packages: &[crate::oven::rustc::OvenRustcRegistrySourcePackage],
    artifact_root: &Path,
    extension_paths: Option<&BTreeSet<String>>,
    base_source_authority: Option<(&Path, &[crate::oven::rustc::OvenRustcRegistrySourcePackage])>,
) -> CliResult<()> {
    let sources = oven_inspection_sources(packages, artifact_root, extension_paths, base_source_authority)?;
    crate::rust_inspect::write_sealed_oven_inspection_source_authority(manifest_dir, sources)
        .map(|_| ())
        .map_err(|error| CliError::failure(format!("failed to install Oven Rust source authority: {error}")))
}

/// Resolve a sealed registry catalog to immutable source roots without writing caller-owned projection state.
#[cfg(feature = "rust_inspect")]
fn oven_inspection_sources(
    packages: &[crate::oven::rustc::OvenRustcRegistrySourcePackage],
    artifact_root: &Path,
    extension_paths: Option<&BTreeSet<String>>,
    base_source_authority: Option<(&Path, &[crate::oven::rustc::OvenRustcRegistrySourcePackage])>,
) -> CliResult<Vec<crate::rust_inspect::OvenInspectionRegistrySource>> {
    packages
        .iter()
        .map(|package| {
            let extension_owns_source = extension_paths.is_some_and(|paths| {
                let prefix = format!("{}/", package.source.relative_root);
                paths.iter().any(|path| path.starts_with(&prefix))
            });
            let source_root = if extension_paths.is_none() || extension_owns_source {
                artifact_root.join(&package.source.relative_root)
            } else {
                let (base_root, base_packages) = base_source_authority.ok_or_else(|| {
                    CliError::failure(format!(
                        "stored Oven project extension omits sealed source `{}` {} without an exact base Loaf authority",
                        package.package, package.version
                    ))
                })?;
                let mut matching = base_packages.iter().filter(|base| {
                    base.package == package.package
                        && base.version == package.version
                        && base.source.registry == package.source.registry
                        && base.source.checksum == package.source.checksum
                        && base.source.digest == package.source.digest
                });
                let Some(base) = matching.next() else {
                    return Err(CliError::failure(format!(
                        "stored Oven project extension source `{}` {} is absent from its required base Loaf",
                        package.package, package.version
                    )));
                };
                if matching.next().is_some() {
                    return Err(CliError::failure(format!(
                        "stored Oven project extension source `{}` {} is ambiguous in its required base Loaf",
                        package.package, package.version
                    )));
                }
                base_root.join(&base.source.relative_root)
            };
            Ok(crate::rust_inspect::OvenInspectionRegistrySource {
                package: package.package.clone(),
                version: package.version.clone(),
                registry: package.source.registry.clone(),
                checksum: package.source.checksum.clone(),
                features: package.features.clone(),
                source_root,
                source_digest: package.source.digest.clone(),
            })
        })
        .collect()
}

/// Return whether a sealed source catalog owns the requested immutable Cargo source archive.
///
/// This check guards source-root hand-off only; it does not authorize reuse of compiled Rust artifacts, whose
/// feature-sensitive receipts are validated by the direct-rustc plan.
#[cfg(feature = "rust_inspect")]
pub(crate) fn registry_source_is_owned_by_catalog(
    source: &crate::oven::rustc::OvenRustcRegistrySourcePackage,
    catalog: &[crate::oven::rustc::OvenRustcRegistrySourcePackage],
) -> bool {
    catalog.iter().any(|candidate| {
        candidate.package == source.package && candidate.version == source.version && candidate.source == source.source
    })
}

/// Prepare the rust-inspect workspace needed before metadata-backed typechecking.
#[cfg(feature = "rust_inspect")]
pub(crate) fn prepare_rust_inspect_typecheck_workspace(
    request: RustInspectTypecheckRequest<'_>,
) -> CliResult<Option<PreparedRustInspectTypecheckWorkspace>> {
    let RustInspectTypecheckRequest {
        project_root,
        project_name,
        manifest,
        modules,
        library_manifest_index,
        cargo_features,
        cargo_policy,
        rust_edition,
        provider_plan,
    } = request;
    let metadata_query_paths = collect_rust_inspect_query_paths(modules);
    let rust_derive_probe_paths = collect_rust_inspect_derive_probe_paths(modules);
    // A quoted `@rust.derive("crate::path::Macro")` needs a prepared workspace for its expansion evidence even when
    // the module imports no other `rust::` items, so probe paths keep this preparation alive on their own.
    if metadata_query_paths.is_empty() && rust_derive_probe_paths.is_empty() {
        return Ok(None);
    }

    let project_requirements = collect_project_requirements(modules, library_manifest_index)?;
    let inline_imports = modules
        .iter()
        .flat_map(|module| collect_rust_dependency_uses(module, false))
        .collect::<Vec<_>>();
    let mut resolved = match resolve_reachable_dependencies(manifest, &inline_imports, true, cargo_features) {
        Ok(resolved) => resolved,
        Err(errors) => {
            let mut msg = String::new();
            let sources = build_source_map(modules);
            for err in errors {
                msg.push_str(&format_dependency_error(&err, &sources));
            }
            return Err(CliError::failure(msg.trim_end()));
        }
    };
    merge_project_requirement_dependencies(&mut resolved, &project_requirements)?;
    let lock_resolution = resolve_lock_context(LockResolutionRequest {
        project_root,
        project_name,
        entry_file: modules.last().map(|module| module.file_path.as_path()),
        manifest,
        resolved: &resolved,
        project_requirements: &project_requirements,
        cargo_features,
        cargo_policy,
        semantic: None,
        package_features: None,
        sdk_profile_override: None,
    })?;
    let cargo_lock_inputs = lock_resolution.cargo_lock_authority.into_generator_inputs();
    let rust_inspect_cargo_flags = cargo_command_flags(cargo_policy, cargo_features);
    let managed_target = resolve_generated_cargo_target(
        None,
        project_root,
        project_root,
        &lock_resolution.cargo_package_name,
        "rust-inspect",
        cargo_lock_inputs.payload.as_deref(),
        cargo_features,
        &rust_inspect_cargo_flags,
    )
    .map_err(|error| CliError::failure(format!("failed to prepare rust-inspect Cargo cache: {error}")))?;
    let (cargo_target_dir, cache_lease, _cache_identity) = managed_target.into_parts();
    let oven_build_inputs = crate::driver::build_unit::oven_build_unit_inputs(
        provider_plan,
        &lock_resolution.project_requirements,
        &lock_resolution.resolved,
    )?;
    let rustc = resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
    let target = rustc_host_target(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let toolchain = rustc_identity(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let project_version = manifest
        .and_then(|manifest| manifest.project.as_ref())
        .and_then(|project| project.version.as_deref())
        .unwrap_or("0.1.0");
    let manifest_dir = prepare_rust_inspect_workspace(RustInspectWorkspaceRequest {
        project_root,
        project_name,
        cargo_package_name: &lock_resolution.cargo_package_name,
        rust_edition,
        resolved: &lock_resolution.resolved,
        project_requirements: &lock_resolution.project_requirements,
        lock_payload: cargo_lock_inputs.payload,
        cargo_lock_projection_root: cargo_lock_inputs.projection_root.as_deref(),
        clear_cargo_lock: cargo_lock_inputs.clear_existing,
        cargo_policy_flags: rust_inspect_cargo_flags,
        cargo_target_dir: &cargo_target_dir,
        rust_inspect_query_paths: &metadata_query_paths,
        rust_derive_probe_paths: &rust_derive_probe_paths,
        prepare_when_empty: false,
        // This workspace supports ordinary `incan check` metadata queries. It is an inspectable projection only:
        // Rust-analyzer must load its receipt-derived `rust-project.json`, not rediscover a Cargo workspace from a
        // path dependency. Besides respecting the normal Oven boundary, that avoids Cargo's global package-name
        // ambiguity for independently selected workspace members.
        direct_oven_inspection: true,
        force_direct_prewarm: false,
        oven_source_authority: Some(OvenRustInspectSourceAuthorityRequest {
            project_version,
            target: &target,
            toolchain: &toolchain,
            profile: "debug",
            features: &cargo_features.cargo_features,
            build_unit_inputs: &oven_build_inputs,
            registry_dependencies: &lock_resolution.resolved.dependencies,
        }),
        prepared_project_source_authorities: None,
        explicit_oven_bake: false,
    })?;
    Ok(manifest_dir.map(|workspace| PreparedRustInspectTypecheckWorkspace {
        manifest_dir: workspace.manifest_dir,
        _cache_lease: cache_lease,
        _source_loaf: workspace._source_loaf,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn registry_source_ownership_unifies_feature_variants_with_the_same_source_archive() {
        let source = crate::oven::rustc::OvenRustcRegistrySource {
            registry: "registry+https://example.invalid/index".to_string(),
            checksum: "fixture-checksum".to_string(),
            relative_root: "registry-sources/syn".to_string(),
            digest: "sha256:fixture-source".to_string(),
        };
        let requested = crate::oven::rustc::OvenRustcRegistrySourcePackage {
            package: "syn".to_string(),
            version: "2.0.117".to_string(),
            features: vec!["derive".to_string()],
            source: source.clone(),
        };
        let owner = crate::oven::rustc::OvenRustcRegistrySourcePackage {
            features: vec!["clone-impls".to_string(), "full".to_string()],
            ..requested.clone()
        };

        assert!(registry_source_is_owned_by_catalog(&requested, &[owner]));
        let different_archive = crate::oven::rustc::OvenRustcRegistrySourcePackage {
            source: crate::oven::rustc::OvenRustcRegistrySource {
                checksum: "other-fixture-checksum".to_string(),
                ..source
            },
            ..requested.clone()
        };
        assert!(!registry_source_is_owned_by_catalog(&different_archive, &[requested]));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn custom_project_layout_cannot_fall_through_after_command_authority_mismatch()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("library"))?;
        fs::create_dir_all(project.path().join("bin"))?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"custom-layout\"\n\n[project.scripts]\nworker = \"bin/worker.incn\"\n\n[build]\nsource-root = \"library\"\n",
        )?;
        fs::write(
            project.path().join("library/lib.incn"),
            "pub def value() -> int:\n    return 1\n",
        )?;
        fs::write(
            project.path().join("bin/worker.incn"),
            "def main() -> None:\n    pass\n",
        )?;

        assert!(!project.path().join("src/lib.incn").exists());
        assert!(!project.path().join("src/main.incn").exists());
        assert!(normal_inspection_requires_installed_project_authority(
            project.path(),
            false,
            true,
            false,
        ));
        assert!(!normal_inspection_requires_installed_project_authority(
            project.path(),
            false,
            true,
            true,
        ));
        assert!(!normal_inspection_requires_installed_project_authority(
            project.path(),
            true,
            true,
            false,
        ));

        let standalone = tempfile::tempdir()?;
        assert!(!normal_inspection_requires_installed_project_authority(
            standalone.path(),
            false,
            false,
            false,
        ));

        let conventional = tempfile::tempdir()?;
        fs::create_dir_all(conventional.path().join("src"))?;
        fs::write(
            conventional.path().join("src/main.incn"),
            "def main() -> None:\n    pass\n",
        )?;
        assert!(normal_inspection_requires_installed_project_authority(
            conventional.path(),
            false,
            false,
            false,
        ));
        Ok(())
    }
}
