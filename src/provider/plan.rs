//! Immutable provider catalog and active compilation projection from RFC 114.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::frontend::library_manifest_index::{
    LibraryArtifactKind, LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
    load_provider_dependency_artifact,
};
use crate::library_manifest::{
    LibraryManifest, LibraryManifestError, ProviderCargoDependency, ProviderImplementationFacet,
    digest_provider_artifact,
};

use super::{PackageFeaturePlan, ResolvedSdkComponents, SdkInventory};

/// Stable identity of one immutable compiled-provider projection.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ProviderIdentity {
    /// Provider package or SDK artifact name.
    pub name: String,
    /// Exact provider version.
    pub version: String,
    /// Content digest recorded by artifact publication.
    pub digest: String,
    /// Public feature projection used when the physical artifact is specialized.
    pub feature_projection: BTreeSet<String>,
}

impl ProviderIdentity {
    /// Render a deterministic key suitable for maps, reports, and lock records.
    pub fn stable_key(&self) -> String {
        let features = self.feature_projection.iter().cloned().collect::<Vec<_>>().join(",");
        format!("{}@{}#{}[{}]", self.name, self.version, self.digest, features)
    }
}

/// Source and authority chain that introduced one provider record.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderProvenance {
    /// Ordinary Incan dependency selected from a project graph.
    ProjectDependency {
        /// Dependency key used under `pub::<key>`.
        dependency_key: String,
        /// Project manifest that declared the dependency.
        manifest_path: PathBuf,
    },
    /// Official or explicitly overridden provider advertised by the active SDK inventory.
    Sdk {
        /// Active SDK identity.
        sdk_identity: String,
        /// Component that supplies this provider.
        component_id: String,
        /// Inventory file that granted reserved namespace authority, when installed.
        inventory_path: Option<PathBuf>,
    },
    /// Compiler-owned symbolic surface without a compiled library artifact.
    Compiler,
}

/// Namespace grant under which exact provider claims are validated.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NamespaceAuthority {
    /// Ordinary dependency may own only `pub::<dependency_key>` and descendants.
    ProjectDependency {
        /// Dependency key granted by the consumer manifest.
        dependency_key: String,
    },
    /// SDK inventory may grant exact `std.*` claims.
    SdkReserved,
    /// Compiler-only roots and symbolic modules.
    Compiler,
}

/// One provider-owned backend requirement selected only after semantic resolution.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackendImplementationRequirement {
    /// Cargo feature used by the current Rust backend adapter.
    CargoFeature {
        /// Generated or linked crate name.
        crate_name: String,
        /// Private Cargo feature name.
        feature: String,
    },
    /// Linked crate required by the current Rust backend adapter.
    CargoDependency {
        /// Relocatable provider-owned dependency specification.
        dependency: ProviderCargoDependency,
    },
}

/// Named private implementation selection derived from semantic provider use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImplementationFacet {
    /// Provider-local stable facet id.
    pub id: String,
    /// Modules whose use selects this facet.
    pub required_modules: BTreeSet<Vec<String>>,
    /// Public provider features whose activation selects this facet.
    pub required_features: BTreeSet<String>,
    /// Current-backend requirements hidden behind this semantic facet.
    pub backend_requirements: Vec<BackendImplementationRequirement>,
}

/// One catalog provider before active module projection.
#[derive(Debug, Clone)]
pub struct ProviderRecord {
    /// Immutable provider identity.
    pub identity: ProviderIdentity,
    /// Provenance suitable for diagnostics and inspection.
    pub provenance: ProviderProvenance,
    /// Namespace grant used to validate exact claims.
    pub authority: NamespaceAuthority,
    /// Exact canonical import modules known to this provider.
    pub namespace_claims: BTreeSet<Vec<String>>,
    /// Whether artifact bytes are present and integrity-checked locally.
    pub available: bool,
    /// Whether the project/component/feature graph enables this provider.
    pub enabled: bool,
    /// Checked semantic manifest when the artifact is locally available.
    pub manifest: Option<Arc<LibraryManifest>>,
    /// Validated generated Rust artifact location when locally available.
    pub artifact: Option<LibraryArtifactMetadata>,
    /// Private backend requirements derived from semantic use.
    pub implementation_facets: Vec<ImplementationFacet>,
}

/// Provider participation state for tooling and reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderParticipation {
    /// Known to the catalog but not present in this SDK installation or artifact store.
    Unavailable,
    /// Available but not enabled by the project selection.
    Disabled,
    /// Enabled and available, but no provider module is reachable in this compilation.
    Enabled,
    /// Enabled, available, and reached by at least one canonical provider module.
    Used,
}

