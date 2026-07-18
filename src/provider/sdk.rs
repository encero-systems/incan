//! SDK inventory, component catalog, and profile selection from RFC 114.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::manifest::ProjectManifest;

/// Stable SDK inventory file name discovered relative to an installed toolchain root.
pub const SDK_INVENTORY_FILE: &str = "sdk-inventory.json";
/// Source catalog used by SDK publishers to build the installed inventory.
pub const SDK_SOURCE_CATALOG_FILE: &str = "sdk-components.toml";
/// Current JSON schema version for SDK inventory files.
pub const SDK_INVENTORY_SCHEMA_VERSION: u32 = 1;

/// One compiled provider artifact advertised by an SDK component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkProviderDescriptor {
    /// Stable provider name within this SDK release.
    pub name: String,
    /// Provider semantic version.
    pub version: String,
    /// Expected artifact digest recorded by the SDK publisher.
    pub digest: String,
    /// Exact canonical module paths granted to this provider.
    pub namespace_claims: BTreeSet<Vec<String>>,
    /// Installed `.incnlib` path when this component is available locally.
    pub manifest_path: Option<PathBuf>,
    /// Installed generated Rust crate root when this component is available locally.
    pub crate_root: Option<PathBuf>,
}

/// One component catalog entry from the active SDK inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkComponent {
    /// Stable component id.
    pub id: String,
    /// Component version inside this SDK release.
    pub version: String,
    /// Whether every installation of this SDK must enable the component.
    pub mandatory: bool,
    /// Whether this local installation contains the component artifacts.
    pub available: bool,
    /// Other component ids required by this component.
    pub dependencies: BTreeSet<String>,
    /// Compiled providers supplied by this component.
    pub providers: Vec<SdkProviderDescriptor>,
}

/// Integrity-checked component catalog and profile definitions for one SDK release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkInventory {
    /// Root against which installed artifact paths resolve.
    pub root: PathBuf,
    /// Stable SDK family id.
    pub sdk_id: String,
    /// Exact SDK release version.
    pub sdk_version: String,
    /// Compiler compatibility requirement authored by the SDK.
    pub compiler_requirement: String,
    /// Complete known component catalog, including unavailable entries.
    pub components: BTreeMap<String, SdkComponent>,
    /// Named profile membership owned by this SDK release.
    pub profiles: BTreeMap<String, BTreeSet<String>>,
}

/// One source project that publishes an SDK component provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkSourceComponent {
    /// Stable component id.
    pub id: String,
    /// Project root containing the component's `incan.toml`.
    pub project_root: PathBuf,
    /// Whether this component is mandatory in every profile.
    pub mandatory: bool,
    /// Other source components that must be published first.
    pub dependencies: BTreeSet<String>,
    /// Publisher-only provider artifacts required while compiling this component without enabling their public SDK
    /// component for consumers.
    pub build_dependencies: BTreeSet<String>,
    /// Top-level reserved namespaces granted to this official component by the SDK publisher.
    pub namespace_roots: BTreeSet<String>,
}

/// Validated source-side SDK component catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkSourceCatalog {
    /// Directory containing the source catalog.
    pub root: PathBuf,
    /// Stable SDK family id.
    pub sdk_id: String,
    /// Exact SDK release version.
    pub sdk_version: String,
    /// Compiler compatibility requirement copied to the installed inventory.
    pub compiler_requirement: String,
    /// Source components keyed by stable id.
    pub components: BTreeMap<String, SdkSourceComponent>,
    /// Named profile membership copied to the installed inventory.
    pub profiles: BTreeMap<String, BTreeSet<String>>,
}

/// Project and command-owned component selection before dependency expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkComponentSelection {
    /// Named SDK profile.
    pub profile: String,
    /// Explicit component additions.
    pub components: BTreeSet<String>,
    /// Deliberate component exclusions.
    pub exclude_components: BTreeSet<String>,
}

impl Default for SdkComponentSelection {
    fn default() -> Self {
        Self {
            profile: "default".to_string(),
            components: BTreeSet::new(),
            exclude_components: BTreeSet::new(),
        }
    }
}

impl SdkComponentSelection {
    /// Derive the persistent project selection, falling back to the conventional `default` profile.
    pub fn from_manifest(manifest: Option<&ProjectManifest>) -> Self {
        let Some(sdk) = manifest.and_then(ProjectManifest::sdk) else {
            return Self::default();
        };
        Self {
            profile: sdk.profile.clone().unwrap_or_else(|| "default".to_string()),
            components: sdk.components.iter().cloned().collect(),
            exclude_components: sdk.exclude_components.iter().cloned().collect(),
        }
    }

