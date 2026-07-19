//! Typed additive package-feature graph from RFC 114.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use incan_core::lang::conventions::validate_package_feature_identifier;
use serde::Serialize;

use crate::frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry, load_provider_dependency_artifact,
};
use crate::library_manifest::{
    LibraryManifest, ProviderDependencyKind, ProviderDependencyMetadata, ProviderFeatureMetadata,
    digest_provider_artifact,
};
use crate::manifest::{ExpandedProjectFeature, MANIFEST_FILENAME, ProjectFeatureDefinition, ProjectManifest};

use super::SdkInventory;

/// Root feature selection supplied by the project manifest and current command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeatureSelection {
    /// Explicit root-package features requested by the command or environment.
    pub requested: BTreeSet<String>,
    /// Suppress the package's conventional `default` feature root.
    pub no_default_features: bool,
    /// Select every feature declared by the root package.
    pub all_features: bool,
}

impl FeatureSelection {
    /// Build a selection from explicit feature names while retaining default features.
    pub fn new<I, S>(requested: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            requested: requested.into_iter().map(Into::into).collect(),
            no_default_features: false,
            all_features: false,
        }
    }
}

/// Why one package feature entered the active closure.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", content = "source", rename_all = "snake_case")]
pub enum FeatureActivationReason {
    /// Conventional package default.
    Default,
    /// Explicit command or environment request.
    Requested,
    /// `--all-features` selected every declaration.
    AllFeatures,
    /// Another local package feature includes this feature.
    IncludedBy(String),
    /// One parent package dependency edge requested this feature.
    DependencyRequest {
        /// Package containing the dependency declaration.
        package: String,
        /// Dependency key used by the requesting package.
        dependency: String,
    },
}

/// Deterministic active projection for one package.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ResolvedPackageFeatures {
    /// Active local package features after additive closure.
    pub active_features: BTreeSet<String>,
    /// Optional Incan dependencies activated by the feature closure.
    pub active_optional_dependencies: BTreeSet<String>,
    /// Public feature requests grouped by active Incan dependency key.
    pub dependency_features: BTreeMap<String, BTreeSet<String>>,
    /// SDK components that must already be enabled and available.
    pub required_sdk_components: BTreeSet<String>,
    /// Stable activation reasons for inspection and lock provenance.
    pub reasons: BTreeMap<String, BTreeSet<FeatureActivationReason>>,
}

/// Validation or resolution failure in one package-owned feature graph.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FeatureGraphError {
    /// A feature, dependency, component, or dependency-feature spelling is malformed.
    #[error("invalid feature member `{member}` in feature `{feature}`: {message}")]
    InvalidMember {
        /// Feature containing the invalid member.
        feature: String,
        /// Authored compact member or expanded field value.
        member: String,
        /// Validation detail.
        message: String,
    },
    /// A local feature edge or root request names no declared feature.
    #[error("unknown package feature `{feature}`{context}")]
    UnknownFeature {
        /// Missing local feature.
        feature: String,
        /// Optional edge context formatted for diagnostics.
        context: String,
    },
    /// A feature edge names no declared Incan dependency.
    #[error("feature `{feature}` references unknown Incan dependency `{dependency}`")]
    UnknownDependency {
        /// Feature containing the edge.
        feature: String,
        /// Missing dependency key.
        dependency: String,
    },
    /// `dep:<name>` or an expanded optional-dependency edge targets a non-optional dependency.
    #[error("feature `{feature}` activates dependency `{dependency}`, but that dependency is not optional")]
    DependencyNotOptional {
        /// Feature containing the edge.
        feature: String,
        /// Non-optional dependency key.
        dependency: String,
    },
    /// A feature closure requests a dependency feature without activating the optional dependency.
    #[error(
        "feature `{feature}` requests `{dependency}/{dependency_feature}`, but optional dependency `{dependency}` is not active"
    )]
    InactiveOptionalDependency {
        /// Feature containing the edge.
        feature: String,
        /// Inactive optional dependency key.
        dependency: String,
        /// Requested dependency-owned feature.
        dependency_feature: String,
    },
    /// The local include graph contains a cycle.
    #[error("package feature cycle: {path}")]
    Cycle {
        /// Stable arrow-separated cycle path.
        path: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct NormalizedFeatureDefinition {
    local_features: BTreeSet<String>,
    optional_dependencies: BTreeSet<String>,
    dependency_features: BTreeMap<String, BTreeSet<String>>,
    required_sdk_components: BTreeSet<String>,
}

/// Validated package-owned feature graph independent of any one command selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageFeatureGraph {
    definitions: BTreeMap<String, NormalizedFeatureDefinition>,
    optional_dependencies: BTreeSet<String>,
}

/// Resolved feature state for one concrete package instance in the dependency graph.
#[derive(Debug, Clone)]
pub struct ResolvedPackageFeatureState {
    /// Human-readable package name from `[project].name` or the project directory.
    pub package_name: String,
    /// Canonical or manifest-resolved project root used for graph identity.
    pub project_root: PathBuf,
    /// Exact source manifest or checked provider artifact that defined this package feature graph.
    pub feature_manifest_path: PathBuf,
    /// Parsed project manifest when source is available; installed providers resolve from their checked artifact.
    pub manifest: Option<ProjectManifest>,
    /// Active feature and optional-dependency projection.
    pub features: ResolvedPackageFeatures,
    /// Incan dependency keys active after optional-dependency resolution.
    pub active_dependencies: BTreeSet<String>,
}

/// One active Incan dependency edge and the feature request it contributes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ResolvedFeatureDependencyEdge {
    /// Project root of the requesting package.
    pub from: PathBuf,
    /// Dependency key authored by the requesting package.
    pub dependency_key: String,
    /// Project root of the requested package instance.
    pub to: PathBuf,
    /// Public features requested on this edge.
    pub requested_features: BTreeSet<String>,
    /// Whether this edge enables the dependency's `default` feature.
    pub default_features: bool,
    /// Whether a package feature activated an optional dependency edge.
    pub optional: bool,
}

/// Unified additive feature projection across one root package and all active path dependencies.
#[derive(Debug, Clone)]
pub struct PackageFeaturePlan {
    root: PathBuf,
    packages: BTreeMap<PathBuf, ResolvedPackageFeatureState>,
    edges: BTreeMap<(PathBuf, String, PathBuf), ResolvedFeatureDependencyEdge>,
}

