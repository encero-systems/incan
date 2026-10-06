//! Preparing the SDK provider inventory in the store: staging, building the components into it, restricting a
//! staged profile, and reporting what the build did.
//!
//! This is the explicit publication path. Discovery of an already published inventory is `inventory`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::Command;
use std::sync::Arc;
use std::{env, fs};

use incan_lang::lang::stdlib;

use crate::error::{ProviderError, ProviderResult};
#[cfg(test)]
use crate::requirements::INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV;
use crate::sdk_store::{
    INTERNAL_SDK_DISTRIBUTION_PROFILE_ENV, INTERNAL_SDK_PROVIDER_PATH_FILE_ENV, INTERNAL_SDK_PROVIDER_STORE_ENV,
    acquire_sdk_provider_store_lock, default_sdk_provider_store, sdk_provider_builder_executable,
    staged_sdk_provider_root, sync_sdk_provider_store, sync_sdk_provider_tree,
};
use crate::{
    SDK_INVENTORY_FILE, SDK_PROVIDER_BUILD_ENV, SDK_SOURCE_CATALOG_FILE, SdkComponent, SdkComponentSelection,
    SdkInventory, SdkProviderDescriptor, SdkSourceCatalog,
};
use incan_frontend::library_manifest::{LibraryManifest, ProviderModuleClaim, digest_provider_artifact};
use oven_model::manifest::ProjectManifest;
#[cfg(test)]
use oven_model::manifest::{INTERNAL_MANIFEST_OVERRIDE_ENV, INTERNAL_PROJECT_ROOT_OVERRIDE_ENV};
#[cfg(test)]
use oven_model::toolchain_layout::GENERATED_CARGO_TARGET_DIR_ENV;
/// Optional external directory for SDK publication timing evidence.
#[cfg(test)]
const INTERNAL_SDK_BUILD_REPORT_DIR_ENV: &str = "INCAN_INTERNAL_SDK_BUILD_REPORT_DIR";

/// Prepare native units and atomically publish checked component metadata supplied by the compiler driver.
///
/// The callback runs in dependency order with the retained native closure, a complete inspection authority, and the
/// inventory of already checked components. It must write the component's `.incnlib` and executable surfaces into
/// its supplied output root. Nothing here launches a component compiler subprocess or reads Cargo metadata.
pub fn prepare_sdk_provider_inventory_with_native_publisher(
    publisher_store_root: Option<&Path>,
    source_root_override: Option<&Path>,
    inputs: &crate::sdk_native::SdkNativeInputs,
    publish: impl FnMut(&Path, &Path, &SdkInventory, &oven_rustc::sdk_closure::SdkCompiledClosure) -> ProviderResult<()>,
) -> ProviderResult<Arc<SdkInventory>> {
    let publication = SdkProviderPublication::resolve(publisher_store_root, source_root_override)?;
    prepare_native_sdk_provider_inventory(publication, inputs, publish)
}

