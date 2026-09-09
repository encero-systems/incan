//! Immutable provider catalog and active compilation projection from RFC 114.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::frontend::library_manifest_index::{
    LibraryArtifactKind, LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
    load_provider_dependency_artifact,
};
use crate::library_manifest::{
    LibraryManifest, LibraryManifestError, ProviderCargoDependency, ProviderDependencyKind, ProviderDependencyMetadata,
    ProviderImplementationFacet, digest_provider_artifact,
};

use super::features::feature_value_location;
use super::{PackageFeaturePlan, ResolvedSdkComponents, SdkInventory};

/// Stable identity of one immutable compiled-provider projection.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
    /// A public artifact query has no exact admitted node or encounters an inconsistent retained edge.
    #[error("cannot query admitted public artifact `{identity}`: {message}")]
    PublicArtifactQuery {
        /// Full requested identity or exact importing dependency key.
        identity: String,
        /// Missing admission or retained association detail.
        message: String,
    },
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
    #[error(
        "package `{package}` feature projection in {manifest_path}{location} requires disabled SDK component `{component}`"
    )]
    RequiredComponentDisabled {
        /// Package that owns the active feature requirement.
        package: String,
        /// Exact source manifest or checked provider artifact that introduced the requirement.
        manifest_path: PathBuf,
        /// Exact source location when the requirement came from an available project manifest.
        location: String,
        /// SDK component that must be selected explicitly.
        component: String,
    },
    /// A compiled library freezes a private SDK implementation that is not equivalent to the active SDK artifact.
    #[error(
        "compiled library `{library}` at {manifest_path} has incompatible private SDK provider `{provider}`: frozen identity `{frozen_identity}`, active SDK identity `{active_identity}`; rebuild the compiled library with the active Incan SDK"
    )]
    IncompatibleCompiledSdkDependency {
        /// Compiled library that froze the private implementation edge.
        library: String,
        /// Checked library manifest carrying the stale edge.
        manifest_path: PathBuf,
        /// SDK provider package name.
        provider: String,
        /// Stable name, version, digest, and feature projection frozen by the library.
        frozen_identity: String,
        /// Matching provider identities advertised by the active SDK, or an explicit absence marker.
        active_identity: String,
    },
}

/// One compiled-library dependency that must be projected through the active equivalent SDK artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SdkDependencyRebinding {
    /// Compiled library crate whose Cargo manifest freezes the old SDK path.
    pub containing_artifact: LibraryArtifactMetadata,
    /// Historical provider crate root recorded relative to the containing artifact; it may no longer exist.
    pub source_crate_root: PathBuf,
    /// Logical Cargo provider package name.
    pub provider_name: String,
    /// Cargo dependency key frozen in the containing generated manifest.
    pub dependency_key: String,
    /// Active inventory-owned provider crate root with equivalent semantic identity.
    pub active_crate_root: PathBuf,
}

/// One compiled artifact that must be copied into the consumer-owned SDK projection graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SdkArtifactProjection {
    /// Immutable compiled artifact whose dependency coordinates require projection.
    pub artifact: LibraryArtifactMetadata,
}

/// One public artifact admitted by the provider graph's checked identity and dependency-edge validation.
///
/// Equal identities reached through transitive edges retain one physical representative in the existing identity
/// table. Import and edge aliases belong to their grants/descriptors, never to that shared representative.
#[derive(Debug, Clone)]
pub(crate) struct PublicProviderArtifact {
    /// Selected package version, artifact digest and public feature projection.
    pub identity: ProviderIdentity,
    /// Checked manifest from that artifact generation.
    pub manifest: Arc<LibraryManifest>,
    /// Materialized artifact location; consumers must not rediscover dependency source.
    pub artifact: LibraryArtifactMetadata,
    /// Physical root key established during admission; querying the graph must not canonicalize disk paths again.
    pub admitted_root: PathBuf,
}

/// One public dependency association captured during the existing artifact admission pass.
#[derive(Debug, Clone)]
struct ResolvedPublicDependency {
    /// Position of the exact checked descriptor in the containing manifest, not a serialized identity.
    descriptor_index: usize,
    /// Full selected target identity key in the admitted artifact table.
    target_key: String,
}

/// Borrowed request and selected target of one already admitted public edge.
pub(crate) struct PublicProviderDependency<'a> {
    /// Position in the original checked manifest, including preceding private descriptors.
    pub descriptor_index: usize,
    /// Original checked request, retaining alias, kind, version and every feature/request dimension.
    pub descriptor: &'a ProviderDependencyMetadata,
    /// Selected artifact, including its actual active feature projection and established physical root.
    pub target: &'a PublicProviderArtifact,
}

/// One private descriptor and its exact SDK target retained during the original admission pass.
#[derive(Debug, Clone)]
struct ResolvedPrivateSdkDependency {
    descriptor_index: usize,
    /// SDK-free adapters retain an unresolved edge instead of claiming selected native support.
    target_key: Option<String>,
}

/// An original private request and its admitted semantic SDK owner, independent of physical path rebinding.
#[allow(dead_code, reason = "Pending #991: source-unit batch input projection is not wired")]
pub(crate) struct PrivateSdkDependency<'a> {
    pub descriptor_index: usize,
    pub descriptor: &'a ProviderDependencyMetadata,
    /// Absent only when the original graph was admitted without an SDK catalog; this grants no native input.
    pub target: Option<&'a ProviderRecord>,
}

/// Products retained from the single compiled-provider graph traversal.
#[derive(Default)]
struct ResolvedArtifactGraph {
    rebindings: Vec<SdkDependencyRebinding>,
    projections: Vec<SdkArtifactProjection>,
    public_artifacts: BTreeMap<String, PublicProviderArtifact>,
    /// Public dependency edges retained by artifact admission, keyed by the containing artifact root.
    public_dependencies: BTreeMap<PathBuf, Vec<ResolvedPublicDependency>>,
    private_sdk_dependencies: BTreeMap<PathBuf, Vec<ResolvedPrivateSdkDependency>>,
    /// Root dependency grants indexed during admission, separate from nested edge aliases.
    public_imports: BTreeMap<String, String>,
}

/// Immutable provider catalog and active module projection shared by every compiler stage.
#[derive(Debug, Clone, Default)]
pub struct ProviderPlan {
    library_manifest_index: LibraryManifestIndex,
    records: BTreeMap<String, ProviderRecord>,
    module_catalog: BTreeMap<Vec<String>, String>,
    used_module_paths: BTreeSet<Vec<String>>,
    sdk_dependency_rebindings: Vec<SdkDependencyRebinding>,
    sdk_artifact_projections: Vec<SdkArtifactProjection>,
    public_artifacts: BTreeMap<String, PublicProviderArtifact>,
    /// Public dependency edges retained by artifact admission, keyed by the containing artifact root.
    public_dependencies: BTreeMap<PathBuf, Vec<ResolvedPublicDependency>>,
    private_sdk_dependencies: BTreeMap<PathBuf, Vec<ResolvedPrivateSdkDependency>>,
    /// Exact ordinary import grants to selected root identities, retained without later path lookup.
    public_imports: BTreeMap<String, String>,
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
        let artifact_graph = resolve_artifact_graph(&indexed_records)?;

