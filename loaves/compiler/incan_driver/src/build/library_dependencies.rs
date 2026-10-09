//! Explicit ordinary package admission shared by source and installed library sessions (#1337/#1698).
//!
//! The caller supplies an exact selected package handoff digest; paths and adjacent manifests are coordinates,
//! never authority. Checked metadata and its original leases are admitted before frontend work. Profile validation
//! is separate and does not invent native plans or infer an authenticated empty execution closure.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use incan_frontend::library_manifest::published_layout::packaged_library_loaf_manifest_path;
use incan_frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry, dependency_crate_root,
    dependency_project_root, load_provider_dependency_artifact,
};
use incan_frontend::provider::namespaces::SelectedProviderNamespace;
use incan_lang::lang::stdlib;
use incan_provider::{PackageFeaturePlan, ProviderModuleResolution, ProviderPlan};
use oven_store::digest_bytes;
use oven_store::store::{OvenStore, OvenStoreLimits};

use super::library_generation::{SelectedLibraryGeneration, select_library_generation_reference};
use super::library_metadata::{
    SelectedLibraryMetadata, select_library_metadata_reference, validate_rust_fact_agreement,
};
use super::library_outputs::packaged_library_loaf_store_root;
use super::library_project::metadata_replay::observe_library_source_digest;
use super::package_loafs::{read_packaged_library_loaf_manifest, validated_packaged_library_loaf_profile};
use super::{
    CheckedPackagedProviderProfile, OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION, OvenPackagedLibraryLoafManifest,
};
use crate::error::{CliError, CliResult};

/// A caller-selected ordinary package, anchored in a resolver, publication or installed-set reference.
///
/// Transitive requests have no import alias. A reserved namespace capability comes from a separately validated
/// selection and cannot be produced by claiming a standard package name. The installed-set adapter that supplies
/// these references is still required; ambient path discovery must never fill a missing digest.
#[derive(Debug, Clone)]
pub struct LibraryDependencyInput {
    /// Physical materialization of the selected ordinary package.
    pub artifact_root: PathBuf,
    /// Exact package-loafs.json content digest anchored in the caller's admitted selection.
    pub handoff_digest: String,
    /// Exact checked public feature projection.
    pub features: BTreeSet<String>,
    /// Direct consumer alias; absent for transitives and namespace-only inputs.
    pub import_alias: Option<String>,
    /// Original reserved namespace grant, independently selected and bound to this same artifact.
    pub namespace: Option<SelectedProviderNamespace>,
}

/// One exact checked package and original metadata owner; execution profiles remain unselected.
struct AdmittedLibraryDependency {
    artifact: LibraryArtifactMetadata,
    package: OvenPackagedLibraryLoafManifest,
    handoff_digest: String,
    metadata: Arc<SelectedLibraryMetadata>,
    generation: Arc<SelectedLibraryGeneration>,
    source_available: bool,
}

/// Immutable command-owned checked package inputs with complete dependency leases and namespace authority.
///
/// This is an explicit preparation foundation. Normal command publication must call it with producer-selected
/// references before the SDK component adapter can be removed. No fallback parser or native preparation runs here.
pub struct PreparedLibraryDependencies {
    nodes: BTreeMap<String, AdmittedLibraryDependency>,
    aliases: BTreeMap<String, String>,
    provider_plan: Arc<ProviderPlan>,
    target: String,
    toolchain: String,
}

impl std::fmt::Debug for PreparedLibraryDependencies {
    /// Describe admitted identities without requiring execution-owner internals to implement formatting.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedLibraryDependencies")
            .field("owners", &self.nodes.keys().collect::<Vec<_>>())
            .field("aliases", &self.aliases)
            .field("target", &self.target)
            .field("toolchain", &self.toolchain)
            .finish()
    }
}

