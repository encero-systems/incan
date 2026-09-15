//! The registry source authorities an explicit project bake installs and a normal command consumes: which sealed
//! inspection sources a project's dependencies resolve to, and the Oven registry lock they must agree with.
//!
//! The whole module rides the `rust_inspect` feature: without the inspector there is nothing here to prepare.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::dependency_resolver::ResolvedDependencies;
use crate::driver::error::{CliError, CliResult};
use crate::driver::lock::PreparedOvenProjectRegistrySourceAuthorities;
use crate::driver::lock::rust_inspect::registry_source_is_owned_by_catalog;
use crate::manifest::DependencySpec;
use crate::oven::legacy_cargo::OvenLegacyCargoInspectionPackage;
use crate::oven::legacy_cargo::cargo_process::resolved_cargo_executable;
use crate::oven::legacy_cargo::explicit_project_bake_inspection_sources;
use crate::oven::loaf::resolve_compiler_owned_loaf_by_identity;
use crate::oven::rustc::OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH;
use crate::oven::rustc::OvenLoadedProjectInspectionAuthority;
use crate::oven::rustc::OvenProjectInspectionConstituent;
use crate::oven::rustc::OvenProjectInspectionSourceOwner;
use crate::oven::rustc::project_inspection_authority_supports_dependencies;
use crate::oven::rustc::project_inspection_test_dependency_envelope_supports_dependencies;
use crate::oven::rustc::validate_project_extension_payload_against_base;

