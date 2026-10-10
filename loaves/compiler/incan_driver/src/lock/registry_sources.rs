//! The registry source authorities an explicit project bake installs and a normal command consumes: which sealed
//! inspection sources a project's dependencies resolve to, and the Oven registry lock they must agree with.
//!
//! The whole module rides the `rust_inspect` feature: without the inspector there is nothing here to prepare.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{CliError, CliResult};
use crate::lock::PreparedOvenProjectRegistrySourceAuthorities;
use crate::lock::rust_inspect::registry_source_is_owned_by_catalog;
use incan_provider::dependency_resolver::ResolvedDependencies;
use oven_cargo_compat::OvenLegacyCargoInspectionPackage;
use oven_model::manifest::DependencySpec;
use oven_rustc::loaf::resolve_compiler_owned_loaf_by_identity;
use oven_rustc::plan::composition::{compose_direct_packaged_provider_plan, compose_packaged_provider_plan};
use oven_rustc::plan::{
    OvenDirectRustcPlanSelection, OvenPackagedLibraryLoafEntry, OvenProjectExtensionExecutionPlan,
    OvenStoredDirectRustcExecutionPlan,
};
use oven_rustc::rustc::OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH;
use oven_rustc::rustc::OvenLoadedProjectInspectionAuthority;
use oven_rustc::rustc::OvenProjectInspectionConstituent;
use oven_rustc::rustc::OvenProjectInspectionSourceOwner;
use oven_rustc::rustc::project_inspection_authority_supports_dependencies;
use oven_rustc::rustc::project_inspection_test_dependency_envelope_mismatch;
use oven_rustc::rustc::validate_project_extension_payload_against_base;

/// Resolve one exact project authority and all named constituents once for the complete test command.
pub fn prepare_project_registry_source_authorities(
    authority: OvenLoadedProjectInspectionAuthority,
) -> CliResult<Arc<PreparedOvenProjectRegistrySourceAuthorities>> {
    prepare_project_registry_source_authorities_with_native_sdk(authority, None)
}

/// Prepare canonical command-owned inspection authority while reusing its explicitly admitted native SDK.
pub fn prepare_project_registry_source_authorities_with_native_sdk(
    authority: OvenLoadedProjectInspectionAuthority,
    native_sdk_context: Option<Arc<crate::build::NativeSdkCommandContext>>,
) -> CliResult<Arc<PreparedOvenProjectRegistrySourceAuthorities>> {
    match prepare_project_registry_source_authorities_inner(authority, native_sdk_context, false)? {
        ProjectRegistrySourceAuthoritySelection::Prepared(prepared) => Ok(prepared),
        ProjectRegistrySourceAuthoritySelection::StaleNativeGeneration => Err(CliError::failure(
            "receipt-selected native SDK differs from command admission",
        )),
    }
}

/// Outcome of selecting optional completed-project inspection lineage for a current command.
pub enum ProjectRegistrySourceAuthoritySelection {
    /// Exact original authority and compatible test dependency plan retained for the command.
    Prepared(Arc<PreparedOvenProjectRegistrySourceAuthorities>),
    /// Intact historical lineage names another native generation and cannot authorize this command.
    StaleNativeGeneration,
}

/// Prepare optional project lineage without making an old native generation mandatory for fresh compilation.
///
/// Only an intact, canonically admitted authority with a different native receipt catalog becomes stale. Damage,
/// malformed bindings and changes to the current command's admission remain errors. A stale result grants no
/// dependency authority: the caller must prepare its actual declarations using its current retained native owners.
pub fn prepare_optional_project_registry_source_authorities_with_native_sdk(
    authority: OvenLoadedProjectInspectionAuthority,
    native_sdk_context: Option<Arc<crate::build::NativeSdkCommandContext>>,
) -> CliResult<ProjectRegistrySourceAuthoritySelection> {
    prepare_project_registry_source_authorities_inner(authority, native_sdk_context, true)
}

