//! Native SDK preparation and frozen inspection authority, selected from the SDK seed rather than Cargo metadata.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use oven_rustc::sdk_closure::{
    ClosureCompileRequest, LocalFacetSelection, SdkCompiledClosure, compile_local_sdk_facet,
    compile_local_sdk_facet_for_target, compile_local_sdk_facets, prepare_closure, prepare_sdk_seed,
};
use oven_store::store::{OvenStore, OvenStoreExecutionPayload, OvenStoreLimits};
use rust_inspect::{
    OVEN_DIRECT_LOAF_PROJECT_FILE, OvenInspectionRegistrySource, write_sealed_oven_inspection_source_authority,
};
use serde::{Deserialize, Serialize};

use crate::SdkSourceCatalog;
use crate::error::{ProviderError, ProviderResult};

/// Native output catalog accompanying one complete SDK publication.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SdkNativeArtifactCatalog {
    /// Version of the publication's native-coordinate schema.
    schema_version: u32,
    /// Store containing the immutable entries recorded below.
    store: PathBuf,
    /// Complete output selection accompanying the native receipt map.
    units: Vec<oven_rustc::sdk_closure::SdkNativeArtifact>,
}

/// Persist the exact admitted native outputs so consumers can reacquire their original store leases.
pub fn write_sdk_native_artifact_catalog(
    closure: &SdkCompiledClosure,
    store: &Path,
    root: &Path,
) -> ProviderResult<()> {
    let units = closure
        .units()
        .iter()
        .map(|unit| unit.native_artifact())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    let catalog = SdkNativeArtifactCatalog {
        schema_version: 1,
        store: store.to_path_buf(),
        units,
    };
    let graph = closure.inspection_project();
    let crates = graph["crates"]
        .as_array()
        .ok_or_else(|| ProviderError::failure("native closure has no inspection graph"))?;
    let mut macros = Vec::new();
    for (unit, record) in closure.units().iter().zip(crates) {
        if record["is_proc_macro"].as_bool() != Some(true) {
            continue;
        }
        let artifact = unit
            .native_artifact()
            .map_err(|error| ProviderError::failure(error.to_string()))?;
        macros.push(rust_inspect::OvenInspectionProcMacro {
            root_module: record["root_module"]
                .as_str()
                .map(PathBuf::from)
                .ok_or_else(|| ProviderError::failure("native macro has no selected source module"))?,
            store: store.to_path_buf(),
            identity: artifact.store_identity,
            receipt_identity: artifact.receipt_identity,
            relative_path: artifact.relative_path,
            digest: artifact.digest,
        });
    }
    rust_inspect::write_oven_inspection_proc_macro_authority(root, macros)
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    std::fs::write(
        root.join(".sealed-native-units.json"),
        serde_json::to_vec_pretty(&catalog).map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))
}

/// Reacquire all SDK native source and output leases, validating their payload bindings and output descriptors.
///
/// Selection uses immutable store coordinates recorded during publication. It never discovers an ambient source
/// cache, resolves requirements, or trusts a native path independently of its admitted output digest.
pub fn retain_sdk_native_artifacts(root: &Path) -> ProviderResult<Vec<OvenStoreExecutionPayload>> {
    select_sdk_native_artifacts(root).map(|selection| selection.owners)
}

/// Native coordinates and their verified owners selected together from one SDK publication.
pub struct SdkNativeSelection {
    /// Exact output descriptors in the same order as their retained execution owners.
    pub units: Vec<oven_rustc::sdk_closure::SdkNativeArtifact>,
    /// Leases and verified source/output payloads that authorize the coordinates above.
    pub owners: Vec<OvenStoreExecutionPayload>,
}