        Ok(Self {
            library_manifest_index,
            records: indexed_records,
            module_catalog,
            used_module_paths: used_module_paths.into_iter().collect(),
            sdk_dependency_rebindings: artifact_graph.rebindings,
            sdk_artifact_projections: artifact_graph.projections,
            public_artifacts: artifact_graph.public_artifacts,
            public_dependencies: artifact_graph.public_dependencies,
            private_sdk_dependencies: artifact_graph.private_sdk_dependencies,
            public_imports: artifact_graph.public_imports,
            bootstrap_sdk_namespace_roots: BTreeSet::new(),
        })
    }

    /// Return the consumer-side dependency manifest index normalized into this plan.
    pub fn library_manifest_index(&self) -> &LibraryManifestIndex {
        &self.library_manifest_index
    }

    /// Return public materialized artifacts already admitted by this plan, including validated transitive facades.
    pub(crate) fn public_artifacts(&self) -> impl Iterator<Item = &PublicProviderArtifact> {
        self.public_artifacts.values()
    }

    /// Borrow one exact admitted public artifact without reopening its physical location or choosing by name.
    pub(crate) fn public_artifact(
        &self,
        identity: &ProviderIdentity,
    ) -> Result<&PublicProviderArtifact, ProviderPlanError> {
        self.public_artifacts
            .get(&identity.stable_key())
            .filter(|artifact| artifact.identity == *identity)
            .ok_or_else(|| ProviderPlanError::PublicArtifactQuery {
                identity: identity.stable_key(),
                message: "no exact admitted public artifact".to_string(),
            })
    }

    /// Borrow the exact checked public edges of an admitted parent; private SDK edges remain separate.
    ///
    /// The descriptor positions and target keys originate in artifact admission. This query performs no source,
    /// manifest or filesystem lookup and does not reinterpret requested features as the child's active features.
    /// Distinct selected feature identities may share one physical root and its unchanged checked descriptor row;
    /// the parent lookup and each import grant still require their full selected identity.
    pub(crate) fn public_dependencies(
        &self,
        identity: &ProviderIdentity,
    ) -> Result<Vec<PublicProviderDependency<'_>>, ProviderPlanError> {
        let parent = self.public_artifact(identity)?;
        self.public_dependencies
            .get(&parent.admitted_root)
            .ok_or_else(|| ProviderPlanError::PublicArtifactQuery {
                identity: identity.stable_key(),
                message: "admitted parent has no retained edge row".to_string(),
            })?
            .iter()
            .map(|edge| {
                let invalid = || ProviderPlanError::PublicArtifactQuery {
                    identity: identity.stable_key(),
                    message: "retained public edge does not match its checked descriptor and selected target"
                        .to_string(),
                };
                let descriptor = parent
                    .manifest
                    .contract_metadata
                    .provider
                    .provider_dependencies
                    .get(edge.descriptor_index)
                    .ok_or_else(invalid)?;
                let target = self.public_artifacts.get(&edge.target_key).ok_or_else(invalid)?;
                if descriptor.kind != ProviderDependencyKind::PublicPackage
                    || descriptor.provider_name != target.identity.name
                    || descriptor.provider_version != target.identity.version
                    || descriptor.artifact_digest != target.identity.digest
                    || edge.target_key != target.identity.stable_key()
                {
                    return Err(invalid());
                }
                Ok(PublicProviderDependency {
                    descriptor_index: edge.descriptor_index,
                    descriptor,
                    target,
                })
            })
            .collect()
    }

    /// Borrow private SDK associations proved by the same pass that admitted the containing public artifact.
    ///
    /// Equal physical paths still retain a selected identity. An SDK-free adapter exposes each unresolved private
    /// descriptor with no target; an empty result means the admitted parent has no private descriptors. This query
    /// performs no file access or selection and never turns semantic SDK equality into a native member grant.
    #[allow(dead_code, reason = "Pending #991: source-unit batch input projection is not wired")]
    pub(crate) fn private_sdk_dependencies(
        &self,
        identity: &ProviderIdentity,
    ) -> Result<Vec<PrivateSdkDependency<'_>>, ProviderPlanError> {
        let parent = self.public_artifact(identity)?;
        let invalid = || ProviderPlanError::PublicArtifactQuery {
            identity: identity.stable_key(),
            message: "retained private SDK edge does not match its checked descriptor and selected target".to_string(),
        };
        self.private_sdk_dependencies
            .get(&parent.admitted_root)
            .ok_or_else(invalid)?
            .iter()
            .map(|edge| {
                let descriptor = parent
                    .manifest
                    .contract_metadata
                    .provider
                    .provider_dependencies
                    .get(edge.descriptor_index)
                    .filter(|descriptor| descriptor.kind == ProviderDependencyKind::PrivateImplementation)
                    .ok_or_else(invalid)?;
                let target = match &edge.target_key {
                    Some(key) => {
                        let target = self.records.get(key).ok_or_else(invalid)?;
                        if !matches!(target.authority, NamespaceAuthority::SdkReserved)
                            || key != &target.identity.stable_key()
                            || descriptor.provider_name != target.identity.name
                            || descriptor.provider_version != target.identity.version
                            || descriptor.artifact_digest != target.identity.digest
                            || descriptor.requested_features != target.identity.feature_projection
                            || descriptor.default_features
                            || descriptor.optional
                            || !target.enabled
                            || !target.available
                            || target.artifact.is_none()
                        {
                            return Err(invalid());
                        }
                        Some(target)
                    }
                    None => None,
                };
                Ok(PrivateSdkDependency {
                    descriptor_index: edge.descriptor_index,
                    descriptor,
                    target,
                })
            })
            .collect()
    }

    /// Resolve an exact ordinary import grant through the index retained at artifact admission.
    ///
    /// Production normalization creates a project record for every loaded dependency-index entry. SDK records use
    /// their separate reserved namespace and are not public import grants, even when a private edge links to them.
    fn public_import_artifact(&self, importing_library: &str) -> Result<&PublicProviderArtifact, String> {
        if !matches!(
            self.library_manifest_index.get(importing_library),
            Some(LibraryManifestIndexEntry::Loaded { .. })
        ) {
            return Err(format!(
                "public signature has no admitted importing library `{importing_library}`"
            ));
        }
        let key = self
            .public_imports
            .get(importing_library)
            .ok_or_else(|| format!("public signature has no admitted importing library `{importing_library}`"))?;
        self.public_artifacts
            .get(key)
            .ok_or_else(|| "admitted root import has no target artifact".to_string())
    }

    /// Project a foreign type through already-admitted public dependency edges and exact public membership.
    ///
    /// This query does not read dependency source or rediscover artifacts. The native route follows the existing
    /// compiler-owned dependency bridge, while semantic identity remains the selected artifact and declaration.
    pub(crate) fn public_nominal_projection(
        &self,
        importing_library: &str,
        origin: &crate::library_manifest::NominalTypeOriginExport,
    ) -> Result<
        (
            PublicProviderArtifact,
            crate::library_manifest::ExportIdentity,
            Vec<String>,
        ),
        String,
    > {
        let (target, export) = self.public_nominal_declaration(origin)?;
        let route = self.public_artifact_route(importing_library, &target.identity)?;
        Ok((target, export, route))
    }

    /// Return the existing admitted public dependency route to one exact selected artifact.
    ///
    /// Nominal and native representation projections share this traversal; neither query discovers another graph.
    pub(crate) fn public_artifact_route(
        &self,
        importing_library: &str,
        identity: &ProviderIdentity,
    ) -> Result<Vec<String>, String> {
        let target = self.public_artifact(identity).map_err(|error| error.to_string())?;
        let root = self.public_import_artifact(importing_library)?;
        let mut pending = std::collections::VecDeque::from([(root, Vec::new())]);
        let mut seen = BTreeSet::new();
        while let Some((parent, route)) = pending.pop_front() {
            if !seen.insert(parent.identity.clone()) {
                continue;
            }
            if parent.identity == target.identity {
                return Ok(route);
            }
            for edge in self
                .public_dependencies(&parent.identity)
                .map_err(|error| error.to_string())?
            {
                let mut child_route = route.clone();
                child_route.push(edge.descriptor.dependency_key.clone());
                pending.push_back((edge.target, child_route));
            }
        }
        Err(format!(
            "public signature in `{importing_library}` has no admitted public dependency route to {}",
            identity.stable_key()
        ))
    }

    /// Bind a producer union representation to its exact admitted owner and existing native dependency route.
    ///
    /// The wrapper must be present in the selected owner's final emitted-definition table. A plausible generated name
    /// is insufficient: its ordered semantic payloads must agree with the representation published by that
    /// artifact.
    pub(crate) fn public_native_union_projection(
        &self,
        importing_library: &str,
        native: &crate::library_manifest::NativeUnionExport,
    ) -> Result<(crate::library_manifest::NativeUnionExport, Vec<String>), String> {
        use crate::library_manifest::{NativeUnionOwnerExport, TypeRef, VisitTypeRefs};
        let identity = match &native.owner {
            NativeUnionOwnerExport::SelectedArtifact(identity) => identity.clone(),
            NativeUnionOwnerExport::ContainingArtifact => {
                self.public_import_artifact(importing_library)?.identity.clone()
            }
        };
        let route = self.public_artifact_route(importing_library, &identity)?;
        let artifact = self
            .public_artifacts
            .get(&identity.stable_key())
            .ok_or("union defining artifact was not admitted")?;
        let candidates = artifact
            .manifest
            .contract_metadata
            .native_unions
            .iter()
            .filter(|candidate| {
                candidate.owner == NativeUnionOwnerExport::ContainingArtifact && candidate.rust_name == native.rust_name
            })
            .cloned()
            .collect::<Vec<_>>();
        let normalize = |native: &crate::library_manifest::NativeUnionExport| -> Result<TypeRef, String> {
            let mut ty = TypeRef::NativeUnion(self.bind_native_union_local_nominals(native, &identity)?);
            ty.visit_type_refs(&mut |ty| match ty {
                TypeRef::Named {
                    name,
                    origin: Some(origin),
                }
                | TypeRef::Applied {
                    name,
                    origin: Some(origin),
                    ..
                } => *name = origin.binding_key(),
                _ => {}
            });
            Ok(ty)
        };
        let requested = normalize(native)?;
        let mut matched = None;
        for candidate in candidates {
            if normalize(&candidate)? == requested {
                matched = Some(candidate);
                break;
            }
        }
        let candidate = matched.ok_or_else(|| {
            format!(
                "native union `{}` has no matching emitted representation in {}",
                native.rust_name,
                identity.stable_key()
            )
        })?;
        let bound = self.bind_native_union_local_nominals(&candidate, &identity)?;
        Ok((bound, route))
    }

    /// Bind retained producer-local leaves only after validating every declaration against its selected owner.
    ///
    /// Even unused entries must belong to the owner's public nominal surface. Nested native wrappers retain their
    /// own owner and binding map; an enclosing union cannot lend them its declaration authority.
    fn bind_native_union_local_nominals(
        &self,
        native: &crate::library_manifest::NativeUnionExport,
        containing_owner: &crate::provider::ProviderIdentity,
    ) -> Result<crate::library_manifest::NativeUnionExport, String> {
        use crate::library_manifest::{NativeUnionOwnerExport, NominalTypeOriginExport, TypeRef};
        let mut bound = native.for_publication();
        let owner = match &bound.owner {
            NativeUnionOwnerExport::ContainingArtifact => containing_owner.clone(),
            NativeUnionOwnerExport::SelectedArtifact(identity) => identity.clone(),
        };
        let mut origins = BTreeMap::new();
        for (spelling, canonical) in &bound.local_nominals {
            let origin = NominalTypeOriginExport {
                provider: owner.clone(),
                canonical: canonical.clone(),
            };
            self.public_nominal_declaration(&origin)?;
            origins.insert(spelling.clone(), origin);
        }
        /// Bind only this wrapper's semantic leaves, crossing nested carriers through their own checked owner.
        fn bind_member(
            plan: &ProviderPlan,
            ty: &mut TypeRef,
            owner: &crate::provider::ProviderIdentity,
            origins: &BTreeMap<String, NominalTypeOriginExport>,
        ) -> Result<(), String> {
            match ty {
                TypeRef::Named { name, origin } | TypeRef::Applied { name, origin, .. } if origin.is_none() => {
                    *origin = origins.get(name).cloned();
                }
                _ => {}
            }
            match ty {
                TypeRef::NativeUnion(nested) => *nested = plan.bind_native_union_local_nominals(nested, owner)?,
                TypeRef::Applied { args, .. } | TypeRef::Tuple { elements: args } => {
                    for arg in args {
                        bind_member(plan, arg, owner, origins)?;
                    }
                }
                TypeRef::Function { params, return_type } => {
                    for param in params {
                        bind_member(plan, param, owner, origins)?;
                    }
                    bind_member(plan, return_type, owner, origins)?;
                }
                TypeRef::Ref { inner } | TypeRef::TypeToken { inner } => bind_member(plan, inner, owner, origins)?,
                TypeRef::Named { .. }
                | TypeRef::TypeParam { .. }
                | TypeRef::SelfType
                | TypeRef::RustPath { .. }
                | TypeRef::Unknown => {}
            }
            Ok(())
        }
        for member in &mut bound.members {
            bind_member(self, member, &owner, &origins)?;
        }
        bound.owner = NativeUnionOwnerExport::SelectedArtifact(owner);
        Ok(bound)
    }

    /// Resolve exact foreign nominal membership independently of its consumer's physical exposure route.
    pub(crate) fn public_nominal_declaration(
        &self,
        origin: &crate::library_manifest::NominalTypeOriginExport,
    ) -> Result<(PublicProviderArtifact, crate::library_manifest::ExportIdentity), String> {
        let target = self
            .public_artifacts
            .get(&origin.provider.stable_key())
            .ok_or_else(|| {
                format!(
                    "public signature requires unadmitted type artifact {}",
                    origin.provider.stable_key()
                )
            })?;
        let canonical = origin
            .canonical
            .hydrate()
            .ok_or("public signature has an invalid nominal identity")?;
        if !matches!(&canonical.origin, incan_semantics_core::SymbolOrigin::Package { library, .. }
            if library == &target.identity.name)
        {
            return Err("public signature nominal identity belongs to a different package".to_string());
        }
        let export = target
            .manifest
            .contract_metadata
            .identity_graph
            .exports
            .iter()
            .filter(|entry| entry.canonical.as_ref() == Some(&origin.canonical))
            .filter(|entry| {
                matches!(
                    entry.kind,
                    crate::library_manifest::ExportIdentityKind::Model
                        | crate::library_manifest::ExportIdentityKind::Class
                        | crate::library_manifest::ExportIdentityKind::Enum
                        | crate::library_manifest::ExportIdentityKind::Newtype
                )
            })
            .min_by(|left, right| {
                left.public_path
                    .len()
                    .cmp(&right.public_path.len())
                    .then(left.public_path.cmp(&right.public_path))
            })
            .ok_or_else(|| {
                format!(
                    "type `{}` is not a public nominal of {}",
                    canonical.declaration_name,
                    origin.provider.stable_key()
                )
            })?;
        Ok((target.clone(), export.clone()))
    }

    /// Find the exact declaring artifact reachable through already-admitted public edges of one import container.
    ///
    /// Canonical package names alone cannot select among multiple artifact generations. Such ambiguity refuses
    /// instead of collapsing nominal types; explicit leaf origins avoid it in newly published signatures.
    pub(crate) fn declaring_public_provider(
        &self,
        importing_library: &str,
        canonical: &incan_semantics_core::CanonicalSymbolId,
    ) -> Result<ProviderIdentity, String> {
        let root = self.public_import_artifact(importing_library)?;
        let mut pending = vec![root];
        let mut seen = BTreeSet::new();
        let mut candidates = BTreeMap::new();
        while let Some(artifact) = pending.pop() {
            if !seen.insert(artifact.identity.clone()) {
                continue;
            }
            if matches!(&canonical.origin, incan_semantics_core::SymbolOrigin::Package { library, .. }
                if library == &artifact.identity.name)
                && artifact
                    .manifest
                    .contract_metadata
                    .identity_graph
                    .exports
                    .iter()
                    .any(|entry| {
                        entry
                            .canonical
                            .as_ref()
                            .and_then(|identity| identity.hydrate())
                            .as_ref()
                            == Some(canonical)
                    })
            {
                candidates.insert(artifact.identity.stable_key(), artifact.identity.clone());
            }
            for edge in self
                .public_dependencies(&artifact.identity)
                .map_err(|error| error.to_string())?
            {
                pending.push(edge.target);
            }
        }
        if candidates.len() != 1 {
            return Err(format!(
                "type `{}` has {} admitted declaring artifacts through `{importing_library}`",
                canonical.declaration_name,
                candidates.len()
            ));
        }
        candidates
            .into_values()
            .next()
            .ok_or("declaring artifact disappeared".into())
    }

    /// Return artifact projections needed to replace stale physical SDK cache paths without mutating either artifact.
    pub(crate) fn sdk_dependency_rebindings(&self) -> &[SdkDependencyRebinding] {
        &self.sdk_dependency_rebindings
    }

    /// Return every compiled artifact whose private or transitive dependency coordinates require projection.
    pub(crate) fn sdk_artifact_projections(&self) -> &[SdkArtifactProjection] {
        &self.sdk_artifact_projections
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
            sdk_dependency_rebindings: Vec::new(),
            sdk_artifact_projections: Vec::new(),
            public_artifacts: BTreeMap::new(),
            public_dependencies: BTreeMap::new(),
            private_sdk_dependencies: BTreeMap::new(),
            public_imports: BTreeMap::new(),
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
            sdk_dependency_rebindings: Vec::new(),
            sdk_artifact_projections: Vec::new(),
            public_artifacts: BTreeMap::new(),
            public_dependencies: BTreeMap::new(),
            private_sdk_dependencies: BTreeMap::new(),
            public_imports: BTreeMap::new(),
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

    /// Return the compiled SDK provider set that generated Cargo projects must link directly.
    ///
    /// Every semantically used provider is a direct root because generated Rust names its owning crate explicitly;
    /// Rust does not place transitive dependencies in the consumer's extern prelude. Compiled project dependencies can
    /// also expose source-owned trait defaults whose checked signatures name a private SDK provider without a direct
    /// consumer import, so their frozen private implementation edges are compiler projection roots as well.
    pub fn sdk_link_roots(&self) -> Vec<&ProviderRecord> {
        let provider_by_dependency_key = self
            .active_sdk_records()
            .filter_map(|provider| {
                provider
                    .artifact
                    .as_ref()
                    .map(|artifact| (artifact.dependency_key.as_str(), provider.identity.stable_key()))
            })
            .collect::<BTreeMap<_, _>>();
        let compiler_projection_roots = self
            .active_records()
            .filter(|provider| matches!(provider.authority, NamespaceAuthority::ProjectDependency { .. }))
            .filter_map(|provider| provider.manifest.as_deref())
            .flat_map(|manifest| &manifest.contract_metadata.provider.provider_dependencies)
            .filter(|dependency| dependency.kind == ProviderDependencyKind::PrivateImplementation)
            .filter_map(|dependency| {
                provider_by_dependency_key
                    .get(dependency.dependency_key.as_str())
                    .cloned()
            })
            .collect::<BTreeSet<_>>();
        let mut selected = self
            .used_sdk_records()
            .map(|provider| provider.identity.stable_key())
            .collect::<BTreeSet<_>>();
        selected.extend(compiler_projection_roots.iter().cloned());
        self.active_sdk_records()
            .filter(|provider| selected.contains(&provider.identity.stable_key()))
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

/// Admit public compiled artifacts and resolve historical private SDK edges through one graph traversal.
///
/// The checked `.incnlib` descriptor is authoritative for the frozen name, version, digest, and feature projection;
/// the old physical cache root is deliberately not read because content-addressed provider generations may already
/// have been collected. Only an enabled, available active SDK record with the exact identity can replace that path.
fn resolve_artifact_graph(
    records: &BTreeMap<String, ProviderRecord>,
) -> Result<ResolvedArtifactGraph, ProviderPlanError> {
    let sdk_records = records
        .values()
        .filter(|record| matches!(record.authority, NamespaceAuthority::SdkReserved))
        .collect::<Vec<_>>();
    let mut public_artifacts = BTreeMap::new();
    let mut public_dependencies = BTreeMap::new();
    let mut private_sdk_dependencies = BTreeMap::new();
    let mut public_imports = BTreeMap::new();
    let mut rebindings = Vec::new();
    let mut projected = BTreeMap::<PathBuf, LibraryArtifactMetadata>::new();
    let mut visited = BTreeSet::new();
    let mut visiting = BTreeSet::new();
    for library in records
        .values()
        .filter(|record| record.enabled && record.available)
        .filter(|record| matches!(record.authority, NamespaceAuthority::ProjectDependency { .. }))
    {
        let (Some(manifest), Some(containing_artifact)) = (library.manifest.as_deref(), library.artifact.as_ref())
        else {
            continue;
        };
        let admitted_root = normalize_artifact_root(&containing_artifact.crate_root);
        if let NamespaceAuthority::ProjectDependency { dependency_key } = &library.authority
            && public_imports
                .insert(dependency_key.clone(), library.identity.stable_key())
                .is_some()
        {
            return Err(ProviderPlanError::PublicArtifactQuery {
                identity: dependency_key.clone(),
                message: "root dependency grant has more than one admitted identity".to_string(),
            });
        }
        public_artifacts.insert(
            library.identity.stable_key(),
            PublicProviderArtifact {
                identity: library.identity.clone(),
                manifest: Arc::new(manifest.clone()),
                artifact: containing_artifact.clone(),
                admitted_root: admitted_root.clone(),
            },
        );
        resolve_sdk_artifact_projection(
            &library.identity.name,
            manifest,
            containing_artifact,
            &admitted_root,
            &sdk_records,
            &mut visiting,
            &mut visited,
            &mut rebindings,
            &mut projected,
            &mut public_artifacts,
            &mut public_dependencies,
            &mut private_sdk_dependencies,
        )?;
    }
    rebindings.sort_by(|left, right| {
        (
            &left.containing_artifact.crate_root,
            &left.provider_name,
            &left.dependency_key,
            &left.source_crate_root,
            &left.active_crate_root,
        )
            .cmp(&(
                &right.containing_artifact.crate_root,
                &right.provider_name,
                &right.dependency_key,
                &right.source_crate_root,
                &right.active_crate_root,
            ))
    });
    rebindings.dedup();
    let projections = projected
        .into_values()
        .map(|artifact| SdkArtifactProjection { artifact })
        .collect();
    Ok(ResolvedArtifactGraph {
        rebindings,
        projections,
        public_artifacts,
        public_dependencies,
        private_sdk_dependencies,
        public_imports,
    })
}

/// Traverse one compiled provider graph and mark every ancestor that must point at a projected child artifact.
#[allow(clippy::too_many_arguments)]
fn resolve_sdk_artifact_projection(
    library_name: &str,
    manifest: &LibraryManifest,
    artifact: &LibraryArtifactMetadata,
    admitted_root: &Path,
    sdk_records: &[&ProviderRecord],
    visiting: &mut BTreeSet<PathBuf>,
    visited: &mut BTreeSet<PathBuf>,
    rebindings: &mut Vec<SdkDependencyRebinding>,
    projected: &mut BTreeMap<PathBuf, LibraryArtifactMetadata>,
    public_artifacts: &mut BTreeMap<String, PublicProviderArtifact>,
    public_dependencies: &mut BTreeMap<PathBuf, Vec<ResolvedPublicDependency>>,
    private_sdk_dependencies: &mut BTreeMap<PathBuf, Vec<ResolvedPrivateSdkDependency>>,
) -> Result<bool, ProviderPlanError> {
    let artifact_root = admitted_root.to_path_buf();
    if visited.contains(&artifact_root) {
        return Ok(projected.contains_key(&artifact_root));
    }
    if !visiting.insert(artifact_root.clone()) {
        return Err(ProviderPlanError::ManifestLoad {
            provider: library_name.to_string(),
            path: artifact.manifest_path.clone(),
            message: "compiled provider dependency graph contains a cycle".to_string(),
        });
    }

    let mut requires_projection = false;
    public_dependencies.entry(artifact_root.clone()).or_default();
    private_sdk_dependencies.entry(artifact_root.clone()).or_default();
    for (descriptor_index, dependency) in manifest
        .contract_metadata
        .provider
        .provider_dependencies
        .iter()
        .enumerate()
    {
        if dependency.kind == ProviderDependencyKind::PrivateImplementation {
            // SDK-free adapters have no replacement inventory. Private implementation edges never grant a public
            // semantic artifact, regardless of whether a native SDK rebinding is available.
            if sdk_records.is_empty() {
                private_sdk_dependencies
                    .entry(artifact_root.clone())
                    .or_default()
                    .push(ResolvedPrivateSdkDependency {
                        descriptor_index,
                        target_key: None,
                    });
                continue;
            }
            let candidates = sdk_records
                .iter()
                .copied()
                .filter(|record| record.identity.name == dependency.provider_name)
                .collect::<Vec<_>>();
            let exact = candidates
                .iter()
                .copied()
                .filter(|record| {
                    record.identity.version == dependency.provider_version
                        && record.identity.digest == dependency.artifact_digest
                        && record.identity.feature_projection == dependency.requested_features
                        && record.enabled
                        && record.available
                        && record.artifact.is_some()
                })
                .collect::<Vec<_>>();
            if dependency.default_features || dependency.optional || exact.len() != 1 {
                return Err(incompatible_compiled_sdk_dependency(
                    library_name,
                    artifact,
                    dependency,
                    &candidates,
                ));
            }
            let Some(active_artifact) = exact[0].artifact.as_ref() else {
                return Err(incompatible_compiled_sdk_dependency(
                    library_name,
                    artifact,
                    dependency,
                    &candidates,
                ));
            };
            private_sdk_dependencies
                .entry(artifact_root.clone())
                .or_default()
                .push(ResolvedPrivateSdkDependency {
                    descriptor_index,
                    target_key: Some(exact[0].identity.stable_key()),
                });
            let source_crate_root =
                normalize_artifact_root(&artifact.crate_root.join(&dependency.relative_artifact_path));
            let active_crate_root = normalize_artifact_root(&active_artifact.crate_root);
            if source_crate_root != active_crate_root {
                rebindings.push(SdkDependencyRebinding {
                    containing_artifact: artifact.clone(),
                    source_crate_root,
                    provider_name: dependency.provider_name.clone(),
                    dependency_key: dependency.dependency_key.clone(),
                    active_crate_root,
                });
                requires_projection = true;
            }
            continue;
        }

        let dependency_root = artifact.crate_root.join(&dependency.relative_artifact_path);
        let loaded = load_provider_dependency_artifact(&dependency.dependency_key, &dependency_root);
        let (dependency_manifest, dependency_artifact) = match loaded {
            LibraryManifestIndexEntry::Loaded { manifest, metadata } => (manifest, metadata),
            LibraryManifestIndexEntry::Failed(failure) => {
                return Err(ProviderPlanError::ManifestLoad {
                    provider: dependency.provider_name.clone(),
                    path: failure.path,
                    message: failure.message,
                });
            }
        };
        validate_transitive_provider_dependency(dependency, &dependency_manifest, &dependency_artifact)?;
        let identity = ProviderIdentity {
            name: dependency.provider_name.clone(),
            version: dependency.provider_version.clone(),
            digest: dependency.artifact_digest.clone(),
            feature_projection: dependency_manifest.contract_metadata.provider.active_features.clone(),
        };
        public_dependencies
            .entry(artifact_root.clone())
            .or_default()
            .push(ResolvedPublicDependency {
                descriptor_index,
                target_key: identity.stable_key(),
            });
        let dependency_admitted_root = normalize_artifact_root(&dependency_artifact.crate_root);
        public_artifacts.insert(
            identity.stable_key(),
            PublicProviderArtifact {
                identity,
                manifest: Arc::new((*dependency_manifest).clone()),
                artifact: dependency_artifact.clone(),
                admitted_root: dependency_admitted_root.clone(),
            },
        );
        if resolve_sdk_artifact_projection(
            &dependency.provider_name,
            &dependency_manifest,
            &dependency_artifact,
            &dependency_admitted_root,
            sdk_records,
            visiting,
            visited,
            rebindings,
            projected,
            public_artifacts,
            public_dependencies,
            private_sdk_dependencies,
        )? {
            requires_projection = true;
        }
    }

    visiting.remove(&artifact_root);
    visited.insert(artifact_root.clone());
    if requires_projection {
        projected.insert(artifact_root, artifact.clone());
    }
    Ok(requires_projection)
}

/// Validate the stable identity on one public compiled-provider edge before traversing it.
fn validate_transitive_provider_dependency(
    descriptor: &ProviderDependencyMetadata,
    manifest: &LibraryManifest,
    artifact: &LibraryArtifactMetadata,
) -> Result<(), ProviderPlanError> {
    if manifest.name != descriptor.provider_name || manifest.version != descriptor.provider_version {
        return Err(ProviderPlanError::ManifestLoad {
            provider: descriptor.provider_name.clone(),
            path: artifact.manifest_path.clone(),
            message: format!(
                "expected {}@{}, found {}@{}",
                descriptor.provider_name, descriptor.provider_version, manifest.name, manifest.version
            ),
        });
    }
    let digest = digest_provider_artifact(&artifact.crate_root).map_err(|error| ProviderPlanError::ManifestLoad {
        provider: descriptor.provider_name.clone(),
        path: artifact.crate_root.clone(),
        message: error.to_string(),
    })?;
    if digest != descriptor.artifact_digest {
        return Err(ProviderPlanError::ManifestLoad {
            provider: descriptor.provider_name.clone(),
            path: artifact.crate_root.clone(),
            message: format!(
                "expected artifact digest `{}`, found `{digest}`",
                descriptor.artifact_digest
            ),
        });
    }
    Ok(())
}

/// Preserve a targeted pre-Cargo diagnostic when an active SDK cannot satisfy one frozen private implementation edge.
fn incompatible_compiled_sdk_dependency(
    library_name: &str,
    artifact: &LibraryArtifactMetadata,
    dependency: &ProviderDependencyMetadata,
    candidates: &[&ProviderRecord],
) -> ProviderPlanError {
    let active_identity = if candidates.is_empty() {
        "<missing from active SDK inventory>".to_string()
    } else {
        candidates
            .iter()
            .map(|candidate| {
                let availability = if !candidate.enabled {
                    "disabled"
                } else if !candidate.available || candidate.artifact.is_none() {
                    "unavailable"
                } else {
                    "available"
                };
                format!("{} ({availability})", candidate.identity.stable_key())
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    ProviderPlanError::IncompatibleCompiledSdkDependency {
        library: library_name.to_string(),
        manifest_path: artifact.manifest_path.clone(),
        provider: dependency.provider_name.clone(),
        frozen_identity: provider_dependency_stable_identity(dependency),
        active_identity,
    }
}

/// Render the same stable identity dimensions used by active SDK provider records.
fn provider_dependency_stable_identity(dependency: &ProviderDependencyMetadata) -> String {
    let features = dependency
        .requested_features
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{}@{}#{}[{}]",
        dependency.provider_name, dependency.provider_version, dependency.artifact_digest, features
    )
}

/// Canonicalize an artifact root when it exists while retaining an absent historical cache coordinate verbatim.
fn normalize_artifact_root(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
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
            LibraryArtifactKind::StandardVocab => {
                format!("standard-vocab:{dependency_key}:{}@{}", manifest.name, manifest.version)
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

/// Translate one checked provider facet into backend-neutral implementation requirements for the resolved plan.
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
            let candidates = [component.clone()];
            return Err(ProviderPlanError::RequiredComponentDisabled {
                package: package.package_name.clone(),
                manifest_path: package.feature_manifest_path.clone(),
                location: feature_value_location(&package.feature_manifest_path, None, &candidates),
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

/// Preserve a library-manifest validation failure as the provider-plan diagnostic payload.
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
    use crate::manifest::ProjectManifest;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Write a materialized provider directly, without a generated Cargo manifest or an SDK.
    fn public_graph_artifact(
        root: &Path,
        name: &str,
        dependencies: Vec<ProviderDependencyMetadata>,
    ) -> Result<LibraryArtifactMetadata, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::write(root.join("src/lib.rs"), "pub fn value() -> i64 { 42 }\n")?;
        let mut manifest = LibraryManifest::new(name, "1.0.0");
        manifest.contract_metadata.provider.provider_dependencies = dependencies;
        let artifact = LibraryArtifactMetadata::from_crate_root(name, name, root);
        manifest.write_to_path(&artifact.manifest_path)?;
        Ok(artifact)
    }

    /// Bind a test edge to actual published child bytes while keeping its request flags distinct from node identity.
    fn public_graph_edge(
        alias: &str,
        child: &LibraryArtifactMetadata,
        relative: &str,
    ) -> Result<ProviderDependencyMetadata, Box<dyn std::error::Error>> {
        Ok(ProviderDependencyMetadata {
            kind: ProviderDependencyKind::PublicPackage,
            dependency_key: alias.to_string(),
            provider_name: child.manifest_name.clone(),
            provider_version: "1.0.0".to_string(),
            artifact_digest: digest_provider_artifact(&child.crate_root)?,
            relative_artifact_path: relative.to_string(),
            requested_features: BTreeSet::from(["requested".to_string()]),
            default_features: true,
            optional: true,
        })
    }

    /// Admit a real root through the same index loader and provider graph used by compilation.
    fn public_graph_plan(root: &LibraryArtifactMetadata) -> Result<ProviderPlan, Box<dyn std::error::Error>> {
        let entry = load_provider_dependency_artifact("entry", &root.crate_root);
        let index = LibraryManifestIndex::from_entries(std::collections::HashMap::from([("entry".to_string(), entry)]));
        Ok(ProviderPlan::from_resolved_inputs(index, None, None, None, [])?)
    }

    /// A diamond reuses exact admitted nodes and ordered request descriptors after physical sources move away.
    #[test]
    fn admitted_public_graph_retains_diamond_edges_without_disk_queries() -> TestResult {
        let workspace = tempfile::tempdir()?;
        let live = workspace.path().join("live");
        let leaf = public_graph_artifact(&live.join("leaf"), "catalog", Vec::new())?;
        let mut private = public_graph_edge("sdk_private", &leaf, "../absent-private-sdk")?;
        private.kind = ProviderDependencyKind::PrivateImplementation;
        let left_edge = public_graph_edge("stock", &leaf, "../leaf")?;
        let left = public_graph_artifact(&live.join("left"), "left", vec![private, left_edge.clone()])?;
        let right = public_graph_artifact(
            &live.join("right"),
            "right",
            vec![public_graph_edge("inventory", &leaf, "../leaf")?],
        )?;
        let root = public_graph_artifact(
            &live.join("root"),
            "root",
            vec![
                public_graph_edge("left_alias", &left, "../left")?,
                public_graph_edge("right_alias", &right, "../right")?,
            ],
        )?;
        let plan = public_graph_plan(&root)?;
        let root = plan.public_import_artifact("entry")?;
        let parents = plan.public_dependencies(&root.identity)?;
        assert_eq!(
            parents.iter().map(|edge| edge.descriptor_index).collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(
            parents
                .iter()
                .map(|edge| edge.descriptor.dependency_key.as_str())
                .collect::<Vec<_>>(),
            ["left_alias", "right_alias"]
        );
        let left_children = plan.public_dependencies(&parents[0].target.identity)?;
        let right_children = plan.public_dependencies(&parents[1].target.identity)?;
        assert_eq!(
            left_children.len(),
            1,
            "the preceding private SDK descriptor must not become public"
        );
        assert_eq!(left_children[0].descriptor, &left_edge);
        assert_eq!(left_children[0].descriptor_index, 1);
        assert_eq!(right_children[0].descriptor_index, 0);
        let private = plan.private_sdk_dependencies(&parents[0].target.identity)?;
        assert_eq!(private.len(), 1);
        assert_eq!(private[0].descriptor_index, 0);
        assert_eq!(private[0].descriptor.dependency_key, "sdk_private");
        assert!(
            private[0].target.is_none(),
            "SDK-free admission must not invent a selected owner"
        );
        assert!(left_children[0].target.identity.feature_projection.is_empty());
        assert!(std::ptr::eq(left_children[0].target, right_children[0].target));
        let leaf_identity = left_children[0].target.identity.clone();
        let expected_route = vec!["left_alias".to_string(), "stock".to_string()];
        assert_eq!(plan.public_artifact_route("entry", &leaf_identity)?, expected_route);
        std::fs::rename(&live, workspace.path().join("moved"))?;
        assert!(!live.exists());
        assert_eq!(plan.public_artifact_route("entry", &leaf_identity)?, expected_route);
        assert_eq!(plan.public_dependencies(&root.identity)?.len(), 2);
        assert_eq!(
            plan.public_dependencies(&parents[0].target.identity)?[0].descriptor_index,
            1
        );
        assert!(plan.public_dependencies(&leaf_identity)?.is_empty());
        assert!(plan.private_sdk_dependencies(&leaf_identity)?.is_empty());
        assert!(
            plan.private_sdk_dependencies(&parents[0].target.identity)?[0]
                .target
                .is_none()
        );
        assert_eq!(plan.public_artifact(&leaf_identity)?.identity, leaf_identity);
        assert_eq!(plan.sdk_dependency_rebindings().len(), 0);
        Ok(())
    }

    /// Same-spelling generations stay distinct, and missing or internally inconsistent associations refuse.
    #[test]
    fn admitted_public_graph_refuses_ambiguous_or_broken_identity_views() -> TestResult {
        let workspace = tempfile::tempdir()?;
        let first = public_graph_artifact(&workspace.path().join("first"), "catalog", Vec::new())?;
        let second = public_graph_artifact(&workspace.path().join("second"), "catalog", Vec::new())?;
        std::fs::write(second.crate_root.join("src/lib.rs"), "pub fn value() -> i64 { 43 }\n")?;
        let root = public_graph_artifact(
            &workspace.path().join("root"),
            "root",
            vec![
                public_graph_edge("first", &first, "../first")?,
                public_graph_edge("second", &second, "../second")?,
            ],
        )?;
        let mut plan = public_graph_plan(&root)?;
        let root_identity = plan.public_import_artifact("entry")?.identity.clone();
        let edges = plan.public_dependencies(&root_identity)?;
        assert_eq!(edges[0].target.identity.name, edges[1].target.identity.name);
        assert_ne!(edges[0].target.identity.digest, edges[1].target.identity.digest);
        assert!(!std::ptr::eq(edges[0].target, edges[1].target));
        let mut wrong = edges[0].target.identity.clone();
        wrong.feature_projection.insert("not-selected".to_string());
        assert!(plan.public_artifact(&wrong).is_err());
        assert!(plan.public_dependencies(&wrong).is_err());
        let root_key = plan.public_artifact(&root_identity)?.admitted_root.clone();
        let rows = plan
            .public_dependencies
            .get_mut(&root_key)
            .ok_or("admitted edges missing")?;
        let row = rows.first_mut().ok_or("first edge missing")?;
        row.descriptor_index = usize::MAX;
        assert!(plan.public_dependencies(&root_identity).is_err());
        let rows = plan
            .public_dependencies
            .get_mut(&root_key)
            .ok_or("admitted edges missing")?;
        let row = rows.first_mut().ok_or("first edge missing")?;
        row.descriptor_index = 0;
        row.target_key = wrong.stable_key();
        assert!(plan.public_dependencies(&root_identity).is_err());
        plan.public_dependencies.remove(&root_key);
        assert!(
            plan.public_dependencies(&root_identity).is_err(),
            "a missing retained row must not become a leaf"
        );
        Ok(())
    }

    /// Direct grants preserve duplicate-identity refusal; transitive aliases share one equal-byte representative.
    #[test]
    fn admitted_public_graph_keeps_relocated_identity_and_aliases_separate() -> TestResult {
        let workspace = tempfile::tempdir()?;
        let first = public_graph_artifact(&workspace.path().join("first"), "catalog", Vec::new())?;
        let second = public_graph_artifact(&workspace.path().join("second"), "catalog", Vec::new())?;
        assert_eq!(
            digest_provider_artifact(&first.crate_root)?,
            digest_provider_artifact(&second.crate_root)?
        );
        let direct_index = LibraryManifestIndex::from_entries(std::collections::HashMap::from([
            (
                "stock".to_string(),
                load_provider_dependency_artifact("stock", &first.crate_root),
            ),
            (
                "inventory".to_string(),
                load_provider_dependency_artifact("inventory", &second.crate_root),
            ),
        ]));
        assert!(matches!(
            ProviderPlan::from_resolved_inputs(direct_index, None, None, None, []),
            Err(ProviderPlanError::DuplicateIdentity { .. })
        ));

        let first_edge = public_graph_edge("stock", &first, "../first")?;
        let second_edge = public_graph_edge("inventory", &second, "../second")?;
        let root = public_graph_artifact(
            &workspace.path().join("root"),
            "root",
            vec![first_edge.clone(), second_edge.clone()],
        )?;
        let plan = public_graph_plan(&root)?;
        let root = plan.public_import_artifact("entry")?;
        let children = plan.public_dependencies(&root.identity)?;
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].descriptor, &first_edge);
        assert_eq!(children[1].descriptor, &second_edge);
        assert!(std::ptr::eq(children[0].target, children[1].target));
        assert_eq!(
            children[0].target.admitted_root,
            std::fs::canonicalize(&children[0].target.artifact.crate_root)?
        );
        assert!(plan.public_dependencies(&children[0].target.identity)?.is_empty());
        assert_eq!(
            plan.public_artifact_route("entry", &children[0].target.identity)?,
            ["stock"]
        );
        Ok(())
    }

    /// Feature projections can share immutable physical descriptors without becoming interchangeable identities.
    #[test]
    fn admitted_public_graph_binds_feature_identity_at_a_shared_root() -> TestResult {
        use crate::library_manifest::{NativeUnionExport, NativeUnionOwnerExport, TypeRef};

        let workspace = tempfile::tempdir()?;
        let leaf = public_graph_artifact(&workspace.path().join("leaf"), "catalog", Vec::new())?;
        let edge = public_graph_edge("stock", &leaf, "../leaf")?;
        let root = public_graph_artifact(&workspace.path().join("root"), "root", vec![edge.clone()])?;
        let mut manifest = LibraryManifest::read_from_path(&root.manifest_path)?;
        let native = NativeUnionExport {
            owner: NativeUnionOwnerExport::ContainingArtifact,
            rust_name: "__IncanUnion0123456789abcdef".to_string(),
            members: vec![
                TypeRef::Named {
                    name: "int".to_string(),
                    origin: None,
                },
                TypeRef::Named {
                    name: "str".to_string(),
                    origin: None,
                },
            ],
            local_nominals: BTreeMap::new(),
            checked_projection: None,
        };
        manifest.contract_metadata.native_unions.push(native.clone());
        manifest.write_to_path(&root.manifest_path)?;
        let index = LibraryManifestIndex::from_entries(std::collections::HashMap::from([
            (
                "first".to_string(),
                load_provider_dependency_artifact("first", &root.crate_root),
            ),
            (
                "second".to_string(),
                load_provider_dependency_artifact("second", &root.crate_root),
            ),
        ]));
        let mut records = project_dependency_records(&index, None)?;
        for record in &mut records {
            let NamespaceAuthority::ProjectDependency { dependency_key } = &record.authority else {
                return Err("fixture did not normalize an ordinary import".into());
            };
            record.identity.feature_projection.insert(dependency_key.clone());
        }
        // Supply two already-selected feature projections to the existing normalization constructor. This does not
        // ask the host to derive feature selection from a shared physical path.
        let plan = ProviderPlan::new(index, records, [])?;
        let first = plan.public_import_artifact("first")?;
        let second = plan.public_import_artifact("second")?;
        assert_ne!(first.identity, second.identity);
        assert_eq!(first.admitted_root, second.admitted_root);
        assert_eq!(first.identity.feature_projection, BTreeSet::from(["first".to_string()]));
        assert_eq!(
            second.identity.feature_projection,
            BTreeSet::from(["second".to_string()])
        );
        let first_children = plan.public_dependencies(&first.identity)?;
        let second_children = plan.public_dependencies(&second.identity)?;
        assert_eq!(first_children.len(), 1);
        assert_eq!(second_children.len(), 1);
        assert_eq!(first_children[0].descriptor, &edge);
        assert_eq!(second_children[0].descriptor, &edge);
        assert_eq!(first_children[0].target.identity, second_children[0].target.identity);
        assert!(plan.public_artifact_route("first", &second.identity).is_err());
        assert!(plan.public_artifact_route("second", &first.identity).is_err());
        std::fs::rename(&root.crate_root, workspace.path().join("moved"))?;
        for (alias, identity) in [("first", &first.identity), ("second", &second.identity)] {
            let (bound, route) = plan.public_native_union_projection(alias, &native)?;
            assert_eq!(bound.owner, NativeUnionOwnerExport::SelectedArtifact(identity.clone()));
            assert!(route.is_empty());
            assert_eq!(
                plan.public_artifact_route(alias, &first_children[0].target.identity)?,
                ["stock"]
            );
        }
        Ok(())
    }

    /// Preserve the removed CLI reload helper's name, version and byte-integrity refusals in existing admission.
    #[test]
    fn public_graph_admission_refuses_wrong_child_identity_or_bytes() -> TestResult {
        for mismatch in ["name", "version", "digest"] {
            let workspace = tempfile::tempdir()?;
            let child = public_graph_artifact(&workspace.path().join("child"), "catalog", Vec::new())?;
            let mut edge = public_graph_edge("stock", &child, "../child")?;
            match mismatch {
                "name" => edge.provider_name = "other".to_string(),
                "version" => edge.provider_version = "2.0.0".to_string(),
                _ => edge.artifact_digest = crate::generated_source::digest_bytes(b"wrong generation"),
            }
            let root = public_graph_artifact(&workspace.path().join("root"), "root", vec![edge])?;
            let Err(error) = public_graph_plan(&root) else {
                return Err(format!("wrong child {mismatch} must not be admitted").into());
            };
            assert!(error.to_string().contains("expected"), "{error}");
        }
        Ok(())
    }

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
    fn sdk_link_roots_keep_each_semantic_owner_direct() -> TestResult {
        let mut core = record(
            "stdlib-core",
            NamespaceAuthority::SdkReserved,
            &[&["std", "derives", "collection"]],
            true,
            true,
        );
        core.artifact = Some(LibraryArtifactMetadata::from_crate_root(
            "stdlib_core",
            "stdlib-core",
            Path::new("/sdk/stdlib-core"),
        ));
        let mut system = record(
            "stdlib-system",
            NamespaceAuthority::SdkReserved,
            &[&["std", "io"]],
            true,
            true,
        );
        system.artifact = Some(LibraryArtifactMetadata::from_crate_root(
            "stdlib_system",
            "stdlib-system",
            Path::new("/sdk/stdlib-system"),
        ));
        Arc::get_mut(system.manifest.as_mut().ok_or("missing system manifest")?)
            .ok_or("system manifest unexpectedly shared")?
            .contract_metadata
            .provider
            .provider_dependencies
            .push(ProviderDependencyMetadata {
                kind: ProviderDependencyKind::PrivateImplementation,
                dependency_key: "stdlib_core".to_string(),
                provider_name: core.identity.name.clone(),
                provider_version: core.identity.version.clone(),
                artifact_digest: core.identity.digest.clone(),
                relative_artifact_path: "../stdlib-core".to_string(),
                requested_features: BTreeSet::new(),
                default_features: false,
                optional: false,
            });
        let plan = ProviderPlan::new(
            LibraryManifestIndex::default(),
            vec![core, system],
            [path(&["std", "derives", "collection"]), path(&["std", "io"])],
        )?;

        assert_eq!(
            plan.sdk_link_roots()
                .iter()
                .map(|provider| provider.identity.name.as_str())
                .collect::<Vec<_>>(),
            ["stdlib-core", "stdlib-system"]
        );
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
    fn disabled_component_requirements_point_to_the_exact_package_feature_entry() -> TestResult {
        let project = tempfile::tempdir()?;
        let manifest_path = project.path().join("loaf.toml");
        std::fs::write(
            &manifest_path,
            "[project]\nname = \"demo\"\n\n[project.features]\ndefault = []\n\n[project.features.web]\nrequires-sdk-components = [\"stdlib-web\"]\n",
        )?;
        let manifest = ProjectManifest::load(&manifest_path)?;
        let features = PackageFeaturePlan::resolve(&manifest, &super::super::FeatureSelection::new(["web"]))?;
        let components = ResolvedSdkComponents {
            sdk_identity: "incan@0.5.0".to_string(),
            profile: "minimal".to_string(),
            enabled: BTreeSet::new(),
            unavailable: BTreeSet::new(),
            reasons: BTreeMap::new(),
        };

        let error = validate_package_component_requirements(&features, &components)
            .err()
            .ok_or("expected a disabled SDK component requirement")?;

        assert!(
            error.to_string().contains("loaf.toml:8:28"),
            "expected exact package-feature component location, got: {error}"
        );
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

    /// Path rebinding preserves the exact admitted private descriptor and active SDK record.
    #[test]
    fn equivalent_private_sdk_dependency_rebinds_to_active_inventory_root_issue911() -> TestResult {
        let workspace = tempfile::tempdir()?;
        let library_artifact = workspace.path().join("library/target/lib");
        let frozen_sdk_artifact = library_artifact.join("private/stdlib-codecs");
        let active_sdk_artifact = workspace.path().join("active-sdk/stdlib-codecs");
        std::fs::create_dir_all(&active_sdk_artifact)?;
        let digest = format!("sha256:{}", "a".repeat(64));
        let features = BTreeSet::from(["json".to_string()]);
        let records = vec![
            compiled_library_record(&library_artifact, &digest, features.clone()),
            active_sdk_record(&active_sdk_artifact, &digest, features),
        ];
        let library_identity = records.first().ok_or("library record absent")?.identity.clone();

        let plan = ProviderPlan::new(LibraryManifestIndex::default(), records, [])?;
        let rebindings = plan.sdk_dependency_rebindings();

        assert_eq!(rebindings.len(), 1);
        assert_eq!(rebindings[0].source_crate_root, frozen_sdk_artifact);
        assert!(
            !rebindings[0].source_crate_root.exists(),
            "the historical cache root must not be required for logical rebinding"
        );
        assert_eq!(
            rebindings[0].active_crate_root,
            std::fs::canonicalize(active_sdk_artifact)?
        );
        assert_eq!(rebindings[0].provider_name, "incan_stdlib_codecs");
        assert_eq!(plan.sdk_artifact_projections().len(), 1);
        let private = plan.private_sdk_dependencies(&library_identity)?;
        assert_eq!(private.len(), 1);
        assert_eq!(private[0].descriptor_index, 0);
        assert_eq!(private[0].descriptor.dependency_key, "incan_stdlib_codecs");
        let selected = private[0].target.ok_or("private SDK target missing")?;
        assert_eq!(selected.identity.digest, digest);
        assert_eq!(
            selected.identity.feature_projection,
            BTreeSet::from(["json".to_string()])
        );
        assert_eq!(
            plan.sdk_link_roots()
                .iter()
                .map(|provider| provider.identity.name.as_str())
                .collect::<Vec<_>>(),
            ["incan_stdlib_codecs"],
            "a compiled dependency's private SDK edge must remain directly linkable for compiler-projected paths"
        );
        Ok(())
    }

    /// Equal paths retain two distinct descriptor slots and one exact SDK owner without any rebinding row.
    #[test]
    fn private_sdk_associations_survive_equal_paths_and_source_relocation() -> TestResult {
        let workspace = tempfile::tempdir()?;
        let live = workspace.path().join("live");
        let library_root = live.join("library");
        let sdk_root = library_root.join("private/stdlib-codecs");
        std::fs::create_dir_all(&sdk_root)?;
        let digest = format!("sha256:{}", "a".repeat(64));
        let features = BTreeSet::from(["json".to_string()]);
        let mut library = compiled_library_record(&library_root, &digest, features.clone());
        let manifest = Arc::make_mut(library.manifest.as_mut().ok_or("library manifest absent")?);
        let mut second = manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .first()
            .ok_or("private descriptor absent")?
            .clone();
        second.dependency_key = "codec_alias".to_string();
        manifest.contract_metadata.provider.provider_dependencies.push(second);
        let library_identity = library.identity.clone();
        let plan = ProviderPlan::new(
            LibraryManifestIndex::default(),
            vec![library, active_sdk_record(&sdk_root, &digest, features)],
            [],
        )?;
        let library = plan.public_artifact(&library_identity)?;
        assert!(plan.sdk_dependency_rebindings().is_empty());
        let edges = plan.private_sdk_dependencies(&library.identity)?;
        assert_eq!(edges.len(), 2);
        assert_eq!(
            edges.iter().map(|edge| edge.descriptor_index).collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(edges[1].descriptor.dependency_key, "codec_alias");
        let first = edges[0].target.ok_or("first target absent")?;
        let second = edges[1].target.ok_or("second target absent")?;
        assert!(std::ptr::eq(first, second));
        std::fs::rename(&live, workspace.path().join("moved"))?;
        assert!(!live.exists());
        let after = plan.private_sdk_dependencies(&library.identity)?;
        assert!(std::ptr::eq(first, after[0].target.ok_or("retained target absent")?));
        assert!(plan.public_dependencies(&library.identity)?.is_empty());
        Ok(())
    }

    /// Missing or changed retained associations refuse; an exact parent lookup never falls back to a name.
    #[test]
    fn private_sdk_associations_refuse_broken_record_coordinates() -> TestResult {
        let workspace = tempfile::tempdir()?;
        let sdk_root = workspace.path().join("sdk");
        std::fs::create_dir_all(&sdk_root)?;
        let digest = format!("sha256:{}", "a".repeat(64));
        let library = compiled_library_record(&workspace.path().join("library"), &digest, BTreeSet::new());
        let identity = library.identity.clone();
        let plan = ProviderPlan::new(
            LibraryManifestIndex::default(),
            vec![library, active_sdk_record(&sdk_root, &digest, BTreeSet::new())],
            [],
        )?;
        let parent = plan.public_artifact(&identity)?;
        let root = parent.admitted_root.clone();
        let mut wrong_identity = identity.clone();
        wrong_identity.feature_projection.insert("unselected".to_string());
        assert!(plan.private_sdk_dependencies(&wrong_identity).is_err());
        for broken_index in [true, false] {
            let mut changed = plan.clone();
            let edge = changed
                .private_sdk_dependencies
                .get_mut(&root)
                .and_then(|edges| edges.first_mut())
                .ok_or("retained edge absent")?;
            if broken_index {
                edge.descriptor_index = usize::MAX;
            } else {
                edge.target_key = Some(identity.stable_key());
            }
            assert!(changed.private_sdk_dependencies(&identity).is_err());
        }
        let mut missing = plan.clone();
        missing.private_sdk_dependencies.remove(&root);
        assert!(missing.private_sdk_dependencies(&identity).is_err());
        let mut changed = plan;
        let target_key = changed
            .private_sdk_dependencies
            .get(&root)
            .and_then(|edges| edges.first())
            .and_then(|edge| edge.target_key.clone())
            .ok_or("target key absent")?;
        changed
            .records
            .get_mut(&target_key)
            .ok_or("selected target absent")?
            .identity
            .feature_projection
            .insert("changed".to_string());
        assert!(changed.private_sdk_dependencies(&identity).is_err());
        Ok(())
    }

    /// Capturing associations does not weaken the original SDK identity, availability or request admission checks.
    #[test]
    fn private_sdk_associations_preserve_admission_refusals() -> TestResult {
        let workspace = tempfile::tempdir()?;
        let sdk_root = workspace.path().join("sdk");
        std::fs::create_dir_all(&sdk_root)?;
        let digest = format!("sha256:{}", "a".repeat(64));
        for case in [
            "disabled",
            "unavailable",
            "digest",
            "version",
            "features",
            "defaults",
            "optional",
        ] {
            let mut library = compiled_library_record(&workspace.path().join("library"), &digest, BTreeSet::new());
            let mut sdk = active_sdk_record(&sdk_root, &digest, BTreeSet::new());
            match case {
                "disabled" => sdk.enabled = false,
                "unavailable" => sdk.available = false,
                "digest" => sdk.identity.digest = format!("sha256:{}", "b".repeat(64)),
                "version" => sdk.identity.version = "9.0.0".to_string(),
                "features" => {
                    sdk.identity.feature_projection.insert("unrequested".to_string());
                }
                _ => {
                    let manifest = Arc::make_mut(library.manifest.as_mut().ok_or("library manifest absent")?);
                    let edge = manifest
                        .contract_metadata
                        .provider
                        .provider_dependencies
                        .first_mut()
                        .ok_or("private descriptor absent")?;
                    edge.default_features = case == "defaults";
                    edge.optional = case == "optional";
                }
            }
            assert!(
                matches!(
                    ProviderPlan::new(LibraryManifestIndex::default(), vec![library, sdk], []),
                    Err(ProviderPlanError::IncompatibleCompiledSdkDependency { .. })
                ),
                "{case}"
            );
        }
        Ok(())
    }

    /// Transitive private associations retain their declaring child alongside ancestor path projections.
    #[test]
    fn transitive_private_sdk_rebinding_projects_every_compiled_ancestor_issue911() -> TestResult {
        let workspace = tempfile::tempdir()?;
        let root_artifact = workspace.path().join("root");
        let child_artifact = workspace.path().join("child");
        let active_sdk_artifact = workspace.path().join("active-sdk/runtime");
        let absent_sdk_artifact = workspace.path().join("old-sdk/runtime");
        for artifact in [&root_artifact, &child_artifact, &active_sdk_artifact] {
            std::fs::create_dir_all(artifact.join("src"))?;
            std::fs::write(
                artifact.join("Cargo.toml"),
                "[package]\nname = \"placeholder\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            )?;
            std::fs::write(artifact.join("src/lib.rs"), "pub fn marker() {}\n")?;
        }
        std::fs::write(
            child_artifact.join("Cargo.toml"),
            "[package]\nname = \"child\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        let sdk_digest = digest_provider_artifact(&active_sdk_artifact)?;
        let mut child_manifest = LibraryManifest::new("child", "0.1.0");
        child_manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .push(ProviderDependencyMetadata {
                kind: ProviderDependencyKind::PrivateImplementation,
                dependency_key: "incan_stdlib_codecs".to_string(),
                provider_name: "incan_stdlib_codecs".to_string(),
                provider_version: "0.5.0".to_string(),
                artifact_digest: sdk_digest.clone(),
                relative_artifact_path: "../old-sdk/runtime".to_string(),
                requested_features: BTreeSet::new(),
                default_features: false,
                optional: false,
            });
        let child_manifest_path = child_artifact.join("child.incnlib");
        child_manifest.write_to_path(&child_manifest_path)?;
        let child_digest = digest_provider_artifact(&child_artifact)?;
        let mut root_manifest = LibraryManifest::new("root", "0.1.0");
        root_manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .push(ProviderDependencyMetadata {
                kind: ProviderDependencyKind::PublicPackage,
                dependency_key: "child".to_string(),
                provider_name: "child".to_string(),
                provider_version: "0.1.0".to_string(),
                artifact_digest: child_digest,
                relative_artifact_path: "../child".to_string(),
                requested_features: BTreeSet::new(),
                default_features: false,
                optional: false,
            });
        let root_record = ProviderRecord {
            identity: ProviderIdentity {
                name: "root".to_string(),
                version: "0.1.0".to_string(),
                digest: digest_provider_artifact(&root_artifact)?,
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::ProjectDependency {
                dependency_key: "root".to_string(),
                manifest_path: root_artifact.join("root.incnlib"),
            },
            authority: NamespaceAuthority::ProjectDependency {
                dependency_key: "root".to_string(),
            },
            namespace_claims: BTreeSet::new(),
            available: true,
            enabled: true,
            manifest: Some(Arc::new(root_manifest)),
            artifact: Some(LibraryArtifactMetadata::from_crate_root("root", "root", &root_artifact)),
            implementation_facets: Vec::new(),
        };
        let root_identity = root_record.identity.clone();
        let plan = ProviderPlan::new(
            LibraryManifestIndex::default(),
            vec![
                root_record,
                active_sdk_record(&active_sdk_artifact, &sdk_digest, BTreeSet::new()),
            ],
            [],
        )?;

        assert_eq!(plan.sdk_dependency_rebindings().len(), 1);
        assert_eq!(plan.sdk_artifact_projections().len(), 2);
        let root = plan.public_artifact(&root_identity)?;
        assert!(plan.private_sdk_dependencies(&root.identity)?.is_empty());
        let public = plan.public_dependencies(&root.identity)?;
        let child = public.first().ok_or("public child absent")?.target;
        let private = plan.private_sdk_dependencies(&child.identity)?;
        assert_eq!(private.len(), 1);
        assert_eq!(private[0].descriptor_index, 0);
        assert_eq!(
            private[0].target.ok_or("child SDK owner absent")?.identity.digest,
            sdk_digest
        );
        assert!(plan.sdk_artifact_projections().iter().any(|projection| {
            normalize_artifact_root(&projection.artifact.crate_root) == normalize_artifact_root(&root_artifact)
        }));
        assert!(plan.sdk_artifact_projections().iter().any(|projection| {
            normalize_artifact_root(&projection.artifact.crate_root) == normalize_artifact_root(&child_artifact)
        }));
        assert!(!absent_sdk_artifact.exists());
        Ok(())
    }

    /// Inactive roots do not admit public executable artifacts or touch unavailable transitive files.
    #[test]
    fn inactive_providers_do_not_expand_the_public_artifact_catalog() -> TestResult {
        let root = tempfile::tempdir()?;
        let mut record = compiled_library_record(root.path(), "unused-sdk-digest", BTreeSet::new());
        let manifest = Arc::make_mut(record.manifest.as_mut().ok_or("test manifest absent")?);
        let dependency = manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .first_mut()
            .ok_or("test edge absent")?;
        dependency.kind = ProviderDependencyKind::PublicPackage;
        dependency.relative_artifact_path = "missing-public-artifact".into();
        record.enabled = false;
        let disabled = ProviderPlan::new(LibraryManifestIndex::default(), vec![record.clone()], [])?;
        assert_eq!(disabled.public_artifacts().count(), 0);
        record.enabled = true;
        record.available = false;
        let unavailable = ProviderPlan::new(LibraryManifestIndex::default(), vec![record], [])?;
        assert_eq!(unavailable.public_artifacts().count(), 0);
        Ok(())
    }

    #[test]
    fn incompatible_private_sdk_dependency_fails_before_cargo_issue911() -> TestResult {
        let workspace = tempfile::tempdir()?;
        let library_artifact = workspace.path().join("library/target/lib");
        let frozen_sdk_artifact = library_artifact.join("private/stdlib-codecs");
        let active_sdk_artifact = workspace.path().join("active-sdk/stdlib-codecs");
        std::fs::create_dir_all(&frozen_sdk_artifact)?;
        std::fs::create_dir_all(&active_sdk_artifact)?;
        let frozen_digest = format!("sha256:{}", "a".repeat(64));
        let active_digest = format!("sha256:{}", "b".repeat(64));
        let features = BTreeSet::from(["json".to_string()]);
        let records = vec![
            compiled_library_record(&library_artifact, &frozen_digest, features.clone()),
            active_sdk_record(&active_sdk_artifact, &active_digest, features),
        ];

        let error = ProviderPlan::new(LibraryManifestIndex::default(), records, [])
            .err()
            .ok_or("expected incompatible compiled SDK dependency")?;

        assert!(matches!(
            error,
            ProviderPlanError::IncompatibleCompiledSdkDependency { .. }
        ));
        assert!(error.to_string().contains("incan_stdlib_codecs"));
        assert!(error.to_string().contains("rebuild the compiled library"));
        Ok(())
    }

    fn compiled_library_record(
        artifact_root: &Path,
        sdk_digest: &str,
        sdk_features: BTreeSet<String>,
    ) -> ProviderRecord {
        let mut manifest = LibraryManifest::new("root_lib", "0.1.0");
        manifest.contract_metadata.provider.provider_dependencies.push(
            crate::library_manifest::ProviderDependencyMetadata {
                kind: crate::library_manifest::ProviderDependencyKind::PrivateImplementation,
                dependency_key: "incan_stdlib_codecs".to_string(),
                provider_name: "incan_stdlib_codecs".to_string(),
                provider_version: "0.5.0".to_string(),
                artifact_digest: sdk_digest.to_string(),
                relative_artifact_path: "private/stdlib-codecs".to_string(),
                requested_features: sdk_features,
                default_features: false,
                optional: false,
            },
        );
        ProviderRecord {
            identity: ProviderIdentity {
                name: "root_lib".to_string(),
                version: "0.1.0".to_string(),
                digest: format!("sha256:{}", "c".repeat(64)),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::ProjectDependency {
                dependency_key: "root_lib".to_string(),
                manifest_path: artifact_root.join("root_lib.incnlib"),
            },
            authority: NamespaceAuthority::ProjectDependency {
                dependency_key: "root_lib".to_string(),
            },
            namespace_claims: BTreeSet::new(),
            available: true,
            enabled: true,
            manifest: Some(Arc::new(manifest)),
            artifact: Some(LibraryArtifactMetadata::from_crate_root(
                "root_lib",
                "root_lib",
                artifact_root,
            )),
            implementation_facets: Vec::new(),
        }
    }

    fn active_sdk_record(artifact_root: &Path, digest: &str, feature_projection: BTreeSet<String>) -> ProviderRecord {
        ProviderRecord {
            identity: ProviderIdentity {
                name: "incan_stdlib_codecs".to_string(),
                version: "0.5.0".to_string(),
                digest: digest.to_string(),
                feature_projection,
            },
            provenance: ProviderProvenance::Sdk {
                sdk_identity: "incan@0.5.0".to_string(),
                component_id: "stdlib-codecs".to_string(),
                inventory_path: None,
            },
            authority: NamespaceAuthority::SdkReserved,
            namespace_claims: BTreeSet::new(),
            available: true,
            enabled: true,
            manifest: Some(Arc::new(LibraryManifest::new("incan_stdlib_codecs", "0.5.0"))),
            artifact: Some(LibraryArtifactMetadata::from_crate_root(
                "incan_stdlib_codecs",
                "incan_stdlib_codecs",
                artifact_root,
            )),
            implementation_facets: Vec::new(),
        }
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
            provider_codegen_revision: crate::version::SDK_PROVIDER_CODEGEN_REVISION,
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
