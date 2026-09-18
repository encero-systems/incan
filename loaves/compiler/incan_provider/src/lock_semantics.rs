//! The semantic half of `oven.lock`: the provider, SDK-component and package-feature state the lock command snapshots
//! from its provider plan, and the sealed-SDK semantic readings it keeps between runs. The lock model itself is
//! `oven_model::lock`; this module is what fills its [`SemanticLockState`] from Incan's provider facts.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use oven_model::digest::digest_bytes;
use oven_model::lock::{
    LockedFeatureEdge, LockedOvenState, LockedPackageFeatures, LockedProvider, LockedSdkComponent, LockedSdkState,
    SemanticLockState, portable_project_path,
};
use sha2::{Digest, Sha256};

use crate::{
    BackendImplementationRequirement, ComponentSelectionReason, PackageFeaturePlan, ProviderParticipation,
    ProviderPlan, ProviderProvenance, ProviderRecord, ResolvedSdkComponents, SdkInventory,
};
use incan_frontend::library_manifest::{
    ProviderSemanticToolchainDependency, digest_provider_semantic_artifact_with_context_and_cache,
    digest_toolchain_source_tree_with_cache,
};
use oven_model::manifest::{DependencySource, DependencySpec};
use oven_model::oven_interop::{InteropCSection, locked_interop_targets_from_section};

/// Layout version of the kept sealed-SDK semantic readings.
///
/// Bump this whenever the semantic digest itself changes meaning, so a compiler never reads a reading an earlier
/// one computed under different rules. Old directories are inert once nothing names them.
const SEALED_SEMANTIC_READING_LAYOUT: &str = "sealed-semantic-reading-v1";

/// Snapshot the shared provider, SDK-component, and package-feature plans into portable canonical lock state.
pub fn semantic_lock_state(
    project_root: &Path,
    interop: Option<&InteropCSection>,
    sdk_inventory: Option<&SdkInventory>,
    sdk_components: Option<&ResolvedSdkComponents>,
    package_features: Option<&PackageFeaturePlan>,
    provider_plan: &ProviderPlan,
    sdk_path_dependencies: &[DependencySpec],
) -> Result<SemanticLockState, String> {
    let provider_identity_map = provider_semantic_identities(provider_plan, sdk_path_dependencies)?;
    semantic_lock_state_from_provider_identities(
        project_root,
        interop,
        sdk_inventory,
        sdk_components,
        package_features,
        provider_plan,
        &provider_identity_map,
    )
}

/// Snapshot semantic lock state using identities already validated for this compilation session.
#[allow(clippy::too_many_arguments, reason = "Mirrors semantic_lock_state with an additional checked session proof")]
pub fn semantic_lock_state_with_provider_identities(
    project_root: &Path,
    interop: Option<&InteropCSection>,
    sdk_inventory: Option<&SdkInventory>,
    sdk_components: Option<&ResolvedSdkComponents>,
    package_features: Option<&PackageFeaturePlan>,
    provider_plan: &ProviderPlan,
    sdk_path_dependencies: &[DependencySpec],
    provider_identities: &CheckedProviderSemanticIdentities,
) -> Result<SemanticLockState, String> {
    let provider_identity_map = provider_identities.for_context(provider_plan, sdk_path_dependencies)?;
    semantic_lock_state_from_provider_identities(
        project_root,
        interop,
        sdk_inventory,
        sdk_components,
        package_features,
        provider_plan,
        provider_identity_map,
    )
}