/// Retain all native selections across the checked publication transaction and its final durable rename.
fn prepare_native_sdk_provider_inventory(
    publication: SdkProviderPublication,
    inputs: &crate::sdk_native::SdkNativeInputs,
    mut publish: impl FnMut(&Path, &Path, &SdkInventory, &oven_rustc::sdk_closure::SdkCompiledClosure) -> ProviderResult<()>,
) -> ProviderResult<Arc<SdkInventory>> {
    let catalog = SdkSourceCatalog::read_from_path(&publication.stdlib_root.join(SDK_SOURCE_CATALOG_FILE))
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    catalog
        .validate_compiler_version(incan_lang::version::INCAN_VERSION)
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    let _lock = acquire_sdk_provider_store_lock(&publication.store_root)?;
    let closure = crate::sdk_native::prepare_sdk_native_closure(&publication.stdlib_root, &catalog, inputs)?;
    let receipts = crate::sdk_native::sdk_native_receipts(&closure)?;
    let identity = crate::sdk_store::sdk_provider_sealed_store_identity(
        &publication.stdlib_root,
        &publication.executable,
        &publication.distribution_profile,
        &receipts,
    )?;
    if let Some(inventory) = load_published_sdk_inventory(&publication.store_root, &identity)? {
        validate_native_sdk_entry(&inventory.root, &receipts)?;
        publish_native_receipt_hint(&publication.store_root, &receipts)?;
        record_sdk_provider_root(&inventory.root)?;
        return Ok(inventory);
    }
    let artifact_root = publication.store_root.join(&identity);
    if artifact_root.exists() {
        return Err(ProviderError::failure(
            "sealed SDK identity already exists without a complete inventory",
        ));
    }
    let staging = staged_sdk_provider_root(&publication.store_root, &identity)?;
    let result = (|| {
        build_sdk_components_into_staging(
            &catalog,
            &staging,
            &closure,
            &inputs.output.join("store"),
            &publication.distribution_profile,
            &mut publish,
        )?;
        let current_identity = crate::sdk_store::sdk_provider_sealed_store_identity(
            &publication.stdlib_root,
            &publication.executable,
            &publication.distribution_profile,
            &receipts,
        )?;
        if current_identity != identity {
            return Err(ProviderError::failure(
                "SDK sources changed while checked components were being published",
            ));
        }
        sync_sdk_provider_tree(&staging)?;
        fs::rename(&staging, &artifact_root).map_err(|error| ProviderError::failure(error.to_string()))?;
        sync_sdk_provider_store(&publication.store_root)?;
        publish_native_receipt_hint(&publication.store_root, &receipts)?;
        let published = load_published_sdk_inventory(&publication.store_root, &identity)?
            .ok_or_else(|| ProviderError::failure("sealed SDK publication lost its inventory"))?;
        record_sdk_provider_root(&artifact_root)?;
        Ok(published)
    })();
    if result.is_err() && staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

/// Publish checked component surfaces in dependency order against the retained native closure and frozen graph.
fn build_sdk_components_into_staging(
    catalog: &SdkSourceCatalog,
    staging: &Path,
    closure: &oven_rustc::sdk_closure::SdkCompiledClosure,
    native_store: &Path,
    profile: &str,
    publish: &mut impl FnMut(
        &Path,
        &Path,
        &SdkInventory,
        &oven_rustc::sdk_closure::SdkCompiledClosure,
    ) -> ProviderResult<()>,
) -> ProviderResult<()> {
    fs::create_dir_all(staging).map_err(|error| ProviderError::failure(error.to_string()))?;
    crate::sdk_native::write_sdk_native_authority(closure, staging)?;
    crate::sdk_native::write_sdk_native_artifact_catalog(closure, native_store, staging)?;
    let mut inventory = source_catalog_inventory(catalog, staging);
    let mut unavailable = std::collections::BTreeMap::new();
    for component in catalog.publication_order() {
        let output = staging.join("components").join(&component.id);
        if let Err(error) = publish(&component.project_root, &output, &inventory, closure) {
            if component.mandatory {
                return Err(error);
            }
            unavailable.insert(component.id.clone(), error.to_string());
            if output.exists() {
                fs::remove_dir_all(&output).map_err(|error| ProviderError::failure(error.to_string()))?;
            }
            continue;
        }
        record_native_component_provider(&mut inventory, component, &output)?;
    }
    fs::write(
        staging.join("unavailable-components.json"),
        serde_json::to_vec_pretty(&unavailable).map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    restrict_staged_sdk_profile(catalog, profile, staging, &mut inventory)?;
    inventory
        .write_to_path(&staging.join(SDK_INVENTORY_FILE))
        .map_err(|error| ProviderError::failure(error.to_string()))
}

/// Atomically replace the discovery hint after its complete immutable SDK generation has been published.
///
/// The store publication lock serializes this sibling write. A cache acquisition repairs a hint lost after the
/// generation rename, so an interrupted hint write cannot strand an otherwise complete SDK.
fn publish_native_receipt_hint(
    store: &Path,
    receipts: &std::collections::BTreeMap<String, String>,
) -> ProviderResult<()> {
    let pending = store.join(".sealed-native-receipts.pending");
    fs::write(
        &pending,
        serde_json::to_vec_pretty(receipts).map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    fs::File::open(&pending)
        .and_then(|file| file.sync_all())
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    fs::rename(pending, store.join(".sealed-native-receipts.json"))
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    sync_sdk_provider_store(store)
}

/// Refuse incomplete or mismatched native generations rather than interpreting an inventory alone as authority.
fn validate_native_sdk_entry(root: &Path, receipts: &std::collections::BTreeMap<String, String>) -> ProviderResult<()> {
    let retained: std::collections::BTreeMap<String, String> = serde_json::from_slice(
        &fs::read(root.join(".sealed-native-receipts.json"))
            .map_err(|error| ProviderError::failure(error.to_string()))?,
    )
    .map_err(|error| ProviderError::failure(error.to_string()))?;
    if &retained != receipts
        || !root.join(".sealed-native-units.json").is_file()
        || !root.join(rust_inspect::OVEN_DIRECT_LOAF_PROJECT_FILE).is_file()
        || !root.join(rust_inspect::OVEN_DIRECT_INSPECTION_AUTHORITY_FILE).is_file()
    {
        return Err(ProviderError::failure(
            "published SDK generation has mismatched or missing native authority",
        ));
    }
    Ok(())
}

/// Validate checked namespace grants and bind a component's complete artifact into the staged SDK inventory.
fn record_native_component_provider(
    inventory: &mut SdkInventory,
    component: &crate::SdkSourceComponent,
    output: &Path,
) -> ProviderResult<()> {
    validate_native_component_payload(output)?;
    let manifest = ProjectManifest::discover(&component.project_root)
        .map_err(|error| ProviderError::failure(error.to_string()))?
        .ok_or_else(|| ProviderError::failure("SDK component has no Loaf declaration"))?;
    let name = manifest
        .project
        .as_ref()
        .and_then(|project| project.name.as_deref())
        .ok_or_else(|| ProviderError::failure("SDK component has no project name"))?;
    let manifest_path = output.join(format!("{name}.incnlib"));
    let checked =
        LibraryManifest::read_from_path(&manifest_path).map_err(|error| ProviderError::failure(error.to_string()))?;
    if checked.name != name
        || manifest.project.as_ref().and_then(|project| project.version.as_deref()) != Some(checked.version.as_str())
    {
        return Err(ProviderError::failure(
            "checked SDK provider identity does not match its component declaration",
        ));
    }
    let namespace_claims = sdk_component_namespace_claims(
        &component.id,
        &component.namespace_roots,
        &checked.contract_metadata.provider.namespace_claims,
    )?;
    let digest = digest_provider_artifact(output).map_err(|error| ProviderError::failure(error.to_string()))?;
    let selected = inventory
        .components
        .get_mut(&component.id)
        .ok_or_else(|| ProviderError::failure("SDK publication lost its component"))?;
    selected.available = true;
    selected.providers = vec![SdkProviderDescriptor {
        name: checked.name,
        version: checked.version,
        digest,
        namespace_claims,
        manifest_path: Some(manifest_path),
        crate_root: Some(output.to_path_buf()),
    }];
    Ok(())
}

/// Reject Cargo metadata, build scripts, links, and special files before any component artifact bytes are read.
fn validate_native_component_payload(root: &Path) -> ProviderResult<()> {
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).map_err(|error| ProviderError::failure(error.to_string()))? {
            let entry = entry.map_err(|error| ProviderError::failure(error.to_string()))?;
            if matches!(
                entry.file_name().to_str(),
                Some("Cargo.toml" | "Cargo.toml.orig" | "Cargo.lock" | "build.rs")
            ) {
                return Err(ProviderError::failure(
                    "native SDK publication cannot admit Cargo metadata or build scripts",
                ));
            }
            let kind = entry
                .file_type()
                .map_err(|error| ProviderError::failure(error.to_string()))?;
            if kind.is_dir() {
                directories.push(entry.path());
            } else if !kind.is_file() {
                return Err(ProviderError::failure(
                    "native SDK publication cannot admit links or special files",
                ));
            }
        }
    }
    Ok(())
}

/// Return the receipt-sealed SDK inventory published for this source checkout, without building or locking.
///
/// Oven consumers never launch the compatibility component publisher on a miss. Their source inventory must bind
/// the same stdlib source bytes, compiler, profile, and retained native receipts as the sealed publication.
/// Compatibility inventories produced with Cargo metadata do not satisfy this source-discovery contract.
///
/// Source reuse requires the sealed publisher's native-receipt catalog. Without it there is no SDK inventory to
/// reuse; consumers must not probe legacy Cargo-based identities to find one. Installed and explicitly selected
/// inventories are discovered before this helper. A corrupt sealed catalog is reported rather than replaced by
/// development workspace resolution.
pub fn find_published_sdk_provider_inventory() -> ProviderResult<Option<Arc<SdkInventory>>> {
    if env::var_os(SDK_PROVIDER_BUILD_ENV).is_some() {
        return Ok(None);
    }
    let has_source_catalog = oven_model::toolchain_layout::find_stdlib_root()
        .is_some_and(|root| root.join(SDK_SOURCE_CATALOG_FILE).is_file());
    if !has_source_catalog {
        return Ok(None);
    }
    let Ok(publication) = SdkProviderPublication::resolve(None, None) else {
        return Ok(None);
    };
    let receipts_path = publication.store_root.join(".sealed-native-receipts.json");
    if !receipts_path.is_file() {
        return Ok(None);
    }
    let receipts = fs::read(&receipts_path)
        .map_err(|error| ProviderError::failure(format!("failed to read sealed SDK native receipts: {error}")))?;
    let receipts = serde_json::from_slice::<std::collections::BTreeMap<String, String>>(&receipts)
        .map_err(|error| ProviderError::failure(format!("invalid sealed SDK native receipts: {error}")))?;
    let identity = crate::sdk_store::sdk_provider_sealed_store_identity(
        &publication.stdlib_root,
        &publication.executable,
        &publication.distribution_profile,
        &receipts,
    )?;
    let inventory = load_published_sdk_inventory(&publication.store_root, &identity)?;
    if let Some(inventory) = inventory.as_ref() {
        validate_native_sdk_entry(&inventory.root, &receipts)?;
    }
    Ok(inventory)
}

/// Every input that decides where, and under which identity, the SDK component providers are published.
///
/// Publication and reuse resolve these the same way, so a reusing command finds exactly the entry a publishing
/// command wrote.
struct SdkProviderPublication {
    /// Canonical standard-library source root holding the component catalog.
    stdlib_root: PathBuf,
    /// Compiler executable whose bytes partition the sealed publication identity.
    executable: PathBuf,
    /// Store whose identity directories hold published inventories.
    store_root: PathBuf,
    /// Distribution profile folded into the store identity.
    distribution_profile: String,
}

impl SdkProviderPublication {
    /// Resolve the publication inputs from the environment, the caller's publisher store and source override.
    fn resolve(publisher_store_root: Option<&Path>, source_root_override: Option<&Path>) -> ProviderResult<Self> {
        let stdlib_root = match source_root_override {
            Some(source_root) => source_root.join("loaves/stdlib"),
            None => oven_model::toolchain_layout::find_stdlib_root().ok_or_else(|| {
                ProviderError::failure(
                    "cannot locate built-in stdlib sources needed to prepare SDK component providers",
                )
            })?,
        };
        let stdlib_root = fs::canonicalize(&stdlib_root).map_err(|error| {
            ProviderError::failure(format!(
                "failed to canonicalize built-in stdlib source directory {}: {error}",
                stdlib_root.display()
            ))
        })?;
        let current_exe = env::current_exe()
            .map_err(|error| ProviderError::failure(format!("failed to resolve current incan executable: {error}")))?;
        let cargo_test_binary = env::var_os("CARGO_BIN_EXE_incan")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        let executable = sdk_provider_builder_executable(cargo_test_binary, current_exe)?;
        let distribution_profile = env::var(INTERNAL_SDK_DISTRIBUTION_PROFILE_ENV)
            .ok()
            .filter(|profile| !profile.is_empty())
            .unwrap_or_else(|| "full".to_string());
        let store_root = publisher_store_root.map(Path::to_path_buf).unwrap_or_else(|| {
            env::var_os(INTERNAL_SDK_PROVIDER_STORE_ENV)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    if cfg!(test) {
                        oven_model::toolchain_layout::development_root().join("target/incan_test_sdk_provider_store")
                    } else {
                        default_sdk_provider_store(
                            &stdlib_root,
                            env::var_os("INCAN_HOME"),
                            env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")),
                        )
                    }
                })
        });
        Ok(Self {
            stdlib_root,
            executable,
            store_root,
            distribution_profile,
        })
    }
}