/// Exact module lookup result preserving distinct remedies.
#[derive(Debug, Clone, Copy)]
pub enum ProviderModuleResolution<'a> {
    /// Enabled and locally available provider.
    Active(&'a ProviderRecord),
    /// Known provider whose component or dependency is disabled.
    Disabled(&'a ProviderRecord),
    /// Enabled provider whose artifact is absent locally.
    Unavailable(&'a ProviderRecord),
    /// No provider claims this exact canonical module.
    Unknown,
}

/// Invalid provider identity, namespace authority, catalog collision, or availability state.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderPlanError {
    /// Two records share one immutable provider key.
    #[error("duplicate provider identity `{identity}`")]
    DuplicateIdentity {
        /// Repeated stable identity key.
        identity: String,
    },
    /// An ordinary dependency or SDK provider claimed a root it does not own.
    #[error("provider `{provider}` is not authorized to claim module `{module}`")]
    UnauthorizedNamespace {
        /// Provider identity name.
        provider: String,
        /// Canonical module path.
        module: String,
    },
    /// Two catalog records claim one exact canonical module.
    #[error(
        "module `{module}` is claimed by both provider `{existing}` ({existing_provenance}) and provider `{incoming}` ({incoming_provenance})"
    )]
    NamespaceCollision {
        /// Canonical module path.
        module: String,
        /// First provider identity.
        existing: String,
        /// Provenance that introduced the first claim.
        existing_provenance: String,
        /// Second provider identity.
        incoming: String,
        /// Provenance that introduced the second claim.
        incoming_provenance: String,
    },
    /// A record marked available has no checked semantic manifest.
    #[error("provider `{provider}` is marked available but has no checked manifest")]
    AvailableManifestMissing {
        /// Provider identity name.
        provider: String,
    },
    /// Compilation requires an enabled provider whose artifact is unavailable.
    #[error("provider `{provider}` is enabled but unavailable")]
    EnabledProviderUnavailable {
        /// Provider identity name.
        provider: String,
    },
    /// Reading or validating an SDK provider manifest failed.
    #[error("failed to load provider `{provider}` manifest at {path}: {message}")]
    ManifestLoad {
        /// Provider identity name.
        provider: String,
        /// Manifest path advertised by the provider catalog.
        path: PathBuf,
        /// Underlying read, parse, or validation detail.
        message: String,
    },
    /// Installed provider metadata disagrees with its SDK inventory descriptor.
    #[error("provider `{provider}` artifact metadata does not match its SDK inventory: {message}")]
    InventoryMismatch {
        /// Provider identity name.
        provider: String,
        /// Mismatched identity, namespace, or location detail.
        message: String,
    },
    /// A package feature requires a component that is not enabled by the project SDK selection.
    #[error("package `{package}` feature projection requires disabled SDK component `{component}`")]
    RequiredComponentDisabled {
        /// Package that owns the active feature requirement.
        package: String,
        /// SDK component that must be selected explicitly.
        component: String,
    },
}

/// Immutable provider catalog and active module projection shared by every compiler stage.
#[derive(Debug, Clone, Default)]
pub struct ProviderPlan {
    library_manifest_index: LibraryManifestIndex,
    records: BTreeMap<String, ProviderRecord>,
    module_catalog: BTreeMap<Vec<String>, String>,
    used_module_paths: BTreeSet<Vec<String>>,
    /// Reserved namespace roots owned by the one SDK component currently being compiled from source.
    ///
    /// This bootstrap-only grant disappears once the checked provider manifest is published and must never be
    /// populated by installed SDK consumers.
    bootstrap_sdk_namespace_roots: BTreeSet<String>,
}

impl ProviderPlan {
    /// Validate provider records and build one deterministic immutable plan.
    pub fn new<I>(
        library_manifest_index: LibraryManifestIndex,
        records: Vec<ProviderRecord>,
        used_module_paths: I,
    ) -> Result<Self, ProviderPlanError>
    where
        I: IntoIterator<Item = Vec<String>>,
    {
        let mut indexed_records: BTreeMap<String, ProviderRecord> = BTreeMap::new();
        let mut module_catalog: BTreeMap<Vec<String>, String> = BTreeMap::new();
        for record in records {
            validate_provider_record(&record)?;
            let key = record.identity.stable_key();
            if indexed_records.contains_key(&key) {
                return Err(ProviderPlanError::DuplicateIdentity { identity: key });
            }
            for claim in &record.namespace_claims {
                if let Some(existing_key) = module_catalog.get(claim) {
                    let existing_record = indexed_records.get(existing_key);
                    let existing = existing_record
                        .map(|provider: &ProviderRecord| provider.identity.name.clone())
                        .unwrap_or_else(|| existing_key.clone());
                    let existing_provenance = existing_record
                        .map(|provider| render_provider_provenance(&provider.provenance))
                        .unwrap_or_else(|| "unknown provenance".to_string());
                    return Err(ProviderPlanError::NamespaceCollision {
                        module: render_module(claim),
                        existing,
                        existing_provenance,
                        incoming: record.identity.name.clone(),
                        incoming_provenance: render_provider_provenance(&record.provenance),
                    });
                }
                module_catalog.insert(claim.clone(), key.clone());
            }
            indexed_records.insert(key, record);
        }

        Ok(Self {
            library_manifest_index,
            records: indexed_records,
            module_catalog,
            used_module_paths: used_module_paths.into_iter().collect(),
            bootstrap_sdk_namespace_roots: BTreeSet::new(),
        })
    }

    /// Return the consumer-side dependency manifest index normalized into this plan.
    pub fn library_manifest_index(&self) -> &LibraryManifestIndex {
        &self.library_manifest_index
    }

    /// Build one provider plan from ordinary dependency artifacts and the active SDK catalog.
    pub fn from_resolved_inputs<I>(
        library_manifest_index: LibraryManifestIndex,
        package_features: Option<&PackageFeaturePlan>,
        sdk_inventory: Option<&SdkInventory>,
        sdk_components: Option<&ResolvedSdkComponents>,
        used_module_paths: I,
    ) -> Result<Self, ProviderPlanError>
    where
        I: IntoIterator<Item = Vec<String>>,
    {
        if let (Some(features), Some(components)) = (package_features, sdk_components) {
            validate_package_component_requirements(features, components)?;
        }

        let mut records = project_dependency_records(&library_manifest_index, package_features)?;
        if let Some(inventory) = sdk_inventory {
            records.extend(sdk_provider_records(inventory, sdk_components)?);
        }
        Self::new(library_manifest_index, records, used_module_paths)
    }

    /// Create a plan that carries an ordinary dependency index and no SDK providers.
    ///
    /// Parser, typechecker, and lowering unit tests use this constructor when their scope is specifically `pub::`
    /// behavior. Production compilation should use [`Self::from_resolved_inputs`].
    pub fn for_library_index(library_manifest_index: LibraryManifestIndex) -> Self {
        Self {
            library_manifest_index,
            ..Self::default()
        }
    }

    /// Create an in-memory SDK provider plan for focused compiler tests and embedding adapters.
    ///
    /// Installed SDK compilation must use inventory-backed identities and integrity checks through
    /// [`Self::from_resolved_inputs`].
    #[doc(hidden)]
    pub fn for_in_memory_sdk_manifest(library_manifest_index: LibraryManifestIndex, manifest: LibraryManifest) -> Self {
        let record = in_memory_sdk_record(manifest);
        let key = record.identity.stable_key();
        let module_catalog = record
            .namespace_claims
            .iter()
            .cloned()
            .map(|claim| (claim, key.clone()))
            .collect();
        Self {
            library_manifest_index,
            records: BTreeMap::from([(key, record)]),
            module_catalog,
            used_module_paths: BTreeSet::new(),
            bootstrap_sdk_namespace_roots: BTreeSet::new(),
        }
    }

