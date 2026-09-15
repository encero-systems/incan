//! Preparing the SDK provider inventory in the store: staging, building the components into it, restricting a
//! staged profile, and reporting what the build did.
//!
//! This is the explicit publication path. Discovery of an already published inventory is `inventory`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::{env, fs};

use incan_core::lang::stdlib;

use crate::error::{ProviderError, ProviderResult};
use crate::inventory::SDK_INVENTORY_OVERRIDE_ENV;
use crate::requirements::INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV;
use crate::sdk_store::{
    INTERNAL_SDK_DISTRIBUTION_PROFILE_ENV, INTERNAL_SDK_PROVIDER_PATH_FILE_ENV, INTERNAL_SDK_PROVIDER_STORE_ENV,
    acquire_sdk_provider_store_lock, configure_sdk_provider_workspace_lock, default_sdk_provider_store,
    sdk_provider_builder_executable, sdk_provider_store_identity, sdk_provider_workspace_lock,
    staged_sdk_provider_root, sync_sdk_provider_store, sync_sdk_provider_tree,
};
use crate::{
    SDK_INVENTORY_FILE, SDK_PROVIDER_BUILD_ENV, SDK_SOURCE_CATALOG_FILE, SdkComponent, SdkComponentSelection,
    SdkInventory, SdkProviderDescriptor, SdkSourceCatalog,
};
use incan_frontend::library_manifest::{LibraryManifest, ProviderModuleClaim, digest_provider_artifact};
use oven_model::manifest::{INTERNAL_MANIFEST_OVERRIDE_ENV, INTERNAL_PROJECT_ROOT_OVERRIDE_ENV, ProjectManifest};
use oven_model::toolchain_layout::GENERATED_CARGO_TARGET_DIR_ENV;
/// Optional external directory for SDK publication timing evidence.
const INTERNAL_SDK_BUILD_REPORT_DIR_ENV: &str = "INCAN_INTERNAL_SDK_BUILD_REPORT_DIR";

/// Build and atomically publish every SDK component provider from the source catalog.
pub fn prepare_sdk_provider_inventory() -> ProviderResult<Arc<SdkInventory>> {
    prepare_sdk_provider_inventory_in_store(None, None)
}