/// Assemble lock state from an identity map after its owning boundary has checked plan/context binding.
fn semantic_lock_state_from_provider_identities(
    project_root: &Path,
    interop: Option<&InteropCSection>,
    sdk_inventory: Option<&SdkInventory>,
    sdk_components: Option<&ResolvedSdkComponents>,
    package_features: Option<&PackageFeaturePlan>,
    provider_plan: &ProviderPlan,
    provider_identity_map: &BTreeMap<String, String>,
) -> Result<SemanticLockState, String> {
    let interop = locked_interop_targets_from_section(project_root, interop)?;
    let oven = (!interop.is_empty()).then_some(LockedOvenState { interop });
    let provider_semantic_identities = provider_plan
        .records()
        .map(|provider| {
            let key = provider.identity.stable_key();
            let identity = provider_identity_map
                .get(&key)
                .cloned()
                .ok_or_else(|| format!("semantic provider identity is missing for `{key}`"))?;
            Ok((provider, identity))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let sdk = match (sdk_inventory, sdk_components) {
        (Some(inventory), Some(components)) => {
            let inventory_digest = semantic_sdk_inventory_digest(inventory, &provider_semantic_identities)?;
            let selected = components
                .enabled
                .iter()
                .filter_map(|id| {
                    let component = inventory.components.get(id)?;
                    let reason = components
                        .reasons
                        .get(id)
                        .map(component_reason)
                        .unwrap_or_else(|| "selected".into());
                    Some(LockedSdkComponent {
                        id: id.clone(),
                        version: component.version.clone(),
                        reason,
                    })
                })
                .collect();
            Some(LockedSdkState {
                identity: inventory.identity(),
                inventory_digest,
                profile: components.profile.clone(),
                components: selected,
            })
        }
        (None, None) => None,
        _ => return Err("SDK inventory and resolved component state must be recorded together".to_string()),
    };

    let packages = package_features
        .iter()
        .flat_map(|plan| plan.packages())
        .map(|package| LockedPackageFeatures {
            package: package.package_name.clone(),
            project_root: portable_project_path(project_root, &package.project_root),
            active_features: package.features.active_features.clone(),
            active_optional_dependencies: package.features.active_optional_dependencies.clone(),
            dependency_features: package.features.dependency_features.clone(),
            required_sdk_components: package.features.required_sdk_components.clone(),
        })
        .collect();
    let feature_edges = package_features
        .iter()
        .flat_map(|plan| plan.edges())
        .map(|edge| LockedFeatureEdge {
            from: portable_project_path(project_root, &edge.from),
            dependency_key: edge.dependency_key.clone(),
            to: portable_project_path(project_root, &edge.to),
            requested_features: edge.requested_features.clone(),
            default_features: edge.default_features,
            optional: edge.optional,
        })
        .collect();
    let providers = provider_semantic_identities
        .into_iter()
        .filter(|(provider, _)| provider.enabled)
        .map(|(provider, identity)| LockedProvider {
            identity,
            participation: participation_name(provider_plan.participation(provider)).to_string(),
            namespace_claims: provider.namespace_claims.clone(),
            used_modules: provider_plan.used_modules(provider),
            implementation_facets: provider_plan
                .linked_implementation_facets(provider)
                .into_iter()
                .map(|facet| facet.id.clone())
                .collect(),
            backend_requirements: provider_plan
                .linked_backend_requirements(provider)
                .iter()
                .map(backend_requirement_name)
                .collect(),
        })
        .collect();
    Ok(SemanticLockState {
        sdk,
        packages,
        feature_edges,
        providers,
        oven,
        workspace_members: Vec::new(),
    })
}

/// Return each checked provider's path-independent semantic identity keyed by its byte-exact catalog identity.
///
/// Provider-plan construction always validates the physical artifact digest before this projection is available.
/// Consumers may therefore use these values only where an approved relocation must preserve compatibility; they must
/// never replace the inventory's byte-exact integrity validation or authorize an unrecorded provider.
pub fn provider_semantic_identities(
    provider_plan: &ProviderPlan,
    sdk_path_dependencies: &[DependencySpec],
) -> Result<BTreeMap<String, String>, String> {
    let semantic_toolchain_dependencies = semantic_toolchain_dependencies(sdk_path_dependencies)?;
    provider_semantic_identities_with_dependencies(provider_plan, &semantic_toolchain_dependencies, None)
}

/// Project provider identities from support roots whose recursive content identities were already checked.
fn provider_semantic_identities_with_dependencies(
    provider_plan: &ProviderPlan,
    semantic_toolchain_dependencies: &[ProviderSemanticToolchainDependency],
    preliminary_context: Option<u64>,
) -> Result<BTreeMap<String, String>, String> {
    let dependency_semantic_digests = provider_dependency_semantic_digests_for_context(
        provider_plan,
        semantic_toolchain_dependencies,
        preliminary_context,
    )?;
    let mut provider_digest_cache = BTreeMap::new();
    provider_plan
        .records()
        .map(|provider| {
            Ok((
                provider.identity.stable_key(),
                locked_provider_semantic_identity(
                    provider,
                    &dependency_semantic_digests,
                    semantic_toolchain_dependencies,
                    &mut provider_digest_cache,
                )?,
            ))
        })
        .collect()
}

/// Provider semantic identities bound to the exact checked plan that produced them.
#[derive(Debug)]
pub struct CheckedProviderSemanticIdentities {
    context_key: String,
    identities: Arc<BTreeMap<String, String>>,
}

impl CheckedProviderSemanticIdentities {
    /// Borrow identities only after rechecking the exact provider and dependency context that produced them.
    pub fn for_context(
        &self,
        provider_plan: &ProviderPlan,
        sdk_path_dependencies: &[DependencySpec],
    ) -> Result<&BTreeMap<String, String>, String> {
        let (_, context_key) = checked_provider_semantic_context(provider_plan, sdk_path_dependencies)?;
        if context_key != self.context_key {
            return Err("provider semantic identities belong to a different checked provider context".to_string());
        }
        Ok(&self.identities)
    }
}

fn checked_provider_semantic_context(
    provider_plan: &ProviderPlan,
    sdk_path_dependencies: &[DependencySpec],
) -> Result<(Vec<ProviderSemanticToolchainDependency>, String), String> {
    // Construction validates and canonicalizes the immutable record set once; every session consumer must still
    // surface a retained construction error before using the process-local identity.
    provider_plan.semantic_projection_persistent_key()?;
    let semantic_toolchain_dependencies = semantic_toolchain_dependencies(sdk_path_dependencies)?;
    for provider in provider_plan.records() {
        let Some(artifact) = provider.artifact.as_ref() else {
            continue;
        };
        let observed = incan_frontend::library_manifest::digest_provider_artifact(&artifact.crate_root)
            .map_err(|error| error.to_string())?;
        if observed != provider.identity.digest {
            return Err(format!(
                "provider artifact `{}` changed after admission",
                provider.identity.stable_key()
            ));
        }
    }
    let plan_identity = provider_plan.semantic_projection_identity();
    let mut context_key = provider_semantic_digest_key(provider_plan, &semantic_toolchain_dependencies);
    context_key.push_str(&format!("\u{19}{plan_identity}"));
    Ok((semantic_toolchain_dependencies, context_key))
}

/// Session-bounded reuse of provider semantic identities after rechecking every mutable physical input.
#[derive(Debug, Default)]
pub struct ProviderSemanticIdentitySession {
    identities: Mutex<BTreeMap<String, Arc<CheckedProviderSemanticIdentities>>>,
}

impl ProviderSemanticIdentitySession {
    /// Return semantic identities for one checked plan and dependency context.
    ///
    /// Every request rehashes support roots and validates provider artifact bytes before consulting the session cache.
    /// The cache therefore removes only the repeated final semantic projection; it cannot turn recorded inventory
    /// claims into authority or conceal source, manifest, dependency-context, or byte changes.
    pub fn identities(
        &self,
        provider_plan: &ProviderPlan,
        sdk_path_dependencies: &[DependencySpec],
    ) -> Result<Arc<CheckedProviderSemanticIdentities>, String> {
        let (semantic_toolchain_dependencies, key) =
            checked_provider_semantic_context(provider_plan, sdk_path_dependencies)?;
        if let Some(identities) = self
            .identities
            .lock()
            .map_err(|_| "provider semantic identity session lock was poisoned".to_string())?
            .get(&key)
            .cloned()
        {
            return Ok(identities);
        }
        let identities = Arc::new(CheckedProviderSemanticIdentities {
            context_key: key.clone(),
            identities: Arc::new(provider_semantic_identities_with_dependencies(
                provider_plan,
                &semantic_toolchain_dependencies,
                None,
            )?),
        });
        let mut cached = self
            .identities
            .lock()
            .map_err(|_| "provider semantic identity session lock was poisoned".to_string())?;
        Ok(cached.entry(key).or_insert(identities).clone())
    }

    /// Return the number of checked input contexts retained by this session.
    #[cfg(test)]
    pub fn entry_count(&self) -> Result<usize, String> {
        self.identities
            .lock()
            .map(|identities| identities.len())
            .map_err(|_| "provider semantic identity session lock was poisoned".to_string())
    }
}

/// Project a physical provider record into its path-independent semantic lock identity.
///
/// Runtime catalog matching keeps the byte-exact provider digest. Only the lock projection substitutes a digest that
/// removes checked delivery paths from generated metadata while retaining source, API, dependency-content, and
/// feature changes.
fn locked_provider_semantic_identity(
    provider: &ProviderRecord,
    dependency_semantic_digests: &BTreeMap<String, String>,
    semantic_toolchain_dependencies: &[ProviderSemanticToolchainDependency],
    resolved_artifacts: &mut BTreeMap<PathBuf, String>,
) -> Result<String, String> {
    let digest = match (provider.manifest.as_deref(), provider.artifact.as_ref()) {
        (Some(manifest), Some(artifact)) => digest_provider_semantic_artifact_with_context_and_cache(
            &artifact.crate_root,
            &artifact.manifest_path,
            &artifact.cargo_toml_path,
            manifest,
            dependency_semantic_digests,
            semantic_toolchain_dependencies,
            resolved_artifacts,
        )
        .map_err(|error| error.to_string())?,
        _ if matches!(provider.provenance, ProviderProvenance::Sdk { .. }) && !provider.available => {
            "unavailable".to_string()
        }
        _ => provider.identity.digest.clone(),
    };
    let features = provider
        .identity
        .feature_projection
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!(
        "{}@{}#{}[{}]",
        provider.identity.name, provider.identity.version, digest, features
    ))
}

/// Resolve exact compiler-owned SDK support roots into path-independent content identities.
fn semantic_toolchain_dependencies(
    sdk_path_dependencies: &[DependencySpec],
) -> Result<Vec<ProviderSemanticToolchainDependency>, String> {
    let mut resolved_packages = BTreeMap::new();
    sdk_path_dependencies
        .iter()
        .filter_map(|dependency| {
            let DependencySource::Path { path } = &dependency.source else {
                return None;
            };
            let package_name = dependency
                .package
                .clone()
                .unwrap_or_else(|| dependency.crate_name.clone());
            // Generated provider artifacts carry their checked `.incnlib` identity and are hashed by the provider
            // graph below. Only compiler support Cargo packages need the separate recursive source closure.
            if path.join(format!("{package_name}.incnlib")).is_file() {
                return None;
            }
            Some(
                digest_toolchain_source_tree_with_cache(path, &mut resolved_packages)
                    .map(|content_digest| ProviderSemanticToolchainDependency {
                        crate_name: dependency.crate_name.clone(),
                        package_name,
                        artifact_root: path.clone(),
                        content_digest,
                    })
                    .map_err(|error| error.to_string()),
            )
        })
        .collect()
}

/// Precompute path-independent identities for every locally available physical provider digest.
#[cfg(test)]
fn provider_dependency_semantic_digests(
    provider_plan: &ProviderPlan,
    semantic_toolchain_dependencies: &[ProviderSemanticToolchainDependency],
) -> Result<BTreeMap<String, String>, String> {
    provider_dependency_semantic_digests_for_context(provider_plan, semantic_toolchain_dependencies, None)
}

/// Precompute dependency semantics under an immutable provider-plan identity.
fn provider_dependency_semantic_digests_for_context(
    provider_plan: &ProviderPlan,
    semantic_toolchain_dependencies: &[ProviderSemanticToolchainDependency],
    context: Option<u64>,
) -> Result<BTreeMap<String, String>, String> {
    provider_dependency_semantic_digests_observed_with_context(
        provider_plan,
        semantic_toolchain_dependencies,
        context,
        None,
    )
}

#[derive(Default)]
struct ProviderSemanticDigestCounters {
    preliminary_provider_digest_calls: usize,
    preliminary_memory_hits: usize,
    preliminary_sealed_hits: usize,
}

/// Run the preliminary physical-to-semantic map while optionally counting its actual digest/cache path.
#[cfg(test)]
fn provider_dependency_semantic_digests_observed(
    provider_plan: &ProviderPlan,
    semantic_toolchain_dependencies: &[ProviderSemanticToolchainDependency],
    counters: Option<&mut ProviderSemanticDigestCounters>,
) -> Result<BTreeMap<String, String>, String> {
    provider_dependency_semantic_digests_observed_with_context(
        provider_plan,
        semantic_toolchain_dependencies,
        None,
        counters,
    )
}

/// Run the preliminary map with its plan-bound memo key and optional measurement counters.
fn provider_dependency_semantic_digests_observed_with_context(
    provider_plan: &ProviderPlan,
    semantic_toolchain_dependencies: &[ProviderSemanticToolchainDependency],
    context: Option<u64>,
    mut counters: Option<&mut ProviderSemanticDigestCounters>,
) -> Result<BTreeMap<String, String>, String> {
    let persistent_key = provider_semantic_digest_key(provider_plan, semantic_toolchain_dependencies);
    let key = if let Some(context) = context {
        format!("{persistent_key}\u{19}{context}")
    } else {
        format!(
            "{persistent_key}{}",
            provider_plan.semantic_projection_persistent_key()?
        )
    };
    static DIGESTS: std::sync::OnceLock<std::sync::Mutex<BTreeMap<String, BTreeMap<String, String>>>> =
        std::sync::OnceLock::new();
    let memo = DIGESTS.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()));
    if let Ok(cached) = memo.lock()
        && let Some(digests) = cached.get(&key)
    {
        if let Some(counters) = counters.as_deref_mut() {
            counters.preliminary_memory_hits += 1;
        }
        return Ok(digests.clone());
    }
    // A process-local plan identity is sufficient only for this in-memory memo. Persisted readings must remain bound
    // to the complete canonical provider-plan key because numeric plan identities repeat in another process.
    let sealed = context
        .is_none()
        .then(|| sealed_reading_path(provider_plan, &key))
        .flatten();
    if let Some(path) = sealed.as_ref()
        && let Some(digests) = read_sealed_semantic_digests(path)
    {
        if let Some(counters) = counters.as_deref_mut() {
            counters.preliminary_sealed_hits += 1;
        }
        if let Ok(mut cached) = memo.lock() {
            cached.insert(key, digests.clone());
        }
        return Ok(digests);
    }
    let mut candidates = BTreeMap::<String, BTreeSet<String>>::new();
    let mut resolved_artifacts = BTreeMap::new();
    for provider in provider_plan.records() {
        let (Some(manifest), Some(artifact)) = (provider.manifest.as_deref(), provider.artifact.as_ref()) else {
            continue;
        };
        if let Some(counters) = counters.as_deref_mut() {
            counters.preliminary_provider_digest_calls += 1;
        }
        let semantic_digest = digest_provider_semantic_artifact_with_context_and_cache(
            &artifact.crate_root,
            &artifact.manifest_path,
            &artifact.cargo_toml_path,
            manifest,
            &BTreeMap::new(),
            semantic_toolchain_dependencies,
            &mut resolved_artifacts,
        )
        .map_err(|error| error.to_string())?;
        candidates
            .entry(provider.identity.digest.clone())
            .or_default()
            .insert(semantic_digest);
    }
    let digests = candidates
        .into_iter()
        .filter_map(|(physical, semantic)| {
            (semantic.len() == 1).then(|| semantic.into_iter().next().map(|semantic| (physical, semantic)))?
        })
        .collect::<BTreeMap<_, _>>();
    if let Some(path) = sealed.as_ref() {
        write_sealed_semantic_digests(path, &digests);
    }
    if let Ok(mut cached) = memo.lock() {
        cached.insert(key, digests.clone());
    }
    Ok(digests)
}