/// Resolve one exact project authority and all named constituents once for the complete test command.
pub(crate) fn prepare_project_registry_source_authorities(
    mut authority: OvenLoadedProjectInspectionAuthority,
) -> CliResult<Arc<PreparedOvenProjectRegistrySourceAuthorities>> {
    struct ResolvedSourceOwner {
        root: PathBuf,
        catalog: Vec<crate::oven::rustc::OvenRustcRegistrySourcePackage>,
    }

    let mut release_loafs = Vec::new();
    let mut owners = Vec::with_capacity(authority.payload.constituents.len());
    let mut stored_index = 0;
    let mut test_dependency_stored_index = None;
    let mut test_dependency_release_identity = None;
    for (constituent_index, constituent) in authority.payload.constituents.iter().enumerate() {
        match constituent {
            OvenProjectInspectionConstituent::ReleaseLoaf {
                loaf_identity,
                build_unit_identity,
                receipt,
            } => {
                let loaf = resolve_compiler_owned_loaf_by_identity(receipt, loaf_identity)
                    .map_err(|error| CliError::failure(error.to_string()))?
                    .ok_or_else(|| {
                        CliError::failure(format!(
                            "project inspection authority requires release Loaf `{loaf_identity}`, but the active toolchain does not provide it"
                        ))
                    })?;
                if loaf.loaf_build_unit_identity != *build_unit_identity || loaf.artifacts.intent != receipt.intent {
                    return Err(CliError::failure(format!(
                        "project inspection authority release Loaf `{loaf_identity}` has different build-unit or intent evidence"
                    )));
                }
                owners.push(ResolvedSourceOwner {
                    root: loaf.artifact_root.clone(),
                    catalog: loaf.artifacts.registry_sources.clone(),
                });
                if authority
                    .payload
                    .test_dependency_envelope
                    .as_ref()
                    .is_some_and(|envelope| envelope.constituent_index == constituent_index)
                {
                    test_dependency_release_identity = Some(loaf.loaf_identity.clone());
                }
                release_loafs.push(loaf);
            }
            OvenProjectInspectionConstituent::Stored {
                artifact_kind,
                base_loaf_identity,
                ..
            } => {
                if authority
                    .payload
                    .test_dependency_envelope
                    .as_ref()
                    .is_some_and(|envelope| envelope.constituent_index == constituent_index)
                {
                    test_dependency_stored_index = Some(stored_index);
                }
                let selected = authority.stored_constituents.get(stored_index).ok_or_else(|| {
                    CliError::failure("project inspection authority lost a store constituent during preparation")
                })?;
                stored_index += 1;
                let catalog = match artifact_kind {
                    crate::oven::store::OvenArtifactKind::DirectRustcPlan => {
                        serde_json::from_slice::<crate::oven::rustc::OvenRustcArtifactManifest>(&selected.payload)
                            .map_err(|error| {
                                CliError::failure(format!(
                                    "project inspection direct-plan constituent is invalid: {error}"
                                ))
                            })?
                            .registry_sources
                    }
                    crate::oven::store::OvenArtifactKind::ProjectPayload => {
                        let payload = serde_json::from_slice::<crate::oven::legacy_cargo::OvenProjectExtensionPayload>(
                            &selected.payload,
                        )
                        .map_err(|error| {
                            CliError::failure(format!("project inspection extension constituent is invalid: {error}"))
                        })?;
                        let base_identity = base_loaf_identity.as_deref().ok_or_else(|| {
                            CliError::failure("project inspection extension constituent omitted its release Loaf")
                        })?;
                        let base = release_loafs
                            .iter()
                            .find(|loaf| loaf.loaf_identity == base_identity)
                            .ok_or_else(|| {
                                CliError::failure(format!(
                                    "project inspection extension requires unlisted release Loaf `{base_identity}`"
                                ))
                            })?;
                        validate_project_extension_payload_against_base(
                            &payload,
                            &base.loaf_identity,
                            &base.loaf_build_unit_identity,
                            &base.artifacts,
                        )
                        .map_err(|error| CliError::failure(error.to_string()))?;
                        payload.complete_plan.registry_sources
                    }
                    unsupported => {
                        return Err(CliError::failure(format!(
                            "project inspection authority names unsupported constituent kind {unsupported:?}"
                        )));
                    }
                };
                owners.push(ResolvedSourceOwner {
                    root: selected.artifact_root.clone(),
                    catalog,
                });
            }
        }
    }

    let mut sources = Vec::with_capacity(authority.payload.registry_sources.len());
    for source in &authority.payload.registry_sources {
        let (root, catalog) = match source.owner {
            OvenProjectInspectionSourceOwner::Authority => {
                (authority.artifact_root(), std::slice::from_ref(&source.package))
            }
            OvenProjectInspectionSourceOwner::Constituent { index } => {
                let owner = owners.get(index).ok_or_else(|| {
                    CliError::failure(format!(
                        "project inspection source references missing constituent index {index}"
                    ))
                })?;
                (owner.root.as_path(), owner.catalog.as_slice())
            }
        };
        if !registry_source_is_owned_by_catalog(&source.package, catalog) {
            return Err(CliError::failure(format!(
                "project inspection source `{}` {} has no exact record in its named owner",
                source.package.package, source.package.version
            )));
        }
        sources.push(crate::rust_inspect::OvenInspectionRegistrySource {
            package: source.package.package.clone(),
            version: source.package.version.clone(),
            registry: source.package.source.registry.clone(),
            checksum: source.package.source.checksum.clone(),
            features: source.package.features.clone(),
            source_root: root.join(&source.package.source.relative_root),
            source_digest: source.package.source.digest.clone(),
        });
    }
    let registry_lock_source = if sources.is_empty() {
        None
    } else {
        let path = authority.artifact_root().join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            CliError::failure(format!(
                "project inspection authority lacks its sealed Cargo.lock at {}: {error}",
                path.display()
            ))
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(CliError::failure(format!(
                "project inspection authority registry lock is not a regular file at {}",
                path.display()
            )));
        }
        Some(path)
    };
    // The explicit bake sealed its Cargo bootstrap's build-script output below the authority root; those files are
    // the only Cargo-free source of generated Rust (prost modules, for one) a direct inspection workspace can read.
    let generated_out_dirs = authority
        .payload
        .generated_out_dirs
        .iter()
        .map(|dir| crate::rust_inspect::SealedGeneratedOutDir {
            out_dir: authority.artifact_root().join(&dir.relative_root),
            version: dir.version.clone(),
        })
        .collect::<Vec<_>>();
    let test_dependency_plan = if let Some(stored_index) = test_dependency_stored_index {
        let constituent_index = authority
            .payload
            .test_dependency_envelope
            .as_ref()
            .ok_or_else(|| CliError::failure("project inspection authority lost its test dependency role"))?
            .constituent_index;
        let receipt = match authority.payload.constituents.get(constituent_index) {
            Some(OvenProjectInspectionConstituent::Stored { receipt, .. }) => receipt.clone(),
            _ => {
                return Err(CliError::failure(
                    "project inspection authority test dependency role no longer names a stored constituent",
                ));
            }
        };
        let selected = authority.stored_constituents.remove(stored_index);
        Some(
            crate::oven::plan::selection::project_test_dependency_plan_from_constituent(selected, &receipt)
                .map_err(crate::driver::error::oven_plan_error)?,
        )
    } else if let Some(identity) = test_dependency_release_identity {
        let index = release_loafs
            .iter()
            .position(|loaf| loaf.loaf_identity == identity)
            .ok_or_else(|| CliError::failure("project inspection authority lost its role-bearing release Loaf"))?;
        Some(crate::oven::plan::OvenDirectRustcPlanSelection::ToolchainLoaf(
            Box::new(release_loafs.remove(index)),
        ))
    } else {
        if authority.payload.test_dependency_envelope.is_some() {
            return Err(CliError::failure(
                "project inspection authority test dependency role did not resolve to its exact constituent",
            ));
        }
        None
    };
    Ok(Arc::new(PreparedOvenProjectRegistrySourceAuthorities {
        authority,
        sources,
        registry_lock_source,
        generated_out_dirs,
        test_dependency_plan,
        _release_loafs: release_loafs,
    }))
}