impl PreparedLibraryDependencies {
    /// Admit exact ordinary package handoffs and complete checked dependency owners, without selecting native plans.
    pub fn admit(
        inputs: &[LibraryDependencyInput],
        target: &str,
        toolchain: &str,
        limits: OvenStoreLimits,
    ) -> CliResult<Self> {
        if target.trim().is_empty() || toolchain.trim().is_empty() {
            return Err(invalid(
                "ordinary library admission requires explicit target and toolchain",
            ));
        }
        // ---- Exact checked owners and caller-selected package handoffs ----
        let mut nodes = BTreeMap::new();
        let mut aliases = BTreeMap::new();
        let mut entries = HashMap::new();
        let mut namespace_selections = Vec::new();
        for input in inputs {
            let artifact_root = fs::canonicalize(&input.artifact_root).map_err(|error| invalid(error.to_string()))?;
            let handoff = fs::read(packaged_library_loaf_manifest_path(&artifact_root))
                .map_err(|error| invalid(error.to_string()))?;
            if digest_bytes(&handoff) != input.handoff_digest {
                return Err(invalid(
                    "ordinary library package handoff differs from its selected reference",
                ));
            }
            let selected_package: OvenPackagedLibraryLoafManifest =
                serde_json::from_slice(&handoff).map_err(|error| invalid(error.to_string()))?;
            let key = input.import_alias.as_deref().unwrap_or("admitted-package");
            let (manifest, artifact) = match load_provider_dependency_artifact(key, &artifact_root) {
                LibraryManifestIndexEntry::Loaded { manifest, metadata } => (*manifest, metadata),
                LibraryManifestIndexEntry::Failed(failure) => return Err(invalid(failure.message)),
            };
            let package = read_packaged_library_loaf_manifest(&artifact)?
                .ok_or_else(|| invalid("ordinary library package handoff is unavailable"))?;
            if serde_json::to_vec(&selected_package).map_err(|error| invalid(error.to_string()))?
                != serde_json::to_vec(&package).map_err(|error| invalid(error.to_string()))?
            {
                return Err(invalid("ordinary library package changed during admission"));
            }
            if package.schema_version != OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION {
                return Err(invalid("ordinary library checked admission requires package schema 7"));
            }
            let reference = package
                .checked_metadata
                .as_ref()
                .ok_or_else(|| invalid("ordinary library package has no advertised checked metadata owner"))?;
            let store = OvenStore::with_release(
                packaged_library_loaf_store_root(&artifact_root),
                limits,
                &incan_oven_facet::compiler_identity(),
            );
            let metadata = select_library_metadata_reference(&store, reference)?;
            metadata.verify_materialization(&artifact_root)?;
            let generation = select_library_generation_reference(
                &store,
                package.checked_generation.as_ref().ok_or_else(|| {
                    invalid("ordinary library package has no original checked generation association")
                })?,
                Arc::clone(&metadata),
                &package.source_authority_digest,
                &package.metadata_files,
            )?;
            if metadata
                .manifest()
                .to_json_string()
                .map_err(|error| invalid(error.to_string()))?
                != manifest.to_json_string().map_err(|error| invalid(error.to_string()))?
                || metadata.checked_files() != package.metadata_files
                || metadata.recipe().name != manifest.name
                || metadata.recipe().version != manifest.version
                || metadata.recipe().target != target
                || metadata.recipe().toolchain != toolchain
                || metadata.recipe().features != input.features.iter().cloned().collect::<Vec<_>>()
                || manifest.contract_metadata.provider.active_features != input.features
            {
                return Err(invalid(
                    "ordinary library checked owner disagrees with selected package or intent",
                ));
            }
            metadata
                .checked_requirements()
                .ok_or_else(|| invalid("ordinary library checked owner lacks complete planning requirements"))?
                .validate()?;
            // ---- Consumer aliases and independently selected namespace grants ----
            let identity = metadata.reference().owner_identity;
            if let Some(alias) = &input.import_alias {
                if alias.trim().is_empty() || alias.contains(':') || alias.contains('.') || input.namespace.is_some() {
                    return Err(invalid(
                        "ordinary library import alias has invalid or competing namespace authority",
                    ));
                }
                if aliases.insert(alias.clone(), identity.clone()).is_some() {
                    return Err(invalid("ordinary library import alias is selected more than once"));
                }
                entries.insert(
                    alias.clone(),
                    LibraryManifestIndexEntry::Loaded {
                        manifest: Box::new(manifest.clone()),
                        metadata: artifact.clone(),
                    },
                );
            }
            if let Some(namespace) = &input.namespace {
                let record = namespace.record();
                let granted_manifest = record
                    .manifest
                    .as_deref()
                    .ok_or_else(|| invalid("namespace grant has no manifest"))?;
                let granted_artifact = record
                    .artifact
                    .as_ref()
                    .ok_or_else(|| invalid("namespace grant has no artifact"))?;
                if fs::canonicalize(&granted_artifact.crate_root).map_err(|error| invalid(error.to_string()))?
                    != artifact_root
                    || record.identity.name != manifest.name
                    || record.identity.version != manifest.version
                    || record.identity.feature_projection != input.features
                    || granted_manifest
                        .to_json_string()
                        .map_err(|error| invalid(error.to_string()))?
                        != manifest.to_json_string().map_err(|error| invalid(error.to_string()))?
                    || record.identity.digest
                        != incan_frontend::library_manifest::digest_provider_artifact(&artifact_root)
                            .map_err(|error| invalid(error.to_string()))?
                {
                    return Err(invalid(
                        "reserved namespace grant differs from its original ordinary package",
                    ));
                }
                namespace_selections.push(namespace.clone());
            }
            let source_available = dependency_project_root(&artifact_root)
                .is_some_and(|root| root.join(oven_model::manifest::LOAF_MANIFEST_FILENAME).is_file());
            if source_available {
                validate_authored_generation(&artifact, &metadata)?;
            }
            if let Some(previous) = nodes.get(&identity) {
                let previous: &AdmittedLibraryDependency = previous;
                if previous.artifact.crate_root != artifact_root || previous.handoff_digest != input.handoff_digest {
                    return Err(invalid(
                        "ordinary library owner has competing physical package coordinates",
                    ));
                }
            } else {
                nodes.insert(
                    identity,
                    AdmittedLibraryDependency {
                        artifact,
                        package,
                        handoff_digest: input.handoff_digest.clone(),
                        metadata,
                        generation,
                        source_available,
                    },
                );
            }
        }
        // ---- Complete retained metadata graph and immutable provider catalog ----
        validate_rust_fact_agreement(nodes.values().map(|node| node.metadata.manifest()))?;
        retain_dependency_owners(&mut nodes)?;
        let mut index = LibraryManifestIndex::from_entries(entries);
        for namespace in &namespace_selections {
            let record = namespace.record();
            let manifest = record
                .manifest
                .as_deref()
                .ok_or_else(|| invalid("namespace grant has no manifest"))?;
            let artifact = record
                .artifact
                .as_ref()
                .ok_or_else(|| invalid("namespace grant has no artifact"))?;
            index
                .add_admitted_standard_vocab_provider(manifest, &artifact.manifest_path, &artifact.crate_root)
                .map_err(|error| invalid(error.to_string()))?;
        }
        let provider_plan = Arc::new(
            ProviderPlan::from_admitted_libraries(index, &namespace_selections, std::iter::empty())
                .map_err(|error| invalid(error.to_string()))?,
        );

        // ---- Every current public route must retain its original checked owner ----
        for artifact in provider_plan.public_artifacts() {
            let selected = nodes
                .values()
                .find(|node| node.artifact.crate_root == artifact.artifact.crate_root)
                .ok_or_else(|| invalid("ordinary provider route lacks its original checked metadata owner"))?;
            if selected
                .metadata
                .manifest()
                .to_json_string()
                .map_err(|error| invalid(error.to_string()))?
                != artifact
                    .manifest
                    .to_json_string()
                    .map_err(|error| invalid(error.to_string()))?
            {
                return Err(invalid(
                    "ordinary provider route disagrees with its checked metadata owner",
                ));
            }
        }
        let result = Self {
            nodes,
            aliases,
            provider_plan,
            target: target.into(),
            toolchain: toolchain.into(),
        };
        result.verify()?;
        Ok(result)
    }