/// Build SDK providers into a publisher-owned store rather than the normal SDK cache.
///
/// This is for the explicitly named Oven `legacy_cargo` transition only. The caller is responsible for copying the
/// resulting immutable inventory into its receipt-bound artifact before the private publisher root is reclaimed.
pub fn prepare_sdk_provider_inventory_in_store(
    publisher_store_root: Option<&Path>,
    source_root_override: Option<&Path>,
) -> ProviderResult<Arc<SdkInventory>> {
    let stdlib_root = match source_root_override {
        Some(source_root) => source_root.join("crates/incan_stdlib/stdlib"),
        None => oven_model::toolchain_layout::find_stdlib_source_dir().ok_or_else(|| {
            ProviderError::failure("cannot locate built-in stdlib sources needed to prepare SDK component providers")
        })?,
    };
    let stdlib_root = fs::canonicalize(&stdlib_root).map_err(|error| {
        ProviderError::failure(format!(
            "failed to canonicalize built-in stdlib source directory {}: {error}",
            stdlib_root.display()
        ))
    })?;
    let catalog = SdkSourceCatalog::read_from_path(&stdlib_root.join(SDK_SOURCE_CATALOG_FILE))
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    catalog
        .validate_compiler_version(incan_core::version::INCAN_VERSION)
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    let current_exe = env::current_exe()
        .map_err(|error| ProviderError::failure(format!("failed to resolve current incan executable: {error}")))?;
    let cargo_test_binary = env::var_os("CARGO_BIN_EXE_incan")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let executable = sdk_provider_builder_executable(cargo_test_binary, current_exe)?;
    let workspace_lock = sdk_provider_workspace_lock(&stdlib_root);
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
    let identity = sdk_provider_store_identity(
        &stdlib_root,
        &executable,
        workspace_lock.as_deref(),
        &distribution_profile,
    )?;
    let _lock = acquire_sdk_provider_store_lock(&store_root)?;
    let mut build_reports = env::var_os(INTERNAL_SDK_BUILD_REPORT_DIR_ENV)
        .filter(|path| !path.is_empty())
        .map(|path| SdkBuildReports::new(Path::new(&path), &store_root, &identity))
        .transpose()?;
    let artifact_root = store_root.join(&identity);
    let inventory_path = artifact_root.join(SDK_INVENTORY_FILE);
    if inventory_path.is_file() {
        let inventory =
            SdkInventory::read_from_path(&inventory_path).map_err(|error| ProviderError::failure(error.to_string()))?;
        inventory
            .validate_compiler_compatibility(
                incan_core::version::INCAN_VERSION,
                incan_core::version::SDK_PROVIDER_CODEGEN_REVISION,
            )
            .map_err(|error| ProviderError::failure(error.to_string()))?;
        record_sdk_provider_root(&artifact_root)?;
        if let Some(reports) = &mut build_reports {
            reports.finish("cache_hit");
        }
        return Ok(Arc::new(inventory));
    }
    if artifact_root.exists() {
        return Err(ProviderError::failure(format!(
            "compiled SDK component artifact at {} is incomplete; refusing to overwrite an already published identity",
            artifact_root.display()
        )));
    }

    let staging_root = staged_sdk_provider_root(&store_root, &identity)?;
    let staged_inventory = match build_sdk_components_into_staging(
        &catalog,
        &executable,
        workspace_lock.as_deref(),
        &staging_root,
        &distribution_profile,
        source_root_override.map(|source_root| (source_root, stdlib_root.as_path())),
        build_reports.as_ref(),
    ) {
        Ok(inventory) => inventory,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging_root);
            return Err(error);
        }
    };
    sync_sdk_provider_tree(&staging_root)?;
    fs::rename(&staging_root, &artifact_root).map_err(|error| {
        ProviderError::failure(format!(
            "failed to publish compiled SDK components from {} to {}: {error}",
            staging_root.display(),
            artifact_root.display()
        ))
    })?;
    sync_sdk_provider_store(&store_root)?;
    let published_inventory_path = artifact_root.join(SDK_INVENTORY_FILE);
    let published = SdkInventory::read_from_path(&published_inventory_path).map_err(|error| {
        ProviderError::failure(format!(
            "failed to load published SDK component inventory for {}: {error}",
            staged_inventory.identity()
        ))
    })?;
    record_sdk_provider_root(&artifact_root)?;
    if let Some(reports) = &mut build_reports {
        reports.finish("published");
    }
    Ok(Arc::new(published))
}

/// Optional operational evidence kept outside the immutable provider store.
struct SdkBuildReports {
    directory: PathBuf,
    identity: String,
    started: std::time::Instant,
    completed: bool,
}

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