/// Cross-package feature resolution failure with package and manifest provenance.
#[derive(Debug, thiserror::Error)]
pub enum PackageFeaturePlanError {
    /// Reading an exact dependency manifest failed.
    #[error("failed to read feature dependency manifest {path}: {source}")]
    ManifestRead {
        /// Exact dependency manifest path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// Parsing an exact dependency manifest failed.
    #[error("failed to parse feature dependency manifest {path}: {message}")]
    ManifestParse {
        /// Exact dependency manifest path.
        path: PathBuf,
        /// Source-anchored manifest error text.
        message: String,
    },
    /// One package-owned feature graph is invalid for the requested projection.
    #[error("package `{package}` feature resolution failed in {manifest_path}{location}: {source}")]
    PackageGraph {
        /// Package name used in diagnostics and inspection.
        package: String,
        /// Exact manifest containing the invalid feature declaration or request.
        manifest_path: PathBuf,
        /// Best-effort exact authored feature-member location, formatted as `:line:column`.
        location: String,
        /// Typed package-owned graph failure.
        source: Box<FeatureGraphError>,
    },
    /// An artifact-only provider cannot satisfy a different physical feature projection without producer source.
    #[error(
        "compiled provider `{package}` at {manifest_path} was built with package features [{built}], but this consumer requires [{requested}]"
    )]
    ArtifactProjectionMismatch {
        /// Provider package name.
        package: String,
        /// Exact checked artifact manifest path.
        manifest_path: PathBuf,
        /// Stable comma-separated physical projection.
        built: String,
        /// Stable comma-separated requested projection.
        requested: String,
    },
    /// A transitive artifact dependency is missing, corrupt, or disagrees with its checked descriptor.
    #[error("compiled provider `{package}` dependency `{dependency}` at {path} is invalid: {message}")]
    ProviderDependencyArtifact {
        /// Provider that owns the dependency edge.
        package: String,
        /// Provider-local dependency key.
        dependency: String,
        /// Resolved dependency artifact path.
        path: PathBuf,
        /// Exact load, identity, projection, or integrity failure.
        message: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PackageFeatureRequest {
    requested: BTreeSet<String>,
    reasons: BTreeMap<String, BTreeSet<FeatureActivationReason>>,
    enable_default: bool,
    all_features: bool,
}

#[derive(Debug, Clone)]
struct CompiledFeaturePackage {
    manifest: LibraryManifest,
    artifact: LibraryArtifactMetadata,
}

impl PackageFeaturePlan {
    /// Resolve the root feature selection and unify every active path dependency request by concrete package root.
    pub fn resolve(
        root_manifest: &ProjectManifest,
        root_selection: &FeatureSelection,
    ) -> Result<Self, PackageFeaturePlanError> {
        Self::resolve_with_sdk_inventory(root_manifest, root_selection, None)
    }

    /// Resolve package features while rebasing frozen private SDK dependencies onto the active inventory.
    ///
    /// Private implementation paths in compiled artifacts are physical coordinates, not semantic package identities.
    /// Feature planning must therefore select an equivalent current SDK artifact before attempting filesystem access;
    /// an older content-addressed cache generation may already have been collected.
    pub fn resolve_with_sdk_inventory(
        root_manifest: &ProjectManifest,
        root_selection: &FeatureSelection,
        sdk_inventory: Option<&SdkInventory>,
    ) -> Result<Self, PackageFeaturePlanError> {
        let root = normalize_project_root(root_manifest.project_root());
        let mut manifests = BTreeMap::from([(root.clone(), root_manifest.clone())]);
        let mut compiled_packages = BTreeMap::<PathBuf, CompiledFeaturePackage>::new();
        let mut requests = BTreeMap::from([(
            root.clone(),
            PackageFeatureRequest {
                requested: root_selection.requested.clone(),
                reasons: root_selection
                    .requested
                    .iter()
                    .map(|feature| (feature.clone(), BTreeSet::from([FeatureActivationReason::Requested])))
                    .collect(),
                enable_default: !root_selection.no_default_features,
                all_features: root_selection.all_features,
            },
        )]);
        let mut processed = BTreeMap::new();
        let mut pending = vec![root.clone()];
        let mut packages = BTreeMap::new();
        let mut edges = BTreeMap::new();

        while let Some(project_root) = pending.pop() {
            let Some(request) = requests.get(&project_root).cloned() else {
                continue;
            };
            if processed.get(&project_root) == Some(&request) {
                continue;
            }
            processed.insert(project_root.clone(), request.clone());

            let manifest = match manifests.get(&project_root) {
                Some(manifest) => Some(manifest.clone()),
                None if project_root.join(MANIFEST_FILENAME).is_file() => {
                    let loaded = read_exact_project_manifest(&project_root)?;
                    manifests.insert(project_root.clone(), loaded.clone());
                    Some(loaded)
                }
                None => None,
            };
            let Some(manifest) = manifest else {
                let Some(compiled_package) = compiled_packages.get(&project_root).cloned() else {
                    continue;
                };
                let compiled_manifest = &compiled_package.manifest;
                let artifact_path = &compiled_package.artifact.manifest_path;
                let package_name = compiled_manifest.name.clone();
                let graph = PackageFeatureGraph::from_provider_metadata(
                    &compiled_manifest.contract_metadata.provider.public_features,
                )
                .map_err(|source| PackageFeaturePlanError::PackageGraph {
                    package: package_name.clone(),
                    manifest_path: artifact_path.clone(),
                    location: String::new(),
                    source: Box::new(source),
                })?;
                let selection = FeatureSelection {
                    requested: request.requested.clone(),
                    no_default_features: !request.enable_default,
                    all_features: request.all_features,
                };
                let mut resolved =
                    graph
                        .resolve(&selection)
                        .map_err(|source| PackageFeaturePlanError::PackageGraph {
                            package: package_name.clone(),
                            manifest_path: artifact_path.clone(),
                            location: String::new(),
                            source: Box::new(source),
                        })?;
                for feature in &selection.requested {
                    if let Some(reasons) = request.reasons.get(feature) {
                        resolved.reasons.insert(feature.clone(), reasons.clone());
                    }
                }
                resolved.required_sdk_components.extend(
                    compiled_manifest
                        .contract_metadata
                        .provider
                        .required_sdk_components
                        .iter()
                        .cloned(),
                );
                if resolved.active_features != compiled_manifest.contract_metadata.provider.active_features {
                    return Err(PackageFeaturePlanError::ArtifactProjectionMismatch {
                        package: package_name,
                        manifest_path: artifact_path.clone(),
                        built: render_features(&compiled_manifest.contract_metadata.provider.active_features),
                        requested: render_features(&resolved.active_features),
                    });
                }
                let dependencies = &compiled_manifest.contract_metadata.provider.provider_dependencies;
                let active_dependencies = dependencies
                    .iter()
                    .filter(|dependency| dependency.kind == ProviderDependencyKind::PublicPackage)
                    .map(|dependency| dependency.dependency_key.clone())
                    .collect::<BTreeSet<_>>();
                for dependency in &resolved.active_optional_dependencies {
                    if !active_dependencies.contains(dependency) {
                        return Err(PackageFeaturePlanError::ProviderDependencyArtifact {
                            package: package_name.clone(),
                            dependency: dependency.clone(),
                            path: artifact_path.clone(),
                            message: "the active optional dependency is absent from the artifact projection"
                                .to_string(),
                        });
                    }
                }
                for dependency in resolved.dependency_features.keys() {
                    if !active_dependencies.contains(dependency) {
                        return Err(PackageFeaturePlanError::ProviderDependencyArtifact {
                            package: package_name.clone(),
                            dependency: dependency.clone(),
                            path: artifact_path.clone(),
                            message: "the active dependency feature request is absent from the artifact projection"
                                .to_string(),
                        });
                    }
                }
                packages.insert(
                    project_root.clone(),
                    ResolvedPackageFeatureState {
                        package_name: package_name.clone(),
                        project_root: project_root.clone(),
                        feature_manifest_path: artifact_path.clone(),
                        manifest: None,
                        features: resolved,
                        active_dependencies,
                    },
                );

                for dependency in dependencies {
                    let frozen_artifact_root = compiled_package
                        .artifact
                        .crate_root
                        .join(&dependency.relative_artifact_path);
                    let dependency_artifact_root = if dependency.kind == ProviderDependencyKind::PrivateImplementation {
                        active_sdk_dependency_root(
                            sdk_inventory,
                            &package_name,
                            artifact_path,
                            dependency,
                            &frozen_artifact_root,
                        )?
                    } else {
                        frozen_artifact_root
                    };
                    let entry =
                        load_provider_dependency_artifact(&dependency.dependency_key, &dependency_artifact_root);
                    let (dependency_manifest, dependency_artifact) = match entry {
                        LibraryManifestIndexEntry::Loaded { manifest, metadata } => (*manifest, metadata),
                        LibraryManifestIndexEntry::Failed(failure) => {
                            return Err(PackageFeaturePlanError::ProviderDependencyArtifact {
                                package: package_name.clone(),
                                dependency: dependency.dependency_key.clone(),
                                path: failure.path,
                                message: failure.message,
                            });
                        }
                    };
                    validate_provider_dependency_descriptor(
                        &package_name,
                        dependency,
                        &dependency_manifest,
                        &dependency_artifact,
                    )?;
                    if dependency.kind == ProviderDependencyKind::PrivateImplementation {
                        continue;
                    }
                    let dependency_root = dependency_artifact.crate_root.clone();
                    edges.insert(
                        (
                            project_root.clone(),
                            dependency.dependency_key.clone(),
                            dependency_root.clone(),
                        ),
                        ResolvedFeatureDependencyEdge {
                            from: project_root.clone(),
                            dependency_key: dependency.dependency_key.clone(),
                            to: dependency_root.clone(),
                            requested_features: dependency.requested_features.clone(),
                            default_features: dependency.default_features,
                            optional: dependency.optional,
                        },
                    );
                    let dependency_request = requests.entry(dependency_root.clone()).or_default();
                    let previous = dependency_request.clone();
                    for feature in &dependency.requested_features {
                        dependency_request.requested.insert(feature.clone());
                        dependency_request.reasons.entry(feature.clone()).or_default().insert(
                            FeatureActivationReason::DependencyRequest {
                                package: package_name.clone(),
                                dependency: dependency.dependency_key.clone(),
                            },
                        );
                    }
                    dependency_request.enable_default |= dependency.default_features;
                    compiled_packages.insert(
                        dependency_root.clone(),
                        CompiledFeaturePackage {
                            manifest: dependency_manifest,
                            artifact: dependency_artifact,
                        },
                    );
                    if dependency_request != &previous || !processed.contains_key(&dependency_root) {
                        pending.push(dependency_root);
                    }
                }
                continue;
            };
            let package_name = manifest_package_name(&manifest, &project_root);
            let graph = PackageFeatureGraph::from_manifest(&manifest).map_err(|source| {
                let location = feature_error_location(manifest.path(), &source);
                PackageFeaturePlanError::PackageGraph {
                    package: package_name.clone(),
                    manifest_path: manifest.path().to_path_buf(),
                    location,
                    source: Box::new(source),
                }
            })?;
            let selection = FeatureSelection {
                requested: request.requested.clone(),
                no_default_features: !request.enable_default,
                all_features: request.all_features,
            };
            let mut resolved = graph.resolve(&selection).map_err(|source| {
                let location = feature_error_location(manifest.path(), &source);
                PackageFeaturePlanError::PackageGraph {
                    package: package_name.clone(),
                    manifest_path: manifest.path().to_path_buf(),
                    location,
                    source: Box::new(source),
                }
            })?;
            for feature in &selection.requested {
                if let Some(reasons) = request.reasons.get(feature) {
                    resolved.reasons.insert(feature.clone(), reasons.clone());
                }
            }
            let active_dependencies = manifest
                .library_dependencies()
                .iter()
                .filter(|(dependency_key, dependency)| {
                    !dependency.optional || resolved.active_optional_dependencies.contains(*dependency_key)
                })
                .map(|(dependency_key, _)| dependency_key.clone())
                .collect::<BTreeSet<_>>();

            packages.insert(
                project_root.clone(),
                ResolvedPackageFeatureState {
                    package_name: package_name.clone(),
                    project_root: project_root.clone(),
                    feature_manifest_path: manifest.path().to_path_buf(),
                    manifest: Some(manifest.clone()),
                    features: resolved.clone(),
                    active_dependencies: active_dependencies.clone(),
                },
            );

            for dependency_key in active_dependencies {
                let Some(dependency) = manifest.library_dependencies().get(&dependency_key) else {
                    continue;
                };
                let dependency_root = normalize_project_root(&dependency.path);
                let mut requested_features = dependency.features.iter().cloned().collect::<BTreeSet<_>>();
                if let Some(conditioned) = resolved.dependency_features.get(&dependency_key) {
                    requested_features.extend(conditioned.iter().cloned());
                }
                edges.insert(
                    (project_root.clone(), dependency_key.clone(), dependency_root.clone()),
                    ResolvedFeatureDependencyEdge {
                        from: project_root.clone(),
                        dependency_key: dependency_key.clone(),
                        to: dependency_root.clone(),
                        requested_features: requested_features.clone(),
                        default_features: dependency.default_features,
                        optional: dependency.optional,
                    },
                );

                let dependency_request = requests.entry(dependency_root.clone()).or_default();
                let previous = dependency_request.clone();
                for feature in requested_features {
                    dependency_request.requested.insert(feature.clone());
                    dependency_request.reasons.entry(feature).or_default().insert(
                        FeatureActivationReason::DependencyRequest {
                            package: package_name.clone(),
                            dependency: dependency_key.clone(),
                        },
                    );
                }
                dependency_request.enable_default |= dependency.default_features;
                if dependency_request != &previous || !processed.contains_key(&dependency_root) {
                    if !dependency_root.join(MANIFEST_FILENAME).is_file() {
                        let index = LibraryManifestIndex::from_project_manifest_dependencies(
                            &manifest,
                            std::iter::once(dependency_key.as_str()),
                        );
                        if let Some(LibraryManifestIndexEntry::Loaded { manifest, metadata }) =
                            index.get(&dependency_key)
                        {
                            compiled_packages.insert(
                                dependency_root.clone(),
                                CompiledFeaturePackage {
                                    manifest: manifest.as_ref().clone(),
                                    artifact: metadata.clone(),
                                },
                            );
                        }
                    }
                    pending.push(dependency_root);
                }
            }
        }

        Ok(Self { root, packages, edges })
    }

    /// Root package identity used by this feature plan.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Iterate over resolved package projections in deterministic project-root order.
    pub fn packages(&self) -> impl Iterator<Item = &ResolvedPackageFeatureState> {
        self.packages.values()
    }

    /// Return one resolved package projection by project root.
    pub fn package(&self, project_root: &Path) -> Option<&ResolvedPackageFeatureState> {
        self.packages.get(&normalize_project_root(project_root))
    }

    /// Iterate over active dependency edges in deterministic identity order.
    pub fn edges(&self) -> impl Iterator<Item = &ResolvedFeatureDependencyEdge> {
        self.edges.values()
    }

    /// Return the resolved root-package projection.
    pub fn root_package(&self) -> Option<&ResolvedPackageFeatureState> {
        self.packages.get(&self.root)
    }
}

/// Select an active-inventory crate root for one frozen private SDK edge before reading its historical cache path.
fn active_sdk_dependency_root(
    inventory: Option<&SdkInventory>,
    package_name: &str,
    package_manifest_path: &Path,
    dependency: &ProviderDependencyMetadata,
    frozen_artifact_root: &Path,
) -> Result<PathBuf, PackageFeaturePlanError> {
    let Some(inventory) = inventory else {
        return Ok(frozen_artifact_root.to_path_buf());
    };
    let candidates = inventory
        .components
        .values()
        .flat_map(|component| component.providers.iter())
        .filter(|provider| provider.name == dependency.provider_name)
        .collect::<Vec<_>>();
    let exact = candidates
        .iter()
        .copied()
        .filter(|provider| {
            provider.version == dependency.provider_version && provider.digest == dependency.artifact_digest
        })
        .collect::<Vec<_>>();
    if dependency.default_features || dependency.optional || exact.len() != 1 {
        let active = if candidates.is_empty() {
            "missing from the active SDK inventory".to_string()
        } else {
            candidates
                .iter()
                .map(|provider| format!("{}@{}#{}", provider.name, provider.version, provider.digest))
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Err(PackageFeaturePlanError::ProviderDependencyArtifact {
            package: package_name.to_string(),
            dependency: dependency.dependency_key.clone(),
            path: package_manifest_path.to_path_buf(),
            message: format!(
                "private SDK provider `{}` froze {}@{}#{}, but the active SDK provides {active}; rebuild the compiled library with the active Incan SDK",
                dependency.provider_name,
                dependency.provider_name,
                dependency.provider_version,
                dependency.artifact_digest
            ),
        });
    }
    let provider = exact[0];
    provider.crate_root.clone().ok_or_else(|| {
        PackageFeaturePlanError::ProviderDependencyArtifact {
            package: package_name.to_string(),
            dependency: dependency.dependency_key.clone(),
            path: package_manifest_path.to_path_buf(),
            message: format!(
                "equivalent private SDK provider `{}` is unavailable in the active SDK installation; install the required component or rebuild with an available SDK profile",
                dependency.provider_name
            ),
        }
    })
}

/// Validate a resolved transitive artifact against the exact identity frozen by its parent provider.
fn validate_provider_dependency_descriptor(
    package_name: &str,
    descriptor: &ProviderDependencyMetadata,
    manifest: &LibraryManifest,
    artifact: &LibraryArtifactMetadata,
) -> Result<(), PackageFeaturePlanError> {
    let mismatch = if manifest.name != descriptor.provider_name {
        Some(format!(
            "expected provider name `{}`, found `{}`",
            descriptor.provider_name, manifest.name
        ))
    } else if manifest.version != descriptor.provider_version {
        Some(format!(
            "expected provider version `{}`, found `{}`",
            descriptor.provider_version, manifest.version
        ))
    } else if descriptor.kind == ProviderDependencyKind::PrivateImplementation
        && manifest.contract_metadata.provider.active_features != descriptor.requested_features
    {
        Some(format!(
            "expected private SDK feature projection [{}], found [{}]",
            render_features(&descriptor.requested_features),
            render_features(&manifest.contract_metadata.provider.active_features)
        ))
    } else {
        None
    };
    if let Some(message) = mismatch {
        return Err(PackageFeaturePlanError::ProviderDependencyArtifact {
            package: package_name.to_string(),
            dependency: descriptor.dependency_key.clone(),
            path: artifact.manifest_path.clone(),
            message,
        });
    }
    let digest = digest_provider_artifact(&artifact.crate_root).map_err(|error| {
        PackageFeaturePlanError::ProviderDependencyArtifact {
            package: package_name.to_string(),
            dependency: descriptor.dependency_key.clone(),
            path: artifact.crate_root.clone(),
            message: error.to_string(),
        }
    })?;
    if digest != descriptor.artifact_digest {
        return Err(PackageFeaturePlanError::ProviderDependencyArtifact {
            package: package_name.to_string(),
            dependency: descriptor.dependency_key.clone(),
            path: artifact.crate_root.clone(),
            message: format!(
                "expected artifact digest `{}`, found `{digest}`",
                descriptor.artifact_digest
            ),
        });
    }
    Ok(())
}

/// Render a deterministic comma-separated feature set for source-anchored package-feature diagnostics.
fn render_features(features: &BTreeSet<String>) -> String {
    features.iter().cloned().collect::<Vec<_>>().join(", ")
}

/// Read exactly `<dependency-root>/incan.toml` without walking into an unrelated ancestor project.
fn read_exact_project_manifest(project_root: &Path) -> Result<ProjectManifest, PackageFeaturePlanError> {
    let path = project_root.join(MANIFEST_FILENAME);
    let content = fs::read_to_string(&path).map_err(|source| PackageFeaturePlanError::ManifestRead {
        path: path.clone(),
        source,
    })?;
    ProjectManifest::from_str(&content, &path).map_err(|error| PackageFeaturePlanError::ManifestParse {
        path,
        message: error.to_string(),
    })
}

/// Normalize a package graph identity while retaining useful error paths when canonicalization is unavailable.
fn normalize_project_root(project_root: &Path) -> PathBuf {
    fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf())
}

/// Derive one human-readable package name without inventing a global semantic identity.
fn manifest_package_name(manifest: &ProjectManifest, project_root: &Path) -> String {
    manifest
        .project
        .as_ref()
        .and_then(|project| project.name.clone())
        .or_else(|| {
            project_root
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| project_root.display().to_string())
}

/// Locate the exact authored feature member responsible for one graph failure when it came from the manifest.
fn feature_error_location(path: &Path, error: &FeatureGraphError) -> String {
    let (owner, candidates) = feature_error_anchor(error);
    feature_value_location(path, owner, &candidates)
}

/// Locate one authored package-feature value in its source manifest, when source is available.
pub(crate) fn feature_value_location(path: &Path, owner: Option<&str>, candidates: &[String]) -> String {
    let Ok(content) = fs::read_to_string(path) else {
        return String::new();
    };
    let mut section = String::new();
    let mut compact_owner_active = false;
    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed.trim_matches(['[', ']']).to_string();
            compact_owner_active = false;
            continue;
        }
        if !section.starts_with("project.features") {
            continue;
        }
        if section == "project.features"
            && let Some((authored_feature, _)) = trimmed.split_once('=')
        {
            compact_owner_active = owner.is_none_or(|owner| authored_feature.trim() == owner);
        }
        let in_owner_section = owner.is_none_or(|owner| {
            section == "project.features" && compact_owner_active || section == format!("project.features.{owner}")
        });
        if !in_owner_section {
            continue;
        }
        for candidate in candidates {
            let quoted = format!("\"{candidate}\"");
            if let Some(column) = line.find(&quoted) {
                return format!(":{}:{}", line_index + 1, column + 1);
            }
        }
    }
    String::new()
}