/// Resolve required Rust facets to their owning Loaves using the admitted source declarations.
///
/// A native output's link name is independent of its package name. The selected owners retain the source/output
/// leases through this observation and subsequent identity projection; filenames and ambient checkout manifests
/// cannot supply the association. Each required facet must have exactly one compatible host-macro or target-library
/// binding in the selection.
pub fn sdk_native_runtime_loaves(selection: &SdkNativeSelection, required: &[&str]) -> ProviderResult<Vec<String>> {
    if selection.units.len() != selection.owners.len() {
        return Err(ProviderError::failure("native runtime units and owners disagree"));
    }
    let mut facets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (unit, owner) in selection.units.iter().zip(&selection.owners) {
        if unit.binding.loaf.starts_with("crates-io/") {
            continue;
        }
        verify_sdk_native_artifact(unit, owner)?;
        let relative = "source/loaf.toml";
        let admitted = owner
            .admitted_materialized_files()
            .iter()
            .find(|file| file.relative_path == relative)
            .ok_or_else(|| ProviderError::failure("native runtime unit has no admitted Loaf declaration"))?;
        let path = owner.artifact_root.join(relative);
        let bytes = std::fs::read(&path).map_err(|error| ProviderError::failure(error.to_string()))?;
        if oven_store::digest_bytes(&bytes) != admitted.digest {
            return Err(ProviderError::failure(
                "native runtime declaration differs from its admitted bytes",
            ));
        }
        let declaration: toml::Value =
            toml::from_str(std::str::from_utf8(&bytes).map_err(|error| ProviderError::failure(error.to_string()))?)
                .map_err(|error| ProviderError::failure(error.to_string()))?;
        let package = declaration
            .get("project")
            .and_then(|project| project.get("name"))
            .and_then(toml::Value::as_str)
            .ok_or_else(|| ProviderError::failure("native runtime declaration has no package name"))?;
        if package != unit.binding.loaf {
            return Err(ProviderError::failure(
                "native runtime package differs from its admitted binding",
            ));
        }
        let rust = declaration.get("rust");
        let name = rust
            .and_then(|rust| rust.get("name"))
            .and_then(toml::Value::as_str)
            .unwrap_or(package)
            .replace('-', "_");
        if !required.contains(&name.as_str()) {
            continue;
        }
        let kind = rust
            .and_then(|rust| rust.get("type"))
            .and_then(toml::Value::as_str)
            .unwrap_or("lib");
        let extension = Path::new(&unit.relative_path)
            .extension()
            .and_then(|extension| extension.to_str());
        let compatible = match kind {
            "proc-macro" => unit.binding.domain == "host" && matches!(extension, Some("dylib" | "so" | "dll")),
            "lib" => unit.binding.domain == "target" && extension == Some("rlib"),
            _ => false,
        };
        if compatible {
            facets.entry(name).or_default().push(unit.binding.loaf.clone());
        }
    }
    required
        .iter()
        .map(|name| match facets.get(*name).map(Vec::as_slice) {
            Some([loaf]) => Ok(loaf.clone()),
            _ => Err(ProviderError::failure(format!(
                "native runtime facet `{name}` has no unique admitted Loaf"
            ))),
        })
        .collect()
}