    /// Create the temporary source-bootstrap SDK adapter while preserving resolved ordinary dependency features.
    ///
    /// This exists only until the source checkout publishes the same inventory-backed SDK artifacts as an installed
    /// distribution. Production installed-SDK compilation must use [`Self::from_resolved_inputs`].
    #[doc(hidden)]
    pub fn for_in_memory_sdk_manifest_with_features(
        library_manifest_index: LibraryManifestIndex,
        package_features: Option<&PackageFeaturePlan>,
        manifest: LibraryManifest,
    ) -> Result<Self, ProviderPlanError> {
        let record = in_memory_sdk_record(manifest);
        let mut records = project_dependency_records(&library_manifest_index, package_features)?;
        records.push(record);
        Self::new(library_manifest_index, records, std::iter::empty())
    }

    /// Create an in-memory SDK provider that owns module paths but has no checked manifest.
    ///
    /// This supports source-backed codegen tests whose provider source is already part of the fixture. Installed and
    /// package compilation must never use this adapter because available compiled providers require checked manifests.
    #[doc(hidden)]
    pub fn for_in_memory_sdk_modules(
        library_manifest_index: LibraryManifestIndex,
        relative_module_paths: impl IntoIterator<Item = Vec<String>>,
    ) -> Self {
        let namespace_claims = relative_module_paths
            .into_iter()
            .map(|relative| {
                let mut path = vec!["std".to_string()];
                path.extend(relative);
                path
            })
            .collect::<BTreeSet<_>>();
        let identity = ProviderIdentity {
            name: "in-memory-source-provider".to_string(),
            version: "0.0.0".to_string(),
            digest: "in-memory:source-provider".to_string(),
            feature_projection: BTreeSet::new(),
        };
        let key = identity.stable_key();
        let record = ProviderRecord {
            identity,
            provenance: ProviderProvenance::Sdk {
                sdk_identity: "in-memory".to_string(),
                component_id: "in-memory-source".to_string(),
                inventory_path: None,
            },
            authority: NamespaceAuthority::SdkReserved,
            namespace_claims: namespace_claims.clone(),
            available: true,
            enabled: true,
            manifest: None,
            artifact: None,
            implementation_facets: Vec::new(),
        };
        Self {
            library_manifest_index,
            records: BTreeMap::from([(key.clone(), record)]),
            module_catalog: namespace_claims.into_iter().map(|claim| (claim, key.clone())).collect(),
            used_module_paths: BTreeSet::new(),
            bootstrap_sdk_namespace_roots: BTreeSet::new(),
        }
    }

    /// Grant source ownership to one SDK component while its checked provider artifact is being bootstrapped.
    #[doc(hidden)]
    pub fn with_bootstrap_sdk_namespace_roots(mut self, roots: impl IntoIterator<Item = String>) -> Self {
        self.bootstrap_sdk_namespace_roots = roots.into_iter().collect();
        self
    }

    /// Return whether the current source-bootstrap component owns this canonical `std.*` module prefix.
    pub fn bootstrap_owns_sdk_module(&self, module: &[String]) -> bool {
        module.first().map(String::as_str) == Some("std")
            && module
                .get(1)
                .is_some_and(|root| self.bootstrap_sdk_namespace_roots.contains(root))
    }

    /// Return the source-bootstrap roots so a session can preserve them while refining module participation.
    pub fn bootstrap_sdk_namespace_roots(&self) -> impl Iterator<Item = &String> {
        self.bootstrap_sdk_namespace_roots.iter()
    }

    /// Iterate over every catalog provider in stable identity order.
    pub fn records(&self) -> impl Iterator<Item = &ProviderRecord> {
        self.records.values()
    }

    /// Return whether this plan carries an SDK-owned reserved-namespace catalog.
    pub fn has_sdk_catalog(&self) -> bool {
        self.records
            .values()
            .any(|provider| matches!(provider.authority, NamespaceAuthority::SdkReserved))
    }

    /// Iterate over enabled and available provider records.
    pub fn active_records(&self) -> impl Iterator<Item = &ProviderRecord> {
        self.records
            .values()
            .filter(|provider| provider.enabled && provider.available)
    }

    /// Iterate over active providers that own exact `std.*` namespace claims.
    pub fn active_sdk_records(&self) -> impl Iterator<Item = &ProviderRecord> {
        self.active_records()
            .filter(|provider| matches!(provider.authority, NamespaceAuthority::SdkReserved))
    }

    /// Iterate over active SDK providers reached by at least one canonical module in this compilation.
    pub fn used_sdk_records(&self) -> impl Iterator<Item = &ProviderRecord> {
        self.active_sdk_records()
            .filter(|provider| self.participation(provider) == ProviderParticipation::Used)
    }

    /// Return the minimal compiled SDK provider set that generated Cargo projects must link directly.
    ///
    /// A semantically used provider can already be supplied through another used provider's checked artifact graph.
    /// Keep that dependency transitive instead of adding the same provider crate to the consumer manifest twice.
    pub fn sdk_link_roots(&self) -> Vec<&ProviderRecord> {
        let used = self.used_sdk_records().collect::<Vec<_>>();
        let provider_by_dependency_key = self
            .active_sdk_records()
            .filter_map(|provider| {
                provider
                    .artifact
                    .as_ref()
                    .map(|artifact| (artifact.dependency_key.as_str(), provider.identity.stable_key()))
            })
            .collect::<BTreeMap<_, _>>();
        let mut supplied_transitively = BTreeSet::new();
        let mut pending = used
            .iter()
            .map(|provider| provider.identity.stable_key())
            .collect::<Vec<_>>();
        let mut traversed = BTreeSet::new();

        while let Some(provider_key) = pending.pop() {
            if !traversed.insert(provider_key.clone()) {
                continue;
            }
            let Some(provider) = self.records.get(&provider_key) else {
                continue;
            };
            let Some(manifest) = provider.manifest.as_deref() else {
                continue;
            };
            for dependency in &manifest.contract_metadata.provider.provider_dependencies {
                let Some(dependency_provider_key) = provider_by_dependency_key.get(dependency.dependency_key.as_str())
                else {
                    continue;
                };
                supplied_transitively.insert(dependency_provider_key.clone());
                pending.push(dependency_provider_key.clone());
            }
        }

        used.into_iter()
            .filter(|provider| !supplied_transitively.contains(&provider.identity.stable_key()))
            .collect()
    }