/// Where this reading may be kept between runs, when the reading describes only Incan's own sealed SDK.
///
/// A reading over project dependencies is not a candidate: those roots are ordinary source the author edits, and a
/// suite that bakes in temporary directories would leave one file behind per fixture. A reading over SDK providers
/// alone is the opposite — the SDK arrives prebuilt in a content-addressed directory, so there is exactly one such
/// reading per installed SDK and toolchain pairing, and it is correct until one of them is replaced.
fn sealed_reading_path(provider_plan: &ProviderPlan, key: &str) -> Option<PathBuf> {
    sealed_reading_path_under(sealed_reading_root()?, provider_plan, key)
}

/// Resolve the Incan home that keeps readings, matching how every other Incan-owned cache is placed.
fn sealed_reading_root() -> Option<PathBuf> {
    std::env::var_os("INCAN_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(path).join(".incan"))
        })
}

/// Place one reading under a given home, or refuse when the reading is not one that may be kept.
fn sealed_reading_path_under(root: PathBuf, provider_plan: &ProviderPlan, key: &str) -> Option<PathBuf> {
    let mut described_any = false;
    for provider in provider_plan.records() {
        if provider.manifest.is_none() || provider.artifact.is_none() {
            continue;
        }
        if !matches!(provider.provenance, ProviderProvenance::Sdk { .. }) {
            return None;
        }
        described_any = true;
    }
    if !described_any {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let name = format!("{:x}.json", hasher.finalize());
    Some(root.join("cache").join(SEALED_SEMANTIC_READING_LAYOUT).join(name))
}

/// Read one kept reading, treating anything unreadable as simply absent.
///
/// A cache may never fail a build. Every failure here — no file, a partial write from a killed process, a layout
/// this compiler does not understand — falls through to computing the reading, which is always correct.
fn read_sealed_semantic_digests(path: &Path) -> Option<BTreeMap<String, String>> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// Keep one reading for later runs, atomically, and give up silently if the filesystem will not take it.
///
/// The rename is what makes a concurrent reader safe: many baker processes share one store, and a reader either
/// sees the previous complete file or the new complete file, never a half-written one.
fn write_sealed_semantic_digests(path: &Path, digests: &BTreeMap<String, String>) {
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(encoded) = serde_json::to_vec(digests) else {
        return;
    };
    let staged = parent.join(format!(
        "{}.{}.staging",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    if fs::write(&staged, encoded).is_err() {
        let _ = fs::remove_file(&staged);
        return;
    }
    if fs::rename(&staged, path).is_err() {
        let _ = fs::remove_file(&staged);
    }
}

/// Name the exact inputs one semantic-digest pass would read.
///
/// The pass walks every present provider artifact and hashes its semantic content. Its answer depends on nothing
/// but which providers are in the plan — each already carrying the content digest the SDK recorded for it — and
/// the exact support-crate roots the toolchain resolved, each likewise carrying its own content digest. Two passes
/// agreeing on all of those are hashing the same bytes.
fn provider_semantic_digest_key(
    provider_plan: &ProviderPlan,
    semantic_toolchain_dependencies: &[ProviderSemanticToolchainDependency],
) -> String {
    let mut key = String::new();
    for provider in provider_plan.records() {
        let (Some(_), Some(artifact)) = (provider.manifest.as_deref(), provider.artifact.as_ref()) else {
            continue;
        };
        key.push('\u{1e}');
        key.push_str(&provider.identity.digest);
        key.push('\u{1d}');
        key.push_str(&artifact.crate_root.to_string_lossy());
    }
    for dependency in semantic_toolchain_dependencies {
        key.push('\u{1c}');
        key.push_str(&dependency.crate_name);
        key.push('\u{1d}');
        key.push_str(&dependency.package_name);
        key.push('\u{1d}');
        key.push_str(&dependency.content_digest);
        key.push('\u{1d}');
        key.push_str(&dependency.artifact_root.to_string_lossy());
    }
    key
}

/// Hash the relocatable SDK inventory after replacing physical provider digests with semantic lock identities.
fn semantic_sdk_inventory_digest(
    inventory: &SdkInventory,
    provider_semantic_identities: &[(&ProviderRecord, String)],
) -> Result<String, String> {
    let mut identities = BTreeMap::<(String, String, String), String>::new();
    for (provider, identity) in provider_semantic_identities {
        let ProviderProvenance::Sdk { component_id, .. } = &provider.provenance else {
            continue;
        };
        let key = (
            component_id.clone(),
            provider.identity.name.clone(),
            provider.identity.version.clone(),
        );
        if identities.insert(key.clone(), identity.clone()).is_some() {
            return Err(format!(
                "SDK component `{}` contains duplicate provider {}@{} while computing semantic inventory identity",
                key.0, key.1, key.2
            ));
        }
    }

    let payload = inventory.to_json().map_err(|error| error.to_string())?;
    let mut value: serde_json::Value = serde_json::from_str(&payload).map_err(|error| error.to_string())?;
    let components = value
        .get_mut("components")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| "serialized SDK inventory has no component map".to_string())?;
    for (component_id, component) in components {
        let Some(providers) = component.get_mut("providers").and_then(serde_json::Value::as_array_mut) else {
            continue;
        };
        for provider in providers {
            let Some(provider_object) = provider.as_object_mut() else {
                continue;
            };
            let Some(name) = provider_object
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            let Some(version) = provider_object
                .get("version")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            if let Some(identity) = identities.get(&(component_id.clone(), name, version)) {
                provider_object.insert("digest".to_string(), serde_json::Value::String(identity.clone()));
            }
        }
    }
    let normalized = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
    Ok(digest_bytes(&normalized))
}

/// Render one component-selection edge in the stable lockfile vocabulary.
fn component_reason(reason: &ComponentSelectionReason) -> String {
    match reason {
        ComponentSelectionReason::Mandatory => "mandatory".to_string(),
        ComponentSelectionReason::Profile { profile } => format!("profile:{profile}"),
        ComponentSelectionReason::Explicit => "explicit".to_string(),
        ComponentSelectionReason::Dependency { required_by } => format!("dependency:{required_by}"),
    }
}

/// Render provider availability, enablement, and use in the stable lockfile vocabulary.
fn participation_name(participation: ProviderParticipation) -> &'static str {
    match participation {
        ProviderParticipation::Unavailable => "unavailable",
        ProviderParticipation::Disabled => "disabled",
        ProviderParticipation::Enabled => "enabled",
        ProviderParticipation::Used => "used",
    }
}

/// Render one private provider implementation requirement in the stable lockfile vocabulary.
fn backend_requirement_name(requirement: &BackendImplementationRequirement) -> String {
    match requirement {
        BackendImplementationRequirement::CargoFeature { crate_name, feature } => {
            format!("cargo-feature:{crate_name}/{feature}")
        }
        BackendImplementationRequirement::CargoDependency { dependency } => {
            format!("cargo-dependency:{}", dependency.crate_name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use oven_model::lock::{
        CargoFeatureSelection, compute_resolved_fingerprint, compute_resolved_fingerprint_with_sdk_paths,
    };

    use crate::inventory::{extend_requirements_with_provider_plan, resolve_sdk_component_selection};
    use crate::requirements::ProjectRequirements;
    use crate::{
        ComponentSelectionReason, ProviderPlan, ProviderProvenance, ProviderRecord, ResolvedSdkComponents,
        SdkComponentSelection, SdkInventory,
    };
    use incan_frontend::library_manifest_index::LibraryManifestIndex;
    use oven_model::manifest::{DependencySource, DependencySpec};
    use oven_model::oven_interop::InteropCSection;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn sdk_path_spec(name: &str, path: &Path) -> DependencySpec {
        DependencySpec {
            crate_name: name.to_string(),
            version: None,
            features: Vec::new(),
            default_features: false,
            source: DependencySource::Path {
                path: path.to_path_buf(),
            },
            optional: false,
            package: None,
        }
    }

    /// Inputs that define one source-checkout-independent SDK semantic state.
    struct ProductionToolchainSemanticFixture {
        specs: Vec<DependencySpec>,
        provider_plan: ProviderPlan,
        inventory: SdkInventory,
        components: ResolvedSdkComponents,
    }

    #[derive(Debug)]
    struct ProviderSemanticMeasurement {
        fixture: &'static str,
        support_closure_roots_hashed: usize,
        support_closure_elapsed: Duration,
        physical_validation_calls: usize,
        physical_validation_elapsed: Duration,
        preliminary_provider_digest_calls: usize,
        preliminary_memory_hits: usize,
        preliminary_sealed_hits: usize,
        preliminary_elapsed: Duration,
        final_provider_digest_calls: usize,
        final_unique_artifacts: usize,
        final_elapsed: Duration,
        semantic_projection_elapsed: Duration,
    }

    impl ProviderSemanticMeasurement {
        fn render(&self) -> String {
            format!(
                "fixture={} support_closure_roots_hashed={} support_closure_us={} physical_validation_calls={} physical_validation_us={} preliminary_provider_digest_calls={} preliminary_memory_hits={} preliminary_sealed_hits={} preliminary_us={} final_provider_digest_calls={} final_unique_artifacts={} final_us={} semantic_projection_us={}",
                self.fixture,
                self.support_closure_roots_hashed,
                self.support_closure_elapsed.as_micros(),
                self.physical_validation_calls,
                self.physical_validation_elapsed.as_micros(),
                self.preliminary_provider_digest_calls,
                self.preliminary_memory_hits,
                self.preliminary_sealed_hits,
                self.preliminary_elapsed.as_micros(),
                self.final_provider_digest_calls,
                self.final_unique_artifacts,
                self.final_elapsed.as_micros(),
                self.semantic_projection_elapsed.as_micros(),
            )
        }
    }

    fn measurement_distribution(label: &str, mut values: Vec<u128>) -> Result<String, String> {
        if values.is_empty() {
            return Err(format!("the `{label}` distribution has no samples"));
        }
        values.sort_unstable();
        let median = values[values.len() / 2];
        Ok(format!(
            "{label}_min_us={} {label}_median_us={} {label}_max_us={}",
            values[0],
            median,
            values[values.len() - 1]
        ))
    }

    fn measure_provider_semantic_identity_pass(
        fixture: &'static str,
        provider_plan: &ProviderPlan,
        specs: &[DependencySpec],
    ) -> Result<(BTreeMap<String, String>, ProviderSemanticMeasurement), String> {
        let support_closure_started = Instant::now();
        let semantic_toolchain_dependencies = semantic_toolchain_dependencies(specs)?;
        let support_closure_elapsed = support_closure_started.elapsed();

        // Admission performs this byte-exact check before semantic projection. Keep it visible as a separate phase:
        // any future optimization must leave integrity validation intact even when a semantic reading is reusable.
        let physical_validation_started = Instant::now();
        let mut physical_validation_calls = 0;
        for provider in provider_plan.records() {
            let Some(artifact) = provider.artifact.as_ref() else {
                continue;
            };
            physical_validation_calls += 1;
            let observed = incan_frontend::library_manifest::digest_provider_artifact(&artifact.crate_root)
                .map_err(|error| error.to_string())?;
            if observed != provider.identity.digest {
                return Err(format!(
                    "physical provider digest changed during measurement for `{}`",
                    provider.identity.stable_key()
                ));
            }
        }
        let physical_validation_elapsed = physical_validation_started.elapsed();

        let preliminary_started = Instant::now();
        let mut preliminary_counters = ProviderSemanticDigestCounters::default();
        let dependency_semantic_digests = provider_dependency_semantic_digests_observed(
            provider_plan,
            &semantic_toolchain_dependencies,
            Some(&mut preliminary_counters),
        )?;
        let preliminary_elapsed = preliminary_started.elapsed();

        let final_started = Instant::now();
        let mut final_provider_digest_calls = 0;
        let mut provider_digest_cache = BTreeMap::new();
        let identities = provider_plan
            .records()
            .map(|provider| {
                if provider.manifest.is_some() && provider.artifact.is_some() {
                    final_provider_digest_calls += 1;
                }
                Ok((
                    provider.identity.stable_key(),
                    locked_provider_semantic_identity(
                        provider,
                        &dependency_semantic_digests,
                        &semantic_toolchain_dependencies,
                        &mut provider_digest_cache,
                    )?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        let final_elapsed = final_started.elapsed();

        Ok((
            identities,
            ProviderSemanticMeasurement {
                fixture,
                support_closure_roots_hashed: semantic_toolchain_dependencies.len(),
                support_closure_elapsed,
                physical_validation_calls,
                physical_validation_elapsed,
                preliminary_provider_digest_calls: preliminary_counters.preliminary_provider_digest_calls,
                preliminary_memory_hits: preliminary_counters.preliminary_memory_hits,
                preliminary_sealed_hits: preliminary_counters.preliminary_sealed_hits,
                preliminary_elapsed,
                final_provider_digest_calls,
                final_unique_artifacts: provider_digest_cache.len(),
                final_elapsed,
                semantic_projection_elapsed: support_closure_elapsed + preliminary_elapsed + final_elapsed,
            },
        ))
    }

    fn production_toolchain_semantic_fixture(
        checkout: &Path,
    ) -> Result<ProductionToolchainSemanticFixture, Box<dyn std::error::Error>> {
        production_toolchain_semantic_fixture_with_native_output(checkout, "pub fn support() {}\n", false, 'a')
    }

    /// Build one SDK fixture whose physical provider output can vary independently of its authored semantic input.
    fn production_toolchain_semantic_fixture_with_native_output(
        checkout: &Path,
        generated_source: &str,
        host_abi: bool,
        source_digest_digit: char,
    ) -> Result<ProductionToolchainSemanticFixture, Box<dyn std::error::Error>> {
        let derive_root = checkout.join("crates/incan_derive");
        fs::create_dir_all(derive_root.join("src"))?;
        fs::write(
            derive_root.join("Cargo.toml"),
            "[package]\nname = \"incan_derive\"\nversion = \"0.5.0\"\n",
        )?;
        fs::write(derive_root.join("src/lib.rs"), "pub fn derive_marker() {}\n")?;

        let provider_root = checkout.join("sdk/components/support-provider");
        fs::create_dir_all(provider_root.join("src"))?;
        fs::write(
            provider_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"support_provider\"\nversion = \"0.5.0\"\n\n[dependencies]\nincan_derive = {{ path = \"{}\" }}\n",
                derive_root.display()
            ),
        )?;
        fs::write(provider_root.join("src/lib.rs"), generated_source)?;
        let manifest_path = provider_root.join("support_provider.incnlib");
        let mut manifest = incan_frontend::library_manifest::LibraryManifest::new("support_provider", "0.5.0");
        manifest.contract_metadata.provider.semantic_source_digest =
            Some(format!("sha256:{}", source_digest_digit.to_string().repeat(64)));
        if host_abi {
            manifest.rust_abi = Some(incan_frontend::library_manifest::LibraryRustAbi {
                schema_version: incan_frontend::library_manifest::RUST_ABI_SCHEMA_VERSION,
                items: Vec::new(),
            });
        }
        manifest.contract_metadata.provider.implementation_facets.push(
            incan_frontend::library_manifest::ProviderImplementationFacet {
                id: "derive-support".to_string(),
                required_modules: BTreeSet::new(),
                required_features: BTreeSet::new(),
                cargo_features: BTreeMap::new(),
                cargo_dependencies: vec![incan_frontend::library_manifest::ProviderCargoDependency {
                    crate_name: "incan_derive".to_string(),
                    package: None,
                    version: None,
                    features: BTreeSet::new(),
                    default_features: false,
                    source: incan_frontend::library_manifest::ProviderCargoDependencySource::Toolchain {
                        relative_path: "crates/incan_derive".to_string(),
                    },
                }],
            },
        );
        manifest.write_to_path(&manifest_path)?;
        let physical_digest = incan_frontend::library_manifest::digest_provider_artifact(&provider_root)?;
        let artifact = incan_frontend::library_manifest_index::LibraryArtifactMetadata::from_manifest_path(
            "support_provider",
            "support_provider",
            manifest_path.clone(),
            provider_root.clone(),
        );
        let provider = ProviderRecord {
            identity: crate::ProviderIdentity {
                name: "support_provider".to_string(),
                version: "0.5.0".to_string(),
                digest: physical_digest.clone(),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::Sdk {
                sdk_identity: "incan@0.5.0".to_string(),
                component_id: "support".to_string(),
                inventory_path: None,
            },
            authority: crate::NamespaceAuthority::SdkReserved,
            namespace_claims: BTreeSet::new(),
            available: true,
            enabled: true,
            manifest: Some(std::sync::Arc::new(manifest)),
            artifact: Some(artifact),
            implementation_facets: Vec::new(),
        };
        let provider_plan = ProviderPlan::new(
            incan_frontend::library_manifest_index::LibraryManifestIndex::default(),
            vec![provider],
            std::iter::empty::<Vec<String>>(),
        )?;
        let inventory = SdkInventory {
            root: checkout.join("sdk"),
            sdk_id: "incan".to_string(),
            sdk_version: "0.5.0".to_string(),
            compiler_requirement: "^0.5".to_string(),
            provider_codegen_revision: incan_lang::version::SDK_PROVIDER_CODEGEN_REVISION,
            components: BTreeMap::from([(
                "support".to_string(),
                crate::SdkComponent {
                    id: "support".to_string(),
                    version: "0.5.0".to_string(),
                    mandatory: false,
                    available: true,
                    dependencies: BTreeSet::new(),
                    providers: vec![crate::SdkProviderDescriptor {
                        name: "support_provider".to_string(),
                        version: "0.5.0".to_string(),
                        digest: physical_digest,
                        namespace_claims: BTreeSet::new(),
                        manifest_path: Some(manifest_path),
                        crate_root: Some(provider_root),
                    }],
                },
            )]),
            profiles: BTreeMap::from([("default".to_string(), BTreeSet::from(["support".to_string()]))]),
        };
        let components = ResolvedSdkComponents {
            sdk_identity: "incan@0.5.0".to_string(),
            profile: "default".to_string(),
            enabled: BTreeSet::from(["support".to_string()]),
            unavailable: BTreeSet::new(),
            reasons: BTreeMap::from([(
                "support".to_string(),
                ComponentSelectionReason::Profile {
                    profile: "default".to_string(),
                },
            )]),
        };
        Ok(ProductionToolchainSemanticFixture {
            specs: vec![sdk_path_spec("incan_derive", &derive_root)],
            provider_plan,
            inventory,
            components,
        })
    }

    /// Add a second provider that depends on the same compiler support crate.
    ///
    /// The measured consumer therefore reaches two provider branches that converge on one support root. This is the
    /// smallest representative diamond for distinguishing repeated support traversal from provider-local hashing.
    fn shared_provider_diamond_fixture(
        checkout: &Path,
    ) -> Result<ProductionToolchainSemanticFixture, Box<dyn std::error::Error>> {
        let mut fixture = production_toolchain_semantic_fixture(checkout)?;
        let derive_root = checkout.join("crates/incan_derive");
        let sibling_root = checkout.join("sdk/components/support-provider-sibling");
        fs::create_dir_all(sibling_root.join("src"))?;
        fs::write(
            sibling_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"support_provider_sibling\"\nversion = \"0.5.0\"\n\n[dependencies]\nincan_derive = {{ path = \"{}\" }}\n",
                derive_root.display()
            ),
        )?;
        fs::write(sibling_root.join("src/lib.rs"), "pub fn sibling_support() {}\n")?;

        let original = fixture
            .provider_plan
            .records()
            .next()
            .cloned()
            .ok_or("the production fixture must contain its provider")?;
        let mut sibling_manifest = original
            .manifest
            .as_deref()
            .cloned()
            .ok_or("the production fixture provider must contain a manifest")?;
        sibling_manifest.name = "support_provider_sibling".to_string();
        let sibling_manifest_path = sibling_root.join("support_provider_sibling.incnlib");
        sibling_manifest.write_to_path(&sibling_manifest_path)?;
        let sibling_digest = incan_frontend::library_manifest::digest_provider_artifact(&sibling_root)?;
        let sibling = ProviderRecord {
            identity: crate::ProviderIdentity {
                name: sibling_manifest.name.clone(),
                version: sibling_manifest.version.clone(),
                digest: sibling_digest,
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::Sdk {
                sdk_identity: "incan@0.5.0".to_string(),
                component_id: "support-sibling".to_string(),
                inventory_path: None,
            },
            authority: crate::NamespaceAuthority::SdkReserved,
            namespace_claims: BTreeSet::new(),
            available: true,
            enabled: true,
            manifest: Some(std::sync::Arc::new(sibling_manifest)),
            artifact: Some(
                incan_frontend::library_manifest_index::LibraryArtifactMetadata::from_manifest_path(
                    "support_provider_sibling",
                    "support_provider_sibling",
                    sibling_manifest_path,
                    sibling_root,
                ),
            ),
            implementation_facets: Vec::new(),
        };
        fixture.provider_plan = ProviderPlan::new(
            incan_frontend::library_manifest_index::LibraryManifestIndex::default(),
            vec![original, sibling],
            std::iter::empty::<Vec<String>>(),
        )?;
        Ok(fixture)
    }

    #[test]
    #[ignore = "serial #1633 measurement; run explicitly on a quiet host"]
    fn measures_provider_semantic_identity_phases_issue1633() -> TestResult {
        let temp = tempfile::tempdir()?;
        let checked = production_toolchain_semantic_fixture(&temp.path().join("checked-fixture"))?;
        let diamond = shared_provider_diamond_fixture(&temp.path().join("shared-provider-diamond"))?;
        assert_eq!(diamond.provider_plan.records().count(), 2);
        assert_eq!(
            diamond.specs.len(),
            1,
            "both provider branches must converge on one support root"
        );

        for (name, fixture) in [("checked", checked), ("shared-provider-diamond", diamond)] {
            let (cold_identities, cold) =
                measure_provider_semantic_identity_pass(name, &fixture.provider_plan, &fixture.specs)?;
            let (warm_identities, warm) =
                measure_provider_semantic_identity_pass(name, &fixture.provider_plan, &fixture.specs)?;
            assert_eq!(
                cold_identities, warm_identities,
                "measurement passes must preserve identity results"
            );
            assert_eq!(
                cold_identities,
                provider_semantic_identities(&fixture.provider_plan, &fixture.specs)?,
                "instrumentation must reproduce the production projection"
            );
            eprintln!("provider-semantic-measurement phase=first-pass {}", cold.render());
            eprintln!("provider-semantic-measurement phase=repeated-pass {}", warm.render());
        }
        Ok(())
    }

    const REPRESENTATIVE_SDK_INVENTORY_ENV: &str = "INCAN_PROVIDER_SEMANTIC_MEASURE_SDK_INVENTORY";
    const REPRESENTATIVE_MEASUREMENT_REPEATS: usize = 9;

    fn load_verified_sdk_measurement_inputs(
        inventory_path: &Path,
    ) -> Result<(ProviderPlan, Vec<DependencySpec>, Duration), Box<dyn std::error::Error>> {
        let admission_started = Instant::now();
        let inventory = SdkInventory::read_from_path(inventory_path)?;
        inventory.validate_compiler_compatibility(
            incan_lang::version::INCAN_VERSION,
            incan_lang::version::SDK_PROVIDER_CODEGEN_REVISION,
        )?;
        let components =
            resolve_sdk_component_selection(&inventory, &SdkComponentSelection::default(), None, None, true)?;
        let provider_plan = ProviderPlan::from_resolved_inputs(
            LibraryManifestIndex::default(),
            None,
            Some(&inventory),
            Some(&components),
            std::iter::empty::<Vec<String>>(),
        )?;
        let mut requirements = ProjectRequirements::default();
        extend_requirements_with_provider_plan(&mut requirements, &provider_plan)?;
        Ok((
            provider_plan,
            requirements.sdk_path_dependencies,
            admission_started.elapsed(),
        ))
    }

    /// Measure the former library-shaped pair of complete semantic identity requests.
    fn measure_uncached_identity_pair(
        provider_plan: &ProviderPlan,
        specs: &[DependencySpec],
    ) -> Result<(BTreeMap<String, String>, Duration), String> {
        let started = Instant::now();
        let first = provider_semantic_identities(provider_plan, specs)?;
        let second = provider_semantic_identities(provider_plan, specs)?;
        assert_eq!(first, second);
        Ok((first, started.elapsed()))
    }

    /// Measure one session projection plus the two checked consumer validations that replace the former pair.
    fn measure_session_identity_pair(
        provider_plan: &ProviderPlan,
        specs: &[DependencySpec],
    ) -> Result<(BTreeMap<String, String>, Duration, Duration, Duration), String> {
        let session = ProviderSemanticIdentitySession::default();
        let session_started = Instant::now();
        let checked = session.identities(provider_plan, specs)?;
        let session_elapsed = session_started.elapsed();
        let first_consumer_started = Instant::now();
        let first = checked.for_context(provider_plan, specs)?.clone();
        let first_consumer_elapsed = first_consumer_started.elapsed();
        let second_consumer_started = Instant::now();
        let second = checked.for_context(provider_plan, specs)?;
        let second_consumer_elapsed = second_consumer_started.elapsed();
        assert_eq!(&first, second);
        Ok((first, session_elapsed, first_consumer_elapsed, second_consumer_elapsed))
    }

    #[test]
    #[ignore = "serial #1633 representative SDK measurement; requires an explicit verified inventory"]
    fn measures_current_verified_sdk_provider_semantics_issue1633() -> TestResult {
        let inventory_path = env::var_os(REPRESENTATIVE_SDK_INVENTORY_ENV)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| format!("set {REPRESENTATIVE_SDK_INVENTORY_ENV} to the verified SDK inventory path"))?;
        if !inventory_path.is_file() {
            return Err(format!(
                "{REPRESENTATIVE_SDK_INVENTORY_ENV} points to missing inventory {}",
                inventory_path.display()
            )
            .into());
        }

        let mut admission_samples = Vec::with_capacity(REPRESENTATIVE_MEASUREMENT_REPEATS);
        let mut support_samples = Vec::with_capacity(REPRESENTATIVE_MEASUREMENT_REPEATS);
        let mut physical_samples = Vec::with_capacity(REPRESENTATIVE_MEASUREMENT_REPEATS);
        let mut preliminary_samples = Vec::with_capacity(REPRESENTATIVE_MEASUREMENT_REPEATS);
        let mut final_samples = Vec::with_capacity(REPRESENTATIVE_MEASUREMENT_REPEATS);
        let mut semantic_projection_samples = Vec::with_capacity(REPRESENTATIVE_MEASUREMENT_REPEATS);
        let mut expected_identities = None;
        let mut production_check_inputs = None;

        for repeat in 0..REPRESENTATIVE_MEASUREMENT_REPEATS {
            let (provider_plan, specs, admission_elapsed) = load_verified_sdk_measurement_inputs(&inventory_path)?;
            let provider_count = provider_plan.records().count();
            if provider_count < 2 {
                return Err(format!(
                    "representative SDK measurement requires multiple providers, but the default profile admitted {provider_count}"
                )
                .into());
            }
            let (identities, measurement) =
                measure_provider_semantic_identity_pass("verified-sdk", &provider_plan, &specs)?;
            if let Some(expected) = expected_identities.as_ref() {
                assert_eq!(
                    expected, &identities,
                    "repeated SDK measurements must preserve identities"
                );
            } else {
                expected_identities = Some(identities);
            }
            admission_samples.push(admission_elapsed.as_micros());
            support_samples.push(measurement.support_closure_elapsed.as_micros());
            physical_samples.push(measurement.physical_validation_elapsed.as_micros());
            preliminary_samples.push(measurement.preliminary_elapsed.as_micros());
            final_samples.push(measurement.final_elapsed.as_micros());
            semantic_projection_samples.push(measurement.semantic_projection_elapsed.as_micros());
            production_check_inputs = Some((provider_plan, specs));
            eprintln!(
                "provider-semantic-measurement repeat={} providers={} admission_us={} {}",
                repeat + 1,
                provider_count,
                admission_elapsed.as_micros(),
                measurement.render()
            );
        }

        eprintln!(
            "provider-semantic-measurement-summary repeats={} {} {} {} {} {} {}",
            REPRESENTATIVE_MEASUREMENT_REPEATS,
            measurement_distribution("admission", admission_samples)?,
            measurement_distribution("support_closure", support_samples)?,
            measurement_distribution("physical_validation", physical_samples)?,
            measurement_distribution("preliminary", preliminary_samples)?,
            measurement_distribution("final", final_samples)?,
            measurement_distribution("semantic_projection", semantic_projection_samples)?,
        );
        let (provider_plan, specs) = production_check_inputs.ok_or("representative measurement produced no inputs")?;
        let production_identities = provider_semantic_identities(&provider_plan, &specs)?;
        assert_eq!(
            expected_identities.as_ref(),
            Some(&production_identities),
            "representative instrumentation must preserve the production result"
        );
        Ok(())
    }

    #[test]
    #[ignore = "serial #1633 before/after measurement; requires an explicit verified inventory"]
    fn measures_verified_sdk_session_reuse_issue1633() -> TestResult {
        let inventory_path = env::var_os(REPRESENTATIVE_SDK_INVENTORY_ENV)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| format!("set {REPRESENTATIVE_SDK_INVENTORY_ENV} to the verified SDK inventory path"))?;
        if !inventory_path.is_file() {
            return Err(format!(
                "{REPRESENTATIVE_SDK_INVENTORY_ENV} points to missing inventory {}",
                inventory_path.display()
            )
            .into());
        }

        let mut admission_samples = Vec::with_capacity(REPRESENTATIVE_MEASUREMENT_REPEATS);
        let mut uncached_pair_samples = Vec::with_capacity(REPRESENTATIVE_MEASUREMENT_REPEATS);
        let mut session_identity_samples = Vec::with_capacity(REPRESENTATIVE_MEASUREMENT_REPEATS);
        let mut first_consumer_samples = Vec::with_capacity(REPRESENTATIVE_MEASUREMENT_REPEATS);
        let mut second_consumer_samples = Vec::with_capacity(REPRESENTATIVE_MEASUREMENT_REPEATS);
        let mut optimized_pair_samples = Vec::with_capacity(REPRESENTATIVE_MEASUREMENT_REPEATS);
        let mut preliminary_prewarm_samples = Vec::with_capacity(REPRESENTATIVE_MEASUREMENT_REPEATS);
        let mut uncached_total_samples = Vec::with_capacity(REPRESENTATIVE_MEASUREMENT_REPEATS);
        let mut optimized_total_samples = Vec::with_capacity(REPRESENTATIVE_MEASUREMENT_REPEATS);

        for repeat in 0..REPRESENTATIVE_MEASUREMENT_REPEATS {
            let (provider_plan, specs, admission_elapsed) = load_verified_sdk_measurement_inputs(&inventory_path)?;
            if provider_plan.records().count() < 2 {
                return Err("representative reuse measurement requires multiple admitted providers".into());
            }

            // Populate only the preliminary map before comparing the two final-projection shapes. The baseline still
            // performs two complete public API requests, matching the former library preparation path.
            let semantic_dependencies = semantic_toolchain_dependencies(&specs)?;
            let prewarm_started = Instant::now();
            let _ = provider_dependency_semantic_digests(&provider_plan, &semantic_dependencies)?;
            let prewarm_elapsed = prewarm_started.elapsed();
            let (uncached_first, uncached_elapsed, optimized) = if repeat % 2 == 0 {
                let (uncached, elapsed) = measure_uncached_identity_pair(&provider_plan, &specs)?;
                let optimized = measure_session_identity_pair(&provider_plan, &specs)?;
                (uncached, elapsed, optimized)
            } else {
                let optimized = measure_session_identity_pair(&provider_plan, &specs)?;
                let (uncached, elapsed) = measure_uncached_identity_pair(&provider_plan, &specs)?;
                (uncached, elapsed, optimized)
            };
            let (optimized_identities, session_elapsed, first_consumer_elapsed, second_consumer_elapsed) = optimized;
            assert_eq!(
                optimized_identities, uncached_first,
                "optimized and uncached identities must be exact peers"
            );

            let optimized_elapsed = session_elapsed + first_consumer_elapsed + second_consumer_elapsed;
            admission_samples.push(admission_elapsed.as_micros());
            uncached_pair_samples.push(uncached_elapsed.as_micros());
            session_identity_samples.push(session_elapsed.as_micros());
            first_consumer_samples.push(first_consumer_elapsed.as_micros());
            second_consumer_samples.push(second_consumer_elapsed.as_micros());
            optimized_pair_samples.push(optimized_elapsed.as_micros());
            preliminary_prewarm_samples.push(prewarm_elapsed.as_micros());
            uncached_total_samples.push((admission_elapsed + uncached_elapsed).as_micros());
            optimized_total_samples.push((admission_elapsed + optimized_elapsed).as_micros());
            eprintln!(
                "provider-semantic-reuse repeat={} admission_us={} preliminary_prewarm_us={} warm_uncached_pair_us={} session_identity_us={} first_consumer_validation_us={} second_consumer_validation_us={} warm_optimized_pair_us={} admission_plus_warm_uncached_us={} admission_plus_warm_optimized_us={}",
                repeat + 1,
                admission_elapsed.as_micros(),
                prewarm_elapsed.as_micros(),
                uncached_elapsed.as_micros(),
                session_elapsed.as_micros(),
                first_consumer_elapsed.as_micros(),
                second_consumer_elapsed.as_micros(),
                optimized_elapsed.as_micros(),
                (admission_elapsed + uncached_elapsed).as_micros(),
                (admission_elapsed + optimized_elapsed).as_micros(),
            );
        }

        eprintln!(
            "provider-semantic-reuse-summary repeats={} {} {} {} {} {} {} {} {} {}",
            REPRESENTATIVE_MEASUREMENT_REPEATS,
            measurement_distribution("admission", admission_samples)?,
            measurement_distribution("preliminary_prewarm", preliminary_prewarm_samples)?,
            measurement_distribution("warm_uncached_pair", uncached_pair_samples)?,
            measurement_distribution("session_identity", session_identity_samples)?,
            measurement_distribution("first_consumer_validation", first_consumer_samples)?,
            measurement_distribution("second_consumer_validation", second_consumer_samples)?,
            measurement_distribution("warm_optimized_pair", optimized_pair_samples)?,
            measurement_distribution("admission_plus_warm_uncached", uncached_total_samples)?,
            measurement_distribution("admission_plus_warm_optimized", optimized_total_samples)?,
        );
        Ok(())
    }

    #[test]
    fn sdk_toolchain_fingerprint_tracks_content_not_source_checkout_path_issue921() -> TestResult {
        let temp = tempfile::tempdir()?;
        let first_checkout = temp.path().join("source-checkout-a");
        let second_checkout = temp.path().join("source-checkout-b");
        let ProductionToolchainSemanticFixture {
            specs: first_specs,
            provider_plan: first_plan,
            inventory: first_inventory,
            components: first_components,
        } = production_toolchain_semantic_fixture(&first_checkout)?;
        let ProductionToolchainSemanticFixture {
            specs: second_specs,
            provider_plan: second_plan,
            inventory: second_inventory,
            components: second_components,
        } = production_toolchain_semantic_fixture(&second_checkout)?;
        let first_semantic = semantic_lock_state(
            &first_checkout,
            None,
            Some(&first_inventory),
            Some(&first_components),
            None,
            &first_plan,
            &first_specs,
        )?;
        let second_semantic = semantic_lock_state(
            &second_checkout,
            None,
            Some(&second_inventory),
            Some(&second_components),
            None,
            &second_plan,
            &second_specs,
        )?;
        assert_eq!(first_semantic, second_semantic);
        let selection = CargoFeatureSelection::default();
        let first_fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            &first_specs,
            &[],
            &selection,
            Some(&first_checkout),
            &first_semantic,
            &first_specs,
        );
        let second_fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            &second_specs,
            &[],
            &selection,
            Some(&second_checkout),
            &second_semantic,
            &second_specs,
        );
        assert_eq!(first_fingerprint, second_fingerprint);

        fs::write(
            second_checkout.join("crates/incan_derive/src/lib.rs"),
            "pub fn derive_marker() { changed(); }\n",
        )?;
        let changed_semantic = semantic_lock_state(
            &second_checkout,
            None,
            Some(&second_inventory),
            Some(&second_components),
            None,
            &second_plan,
            &second_specs,
        )?;
        assert_ne!(second_semantic, changed_semantic);
        let changed_fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            &second_specs,
            &[],
            &selection,
            Some(&second_checkout),
            &changed_semantic,
            &second_specs,
        );
        assert_ne!(second_fingerprint, changed_fingerprint);
        Ok(())
    }

    #[test]
    fn a_sealed_sdk_reading_is_kept_between_runs_and_a_partial_one_reads_as_absent() -> TestResult {
        // Keeping a reading is only correct for providers that arrive prebuilt, and a kept reading may never fail
        // a build: anything unreadable has to look like nothing was kept at all.
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("incan-home");
        let fixture = production_toolchain_semantic_fixture(&temp.path().join("sealed"))?;

        let Some(kept) = sealed_reading_path_under(home.clone(), &fixture.provider_plan, "reading-key") else {
            return Err("an SDK-only reading must be keepable".into());
        };
        assert!(kept.starts_with(&home), "a kept reading belongs under the Incan home");
        assert_eq!(
            Some(kept.clone()),
            sealed_reading_path_under(home.clone(), &fixture.provider_plan, "reading-key"),
            "one reading must always land on one path"
        );
        assert_ne!(
            Some(kept.clone()),
            sealed_reading_path_under(home, &fixture.provider_plan, "another-key"),
            "a different reading must land somewhere else"
        );

        let digests = BTreeMap::from([("sha256:physical".to_string(), "sha256:semantic".to_string())]);
        write_sealed_semantic_digests(&kept, &digests);
        assert_eq!(read_sealed_semantic_digests(&kept), Some(digests));

        fs::write(&kept, b"{ not json")?;
        assert_eq!(
            read_sealed_semantic_digests(&kept),
            None,
            "a partial write from a killed process must read as absent, not as an error"
        );
        assert_eq!(read_sealed_semantic_digests(&temp.path().join("absent.json")), None);
        Ok(())
    }

    #[test]
    fn repeating_a_semantic_identity_pass_answers_from_the_first_one() -> TestResult {
        // The pass hashes every present provider artifact, and a bake asks for it twice. Repeating it must answer
        // identically, and a relocated SDK must not be served the earlier reading: its provider artifacts sit at
        // different roots, which is exactly what the surrounding relocation test distinguishes.
        let temp = tempfile::tempdir()?;
        let sealed = production_toolchain_semantic_fixture(&temp.path().join("sealed"))?;
        let relocated = production_toolchain_semantic_fixture(&temp.path().join("relocated"))?;

        let first = provider_semantic_identities(&sealed.provider_plan, &sealed.specs)?;
        let repeated = provider_semantic_identities(&sealed.provider_plan, &sealed.specs)?;
        assert_eq!(first, repeated, "a repeated pass over one plan must answer identically");

        let elsewhere = provider_semantic_identities(&relocated.provider_plan, &relocated.specs)?;
        assert_ne!(
            first.keys().collect::<Vec<_>>(),
            elsewhere.keys().collect::<Vec<_>>(),
            "a second SDK root is a separate reading, not a repeat of the first"
        );
        Ok(())
    }

    #[test]
    fn one_semantic_identity_session_reuses_only_an_unchanged_checked_context() -> TestResult {
        let temp = tempfile::tempdir()?;
        let fixture = production_toolchain_semantic_fixture(&temp.path().join("session-reuse"))?;
        let session = ProviderSemanticIdentitySession::default();

        let first = session.identities(&fixture.provider_plan, &fixture.specs)?;
        let repeated = session.identities(&fixture.provider_plan, &fixture.specs)?;
        assert!(
            Arc::ptr_eq(&first, &repeated),
            "an unchanged checked context must reuse one result"
        );
        assert_eq!(session.entry_count()?, 1);
        assert_eq!(
            first.for_context(&fixture.provider_plan, &fixture.specs)?,
            &provider_semantic_identities(&fixture.provider_plan, &fixture.specs)?,
            "session reuse must preserve the uncached production result"
        );
        let uncached_lock = semantic_lock_state(
            temp.path(),
            None,
            Some(&fixture.inventory),
            Some(&fixture.components),
            None,
            &fixture.provider_plan,
            &fixture.specs,
        )?;
        let cached_lock = semantic_lock_state_with_provider_identities(
            temp.path(),
            None,
            Some(&fixture.inventory),
            Some(&fixture.components),
            None,
            &fixture.provider_plan,
            &fixture.specs,
            &first,
        )?;
        assert_eq!(
            cached_lock, uncached_lock,
            "the session-aware lock seam must preserve exact lock state"
        );
        Ok(())
    }

    #[test]
    fn semantic_identity_session_rekeys_changed_dependency_context() -> TestResult {
        let temp = tempfile::tempdir()?;
        let fixture = production_toolchain_semantic_fixture(&temp.path().join("dependency-context"))?;
        let session = ProviderSemanticIdentitySession::default();

        let first = session.identities(&fixture.provider_plan, &fixture.specs)?;
        let first_identities = first.for_context(&fixture.provider_plan, &fixture.specs)?.clone();
        fs::write(
            temp.path().join("dependency-context/crates/incan_derive/src/lib.rs"),
            "pub fn derive_marker() { changed(); }\n",
        )?;
        let changed = session.identities(&fixture.provider_plan, &fixture.specs)?;
        assert_ne!(
            &first_identities,
            changed.for_context(&fixture.provider_plan, &fixture.specs)?,
            "changed support content must produce a distinct semantic result"
        );
        assert_eq!(
            session.entry_count()?,
            2,
            "the changed checked context must not reuse the earlier entry"
        );
        Ok(())
    }

    #[test]
    fn semantic_identity_session_binds_checked_in_memory_manifest_and_features() -> TestResult {
        let temp = tempfile::tempdir()?;
        let fixture = production_toolchain_semantic_fixture(&temp.path().join("checked-facts"))?;
        let session = ProviderSemanticIdentitySession::default();
        let first = session.identities(&fixture.provider_plan, &fixture.specs)?;

        let equivalent_records = fixture
            .provider_plan
            .records()
            .cloned()
            .map(|mut record| {
                record.manifest = record.manifest.as_deref().cloned().map(Arc::new);
                record
            })
            .collect();
        let equivalent_plan = ProviderPlan::new(
            incan_frontend::library_manifest_index::LibraryManifestIndex::default(),
            equivalent_records,
            std::iter::empty::<Vec<String>>(),
        )?;
        assert_eq!(
            fixture.provider_plan.semantic_projection_persistent_key()?,
            equivalent_plan.semantic_projection_persistent_key()?,
            "equivalent admitted provider facts must reconstruct the same persisted key"
        );

        let mut changed_record = fixture
            .provider_plan
            .records()
            .next()
            .cloned()
            .ok_or("fixture provider is missing")?;
        let mut changed_manifest = changed_record
            .manifest
            .as_deref()
            .cloned()
            .ok_or("fixture manifest is missing")?;
        changed_manifest.contract_metadata.provider.semantic_source_digest = Some(format!("sha256:{}", "b".repeat(64)));
        changed_manifest.contract_metadata.provider.public_features.insert(
            "checked-feature".to_string(),
            incan_frontend::library_manifest::ProviderFeatureMetadata::default(),
        );
        changed_manifest
            .contract_metadata
            .provider
            .active_features
            .insert("checked-feature".to_string());
        changed_manifest.contract_metadata.provider.implementation_facets[0].cargo_dependencies[0]
            .features
            .insert("changed-dependency-feature".to_string());
        changed_record.manifest = Some(Arc::new(changed_manifest));
        changed_record
            .identity
            .feature_projection
            .insert("checked-feature".to_string());
        let changed_plan = ProviderPlan::new(
            incan_frontend::library_manifest_index::LibraryManifestIndex::default(),
            vec![changed_record],
            std::iter::empty::<Vec<String>>(),
        )?;
        assert_ne!(
            fixture.provider_plan.semantic_projection_persistent_key(),
            changed_plan.semantic_projection_persistent_key(),
            "changed checked dependency facts must change persisted preliminary-map authority"
        );
        let semantic_dependencies = semantic_toolchain_dependencies(&fixture.specs)?;
        let mut preliminary_counters = ProviderSemanticDigestCounters::default();
        let _ = provider_dependency_semantic_digests_observed(
            &changed_plan,
            &semantic_dependencies,
            Some(&mut preliminary_counters),
        )?;
        assert_eq!(
            preliminary_counters.preliminary_provider_digest_calls, 1,
            "a new immutable provider plan must recompute its preliminary map"
        );
        assert_eq!(preliminary_counters.preliminary_memory_hits, 0);
        let changed = session.identities(&changed_plan, &fixture.specs)?;
        assert_ne!(
            first.for_context(&fixture.provider_plan, &fixture.specs)?,
            changed.for_context(&changed_plan, &fixture.specs)?,
            "checked manifest and feature facts must select a distinct result"
        );
        assert_eq!(session.entry_count()?, 2);
        assert!(
            first.for_context(&changed_plan, &fixture.specs).is_err(),
            "a result must refuse a different checked plan"
        );
        Ok(())
    }

    #[test]
    fn semantic_identity_session_refuses_provider_source_or_manifest_tampering() -> TestResult {
        let temp = tempfile::tempdir()?;
        for (name, relative, content) in [
            ("source", "src/lib.rs", "pub fn support() { tampered(); }\n"),
            ("manifest", "support_provider.incnlib", "{\"tampered\":true}\n"),
        ] {
            let fixture = production_toolchain_semantic_fixture(&temp.path().join(name))?;
            let session = ProviderSemanticIdentitySession::default();
            let checked = session.identities(&fixture.provider_plan, &fixture.specs)?;
            fs::write(
                temp.path()
                    .join(name)
                    .join("sdk/components/support-provider")
                    .join(relative),
                content,
            )?;
            assert!(
                semantic_lock_state_with_provider_identities(
                    temp.path(),
                    None,
                    Some(&fixture.inventory),
                    Some(&fixture.components),
                    None,
                    &fixture.provider_plan,
                    &fixture.specs,
                    &checked,
                )
                .is_err(),
                "{name} tampering must invalidate a retained checked bundle"
            );
            let error = session.identities(&fixture.provider_plan, &fixture.specs);
            assert!(
                error.is_err(),
                "{name} tampering must be refused by the existing session"
            );

            let fresh = ProviderSemanticIdentitySession::default();
            assert!(
                fresh.identities(&fixture.provider_plan, &fixture.specs).is_err(),
                "{name} tampering must also be refused by a fresh session"
            );
        }
        Ok(())
    }

    #[test]
    fn native_provider_compatibility_identity_allows_only_checked_runtime_relocation() -> TestResult {
        let temp = tempfile::tempdir()?;
        let first = production_toolchain_semantic_fixture(&temp.path().join("source-checkout-a"))?;
        let second = production_toolchain_semantic_fixture(&temp.path().join("sealed-suite-runtime"))?;

        let first_identities = provider_semantic_identities(&first.provider_plan, &first.specs)?;
        let second_identities = provider_semantic_identities(&second.provider_plan, &second.specs)?;
        assert_ne!(
            first_identities.keys().collect::<Vec<_>>(),
            second_identities.keys().collect::<Vec<_>>(),
            "the fixture must retain distinct byte-exact provider identities after Cargo path relocation"
        );
        assert_eq!(
            first_identities.values().collect::<Vec<_>>(),
            second_identities.values().collect::<Vec<_>>(),
            "checked compiler-runtime relocation must retain native compatibility"
        );

        let relocated_runtime = temp.path().join("sealed-suite-runtime/crates/incan_derive");
        fs::write(
            temp.path()
                .join("sealed-suite-runtime/sdk/components/support-provider/Cargo.toml"),
            format!(
                "[package]\nname = \"support_provider\"\nversion = \"0.5.0\"\npublish = false\n\n[dependencies]\nincan_derive = {{ path = \"{}\" }}\n",
                relocated_runtime.display()
            ),
        )?;
        let tampered = provider_semantic_identities(&second.provider_plan, &second.specs)?;
        assert_ne!(
            second_identities.values().collect::<Vec<_>>(),
            tampered.values().collect::<Vec<_>>(),
            "native compatibility must still bind the checked provider Cargo contract"
        );
        Ok(())
    }

    #[test]
    fn sdk_semantic_state_and_fingerprint_ignore_native_provider_outputs_issue931() -> TestResult {
        let temp = tempfile::tempdir()?;
        let first_checkout = temp.path().join("macos-source");
        let second_checkout = temp.path().join("linux-source");
        let first = production_toolchain_semantic_fixture_with_native_output(
            &first_checkout,
            "pub fn host_marker() -> &'static str { \"macos\" }\n",
            false,
            'a',
        )?;
        let second = production_toolchain_semantic_fixture_with_native_output(
            &second_checkout,
            "pub fn host_marker() -> &'static str { \"linux\" }\n",
            true,
            'a',
        )?;
        let first_physical = first
            .inventory
            .components
            .get("support")
            .and_then(|component| component.providers.first())
            .ok_or("first fixture did not publish the support provider")?;
        let second_physical = second
            .inventory
            .components
            .get("support")
            .and_then(|component| component.providers.first())
            .ok_or("second fixture did not publish the support provider")?;
        assert_ne!(
            first_physical.digest, second_physical.digest,
            "the regression requires distinct physical native artifacts"
        );

        let first_semantic = semantic_lock_state(
            &first_checkout,
            None,
            Some(&first.inventory),
            Some(&first.components),
            None,
            &first.provider_plan,
            &first.specs,
        )?;
        let second_semantic = semantic_lock_state(
            &second_checkout,
            None,
            Some(&second.inventory),
            Some(&second.components),
            None,
            &second.provider_plan,
            &second.specs,
        )?;
        assert_eq!(first_semantic, second_semantic);
        assert_eq!(first_semantic.sdk, second_semantic.sdk);

        let selection = CargoFeatureSelection::default();
        let first_fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            &first.specs,
            &[],
            &selection,
            Some(&first_checkout),
            &first_semantic,
            &first.specs,
        );
        let second_fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            &second.specs,
            &[],
            &selection,
            Some(&second_checkout),
            &second_semantic,
            &second.specs,
        );
        assert_eq!(first_fingerprint, second_fingerprint);

        let changed = production_toolchain_semantic_fixture_with_native_output(
            &temp.path().join("changed-source"),
            "pub fn host_marker() -> &'static str { \"linux\" }\n",
            true,
            'b',
        )?;
        let changed_semantic = semantic_lock_state(
            &temp.path().join("changed-source"),
            None,
            Some(&changed.inventory),
            Some(&changed.components),
            None,
            &changed.provider_plan,
            &changed.specs,
        )?;
        assert_ne!(second_semantic, changed_semantic);
        assert_ne!(
            second_fingerprint,
            compute_resolved_fingerprint_with_sdk_paths(
                &changed.specs,
                &[],
                &selection,
                Some(&temp.path().join("changed-source")),
                &changed_semantic,
                &changed.specs,
            )
        );
        Ok(())
    }

    #[test]
    fn sdk_inventory_digest_uses_provider_semantics_not_physical_artifact_digest_issue921() -> TestResult {
        let inventory = |root: &Path, physical_digest: &str| SdkInventory {
            root: root.to_path_buf(),
            sdk_id: "incan".to_string(),
            sdk_version: "0.5.0".to_string(),
            compiler_requirement: "^0.5".to_string(),
            provider_codegen_revision: incan_lang::version::SDK_PROVIDER_CODEGEN_REVISION,
            components: BTreeMap::from([(
                "stdlib-data".to_string(),
                crate::SdkComponent {
                    id: "stdlib-data".to_string(),
                    version: "0.5.0".to_string(),
                    mandatory: false,
                    available: true,
                    dependencies: BTreeSet::new(),
                    providers: vec![crate::SdkProviderDescriptor {
                        name: "incan_stdlib_data".to_string(),
                        version: "0.5.0".to_string(),
                        digest: physical_digest.to_string(),
                        namespace_claims: BTreeSet::from([vec!["std".to_string(), "regex".to_string()]]),
                        manifest_path: Some(root.join("components/stdlib-data/incan_stdlib_data.incnlib")),
                        crate_root: Some(root.join("components/stdlib-data")),
                    }],
                },
            )]),
            profiles: BTreeMap::from([("default".to_string(), BTreeSet::from(["stdlib-data".to_string()]))]),
        };
        let provider = ProviderRecord {
            identity: crate::ProviderIdentity {
                name: "incan_stdlib_data".to_string(),
                version: "0.5.0".to_string(),
                digest: "sha256:physical-a".to_string(),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::Sdk {
                sdk_identity: "incan@0.5.0".to_string(),
                component_id: "stdlib-data".to_string(),
                inventory_path: None,
            },
            authority: crate::NamespaceAuthority::SdkReserved,
            namespace_claims: BTreeSet::new(),
            available: true,
            enabled: true,
            manifest: None,
            artifact: None,
            implementation_facets: Vec::new(),
        };
        let semantic_identity = "incan_stdlib_data@0.5.0#sha256:semantic[]".to_string();
        let first = inventory(Path::new("/provider-home-a"), "sha256:physical-a");
        let second = inventory(Path::new("/provider-home-b"), "sha256:physical-b");
        assert_eq!(
            semantic_sdk_inventory_digest(&first, &[(&provider, semantic_identity.clone())])?,
            semantic_sdk_inventory_digest(&second, &[(&provider, semantic_identity.clone())])?
        );
        assert_ne!(
            semantic_sdk_inventory_digest(&second, &[(&provider, semantic_identity)])?,
            semantic_sdk_inventory_digest(
                &second,
                &[(&provider, "incan_stdlib_data@0.5.0#sha256:changed[]".to_string())],
            )?
        );
        Ok(())
    }

    #[test]
    fn semantic_lock_state_freezes_declared_oven_interop_requirements() -> TestResult {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("interop/include"))?;
        fs::create_dir_all(project.path().join("interop/src"))?;
        fs::create_dir_all(project.path().join("interop/lib"))?;
        fs::write(project.path().join("interop/include/bridge.h"), "int bridge(void);\n")?;
        fs::write(
            project.path().join("interop/src/bridge.c"),
            "int bridge(void) { return 7; }\n",
        )?;
        fs::write(project.path().join("interop/lib/libfixture.a"), b"fixture archive")?;
        let mut interop = InteropCSection {
            schema: oven_model::oven_interop::INTEROP_C_SCHEMA_VERSION,
            targets: vec![oven_model::oven_interop::InteropCTarget {
                target: "aarch64-apple-ios".to_string(),
                toolchain: Some(oven_model::oven_interop::ToolchainRequirement {
                    capability: "apple-clang".to_string(),
                    version: Some(">=17, <18".to_string()),
                }),
                sdk: Some(oven_model::oven_interop::ToolchainRequirement {
                    capability: "iphoneos".to_string(),
                    version: Some(">=18, <19".to_string()),
                }),
                platform: Some(oven_model::oven_interop::InteropTargetPlatform::Ios {
                    deployment_target: "13.0".to_string(),
                }),
                headers: vec!["interop/include/bridge.h".to_string()],
                definitions: vec!["FIXTURE=1".to_string()],
                artifacts: vec![oven_model::oven_interop::InteropArtifact {
                    name: "fixture".to_string(),
                    kind: oven_model::oven_interop::InteropArtifactKind::Static,
                    path: Some("interop/lib/libfixture.a".to_string()),
                    origin: None,
                    capability: None,
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: Vec::new(),
                }],
                bindings: Vec::new(),
                shims: vec![oven_model::oven_interop::InteropShim {
                    name: "fixture_bridge".to_string(),
                    language: oven_model::oven_interop::InteropShimLanguage::C,
                    sources: vec!["interop/src/bridge.c".to_string()],
                    headers: vec!["interop/include/bridge.h".to_string()],
                    output: "fixture_bridge".to_string(),
                }],
            }],
        };
        let cargo_features = CargoFeatureSelection::default();
        let first = semantic_lock_state(
            project.path(),
            Some(&interop),
            None,
            None,
            None,
            &ProviderPlan::default(),
            &[],
        )?;
        let first_interop = first.oven.as_ref().ok_or("lock state omitted Oven requirements")?;
        assert_eq!(first_interop.interop.len(), 1);
        assert_eq!(
            first_interop.interop[0].platform,
            Some(oven_model::oven_interop::InteropTargetPlatform::Ios {
                deployment_target: "13.0".to_string(),
            })
        );
        assert_eq!(first_interop.interop[0].headers[0].path, "interop/include/bridge.h");
        let first_fingerprint = compute_resolved_fingerprint(&[], &[], &cargo_features, Some(project.path()), &first);

        interop.targets[0].platform = Some(oven_model::oven_interop::InteropTargetPlatform::Ios {
            deployment_target: "14.0".to_string(),
        });
        let changed_platform = semantic_lock_state(
            project.path(),
            Some(&interop),
            None,
            None,
            None,
            &ProviderPlan::default(),
            &[],
        )?;
        assert_ne!(first.oven, changed_platform.oven);
        assert_ne!(
            first_fingerprint,
            compute_resolved_fingerprint(&[], &[], &cargo_features, Some(project.path()), &changed_platform)
        );
        interop.targets[0].platform = Some(oven_model::oven_interop::InteropTargetPlatform::Ios {
            deployment_target: "13.0".to_string(),
        });

        fs::write(
            project.path().join("interop/src/bridge.c"),
            "int bridge(void) { return 8; }\n",
        )?;
        let second = semantic_lock_state(
            project.path(),
            Some(&interop),
            None,
            None,
            None,
            &ProviderPlan::default(),
            &[],
        )?;
        assert_ne!(first.oven, second.oven);
        assert_ne!(
            first_fingerprint,
            compute_resolved_fingerprint(&[], &[], &cargo_features, Some(project.path()), &second)
        );
        Ok(())
    }
}