impl PreparedOvenProjectRegistrySourceAuthorities {
    /// Return the exact role-bearing dependency envelope after validating this generated batch's complete surface.
    pub(crate) fn test_dependency_plan(
        &self,
        dependencies: &[DependencySpec],
    ) -> CliResult<Option<&crate::oven::plan::OvenDirectRustcPlanSelection>> {
        if self.authority.payload.test_dependency_envelope.is_none() {
            return Ok(None);
        }
        let promoted = crate::driver::build_unit::promoted_oven_test_dependencies(&ResolvedDependencies {
            dependencies: dependencies.to_vec(),
            dev_dependencies: Vec::new(),
        })?;
        if !project_inspection_authority_supports_dependencies(&self.authority.payload, &promoted) {
            return Err(project_inspection_selection_mismatch("this test dependency subset"));
        }
        if !project_inspection_test_dependency_envelope_supports_dependencies(
            &self.authority.payload,
            &promoted,
            crate::oven_facet::provider_hooks().as_ref(),
        )
        .map_err(|error| CliError::failure(error.to_string()))?
        {
            let expected = self
                .authority
                .payload
                .test_dependency_envelope
                .as_ref()
                .map(|envelope| envelope.dependency_roots.keys().cloned().collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            let actual = promoted
                .iter()
                .map(|dependency| dependency.crate_name.replace('-', "_"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(CliError::failure(format!(
                "Oven Alpha project inspection authority has a missing, stale, or incompatible test dependency root (sealed aliases: [{expected}]; requested aliases: [{actual}]); rerun `incan oven bake --project .`"
            )));
        }
        self.test_dependency_plan.as_ref().map(Some).ok_or_else(|| {
            CliError::failure(
                "project inspection authority lost its exact test dependency plan while retaining its role",
            )
        })
    }

    /// Bind generated test receipts to the exact project authority selected once for this command.
    pub(crate) fn authority_identity(&self) -> &str {
        self.authority.identity()
    }

    /// Project the one complete exact authority for one generated test batch.
    pub(super) fn install_for_dependencies(
        &self,
        manifest_dir: &Path,
        dependencies: &[DependencySpec],
    ) -> CliResult<bool> {
        let registry_dependency_count = dependencies
            .iter()
            .filter(|dependency| matches!(dependency.source, crate::manifest::DependencySource::Registry))
            .count();
        if registry_dependency_count == 0 {
            // The exact project/output/authority leases are still the conventional project's command authority. No
            // registry projection is needed, but this must not fall through to generic compatibility selection.
            return Ok(true);
        }
        if !project_inspection_authority_supports_dependencies(&self.authority.payload, dependencies) {
            return Err(project_inspection_selection_mismatch(
                "the requested normal and dev registry dependencies",
            ));
        }
        crate::rust_inspect::write_sealed_oven_inspection_source_authority(manifest_dir, self.sources.clone())
            .map_err(|error| CliError::failure(format!("failed to install Oven Rust source authority: {error}")))?;
        crate::rust_inspect::write_oven_generated_out_dirs(manifest_dir, &self.generated_out_dirs).map_err(
            |error| CliError::failure(format!("failed to install Oven generated output directories: {error}")),
        )?;
        if let Some(lock) = self.registry_lock_source.as_deref() {
            install_oven_registry_lock(lock, &manifest_dir.join("Cargo.lock"))?;
        }
        Ok(true)
    }
}

/// Build the diagnostic for a completed project Loaf that does not cover the requested inspection surface.
fn project_inspection_selection_mismatch(requested_surface: &str) -> CliError {
    CliError::failure(format!(
        "Oven Alpha project inspection authority does not cover {requested_surface}. The command selected registry roots outside the completed project Loaf's baked dependency surface. A command-local `--sdk-profile` or package-feature selection cannot reuse a Loaf baked for different roots. Use the baked selection; for a different SDK profile, persist it in `[sdk]` in `loaf.toml` and rebake; for different package features, rerun `incan oven bake --project .` with the same feature flags."
    ))
}

/// Resolve the exact direct registry roots whose source trees must be available while checking one project.
/// Translate declared registry dependencies into the exact root selectors shared by source inspection and the
/// explicit Oven publisher. Keeping this conversion here gives both phases one package/rename/version boundary.
pub(crate) fn inspection_packages_for_dependencies(
    dependencies: &[DependencySpec],
) -> CliResult<Vec<OvenLegacyCargoInspectionPackage>> {
    let mut packages = dependencies
        .iter()
        .filter(|dependency| matches!(dependency.source, crate::manifest::DependencySource::Registry))
        .map(|dependency| {
            let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
            let version_requirement = dependency.version.as_deref().ok_or_else(|| {
                CliError::failure(format!(
                    "Oven inspection source declaration for `{package}` is missing its locked version requirement"
                ))
            })?;
            Ok(OvenLegacyCargoInspectionPackage {
                package: package.to_string(),
                version_requirement: version_requirement.to_string(),
            })
        })
        .collect::<CliResult<Vec<_>>>()?;
    packages.sort();
    packages.dedup();
    Ok(packages)
}

/// Acquire source authority only while the user explicitly publishes a project Loaf.
///
/// This is deliberately a metadata-only, locked/offline Cargo invocation. Its copied, digested source trees drive
/// the immediate direct inspection pass; the following project publisher seals the same checked package closure into
/// the receipt-bound plan. Normal build, run, and test never reach this helper.
pub(crate) fn acquire_explicit_project_inspection_sources(
    manifest_dir: &Path,
    features: &[String],
    dependencies: &[DependencySpec],
    release_registry_lock: Option<&Path>,
) -> CliResult<()> {
    let authority_root = manifest_dir.join("oven-inspection-authority");
    fs::create_dir_all(&authority_root).map_err(|error| {
        CliError::failure(format!(
            "failed to create explicit Oven inspection-source authority at {}: {error}",
            authority_root.display()
        ))
    })?;
    let packages = inspection_packages_for_dependencies(dependencies)?;
    let cargo = resolved_cargo_executable()
        .map_err(|error| CliError::failure(format!("cannot resolve Cargo for explicit Oven bake: {error}")))?;
    let sources = explicit_project_bake_inspection_sources(
        &cargo,
        &manifest_dir.join("Cargo.toml"),
        features,
        &packages,
        &authority_root,
        release_registry_lock,
    )
    .map_err(|error| CliError::failure(error.to_string()))?;
    let sources = sources
        .into_iter()
        .map(|source| crate::rust_inspect::OvenInspectionRegistrySource {
            package: source.package,
            version: source.version,
            registry: source.registry,
            checksum: source.checksum,
            features: source.features,
            source_root: source.source_root,
            source_digest: source.source_digest,
        })
        .collect();
    crate::rust_inspect::write_sealed_oven_inspection_source_authority(manifest_dir, sources)
        .map(|_| ())
        .map_err(|error| CliError::failure(format!("failed to install explicit Oven inspection authority: {error}")))
}

/// Install a sealed registry lock as writable caller-owned inspection state.
///
/// Loaf artifacts are intentionally read-only. Copying their filesystem permissions into the mutable inspection
/// workspace makes the first projection impossible to replace on reuse, so only the verified bytes cross this
/// ownership boundary.
fn install_oven_registry_lock(source: &Path, destination: &Path) -> CliResult<()> {
    let payload = fs::read(source).map_err(|error| {
        CliError::failure(format!(
            "failed to read Loaf registry lock from {}: {error}",
            source.display()
        ))
    })?;
    if destination.exists() {
        fs::remove_file(destination).map_err(|error| {
            CliError::failure(format!(
                "failed to replace projected Loaf registry lock {}: {error}",
                destination.display()
            ))
        })?;
    }
    fs::write(destination, payload).map_err(|error| {
        CliError::failure(format!(
            "failed to install Loaf registry lock from {}: {error}",
            source.display()
        ))
    })
}

/// Require the exact checked graph whenever a Loaf supplies registry source authority.
///
/// The Rust-inspection loader may otherwise see the copied source directories while resolving their dependencies
/// against ambient state. A missing lock is an invalid Loaf, not permission to consult Cargo or a local registry.
pub(crate) fn install_required_oven_registry_lock(
    has_registry_sources: bool,
    artifact_root: &Path,
    destination: &Path,
) -> CliResult<()> {
    if !has_registry_sources {
        return Ok(());
    }
    let sealed_lock = artifact_root.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
    let metadata = fs::symlink_metadata(&sealed_lock).map_err(|error| {
        CliError::failure(format!(
            "selected Oven Loaf declares registry sources but lacks its sealed Cargo.lock at {}: {error}",
            sealed_lock.display()
        ))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CliError::failure(format!(
            "selected Oven Loaf declares registry sources but its sealed Cargo.lock is not a regular file at {}",
            sealed_lock.display()
        )));
    }
    install_oven_registry_lock(&sealed_lock, destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(test)]
    #[test]
    fn sealed_oven_registry_lock_can_replace_a_prior_projection() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let sealed_lock = temp_dir.path().join("sealed.lock");
        let projected_lock = temp_dir.path().join("Cargo.lock");
        fs::write(&sealed_lock, "version = 4\n")?;
        let mut sealed_permissions = fs::metadata(&sealed_lock)?.permissions();
        sealed_permissions.set_readonly(true);
        fs::set_permissions(&sealed_lock, sealed_permissions)?;

        install_oven_registry_lock(&sealed_lock, &projected_lock)?;
        let mut projected_permissions = fs::metadata(&projected_lock)?.permissions();
        projected_permissions.set_readonly(true);
        fs::set_permissions(&projected_lock, projected_permissions)?;
        install_oven_registry_lock(&sealed_lock, &projected_lock)?;

        assert_eq!(fs::read_to_string(&projected_lock)?, "version = 4\n");
        assert!(!fs::metadata(&projected_lock)?.permissions().readonly());
        Ok(())
    }

    #[test]
    fn registry_sources_refuse_a_loaf_without_its_sealed_lock() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let destination = temp_dir.path().join("Cargo.lock");

        let Err(error) = install_required_oven_registry_lock(true, temp_dir.path(), &destination) else {
            return Err(std::io::Error::other("missing sealed registry lock was accepted").into());
        };

        assert!(error.to_string().contains("declares registry sources"));
        assert!(!destination.exists());
        Ok(())
    }

    #[test]
    fn project_inspection_selection_mismatch_explains_the_transient_selection_boundary() {
        let diagnostic = project_inspection_selection_mismatch("the requested registry dependencies").to_string();

        assert!(diagnostic.contains("command-local `--sdk-profile` or package-feature selection"));
        assert!(diagnostic.contains("persist it in `[sdk]` in `loaf.toml` and rebake"));
        assert!(diagnostic.contains("with the same feature flags"));
    }
}