    /// Borrow the exact checked catalog, retaining all original metadata leases in this command capability.
    pub fn provider_plan(&self) -> &Arc<ProviderPlan> {
        &self.provider_plan
    }

    /// Borrow direct consumer aliases bound to canonical metadata owners, without assigning aliases to transitives.
    pub fn direct_aliases(&self) -> &BTreeMap<String, String> {
        &self.aliases
    }

    /// Borrow original checked owners and planning contracts retained by this preparation.
    ///
    /// Current plan construction consumes their checked requirements; it must not copy or replay an old native plan.
    pub fn checked_packages(&self) -> impl Iterator<Item = (&str, &Arc<SelectedLibraryMetadata>)> {
        self.nodes
            .iter()
            .map(|(identity, node)| (identity.as_str(), &node.metadata))
    }

    /// Match root aliases and feature selections against the current resolved manifest graph before session use.
    pub fn validate_feature_plan(&self, features: &PackageFeaturePlan) -> CliResult<()> {
        let root = features
            .root_package()
            .ok_or_else(|| invalid("ordinary session has no root feature state"))?;
        if root.active_dependencies != self.aliases.keys().cloned().collect::<BTreeSet<_>>()
            || features
                .packages()
                .any(|package| !package.features.required_sdk_components.is_empty())
        {
            return Err(invalid(
                "ordinary session dependency aliases or component requirements lack admitted authority",
            ));
        }
        for edge in features.edges() {
            let state = features
                .package(&edge.to)
                .ok_or_else(|| invalid("ordinary session dependency feature state is absent"))?;
            let expected = selected_artifact_root(state)?;
            let node = if edge.from == features.root() {
                let identity = self
                    .aliases
                    .get(&edge.dependency_key)
                    .ok_or_else(|| invalid("ordinary session active edge has no admitted package"))?;
                self.nodes
                    .get(identity)
                    .ok_or_else(|| invalid("ordinary session checked owner is absent"))?
            } else {
                self.nodes
                    .values()
                    .find(|node| node.artifact.crate_root == expected)
                    .ok_or_else(|| invalid("ordinary session transitive edge has no admitted checked package"))?
            };
            if expected != node.artifact.crate_root
                || state.package_name != node.metadata.manifest().name
                || state.features.active_features.iter().cloned().collect::<Vec<_>>() != node.metadata.recipe().features
            {
                return Err(invalid(
                    "ordinary session dependency differs from the current resolved package graph",
                ));
            }
        }
        self.verify()
    }