/// Load the inventory published under `identity` in `store_root`, or `None` when that identity has no entry.
///
/// Unlike [`prepare_native_sdk_provider_inventory`] this takes no store lock and never builds or publishes:
/// publication renames a complete identity directory into place, so an inventory file that exists is whole.
fn load_published_sdk_inventory(store_root: &Path, identity: &str) -> ProviderResult<Option<Arc<SdkInventory>>> {
    let inventory_path = store_root.join(identity).join(SDK_INVENTORY_FILE);
    if !inventory_path.is_file() {
        return Ok(None);
    }
    let inventory =
        SdkInventory::read_from_path(&inventory_path).map_err(|error| ProviderError::failure(error.to_string()))?;
    inventory
        .validate_compiler_compatibility(
            incan_lang::version::INCAN_VERSION,
            incan_lang::version::SDK_PROVIDER_CODEGEN_REVISION,
        )
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    Ok(Some(Arc::new(inventory)))
}

/// Optional operational evidence kept outside the immutable provider store.
#[cfg(test)]
struct SdkBuildReports {
    directory: PathBuf,
    identity: String,
    started: std::time::Instant,
    completed: bool,
}

#[cfg(test)]
impl SdkBuildReports {
    /// Require an existing external directory before creating a unique publication report session.
    fn new(directory: &Path, store: &Path, identity: &str) -> ProviderResult<Self> {
        let directory = fs::canonicalize(directory).map_err(|error| {
            ProviderError::failure(format!(
                "SDK build report directory must already exist: {}: {error}",
                directory.display()
            ))
        })?;
        let store = fs::canonicalize(store).map_err(|error| ProviderError::failure(error.to_string()))?;
        if directory.starts_with(&store) || store.starts_with(&directory) {
            return Err(ProviderError::failure(
                "SDK build report directory must be separate from the provider store",
            ));
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| ProviderError::failure(error.to_string()))?
            .as_nanos();
        let directory = directory.join(format!("sdk-build-{}-{nonce}", std::process::id()));
        fs::create_dir(&directory).map_err(|error| ProviderError::failure(error.to_string()))?;
        let reports = Self {
            directory,
            identity: identity.to_string(),
            started: std::time::Instant::now(),
            completed: false,
        };
        reports.summary("preparing");
        Ok(reports)
    }

