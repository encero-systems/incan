//! Receipt-bound consumer plans assembled only from an already published native SDK.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use crate::build::{OvenDirectRustcPlanPreparation, OvenToolchainMaterialization};
use crate::error::{CliError, CliResult};
use oven_model::manifest::DependencySpec;
use oven_rustc::rustc::{
    OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactExtern, OvenRustcArtifactManifest,
    OvenRustcSourceSearchClosure, OvenRustcSupportingArtifact,
};
use oven_store::store::{OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore};

/// One command's admitted native SDK publication and original execution leases (#1698).
///
/// Canonical selectors remain the only admission path. The complete owner set is retained; catalog dependency edges
/// are not authenticated by identity_binding and cannot select a smaller closure. Every handoff revalidates the
/// descriptors and receipt witnesses rather than treating this context as a cached authorization boolean.
pub struct NativeSdkCommandContext {
    inventory: Arc<incan_provider::SdkInventory>,
    selection: incan_provider::sdk_native::SdkNativeSelection,
    shared_owners: oven_rustc::plan::shared::OvenSharedNativeOwners,
}

impl NativeSdkCommandContext {
    /// Admit a published native SDK once; legacy inventories without native coordinates keep their existing route.
    pub fn discover() -> CliResult<Option<Arc<Self>>> {
        let Some(inventory) = incan_provider::inventory::discover_or_reuse_published_sdk_inventory()? else {
            return Ok(None);
        };
        if !inventory.root.join(".sealed-native-units.json").is_file() {
            return Ok(None);
        }
        Self::from_inventory(inventory).map(Some)
    }

    /// Bind one explicitly selected inventory to the canonical native descriptors and retained owner index.
    pub fn from_inventory(inventory: Arc<incan_provider::SdkInventory>) -> CliResult<Arc<Self>> {
        let started = std::time::Instant::now();
        let selection = incan_provider::sdk_native::select_sdk_native_artifacts(&inventory.root)?;
        let shared_owners = oven_rustc::plan::shared::OvenSharedNativeOwners::from_selected(&selection.owners)
            .map_err(crate::error::oven_plan_error)?;
        selection.verify()?;
        tracing::debug!(
            admitted_native_owners = selection.owners.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "native SDK command admission completed"
        );
        Ok(Arc::new(Self {
            inventory,
            selection,
            shared_owners,
        }))
    }

    /// Revalidate metadata, original payloads and witness bytes before projecting another physical consumer.
    pub fn verify(&self) -> CliResult<()> {
        self.selection.verify().map_err(Into::into)
    }

    /// Borrow the canonical receipt bytes captured by the same admission as this command's native owners.
    pub fn receipt_catalog(&self) -> CliResult<&[u8]> {
        self.verify()?;
        Ok(self.selection.receipt_catalog())
    }

    /// Report the actual number of owners acquired by this context's canonical admission.
    pub fn admitted_owner_count(&self) -> usize {
        self.selection.owners.len()
    }

    /// Project runtime identity through the existing source-bound engine from this exact retained selection.
    pub(crate) fn runtime_inputs(
        &self,
        providers: &[String],
        facets: &[String],
        dependencies: &[DependencySpec],
    ) -> CliResult<BTreeMap<String, String>> {
        super::native_runtime_inputs::runtime_inputs(
            &self.selection,
            self.selection.receipt_catalog(),
            providers,
            facets,
            dependencies,
        )
    }

    /// Install the existing frozen SDK inspection projection while sharing its command-admitted source owners.
    #[cfg(feature = "rust_inspect")]
    pub(crate) fn install_inspection_authority(
        &self,
        destination: &Path,
        dependencies: &[DependencySpec],
    ) -> CliResult<Option<Vec<Arc<oven_store::store::OvenStoreExecutionPayload>>>> {
        crate::sdk_closure::install_sdk_inspection_authority_from_selection(
            &self.inventory.root,
            destination,
            dependencies,
            &self.inventory,
            &self.selection,
        )
    }

