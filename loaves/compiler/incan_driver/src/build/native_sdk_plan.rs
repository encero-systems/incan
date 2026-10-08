//! Receipt-bound consumer plans assembled only from an already published native SDK.

use std::collections::BTreeMap;
use std::path::Path;

use crate::build::{OvenDirectRustcPlanPreparation, OvenToolchainMaterialization};
use crate::error::{CliError, CliResult};
use oven_model::manifest::DependencySpec;
use oven_rustc::rustc::{
    OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactExtern, OvenRustcArtifactManifest,
    OvenRustcSourceSearchClosure, OvenRustcSupportingArtifact,
};
use oven_store::store::{OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore};

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
    let Some(expected) = receipt.and_then(|receipt| receipt.sources.build_unit_inputs.get("sdk-native-closure")) else {
        return Ok(dependencies.to_vec());
    };
    let inventory = incan_provider::inventory::discover_or_reuse_published_sdk_inventory()?.ok_or_else(|| {
        CliError::failure("receipt-selected native SDK is unavailable; prepare-sdk must restore its authority")
    })?;
    let bytes = std::fs::read(inventory.root.join(".sealed-native-receipts.json"))
        .map_err(|error| CliError::failure(error.to_string()))?;
    let catalog: BTreeMap<String, String> =
        serde_json::from_slice(&bytes).map_err(|error| CliError::failure(error.to_string()))?;
    let canonical = serde_json::to_vec(&catalog).map_err(|error| CliError::failure(error.to_string()))?;
    if oven_store::digest_bytes(&canonical) != *expected {
        return Err(CliError::failure(
            "project authority's native SDK receipt catalog is stale; rerun oven bake",
        ));
    }
    let selection = incan_provider::sdk_native::select_sdk_native_artifacts(&inventory.root)?;
    let mut project = Vec::new();
    for dependency in dependencies {
        if incan_provider::sdk_native::sdk_native_dependency_is_covered(&inventory, &selection, dependency)? {
            continue;
        }
        project.push(dependency.clone());
    }
    Ok(project)
}

/// Select a published SDK-only closure without letting a native SDK consumer enter compatibility publication.
///
/// Every native unit is reacquired under its original lease, and every provider output is validated against its
/// native descriptor. Uncovered project dependencies refuse here rather than invoking a resolver or Cargo reader.
pub(crate) fn select_native_sdk_plan(
    store: &OvenStore,
    receipt: &oven_store::OvenReceipt,
    dependencies: &[DependencySpec],
) -> CliResult<Option<OvenDirectRustcPlanPreparation>> {
    let Some(inventory) = incan_provider::inventory::discover_or_reuse_published_sdk_inventory()? else {
        return Ok(None);
    };
    if !inventory.root.join(".sealed-native-units.json").is_file() {
        return Ok(None);
    }
    let selection = incan_provider::sdk_native::select_sdk_native_artifacts(&inventory.root)?;
    for dependency in dependencies {
        if !incan_provider::sdk_native::sdk_native_dependency_is_covered(&inventory, &selection, dependency)? {
            return Err(CliError::failure(format!(
                "native SDK plan does not cover dependency `{}`; publish its own native authority first",
                dependency.crate_name
            )));
        }
    }
    if let Some(plan) =
        super::plan_selection::select_published_project_plan(store, receipt, OvenToolchainMaterialization::Reused)?
    {
        return Ok(Some(plan));
    }
    let (mut manifest, mut files) = native_unit_manifest(receipt, &selection, dependencies)?;
    add_native_providers(&inventory, &mut manifest, &mut files)?;
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
    super::plan_selection::select_published_project_plan(store, receipt, OvenToolchainMaterialization::Reused)
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

/// Validate a registry extern by exact native output bytes and SDK binding, without a Cargo source catalog.
///
/// A miss preserves the caller's existing registry authority checks. This grant applies only to the SDK's unique
/// covered version/feature/domain binding and its admitted output digest, never merely to a same-named crate.
pub(crate) fn native_registry_dependency_is_selected(dependency: &DependencySpec, artifact: &Path) -> CliResult<bool> {
    let Some(inventory) = incan_provider::inventory::discover_or_reuse_published_sdk_inventory()? else {
        return Ok(false);
    };
    if !inventory.root.join(".sealed-native-units.json").is_file() {
        return Ok(false);
    }
    let selection = incan_provider::sdk_native::select_sdk_native_artifacts(&inventory.root)?;
    if !incan_provider::sdk_native::sdk_native_dependency_is_covered(&inventory, &selection, dependency)? {
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