/// Return the owning feature and authored value spellings most likely to identify one exact manifest item.
fn feature_error_anchor(error: &FeatureGraphError) -> (Option<&str>, Vec<String>) {
    match error {
        FeatureGraphError::InvalidMember { feature, member, .. } => (Some(feature), vec![member.clone()]),
        FeatureGraphError::UnknownFeature { feature, context } => {
            let owner = context.split('`').nth(1).filter(|owner| !owner.is_empty());
            (owner, vec![feature.clone()])
        }
        FeatureGraphError::UnknownDependency { feature, dependency }
        | FeatureGraphError::DependencyNotOptional { feature, dependency } => {
            (Some(feature), vec![format!("dep:{dependency}"), dependency.clone()])
        }
        FeatureGraphError::InactiveOptionalDependency {
            feature,
            dependency,
            dependency_feature,
        } => (
            Some(feature),
            vec![format!("{dependency}/{dependency_feature}"), dependency_feature.clone()],
        ),
        FeatureGraphError::Cycle { path } => {
            let mut cycle = path.split(" -> ");
            let owner = cycle.next();
            let included = cycle.next().unwrap_or_default();
            (owner, vec![included.to_string()])
        }
    }
}

impl PackageFeatureGraph {
    /// Reconstruct the public feature graph from a checked provider artifact without reopening producer source.
    pub fn from_provider_metadata(
        features: &BTreeMap<String, ProviderFeatureMetadata>,
    ) -> Result<Self, FeatureGraphError> {
        let mut definitions = BTreeMap::new();
        let mut optional_dependencies = BTreeSet::new();
        for (name, metadata) in features {
            validate_identifier(name).map_err(|message| FeatureGraphError::InvalidMember {
                feature: name.clone(),
                member: name.clone(),
                message,
            })?;
            optional_dependencies.extend(metadata.optional_dependencies.iter().cloned());
            definitions.insert(
                name.clone(),
                NormalizedFeatureDefinition {
                    local_features: metadata.includes.clone(),
                    optional_dependencies: metadata.optional_dependencies.clone(),
                    dependency_features: metadata.dependency_features.clone(),
                    required_sdk_components: metadata.required_sdk_components.clone(),
                },
            );
        }
        for (feature, definition) in &definitions {
            for included in &definition.local_features {
                if !definitions.contains_key(included) {
                    return Err(FeatureGraphError::UnknownFeature {
                        feature: included.clone(),
                        context: format!(" referenced by `{feature}`"),
                    });
                }
            }
        }
        validate_acyclic(&definitions)?;
        Ok(Self {
            definitions,
            optional_dependencies,
        })
    }