    /// Persist telemetry without changing the compiler's publication result or original failure diagnostic.
    fn write(&self, name: &str, value: &serde_json::Value) {
        let result = serde_json::to_vec_pretty(value)
            .map_err(std::io::Error::other)
            .and_then(|bytes| fs::write(self.directory.join(name), bytes));
        if let Err(error) = result {
            eprintln!("warning: SDK build timing report unavailable: {error}");
        }
    }

    /// Describe work after acquiring the store lock; source identity calculation and lock waiting precede this scope.
    fn summary(&self, status: &str) {
        self.write(
            "summary.json",
            &serde_json::json!({
                "schema_version": 1, "status": status, "sdk_store_identity": self.identity,
                "elapsed_scope": "after_store_lock",
                "elapsed_ms": self.started.elapsed().as_millis()
            }),
        );
    }

    /// Mark a successfully validated cache acquisition or completed publication.
    fn finish(&mut self, status: &str) {
        self.summary(status);
        self.completed = true;
    }

    /// Select a component-specific child output; the SDK environment helper disables descendant report sessions.
    fn configure(&self, command: &mut Command, component: &str) {
        command
            .args(["--report", "json", "--report-output"])
            .arg(self.directory.join(format!("{component}.build.json")));
    }

    /// Retain bounded successful phase data and an explicit unavailable reason on missing or malformed child output.
    fn component(&self, component: &str, elapsed: std::time::Duration, output: &std::process::Output) {
        let path = self.directory.join(format!("{component}.build.json"));
        let parsed = (|| -> Result<serde_json::Value, String> {
            let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
            if metadata.len() > 4 * 1024 * 1024 {
                return Err("child report exceeds 4 MiB".to_string());
            }
            let bytes = fs::read(&path).map_err(|error| error.to_string())?;
            let report: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            let timings = report
                .get("timings_ms")
                .and_then(serde_json::Value::as_object)
                .filter(|timings| !timings.is_empty() && timings.values().all(|value| value.as_u64().is_some()))
                .ok_or("child report has no valid timings_ms")?;
            Ok(serde_json::Value::Object(timings.clone()))
        })();
        let (status, timings, reason) = match parsed {
            Ok(timings) => ("available", timings, None),
            Err(reason) => ("unavailable", serde_json::Value::Null, Some(reason)),
        };
        self.write(
            &format!("{component}.timing.json"),
            &serde_json::json!({
                "schema_version": 1, "component": component, "elapsed_ms": elapsed.as_millis(),
                "success": output.status.success(), "exit_code": output.status.code(),
                "report_status": status, "timings_ms": timings, "unavailable_reason": reason,
                "timings_semantics": "inclusive_nested_scopes"
            }),
        );
    }
}