/// Select native descriptors with their admitted owners instead of allowing consumers to trust output paths alone.
pub fn select_sdk_native_artifacts(root: &Path) -> ProviderResult<SdkNativeSelection> {
    let catalog: SdkNativeArtifactCatalog = serde_json::from_slice(
        &std::fs::read(root.join(".sealed-native-units.json"))
            .map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    if catalog.schema_version != 1 || catalog.units.is_empty() {
        return Err(ProviderError::failure(
            "SDK native output catalog is empty or unsupported",
        ));
    }
    let receipts: BTreeMap<String, String> = serde_json::from_slice(
        &std::fs::read(root.join(".sealed-native-receipts.json"))
            .map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    if receipts.len() != catalog.units.len() {
        return Err(ProviderError::failure(
            "SDK native coordinate and receipt catalogs disagree",
        ));
    }
    let store = OvenStore::new(
        catalog.store,
        OvenStoreLimits::new(4 * 1024 * 1024 * 1024, 4 * 1024 * 1024 * 1024, 4 * 1024 * 1024 * 1024),
    );
    let identities = catalog
        .units
        .iter()
        .map(|unit| unit.store_identity.clone())
        .collect::<Vec<_>>();
    let selected = store
        .select_payloads_for_execution(&identities)
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    for (unit, owner) in catalog.units.iter().zip(&selected) {
        let key = serde_json::to_string(&unit.binding.identity_binding())
            .map_err(|error| ProviderError::failure(error.to_string()))?;
        if receipts.get(&key) != Some(&unit.receipt_identity) {
            return Err(ProviderError::failure(
                "SDK native output catalog disagrees with its selected receipt",
            ));
        }
        verify_sdk_native_artifact(unit, owner)?;
    }
    Ok(SdkNativeSelection {
        units: catalog.units,
        owners: selected,
    })
}

/// Authenticate a native descriptor against its retained owner before selecting its output or package facets.
fn verify_sdk_native_artifact(
    unit: &oven_rustc::sdk_closure::SdkNativeArtifact,
    owner: &OvenStoreExecutionPayload,
) -> ProviderResult<()> {
    owner
        .verify_proven_native_payload()
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    let actual: oven_rustc::sdk_closure::SdkLockedUnit =
        serde_json::from_slice(&owner.payload).map_err(|error| ProviderError::failure(error.to_string()))?;
    let actual =
        serde_json::to_value(actual.identity_binding()).map_err(|error| ProviderError::failure(error.to_string()))?;
    let expected = serde_json::to_value(unit.binding.identity_binding())
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    if actual != expected
        || unit.store_identity != owner.manifest.identity
        || owner.manifest.domain != format!("sdk-source-unit-{}", unit.binding.domain)
        || owner.manifest.receipt_identity != unit.receipt_identity
        || unit.relative_path.contains(['/', '\\'])
        || !matches!(
            Path::new(&unit.relative_path)
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("rlib" | "dylib" | "so" | "dll")
        )
        || !owner
            .admitted_materialized_files()
            .iter()
            .any(|file| file.relative_path == unit.relative_path && file.digest == unit.digest)
    {
        return Err(ProviderError::failure(
            "SDK native output catalog disagrees with its admitted store entry",
        ));
    }
    Ok(())
}

/// Explicit immutable archive and index inputs for automatic source SDK preparation.
pub struct SdkNativeInputs {
    /// Digest-addressed admitted archive directory.
    pub blobs: PathBuf,
    /// Repository containing the closure executor's pinned index commit.
    pub index: PathBuf,
    /// Retained receipt store; independent of a provider publication's staging directory.
    pub output: PathBuf,
    /// Managed compiler selected for all local and adopted units.
    pub rustc: PathBuf,
    /// Optional explicit compiler-companion graph with a resolved registry closure and selected local facets.
    pub compiler_graph: Option<PathBuf>,
}

impl SdkNativeInputs {
    /// Resolve explicitly supplied archives and index without discovering ambient Cargo caches.
    pub fn discover(store: &Path) -> ProviderResult<Self> {
        let required = |name: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .ok_or_else(|| ProviderError::failure(format!("native SDK preparation requires {name}")))
        };
        Ok(Self {
            blobs: required("INCAN_SDK_NATIVE_BLOBS")?,
            index: required("INCAN_SDK_NATIVE_INDEX")?,
            output: store.join(".native"),
            rustc: oven_rustc::rustc::resolve_active_rustc()
                .map_err(|error| ProviderError::failure(error.to_string()))?,
            compiler_graph: std::env::var_os("INCAN_SDK_NATIVE_COMPILER_GRAPH").map(PathBuf::from),
        })
    }
}

/// Compile the sealed closure, its local companions, and every component declaring a Rust facet.
///
/// Successful independent units remain receipt-bound and reusable when native compilation fails. The checked
/// publisher admits only components whose own facets and dependency providers are available; named failures remain
/// in the closure report and cannot authorize missing outputs.
pub fn prepare_sdk_native_closure(
    stdlib: &Path,
    catalog: &SdkSourceCatalog,
    inputs: &SdkNativeInputs,
) -> ProviderResult<SdkCompiledClosure> {
    let graph = inputs
        .compiler_graph
        .as_ref()
        .map(|path| read_compiler_graph(path))
        .transpose()?;
    let mut closure = if let Some((owner, graph)) = &graph {
        prepare_closure(&ClosureCompileRequest {
            primary: &[],
            lock: &owner.join(&graph.registry_lock),
            blobs: &inputs.blobs,
            output: &inputs.output,
            rustc: &inputs.rustc,
            index: &inputs.index,
            index_commit: &graph.index_commit,
            target: &oven_rustc::rustc::rustc_host_target(&inputs.rustc)
                .map_err(|error| ProviderError::failure(error.to_string()))?,
            profile: "debug",
        })
    } else {
        prepare_sdk_seed(
            &stdlib.join("sdk-lock.json"),
            &inputs.blobs,
            &inputs.output,
            &inputs.rustc,
            &inputs.index,
        )
    }
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    std::fs::write(
        inputs.output.join("closure-report.json"),
        serde_json::to_vec_pretty(closure.report()).map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    let mut local_failures = Vec::new();
    let source_root = stdlib
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| ProviderError::failure("SDK source root has no owning toolchain"))?;
    for (relative, domain, features) in [
        ("loaves/kernel/incan_lang", "target", Vec::new()),
        ("loaves/stdlib/derive/incan_derive", "host", Vec::new()),
        ("loaves/stdlib/derive/incan_web_macros", "host", Vec::new()),
        ("loaves/kernel/incan_vocab", "target", vec!["serde".to_string()]),
    ] {
        let requested = graph.as_ref().and_then(|(owner, graph)| {
            graph.facets.iter().find(|facet| {
                owner.join(&facet.project).canonicalize().ok() == source_root.join(relative).canonicalize().ok()
            })
        });
        let features = requested.map(|facet| facet.features.as_slice()).unwrap_or(&features);
        if let Err(error) = compile_local_sdk_facet(
            &mut closure,
            &source_root.join(relative),
            features,
            domain,
            &inputs.output,
            &inputs.rustc,
        ) {
            local_failures.push(format!("SDK companion {relative}: {error}"));
        }
    }
    if let Some((owner, graph)) = &graph {
        compile_local_sdk_facets(&mut closure, &graph.facets, owner, &inputs.output, &inputs.rustc)
            .map_err(|error| ProviderError::failure(error.to_string()))?;
    }
    if let Err(error) = attach_vocabulary_desugarer_closure(&mut closure, stdlib, source_root, inputs) {
        local_failures.push(format!("SDK vocabulary desugarer closure: {error}"));
    }
    for component in catalog.publication_order() {
        let declaration = std::fs::read_to_string(component.project_root.join("loaf.toml"))
            .map_err(|error| ProviderError::failure(error.to_string()))?;
        let declaration: toml::Value =
            toml::from_str(&declaration).map_err(|error| ProviderError::failure(error.to_string()))?;
        if declaration.get("rust").is_some()
            && let Err(error) = compile_local_sdk_facet(
                &mut closure,
                &component.project_root,
                &[],
                "target",
                &inputs.output,
                &inputs.rustc,
            )
        {
            local_failures.push(format!("SDK component {}: {error}", component.id));
        }
    }
    let mut report =
        serde_json::to_value(closure.report()).map_err(|error| ProviderError::failure(error.to_string()))?;
    report["local_failures"] =
        serde_json::to_value(&local_failures).map_err(|error| ProviderError::failure(error.to_string()))?;
    std::fs::write(
        inputs.output.join("closure-report.json"),
        serde_json::to_vec_pretty(&report).map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    Ok(closure)
}

/// Explicit compiler-companion selection, resolved before SDK publication and independent of Cargo metadata.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompilerNativeGraph {
    /// Immutable registry revision governing adopted sources and facts.
    index_commit: String,
    /// Complete registry resolution including the ordinary SDK inputs, relative to this document.
    registry_lock: PathBuf,
    /// Selected local compilation units with their exact feature and domain policy.
    facets: Vec<LocalFacetSelection>,
}

/// Read the explicit native graph relative to its canonical owner; missing input never falls back to Cargo.
fn read_compiler_graph(path: &Path) -> ProviderResult<(PathBuf, CompilerNativeGraph)> {
    let path = path
        .canonicalize()
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    let owner = path
        .parent()
        .ok_or_else(|| ProviderError::failure("compiler graph has no owner"))?
        .to_path_buf();
    let graph =
        serde_json::from_slice(&std::fs::read(&path).map_err(|error| ProviderError::failure(error.to_string()))?)
            .map_err(|error| ProviderError::failure(error.to_string()))?;
    Ok((owner, graph))
}

/// Bind explicit companion policy, registry resolution, and every local source snapshot to SDK freshness.
pub(crate) fn compiler_native_graph_digest(path: &Path) -> ProviderResult<String> {
    let (owner, graph) = read_compiler_graph(path)?;
    let mut bytes = std::fs::read(path).map_err(|error| ProviderError::failure(error.to_string()))?;
    bytes.extend(
        std::fs::read(owner.join(graph.registry_lock)).map_err(|error| ProviderError::failure(error.to_string()))?,
    );
    for facet in graph.facets {
        let digest =
            oven_rustc::sdk_closure::local_sdk_facet_source_digest(&owner.join(facet.project), &std::env::temp_dir())
                .map_err(|error| ProviderError::failure(error.to_string()))?;
        bytes.extend(digest.as_bytes());
    }
    Ok(oven_store::digest_bytes(&bytes))
}

/// Target triple every SDK vocabulary desugarer is compiled for.
pub const VOCABULARY_DESUGARER_TARGET: &str = "wasm32-wasip1";

/// The desugarer closure's committed resolution, relative to the standard-library root.
const VOCABULARY_DESUGARER_LOCK: &str = "vocab-wasm-lock.json";

/// Index commit whose build facts the desugarer closure compiles under; it records the wasm32-wasip1 facts.
const VOCABULARY_DESUGARER_INDEX_COMMIT: &str = "d6e8b1e0ce9a47bd1f9da967c270636dc8cab956";

/// Compile the vocabulary desugarer inputs for wasm32-wasip1 and retain them beside the SDK closure.
///
/// The closure is the SDK seed's own serde family cut to what `incan_vocab` needs, plus `incan_vocab` itself built for
/// the same target. Its units are receipt-bound like the seed's, so a desugarer is built only from admitted archives.
fn attach_vocabulary_desugarer_closure(
    closure: &mut SdkCompiledClosure,
    stdlib: &Path,
    source_root: &Path,
    inputs: &SdkNativeInputs,
) -> Result<(), Box<dyn std::error::Error>> {
    let output = inputs.output.join(VOCABULARY_DESUGARER_TARGET);
    let mut vocabulary = prepare_closure(&ClosureCompileRequest {
        primary: &[],
        lock: &stdlib.join(VOCABULARY_DESUGARER_LOCK),
        blobs: &inputs.blobs,
        output: &output,
        rustc: &inputs.rustc,
        index: &inputs.index,
        index_commit: VOCABULARY_DESUGARER_INDEX_COMMIT,
        target: VOCABULARY_DESUGARER_TARGET,
        profile: "debug",
    })?;
    compile_local_sdk_facet_for_target(
        &mut vocabulary,
        &source_root.join("loaves/kernel/incan_vocab"),
        &["serde".to_string()],
        "target",
        &output,
        &inputs.rustc,
        VOCABULARY_DESUGARER_TARGET,
    )?;
    closure.attach_auxiliary_target(VOCABULARY_DESUGARER_TARGET, vocabulary)
}

/// Identify complete native bindings, retaining domain and features so host and target units cannot collide.
pub fn sdk_native_receipts(closure: &SdkCompiledClosure) -> ProviderResult<BTreeMap<String, String>> {
    let mut receipts = BTreeMap::new();
    for unit in closure.units() {
        let key = serde_json::to_string(&unit.binding().identity_binding())
            .map_err(|error| ProviderError::failure(error.to_string()))?;
        if receipts.insert(key, unit.compiled_identity().to_string()).is_some() {
            return Err(ProviderError::failure("duplicate native SDK binding"));
        }
    }
    Ok(receipts)
}

/// Write receipt-bound source authority and the exact compiled alias graph without resolving declarations again.
///
/// The caller retains the closure's leases until its complete publication transaction is durable. This helper does
/// not authorize a partial provider inventory or an unavailable native unit.
pub fn write_sdk_native_authority(closure: &SdkCompiledClosure, root: &Path) -> ProviderResult<PathBuf> {
    let mut sources = BTreeMap::new();
    for unit in closure.units() {
        let source_root = unit.source_root();
        let binding = unit.binding();
        let (package, registry) = match binding.loaf.strip_prefix("crates-io/") {
            Some(package) => (package, "registry+incan.pub/crates-io"),
            None => (binding.loaf.as_str(), "sdk+native"),
        };
        let source_digest =
            oven_store::digest_source_tree(&source_root).map_err(|error| ProviderError::failure(error.to_string()))?;
        sources
            .entry((
                package.to_string(),
                binding.version.clone(),
                binding.features.clone(),
                source_root.clone(),
            ))
            .or_insert_with(|| OvenInspectionRegistrySource {
                package: package.to_string(),
                version: binding.version.clone(),
                registry: registry.to_string(),
                checksum: binding.archive_digest.clone(),
                features: binding.features.clone(),
                source_root,
                source_digest,
            });
    }
    let authority = write_sealed_oven_inspection_source_authority(root, sources.into_values().collect())
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    std::fs::write(
        root.join(OVEN_DIRECT_LOAF_PROJECT_FILE),
        serde_json::to_vec_pretty(&closure.inspection_project())
            .map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    std::fs::write(
        root.join(".sealed-native-receipts.json"),
        serde_json::to_vec_pretty(&sdk_native_receipts(closure)?)
            .map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    Ok(authority)
}

/// Check a consumer requirement against one admitted SDK selection without resolving another dependency graph.
///
/// Registry requirements bind package, version, domain, and features. Path requirements must name an exact
/// published SDK provider root, a catalog-owned facet, or reproduce the complete source identity of an admitted
/// compiler companion. A matching package name alone never borrows the SDK's native authority.
pub fn sdk_native_dependency_is_covered(
    inventory: &crate::SdkInventory,
    selection: &SdkNativeSelection,
    dependency: &oven_model::manifest::DependencySpec,
) -> ProviderResult<bool> {
    use oven_model::manifest::DependencySource;
    let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
    let requirement = dependency
        .version
        .as_deref()
        .map(semver::VersionReq::parse)
        .transpose()
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    let accepts_version = |version: &str| {
        semver::Version::parse(version).is_ok_and(|version| {
            requirement
                .as_ref()
                .is_none_or(|requirement| requirement.matches(&version))
        })
    };
    let mut local_digest = None;
    if let DependencySource::Path { path } = &dependency.source {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        for provider in inventory
            .components
            .values()
            .filter(|component| component.available)
            .flat_map(|component| &component.providers)
        {
            if let Some(root) = provider.crate_root.as_ref()
                && canonical == root.canonicalize().unwrap_or_else(|_| root.clone())
                && package == provider.name
                && accepts_version(&provider.version)
            {
                return Ok(true);
            }
        }
        if !sdk_native_facet_path_matches(package, &canonical)? {
            if !canonical.join("loaf.toml").is_file() {
                return Ok(false);
            }
            let declaration: toml::Value = toml::from_str(
                &std::fs::read_to_string(canonical.join("loaf.toml"))
                    .map_err(|error| ProviderError::failure(error.to_string()))?,
            )
            .map_err(|error| ProviderError::failure(error.to_string()))?;
            let loaf = declaration
                .get("project")
                .and_then(|project| project.get("name"))
                .and_then(toml::Value::as_str)
                .ok_or_else(|| ProviderError::failure("native companion has no declared Loaf identity"))?;
            if !selection
                .units
                .iter()
                .any(|unit| unit.binding.loaf == loaf && !unit.binding.loaf.starts_with("crates-io/"))
            {
                return Ok(false);
            }
            local_digest = Some(
                oven_rustc::sdk_closure::local_sdk_facet_source_digest(&canonical, &std::env::temp_dir())
                    .map_err(|error| ProviderError::failure(error.to_string()))?,
            );
        }
    } else if !matches!(dependency.source, DependencySource::Registry) {
        return Ok(false);
    }
    let candidates = selection
        .units
        .iter()
        .filter(|unit| {
            let name = Path::new(&unit.relative_path)
                .file_stem()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("lib"));
            let package_matches = match dependency.source {
                DependencySource::Registry => unit.binding.loaf == format!("crates-io/{package}"),
                DependencySource::Path { .. } => {
                    name == Some(package.replace('-', "_").as_str()) && !unit.binding.loaf.starts_with("crates-io/")
                }
                DependencySource::Git { .. } => false,
            };
            package_matches
                && local_digest
                    .as_ref()
                    .is_none_or(|digest| unit.binding.archive_digest == *digest)
                && accepts_version(&unit.binding.version)
                && (unit.binding.domain == "target"
                    || Path::new(&unit.relative_path)
                        .extension()
                        .is_some_and(|extension| extension == "dylib"))
                && dependency
                    .features
                    .iter()
                    .all(|feature| unit.binding.features.contains(feature))
        })
        .count();
    Ok(candidates == 1)
}

/// Match only the compiler's named native companions and component facets against their owning source catalog.
fn sdk_native_facet_path_matches(crate_name: &str, path: &Path) -> ProviderResult<bool> {
    let Some(stdlib) = oven_model::toolchain_layout::find_stdlib_root() else {
        return Ok(false);
    };
    let Some(source) = stdlib.parent().and_then(Path::parent) else {
        return Ok(false);
    };
    let mut roots = match crate_name {
        "incan_lang" | "incan_vocab" => vec![source.join("loaves/kernel").join(crate_name)],
        "incan_derive" | "incan_web_macros" => vec![stdlib.join("derive").join(crate_name)],
        _ => Vec::new(),
    };
    let catalog = SdkSourceCatalog::read_from_path(&stdlib.join(crate::SDK_SOURCE_CATALOG_FILE))
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    for component in catalog.components.values() {
        let text = std::fs::read_to_string(component.project_root.join("loaf.toml"))
            .map_err(|error| ProviderError::failure(error.to_string()))?;
        let declaration: toml::Value =
            toml::from_str(&text).map_err(|error| ProviderError::failure(error.to_string()))?;
        if declaration
            .get("rust")
            .and_then(|rust| rust.get("name"))
            .and_then(toml::Value::as_str)
            == Some(crate_name)
        {
            roots.push(component.project_root.clone());
            roots.push(component.project_root.join("rust"));
        }
    }
    Ok(roots
        .into_iter()
        .any(|root| root.canonicalize().unwrap_or(root) == path))
}

#[cfg(test)]
mod tests {
    /// Native companions retain their package/facet association and refuse changed source or forged selection facts.
    #[test]
    fn compiler_companion_path_requires_current_source_identity() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let project = root.path().join("companion");
        std::fs::create_dir_all(project.join("src"))?;
        std::fs::write(
            project.join("loaf.toml"),
            "[project]\nname='companion-package'\nversion='1.0.0'\n[rust]\nname='companion'\ntype='lib'\nedition='2024'\n",
        )?;
        std::fs::write(project.join("src/lib.rs"), "pub fn value() -> u8 { 1 }")?;
        let seed = root.path().join("seed.json");
        std::fs::write(&seed, r#"{"schema":"incan.oven.loaf-resolution/1","units":[]}"#)?;
        let output = root.path().join("native");
        let rustc = oven_rustc::rustc::resolve_active_rustc()?;
        let mut closure = oven_rustc::sdk_closure::prepare_sdk_seed(&seed, root.path(), &output, &rustc, root.path())?;
        oven_rustc::sdk_closure::compile_local_sdk_facet(&mut closure, &project, &[], "target", &output, &rustc)?;
        let authority = root.path().join("authority");
        std::fs::create_dir(&authority)?;
        super::write_sdk_native_authority(&closure, &authority)?;
        super::write_sdk_native_artifact_catalog(&closure, &output.join("store"), &authority)?;
        let mut selection = super::select_sdk_native_artifacts(&authority)?;
        assert_eq!(
            super::sdk_native_runtime_loaves(&selection, &["companion"])?,
            ["companion-package"]
        );
        assert!(super::sdk_native_runtime_loaves(&selection, &["companion-package"]).is_err());
        selection.units[0].binding.loaf = "wrong-owner".to_string();
        assert!(super::sdk_native_runtime_loaves(&selection, &["companion"]).is_err());
        selection.units[0].binding.loaf = "companion-package".to_string();
        selection.units[0].binding.domain = "host".to_string();
        assert!(super::sdk_native_runtime_loaves(&selection, &["companion"]).is_err());
        selection.units[0].binding.domain = "target".to_string();
        let declaration = selection.owners[0].artifact_root.join("source/loaf.toml");
        let admitted_bytes = std::fs::read(&declaration)?;
        let replacement = declaration.with_extension("replacement");
        std::fs::write(
            &replacement,
            "[project]\nname='companion-package'\n[rust]\nname='forged-facet'\n",
        )?;
        std::fs::rename(&replacement, &declaration)?;
        assert!(super::sdk_native_runtime_loaves(&selection, &["forged-facet"]).is_err());
        std::fs::write(&replacement, admitted_bytes)?;
        std::fs::rename(&replacement, &declaration)?;
        assert_eq!(
            super::sdk_native_runtime_loaves(&selection, &["companion"])?,
            ["companion-package"]
        );
        let mut duplicate = super::select_sdk_native_artifacts(&authority)?;
        duplicate.units.extend(selection.units.iter().cloned());
        duplicate
            .owners
            .extend(super::select_sdk_native_artifacts(&authority)?.owners);
        assert!(super::sdk_native_runtime_loaves(&duplicate, &["companion"]).is_err());
        let inventory = crate::SdkInventory {
            root: authority,
            sdk_id: "fixture".to_string(),
            sdk_version: "1.0.0".to_string(),
            compiler_requirement: "*".to_string(),
            provider_codegen_revision: incan_lang::version::SDK_PROVIDER_CODEGEN_REVISION,
            components: std::collections::BTreeMap::new(),
            profiles: std::collections::BTreeMap::new(),
        };
        let request = oven_model::manifest::DependencySpec {
            crate_name: "companion".to_string(),
            version: Some("1".to_string()),
            features: Vec::new(),
            default_features: false,
            source: oven_model::manifest::DependencySource::Path { path: project.clone() },
            optional: false,
            package: None,
        };
        let unpublished = oven_model::manifest::DependencySpec {
            source: oven_model::manifest::DependencySource::Path {
                path: root.path().join("unpublished"),
            },
            ..request.clone()
        };
        assert!(!super::sdk_native_dependency_is_covered(
            &inventory,
            &selection,
            &unpublished
        )?);
        assert!(super::sdk_native_dependency_is_covered(
            &inventory, &selection, &request
        )?);
        std::fs::write(project.join("Cargo.toml"), "poisoned Cargo input")?;
        assert!(super::sdk_native_dependency_is_covered(
            &inventory, &selection, &request
        )?);
        std::fs::write(project.join("src/lib.rs"), "pub fn value() -> u8 { 2 }")?;
        assert!(!super::sdk_native_dependency_is_covered(
            &inventory, &selection, &request
        )?);
        assert_eq!(
            super::sdk_native_runtime_loaves(&selection, &["companion"])?,
            ["companion-package"]
        );
        Ok(())
    }

    /// Component imports parse using their Loaf dependency declarations instead of removed inline version hints.
    #[test]
    fn system_sources_use_loaf_dependency_declarations() -> Result<(), Box<dyn std::error::Error>> {
        let root = oven_model::toolchain_layout::development_root().join("loaves/stdlib/system/src");
        for relative in [
            "fs/file.incn",
            "fs/locking.incn",
            "fs/path.incn",
            "io.incn",
            "tempfile.incn",
        ] {
            let source = std::fs::read_to_string(root.join(relative))?;
            crate::test_support::parsed_module_for_test(&source).map_err(|error| format!("{relative}: {error}"))?;
        }
        Ok(())
    }
}