    /// Require a receipt's portable catalog binding to name this exact command-admitted publication.
    fn verify_receipt(&self, receipt: &oven_store::OvenReceipt) -> CliResult<()> {
        if !self.receipts_match_admission(&[receipt])? {
            return Err(CliError::failure(
                "receipt-selected native SDK differs from command admission",
            ));
        }
        Ok(())
    }

    /// Compare genuine receipt generations after one revalidation of this command's original native owners.
    ///
    /// A differing catalog is useful only to optional lineage selection. Strict execution still calls
    /// `verify_receipt`; malformed receipts and changed current admission are errors in both paths. Every receipt
    /// is checked even after a mismatch, so a stale generation cannot hide a later malformed binding.
    pub(crate) fn receipts_match_admission(&self, receipts: &[&oven_store::OvenReceipt]) -> CliResult<bool> {
        self.verify()?;
        let current = oven_store::digest_bytes(self.selection.receipt_catalog());
        let mut matches = true;
        for receipt in receipts {
            receipt
                .verify_identity()
                .map_err(|error| CliError::failure(format!("invalid native consumer receipt: {error}")))?;
            if let Some(expected) = receipt.sources.build_unit_inputs.get("sdk-native-closure") {
                if !expected.strip_prefix("sha256:").is_some_and(|hex| {
                    hex.len() == 64
                        && hex
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                }) {
                    return Err(CliError::failure(
                        "native consumer receipt has an invalid catalog identity",
                    ));
                }
                if *expected != current {
                    matches = false;
                }
            }
        }
        Ok(matches)
    }

    /// Borrow the exact retained owner index after checking the caller's catalog binding and current witnesses.
    pub(crate) fn owners_for_receipt(
        &self,
        receipt: &oven_store::OvenReceipt,
    ) -> CliResult<&oven_rustc::plan::shared::OvenSharedNativeOwners> {
        self.verify_receipt(receipt)?;
        Ok(&self.shared_owners)
    }
}

/// Keep project dependency roots separate from registry capabilities supplied by the receipt-selected SDK.
///
/// The project source digest still binds its declarations. SDK requests must match the exact retained
/// package/version/features/domain selection and the receipt's canonical catalog digest. They require no project
/// Cargo lock or duplicate source catalog; local companions additionally reproduce their source snapshot identity.
/// Missing or incompatible requests remain project roots and refuse normally.
pub(crate) fn project_dependencies_without_sdk_registry_inputs(
    dependencies: &[DependencySpec],
    receipt: Option<&oven_store::OvenReceipt>,
) -> CliResult<Vec<DependencySpec>> {
    // An empty dependency list grants no SDK capability and needs no native owner selection.
    if dependencies.is_empty() {
        return Ok(Vec::new());
    }
    if receipt
        .and_then(|receipt| receipt.sources.build_unit_inputs.get("sdk-native-closure"))
        .is_none()
    {
        return Ok(dependencies.to_vec());
    }
    let context = NativeSdkCommandContext::discover()?;
    project_dependencies_without_sdk_registry_inputs_with_context(dependencies, receipt, context.as_deref())
}

/// Reuse one command's native admission while retaining the existing declaration coverage predicate.
pub(crate) fn project_dependencies_without_sdk_registry_inputs_with_context(
    dependencies: &[DependencySpec],
    receipt: Option<&oven_store::OvenReceipt>,
    context: Option<&NativeSdkCommandContext>,
) -> CliResult<Vec<DependencySpec>> {
    if dependencies.is_empty() {
        return Ok(Vec::new());
    }
    let Some(expected) = receipt.and_then(|receipt| receipt.sources.build_unit_inputs.get("sdk-native-closure")) else {
        return Ok(dependencies.to_vec());
    };
    let context = context.ok_or_else(|| {
        CliError::failure("receipt-selected native SDK is unavailable; prepare-sdk must restore its authority")
    })?;
    context.verify()?;
    if oven_store::digest_bytes(context.selection.receipt_catalog()) != *expected {
        return Err(CliError::failure(
            "project authority's native SDK receipt catalog is stale; rerun oven bake",
        ));
    }
    let mut project = Vec::new();
    for dependency in dependencies {
        if incan_provider::sdk_native::sdk_native_dependency_is_covered(
            &context.inventory,
            &context.selection,
            dependency,
        )? {
            continue;
        }
        project.push(dependency.clone());
    }
    Ok(project)
}