#[cfg(test)]
impl Drop for SdkBuildReports {
    fn drop(&mut self) {
        if !self.completed {
            self.summary("failed");
        }
    }
}

/// Report the exact immutable provider root to release packaging when requested.
fn record_sdk_provider_root(artifact_root: &Path) -> ProviderResult<()> {
    let Some(path_file) = env::var_os(INTERNAL_SDK_PROVIDER_PATH_FILE_ENV).filter(|path| !path.is_empty()) else {
        return Ok(());
    };
    fs::write(&path_file, format!("{}\n", artifact_root.display())).map_err(|error| {
        ProviderError::failure(format!(
            "failed to record SDK provider root in {}: {error}",
            PathBuf::from(path_file).display()
        ))
    })
}

/// Preserve a caller-owned Cargo target while keeping the ordinary provider-publication fallback transaction-local.
#[cfg(test)]
fn configure_sdk_provider_build_environment(
    command: &mut Command,
    component_id: &str,
    transaction_cargo_target: &Path,
    caller_cargo_target: Option<&std::ffi::OsStr>,
    toolchain_source: Option<(&Path, &Path)>,
) {
    let cargo_target_dir = caller_cargo_target.map(Path::new).unwrap_or(transaction_cargo_target);
    command
        .env_remove(INTERNAL_MANIFEST_OVERRIDE_ENV)
        .env_remove(INTERNAL_PROJECT_ROOT_OVERRIDE_ENV)
        .env_remove(INTERNAL_SDK_BUILD_REPORT_DIR_ENV)
        .env(SDK_PROVIDER_BUILD_ENV, component_id)
        .env(INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV, "1")
        // Share transient Cargo artifacts across components, then remove them before immutable provider publication.
        .env(GENERATED_CARGO_TARGET_DIR_ENV, cargo_target_dir);
    if let Some((source_root, stdlib_root)) = toolchain_source {
        command
            .env("INCAN_SOURCE_ROOT", source_root)
            .env("INCAN_STDLIB", stdlib_root);
    }
}

/// Validate producer claims against the namespace grant before publishing them into the SDK inventory.
fn sdk_component_namespace_claims(
    component_id: &str,
    namespace_roots: &BTreeSet<String>,
    claims: &[ProviderModuleClaim],
) -> ProviderResult<BTreeSet<Vec<String>>> {
    let unauthorized = claims
        .iter()
        .filter(|claim| {
            claim
                .module_path
                .first()
                .is_none_or(|root| !namespace_roots.contains(root))
        })
        .map(|claim| claim.module_path.join("."))
        .collect::<Vec<_>>();
    if !unauthorized.is_empty() {
        return Err(ProviderError::failure(format!(
            "SDK component `{component_id}` claims module(s) {} outside its granted namespace roots [{}]",
            unauthorized.join(", "),
            namespace_roots.iter().cloned().collect::<Vec<_>>().join(", ")
        )));
    }

    Ok(claims
        .iter()
        .map(|claim| {
            let mut path = vec![stdlib::STDLIB_ROOT.to_string()];
            path.extend(claim.module_path.iter().cloned());
            path
        })
        .collect())
}

/// Remove provider payloads outside one release distribution profile while retaining their catalog records.
fn restrict_staged_sdk_profile(
    catalog: &SdkSourceCatalog,
    distribution_profile: &str,
    staging_root: &Path,
    inventory: &mut SdkInventory,
) -> ProviderResult<()> {
    if !catalog.profiles.contains_key(distribution_profile) {
        return Err(ProviderError::failure(format!(
            "unknown SDK distribution profile `{distribution_profile}`"
        )));
    }
    let resolved = inventory
        .resolve_catalog(&SdkComponentSelection {
            profile: distribution_profile.to_string(),
            components: BTreeSet::new(),
            exclude_components: BTreeSet::new(),
        })
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    for component in inventory.components.values_mut() {
        if resolved.enabled.contains(&component.id) {
            continue;
        }
        component.available = false;
        for provider in &mut component.providers {
            provider.manifest_path = None;
            provider.crate_root = None;
        }
        let component_root = staging_root.join("components").join(&component.id);
        if component_root.exists() {
            fs::remove_dir_all(&component_root).map_err(|error| {
                ProviderError::failure(format!(
                    "failed to exclude SDK component payload {}: {error}",
                    component_root.display()
                ))
            })?;
        }
    }
    Ok(())
}