    /// Derive project selection while replacing only the profile for one non-persistent command invocation.
    pub fn from_manifest_with_profile_override(
        manifest: Option<&ProjectManifest>,
        profile_override: Option<&str>,
    ) -> Self {
        let mut selection = Self::from_manifest(manifest);
        if let Some(profile) = profile_override.filter(|profile| !profile.is_empty()) {
            selection.profile = profile.to_string();
        }
        selection
    }
}

/// Stable explanation for why an SDK component is enabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ComponentSelectionReason {
    /// Required by every project using this SDK release.
    Mandatory,
    /// Selected through the named SDK profile.
    Profile {
        /// Profile that contributed the component.
        profile: String,
    },
    /// Added explicitly by the project or current command.
    Explicit,
    /// Added as a transitive dependency of another component.
    Dependency {
        /// Direct component requiring this dependency.
        required_by: String,
    },
}

/// Expanded component state used by provider planning and inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSdkComponents {
    /// Active SDK identity rendered as `<id>@<version>`.
    pub sdk_identity: String,
    /// Selected profile name.
    pub profile: String,
    /// All enabled components after mandatory and dependency expansion.
    pub enabled: BTreeSet<String>,
    /// Enabled components whose artifacts are absent from this installation.
    pub unavailable: BTreeSet<String>,
    /// One stable primary reason for every enabled component.
    pub reasons: BTreeMap<String, ComponentSelectionReason>,
}