/// Select a published SDK-only closure without letting a native SDK consumer enter compatibility publication.
///
/// Every native unit is admitted under its original lease, and every provider output is validated against its
/// native descriptor. Uncovered project dependencies refuse here rather than invoking a resolver or Cargo reader.
pub(crate) fn select_native_sdk_plan(
    store: &OvenStore,
    receipt: &oven_store::OvenReceipt,
    dependencies: &[DependencySpec],
) -> CliResult<Option<OvenDirectRustcPlanPreparation>> {
    let context = NativeSdkCommandContext::discover()?;
    select_native_sdk_plan_with_context(store, receipt, dependencies, context.as_deref())
}

/// Assemble the canonical SDK-only plan from one admitted command context without reacquiring its native owners.
pub(crate) fn select_native_sdk_plan_with_context(
    store: &OvenStore,
    receipt: &oven_store::OvenReceipt,
    dependencies: &[DependencySpec],
    context: Option<&NativeSdkCommandContext>,
) -> CliResult<Option<OvenDirectRustcPlanPreparation>> {
    let Some(context) = context else {
        return Ok(None);
    };
    context.verify_receipt(receipt)?;
    let inventory = &context.inventory;
    let selection = &context.selection;
    for dependency in dependencies {
        if !incan_provider::sdk_native::sdk_native_dependency_is_covered(inventory, selection, dependency)? {
            return Err(CliError::failure(format!(
                "native SDK plan does not cover dependency `{}`; publish its own native authority first",
                dependency.crate_name
            )));
        }
    }
    if let Some(plan) = super::plan_selection::select_published_project_plan_with_native_owners(
        store,
        receipt,
        OvenToolchainMaterialization::Reused,
        Some(&context.shared_owners),
    )? {
        return Ok(Some(plan));
    }
    let (mut manifest, mut files) = native_unit_manifest(receipt, selection, dependencies)?;
    add_native_providers(inventory, &mut manifest, &mut files)?;
    seal_source_roles(receipt, &mut manifest);
    manifest
        .validate_shape(&receipt.intent)
        .map_err(|error| CliError::failure(error.to_string()))?;
    store
        .publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "sdk-native-consumer-plan".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&oven_rustc::plan::shared::OvenSharedNativePlan {
                artifacts: manifest,
                shared_native_roots: selection
                    .units
                    .iter()
                    .zip(&selection.owners)
                    .enumerate()
                    .map(|(index, (unit, owner))| {
                        Ok(oven_rustc::plan::shared::OvenSharedNativeRoot {
                            store: owner
                                .artifact_root
                                .parent()
                                .and_then(Path::parent)
                                .and_then(Path::parent)
                                .ok_or_else(|| CliError::failure("native unit has no store root"))?
                                .to_path_buf(),
                            identity: unit.store_identity.clone(),
                            receipt_identity: unit.receipt_identity.clone(),
                            prefix: format!("units/{index}"),
                        })
                    })
                    .collect::<CliResult<Vec<_>>>()?,
            })
            .map_err(|error| CliError::failure(error.to_string()))?,
            materialized_files: files,
            materialized_directories: Vec::new(),
        })
        .map_err(|error| CliError::failure(error.to_string()))?;
    super::plan_selection::select_published_project_plan_with_native_owners(
        store,
        receipt,
        OvenToolchainMaterialization::Reused,
        Some(&context.shared_owners),
    )
}