    /// Refuse implicit standard-source fallback outside the exact admitted namespace closure.
    ///
    /// Parent namespaces with admitted descendants are legal import containers. They grant no extra module owner;
    /// an unavailable exact module requires another selected ordinary package, never ambient source discovery.
    pub fn validate_module_usage(&self, modules: &BTreeSet<Vec<String>>) -> CliResult<()> {
        for module in modules
            .iter()
            .filter(|module| module.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT))
        {
            if !matches!(
                self.provider_plan.resolve_module(module),
                ProviderModuleResolution::Active(_)
            ) && !self.provider_plan.catalogs_modules_below(module)
            {
                return Err(invalid(format!(
                    "ordinary session module `{}` lacks an admitted namespace owner",
                    module.join(".")
                )));
            }
        }
        Ok(())
    }

    /// Revalidate original owners and mutable handoff coordinates; changed authored sources refuse through the shared
    /// validator.
    pub fn verify(&self) -> CliResult<()> {
        for node in self.nodes.values() {
            if node.source_available
                && !dependency_project_root(&node.artifact.crate_root)
                    .is_some_and(|root| root.join(oven_model::manifest::LOAF_MANIFEST_FILENAME).is_file())
            {
                return Err(invalid(
                    "ordinary library authored source authority disappeared after admission",
                ));
            }
            let bytes = fs::read(packaged_library_loaf_manifest_path(&node.artifact.crate_root))
                .map_err(|error| invalid(error.to_string()))?;
            if digest_bytes(&bytes) != node.handoff_digest {
                return Err(invalid("ordinary library package handoff changed after admission"));
            }
            let current = read_packaged_library_loaf_manifest(&node.artifact)?
                .ok_or_else(|| invalid("ordinary library package handoff disappeared after admission"))?;
            if serde_json::to_vec(&current).map_err(|error| invalid(error.to_string()))?
                != serde_json::to_vec(&node.package).map_err(|error| invalid(error.to_string()))?
                || current.metadata_files != node.metadata.checked_files()
            {
                return Err(invalid("ordinary library checked handoff changed after admission"));
            }
            node.metadata.verify_materialization(&node.artifact.crate_root)?;
            node.generation.verify()?;
            if node.source_available {
                validate_authored_generation(&node.artifact, &node.metadata)?;
            }
        }
        Ok(())
    }

    /// Validate actual requested profile outputs separately from metadata-only admission, keyed by canonical owner.
    ///
    /// Record dependency labels are diagnostic coordinates. Root alias authority remains in the provider plan;
    /// transitive and namespace-only packages are never assigned a fabricated consumer import alias.
    ///
    /// Returned records still require the existing native import/plan selector before execution. This method does not
    /// fabricate plans, acquire unrelated execution owners, or treat absent profile knowledge as an empty closure.
    pub fn execution_profiles(
        &self,
        profiles: &[&str],
    ) -> CliResult<BTreeMap<String, Vec<CheckedPackagedProviderProfile>>> {
        self.verify()?;
        let mut result = BTreeMap::new();
        for (identity, node) in &self.nodes {
            let mut selected = Vec::new();
            for profile in profiles {
                let package = validated_packaged_library_loaf_profile(
                    &node.artifact,
                    &node.package,
                    profile,
                    &self.target,
                    &self.toolchain,
                )?
                .ok_or_else(|| {
                    invalid(format!(
                        "ordinary library lacks compatible requested profile `{profile}`"
                    ))
                })?;
                selected.push(CheckedPackagedProviderProfile {
                    dependency_key: node.metadata.manifest().name.clone(),
                    artifact_root: node.artifact.crate_root.clone(),
                    source_available: node.source_available,
                    profile: (*profile).into(),
                    package,
                });
            }
            result.insert(identity.clone(), selected);
        }
        Ok(result)
    }
}