/// Create the unavailable installation catalog before component publication begins.
fn source_catalog_inventory(catalog: &SdkSourceCatalog, root: &Path) -> SdkInventory {
    let components = catalog
        .components
        .iter()
        .map(|(id, component)| {
            (
                id.clone(),
                SdkComponent {
                    id: id.clone(),
                    version: catalog.sdk_version.clone(),
                    mandatory: component.mandatory,
                    available: false,
                    dependencies: component.dependencies.clone(),
                    providers: Vec::new(),
                },
            )
        })
        .collect();
    SdkInventory {
        root: root.to_path_buf(),
        sdk_id: catalog.sdk_id.clone(),
        sdk_version: catalog.sdk_version.clone(),
        compiler_requirement: catalog.compiler_requirement.clone(),
        provider_codegen_revision: incan_lang::version::SDK_PROVIDER_CODEGEN_REVISION,
        components,
        profiles: catalog.profiles.clone(),
    }
}

/// Preserve nested compiler stdout and stderr when one component publication fails.
#[cfg(test)]
fn nested_sdk_component_build_error(
    component: &str,
    project_root: &Path,
    output: &std::process::Output,
) -> ProviderError {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let diagnostics = [stderr.trim(), stdout.trim()]
        .into_iter()
        .filter(|message| !message.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    ProviderError::failure(format!(
        "failed to prepare SDK component `{component}` at {}{}",
        project_root.display(),
        if diagnostics.is_empty() {
            String::new()
        } else {
            format!("\n{diagnostics}")
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A changed native output descriptor cannot retarget the publication's admitted receipt binding.
    fn assert_native_catalog_refuses_mutation(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let path = root.join(".sealed-native-units.json");
        let original = fs::read(&path)?;
        let mut changed: serde_json::Value = serde_json::from_slice(&original)?;
        changed["units"][0]["digest"] = serde_json::Value::String(format!("sha256:{}", "0".repeat(64)));
        fs::write(&path, serde_json::to_vec(&changed)?)?;
        assert!(crate::sdk_native::retain_sdk_native_artifacts(root).is_err());
        fs::write(path, original)?;
        Ok(())
    }

    /// Native publication reuses receipts, ignores poisoned Cargo inputs, and leaves failed generations unpublished.
    #[test]
    fn native_sdk_transaction_reuses_and_rolls_back() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let checkout = temp.path().join("checkout");
        let stdlib = checkout.join("loaves/stdlib");
        fs::create_dir_all(&stdlib)?;
        fs::write(
            stdlib.join("sdk-lock.json"),
            r#"{"schema":"incan.oven.loaf-resolution/1","units":[]}"#,
        )?;
        fs::write(
            stdlib.join(SDK_SOURCE_CATALOG_FILE),
            format!(
                "[sdk]\nid='incan'\nversion='{}'\ncompiler-requirement='={}'\n[profiles]\ndefault=['fixture']\nfull=['fixture']\n[components.fixture]\nproject='fixture'\nnamespace-roots=['fixture']\n",
                incan_lang::version::INCAN_VERSION,
                incan_lang::version::INCAN_VERSION,
            ),
        )?;
        for (relative, name, kind) in [
            ("loaves/kernel/incan_lang", "fixture_lang", "lib"),
            ("loaves/kernel/incan_vocab", "fixture_vocab", "lib"),
            ("loaves/stdlib/derive/incan_derive", "fixture_derive", "proc-macro"),
            (
                "loaves/stdlib/derive/incan_web_macros",
                "fixture_web_macros",
                "proc-macro",
            ),
        ] {
            let root = checkout.join(relative);
            fs::create_dir_all(root.join("src"))?;
            fs::write(
                root.join("loaf.toml"),
                format!(
                    "[project]\nname='{name}'\nversion='1.0.0'\n[rust]\nname='{name}'\ntype='{kind}'\nedition='2024'\n",
                ),
            )?;
            fs::write(
                root.join("src/lib.rs"),
                if kind == "proc-macro" {
                    "extern crate proc_macro; #[proc_macro] pub fn identity(input: proc_macro::TokenStream) -> proc_macro::TokenStream { input }"
                } else {
                    "pub fn value() -> u8 { 1 }"
                },
            )?;
            fs::write(root.join("Cargo.toml"), "poisoned metadata")?;
            fs::write(root.join("Cargo.lock"), "poisoned lock")?;
            fs::write(root.join("build.rs"), "compile_error!(\"must remain inert\");")?;
        }
        let component = stdlib.join("fixture");
        fs::create_dir_all(&component)?;
        fs::write(
            component.join("loaf.toml"),
            "[project]\nname='fixture'\nversion='1.0.0'\n",
        )?;
        let executable = temp.path().join("compiler");
        fs::write(&executable, "compiler identity")?;
        let store = temp.path().join("store");
        let inputs = crate::sdk_native::SdkNativeInputs {
            blobs: temp.path().to_path_buf(),
            index: temp.path().to_path_buf(),
            output: store.join(".native"),
            rustc: oven_rustc::rustc::resolve_active_rustc()?,
        };
        let publication = || SdkProviderPublication {
            stdlib_root: stdlib.clone(),
            executable: executable.clone(),
            store_root: store.clone(),
            distribution_profile: "full".to_string(),
        };
        let first = prepare_native_sdk_provider_inventory(publication(), &inputs, |_, output, _, closure| {
            assert_eq!(closure.units().len(), 4);
            fs::create_dir_all(output).map_err(|error| ProviderError::failure(error.to_string()))?;
            LibraryManifest::new("fixture", "1.0.0")
                .write_to_path(&output.join("fixture.incnlib"))
                .map_err(|error| ProviderError::failure(error.to_string()))
        })?;
        assert!(first.root.join(".sealed-native-receipts.json").is_file());
        assert!(first.root.join(rust_inspect::OVEN_DIRECT_LOAF_PROJECT_FILE).is_file());
        assert_eq!(crate::sdk_native::retain_sdk_native_artifacts(&first.root)?.len(), 4);
        assert_native_catalog_refuses_mutation(&first.root)?;
        let second = prepare_native_sdk_provider_inventory(publication(), &inputs, |_, _, _, _| {
            Err(ProviderError::failure(
                "a cache hit must not call the checked publisher",
            ))
        })?;
        assert_eq!(first.root, second.root);
        let previous_hint = fs::read(store.join(".sealed-native-receipts.json"))?;
        fs::remove_file(store.join(".sealed-native-receipts.json"))?;
        prepare_native_sdk_provider_inventory(publication(), &inputs, |_, _, _, _| {
            Err(ProviderError::failure(
                "repairing a hint must reuse the immutable SDK generation",
            ))
        })?;
        assert_eq!(previous_hint, fs::read(store.join(".sealed-native-receipts.json"))?);
        fs::write(
            component.join("loaf.toml"),
            "[project]\nname='fixture'\nversion='1.0.1'\n",
        )?;
        let failure = prepare_native_sdk_provider_inventory(publication(), &inputs, |_, _, _, _| {
            Err(ProviderError::failure("checked publication refused"))
        })
        .err()
        .ok_or("publication should fail")?;
        assert!(failure.to_string().contains("checked publication refused"));
        assert_eq!(previous_hint, fs::read(store.join(".sealed-native-receipts.json"))?);
        assert!(first.root.join(SDK_INVENTORY_FILE).is_file());
        assert!(
            !fs::read_dir(&store)?
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().contains("staging"))
        );
        Ok(())
    }

    #[test]
    fn sdk_build_reports_reject_store_paths_and_distinguish_cache_hits() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let store = root.path().join("store");
        let reports_root = root.path().join("reports");
        fs::create_dir_all(store.join("artifact"))?;
        fs::create_dir(&reports_root)?;
        assert!(SdkBuildReports::new(&store.join("artifact"), &store, "identity").is_err());
        let mut reports = SdkBuildReports::new(&reports_root, &store, "identity")?;
        reports.finish("cache_hit");
        let summary: serde_json::Value = serde_json::from_slice(&fs::read(reports.directory.join("summary.json"))?)?;
        assert_eq!(summary["status"], "cache_hit");
        assert_eq!(summary["elapsed_scope"], "after_store_lock");
        assert_eq!(fs::read_dir(&reports.directory)?.count(), 1);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn sdk_build_reports_transport_mock_children_without_replacing_failures() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        let store = root.path().join("store");
        let reports_root = root.path().join("reports");
        fs::create_dir(&store)?;
        fs::create_dir(&reports_root)?;
        let reports = SdkBuildReports::new(&reports_root, &store, "identity")?;
        let mut child = Command::new("sh");
        child.args([
            "-c",
            r#"printf '%s' '{"timings_ms":{"library_prepare_total":7}}' > "$4""#,
            "mock",
        ]);
        configure_sdk_provider_build_environment(&mut child, "stdlib-data", root.path(), None, None);
        reports.configure(&mut child, "stdlib-data");
        assert!(
            child
                .get_envs()
                .any(|(name, value)| name == INTERNAL_SDK_BUILD_REPORT_DIR_ENV && value.is_none())
        );
        let output = child.output()?;
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
        reports.component("stdlib-data", std::time::Duration::from_millis(11), &output);
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(reports.directory.join("stdlib-data.timing.json"))?)?;
        assert_eq!(value["report_status"], "available");
        assert_eq!(value["timings_semantics"], "inclusive_nested_scopes");
        assert_eq!(value["timings_ms"]["library_prepare_total"], 7);
        let failed = Command::new("sh")
            .args(["-c", "printf original-diagnostic >&2; exit 9"])
            .output()?;
        reports.component("missing", std::time::Duration::ZERO, &failed);
        fs::write(reports.directory.join("malformed.build.json"), "not-json")?;
        reports.component("malformed", std::time::Duration::ZERO, &output);
        for component in ["missing", "malformed"] {
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(reports.directory.join(format!("{component}.timing.json")))?)?;
            assert_eq!(value["report_status"], "unavailable");
            assert!(value["timings_ms"].is_null());
        }
        assert_eq!(failed.status.code(), Some(9));
        assert_eq!(failed.stderr, b"original-diagnostic");
        assert!(
            nested_sdk_component_build_error("missing", root.path(), &failed)
                .to_string()
                .contains("original-diagnostic")
        );
        let directory = reports.directory.clone();
        drop(reports);
        let summary: serde_json::Value = serde_json::from_slice(&fs::read(directory.join("summary.json"))?)?;
        assert_eq!(summary["status"], "failed");
        Ok(())
    }

    #[test]
    fn sdk_provider_build_uses_transaction_local_cargo_target() {
        let mut command = Command::new("incan");
        let target = Path::new("/staging/.cargo-target");
        configure_sdk_provider_build_environment(&mut command, "stdlib-core", target, None, None);
        let configured_target = command
            .get_envs()
            .find_map(|(name, value)| (name == GENERATED_CARGO_TARGET_DIR_ENV).then_some(value))
            .flatten();
        assert_eq!(configured_target, Some(target.as_os_str()));
    }

    #[test]
    fn sdk_provider_build_preserves_caller_owned_cargo_target() {
        let mut command = Command::new("incan");
        let transaction_target = Path::new("/staging/.cargo-target");
        let caller_target = Path::new("/ci/shared-generated-target");
        configure_sdk_provider_build_environment(
            &mut command,
            "stdlib-core",
            transaction_target,
            Some(caller_target.as_os_str()),
            None,
        );
        let configured_target = command
            .get_envs()
            .find_map(|(name, value)| (name == GENERATED_CARGO_TARGET_DIR_ENV).then_some(value))
            .flatten();
        assert_eq!(configured_target, Some(caller_target.as_os_str()));
    }

    #[test]
    fn sdk_provider_build_pins_an_explicit_compiler_source_tree() {
        let mut command = Command::new("incan");
        let target = Path::new("/staging/.cargo-target");
        let compiler_root = Path::new("/compiler");
        let stdlib_root = Path::new("/compiler/loaves/stdlib");
        configure_sdk_provider_build_environment(
            &mut command,
            "stdlib-core",
            target,
            None,
            Some((compiler_root, stdlib_root)),
        );
        let source_root = command
            .get_envs()
            .find_map(|(name, value)| (name == "INCAN_SOURCE_ROOT").then_some(value))
            .flatten();
        let configured_stdlib = command
            .get_envs()
            .find_map(|(name, value)| (name == "INCAN_STDLIB").then_some(value))
            .flatten();
        assert_eq!(source_root, Some(compiler_root.as_os_str()));
        assert_eq!(configured_stdlib, Some(stdlib_root.as_os_str()));
    }

    #[test]
    fn restricted_sdk_profile_retains_unavailable_provider_catalog_facts() -> Result<(), Box<dyn std::error::Error>> {
        let catalog_path = oven_model::toolchain_layout::development_root()
            .join("loaves/stdlib")
            .join(SDK_SOURCE_CATALOG_FILE);
        let catalog = SdkSourceCatalog::read_from_path(&catalog_path)?;
        let tmp = tempfile::tempdir()?;
        let staging_root = tmp.path().join("sdk");
        let component_root = staging_root.join("components/stdlib-system");
        fs::create_dir_all(&component_root)?;
        let mut inventory = source_catalog_inventory(&catalog, &staging_root);
        let system = inventory
            .components
            .get_mut("stdlib-system")
            .ok_or("missing stdlib-system component")?;
        system.available = true;
        system.providers.push(SdkProviderDescriptor {
            name: "incan_stdlib_system".to_string(),
            version: "0.5.0".to_string(),
            digest: "sha256:fixture".to_string(),
            namespace_claims: BTreeSet::from([vec!["std".to_string(), "fs".to_string(), "path".to_string()]]),
            manifest_path: Some(component_root.join("incan_stdlib_system.incnlib")),
            crate_root: Some(component_root.clone()),
        });

        restrict_staged_sdk_profile(&catalog, "minimal", &staging_root, &mut inventory)?;

        let system = inventory
            .components
            .get("stdlib-system")
            .ok_or("missing restricted stdlib-system component")?;
        let provider = system
            .providers
            .first()
            .ok_or("missing unavailable provider descriptor")?;
        assert!(!system.available);
        assert!(provider.manifest_path.is_none());
        assert!(provider.crate_root.is_none());
        assert!(
            provider
                .namespace_claims
                .contains(&vec!["std".to_string(), "fs".to_string(), "path".to_string(),])
        );
        assert!(!component_root.exists());
        Ok(())
    }

    #[test]
    fn sdk_component_publication_rejects_namespace_claims_outside_its_grant() -> Result<(), Box<dyn std::error::Error>>
    {
        let claims = vec![
            ProviderModuleClaim {
                module_path: vec!["json".to_string()],
                required_features: BTreeSet::new(),
            },
            ProviderModuleClaim {
                module_path: vec!["web".to_string(), "routing".to_string()],
                required_features: BTreeSet::new(),
            },
        ];

        let error = sdk_component_namespace_claims("stdlib-data", &BTreeSet::from(["json".to_string()]), &claims)
            .err()
            .ok_or("unauthorized namespace claim should fail SDK component publication")?;

        assert!(error.message.contains("stdlib-data"));
        assert!(error.message.contains("web.routing"));
        assert!(error.message.contains("outside its granted namespace roots"));
        Ok(())
    }
}