    /// Return every exact `std.*` module path supplied by active SDK providers.
    pub fn active_std_module_paths(&self) -> BTreeSet<Vec<String>> {
        self.active_sdk_records()
            .flat_map(|provider| provider.namespace_claims.iter().cloned())
            .collect()
    }

    /// Return the active SDK provider that owns an exact canonical `std.*` module path.
    pub fn active_sdk_provider_for_module(&self, module: &[String]) -> Option<&ProviderRecord> {
        match self.resolve_module(module) {
            ProviderModuleResolution::Active(provider)
                if matches!(provider.authority, NamespaceAuthority::SdkReserved) =>
            {
                Some(provider)
            }
            _ => None,
        }
    }

    /// Resolve one exact canonical module while preserving disabled and unavailable states.
    pub fn resolve_module(&self, module: &[String]) -> ProviderModuleResolution<'_> {
        let Some(key) = self.module_catalog.get(module) else {
            return ProviderModuleResolution::Unknown;
        };
        let Some(provider) = self.records.get(key) else {
            return ProviderModuleResolution::Unknown;
        };
        if !provider.enabled {
            ProviderModuleResolution::Disabled(provider)
        } else if !provider.available {
            ProviderModuleResolution::Unavailable(provider)
        } else {
            ProviderModuleResolution::Active(provider)
        }
    }

    /// Return the participation state of one provider in this compilation.
    pub fn participation(&self, provider: &ProviderRecord) -> ProviderParticipation {
        if !provider.enabled {
            ProviderParticipation::Disabled
        } else if !provider.available {
            ProviderParticipation::Unavailable
        } else if provider
            .namespace_claims
            .iter()
            .any(|claim| self.used_module_paths.contains(claim))
        {
            ProviderParticipation::Used
        } else {
            ProviderParticipation::Enabled
        }
    }

    /// Return provider-owned module paths reached by this compilation.
    pub fn used_modules(&self, provider: &ProviderRecord) -> BTreeSet<Vec<String>> {
        provider
            .namespace_claims
            .intersection(&self.used_module_paths)
            .cloned()
            .collect()
    }

    /// Return implementation facets selected by this compilation's semantic module use.
    ///
    /// Facet module paths are provider-local. This method applies the provider's granted namespace before comparing
    /// them with canonical used-module paths, keeping backend selection out of stdlib-specific import inspection.
    pub fn selected_implementation_facets<'a>(&'a self, provider: &'a ProviderRecord) -> Vec<&'a ImplementationFacet> {
        provider
            .implementation_facets
            .iter()
            .filter(|facet| {
                facet.required_modules.is_empty()
                    || facet
                        .required_modules
                        .iter()
                        .map(|module| canonical_provider_module(provider, module))
                        .any(|module| self.used_module_paths.contains(&module))
            })
            .collect()
    }

    /// Return the private backend requirements selected by active provider facets.
    pub fn selected_backend_requirements(
        &self,
        provider: &ProviderRecord,
    ) -> BTreeSet<BackendImplementationRequirement> {
        self.selected_implementation_facets(provider)
            .into_iter()
            .flat_map(|facet| facet.backend_requirements.iter().cloned())
            .collect()
    }

    /// Reject any enabled provider whose artifact is unavailable before compilation starts.
    pub fn validate_compilation_ready(&self) -> Result<(), ProviderPlanError> {
        if let Some(provider) = self
            .records
            .values()
            .find(|provider| provider.enabled && !provider.available)
        {
            return Err(ProviderPlanError::EnabledProviderUnavailable {
                provider: provider.identity.name.clone(),
            });
        }
        Ok(())
    }
}

/// Render one concise provider provenance chain for human diagnostics.
fn render_provider_provenance(provenance: &ProviderProvenance) -> String {
    match provenance {
        ProviderProvenance::ProjectDependency {
            dependency_key,
            manifest_path,
        } => format!("dependency `{dependency_key}` from {}", manifest_path.display()),
        ProviderProvenance::Sdk {
            sdk_identity,
            component_id,
            inventory_path,
        } => match inventory_path {
            Some(path) => format!(
                "SDK `{sdk_identity}` component `{component_id}` from {}",
                path.display()
            ),
            None => format!("SDK `{sdk_identity}` component `{component_id}`"),
        },
        ProviderProvenance::Compiler => "compiler-owned surface".to_string(),
    }
}

/// Apply one provider's consumer-granted namespace to a provider-local module path.
fn canonical_provider_module(provider: &ProviderRecord, module: &[String]) -> Vec<String> {
    let mut canonical = match &provider.authority {
        NamespaceAuthority::ProjectDependency { dependency_key } => {
            vec!["pub".to_string(), dependency_key.clone()]
        }
        NamespaceAuthority::SdkReserved => vec!["std".to_string()],
        NamespaceAuthority::Compiler => Vec::new(),
    };
    canonical.extend(module.iter().cloned());
    canonical
}