/// Resolve every recipe dependency by original immutable identity and retain exact child leases, rejecting cycles.
fn retain_dependency_owners(nodes: &mut BTreeMap<String, AdmittedLibraryDependency>) -> CliResult<()> {
    /// Resolve one exact original child owner before attaching its retained lease.
    fn retain(
        identity: &str,
        nodes: &BTreeMap<String, AdmittedLibraryDependency>,
        done: &mut BTreeMap<String, Arc<SelectedLibraryMetadata>>,
        visiting: &mut BTreeSet<String>,
    ) -> CliResult<Arc<SelectedLibraryMetadata>> {
        if let Some(owner) = done.get(identity) {
            return Ok(Arc::clone(owner));
        }
        if !visiting.insert(identity.into()) {
            return Err(invalid("ordinary checked dependency owner graph contains a cycle"));
        }
        let node = nodes
            .get(identity)
            .ok_or_else(|| invalid("ordinary checked dependency owner is missing"))?;
        let dependencies = node
            .metadata
            .recipe()
            .dependencies
            .values()
            .map(|dependency| retain(&dependency.owner_identity, nodes, done, visiting))
            .collect::<CliResult<Vec<_>>>()?;
        let owner = node.metadata.retaining_dependencies(&dependencies)?;
        visiting.remove(identity);
        done.insert(identity.into(), Arc::clone(&owner));
        Ok(owner)
    }
    let mut done = BTreeMap::new();
    for identity in nodes.keys() {
        retain(identity, nodes, &mut done, &mut BTreeSet::new())?;
    }
    for (identity, owner) in done {
        nodes
            .get_mut(&identity)
            .ok_or_else(|| invalid("ordinary checked dependency owner vanished"))?
            .metadata = owner;
    }
    Ok(())
}

/// Follow the resolver's typed source/compiled coordinate contract, including relocated installed artifacts.
fn selected_artifact_root(state: &incan_provider::ResolvedPackageFeatureState) -> CliResult<PathBuf> {
    let selected = if let Some(manifest) = &state.manifest {
        dependency_crate_root(manifest.project_root())
    } else {
        state
            .feature_manifest_path
            .parent()
            .ok_or_else(|| invalid("ordinary session compiled manifest has no artifact coordinate"))?
            .to_path_buf()
    };
    fs::canonicalize(selected).map_err(|error| invalid(error.to_string()))
}

/// Observe authored source in the exact same domain that produced this checked metadata owner.
fn validate_authored_generation(
    artifact: &LibraryArtifactMetadata,
    metadata: &SelectedLibraryMetadata,
) -> CliResult<()> {
    let root = dependency_project_root(&artifact.crate_root)
        .ok_or_else(|| invalid("ordinary library authored generation has no project coordinate"))?;
    if observe_library_source_digest(&root, &metadata.recipe().features)? != metadata.recipe().source_digest {
        return Err(invalid(
            "ordinary library checked owner differs from current authored metadata generation",
        ));
    }
    Ok(())
}

/// Keep refusal diagnostics in one ordinary package admission family.
fn invalid(message: impl Into<String>) -> CliError {
    CliError::failure(message.into())
}

#[cfg(test)]
mod tests;