    /// Parse and validate every feature declaration in one project manifest.
    pub fn from_manifest(manifest: &ProjectManifest) -> Result<Self, FeatureGraphError> {
        let dependencies = manifest.library_dependencies();
        let optional_dependencies = dependencies
            .iter()
            .filter(|(_, dependency)| dependency.optional)
            .map(|(name, _)| name.clone())
            .collect();
        let mut definitions = BTreeMap::new();

        for (name, definition) in manifest.project_features() {
            validate_identifier(name).map_err(|message| FeatureGraphError::InvalidMember {
                feature: name.clone(),
                member: name.clone(),
                message,
            })?;
            definitions.insert(name.clone(), normalize_definition(name, definition)?);
        }

        for (feature, definition) in &definitions {
            for included in &definition.local_features {
                if !definitions.contains_key(included) {
                    return Err(FeatureGraphError::UnknownFeature {
                        feature: included.clone(),
                        context: format!(" referenced by `{feature}`"),
                    });
                }
            }
            for dependency in &definition.optional_dependencies {
                let Some(spec) = dependencies.get(dependency) else {
                    return Err(FeatureGraphError::UnknownDependency {
                        feature: feature.clone(),
                        dependency: dependency.clone(),
                    });
                };
                if !spec.optional {
                    return Err(FeatureGraphError::DependencyNotOptional {
                        feature: feature.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
            for dependency in definition.dependency_features.keys() {
                if !dependencies.contains_key(dependency) {
                    return Err(FeatureGraphError::UnknownDependency {
                        feature: feature.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
        }

        validate_acyclic(&definitions)?;
        Ok(Self {
            definitions,
            optional_dependencies,
        })
    }

    /// Resolve one additive active feature projection.
    pub fn resolve(&self, selection: &FeatureSelection) -> Result<ResolvedPackageFeatures, FeatureGraphError> {
        let mut resolved = ResolvedPackageFeatures::default();
        let mut pending = Vec::new();

        if !selection.no_default_features && self.definitions.contains_key("default") {
            add_root(&mut resolved, &mut pending, "default", FeatureActivationReason::Default);
        }
        for requested in &selection.requested {
            if !self.definitions.contains_key(requested) {
                return Err(FeatureGraphError::UnknownFeature {
                    feature: requested.clone(),
                    context: " requested by the current command".to_string(),
                });
            }
            add_root(
                &mut resolved,
                &mut pending,
                requested,
                FeatureActivationReason::Requested,
            );
        }
        if selection.all_features {
            for feature in self.definitions.keys() {
                add_root(
                    &mut resolved,
                    &mut pending,
                    feature,
                    FeatureActivationReason::AllFeatures,
                );
            }
        }

        while let Some(feature) = pending.pop() {
            let Some(definition) = self.definitions.get(&feature) else {
                continue;
            };
            resolved
                .active_optional_dependencies
                .extend(definition.optional_dependencies.iter().cloned());
            resolved
                .required_sdk_components
                .extend(definition.required_sdk_components.iter().cloned());
            for (dependency, features) in &definition.dependency_features {
                resolved
                    .dependency_features
                    .entry(dependency.clone())
                    .or_default()
                    .extend(features.iter().cloned());
            }
            for included in &definition.local_features {
                resolved
                    .reasons
                    .entry(included.clone())
                    .or_default()
                    .insert(FeatureActivationReason::IncludedBy(feature.clone()));
                if resolved.active_features.insert(included.clone()) {
                    pending.push(included.clone());
                }
            }
        }

        for active_feature in &resolved.active_features {
            let Some(definition) = self.definitions.get(active_feature) else {
                continue;
            };
            for (dependency, dependency_features) in &definition.dependency_features {
                if self.optional_dependencies.contains(dependency)
                    && !resolved.active_optional_dependencies.contains(dependency)
                {
                    let dependency_feature = dependency_features.iter().next().cloned().unwrap_or_default();
                    return Err(FeatureGraphError::InactiveOptionalDependency {
                        feature: active_feature.clone(),
                        dependency: dependency.clone(),
                        dependency_feature,
                    });
                }
            }
        }

        Ok(resolved)
    }

    /// Return every declared package feature in stable order.
    pub fn declared_features(&self) -> impl Iterator<Item = &str> {
        self.definitions.keys().map(String::as_str)
    }

    /// Project the validated graph into transport-stable compiled-provider metadata.
    pub fn provider_metadata(&self) -> BTreeMap<String, ProviderFeatureMetadata> {
        self.definitions
            .iter()
            .map(|(name, definition)| {
                (
                    name.clone(),
                    ProviderFeatureMetadata {
                        includes: definition.local_features.clone(),
                        optional_dependencies: definition.optional_dependencies.clone(),
                        dependency_features: definition.dependency_features.clone(),
                        required_sdk_components: definition.required_sdk_components.clone(),
                    },
                )
            })
            .collect()
    }
}

/// Add one root feature and its reason to the pending closure.
fn add_root(
    resolved: &mut ResolvedPackageFeatures,
    pending: &mut Vec<String>,
    feature: &str,
    reason: FeatureActivationReason,
) {
    resolved.reasons.entry(feature.to_string()).or_default().insert(reason);
    if resolved.active_features.insert(feature.to_string()) {
        pending.push(feature.to_string());
    }
}

/// Normalize either authored feature representation into typed edge sets.
fn normalize_definition(
    feature: &str,
    definition: &ProjectFeatureDefinition,
) -> Result<NormalizedFeatureDefinition, FeatureGraphError> {
    match definition {
        ProjectFeatureDefinition::Compact(members) => normalize_compact_definition(feature, members),
        ProjectFeatureDefinition::Expanded(expanded) => normalize_expanded_definition(feature, expanded),
    }
}

/// Parse the compact feature-member reference language.
fn normalize_compact_definition(
    feature: &str,
    members: &[String],
) -> Result<NormalizedFeatureDefinition, FeatureGraphError> {
    let mut normalized = NormalizedFeatureDefinition::default();
    for member in members {
        let member = member.trim();
        if let Some(dependency) = member.strip_prefix("dep:") {
            validate_member_identifier(feature, member, dependency)?;
            normalized.optional_dependencies.insert(dependency.to_string());
        } else if let Some((dependency, dependency_feature)) = member.split_once('/') {
            if dependency_feature.contains('/') {
                return Err(invalid_member(feature, member, "expected exactly one `/` separator"));
            }
            validate_member_identifier(feature, member, dependency)?;
            validate_member_identifier(feature, member, dependency_feature)?;
            normalized
                .dependency_features
                .entry(dependency.to_string())
                .or_default()
                .insert(dependency_feature.to_string());
        } else {
            validate_member_identifier(feature, member, member)?;
            normalized.local_features.insert(member.to_string());
        }
    }
    Ok(normalized)
}

/// Normalize the expanded typed feature table.
fn normalize_expanded_definition(
    feature: &str,
    expanded: &ExpandedProjectFeature,
) -> Result<NormalizedFeatureDefinition, FeatureGraphError> {
    let mut normalized = NormalizedFeatureDefinition::default();
    for included in &expanded.includes {
        validate_member_identifier(feature, included, included)?;
        normalized.local_features.insert(included.clone());
    }
    for dependency in &expanded.optional_dependencies {
        validate_member_identifier(feature, dependency, dependency)?;
        normalized.optional_dependencies.insert(dependency.clone());
    }
    for (dependency, features) in &expanded.dependency_features {
        validate_member_identifier(feature, dependency, dependency)?;
        for dependency_feature in features {
            validate_member_identifier(feature, dependency_feature, dependency_feature)?;
            normalized
                .dependency_features
                .entry(dependency.clone())
                .or_default()
                .insert(dependency_feature.clone());
        }
    }
    for component in &expanded.requires_sdk_components {
        validate_member_identifier(feature, component, component)?;
        normalized.required_sdk_components.insert(component.clone());
    }
    Ok(normalized)
}

/// Validate one identifier embedded in a feature member and retain authored context on failure.
fn validate_member_identifier(feature: &str, member: &str, identifier: &str) -> Result<(), FeatureGraphError> {
    validate_identifier(identifier).map_err(|message| invalid_member(feature, member, message))
}

/// Build one invalid-member error while preserving the authored feature and member.
fn invalid_member(feature: &str, member: &str, message: impl Into<String>) -> FeatureGraphError {
    FeatureGraphError::InvalidMember {
        feature: feature.to_string(),
        member: member.to_string(),
        message: message.into(),
    }
}

/// Validate the RFC 114 feature, dependency, and component identifier subset.
fn validate_identifier(identifier: &str) -> Result<(), String> {
    validate_package_feature_identifier(identifier).map_err(str::to_string)
}

/// Reject cycles in the complete local feature graph, including inactive declarations.
fn validate_acyclic(definitions: &BTreeMap<String, NormalizedFeatureDefinition>) -> Result<(), FeatureGraphError> {
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut stack = Vec::new();
    for feature in definitions.keys() {
        visit_feature(feature, definitions, &mut visiting, &mut visited, &mut stack)?;
    }
    Ok(())
}

/// Depth-first cycle validation for one local feature.
fn visit_feature(
    feature: &str,
    definitions: &BTreeMap<String, NormalizedFeatureDefinition>,
    visiting: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
    stack: &mut Vec<String>,
) -> Result<(), FeatureGraphError> {
    if visited.contains(feature) {
        return Ok(());
    }
    if visiting.contains(feature) {
        let start = stack.iter().position(|entry| entry == feature).unwrap_or(0);
        let mut cycle = stack[start..].to_vec();
        cycle.push(feature.to_string());
        return Err(FeatureGraphError::Cycle {
            path: cycle.join(" -> "),
        });
    }

    visiting.insert(feature.to_string());
    stack.push(feature.to_string());
    if let Some(definition) = definitions.get(feature) {
        for included in &definition.local_features {
            visit_feature(included, definitions, visiting, visited, stack)?;
        }
    }
    stack.pop();
    visiting.remove(feature);
    visited.insert(feature.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::manifest::ProjectManifest;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn manifest(content: &str) -> Result<ProjectManifest, crate::manifest::ManifestError> {
        ProjectManifest::from_str(content, Path::new("/workspace/incan.toml"))
    }

    #[test]
    fn resolves_defaults_compact_and_expanded_edges() -> TestResult {
        let manifest = manifest(
            r#"
[project]
name = "reporting"

[project.features]
default = ["json"]
json = ["dep:serializer", "serializer/json"]

[project.features.server]
includes = ["json"]
optional-dependencies = ["http_server"]
dependency-features = { http_server = ["tls"] }
requires-sdk-components = ["stdlib-web"]

[dependencies]
serializer = { path = "../serializer", optional = true, default-features = false }
http_server = { path = "../http_server", optional = true, default-features = false }
"#,
        )?;
        let selection = FeatureSelection::new(["server"]);
        let resolved = PackageFeatureGraph::from_manifest(&manifest)?.resolve(&selection)?;

        assert_eq!(resolved.active_features, set(["default", "json", "server"]));
        assert_eq!(
            resolved.active_optional_dependencies,
            set(["http_server", "serializer"])
        );
        assert_eq!(resolved.dependency_features.get("serializer"), Some(&set(["json"])));
        assert_eq!(resolved.dependency_features.get("http_server"), Some(&set(["tls"])));
        assert_eq!(resolved.required_sdk_components, set(["stdlib-web"]));
        Ok(())
    }

    #[test]
    fn no_default_features_suppresses_only_the_default_root() -> TestResult {
        let manifest = manifest(
            r#"
[project.features]
default = ["json"]
json = []
http = []
"#,
        )?;
        let selection = FeatureSelection {
            requested: set(["http"]),
            no_default_features: true,
            all_features: false,
        };
        let resolved = PackageFeatureGraph::from_manifest(&manifest)?.resolve(&selection)?;

        assert_eq!(resolved.active_features, set(["http"]));
        Ok(())
    }

    #[test]
    fn reconstructs_artifact_owned_feature_graph_without_producer_source() -> TestResult {
        let metadata = BTreeMap::from([
            (
                "default".to_string(),
                ProviderFeatureMetadata {
                    includes: set(["json"]),
                    ..ProviderFeatureMetadata::default()
                },
            ),
            (
                "json".to_string(),
                ProviderFeatureMetadata {
                    optional_dependencies: set(["serializer"]),
                    dependency_features: BTreeMap::from([("serializer".to_string(), set(["json"]))]),
                    required_sdk_components: set(["stdlib-data"]),
                    ..ProviderFeatureMetadata::default()
                },
            ),
        ]);
        let resolved = PackageFeatureGraph::from_provider_metadata(&metadata)?.resolve(&FeatureSelection::default())?;

        assert_eq!(resolved.active_features, set(["default", "json"]));
        assert_eq!(resolved.active_optional_dependencies, set(["serializer"]));
        assert_eq!(resolved.dependency_features.get("serializer"), Some(&set(["json"])));
        assert_eq!(resolved.required_sdk_components, set(["stdlib-data"]));
        Ok(())
    }

    #[test]
    fn rejects_cycles_even_when_the_cycle_is_inactive() -> TestResult {
        let manifest = manifest(
            r#"
[project.features]
a = ["b"]
b = ["a"]
"#,
        )?;
        let error = PackageFeatureGraph::from_manifest(&manifest)
            .err()
            .ok_or("expected feature cycle")?;

        assert!(matches!(error, FeatureGraphError::Cycle { .. }));
        assert!(error.to_string().contains("a -> b -> a") || error.to_string().contains("b -> a -> b"));
        Ok(())
    }

    #[test]
    fn rejects_dependency_feature_request_without_activating_optional_dependency() -> TestResult {
        let manifest = manifest(
            r#"
[project.features]
json = ["serializer/json"]

[dependencies]
serializer = { path = "../serializer", optional = true }
"#,
        )?;
        let graph = PackageFeatureGraph::from_manifest(&manifest)?;
        let error = graph
            .resolve(&FeatureSelection::new(["json"]))
            .err()
            .ok_or("expected inactive optional dependency")?;

        assert!(matches!(error, FeatureGraphError::InactiveOptionalDependency { .. }));
        assert!(error.to_string().contains("serializer"));
        Ok(())
    }

    #[test]
    fn rejects_unknown_requested_feature() -> TestResult {
        let manifest = manifest("[project.features]\njson = []\n")?;
        let graph = PackageFeatureGraph::from_manifest(&manifest)?;
        let error = graph
            .resolve(&FeatureSelection::new(["missing"]))
            .err()
            .ok_or("expected unknown feature")?;

        assert!(matches!(error, FeatureGraphError::UnknownFeature { .. }));
        Ok(())
    }

    #[test]
    fn package_feature_errors_point_to_the_exact_manifest_array_item() -> TestResult {
        let workspace = tempfile::tempdir()?;
        let manifest_path = workspace.path().join("incan.toml");
        fs::write(
            &manifest_path,
            "[project]\nname = \"demo\"\n\n[project.features]\ndefault = [\n    \"missing\",\n]\n",
        )?;
        let manifest = ProjectManifest::discover(workspace.path())?.ok_or("missing project manifest")?;
        let error = PackageFeaturePlan::resolve(&manifest, &FeatureSelection::default())
            .err()
            .ok_or("expected unknown feature")?;

        assert!(
            error.to_string().contains("incan.toml:6:5"),
            "expected exact feature member location, got: {error}"
        );
        Ok(())
    }

    #[test]
    fn resolves_dependency_feature_projection_across_path_packages() -> TestResult {
        let workspace = tempfile::tempdir()?;
        let serializer = workspace.path().join("serializer");
        let reporting = workspace.path().join("reporting");
        fs::create_dir_all(&serializer)?;
        fs::create_dir_all(&reporting)?;
        fs::write(
            serializer.join("incan.toml"),
            r#"
[project]
name = "serializer"

[project.features]
default = []
json = []
"#,
        )?;
        fs::write(
            reporting.join("incan.toml"),
            r#"
[project]
name = "reporting"

[project.features]
default = ["json"]
json = ["dep:serializer", "serializer/json"]

[dependencies]
serializer = { path = "../serializer", optional = true, default-features = false }
"#,
        )?;
        let root = ProjectManifest::discover(&reporting)?.ok_or("missing root manifest")?;
        let plan = PackageFeaturePlan::resolve(&root, &FeatureSelection::default())?;
        let serializer_state = plan
            .packages()
            .find(|package| package.package_name == "serializer")
            .ok_or("missing serializer feature state")?;

        assert_eq!(serializer_state.features.active_features, set(["json"]));
        assert!(!serializer_state.features.active_features.contains("default"));
        assert_eq!(
            serializer_state.features.reasons.get("json"),
            Some(&BTreeSet::from([FeatureActivationReason::DependencyRequest {
                package: "reporting".to_string(),
                dependency: "serializer".to_string(),
            }]))
        );
        assert_eq!(plan.edges().count(), 1);
        Ok(())
    }

    #[test]
    fn resolves_relocated_transitive_artifact_graph_without_producer_source() -> TestResult {
        let workspace = tempfile::tempdir()?;
        let original = workspace.path().join("original");
        let serializer_root = original.join("serializer");
        let serializer_artifact = serializer_root.join("target/lib");
        write_test_provider_artifact(&serializer_artifact, "serializer_core")?;
        let mut serializer = LibraryManifest::new("serializer_core", "0.5.0");
        serializer.contract_metadata.provider.public_features =
            BTreeMap::from([("json".to_string(), ProviderFeatureMetadata::default())]);
        serializer.contract_metadata.provider.active_features = BTreeSet::from(["json".to_string()]);
        serializer.write_to_path(&serializer_artifact.join("serializer_core.incnlib"))?;
        let serializer_digest = digest_provider_artifact(&serializer_artifact)?;

        let runtime_root = original.join("reporting_runtime");
        let runtime_artifact = runtime_root.join("target/lib");
        write_test_provider_artifact(&runtime_artifact, "reporting_runtime")?;
        LibraryManifest::new("reporting_runtime", "0.5.0")
            .write_to_path(&runtime_artifact.join("reporting_runtime.incnlib"))?;
        let runtime_digest = digest_provider_artifact(&runtime_artifact)?;

        let reporting_root = original.join("reporting");
        let reporting_artifact = reporting_root.join("target/lib");
        write_test_provider_artifact(&reporting_artifact, "reporting_core")?;
        let mut reporting = LibraryManifest::new("reporting_core", "0.5.0");
        reporting.contract_metadata.provider.public_features = BTreeMap::from([
            (
                "default".to_string(),
                ProviderFeatureMetadata {
                    includes: BTreeSet::from(["json".to_string()]),
                    ..ProviderFeatureMetadata::default()
                },
            ),
            (
                "json".to_string(),
                ProviderFeatureMetadata {
                    optional_dependencies: BTreeSet::from(["serializer".to_string()]),
                    dependency_features: BTreeMap::from([(
                        "serializer".to_string(),
                        BTreeSet::from(["json".to_string()]),
                    )]),
                    ..ProviderFeatureMetadata::default()
                },
            ),
        ]);
        reporting.contract_metadata.provider.active_features =
            BTreeSet::from(["default".to_string(), "json".to_string()]);
        reporting
            .contract_metadata
            .provider
            .provider_dependencies
            .push(ProviderDependencyMetadata {
                kind: crate::library_manifest::ProviderDependencyKind::PublicPackage,
                dependency_key: "serializer".to_string(),
                provider_name: "serializer_core".to_string(),
                provider_version: "0.5.0".to_string(),
                artifact_digest: serializer_digest,
                relative_artifact_path: "../../../serializer/target/lib".to_string(),
                requested_features: BTreeSet::from(["json".to_string()]),
                default_features: false,
                optional: true,
            });
        reporting
            .contract_metadata
            .provider
            .provider_dependencies
            .push(ProviderDependencyMetadata {
                kind: ProviderDependencyKind::PrivateImplementation,
                dependency_key: "reporting_runtime".to_string(),
                provider_name: "reporting_runtime".to_string(),
                provider_version: "0.5.0".to_string(),
                artifact_digest: runtime_digest,
                relative_artifact_path: "../../../reporting_runtime/target/lib".to_string(),
                requested_features: BTreeSet::new(),
                default_features: false,
                optional: false,
            });
        reporting.write_to_path(&reporting_artifact.join("reporting_core.incnlib"))?;

        let consumer_root = original.join("consumer");
        fs::create_dir_all(&consumer_root)?;
        fs::write(
            consumer_root.join(MANIFEST_FILENAME),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nreporting = { path = \"../reporting\" }\n",
        )?;

        let relocated = workspace.path().join("relocated");
        fs::rename(&original, &relocated)?;
        let consumer = ProjectManifest::discover(&relocated.join("consumer"))?.ok_or("missing consumer manifest")?;
        let plan = PackageFeaturePlan::resolve(&consumer, &FeatureSelection::default())?;
        let packages = plan
            .packages()
            .map(|package| package.package_name.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(
            packages,
            BTreeSet::from(["consumer", "reporting_core", "serializer_core"])
        );
        assert_eq!(plan.edges().count(), 2);
        assert!(
            !packages.contains("reporting_runtime"),
            "private implementation providers must not acquire a public package namespace"
        );
        assert!(
            plan.packages()
                .filter(|package| package.package_name != "consumer")
                .all(|package| package.manifest.is_none())
        );

        fs::write(
            relocated.join("serializer/target/lib/src/lib.rs"),
            "pub fn marker() { let _corrupt = true; }\n",
        )?;
        let error = PackageFeaturePlan::resolve(&consumer, &FeatureSelection::default())
            .err()
            .ok_or("corrupt transitive provider artifact should fail integrity validation")?;
        assert!(error.to_string().contains("expected artifact digest"));
        Ok(())
    }

    #[test]
    fn package_feature_plan_rebinds_private_sdk_before_reading_absent_cache_issue911() -> TestResult {
        let workspace = tempfile::tempdir()?;
        let reporting_root = workspace.path().join("reporting");
        let reporting_artifact = reporting_root.join("target/lib");
        let absent_sdk = workspace.path().join("sdk-cache-a/runtime");
        let active_sdk = workspace.path().join("sdk-cache-b/runtime");
        write_test_provider_artifact(&reporting_artifact, "reporting")?;
        write_test_provider_artifact(&active_sdk, "incan_issue911_runtime")?;

        let active_manifest_path = active_sdk.join("incan_issue911_runtime.incnlib");
        LibraryManifest::new("incan_issue911_runtime", "0.5.0").write_to_path(&active_manifest_path)?;
        let active_digest = digest_provider_artifact(&active_sdk)?;
        let mut reporting = LibraryManifest::new("reporting", "0.5.0");
        reporting
            .contract_metadata
            .provider
            .provider_dependencies
            .push(ProviderDependencyMetadata {
                kind: ProviderDependencyKind::PrivateImplementation,
                dependency_key: "incan_issue911_runtime".to_string(),
                provider_name: "incan_issue911_runtime".to_string(),
                provider_version: "0.5.0".to_string(),
                artifact_digest: active_digest.clone(),
                relative_artifact_path: "../../../sdk-cache-a/runtime".to_string(),
                requested_features: BTreeSet::new(),
                default_features: false,
                optional: false,
            });
        reporting.write_to_path(&reporting_artifact.join("reporting.incnlib"))?;

        let consumer_root = workspace.path().join("consumer");
        fs::create_dir_all(&consumer_root)?;
        fs::write(
            consumer_root.join(MANIFEST_FILENAME),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nreporting = { path = \"../reporting\" }\n",
        )?;
        let inventory = SdkInventory {
            root: workspace.path().join("sdk-cache-b"),
            sdk_id: "incan".to_string(),
            sdk_version: "0.5.0".to_string(),
            compiler_requirement: ">=0.5.0-dev.16,<0.6.0".to_string(),
            components: BTreeMap::from([(
                "runtime".to_string(),
                crate::provider::SdkComponent {
                    id: "runtime".to_string(),
                    version: "0.5.0".to_string(),
                    mandatory: true,
                    available: true,
                    dependencies: BTreeSet::new(),
                    providers: vec![crate::provider::SdkProviderDescriptor {
                        name: "incan_issue911_runtime".to_string(),
                        version: "0.5.0".to_string(),
                        digest: active_digest,
                        namespace_claims: BTreeSet::new(),
                        manifest_path: Some(active_manifest_path),
                        crate_root: Some(active_sdk),
                    }],
                },
            )]),
            profiles: BTreeMap::new(),
        };
        let consumer = ProjectManifest::discover(&consumer_root)?.ok_or("missing consumer manifest")?;

        let plan =
            PackageFeaturePlan::resolve_with_sdk_inventory(&consumer, &FeatureSelection::default(), Some(&inventory))?;

        assert!(plan.packages().any(|package| package.package_name == "reporting"));
        assert!(
            !absent_sdk.exists(),
            "feature planning must not reopen the stale cache root"
        );
        Ok(())
    }

    fn write_test_provider_artifact(artifact_root: &Path, package_name: &str) -> TestResult {
        fs::create_dir_all(artifact_root.join("src"))?;
        fs::write(
            artifact_root.join("Cargo.toml"),
            format!("[package]\nname = \"{package_name}\"\nversion = \"0.5.0\"\nedition = \"2024\"\n"),
        )?;
        fs::write(artifact_root.join("src/lib.rs"), "pub fn marker() {}\n")?;
        Ok(())
    }

    fn set<const N: usize>(values: [&str; N]) -> std::collections::BTreeSet<String> {
        values.into_iter().map(str::to_string).collect()
    }
}