/// Invalid serialized SDK catalog or provider descriptor.
#[derive(Debug, thiserror::Error)]
pub enum SdkInventoryError {
    /// Reading the inventory file failed.
    #[error("failed to read SDK inventory {path}: {source}")]
    Read {
        /// Inventory path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// Writing a validated inventory file failed.
    #[error("failed to write SDK inventory {path}: {source}")]
    Write {
        /// Inventory path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// JSON decoding failed.
    #[error("failed to parse SDK inventory: {0}")]
    Parse(String),
    /// JSON encoding failed.
    #[error("failed to serialize SDK inventory: {0}")]
    Serialize(String),
    /// The schema version is not supported by this compiler.
    #[error("unsupported SDK inventory schema {actual}; expected {expected}")]
    UnsupportedSchema {
        /// Decoded schema version.
        actual: u32,
        /// Compiler-supported schema version.
        expected: u32,
    },
    /// A required identity, version, path, digest, profile, or edge is invalid.
    #[error("invalid SDK inventory: {0}")]
    Invalid(String),
    /// The component dependency graph contains a cycle.
    #[error("SDK component dependency cycle: {path}")]
    ComponentCycle {
        /// Stable arrow-separated cycle path.
        path: String,
    },
}

/// Project component selection cannot be satisfied by a valid SDK catalog.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SdkResolutionError {
    /// The requested profile does not exist in this SDK release.
    #[error("unknown SDK profile `{profile}` for {sdk_identity}")]
    UnknownProfile {
        /// Requested profile.
        profile: String,
        /// Active SDK identity.
        sdk_identity: String,
    },
    /// A project addition or exclusion names no catalog component.
    #[error("unknown SDK component `{component}` for {sdk_identity}")]
    UnknownComponent {
        /// Missing component id.
        component: String,
        /// Active SDK identity.
        sdk_identity: String,
    },
    /// A mandatory component was explicitly excluded.
    #[error("SDK component `{component}` is mandatory for {sdk_identity} and cannot be excluded")]
    MandatoryComponentExcluded {
        /// Mandatory component id.
        component: String,
        /// Active SDK identity.
        sdk_identity: String,
    },
    /// One component appears in both explicit additions and exclusions.
    #[error("SDK component `{component}` is both enabled and excluded explicitly")]
    SelectedComponentExcluded {
        /// Conflicting component id.
        component: String,
    },
    /// Exclusion breaks a selected component dependency path.
    #[error("SDK component exclusion breaks required path {path}")]
    ExcludedRequiredComponent {
        /// Excluded dependency id.
        component: String,
        /// Arrow-separated dependency path from a selected root.
        path: String,
    },
    /// A component is enabled but omitted from the local SDK installation.
    #[error("SDK component `{component}` is enabled but unavailable in {sdk_identity}")]
    EnabledComponentUnavailable {
        /// Unavailable enabled component.
        component: String,
        /// Active SDK identity.
        sdk_identity: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSdkInventory {
    schema_version: u32,
    sdk_id: String,
    sdk_version: String,
    compiler_requirement: String,
    components: BTreeMap<String, RawSdkComponent>,
    profiles: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSdkComponent {
    version: String,
    #[serde(default)]
    mandatory: bool,
    available: bool,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    providers: Vec<RawSdkProviderDescriptor>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSdkProviderDescriptor {
    name: String,
    version: String,
    digest: String,
    #[serde(default)]
    namespace_claims: Vec<Vec<String>>,
    manifest_path: Option<String>,
    crate_root: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSdkSourceCatalog {
    sdk: RawSdkSourceIdentity,
    profiles: BTreeMap<String, Vec<String>>,
    components: BTreeMap<String, RawSdkSourceComponent>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawSdkSourceIdentity {
    id: String,
    version: String,
    compiler_requirement: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSdkSourceComponent {
    project: String,
    #[serde(default)]
    mandatory: bool,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default, rename = "build-dependencies")]
    build_dependencies: Vec<String>,
    #[serde(default, rename = "namespace-roots")]
    namespace_roots: Vec<String>,
}

impl SdkSourceCatalog {
    /// Read and validate the SDK publisher's component source catalog.
    pub fn read_from_path(path: &Path) -> Result<Self, SdkInventoryError> {
        let content = fs::read_to_string(path).map_err(|source| SdkInventoryError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let raw: RawSdkSourceCatalog = toml::from_str(&content)
            .map_err(|error| SdkInventoryError::Parse(format!("source component catalog: {error}")))?;
        let root = path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        validate_nonempty_identifier("SDK id", &raw.sdk.id)?;
        Version::parse(&raw.sdk.version)
            .map_err(|error| SdkInventoryError::Invalid(format!("invalid SDK source version: {error}")))?;
        VersionReq::parse(&raw.sdk.compiler_requirement)
            .map_err(|error| SdkInventoryError::Invalid(format!("invalid compiler_requirement: {error}")))?;

        let mut components = BTreeMap::new();
        for (id, component) in raw.components {
            validate_nonempty_identifier("component id", &id)?;
            let project = PathBuf::from(&component.project);
            if project.is_absolute()
                || project
                    .components()
                    .any(|segment| matches!(segment, std::path::Component::ParentDir))
            {
                return Err(SdkInventoryError::Invalid(format!(
                    "component `{id}` project path `{}` must stay relative to the source catalog",
                    component.project
                )));
            }
            components.insert(
                id.clone(),
                SdkSourceComponent {
                    id,
                    project_root: root.join(project),
                    mandatory: component.mandatory,
                    dependencies: component.dependencies.into_iter().collect(),
                    build_dependencies: component.build_dependencies.into_iter().collect(),
                    namespace_roots: component.namespace_roots.into_iter().collect(),
                },
            );
        }
        let mut namespace_owners = BTreeMap::new();
        for component in components.values() {
            for namespace in &component.namespace_roots {
                validate_nonempty_identifier("namespace root", namespace)?;
                if let Some(existing) = namespace_owners.insert(namespace.clone(), component.id.clone()) {
                    return Err(SdkInventoryError::Invalid(format!(
                        "namespace root `{namespace}` is assigned to both component `{existing}` and component `{}`",
                        component.id
                    )));
                }
            }
            for dependency in &component.dependencies {
                if !components.contains_key(dependency) {
                    return Err(SdkInventoryError::Invalid(format!(
                        "component `{}` depends on unknown component `{dependency}`",
                        component.id
                    )));
                }
            }
            for dependency in &component.build_dependencies {
                if !components.contains_key(dependency) {
                    return Err(SdkInventoryError::Invalid(format!(
                        "component `{}` has unknown publisher build dependency `{dependency}`",
                        component.id
                    )));
                }
            }
        }
        let profiles = raw
            .profiles
            .into_iter()
            .map(|(name, members)| (name, members.into_iter().collect::<BTreeSet<_>>()))
            .collect::<BTreeMap<_, _>>();
        for (profile, members) in &profiles {
            validate_nonempty_identifier("profile name", profile)?;
            for component in members {
                if !components.contains_key(component) {
                    return Err(SdkInventoryError::Invalid(format!(
                        "profile `{profile}` references unknown component `{component}`"
                    )));
                }
            }
        }
        if !profiles.contains_key("default") {
            return Err(SdkInventoryError::Invalid(
                "SDK source catalog must define a `default` profile".to_string(),
            ));
        }
        let graph = components
            .iter()
            .map(|(id, component)| {
                let mut publication_dependencies = component.dependencies.clone();
                publication_dependencies.extend(component.build_dependencies.iter().cloned());
                (
                    id.clone(),
                    SdkComponent {
                        id: id.clone(),
                        version: raw.sdk.version.clone(),
                        mandatory: component.mandatory,
                        available: false,
                        dependencies: publication_dependencies,
                        providers: Vec::new(),
                    },
                )
            })
            .collect();
        validate_component_graph(&graph)?;
        Ok(Self {
            root,
            sdk_id: raw.sdk.id,
            sdk_version: raw.sdk.version,
            compiler_requirement: raw.sdk.compiler_requirement,
            components,
            profiles,
        })
    }

    /// Return source components in deterministic dependency-first publication order.
    pub fn publication_order(&self) -> Vec<&SdkSourceComponent> {
        let mut ordered = Vec::new();
        let mut emitted = BTreeSet::new();
        while ordered.len() < self.components.len() {
            let mut progressed = false;
            for component in self.components.values() {
                let publication_ready =
                    component.dependencies.is_subset(&emitted) && component.build_dependencies.is_subset(&emitted);
                if emitted.contains(&component.id) || !publication_ready {
                    continue;
                }
                emitted.insert(component.id.clone());
                ordered.push(component);
                progressed = true;
            }
            if !progressed {
                break;
            }
        }
        ordered
    }
}

impl SdkInventory {
    /// Read and validate an SDK inventory file from disk.
    pub fn read_from_path(path: &Path) -> Result<Self, SdkInventoryError> {
        let content = fs::read_to_string(path).map_err(|source| SdkInventoryError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let root = path.parent().unwrap_or_else(|| Path::new("."));
        Self::from_json(&content, root)
    }

    /// Decode and validate an SDK inventory against its installation root.
    pub fn from_json(content: &str, root: &Path) -> Result<Self, SdkInventoryError> {
        let raw: RawSdkInventory =
            serde_json::from_str(content).map_err(|error| SdkInventoryError::Parse(error.to_string()))?;
        if raw.schema_version != SDK_INVENTORY_SCHEMA_VERSION {
            return Err(SdkInventoryError::UnsupportedSchema {
                actual: raw.schema_version,
                expected: SDK_INVENTORY_SCHEMA_VERSION,
            });
        }
        validate_nonempty_identifier("SDK id", &raw.sdk_id)?;
        Version::parse(&raw.sdk_version)
            .map_err(|error| SdkInventoryError::Invalid(format!("invalid sdk_version: {error}")))?;
        VersionReq::parse(&raw.compiler_requirement)
            .map_err(|error| SdkInventoryError::Invalid(format!("invalid compiler_requirement: {error}")))?;

        let mut components = BTreeMap::new();
        for (id, component) in raw.components {
            validate_nonempty_identifier("component id", &id)?;
            Version::parse(&component.version).map_err(|error| {
                SdkInventoryError::Invalid(format!("component `{id}` has invalid version: {error}"))
            })?;
            let providers = component
                .providers
                .into_iter()
                .map(|provider| normalize_provider(provider, root, &id, component.available))
                .collect::<Result<Vec<_>, _>>()?;
            components.insert(
                id.clone(),
                SdkComponent {
                    id,
                    version: component.version,
                    mandatory: component.mandatory,
                    available: component.available,
                    dependencies: component.dependencies.into_iter().collect(),
                    providers,
                },
            );
        }

        for component in components.values() {
            for dependency in &component.dependencies {
                if !components.contains_key(dependency) {
                    return Err(SdkInventoryError::Invalid(format!(
                        "component `{}` depends on unknown component `{dependency}`",
                        component.id
                    )));
                }
            }
        }

        let profiles = raw
            .profiles
            .into_iter()
            .map(|(profile, members)| (profile, members.into_iter().collect::<BTreeSet<_>>()))
            .collect::<BTreeMap<_, _>>();
        for (profile, members) in &profiles {
            validate_nonempty_identifier("profile name", profile)?;
            for component in members {
                if !components.contains_key(component) {
                    return Err(SdkInventoryError::Invalid(format!(
                        "profile `{profile}` references unknown component `{component}`"
                    )));
                }
            }
        }
        if !profiles.contains_key("default") {
            return Err(SdkInventoryError::Invalid(
                "component-aware SDK inventory must define a `default` profile".to_string(),
            ));
        }
        validate_component_graph(&components)?;

        Ok(Self {
            root: root.to_path_buf(),
            sdk_id: raw.sdk_id,
            sdk_version: raw.sdk_version,
            compiler_requirement: raw.compiler_requirement,
            components,
            profiles,
        })
    }

    /// Serialize this inventory with relocatable artifact paths relative to its installation root.
    pub fn to_json(&self) -> Result<String, SdkInventoryError> {
        let components = self
            .components
            .iter()
            .map(|(id, component)| {
                let providers = component
                    .providers
                    .iter()
                    .map(|provider| {
                        Ok(RawSdkProviderDescriptor {
                            name: provider.name.clone(),
                            version: provider.version.clone(),
                            digest: provider.digest.clone(),
                            namespace_claims: provider.namespace_claims.iter().cloned().collect(),
                            manifest_path: provider
                                .manifest_path
                                .as_deref()
                                .map(|path| relative_inventory_path(&self.root, path))
                                .transpose()?,
                            crate_root: provider
                                .crate_root
                                .as_deref()
                                .map(|path| relative_inventory_path(&self.root, path))
                                .transpose()?,
                        })
                    })
                    .collect::<Result<Vec<_>, SdkInventoryError>>()?;
                Ok((
                    id.clone(),
                    RawSdkComponent {
                        version: component.version.clone(),
                        mandatory: component.mandatory,
                        available: component.available,
                        dependencies: component.dependencies.iter().cloned().collect(),
                        providers,
                    },
                ))
            })
            .collect::<Result<BTreeMap<_, _>, SdkInventoryError>>()?;
        let raw = RawSdkInventory {
            schema_version: SDK_INVENTORY_SCHEMA_VERSION,
            sdk_id: self.sdk_id.clone(),
            sdk_version: self.sdk_version.clone(),
            compiler_requirement: self.compiler_requirement.clone(),
            components,
            profiles: self
                .profiles
                .iter()
                .map(|(name, members)| (name.clone(), members.iter().cloned().collect()))
                .collect(),
        };
        serde_json::to_string_pretty(&raw).map_err(|error| SdkInventoryError::Serialize(error.to_string()))
    }

    /// Write a validated, relocatable inventory to disk.
    pub fn write_to_path(&self, path: &Path) -> Result<(), SdkInventoryError> {
        let payload = self.to_json()?;
        fs::write(path, format!("{payload}\n")).map_err(|source| SdkInventoryError::Write {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Expand and validate component selection while retaining unavailable state for inspection.
    pub fn resolve_catalog(
        &self,
        selection: &SdkComponentSelection,
    ) -> Result<ResolvedSdkComponents, SdkResolutionError> {
        let sdk_identity = self.identity();
        let Some(profile_members) = self.profiles.get(&selection.profile) else {
            return Err(SdkResolutionError::UnknownProfile {
                profile: selection.profile.clone(),
                sdk_identity,
            });
        };
        for component in selection.components.iter().chain(selection.exclude_components.iter()) {
            if !self.components.contains_key(component) {
                return Err(SdkResolutionError::UnknownComponent {
                    component: component.clone(),
                    sdk_identity: self.identity(),
                });
            }
        }
        if let Some(component) = selection.components.intersection(&selection.exclude_components).next() {
            return Err(SdkResolutionError::SelectedComponentExcluded {
                component: component.clone(),
            });
        }
        for component in self.components.values().filter(|component| component.mandatory) {
            if selection.exclude_components.contains(&component.id) {
                return Err(SdkResolutionError::MandatoryComponentExcluded {
                    component: component.id.clone(),
                    sdk_identity: self.identity(),
                });
            }
        }

        let mut resolved = ResolvedSdkComponents {
            sdk_identity: self.identity(),
            profile: selection.profile.clone(),
            enabled: BTreeSet::new(),
            unavailable: BTreeSet::new(),
            reasons: BTreeMap::new(),
        };

        for component in self.components.values().filter(|component| component.mandatory) {
            self.enable_component(
                &component.id,
                ComponentSelectionReason::Mandatory,
                selection,
                &mut resolved,
                &mut Vec::new(),
            )?;
        }
        for component in profile_members {
            self.enable_component(
                component,
                ComponentSelectionReason::Profile {
                    profile: selection.profile.clone(),
                },
                selection,
                &mut resolved,
                &mut Vec::new(),
            )?;
        }
        for component in &selection.components {
            self.enable_component(
                component,
                ComponentSelectionReason::Explicit,
                selection,
                &mut resolved,
                &mut Vec::new(),
            )?;
            resolved
                .reasons
                .insert(component.clone(), ComponentSelectionReason::Explicit);
        }

        for component in &resolved.enabled {
            if self.components.get(component).is_some_and(|entry| !entry.available) {
                resolved.unavailable.insert(component.clone());
            }
        }
        Ok(resolved)
    }

    /// Resolve components for compilation, rejecting every enabled but unavailable artifact.
    pub fn resolve(&self, selection: &SdkComponentSelection) -> Result<ResolvedSdkComponents, SdkResolutionError> {
        let resolved = self.resolve_catalog(selection)?;
        if let Some(component) = resolved.unavailable.iter().next() {
            return Err(SdkResolutionError::EnabledComponentUnavailable {
                component: component.clone(),
                sdk_identity: resolved.sdk_identity.clone(),
            });
        }
        Ok(resolved)
    }

    /// Render the stable active SDK identity.
    pub fn identity(&self) -> String {
        format!("{}@{}", self.sdk_id, self.sdk_version)
    }

    /// Verify that this inventory supports the active compiler version.
    pub fn validate_compiler_version(&self, compiler_version: &str) -> Result<(), SdkInventoryError> {
        let requirement = VersionReq::parse(&self.compiler_requirement)
            .map_err(|error| SdkInventoryError::Invalid(format!("invalid compiler_requirement: {error}")))?;
        let compiler = Version::parse(compiler_version).map_err(|error| {
            SdkInventoryError::Invalid(format!("invalid compiler version `{compiler_version}`: {error}"))
        })?;
        if requirement.matches(&compiler) {
            Ok(())
        } else {
            Err(SdkInventoryError::Invalid(format!(
                "SDK {} requires compiler `{}`, but the active compiler is `{compiler_version}`",
                self.identity(),
                self.compiler_requirement
            )))
        }
    }

    /// Add one component and its transitive dependencies to the resolved selection.
    fn enable_component(
        &self,
        component: &str,
        reason: ComponentSelectionReason,
        selection: &SdkComponentSelection,
        resolved: &mut ResolvedSdkComponents,
        path: &mut Vec<String>,
    ) -> Result<(), SdkResolutionError> {
        path.push(component.to_string());
        if selection.exclude_components.contains(component) {
            let rendered_path = path.join(" -> ");
            path.pop();
            return Err(if rendered_path.contains(" -> ") {
                SdkResolutionError::ExcludedRequiredComponent {
                    component: component.to_string(),
                    path: rendered_path,
                }
            } else {
                SdkResolutionError::SelectedComponentExcluded {
                    component: component.to_string(),
                }
            });
        }
        let Some(entry) = self.components.get(component) else {
            path.pop();
            return Err(SdkResolutionError::UnknownComponent {
                component: component.to_string(),
                sdk_identity: self.identity(),
            });
        };

        let inserted = resolved.enabled.insert(component.to_string());
        if inserted {
            resolved.reasons.insert(component.to_string(), reason);
            for dependency in &entry.dependencies {
                self.enable_component(
                    dependency,
                    ComponentSelectionReason::Dependency {
                        required_by: component.to_string(),
                    },
                    selection,
                    resolved,
                    path,
                )?;
            }
        }
        path.pop();
        Ok(())
    }
}

/// Normalize and validate one raw provider artifact entry.
fn normalize_provider(
    provider: RawSdkProviderDescriptor,
    root: &Path,
    component: &str,
    available: bool,
) -> Result<SdkProviderDescriptor, SdkInventoryError> {
    validate_nonempty_identifier("provider name", &provider.name)?;
    Version::parse(&provider.version).map_err(|error| {
        SdkInventoryError::Invalid(format!(
            "provider `{}` in component `{component}` has invalid version: {error}",
            provider.name
        ))
    })?;
    if provider.digest.trim().is_empty() {
        return Err(SdkInventoryError::Invalid(format!(
            "provider `{}` in component `{component}` has an empty digest",
            provider.name
        )));
    }
    if available && (provider.manifest_path.is_none() || provider.crate_root.is_none()) {
        return Err(SdkInventoryError::Invalid(format!(
            "available provider `{}` in component `{component}` must declare manifest_path and crate_root",
            provider.name
        )));
    }
    let namespace_claims = provider
        .namespace_claims
        .into_iter()
        .map(|claim| validate_namespace_claim(&provider.name, claim))
        .collect::<Result<_, _>>()?;
    Ok(SdkProviderDescriptor {
        name: provider.name,
        version: provider.version,
        digest: provider.digest,
        namespace_claims,
        manifest_path: provider
            .manifest_path
            .map(|path| resolve_inventory_path(root, &path))
            .transpose()?,
        crate_root: provider
            .crate_root
            .map(|path| resolve_inventory_path(root, &path))
            .transpose()?,
    })
}

/// Resolve one inventory-authored path against the SDK root without canonicalizing absent optional components.
fn resolve_inventory_path(root: &Path, authored: &str) -> Result<PathBuf, SdkInventoryError> {
    let path = PathBuf::from(authored);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(SdkInventoryError::Invalid(format!(
            "artifact path `{authored}` must be relative to the SDK inventory and cannot contain `..`"
        )));
    }
    Ok(root.join(path))
}

/// Convert one installed artifact path back to a portable inventory path.
fn relative_inventory_path(root: &Path, path: &Path) -> Result<String, SdkInventoryError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        SdkInventoryError::Invalid(format!(
            "artifact path `{}` is outside SDK root `{}`",
            path.display(),
            root.display()
        ))
    })?;
    if relative.as_os_str().is_empty() {
        return Err(SdkInventoryError::Invalid(
            "artifact path cannot identify the SDK inventory root itself".to_string(),
        ));
    }
    Ok(relative.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"))
}

/// Validate and normalize one exact provider namespace claim.
fn validate_namespace_claim(provider: &str, claim: Vec<String>) -> Result<Vec<String>, SdkInventoryError> {
    if claim.is_empty() {
        return Err(SdkInventoryError::Invalid(format!(
            "provider `{provider}` contains an empty namespace claim"
        )));
    }
    for segment in &claim {
        validate_nonempty_identifier("namespace segment", segment)?;
    }
    Ok(claim)
}

/// Validate a stable SDK/component/profile/provider identifier.
fn validate_nonempty_identifier(kind: &str, value: &str) -> Result<(), SdkInventoryError> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(SdkInventoryError::Invalid(format!("{kind} cannot be empty")));
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || chars.any(|ch| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
    {
        return Err(SdkInventoryError::Invalid(format!(
            "{kind} `{value}` must use ASCII letters, digits, underscores, or hyphens"
        )));
    }
    Ok(())
}

/// Reject cycles in the complete SDK component graph.
fn validate_component_graph(components: &BTreeMap<String, SdkComponent>) -> Result<(), SdkInventoryError> {
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut stack = Vec::new();
    for component in components.keys() {
        visit_component(component, components, &mut visiting, &mut visited, &mut stack)?;
    }
    Ok(())
}

/// Depth-first cycle validation for one SDK component.
fn visit_component(
    component: &str,
    components: &BTreeMap<String, SdkComponent>,
    visiting: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
    stack: &mut Vec<String>,
) -> Result<(), SdkInventoryError> {
    if visited.contains(component) {
        return Ok(());
    }
    if visiting.contains(component) {
        let start = stack.iter().position(|entry| entry == component).unwrap_or(0);
        let mut cycle = stack[start..].to_vec();
        cycle.push(component.to_string());
        return Err(SdkInventoryError::ComponentCycle {
            path: cycle.join(" -> "),
        });
    }

    visiting.insert(component.to_string());
    stack.push(component.to_string());
    if let Some(entry) = components.get(component) {
        for dependency in &entry.dependencies {
            visit_component(dependency, components, visiting, visited, stack)?;
        }
    }
    stack.pop();
    visiting.remove(component);
    visited.insert(component.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const INVENTORY: &str = r#"
{
  "schema_version": 1,
  "sdk_id": "incan",
  "sdk_version": "0.5.0",
  "compiler_requirement": ">=0.5.0-dev.5,<0.6.0",
  "components": {
    "stdlib-core": {
      "version": "0.5.0",
      "mandatory": true,
      "available": true,
      "dependencies": [],
      "providers": []
    },
    "stdlib-data": {
      "version": "0.5.0",
      "mandatory": false,
      "available": true,
      "dependencies": ["stdlib-core"],
      "providers": []
    },
    "stdlib-web": {
      "version": "0.5.0",
      "mandatory": false,
      "available": false,
      "dependencies": ["stdlib-data"],
      "providers": []
    }
  },
  "profiles": {
    "minimal": ["stdlib-core"],
    "default": ["stdlib-core", "stdlib-data"],
    "full": ["stdlib-core", "stdlib-data", "stdlib-web"]
  }
}
"#;

    #[test]
    fn expands_profile_additions_and_component_dependencies() -> TestResult {
        let inventory = SdkInventory::from_json(INVENTORY, Path::new("/sdk"))?;
        let selection = SdkComponentSelection {
            profile: "minimal".to_string(),
            components: set(["stdlib-data"]),
            exclude_components: Default::default(),
        };
        let resolved = inventory.resolve(&selection)?;

        assert_eq!(resolved.enabled, set(["stdlib-core", "stdlib-data"]));
        assert!(resolved.unavailable.is_empty());
        assert!(matches!(
            resolved.reasons.get("stdlib-data"),
            Some(ComponentSelectionReason::Explicit)
        ));
        Ok(())
    }

    #[test]
    fn distinguishes_enabled_but_unavailable_components() -> TestResult {
        let inventory = SdkInventory::from_json(INVENTORY, Path::new("/sdk"))?;
        let selection = SdkComponentSelection {
            profile: "minimal".to_string(),
            components: set(["stdlib-web"]),
            exclude_components: Default::default(),
        };
        let catalog = inventory.resolve_catalog(&selection)?;

        assert_eq!(catalog.unavailable, set(["stdlib-web"]));
        let error = inventory
            .resolve(&selection)
            .err()
            .ok_or("expected unavailable component")?;
        assert!(matches!(error, SdkResolutionError::EnabledComponentUnavailable { .. }));
        Ok(())
    }

    #[test]
    fn reports_dependency_path_when_an_exclusion_breaks_selection() -> TestResult {
        let inventory = SdkInventory::from_json(INVENTORY, Path::new("/sdk"))?;
        let selection = SdkComponentSelection {
            profile: "minimal".to_string(),
            components: set(["stdlib-web"]),
            exclude_components: set(["stdlib-data"]),
        };
        let error = inventory
            .resolve_catalog(&selection)
            .err()
            .ok_or("expected invalid exclusion")?;

        assert!(matches!(error, SdkResolutionError::ExcludedRequiredComponent { .. }));
        assert!(error.to_string().contains("stdlib-web -> stdlib-data"));
        Ok(())
    }

    #[test]
    fn rejects_mandatory_component_exclusion() -> TestResult {
        let inventory = SdkInventory::from_json(INVENTORY, Path::new("/sdk"))?;
        let selection = SdkComponentSelection {
            profile: "minimal".to_string(),
            components: Default::default(),
            exclude_components: set(["stdlib-core"]),
        };
        let error = inventory
            .resolve_catalog(&selection)
            .err()
            .ok_or("expected mandatory exclusion")?;

        assert!(matches!(error, SdkResolutionError::MandatoryComponentExcluded { .. }));
        Ok(())
    }

    #[test]
    fn rejects_inventory_component_cycles() {
        let cyclic = INVENTORY.replace("\"dependencies\": [],", "\"dependencies\": [\"stdlib-data\"],");
        let error = SdkInventory::from_json(&cyclic, Path::new("/sdk"));

        assert!(matches!(error, Err(SdkInventoryError::ComponentCycle { .. })));
    }

    #[test]
    fn rejects_incompatible_compiler_version() -> TestResult {
        let inventory = SdkInventory::from_json(INVENTORY, Path::new("/sdk"))?;

        inventory.validate_compiler_version("0.5.0-dev.5")?;
        let error = inventory
            .validate_compiler_version("0.6.0")
            .err()
            .ok_or("expected incompatible compiler version")?;
        assert!(error.to_string().contains("requires compiler"));
        Ok(())
    }

    #[test]
    fn rejects_provider_paths_that_escape_the_inventory_root() {
        let with_provider = INVENTORY.replace(
            "\"providers\": []",
            r#""providers": [{
        "name": "stdlib_core",
        "version": "0.5.0",
        "digest": "sha256:fixture",
        "namespace_claims": [["std", "result"]],
        "manifest_path": "../outside.incnlib",
        "crate_root": "components/stdlib-core"
      }]"#,
        );

        assert!(matches!(
            SdkInventory::from_json(&with_provider, Path::new("/sdk")),
            Err(SdkInventoryError::Invalid(_))
        ));
    }

    #[test]
    fn command_profile_override_preserves_project_component_refinements() -> TestResult {
        let manifest = ProjectManifest::from_str(
            r#"
[project]
name = "profile_override"

[sdk]
profile = "default"
components = ["stdlib-web"]
exclude-components = ["stdlib-data"]
"#,
            Path::new("/project/incan.toml"),
        )?;

        let selection = SdkComponentSelection::from_manifest_with_profile_override(Some(&manifest), Some("minimal"));
        assert_eq!(selection.profile, "minimal");
        assert_eq!(selection.components, set(["stdlib-web"]));
        assert_eq!(selection.exclude_components, set(["stdlib-data"]));
        Ok(())
    }

    #[test]
    fn source_catalog_assigns_hashing_to_data_without_a_private_codecs_edge() -> TestResult {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("crates/incan_stdlib/stdlib")
            .join(SDK_SOURCE_CATALOG_FILE);
        let catalog = SdkSourceCatalog::read_from_path(&path)?;
        let data = catalog
            .components
            .get("stdlib-data")
            .ok_or("missing stdlib-data source component")?;

        assert!(data.build_dependencies.is_empty());
        assert!(!data.dependencies.contains("stdlib-codecs"));
        assert!(data.namespace_roots.contains("hash"));
        let codecs = catalog
            .components
            .get("stdlib-codecs")
            .ok_or("missing stdlib-codecs source component")?;
        assert!(!codecs.namespace_roots.contains("hash"));
        Ok(())
    }

    fn set<const N: usize>(values: [&str; N]) -> std::collections::BTreeSet<String> {
        values.into_iter().map(str::to_string).collect()
    }
}