/// Normalize one checked source-bootstrap manifest into the same record shape as an installed SDK provider.
fn in_memory_sdk_record(manifest: LibraryManifest) -> ProviderRecord {
    let active_features = manifest.contract_metadata.provider.active_features.clone();
    let namespace_claims = active_provider_claims(&manifest, &active_features)
        .into_iter()
        .map(|relative| {
            let mut path = vec!["std".to_string()];
            path.extend(relative);
            path
        })
        .collect();
    let implementation_facets = implementation_facets(&manifest, &active_features);
    let name = manifest.name.clone();
    let version = manifest.version.clone();
    ProviderRecord {
        identity: ProviderIdentity {
            name: name.clone(),
            version,
            digest: format!("in-memory:{name}"),
            feature_projection: active_features,
        },
        provenance: ProviderProvenance::Sdk {
            sdk_identity: "in-memory".to_string(),
            component_id: "in-memory".to_string(),
            inventory_path: None,
        },
        authority: NamespaceAuthority::SdkReserved,
        namespace_claims,
        available: true,
        enabled: true,
        manifest: Some(Arc::new(manifest)),
        artifact: None,
        implementation_facets,
    }
}

/// Normalize loaded ordinary dependencies into provider records under their consumer-granted `pub::<key>` roots.
fn project_dependency_records(
    index: &LibraryManifestIndex,
    package_features: Option<&PackageFeaturePlan>,
) -> Result<Vec<ProviderRecord>, ProviderPlanError> {
    let mut records = Vec::new();
    for (dependency_key, manifest, artifact) in index.loaded_entries() {
        let active_features = package_features
            .and_then(|features| features.package(artifact_project_root(artifact)))
            .map(|package| package.features.active_features.clone())
            .unwrap_or_else(|| manifest.contract_metadata.provider.active_features.clone());
        let relative_claims = active_provider_claims(manifest, &active_features);
        let namespace_claims = relative_claims
            .into_iter()
            .map(|relative| {
                let mut claim = vec!["pub".to_string(), dependency_key.to_string()];
                claim.extend(relative);
                claim
            })
            .collect();
        let digest = match artifact.kind {
            LibraryArtifactKind::Materialized => {
                digest_provider_artifact(&artifact.crate_root).map_err(|error| ProviderPlanError::ManifestLoad {
                    provider: manifest.name.clone(),
                    path: artifact.crate_root.clone(),
                    message: error.to_string(),
                })?
            }
            LibraryArtifactKind::ParserSource => {
                format!("parser-source:{dependency_key}:{}@{}", manifest.name, manifest.version)
            }
        };
        let identity = ProviderIdentity {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            digest,
            feature_projection: active_features.clone(),
        };
        records.push(ProviderRecord {
            identity,
            provenance: ProviderProvenance::ProjectDependency {
                dependency_key: dependency_key.to_string(),
                manifest_path: artifact.manifest_path.clone(),
            },
            authority: NamespaceAuthority::ProjectDependency {
                dependency_key: dependency_key.to_string(),
            },
            namespace_claims,
            available: true,
            enabled: true,
            manifest: Some(Arc::new(manifest.clone())),
            artifact: (artifact.kind == LibraryArtifactKind::Materialized).then(|| artifact.clone()),
            implementation_facets: implementation_facets(manifest, &active_features),
        });
    }
    Ok(records)
}

/// Normalize every known SDK provider, including disabled and unavailable component records, into the shared catalog.
fn sdk_provider_records(
    inventory: &SdkInventory,
    resolved: Option<&ResolvedSdkComponents>,
) -> Result<Vec<ProviderRecord>, ProviderPlanError> {
    let enabled_components = resolved.map(|selection| &selection.enabled);
    let mut records = Vec::new();
    for component in inventory.components.values() {
        let enabled = enabled_components
            .map(|enabled| enabled.contains(&component.id))
            .unwrap_or(component.mandatory);
        for descriptor in &component.providers {
            let available = component.available;
            let (manifest, artifact) = if available {
                let manifest_path =
                    descriptor
                        .manifest_path
                        .as_ref()
                        .ok_or_else(|| ProviderPlanError::InventoryMismatch {
                            provider: descriptor.name.clone(),
                            message: "available provider has no manifest path".to_string(),
                        })?;
                let crate_root =
                    descriptor
                        .crate_root
                        .as_ref()
                        .ok_or_else(|| ProviderPlanError::InventoryMismatch {
                            provider: descriptor.name.clone(),
                            message: "available provider has no generated crate root".to_string(),
                        })?;
                let loaded = LibraryManifest::read_from_path(manifest_path).map_err(|error| {
                    ProviderPlanError::ManifestLoad {
                        provider: descriptor.name.clone(),
                        path: manifest_path.clone(),
                        message: manifest_error_message(error),
                    }
                })?;
                validate_sdk_descriptor(descriptor, &loaded, manifest_path)?;
                let artifact = LibraryArtifactMetadata::from_manifest_path(
                    descriptor.name.clone(),
                    loaded.name.clone(),
                    manifest_path.clone(),
                    crate_root.clone(),
                );
                (Some(Arc::new(loaded)), Some(artifact))
            } else {
                (None, None)
            };
            let active_features = manifest
                .as_ref()
                .map(|manifest| manifest.contract_metadata.provider.active_features.clone())
                .unwrap_or_default();
            let implementation_facets = manifest
                .as_ref()
                .map(|manifest| implementation_facets(manifest, &active_features))
                .unwrap_or_default();
            records.push(ProviderRecord {
                identity: ProviderIdentity {
                    name: descriptor.name.clone(),
                    version: descriptor.version.clone(),
                    digest: descriptor.digest.clone(),
                    feature_projection: active_features,
                },
                provenance: ProviderProvenance::Sdk {
                    sdk_identity: inventory.identity(),
                    component_id: component.id.clone(),
                    inventory_path: Some(inventory.root.join(super::SDK_INVENTORY_FILE)),
                },
                authority: NamespaceAuthority::SdkReserved,
                namespace_claims: descriptor.namespace_claims.clone(),
                available,
                enabled,
                manifest,
                artifact,
                implementation_facets,
            });
        }
    }
    Ok(records)
}

/// Return active provider-local module claims, falling back to checked API metadata for pre-RFC-114 artifacts.
fn active_provider_claims(manifest: &LibraryManifest, active_features: &BTreeSet<String>) -> BTreeSet<Vec<String>> {
    let provider = &manifest.contract_metadata.provider;
    if !provider.namespace_claims.is_empty() {
        return provider
            .namespace_claims
            .iter()
            .filter(|claim| claim.required_features.is_subset(active_features))
            .map(|claim| claim.module_path.clone())
            .collect();
    }
    manifest
        .contract_metadata
        .api
        .iter()
        .flat_map(|api| api.modules.iter())
        .filter(|module| module.module_path.as_slice() != ["main"])
        .map(|module| module.module_path.clone())
        .collect()
}