/// Name admitted native members in distinct logical unit directories without copying their bytes.
///
/// The receipt catalog in the build-unit identity binds package, feature, domain and archive selection. Inspection
/// retains its separately sealed source authority; the legacy registry source catalog requires Cargo declarations
/// and is deliberately not reconstructed from these build-system-neutral units.
fn native_unit_manifest(
    receipt: &oven_store::OvenReceipt,
    selection: &incan_provider::sdk_native::SdkNativeSelection,
    dependencies: &[DependencySpec],
) -> CliResult<(OvenRustcArtifactManifest, Vec<OvenArtifactMaterializedFile>)> {
    let mut manifest = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        entrypoint_externs: BTreeMap::new(),
        entrypoint_dependency_search_paths: BTreeMap::new(),
        registry_leaves: Vec::new(),
        registry_sources: Vec::new(),
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: Vec::new(),
    };
    let files = Vec::new();
    for (index, (unit, owner)) in selection.units.iter().zip(&selection.owners).enumerate() {
        if owner.manifest.intent.target != receipt.intent.target
            || owner.manifest.intent.toolchain != receipt.intent.toolchain
        {
            return Err(CliError::failure(format!(
                "native SDK unit `{}` has a different compiler or target",
                unit.binding.loaf
            )));
        }
        let prefix = format!("units/{index}");
        manifest.dependency_search_paths.push(prefix.clone());
        let name = Path::new(&unit.relative_path)
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("lib"))
            .ok_or_else(|| CliError::failure("native SDK output has no crate name"))?
            .to_string();
        let artifact = OvenRustcArtifactExtern {
            crate_name: name.clone(),
            relative_path: format!("{prefix}/{}", unit.relative_path),
            digest: unit.digest.clone(),
        };
        for alias in native_unit_root_aliases(unit, &name, dependencies)? {
            if manifest.externs.iter().any(|existing| existing.crate_name == alias) {
                return Err(CliError::failure(format!("native SDK root `{alias}` is ambiguous")));
            }
            let mut external = artifact.clone();
            external.crate_name = alias;
            manifest.externs.push(external);
        }
        for file in owner
            .admitted_materialized_files()
            .iter()
            .filter(|file| !file.relative_path.starts_with("source/"))
        {
            let relative_path = format!("{prefix}/{}", file.relative_path);
            if Path::new(&file.relative_path)
                .file_name()
                .is_some_and(|name| name == "Cargo.toml" || name == "Cargo.lock")
            {
                return Err(CliError::failure(
                    "native SDK member unexpectedly includes Cargo metadata",
                ));
            }
            if file.relative_path.ends_with(".a") {
                let parent = Path::new(&relative_path)
                    .parent()
                    .ok_or_else(|| CliError::failure("native archive has no parent"))?;
                manifest.native_search_paths.push(parent.to_string_lossy().into_owned());
            }
            manifest.supporting_artifacts.push(OvenRustcSupportingArtifact {
                relative_path: relative_path.clone(),
                digest: file.digest.clone(),
            });
        }
    }
    manifest.native_search_paths.sort();
    manifest.native_search_paths.dedup();
    Ok((manifest, files))
}

/// Select direct aliases from the already resolved unit, preserving version, features and compilation domain.
fn native_unit_root_aliases(
    unit: &oven_rustc::sdk_closure::SdkNativeArtifact,
    name: &str,
    dependencies: &[DependencySpec],
) -> CliResult<Vec<String>> {
    if unit.binding.domain != "target" && !unit.relative_path.ends_with(".dylib") {
        return Ok(Vec::new());
    }
    let mut aliases = std::collections::BTreeSet::new();
    // Public provider signatures can expose local facet types even when the caller imports only the Incan facade.
    if !unit.binding.loaf.starts_with("crates-io/") {
        aliases.insert(name.to_string());
    }
    let version =
        semver::Version::parse(&unit.binding.version).map_err(|error| CliError::failure(error.to_string()))?;
    for dependency in dependencies {
        let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
        let matches_package = if matches!(dependency.source, oven_model::manifest::DependencySource::Registry) {
            unit.binding.loaf == format!("crates-io/{package}")
        } else {
            package.replace('-', "_") == name
        };
        let requirement = dependency
            .version
            .as_deref()
            .map(semver::VersionReq::parse)
            .transpose()
            .map_err(|error| CliError::failure(error.to_string()))?;
        if matches_package
            && requirement
                .as_ref()
                .is_none_or(|requirement| requirement.matches(&version))
            && dependency
                .features
                .iter()
                .all(|feature| unit.binding.features.contains(feature))
        {
            aliases.insert(dependency.crate_name.replace('-', "_"));
        }
    }
    Ok(aliases.into_iter().collect())
}