/// Share exact source and constituent preparation while allowing only optional callers to reject stale lineage.
fn prepare_project_registry_source_authorities_inner(
    mut authority: OvenLoadedProjectInspectionAuthority,
    native_sdk_context: Option<Arc<crate::build::NativeSdkCommandContext>>,
    optional: bool,
) -> CliResult<ProjectRegistrySourceAuthoritySelection> {
    if optional {
        authority
            .verify()
            .map_err(|error| CliError::failure(error.to_string()))?;
    }
    struct ResolvedSourceCatalog {
        root: PathBuf,
        packages: Vec<oven_rustc::rustc::OvenRustcRegistrySourcePackage>,
    }

    struct ResolvedSourceOwner {
        catalogs: Vec<ResolvedSourceCatalog>,
    }

    let mut release_loafs = Vec::new();
    let mut owners = Vec::with_capacity(authority.payload.constituents.len());
    let mut stored_index = 0;
    let test_dependency_roles = authority
        .payload
        .test_dependency_envelope
        .as_ref()
        .map(|envelope| {
            std::iter::once((envelope.constituent_index, None))
                .chain(
                    envelope
                        .provider_constituents
                        .iter()
                        .map(|provider| (provider.constituent_index, Some(provider.dependency_key.clone()))),
                )
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut test_dependency_stored_roles = Vec::new();
    let mut test_dependency_release_identity = None;
    let mut native_receipts = Vec::new();
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
                    catalogs: vec![ResolvedSourceCatalog {
                        root: loaf.artifact_root.clone(),
                        packages: loaf.artifacts.registry_sources.clone(),
                    }],
                });
                if test_dependency_roles
                    .get(&constituent_index)
                    .is_some_and(Option::is_none)
                {
                    test_dependency_release_identity = Some(loaf.loaf_identity.clone());
                }
                release_loafs.push(loaf);
            }
            OvenProjectInspectionConstituent::Stored {
                artifact_kind,
                base_loaf_identity,
                receipt,
                ..
            } => {
                if let Some(dependency_key) = test_dependency_roles.get(&constituent_index) {
                    test_dependency_stored_roles.push((constituent_index, stored_index, dependency_key.clone()));
                }
                let selected = authority.stored_constituents.get(stored_index).ok_or_else(|| {
                    CliError::failure("project inspection authority lost a store constituent during preparation")
                })?;
                stored_index += 1;
                if selected.manifest.domain == "sdk-native-consumer-plan" {
                    if optional && !receipt.sources.build_unit_inputs.contains_key("sdk-native-closure") {
                        return Err(CliError::failure(
                            "native project inspection constituent lacks its receipt catalog identity",
                        ));
                    }
                    native_receipts.push(receipt);
                }
                let catalogs = match artifact_kind {
                    oven_store::store::OvenArtifactKind::DirectRustcPlan => {
                        let packages =
                            serde_json::from_slice::<oven_rustc::rustc::OvenRustcArtifactManifest>(&selected.payload)
                                .map_err(|error| {
                                    CliError::failure(format!(
                                        "project inspection direct-plan constituent is invalid: {error}"
                                    ))
                                })?
                                .registry_sources;
                        vec![ResolvedSourceCatalog {
                            root: selected.artifact_root.clone(),
                            packages,
                        }]
                    }
                    oven_store::store::OvenArtifactKind::ProjectPayload => {
                        let payload =
                            serde_json::from_slice::<oven_cargo_compat::OvenProjectExtensionPayload>(&selected.payload)
                                .map_err(|error| {
                                    CliError::failure(format!(
                                        "project inspection extension constituent is invalid: {error}"
                                    ))
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
                        let extension_packages = payload
                            .complete_plan
                            .registry_sources
                            .iter()
                            .filter(|package| extension_owns_registry_source(&payload.extension_paths, package))
                            .cloned()
                            .collect();
                        vec![
                            ResolvedSourceCatalog {
                                root: selected.artifact_root.clone(),
                                packages: extension_packages,
                            },
                            ResolvedSourceCatalog {
                                root: base.artifact_root.clone(),
                                packages: base.artifacts.registry_sources.clone(),
                            },
                        ]
                    }
                    unsupported => {
                        return Err(CliError::failure(format!(
                            "project inspection authority names unsupported constituent kind {unsupported:?}"
                        )));
                    }
                };
                owners.push(ResolvedSourceOwner { catalogs });
            }
        }
    }

    let mut sources = Vec::with_capacity(authority.payload.registry_sources.len());
    for source in &authority.payload.registry_sources {
        let root = match source.owner {
            OvenProjectInspectionSourceOwner::Authority => authority.artifact_root(),
            OvenProjectInspectionSourceOwner::Constituent { index } => {
                let owner = owners.get(index).ok_or_else(|| {
                    CliError::failure(format!(
                        "project inspection source references missing constituent index {index}"
                    ))
                })?;
                owner
                    .catalogs
                    .iter()
                    .find(|catalog| registry_source_is_owned_by_catalog(&source.package, &catalog.packages))
                    .map(|catalog| catalog.root.as_path())
                    .ok_or_else(|| {
                        CliError::failure(format!(
                            "project inspection source `{}` {} has no exact record in its named owner",
                            source.package.package, source.package.version
                        ))
                    })?
            }
        };
        sources.push(::rust_inspect::OvenInspectionRegistrySource {
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
        .map(|dir| ::rust_inspect::SealedGeneratedOutDir {
            out_dir: authority.artifact_root().join(&dir.relative_root),
            version: dir.version.clone(),
        })
        .collect::<Vec<_>>();
    if optional
        && let Some(context) = native_sdk_context.as_deref()
        && !context.receipts_match_admission(&native_receipts)?
    {
        tracing::debug!(
            "completed project inspection lineage has another native generation; preparing current authority"
        );
        return Ok(ProjectRegistrySourceAuthoritySelection::StaleNativeGeneration);
    }
    let test_dependency_plan = prepare_project_test_dependency_plan(
        &mut authority,
        &mut release_loafs,
        test_dependency_stored_roles,
        test_dependency_release_identity,
        native_sdk_context.as_deref(),
    )?;
    Ok(ProjectRegistrySourceAuthoritySelection::Prepared(Arc::new(
        PreparedOvenProjectRegistrySourceAuthorities {
            native_sdk_context,
            authority,
            sources,
            registry_lock_source,
            generated_out_dirs,
            test_dependency_plan,
            _release_loafs: release_loafs,
        },
    )))
}

/// Return whether a stored project extension physically owns one sealed registry source tree.
fn extension_owns_registry_source(
    extension_paths: &[String],
    package: &oven_rustc::rustc::OvenRustcRegistrySourcePackage,
) -> bool {
    let prefix = format!("{}/", package.source.relative_root);
    extension_paths
        .iter()
        .any(|path| path == &package.source.relative_root || path.starts_with(&prefix))
}

/// Reconstruct and compose every role-bearing test constituent without resolving or compiling another crate.
fn prepare_project_test_dependency_plan(
    authority: &mut OvenLoadedProjectInspectionAuthority,
    release_loafs: &mut Vec<oven_rustc::loaf::OvenToolchainLoaf>,
    mut stored_roles: Vec<(usize, usize, Option<String>)>,
    release_identity: Option<String>,
    native_sdk_context: Option<&crate::build::NativeSdkCommandContext>,
) -> CliResult<Option<OvenDirectRustcPlanSelection>> {
    let envelope = authority.payload.test_dependency_envelope.as_ref();
    if envelope.is_none() {
        return Ok(None);
    }
    stored_roles.sort_by_key(|(_, stored_index, _)| std::cmp::Reverse(*stored_index));
    let mut main = None;
    let mut providers = Vec::new();
    for (constituent_index, stored_index, dependency_key) in stored_roles {
        let receipt = match authority.payload.constituents.get(constituent_index) {
            Some(OvenProjectInspectionConstituent::Stored { receipt, .. }) => receipt.clone(),
            _ => {
                return Err(CliError::failure(
                    "project inspection authority test dependency role no longer names a stored constituent",
                ));
            }
        };
        let selected = authority.stored_constituents.remove(stored_index);
        let admitted = if selected.manifest.domain == "sdk-native-consumer-plan" {
            native_sdk_context
                .map(|context| context.owners_for_receipt(&receipt))
                .transpose()?
        } else {
            None
        };
        let plan = oven_rustc::plan::selection::project_test_dependency_plan_from_constituent_with_native_owners(
            selected, &receipt, admitted,
        )
        .map_err(crate::error::oven_plan_error)?;
        if let Some(dependency_key) = dependency_key {
            providers.push((dependency_key, receipt, plan));
        } else {
            main = Some((receipt, plan));
        }
    }
    if let Some(identity) = release_identity {
        let index = release_loafs
            .iter()
            .position(|loaf| loaf.loaf_identity == identity)
            .ok_or_else(|| CliError::failure("project inspection authority lost its role-bearing release Loaf"))?;
        let loaf = release_loafs.remove(index);
        let receipt = authority
            .payload
            .test_dependency_envelope
            .as_ref()
            .and_then(|envelope| authority.payload.constituents.get(envelope.constituent_index))
            .and_then(|constituent| match constituent {
                OvenProjectInspectionConstituent::ReleaseLoaf { receipt, .. } => Some(receipt.clone()),
                OvenProjectInspectionConstituent::Stored { .. } => None,
            })
            .ok_or_else(|| CliError::failure("project inspection authority lost its release-Loaf receipt"))?;
        main = Some((receipt, OvenDirectRustcPlanSelection::ToolchainLoaf(Box::new(loaf))));
    }
    if providers.is_empty() {
        return main.map(|(_, plan)| Some(plan)).ok_or_else(|| {
            CliError::failure("project inspection authority test dependency role did not resolve to its constituent")
        });
    }
    let expected_intent = main
        .as_ref()
        .map(|(receipt, _)| receipt.intent.clone())
        .ok_or_else(|| CliError::failure("project inspection provider roles lost the project-owned test delta"))?;
    let mut extensions: Vec<(String, OvenPackagedLibraryLoafEntry, OvenProjectExtensionExecutionPlan)> = Vec::new();
    let mut direct: Vec<(String, OvenPackagedLibraryLoafEntry, OvenStoredDirectRustcExecutionPlan)> = Vec::new();
    let inputs = std::iter::once(("project-test-dependencies".to_string(), main)).chain(
        providers
            .into_iter()
            .map(|(key, receipt, plan)| (key, Some((receipt, plan)))),
    );
    for (dependency_key, input) in inputs {
        let Some((receipt, plan)) = input else {
            continue;
        };
        match plan {
            OvenDirectRustcPlanSelection::Stored(selected) => {
                let entry = OvenPackagedLibraryLoafEntry {
                    receipt,
                    identity: selected.identity.clone(),
                    kind: oven_store::store::OvenArtifactKind::DirectRustcPlan,
                    base_loaf_identity: None,
                };
                direct.push((dependency_key, entry, *selected));
            }
            OvenDirectRustcPlanSelection::ProjectExtension(selected) => {
                let entry = OvenPackagedLibraryLoafEntry {
                    receipt,
                    identity: selected.extension.identity.clone(),
                    kind: oven_store::store::OvenArtifactKind::ProjectPayload,
                    base_loaf_identity: Some(selected.base.loaf_identity.clone()),
                };
                extensions.push((dependency_key, entry, *selected));
            }
            OvenDirectRustcPlanSelection::ToolchainLoaf(_) => {}
            OvenDirectRustcPlanSelection::PackagedProvider(_) => {
                return Err(CliError::failure(
                    "project inspection authority nested an already composed provider plan",
                ));
            }
        }
    }
    if !extensions.is_empty() && !direct.is_empty() {
        return Err(CliError::failure(
            "project inspection authority cannot compose mixed extension and direct provider closures",
        ));
    }
    let composed = if !extensions.is_empty() {
        compose_packaged_provider_plan(extensions, &expected_intent).map_err(crate::error::oven_plan_error)?
    } else {
        compose_direct_packaged_provider_plan(direct, &expected_intent).map_err(crate::error::oven_plan_error)?
    };
    Ok(Some(OvenDirectRustcPlanSelection::PackagedProvider(Box::new(composed))))
}

impl PreparedOvenProjectRegistrySourceAuthorities {
    /// Return the exact dependency constituent whose native SDK catalog authorizes separately owned SDK inputs.
    fn native_sdk_dependency_receipt(&self) -> Option<&oven_store::OvenReceipt> {
        let envelope = self.authority.payload.test_dependency_envelope.as_ref()?;
        match self.authority.payload.constituents.get(envelope.constituent_index)? {
            OvenProjectInspectionConstituent::ReleaseLoaf { receipt, .. }
            | OvenProjectInspectionConstituent::Stored { receipt, .. } => Some(receipt),
        }
    }

    /// Return the exact role-bearing dependency envelope after validating this generated batch's complete surface.
    /// SDK registry inputs require the constituent's exact native catalog before their separately owned roots are
    /// excluded.
    pub fn test_dependency_plan(
        &self,
        dependencies: &[DependencySpec],
    ) -> CliResult<Option<&oven_rustc::plan::OvenDirectRustcPlanSelection>> {
        if self.authority.payload.test_dependency_envelope.is_none() {
            return Ok(None);
        }
        let promoted = crate::build_unit::promoted_oven_test_dependencies(&ResolvedDependencies {
            dependencies: dependencies.to_vec(),
            dev_dependencies: Vec::new(),
        })?;
        let promoted = match self.native_sdk_context.as_deref() {
            Some(context) => {
                crate::build::native_sdk_plan::project_dependencies_without_sdk_registry_inputs_with_context(
                    &promoted,
                    self.native_sdk_dependency_receipt(),
                    Some(context),
                )?
            }
            None => crate::build::native_sdk_plan::project_dependencies_without_sdk_registry_inputs(
                &promoted,
                self.native_sdk_dependency_receipt(),
            )?,
        };
        if !project_inspection_authority_supports_dependencies(&self.authority.payload, &promoted) {
            return Err(project_inspection_selection_mismatch("this test dependency subset"));
        }
        if let Some(mismatch) = project_inspection_test_dependency_envelope_mismatch(
            &self.authority.payload,
            &promoted,
            incan_oven_facet::provider_hooks().as_ref(),
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
                "Oven Alpha project inspection authority has a missing, stale, or incompatible test dependency root: {mismatch} (sealed aliases: [{expected}]; requested aliases: [{actual}]); rerun `incan oven bake --project .`"
            )));
        }
        self.test_dependency_plan.as_ref().map(Some).ok_or_else(|| {
            CliError::failure(
                "project inspection authority lost its exact test dependency plan while retaining its role",
            )
        })
    }

    /// Bind generated test receipts to the exact project authority selected once for this command.
    pub fn authority_identity(&self) -> &str {
        self.authority.identity()
    }

    /// Project the one complete exact authority for one generated test batch.
    pub(crate) fn install_for_dependencies(
        &self,
        manifest_dir: &Path,
        dependencies: &[DependencySpec],
    ) -> CliResult<bool> {
        let project_dependencies = match self.native_sdk_context.as_deref() {
            Some(context) => {
                crate::build::native_sdk_plan::project_dependencies_without_sdk_registry_inputs_with_context(
                    dependencies,
                    self.native_sdk_dependency_receipt(),
                    Some(context),
                )?
            }
            None => crate::build::native_sdk_plan::project_dependencies_without_sdk_registry_inputs(
                dependencies,
                self.native_sdk_dependency_receipt(),
            )?,
        };
        let dependencies = project_dependencies.as_slice();
        let registry_dependency_count = dependencies
            .iter()
            .filter(|dependency| matches!(dependency.source, oven_model::manifest::DependencySource::Registry))
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
        ::rust_inspect::write_sealed_oven_inspection_source_authority(manifest_dir, self.sources.clone())
            .map_err(|error| CliError::failure(format!("failed to install Oven Rust source authority: {error}")))?;
        ::rust_inspect::write_oven_generated_out_dirs(manifest_dir, &self.generated_out_dirs).map_err(|error| {
            CliError::failure(format!("failed to install Oven generated output directories: {error}"))
        })?;
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
pub fn inspection_packages_for_dependencies(
    dependencies: &[DependencySpec],
) -> CliResult<Vec<OvenLegacyCargoInspectionPackage>> {
    let mut packages = dependencies
        .iter()
        .filter(|dependency| matches!(dependency.source, oven_model::manifest::DependencySource::Registry))
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

/// Install sysroot-only inspection authority or refuse dependencies that lack a resolved Loaf.
///
/// Existing sealed source selections are installed before this fallback. It must never discover a registry
/// closure through Cargo: dependency adoption and resolution belong to Oven's Loaf resolver.
pub fn acquire_explicit_project_inspection_sources(
    manifest_dir: &Path,
    project_root: &Path,
    dependencies: &[DependencySpec],
) -> CliResult<()> {
    let manifest = oven_model::manifest::ProjectManifest::discover(project_root)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let declared = manifest.as_ref().and_then(|manifest| {
        manifest
            .rust_dependencies()
            .iter()
            .chain(manifest.rust_dev_dependencies())
            .min_by_key(|(name, _)| *name)
    });
    let dependency_name = declared
        .map(|(name, _)| name.as_str())
        .or_else(|| dependencies.first().map(|dependency| dependency.crate_name.as_str()));
    if let Some(dependency_name) = dependency_name {
        return Err(CliError::failure(format!(
            "Rust dependency `{}` needs a Loaf resolution; bake requires a sealed Oven inspection source authority",
            dependency_name
        )));
    }
    ::rust_inspect::write_sealed_oven_inspection_source_authority(manifest_dir, Vec::new())
        .map(|_| ())
        .map_err(|error| CliError::failure(format!("failed to install sysroot inspection authority: {error}")))
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
pub fn install_required_oven_registry_lock(
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
mod optional_lineage_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// Sysroot authority needs no registry discovery; unresolved Rust dependencies are named explicitly.
    #[test]
    fn explicit_inspection_installs_sysroot_or_refuses_unresolved_dependency() -> Result<(), Box<dyn std::error::Error>>
    {
        let workspace = tempfile::tempdir()?;
        acquire_explicit_project_inspection_sources(workspace.path(), workspace.path(), &[])?;
        let authority = ::rust_inspect::oven_inspection_registry_source_roots(workspace.path())?;
        assert!(authority.is_empty());
        let dependency = DependencySpec {
            crate_name: "semver".to_string(),
            version: Some("1".to_string()),
            features: Vec::new(),
            default_features: true,
            source: oven_model::manifest::DependencySource::Registry,
            optional: false,
            package: None,
        };
        let error = acquire_explicit_project_inspection_sources(workspace.path(), workspace.path(), &[dependency])
            .err()
            .ok_or("unresolved dependency was accepted")?;
        assert!(
            error
                .message
                .contains("Rust dependency `semver` needs a Loaf resolution")
        );
        Ok(())
    }

    /// Build one made-up sealed source record for ownership-routing tests.
    fn registry_source_package(
        package: &str,
        relative_root: &str,
    ) -> oven_rustc::rustc::OvenRustcRegistrySourcePackage {
        oven_rustc::rustc::OvenRustcRegistrySourcePackage {
            package: package.to_string(),
            version: "1.0.0".to_string(),
            features: Vec::new(),
            source: oven_rustc::rustc::OvenRustcRegistrySource {
                registry: "registry+https://packages.invalid/index".to_string(),
                checksum: format!("{package}-checksum"),
                relative_root: relative_root.to_string(),
                digest: format!("sha256:{package}"),
            },
        }
    }

    #[test]
    fn project_extension_source_ownership_follows_physically_retained_paths() {
        let project = registry_source_package("project-models", "registry-sources/project-models");
        let base = registry_source_package("base-support", "registry-sources/base-support");
        let extension_paths = vec![
            "registry-sources/project-models/Cargo.toml".to_string(),
            "registry-sources/project-models/src/lib.rs".to_string(),
        ];

        assert!(extension_owns_registry_source(&extension_paths, &project));
        assert!(
            !extension_owns_registry_source(&extension_paths, &base),
            "a complete composed plan must not assign a base-only source to the extension artifact root"
        );
    }

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