/// Translate provider-local implementation facets into the backend-neutral compiler plan representation.
fn implementation_facets(manifest: &LibraryManifest, active_features: &BTreeSet<String>) -> Vec<ImplementationFacet> {
    manifest
        .contract_metadata
        .provider
        .implementation_facets
        .iter()
        .filter(|facet| facet.required_features.is_subset(active_features))
        .map(implementation_facet)
        .collect()
}

fn implementation_facet(facet: &ProviderImplementationFacet) -> ImplementationFacet {
    let mut backend_requirements = facet
        .cargo_dependencies
        .iter()
        .map(|dependency| BackendImplementationRequirement::CargoDependency {
            dependency: dependency.clone(),
        })
        .collect::<Vec<_>>();
    for (crate_name, features) in &facet.cargo_features {
        backend_requirements.extend(
            features
                .iter()
                .map(|feature| BackendImplementationRequirement::CargoFeature {
                    crate_name: crate_name.clone(),
                    feature: feature.clone(),
                }),
        );
    }
    ImplementationFacet {
        id: facet.id.clone(),
        required_modules: facet.required_modules.clone(),
        required_features: facet.required_features.clone(),
        backend_requirements,
    }
}

/// Validate active package-owned component requirements without mutating the project component selection.
fn validate_package_component_requirements(
    features: &PackageFeaturePlan,
    components: &ResolvedSdkComponents,
) -> Result<(), ProviderPlanError> {
    for package in features.packages() {
        if let Some(component) = package
            .features
            .required_sdk_components
            .iter()
            .find(|component| !components.enabled.contains(*component))
        {
            return Err(ProviderPlanError::RequiredComponentDisabled {
                package: package.package_name.clone(),
                component: component.clone(),
            });
        }
    }
    Ok(())
}

/// Recover the producer project root from the conventional generated artifact layout.
fn artifact_project_root(artifact: &LibraryArtifactMetadata) -> &Path {
    artifact
        .crate_root
        .parent()
        .and_then(Path::parent)
        .unwrap_or(artifact.crate_root.as_path())
}

/// Verify that an installed provider agrees with the SDK identity and namespace grant that authorized it.
fn validate_sdk_descriptor(
    descriptor: &super::SdkProviderDescriptor,
    manifest: &LibraryManifest,
    manifest_path: &Path,
) -> Result<(), ProviderPlanError> {
    if descriptor.name != manifest.name || descriptor.version != manifest.version {
        return Err(ProviderPlanError::InventoryMismatch {
            provider: descriptor.name.clone(),
            message: format!(
                "inventory declares {}@{}, but {} contains {}@{}",
                descriptor.name,
                descriptor.version,
                manifest_path.display(),
                manifest.name,
                manifest.version
            ),
        });
    }
    let crate_root = descriptor
        .crate_root
        .as_deref()
        .ok_or_else(|| ProviderPlanError::InventoryMismatch {
            provider: descriptor.name.clone(),
            message: "available provider has no generated crate root".to_string(),
        })?;
    if !manifest_path.starts_with(crate_root) {
        return Err(ProviderPlanError::InventoryMismatch {
            provider: descriptor.name.clone(),
            message: format!(
                "manifest {} is outside generated crate root {}",
                manifest_path.display(),
                crate_root.display()
            ),
        });
    }
    let expected_claims = active_provider_claims(manifest, &manifest.contract_metadata.provider.active_features)
        .into_iter()
        .map(|relative| {
            let mut canonical = vec!["std".to_string()];
            canonical.extend(relative);
            canonical
        })
        .collect::<BTreeSet<_>>();
    if descriptor.namespace_claims != expected_claims {
        return Err(ProviderPlanError::InventoryMismatch {
            provider: descriptor.name.clone(),
            message: "inventory namespace claims differ from the checked provider manifest".to_string(),
        });
    }
    let digest = digest_provider_artifact(crate_root).map_err(|error| ProviderPlanError::ManifestLoad {
        provider: descriptor.name.clone(),
        path: crate_root.to_path_buf(),
        message: error.to_string(),
    })?;
    if descriptor.digest != digest {
        return Err(ProviderPlanError::InventoryMismatch {
            provider: descriptor.name.clone(),
            message: format!("expected digest `{}`, found `{digest}`", descriptor.digest),
        });
    }
    for dependency in &manifest.contract_metadata.provider.provider_dependencies {
        let dependency_root = crate_root.join(&dependency.relative_artifact_path);
        let loaded = load_provider_dependency_artifact(&dependency.dependency_key, &dependency_root);
        let (dependency_manifest, dependency_artifact) = match loaded {
            LibraryManifestIndexEntry::Loaded { manifest, metadata } => (manifest, metadata),
            LibraryManifestIndexEntry::Failed(failure) => {
                return Err(ProviderPlanError::InventoryMismatch {
                    provider: descriptor.name.clone(),
                    message: format!(
                        "provider dependency `{}` could not be loaded from {}: {}",
                        dependency.dependency_key,
                        failure.path.display(),
                        failure.message
                    ),
                });
            }
        };
        if dependency_manifest.name != dependency.provider_name
            || dependency_manifest.version != dependency.provider_version
        {
            return Err(ProviderPlanError::InventoryMismatch {
                provider: descriptor.name.clone(),
                message: format!(
                    "provider dependency `{}` expected {}@{}, found {}@{}",
                    dependency.dependency_key,
                    dependency.provider_name,
                    dependency.provider_version,
                    dependency_manifest.name,
                    dependency_manifest.version
                ),
            });
        }
        let dependency_digest = digest_provider_artifact(&dependency_artifact.crate_root).map_err(|error| {
            ProviderPlanError::ManifestLoad {
                provider: dependency.provider_name.clone(),
                path: dependency_artifact.crate_root.clone(),
                message: error.to_string(),
            }
        })?;
        if dependency_digest != dependency.artifact_digest {
            return Err(ProviderPlanError::InventoryMismatch {
                provider: descriptor.name.clone(),
                message: format!(
                    "provider dependency `{}` expected digest `{}`, found `{dependency_digest}`",
                    dependency.dependency_key, dependency.artifact_digest
                ),
            });
        }
    }
    Ok(())
}