/// Check one declared extern against the command's retained SDK output and existing version/feature predicate.
pub(crate) fn native_registry_dependency_is_selected_with_context(
    dependency: &DependencySpec,
    artifact: &Path,
    context: Option<&NativeSdkCommandContext>,
) -> CliResult<bool> {
    let Some(context) = context else {
        return Ok(false);
    };
    context.verify()?;
    let inventory = &context.inventory;
    let selection = &context.selection;
    if !incan_provider::sdk_native::sdk_native_dependency_is_covered(inventory, selection, dependency)? {
        return Ok(false);
    }
    let digest =
        oven_store::digest_bytes(&std::fs::read(artifact).map_err(|error| CliError::failure(error.to_string()))?);
    for unit in &selection.units {
        let name = Path::new(&unit.relative_path)
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("lib"))
            .ok_or_else(|| CliError::failure("SDK registry output has no crate name"))?;
        if unit.digest == digest
            && native_unit_root_aliases(unit, name, std::slice::from_ref(dependency))?
                .contains(&dependency.crate_name.replace('-', "_"))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Include checked SDK facades so generated wrappers never rebuild provider paths through Cargo metadata.
fn add_native_providers(
    inventory: &incan_provider::SdkInventory,
    manifest: &mut OvenRustcArtifactManifest,
    files: &mut Vec<OvenArtifactMaterializedFile>,
) -> CliResult<()> {
    for provider in inventory.components.values().flat_map(|component| &component.providers) {
        let (Some(root), Some(path)) = (&provider.crate_root, &provider.manifest_path) else {
            continue;
        };
        let library = incan_frontend::library_manifest::LibraryManifest::read_from_path(path)
            .map_err(|error| CliError::failure(error.to_string()))?;
        let native = incan_frontend::library_manifest::read_native_provider_artifact(root, &library)
            .map_err(|error| CliError::failure(error.to_string()))?
            .ok_or_else(|| {
                CliError::failure(format!(
                    "SDK provider `{}` lacks native output authority",
                    provider.name
                ))
            })?;
        let relative_path = format!("providers/{}/{}", native.output.crate_name, native.output.relative_path);
        let parent = Path::new(&relative_path)
            .parent()
            .ok_or_else(|| CliError::failure("SDK facade has no parent"))?;
        manifest
            .dependency_search_paths
            .push(parent.to_string_lossy().into_owned());
        manifest.externs.push(OvenRustcArtifactExtern {
            crate_name: native.output.crate_name,
            relative_path: relative_path.clone(),
            digest: native.output.digest,
        });
        files.push(OvenArtifactMaterializedFile {
            source_path: root.join(native.output.relative_path),
            relative_path,
        });
    }
    Ok(())
}

/// Bind every declared source role to exact members of the same already admitted dependency closure.
fn seal_source_roles(receipt: &oven_store::OvenReceipt, manifest: &mut OvenRustcArtifactManifest) {
    manifest.supporting_artifacts.retain(|file| {
        !manifest
            .externs
            .iter()
            .any(|external| external.relative_path == file.relative_path)
    });
    let mut declared = manifest
        .supporting_artifacts
        .iter()
        .map(|file| (file.relative_path.clone(), file.digest.clone()))
        .collect::<BTreeMap<_, _>>();
    for file in &manifest.externs {
        declared.insert(file.relative_path.clone(), file.digest.clone());
    }
    let closure = OvenRustcSourceSearchClosure::publisher_selected(manifest.dependency_search_paths.clone(), &declared);
    let names = manifest
        .externs
        .iter()
        .map(|file| file.crate_name.clone())
        .collect::<Vec<_>>();
    for role in receipt.sources.supplemental_digests.keys() {
        manifest.entrypoint_externs.insert(role.clone(), names.clone());
        manifest
            .entrypoint_dependency_search_paths
            .insert(role.clone(), closure.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::{native_unit_manifest, native_unit_root_aliases, seal_source_roles};
    use std::path::Path;

    /// The covered debug-target fast path shares the command's original SDK owner instead of reacquiring it.
    #[test]
    fn dev7_native_admission_test_envelope_reuses_held_owner() -> Result<(), Box<dyn std::error::Error>> {
        use oven_store::store::{
            OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStoreLimits,
        };
        use std::collections::BTreeMap;
        use std::sync::Arc;

        let root = tempfile::tempdir()?;
        let source = root.path().join("main.rs");
        std::fs::write(&source, "fn main() {}\n")?;
        let unit_receipt = oven_store::receipt_generated_project(
            &oven_store::OvenGeneratedProjectRequest::new(
                root.path(),
                "envelope_owner",
                "1.0.0",
                "fixture-target",
                "fixture-toolchain",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &source),
        )?;
        let binding = oven_rustc::sdk_closure::SdkLockedUnit {
            loaf: "crates-io/fixture".to_string(),
            version: "1.0.0".to_string(),
            archive_digest: oven_store::digest_bytes(b"fixture source"),
            domain: "target".to_string(),
            features: Vec::new(),
            target_predicates: Vec::new(),
            edges: None,
        };
        let output = root.path().join("libfixture.rlib");
        std::fs::write(&output, b"opaque held owner control")?;
        let native_store = super::OvenStore::new(
            root.path().join("native-store"),
            OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000),
        );
        let owner = native_store.publish(&OvenArtifactPublishRequest {
            receipt: unit_receipt.clone(),
            domain: "sdk-source-unit-target".to_string(),
            kind: OvenArtifactKind::Engine,
            payload: serde_json::to_vec(&binding.identity_binding())?,
            materialized_files: vec![OvenArtifactMaterializedFile {
                source_path: output,
                relative_path: "libfixture.rlib".to_string(),
            }],
            materialized_directories: Vec::new(),
        })?;
        let unit = oven_rustc::sdk_closure::SdkNativeArtifact {
            binding: binding.clone(),
            store_identity: owner.identity,
            receipt_identity: unit_receipt.identity.clone(),
            relative_path: "libfixture.rlib".to_string(),
            digest: oven_store::digest_bytes(b"opaque held owner control"),
        };
        std::fs::write(
            root.path().join(".sealed-native-units.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1, "store": native_store.root(), "units": [unit],
            }))?,
        )?;
        std::fs::write(
            root.path().join(".sealed-native-receipts.json"),
            serde_json::to_vec(&BTreeMap::from([(
                serde_json::to_string(&binding.identity_binding())?,
                unit_receipt.identity.clone(),
            )]))?,
        )?;
        let context = super::NativeSdkCommandContext::from_inventory(Arc::new(incan_provider::SdkInventory {
            root: root.path().to_path_buf(),
            sdk_id: "fixture".to_string(),
            sdk_version: "1.0.0".to_string(),
            compiler_requirement: "*".to_string(),
            provider_codegen_revision: incan_lang::version::SDK_PROVIDER_CODEGEN_REVISION,
            components: BTreeMap::new(),
            profiles: BTreeMap::new(),
        }))?;
        let digest = oven_store::digest_dependency_specs(&[], incan_oven_facet::provider_hooks().as_ref())?;
        let receipt = oven_store::receipt_with_build_unit_input(&unit_receipt, "rust-dependencies", digest)?;
        let receipt = oven_store::receipt_with_build_unit_input(
            &receipt,
            "sdk-native-closure",
            oven_store::digest_bytes(context.receipt_catalog()?),
        )?;
        let (mut manifest, files) = native_unit_manifest(&receipt, &context.selection, &[])?;
        seal_source_roles(&receipt, &mut manifest);
        let consumer_store = super::OvenStore::new(
            root.path().join("consumer-store"),
            OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000),
        );
        // Match the production plan's held-owner coordinate, including canonicalized temporary-directory ancestors.
        let admitted_store = context.selection.owners[0]
            .artifact_root
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .ok_or("fixture owner has no admitted store root")?
            .to_path_buf();
        assert_eq!(admitted_store, native_store.root().canonicalize()?);
        consumer_store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "sdk-native-consumer-plan".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&oven_rustc::plan::shared::OvenSharedNativePlan {
                artifacts: manifest,
                shared_native_roots: vec![oven_rustc::plan::shared::OvenSharedNativeRoot {
                    store: admitted_store,
                    identity: context.selection.units[0].store_identity.clone(),
                    receipt_identity: unit_receipt.identity,
                    prefix: "units/0".to_string(),
                }],
            })?,
            materialized_files: files,
            materialized_directories: Vec::new(),
        })?;
        let mut bake = crate::build::OvenProjectBakeAuthorityContext {
            native_sdk_context: Some(Arc::clone(&context)),
            ..Default::default()
        };
        let baseline_owners = Arc::strong_count(&context.selection.owners[0]);
        let envelope = crate::build::plan_selection::prepare_oven_test_dependency_envelope(
            &consumer_store,
            root.path(),
            &incan_provider::dependency_resolver::ResolvedDependencies {
                dependencies: Vec::new(),
                dev_dependencies: Vec::new(),
            },
            std::slice::from_ref(&receipt),
            Some(&mut bake),
        )?;
        assert_eq!(envelope.receipt.identity, receipt.identity);
        assert_eq!(context.admitted_owner_count(), 1);
        assert_eq!(
            Arc::strong_count(&context.selection.owners[0]),
            baseline_owners + 1,
            "covered target must retain the original Arc, rather than a newly acquired payload"
        );
        drop(envelope);
        assert_eq!(Arc::strong_count(&context.selection.owners[0]), baseline_owners);
        Ok(())
    }

    /// A real admitted local library becomes a source-scoped plan, with byte validation and compiler mismatch refusal.
    #[test]
    fn native_sdk_plan_retains_exact_units_without_cargo_projection() -> Result<(), Box<dyn std::error::Error>> {
        use oven_rustc::sdk_closure::{compile_local_sdk_facet, prepare_sdk_seed};
        use oven_store::{OvenGeneratedProjectRequest, receipt_generated_project};

        let root = tempfile::tempdir()?;
        let seed = root.path().join("seed.json");
        std::fs::write(&seed, r#"{"schema":"incan.oven.loaf-resolution/1","units":[]}"#)?;
        let output = root.path().join("native");
        let rustc = oven_rustc::rustc::resolve_active_rustc()?;
        let mut closure = prepare_sdk_seed(&seed, root.path(), &output, &rustc, root.path())?;
        let project = root.path().join("local");
        std::fs::create_dir_all(project.join("src"))?;
        std::fs::write(
            project.join("loaf.toml"),
            "[project]\nname='local'\nversion='1.0.0'\n[rust]\nname='local'\ntype='lib'\nedition='2024'\n",
        )?;
        std::fs::write(project.join("src/lib.rs"), "pub fn value() -> u8 { 1 }")?;
        compile_local_sdk_facet(&mut closure, &project, &[], "target", &output, &rustc)?;
        let authority = root.path().join("authority");
        std::fs::create_dir(&authority)?;
        incan_provider::sdk_native::write_sdk_native_authority(&closure, &authority)?;
        incan_provider::sdk_native::write_sdk_native_artifact_catalog(&closure, &output.join("store"), &authority)?;
        let selection = incan_provider::sdk_native::select_sdk_native_artifacts(&authority)?;
        let generated = root.path().join("main.rs");
        std::fs::write(&generated, "fn main() {}")?;
        let intent = &selection
            .owners
            .first()
            .ok_or("native fixture has no admitted owner")?
            .manifest
            .intent;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                root.path(),
                "consumer",
                "0.1.0",
                &intent.target,
                &intent.toolchain,
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated),
        )?;
        let dependency = oven_model::manifest::DependencySpec {
            crate_name: "local".to_string(),
            version: Some("1".to_string()),
            features: Vec::new(),
            default_features: true,
            source: oven_model::manifest::DependencySource::Path { path: project },
            optional: false,
            package: None,
        };
        let (mut manifest, files) = native_unit_manifest(&receipt, &selection, std::slice::from_ref(&dependency))?;
        seal_source_roles(&receipt, &mut manifest);
        manifest.validate_shape(&receipt.intent)?;
        assert!(files.is_empty());
        assert!(manifest.registry_sources.is_empty());
        assert_eq!(manifest.externs.len(), 1);
        assert!(manifest.supporting_artifacts.is_empty());
        check_shared_consumer_plan(root.path(), &output, &receipt, &manifest, &selection)?;
        assert_eq!(
            manifest.entrypoint_dependency_search_paths["generated-root"]
                .publisher_paths
                .len(),
            1
        );
        let mut wrong_compiler = receipt.clone();
        wrong_compiler.intent.toolchain.push_str(" changed");
        assert!(native_unit_manifest(&wrong_compiler, &selection, &[]).is_err());
        let mut registry_unit = selection.units.first().ok_or("native fixture has no unit")?.clone();
        registry_unit.binding.loaf = "crates-io/local".to_string();
        let mut renamed = dependency;
        renamed.source = oven_model::manifest::DependencySource::Registry;
        renamed.package = Some("local".to_string());
        renamed.crate_name = "renamed_local".to_string();
        assert_eq!(
            native_unit_root_aliases(&registry_unit, "local", &[renamed.clone()])?,
            vec!["renamed_local"]
        );
        renamed.version = Some("2".to_string());
        assert!(native_unit_root_aliases(&registry_unit, "local", &[renamed.clone()])?.is_empty());
        renamed.version = Some("1".to_string());
        renamed.features.push("absent".to_string());
        assert!(native_unit_root_aliases(&registry_unit, "local", &[renamed])?.is_empty());
        Ok(())
    }
    /// A small consumer publication resolves the original native bytes while retaining their receipt leases.
    fn check_shared_consumer_plan(
        root: &Path,
        output: &Path,
        receipt: &oven_store::OvenReceipt,
        manifest: &oven_rustc::rustc::OvenRustcArtifactManifest,
        selection: &incan_provider::sdk_native::SdkNativeSelection,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let consumer_store = oven_store::store::OvenStore::new(
            root.join("consumer-store"),
            oven_store::store::OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000),
        );
        let unit = selection.units.first().ok_or("native fixture has no unit")?;
        let shared = oven_rustc::plan::shared::OvenSharedNativePlan {
            artifacts: manifest.clone(),
            shared_native_roots: vec![oven_rustc::plan::shared::OvenSharedNativeRoot {
                store: output.join("store"),
                identity: unit.store_identity.clone(),
                receipt_identity: unit.receipt_identity.clone(),
                prefix: "units/0".to_string(),
            }],
        };
        let published = consumer_store.publish(&oven_store::store::OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "sdk-native-consumer-plan".to_string(),
            kind: oven_store::store::OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&shared)?,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })?;
        assert!(published.materialized_files.is_empty());
        let selected =
            oven_rustc::plan::selection::select_receipt_direct_rustc_execution_plan(&consumer_store, receipt)?
                .ok_or("shared consumer plan was not selected")?;
        assert_eq!(
            selected.artifact_plan.externs[0].1,
            selection.owners[0].artifact_root.join("liblocal.rlib")
        );
        assert_eq!(
            selected.artifact_plan.dependency_search_paths,
            vec![selection.owners[0].artifact_root.clone()]
        );
        Ok(())
    }
}