/// Build source components in dependency order while exposing only already-published providers to each producer.
fn build_sdk_components_into_staging(
    catalog: &SdkSourceCatalog,
    executable: &Path,
    workspace_lock: Option<&Path>,
    staging_root: &Path,
    distribution_profile: &str,
    toolchain_source: Option<(&Path, &Path)>,
    build_reports: Option<&SdkBuildReports>,
) -> ProviderResult<SdkInventory> {
    fs::create_dir_all(staging_root).map_err(|error| {
        ProviderError::failure(format!(
            "failed to create SDK component staging directory {}: {error}",
            staging_root.display()
        ))
    })?;
    if let Some(workspace_lock) = workspace_lock {
        fs::copy(workspace_lock, staging_root.join("Cargo.lock")).map_err(|error| {
            ProviderError::failure(format!(
                "failed to publish shared SDK provider lock from {}: {error}",
                workspace_lock.display()
            ))
        })?;
    }
    let mut inventory = source_catalog_inventory(catalog, staging_root);
    let inventory_path = staging_root.join(SDK_INVENTORY_FILE);
    let cargo_target_dir = staging_root.join(".cargo-target");
    let caller_cargo_target = env::var_os(GENERATED_CARGO_TARGET_DIR_ENV).filter(|path| !path.is_empty());
    let mut built_any = false;

    for component in catalog.publication_order() {
        let output_root = staging_root.join("components").join(&component.id);
        let manifest = ProjectManifest::discover(&component.project_root)
            .map_err(|error| ProviderError::failure(error.to_string()))?
            .ok_or_else(|| {
                ProviderError::failure(format!(
                    "SDK component `{}` has no loaf.toml at {}",
                    component.id,
                    component.project_root.display()
                ))
            })?;
        let provider_name = manifest
            .project
            .as_ref()
            .and_then(|project| project.name.clone())
            .ok_or_else(|| ProviderError::failure(format!("SDK component `{}` has no project name", component.id)))?;
        eprintln!(
            "Preparing SDK component `{}` with `incan build --lib` in {}",
            component.id,
            component.project_root.display()
        );
        let mut command = Command::new(executable);
        command
            .current_dir(&component.project_root)
            .args(["build", "--lib", "."])
            .arg(&output_root)
            .arg("--all-features");
        configure_sdk_provider_build_environment(
            &mut command,
            &component.id,
            &cargo_target_dir,
            caller_cargo_target.as_deref(),
            toolchain_source,
        );
        if built_any {
            inventory
                .write_to_path(&inventory_path)
                .map_err(|error| ProviderError::failure(error.to_string()))?;
            command.env(SDK_INVENTORY_OVERRIDE_ENV, &inventory_path);
        } else {
            command.env_remove(SDK_INVENTORY_OVERRIDE_ENV);
        }
        configure_sdk_provider_workspace_lock(&mut command, workspace_lock);
        if let Some(reports) = build_reports {
            reports.configure(&mut command, &component.id);
        }
        let component_started = std::time::Instant::now();
        let output = command.output().map_err(|error| {
            ProviderError::failure(format!(
                "failed to run SDK component build for `{}` at {}: {error}",
                component.id,
                component.project_root.display()
            ))
        })?;
        if let Some(reports) = build_reports {
            reports.component(&component.id, component_started.elapsed(), &output);
        }
        if !output.status.success() {
            return Err(nested_sdk_component_build_error(
                component.id.as_str(),
                &component.project_root,
                &output,
            ));
        }
        let manifest_path = output_root.join(format!("{provider_name}.incnlib"));
        let provider_manifest = LibraryManifest::read_from_path(&manifest_path).map_err(|error| {
            ProviderError::failure(format!(
                "failed to read SDK component `{}` manifest {}: {error}",
                component.id,
                manifest_path.display()
            ))
        })?;
        let component_lock = output_root.join("Cargo.lock");
        if component_lock.is_file() {
            fs::remove_file(&component_lock).map_err(|error| {
                ProviderError::failure(format!(
                    "failed to remove duplicated SDK component lock {}: {error}",
                    component_lock.display()
                ))
            })?;
        }
        let namespace_claims = sdk_component_namespace_claims(
            &component.id,
            &component.namespace_roots,
            &provider_manifest.contract_metadata.provider.namespace_claims,
        )?;
        let digest = digest_provider_artifact(&output_root).map_err(|error| {
            ProviderError::failure(format!(
                "failed to hash SDK component `{}` artifact {}: {error}",
                component.id,
                output_root.display()
            ))
        })?;
        let inventory_component = inventory.components.get_mut(&component.id).ok_or_else(|| {
            ProviderError::failure(format!(
                "SDK source catalog lost component `{}` while publishing",
                component.id
            ))
        })?;
        inventory_component.available = true;
        inventory_component.providers = vec![SdkProviderDescriptor {
            name: provider_manifest.name,
            version: provider_manifest.version,
            digest,
            namespace_claims,
            manifest_path: Some(manifest_path),
            crate_root: Some(output_root),
        }];
        built_any = true;
    }
    if cargo_target_dir.exists() {
        fs::remove_dir_all(&cargo_target_dir).map_err(|error| {
            ProviderError::failure(format!(
                "failed to remove transient SDK provider Cargo target {}: {error}",
                cargo_target_dir.display()
            ))
        })?;
    }
    restrict_staged_sdk_profile(catalog, distribution_profile, staging_root, &mut inventory)?;
    inventory
        .write_to_path(&inventory_path)
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    Ok(inventory)
}

/// Preserve a caller-owned Cargo target while keeping the ordinary provider-publication fallback transaction-local.
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
        provider_codegen_revision: incan_core::version::SDK_PROVIDER_CODEGEN_REVISION,
        components,
        profiles: catalog.profiles.clone(),
    }
}

/// Preserve nested compiler stdout and stderr when one component publication fails.
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
        let stdlib_root = Path::new("/compiler/crates/incan_stdlib/stdlib");
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
            .join("crates/incan_stdlib/stdlib")
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