fn manifest_error_message(error: LibraryManifestError) -> String {
    error.to_string()
}

/// Validate artifact completeness and exact namespace authority for one provider record.
fn validate_provider_record(record: &ProviderRecord) -> Result<(), ProviderPlanError> {
    if record.available && record.manifest.is_none() {
        return Err(ProviderPlanError::AvailableManifestMissing {
            provider: record.identity.name.clone(),
        });
    }
    for claim in &record.namespace_claims {
        let authorized = match &record.authority {
            NamespaceAuthority::ProjectDependency { dependency_key } => {
                claim.first().map(String::as_str) == Some("pub")
                    && claim.get(1).map(String::as_str) == Some(dependency_key.as_str())
            }
            NamespaceAuthority::SdkReserved => claim.first().map(String::as_str) == Some("std"),
            NamespaceAuthority::Compiler => claim.first().map(String::as_str) != Some("std"),
        };
        if !authorized {
            return Err(ProviderPlanError::UnauthorizedNamespace {
                provider: record.identity.name.clone(),
                module: render_module(claim),
            });
        }
    }
    Ok(())
}

/// Render one canonical module path for diagnostics and inspection.
fn render_module(module: &[String]) -> String {
    module.join(".")
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    use super::*;
    use crate::frontend::library_manifest_index::LibraryManifestIndex;
    use crate::library_manifest::LibraryManifest;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn resolves_active_disabled_and_unavailable_provider_modules() -> TestResult {
        let records = vec![
            record(
                "stdlib-core",
                NamespaceAuthority::SdkReserved,
                &[&["std", "result"]],
                true,
                true,
            ),
            record(
                "stdlib-web",
                NamespaceAuthority::SdkReserved,
                &[&["std", "web"]],
                true,
                false,
            ),
            record(
                "stdlib-data",
                NamespaceAuthority::SdkReserved,
                &[&["std", "json"]],
                false,
                true,
            ),
        ];
        let plan = ProviderPlan::new(LibraryManifestIndex::default(), records, [])?;

        assert!(matches!(
            plan.resolve_module(&path(&["std", "result"])),
            ProviderModuleResolution::Active(_)
        ));
        assert!(matches!(
            plan.resolve_module(&path(&["std", "web"])),
            ProviderModuleResolution::Disabled(_)
        ));
        assert!(matches!(
            plan.resolve_module(&path(&["std", "json"])),
            ProviderModuleResolution::Unavailable(_)
        ));
        assert!(matches!(
            plan.resolve_module(&path(&["std", "missing"])),
            ProviderModuleResolution::Unknown
        ));
        Ok(())
    }

    #[test]
    fn rejects_project_dependency_claiming_reserved_std_namespace() {
        let records = vec![record(
            "widgets",
            NamespaceAuthority::ProjectDependency {
                dependency_key: "widgets".to_string(),
            },
            &[&["std", "widgets"]],
            true,
            true,
        )];
        let result = ProviderPlan::new(LibraryManifestIndex::default(), records, []);

        assert!(matches!(result, Err(ProviderPlanError::UnauthorizedNamespace { .. })));
    }

    #[test]
    fn rejects_duplicate_exact_module_claims() -> TestResult {
        let records = vec![
            record(
                "stdlib-core-a",
                NamespaceAuthority::SdkReserved,
                &[&["std", "result"]],
                true,
                true,
            ),
            record(
                "stdlib-core-b",
                NamespaceAuthority::SdkReserved,
                &[&["std", "result"]],
                true,
                true,
            ),
        ];
        let result = ProviderPlan::new(LibraryManifestIndex::default(), records, []);

        let error = result.err().ok_or("expected namespace collision")?;
        assert!(matches!(error, ProviderPlanError::NamespaceCollision { .. }));
        assert!(
            error
                .to_string()
                .contains("SDK `incan@0.5.0` component `stdlib-core-a`")
        );
        assert!(
            error
                .to_string()
                .contains("SDK `incan@0.5.0` component `stdlib-core-b`")
        );
        Ok(())
    }

    #[test]
    fn used_state_is_derived_from_canonical_module_reachability() -> TestResult {
        let records = vec![record(
            "stdlib-core",
            NamespaceAuthority::SdkReserved,
            &[&["std", "result"], &["std", "traits", "convert"]],
            true,
            true,
        )];
        let plan = ProviderPlan::new(
            LibraryManifestIndex::default(),
            records,
            [path(&["std", "traits", "convert"])],
        )?;
        let provider = plan.records().next().ok_or("missing provider")?;

        assert_eq!(plan.participation(provider), ProviderParticipation::Used);
        assert_eq!(plan.used_modules(provider), set_paths(&[&["std", "traits", "convert"]]));
        Ok(())
    }

    #[test]
    fn source_bootstrap_grants_only_catalog_selected_std_roots() {
        let plan = ProviderPlan::default().with_bootstrap_sdk_namespace_roots(["io".to_string(), "fs".to_string()]);

        assert!(plan.bootstrap_owns_sdk_module(&path(&["std", "io"])));
        assert!(plan.bootstrap_owns_sdk_module(&path(&["std", "fs", "locking"])));
        assert!(!plan.bootstrap_owns_sdk_module(&path(&["std", "web"])));
        assert!(!plan.bootstrap_owns_sdk_module(&path(&["pub", "io"])));
        assert_eq!(
            plan.bootstrap_sdk_namespace_roots().cloned().collect::<BTreeSet<_>>(),
            BTreeSet::from(["fs".to_string(), "io".to_string()])
        );
    }

    #[test]
    fn ordinary_provider_plans_have_no_source_bootstrap_authority() -> TestResult {
        let plan = ProviderPlan::new(
            LibraryManifestIndex::default(),
            vec![record(
                "stdlib-core",
                NamespaceAuthority::SdkReserved,
                &[&["std", "result"]],
                true,
                true,
            )],
            [],
        )?;

        assert!(matches!(
            plan.resolve_module(&path(&["std", "unknown"])),
            ProviderModuleResolution::Unknown
        ));
        assert!(!plan.bootstrap_owns_sdk_module(&path(&["std", "result"])));
        assert_eq!(plan.bootstrap_sdk_namespace_roots().count(), 0);
        Ok(())
    }

    #[test]
    fn sdk_provider_integrity_covers_generated_rust_not_only_the_manifest() -> TestResult {
        let artifact = tempfile::tempdir()?;
        std::fs::create_dir_all(artifact.path().join("src"))?;
        std::fs::write(artifact.path().join("src/lib.rs"), "pub fn value() -> i32 { 1 }")?;
        std::fs::write(
            artifact.path().join("Cargo.toml"),
            "[package]\nname = \"stdlib_core\"\nversion = \"0.5.0\"\n",
        )?;
        let manifest_path = artifact.path().join("stdlib_core.incnlib");
        let mut manifest = LibraryManifest::new("stdlib_core", "0.5.0");
        manifest.contract_metadata.provider.namespace_claims = vec![crate::library_manifest::ProviderModuleClaim {
            module_path: path(&["result"]),
            required_features: BTreeSet::new(),
        }];
        manifest.write_to_path(&manifest_path)?;
        let inventory = sdk_inventory(
            artifact.path(),
            &manifest_path,
            digest_provider_artifact(artifact.path())?,
        );
        let selection = inventory.resolve(&super::super::SdkComponentSelection::default())?;

        ProviderPlan::from_resolved_inputs(
            LibraryManifestIndex::default(),
            None,
            Some(&inventory),
            Some(&selection),
            [],
        )?;

        std::fs::write(artifact.path().join("src/lib.rs"), "pub fn value() -> i32 { 2 }")?;
        let result = ProviderPlan::from_resolved_inputs(
            LibraryManifestIndex::default(),
            None,
            Some(&inventory),
            Some(&selection),
            [],
        );
        assert!(matches!(result, Err(ProviderPlanError::InventoryMismatch { .. })));
        Ok(())
    }

    #[test]
    fn sdk_inventory_claims_must_match_checked_provider_claims() -> TestResult {
        let artifact = tempfile::tempdir()?;
        std::fs::create_dir_all(artifact.path().join("src"))?;
        std::fs::write(artifact.path().join("src/lib.rs"), "")?;
        let manifest_path = artifact.path().join("stdlib_core.incnlib");
        let mut manifest = LibraryManifest::new("stdlib_core", "0.5.0");
        manifest.contract_metadata.provider.namespace_claims = vec![crate::library_manifest::ProviderModuleClaim {
            module_path: path(&["result"]),
            required_features: BTreeSet::new(),
        }];
        manifest.write_to_path(&manifest_path)?;
        let mut inventory = sdk_inventory(
            artifact.path(),
            &manifest_path,
            digest_provider_artifact(artifact.path())?,
        );
        inventory
            .components
            .get_mut("stdlib-core")
            .ok_or("missing component")?
            .providers[0]
            .namespace_claims = set_paths(&[&["std", "future"]]);
        let selection = inventory.resolve(&super::super::SdkComponentSelection::default())?;

        let result = ProviderPlan::from_resolved_inputs(
            LibraryManifestIndex::default(),
            None,
            Some(&inventory),
            Some(&selection),
            [],
        );
        assert!(matches!(result, Err(ProviderPlanError::InventoryMismatch { .. })));
        Ok(())
    }

    fn record(
        name: &str,
        authority: NamespaceAuthority,
        claims: &[&[&str]],
        available: bool,
        enabled: bool,
    ) -> ProviderRecord {
        let manifest = available.then(|| Arc::new(LibraryManifest::new(name, "0.5.0")));
        ProviderRecord {
            identity: ProviderIdentity {
                name: name.to_string(),
                version: "0.5.0".to_string(),
                digest: format!("sha256:{name}"),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::Sdk {
                sdk_identity: "incan@0.5.0".to_string(),
                component_id: name.to_string(),
                inventory_path: None,
            },
            authority,
            namespace_claims: claims.iter().map(|claim| path(claim)).collect(),
            available,
            enabled,
            manifest,
            artifact: None,
            implementation_facets: Vec::new(),
        }
    }

    fn path(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    fn set_paths(values: &[&[&str]]) -> BTreeSet<Vec<String>> {
        values.iter().map(|value| path(value)).collect()
    }

    fn sdk_inventory(root: &Path, manifest_path: &Path, digest: String) -> SdkInventory {
        SdkInventory {
            root: root.to_path_buf(),
            sdk_id: "incan".to_string(),
            sdk_version: "0.5.0".to_string(),
            compiler_requirement: ">=0.5.0-dev.5,<0.6.0".to_string(),
            components: BTreeMap::from([(
                "stdlib-core".to_string(),
                super::super::SdkComponent {
                    id: "stdlib-core".to_string(),
                    version: "0.5.0".to_string(),
                    mandatory: true,
                    available: true,
                    dependencies: BTreeSet::new(),
                    providers: vec![super::super::SdkProviderDescriptor {
                        name: "stdlib_core".to_string(),
                        version: "0.5.0".to_string(),
                        digest,
                        namespace_claims: set_paths(&[&["std", "result"]]),
                        manifest_path: Some(manifest_path.to_path_buf()),
                        crate_root: Some(root.to_path_buf()),
                    }],
                },
            )]),
            profiles: BTreeMap::from([
                ("minimal".to_string(), BTreeSet::from(["stdlib-core".to_string()])),
                ("default".to_string(), BTreeSet::from(["stdlib-core".to_string()])),
                ("full".to_string(), BTreeSet::from(["stdlib-core".to_string()])),
            ]),
        }
    }
}
