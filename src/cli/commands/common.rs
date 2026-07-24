//! Shared utilities used across multiple CLI command pipelines.
//!
//! This module contains functions for source file reading, module collection, project root resolution,
//! dependency helpers, and Cargo flag construction.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, LazyLock, Mutex};

#[cfg(feature = "rust_inspect")]
use crate::backend::ProjectGenerator;
use crate::backend::ir::detect_serde_non_import_usage;
use crate::backend::project::generator::GENERATED_CARGO_TARGET_DIR_ENV;
use crate::backend::project::{GENERATED_TOOLCHAIN_SUPPORT_CRATES, INCAN_STDLIB_CRATE_NAME};
use crate::cli::prelude::ParsedModule;
use crate::cli::{CliError, CliResult};
use crate::dependency_resolver::ResolvedDependencies;
use crate::dependency_resolver::{DependencyError, InlineRustImport};
use crate::frontend::ast::{ImportKind, ImportPath, Program, Span};
use crate::frontend::contract_metadata::{
    CanonicalModelBundle, materialize_contract_models, read_project_model_bundles,
};
use crate::frontend::hir::build_semantic_module_snapshot_v0;
use crate::frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestFailureKind, LibraryManifestIndex, LibraryManifestIndexEntry,
};
use crate::frontend::module::{
    SourceModuleImportResolution, canonicalize_source_module_segments, logical_source_import_candidates,
    resolve_program_source_imports,
};
use crate::frontend::testing_markers::{
    TestingMarkerSemantics, load_testing_marker_semantics, testing_marker_semantics_from_manifest,
};
use crate::frontend::typechecker::TypeCheckInfo;
use crate::frontend::typechecker::stdlib_loader::StdlibAstCache;
use crate::frontend::{ast_walk, diagnostics, lexer, parser, typechecker, vocab_desugar_pass};
use crate::library_manifest::{
    LibraryManifest, ProviderCargoDependency, ProviderCargoDependencySource, ProviderModuleClaim,
    digest_provider_artifact,
};
use crate::lockfile::CargoFeatureSelection;
use crate::manifest::{DependencySource, DependencySpec};
use crate::manifest::{
    INTERNAL_MANIFEST_OVERRIDE_ENV, INTERNAL_PROJECT_ROOT_OVERRIDE_ENV, MANIFEST_FILENAME, ProjectManifest,
};
use crate::project_lifecycle::toolchain::ToolchainConstraintSet;
use crate::provider::{
    BackendImplementationRequirement, FeatureSelection, PackageFeatureGraph, PackageFeaturePlan,
    ProviderModuleResolution, ProviderPlan, ProviderProvenance, ResolvedSdkComponents, SDK_INVENTORY_FILE,
    SDK_PROVIDER_BUILD_ENV, SDK_SOURCE_CATALOG_FILE, SdkArtifactProjection, SdkComponent, SdkComponentSelection,
    SdkDependencyRebinding, SdkInventory, SdkProviderDescriptor, SdkResolutionError, SdkSourceCatalog,
};
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect::{Inspector, InspectorConfig};
use crate::workspace::WorkspaceGraph;
use incan_core::lang::{
    stdlib::{self, StdlibExtraCrateDep, StdlibExtraCrateSource},
    surface::result_methods,
};
use sha2::{Digest, Sha256};

use super::vocab_extraction::collect_library_vocab_metadata_for_parser;

/// Maximum source file size (100 MB)
///
/// Files larger than this are rejected to prevent out-of-memory conditions during compilation.
const MAX_SOURCE_SIZE: u64 = 100 * 1024 * 1024;
static PREPARED_LIBRARY_DEPENDENCIES: LazyLock<Mutex<HashMap<PathBuf, BTreeSet<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static SDK_PROVIDER_COMPILER_DIGESTS: LazyLock<Mutex<HashMap<PathBuf, [u8; 32]>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
pub(crate) const INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV: &str = "INCAN_INTERNAL_LIBRARY_ARTIFACT_ONLY";
/// Internal provider-store override used by isolated compiler and packaging tests.
const INTERNAL_SDK_PROVIDER_STORE_ENV: &str = "INCAN_INTERNAL_SDK_PROVIDER_STORE";
/// Internal file through which release packaging receives the exact immutable SDK provider root.
const INTERNAL_SDK_PROVIDER_PATH_FILE_ENV: &str = "INCAN_INTERNAL_SDK_PROVIDER_PATH_FILE";
/// Internal SDK distribution profile used by release packaging to omit component payloads physically.
const INTERNAL_SDK_DISTRIBUTION_PROFILE_ENV: &str = "INCAN_INTERNAL_SDK_DISTRIBUTION_PROFILE";
/// Internal path override for the Cargo.lock payload used while producing a compiler-owned artifact.
pub(crate) const INTERNAL_CARGO_LOCK_PAYLOAD_PATH_ENV: &str = "INCAN_INTERNAL_CARGO_LOCK_PAYLOAD_PATH";
/// Explicit active SDK inventory override used by toolchain selection and SDK publication.
pub(crate) const SDK_INVENTORY_OVERRIDE_ENV: &str = "INCAN_SDK_INVENTORY";

/// One compiler diagnostic with enough source context for either human or machine-readable rendering.
#[derive(Debug, Clone)]
pub(crate) struct CliDiagnostic {
    pub file_path: String,
    pub source: String,
    pub error: diagnostics::CompileError,
    pub phase: diagnostics::DiagnosticPhase,
}

/// Structured failure produced by shared CLI collection/typechecking helpers.
#[derive(Debug, Clone)]
pub(crate) struct CliDiagnosticFailure {
    pub diagnostics: Vec<CliDiagnostic>,
}

impl CliDiagnosticFailure {
    /// Build one structured diagnostic failure while preserving the source text needed for JSON span projection.
    pub(crate) fn single(
        file_path: impl Into<String>,
        source: impl Into<String>,
        error: diagnostics::CompileError,
        phase: diagnostics::DiagnosticPhase,
    ) -> Self {
        Self {
            diagnostics: vec![CliDiagnostic {
                file_path: file_path.into(),
                source: source.into(),
                error,
                phase,
            }],
        }
    }

    /// Build one structured failure from parser or typechecker errors that all belong to the same source file.
    pub(crate) fn from_errors(
        file_path: impl Into<String>,
        source: impl Into<String>,
        errors: Vec<diagnostics::CompileError>,
        phase: diagnostics::DiagnosticPhase,
    ) -> Self {
        let file_path = file_path.into();
        let source = source.into();
        Self {
            diagnostics: errors
                .into_iter()
                .map(|error| CliDiagnostic {
                    file_path: file_path.clone(),
                    source: source.clone(),
                    error,
                    phase,
                })
                .collect(),
        }
    }

    /// Render the structured diagnostics through the existing source-highlighted human diagnostic formatter.
    pub(crate) fn render_human(&self) -> String {
        let mut rendered = String::new();
        for diagnostic in &self.diagnostics {
            rendered.push_str(&diagnostics::format_error(
                &diagnostic.file_path,
                &diagnostic.source,
                &diagnostic.error,
            ));
            rendered.push('\n');
        }
        rendered.trim_end().to_string()
    }
}

impl From<CliError> for CliDiagnosticFailure {
    fn from(error: CliError) -> Self {
        Self::single(
            "<command>",
            "",
            diagnostics::CompileError::new(error.message, Span::default()),
            diagnostics::DiagnosticPhase::Tooling,
        )
    }
}

#[derive(Debug, Clone)]
struct SourceReadFailure {
    message: String,
}

/// Unified project requirements collected from parsed modules and loaded provider manifests.
#[derive(Debug, Clone, Default)]
pub(crate) struct ProjectRequirements {
    /// Required stdlib feature flags, such as `json`, `async`, and `web`.
    pub stdlib_features: Vec<String>,
    /// Required Cargo dependencies contributed by stdlib namespaces and provider manifests.
    pub dependencies: Vec<DependencySpec>,
    /// Immutable compiled-library projections that replace obsolete physical SDK cache coordinates.
    pub sdk_dependency_rebindings: Vec<SdkDependencyRebinding>,
    /// Path dependencies proven to be owned by the active SDK/toolchain rather than an ordinary project source.
    pub sdk_path_dependencies: Vec<DependencySpec>,
    /// Complete compiled-artifact closure whose transitive coordinates must be projected together.
    pub sdk_artifact_projections: Vec<SdkArtifactProjection>,
}

/// Select the Incan CLI executable that prepares SDK provider artifacts.
///
/// Cargo integration tests and development utilities do not run inside the `incan` CLI. Tests receive the real binary
/// through `CARGO_BIN_EXE_incan`; utility binaries use the sibling CLI built in the same target directory. Returning an
/// error is important: executing a generator with CLI arguments can exit successfully without publishing an artifact.
fn sdk_provider_builder_executable(
    cargo_test_binary: Option<PathBuf>,
    current_executable: PathBuf,
) -> CliResult<PathBuf> {
    if let Some(executable) = cargo_test_binary.filter(|path| path.is_file()) {
        return Ok(executable);
    }

    let binary_dir = current_executable.parent().unwrap_or_else(|| Path::new("."));
    let mut sibling = binary_dir.join("incan");
    sibling.set_extension(std::env::consts::EXE_EXTENSION);
    if sibling.is_file() {
        return Ok(sibling);
    }

    let mut parent_sibling = binary_dir.parent().unwrap_or_else(|| Path::new(".")).join("incan");
    parent_sibling.set_extension(std::env::consts::EXE_EXTENSION);
    if parent_sibling.is_file() {
        return Ok(parent_sibling);
    }

    Err(CliError::failure(format!(
        "SDK provider publication requires the incan CLI executable at {} or {}; build that binary before running compiler-backed utilities",
        sibling.display(),
        parent_sibling.display()
    )))
}

/// Find the verified workspace Cargo.lock available to a development SDK provider build.
///
/// A standalone artifact crate otherwise resolves its own newest compatible versions, which can differ from the
/// compiler workspace's verified offline cache. Installed SDK layouts need not contain a workspace lockfile, so they
/// deliberately retain normal Cargo resolution.
fn sdk_provider_workspace_lock(stdlib_root: &Path) -> Option<PathBuf> {
    stdlib_root
        .ancestors()
        .skip(1)
        .map(|parent| parent.join("Cargo.lock"))
        .find(|path| path.is_file())
        .map(|path| fs::canonicalize(&path).unwrap_or(path))
}

/// Seed a development SDK provider build from the verified enclosing workspace lockfile.
fn seed_sdk_provider_workspace_lock(workspace_lock: Option<&Path>, artifact_root: &Path) -> CliResult<()> {
    let Some(workspace_lock) = workspace_lock else {
        return Ok(());
    };
    fs::create_dir_all(artifact_root).map_err(|error| {
        CliError::failure(format!(
            "failed to create SDK provider artifact directory {}: {error}",
            artifact_root.display()
        ))
    })?;
    fs::copy(workspace_lock, artifact_root.join("Cargo.lock")).map_err(|error| {
        CliError::failure(format!(
            "failed to seed SDK provider artifact lock from {}: {error}",
            workspace_lock.display()
        ))
    })?;
    Ok(())
}

/// Keep the bootstrap artifact lock alive for the whole preparation/publish transaction.
///
/// The compiler cannot call its own Incan `std.fs` artifact before that artifact exists. This is therefore a
/// deliberately narrow native bootstrap boundary, mirroring RFC 112's advisory-lock contract while the compiler
/// produces the first Incan-owned stdlib artifact.
struct SdkProviderStoreLock {
    _file: fs::File,
}

/// Acquire the artifact-store lock that serializes all bootstrap builds and publications.
fn acquire_sdk_provider_store_lock(store_root: &Path) -> CliResult<SdkProviderStoreLock> {
    fs::create_dir_all(store_root).map_err(|error| {
        CliError::failure(format!(
            "failed to create SDK provider store {}: {error}",
            store_root.display()
        ))
    })?;
    let lock_path = store_root.join(".incan.lock");
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| CliError::failure(format!("failed to open artifact lock {}: {error}", lock_path.display())))?;
    file.lock().map_err(|error| {
        CliError::failure(format!(
            "failed to acquire artifact lock {}: {error}",
            lock_path.display()
        ))
    })?;
    Ok(SdkProviderStoreLock { _file: file })
}

/// Hash one sorted provider source subtree while excluding generated build output.
fn hash_sdk_provider_source_tree(root: &Path, current: &Path, hasher: &mut Sha256) -> CliResult<()> {
    let mut entries = fs::read_dir(current)
        .map_err(|error| {
            CliError::failure(format!(
                "failed to read stdlib source directory {}: {error}",
                current.display()
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            CliError::failure(format!(
                "failed to enumerate stdlib source directory {}: {error}",
                current.display()
            ))
        })?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|error| {
            CliError::failure(format!(
                "failed to make stdlib source path {} relative: {error}",
                path.display()
            ))
        })?;
        if relative.components().any(|component| component.as_os_str() == "target") {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| {
            CliError::failure(format!(
                "failed to inspect stdlib source path {}: {error}",
                path.display()
            ))
        })?;
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0]);
        if file_type.is_dir() {
            hasher.update(b"directory\0");
            hash_sdk_provider_source_tree(root, &path, hasher)?;
        } else if file_type.is_file() {
            hasher.update(b"file\0");
            let bytes = fs::read(&path).map_err(|error| {
                CliError::failure(format!("failed to read stdlib source file {}: {error}", path.display()))
            })?;
            hasher.update(bytes);
        } else if file_type.is_symlink() {
            hasher.update(b"symlink\0");
            let target = fs::read_link(&path).map_err(|error| {
                CliError::failure(format!(
                    "failed to read stdlib source symlink {}: {error}",
                    path.display()
                ))
            })?;
            hasher.update(target.to_string_lossy().as_bytes());
        }
        hasher.update([0xff]);
    }
    Ok(())
}

/// Derive the immutable provider-store identity from every input that can change generated Rust or its dependency
/// closure. The identity is content based, so a stale provider set is never accepted because a directory exists.
fn sdk_provider_store_identity(
    source_root: &Path,
    executable: &Path,
    workspace_lock: Option<&Path>,
    distribution_profile: &str,
) -> CliResult<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"incan-sdk-provider-store-v2\0");
    hash_sdk_provider_source_tree(source_root, source_root, &mut hasher)?;
    hasher.update(b"compiler-version\0");
    hasher.update(crate::version::INCAN_VERSION.as_bytes());
    hasher.update(b"distribution-profile\0");
    hasher.update(distribution_profile.as_bytes());

    hasher.update(b"compiler-executable-content\0");
    let executable = fs::canonicalize(executable).unwrap_or_else(|_| executable.to_path_buf());
    hasher.update(sdk_provider_compiler_digest(&executable)?);

    hasher.update(b"workspace-lock\0");
    if let Some(workspace_lock) = workspace_lock {
        hasher.update(fs::read(workspace_lock).map_err(|error| {
            CliError::failure(format!(
                "failed to read workspace lock {}: {error}",
                workspace_lock.display()
            ))
        })?);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Hash the running compiler once per process with BLAKE3's optimized implementation, independent of its path.
fn sdk_provider_compiler_digest(executable: &Path) -> CliResult<[u8; 32]> {
    if let Some(digest) = SDK_PROVIDER_COMPILER_DIGESTS
        .lock()
        .map_err(|_| CliError::failure("failed to lock the compiler-content digest cache"))?
        .get(executable)
        .copied()
    {
        return Ok(digest);
    }

    let mut executable_file = fs::File::open(executable).map_err(|error| {
        CliError::failure(format!(
            "failed to read compiler executable {}: {error}",
            executable.display()
        ))
    })?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = executable_file.read(&mut buffer).map_err(|error| {
            CliError::failure(format!(
                "failed to read compiler executable {}: {error}",
                executable.display()
            ))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = *hasher.finalize().as_bytes();
    SDK_PROVIDER_COMPILER_DIGESTS
        .lock()
        .map_err(|_| CliError::failure("failed to lock the compiler-content digest cache"))?
        .insert(executable.to_path_buf(), digest);
    Ok(digest)
}

/// Select one user-shared development cache instead of duplicating identical provider artifacts in every checkout.
fn default_sdk_provider_store(
    stdlib_root: &Path,
    incan_home: Option<std::ffi::OsString>,
    user_home: Option<std::ffi::OsString>,
) -> PathBuf {
    incan_home
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            user_home
                .filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(path).join(".incan"))
        })
        .map(|root| root.join("cache").join("providers").join("sdk-v2"))
        .unwrap_or_else(|| stdlib_root.join("target").join("incan_sdk_components"))
}

/// Flush every staged artifact file and directory before atomic publication.
fn sync_sdk_provider_tree(path: &Path) -> CliResult<()> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| {
            CliError::failure(format!(
                "failed to read staged artifact directory {}: {error}",
                path.display()
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            CliError::failure(format!(
                "failed to enumerate staged artifact directory {}: {error}",
                path.display()
            ))
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let entry_path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            CliError::failure(format!(
                "failed to inspect staged artifact path {}: {error}",
                entry_path.display()
            ))
        })?;
        if file_type.is_dir() {
            sync_sdk_provider_tree(&entry_path)?;
        } else if file_type.is_file() {
            fs::File::open(&entry_path)
                .and_then(|file| file.sync_all())
                .map_err(|error| {
                    CliError::failure(format!(
                        "failed to synchronize staged artifact file {}: {error}",
                        entry_path.display()
                    ))
                })?;
        }
    }
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            CliError::failure(format!(
                "failed to synchronize staged artifact directory {}: {error}",
                path.display()
            ))
        })
}

/// Flush the artifact store after publishing a new immutable artifact directory.
fn sync_sdk_provider_store(store_root: &Path) -> CliResult<()> {
    fs::File::open(store_root)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            CliError::failure(format!(
                "failed to synchronize artifact store {}: {error}",
                store_root.display()
            ))
        })
}

/// Allocate a unique private staging directory for one artifact identity.
fn staged_sdk_provider_root(store_root: &Path, identity: &str) -> CliResult<PathBuf> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| CliError::failure(format!("system clock predates Unix epoch: {error}")))?;
    Ok(store_root.join(format!(
        ".staging-{identity}-{}-{}",
        std::process::id(),
        elapsed.as_nanos()
    )))
}

/// Build and atomically publish every SDK component provider from the source catalog.
fn prepare_sdk_provider_inventory() -> CliResult<Arc<SdkInventory>> {
    let stdlib_root = crate::cli::prelude::find_stdlib_dir().ok_or_else(|| {
        CliError::failure("cannot locate built-in stdlib sources needed to prepare SDK component providers")
    })?;
    let stdlib_root = fs::canonicalize(&stdlib_root).map_err(|error| {
        CliError::failure(format!(
            "failed to canonicalize built-in stdlib source directory {}: {error}",
            stdlib_root.display()
        ))
    })?;
    let catalog = SdkSourceCatalog::read_from_path(&stdlib_root.join(SDK_SOURCE_CATALOG_FILE))
        .map_err(|error| CliError::failure(error.to_string()))?;
    let current_exe = env::current_exe()
        .map_err(|error| CliError::failure(format!("failed to resolve current incan executable: {error}")))?;
    let cargo_test_binary = env::var_os("CARGO_BIN_EXE_incan")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let executable = sdk_provider_builder_executable(cargo_test_binary, current_exe)?;
    let workspace_lock = sdk_provider_workspace_lock(&stdlib_root);
    let distribution_profile = env::var(INTERNAL_SDK_DISTRIBUTION_PROFILE_ENV)
        .ok()
        .filter(|profile| !profile.is_empty())
        .unwrap_or_else(|| "full".to_string());
    let store_root = env::var_os(INTERNAL_SDK_PROVIDER_STORE_ENV)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if cfg!(test) {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/incan_test_sdk_provider_store")
            } else {
                default_sdk_provider_store(
                    &stdlib_root,
                    env::var_os("INCAN_HOME"),
                    env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")),
                )
            }
        });
    let identity = sdk_provider_store_identity(
        &stdlib_root,
        &executable,
        workspace_lock.as_deref(),
        &distribution_profile,
    )?;
    let _lock = acquire_sdk_provider_store_lock(&store_root)?;
    let artifact_root = store_root.join(&identity);
    let inventory_path = artifact_root.join(SDK_INVENTORY_FILE);
    if inventory_path.is_file() {
        let inventory =
            SdkInventory::read_from_path(&inventory_path).map_err(|error| CliError::failure(error.to_string()))?;
        inventory
            .validate_compiler_compatibility(
                crate::version::INCAN_VERSION,
                crate::version::SDK_PROVIDER_CODEGEN_REVISION,
            )
            .map_err(|error| CliError::failure(error.to_string()))?;
        record_sdk_provider_root(&artifact_root)?;
        return Ok(Arc::new(inventory));
    }
    if artifact_root.exists() {
        return Err(CliError::failure(format!(
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
    ) {
        Ok(inventory) => inventory,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging_root);
            return Err(error);
        }
    };
    sync_sdk_provider_tree(&staging_root)?;
    fs::rename(&staging_root, &artifact_root).map_err(|error| {
        CliError::failure(format!(
            "failed to publish compiled SDK components from {} to {}: {error}",
            staging_root.display(),
            artifact_root.display()
        ))
    })?;
    sync_sdk_provider_store(&store_root)?;
    let published_inventory_path = artifact_root.join(SDK_INVENTORY_FILE);
    let published = SdkInventory::read_from_path(&published_inventory_path).map_err(|error| {
        CliError::failure(format!(
            "failed to load published SDK component inventory for {}: {error}",
            staged_inventory.identity()
        ))
    })?;
    record_sdk_provider_root(&artifact_root)?;
    Ok(Arc::new(published))
}

/// Report the exact immutable provider root to release packaging when requested.
fn record_sdk_provider_root(artifact_root: &Path) -> CliResult<()> {
    let Some(path_file) = env::var_os(INTERNAL_SDK_PROVIDER_PATH_FILE_ENV).filter(|path| !path.is_empty()) else {
        return Ok(());
    };
    fs::write(&path_file, format!("{}\n", artifact_root.display())).map_err(|error| {
        CliError::failure(format!(
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
) -> CliResult<SdkInventory> {
    fs::create_dir_all(staging_root).map_err(|error| {
        CliError::failure(format!(
            "failed to create SDK component staging directory {}: {error}",
            staging_root.display()
        ))
    })?;
    if let Some(workspace_lock) = workspace_lock {
        fs::copy(workspace_lock, staging_root.join("Cargo.lock")).map_err(|error| {
            CliError::failure(format!(
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
        seed_sdk_provider_workspace_lock(workspace_lock, &output_root)?;
        let manifest = ProjectManifest::discover(&component.project_root)
            .map_err(|error| CliError::failure(error.to_string()))?
            .ok_or_else(|| {
                CliError::failure(format!(
                    "SDK component `{}` has no incan.toml at {}",
                    component.id,
                    component.project_root.display()
                ))
            })?;
        let provider_name = manifest
            .project
            .as_ref()
            .and_then(|project| project.name.clone())
            .ok_or_else(|| CliError::failure(format!("SDK component `{}` has no project name", component.id)))?;
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
        );
        if built_any {
            inventory
                .write_to_path(&inventory_path)
                .map_err(|error| CliError::failure(error.to_string()))?;
            command.env(SDK_INVENTORY_OVERRIDE_ENV, &inventory_path);
        } else {
            command.env_remove(SDK_INVENTORY_OVERRIDE_ENV);
        }
        if let Some(workspace_lock) = workspace_lock {
            command.env(INTERNAL_CARGO_LOCK_PAYLOAD_PATH_ENV, workspace_lock);
        }
        let output = command.output().map_err(|error| {
            CliError::failure(format!(
                "failed to run SDK component build for `{}` at {}: {error}",
                component.id,
                component.project_root.display()
            ))
        })?;
        if !output.status.success() {
            return Err(nested_sdk_component_build_error(
                component.id.as_str(),
                &component.project_root,
                &output,
            ));
        }
        let manifest_path = output_root.join(format!("{provider_name}.incnlib"));
        let provider_manifest = LibraryManifest::read_from_path(&manifest_path).map_err(|error| {
            CliError::failure(format!(
                "failed to read SDK component `{}` manifest {}: {error}",
                component.id,
                manifest_path.display()
            ))
        })?;
        let component_lock = output_root.join("Cargo.lock");
        if component_lock.is_file() {
            fs::remove_file(&component_lock).map_err(|error| {
                CliError::failure(format!(
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
            CliError::failure(format!(
                "failed to hash SDK component `{}` artifact {}: {error}",
                component.id,
                output_root.display()
            ))
        })?;
        let inventory_component = inventory.components.get_mut(&component.id).ok_or_else(|| {
            CliError::failure(format!(
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
            CliError::failure(format!(
                "failed to remove transient SDK provider Cargo target {}: {error}",
                cargo_target_dir.display()
            ))
        })?;
    }
    restrict_staged_sdk_profile(catalog, distribution_profile, staging_root, &mut inventory)?;
    inventory
        .write_to_path(&inventory_path)
        .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(inventory)
}

/// Preserve a caller-owned Cargo target while keeping the ordinary provider-publication fallback transaction-local.
fn configure_sdk_provider_build_environment(
    command: &mut Command,
    component_id: &str,
    transaction_cargo_target: &Path,
    caller_cargo_target: Option<&std::ffi::OsStr>,
) {
    let cargo_target_dir = caller_cargo_target.map(Path::new).unwrap_or(transaction_cargo_target);
    command
        .env_remove(INTERNAL_MANIFEST_OVERRIDE_ENV)
        .env_remove(INTERNAL_PROJECT_ROOT_OVERRIDE_ENV)
        .env(SDK_PROVIDER_BUILD_ENV, component_id)
        .env(INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV, "1")
        // Share transient Cargo artifacts across components, then remove them before immutable provider publication.
        .env(GENERATED_CARGO_TARGET_DIR_ENV, cargo_target_dir);
}

/// Validate producer claims against the namespace grant before publishing them into the SDK inventory.
fn sdk_component_namespace_claims(
    component_id: &str,
    namespace_roots: &BTreeSet<String>,
    claims: &[ProviderModuleClaim],
) -> CliResult<BTreeSet<Vec<String>>> {
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
        return Err(CliError::failure(format!(
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
) -> CliResult<()> {
    if !catalog.profiles.contains_key(distribution_profile) {
        return Err(CliError::failure(format!(
            "unknown SDK distribution profile `{distribution_profile}`"
        )));
    }
    let resolved = inventory
        .resolve_catalog(&SdkComponentSelection {
            profile: distribution_profile.to_string(),
            components: BTreeSet::new(),
            exclude_components: BTreeSet::new(),
        })
        .map_err(|error| CliError::failure(error.to_string()))?;
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
                CliError::failure(format!(
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
        provider_codegen_revision: crate::version::SDK_PROVIDER_CODEGEN_REVISION,
        components,
        profiles: catalog.profiles.clone(),
    }
}

/// Preserve nested compiler stdout and stderr when one component publication fails.
fn nested_sdk_component_build_error(component: &str, project_root: &Path, output: &std::process::Output) -> CliError {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let diagnostics = [stderr.trim(), stdout.trim()]
        .into_iter()
        .filter(|message| !message.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    CliError::failure(format!(
        "failed to prepare SDK component `{component}` at {}{}",
        project_root.display(),
        if diagnostics.is_empty() {
            String::new()
        } else {
            format!("\n{diagnostics}")
        }
    ))
}

/// Cargo execution policy resolved from CLI inputs and environment defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CargoPolicy {
    pub(crate) offline: bool,
    pub(crate) locked: bool,
    pub(crate) frozen: bool,
    pub(crate) extra_args: Vec<String>,
}

/// CLI policy flags, including explicit disables for environment defaults.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CargoPolicyCliFlags {
    pub offline: bool,
    pub no_offline: bool,
    pub locked: bool,
    pub no_locked: bool,
    pub frozen: bool,
    pub no_frozen: bool,
}

impl CargoPolicy {
    /// Resolve policy for a user-facing build/run/test command.
    pub(crate) fn from_cli_and_env(
        cli_flags: CargoPolicyCliFlags,
        cli_cargo_args: Vec<String>,
        cli_passthrough_args: Vec<String>,
    ) -> Self {
        Self::from_sources(cli_flags, cli_cargo_args, cli_passthrough_args, |name| {
            env::var(name).ok()
        })
    }

    /// Build an explicit policy for internal Cargo invocations that should not read RFC 020 env defaults.
    pub(crate) fn explicit(offline: bool, locked: bool, frozen: bool, extra_args: Vec<String>) -> Self {
        let mut policy = Self {
            offline,
            locked,
            frozen,
            extra_args,
        };
        policy.normalize();
        policy
    }

    /// Resolve policy from injected sources; used by tests to avoid mutating process env.
    fn from_sources<F>(
        cli_flags: CargoPolicyCliFlags,
        mut cli_cargo_args: Vec<String>,
        cli_passthrough_args: Vec<String>,
        env_value: F,
    ) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let env_frozen = env_flag_value(env_value("INCAN_FROZEN").as_deref());
        let env_offline = env_flag_value(env_value("INCAN_OFFLINE").as_deref());
        let env_locked = env_flag_value(env_value("INCAN_LOCKED").as_deref());

        cli_cargo_args.extend(cli_passthrough_args);
        let extra_args = if cli_cargo_args.is_empty() {
            split_env_cargo_args(env_value("INCAN_CARGO_ARGS").as_deref())
        } else {
            cli_cargo_args
        };

        Self::explicit(
            resolve_cli_env_flag(env_offline, cli_flags.offline, cli_flags.no_offline),
            resolve_cli_env_flag(env_locked, cli_flags.locked, cli_flags.no_locked),
            resolve_cli_env_flag(env_frozen, cli_flags.frozen, cli_flags.no_frozen),
            extra_args,
        )
    }

    /// Apply derived policy semantics after raw source resolution.
    fn normalize(&mut self) {
        if self.frozen {
            self.offline = true;
            self.locked = true;
        }
    }
}

/// Enforce the project-level `requires-incan` constraint for a project-aware command.
pub(crate) fn enforce_project_toolchain_constraint(manifest: &ProjectManifest) -> CliResult<()> {
    enforce_toolchain_constraints(&ToolchainConstraintSet::from_project_manifest(manifest))
}

/// Enforce an already-resolved effective `requires-incan` constraint set.
pub(crate) fn enforce_toolchain_constraints(constraints: &ToolchainConstraintSet) -> CliResult<()> {
    constraints
        .enforce_current()
        .map_err(|error| CliError::failure(error.to_string()))
}

/// Resolve one boolean policy input with CLI enable/disable flags over env defaults.
fn resolve_cli_env_flag(env_default: bool, cli_enable: bool, cli_disable: bool) -> bool {
    if cli_enable {
        true
    } else if cli_disable {
        false
    } else {
        env_default
    }
}

/// Parse a boolean RFC 020 environment flag value.
fn env_flag_value(value: Option<&str>) -> bool {
    value.is_some_and(|value| matches!(value, "1" | "true" | "TRUE" | "on" | "ON"))
}

/// Split `INCAN_CARGO_ARGS` using the RFC 020 whitespace-only rule.
fn split_env_cargo_args(value: Option<&str>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(str::split_whitespace)
        .map(str::to_string)
        .collect()
}

/// Discover the active component-aware SDK relative to the selected toolchain or an explicit override.
fn discover_active_sdk_inventory() -> CliResult<Option<Arc<SdkInventory>>> {
    let explicit = env::var_os(SDK_INVENTORY_OVERRIDE_ENV)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let inventory_path = if let Some(path) = explicit.as_ref() {
        if !path.is_file() {
            return Err(CliError::failure(format!(
                "{SDK_INVENTORY_OVERRIDE_ENV} points to missing SDK inventory {}",
                path.display()
            )));
        }
        Some(path.clone())
    } else {
        crate::toolchain_layout::current_executable_search_bases()
            .into_iter()
            .flat_map(|base| {
                [
                    base.join(SDK_INVENTORY_FILE),
                    base.join("share").join("incan").join(SDK_INVENTORY_FILE),
                    base.join("share").join("incan").join("sdk").join(SDK_INVENTORY_FILE),
                ]
            })
            .find(|path| path.is_file())
    };
    let Some(path) = inventory_path else {
        return Ok(None);
    };
    let inventory = SdkInventory::read_from_path(&path).map_err(|error| CliError::failure(error.to_string()))?;
    inventory
        .validate_compiler_compatibility(
            crate::version::INCAN_VERSION,
            crate::version::SDK_PROVIDER_CODEGEN_REVISION,
        )
        .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(Some(Arc::new(inventory)))
}

/// Discover an installed SDK inventory or publish the source checkout's component providers on demand.
pub(crate) fn prepare_or_discover_sdk_inventory() -> CliResult<Option<Arc<SdkInventory>>> {
    if let Some(inventory) = discover_active_sdk_inventory()? {
        return Ok(Some(inventory));
    }
    if env::var_os(SDK_PROVIDER_BUILD_ENV).is_some() {
        return Ok(None);
    }
    let has_source_catalog =
        crate::cli::prelude::find_stdlib_dir().is_some_and(|root| root.join(SDK_SOURCE_CATALOG_FILE).is_file());
    if has_source_catalog {
        prepare_sdk_provider_inventory().map(Some)
    } else {
        Ok(None)
    }
}

/// Reject explicit component-aware selection when the active toolchain exposes only the legacy monolithic SDK.
fn validate_component_inventory_selection(
    manifest: Option<&ProjectManifest>,
    sdk_profile_override: Option<&str>,
    inventory: Option<&SdkInventory>,
) -> CliResult<()> {
    let explicit_selection = manifest.and_then(ProjectManifest::sdk).is_some() || sdk_profile_override.is_some();
    if inventory.is_some() || !explicit_selection {
        return Ok(());
    }
    let message = concat!(
        "the active Incan SDK has no component inventory, so explicit `[sdk]` selection is unavailable; ",
        "use a component-aware v0.5 SDK or remove the explicit SDK selection",
    );
    let message = manifest
        .and_then(|manifest| sdk_manifest_value_location(manifest, &[]))
        .map_or_else(|| message.to_string(), |location| format!("{location}: {message}"));
    Err(CliError::failure(message))
}

/// Resolve one SDK component selection and retain the manifest or command provenance of configuration failures.
pub(crate) fn resolve_sdk_component_selection(
    inventory: &SdkInventory,
    selection: &SdkComponentSelection,
    manifest: Option<&ProjectManifest>,
    sdk_profile_override: Option<&str>,
    require_available: bool,
) -> CliResult<ResolvedSdkComponents> {
    let result = if require_available {
        inventory.resolve(selection)
    } else {
        inventory.resolve_catalog(selection)
    };
    result.map_err(|error| {
        CliError::failure(format_sdk_selection_error(
            &error,
            selection,
            manifest,
            sdk_profile_override,
        ))
    })
}

/// Attach exact `[sdk]` source provenance to one component-resolution failure when it came from the manifest.
fn format_sdk_selection_error(
    error: &SdkResolutionError,
    selection: &SdkComponentSelection,
    manifest: Option<&ProjectManifest>,
    sdk_profile_override: Option<&str>,
) -> String {
    if matches!(error, SdkResolutionError::UnknownProfile { profile, .. } if sdk_profile_override == Some(profile)) {
        return format!("{error} (selected by the current command's `--sdk-profile` override)");
    }
    let candidates = match error {
        SdkResolutionError::UnknownProfile { profile, .. } => vec![profile.clone()],
        SdkResolutionError::UnknownComponent { component, .. }
        | SdkResolutionError::MandatoryComponentExcluded { component, .. }
        | SdkResolutionError::SelectedComponentExcluded { component }
        | SdkResolutionError::ExcludedRequiredComponent { component, .. }
        | SdkResolutionError::EnabledComponentUnavailable { component, .. } => {
            vec![component.clone(), selection.profile.clone()]
        }
    };
    manifest
        .and_then(|manifest| sdk_manifest_value_location(manifest, &candidates))
        .map_or_else(|| error.to_string(), |location| format!("{location}: {error}"))
}

/// Locate an authored SDK selection value, falling back to the `[sdk]` table header for derived failures.
fn sdk_manifest_value_location(manifest: &ProjectManifest, candidates: &[String]) -> Option<String> {
    let content = fs::read_to_string(manifest.path()).ok()?;
    let mut in_sdk = false;
    let mut sdk_header = None;
    let mut sdk_lines = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_sdk = trimmed == "[sdk]";
            if in_sdk {
                sdk_header = Some(format!("{}:{}:1", manifest.path().display(), line_index + 1));
            }
            continue;
        }
        if !in_sdk {
            continue;
        }
        sdk_lines.push((line_index, line));
    }
    for candidate in candidates {
        for (line_index, line) in &sdk_lines {
            let quoted = format!("\"{candidate}\"");
            if let Some(column) = line.find(&quoted) {
                return Some(format!(
                    "{}:{}:{}",
                    manifest.path().display(),
                    line_index + 1,
                    column + 1
                ));
            }
        }
    }
    sdk_header
}

/// Add linked generated crates selected by active compiled providers to the current backend requirements.
pub(crate) fn extend_requirements_with_provider_plan(
    requirements: &mut ProjectRequirements,
    provider_plan: &ProviderPlan,
) -> CliResult<()> {
    let sdk_providers = provider_plan
        .sdk_link_roots()
        .into_iter()
        .map(|provider| provider.identity.stable_key())
        .collect::<BTreeSet<_>>();
    extend_requirements_with_selected_sdk_providers(requirements, provider_plan, &sdk_providers)
}

/// Add ordinary providers and the selected direct SDK provider set to backend requirements.
fn extend_requirements_with_selected_sdk_providers(
    requirements: &mut ProjectRequirements,
    provider_plan: &ProviderPlan,
    sdk_providers: &BTreeSet<String>,
) -> CliResult<()> {
    // Projection helpers do not retain the ProviderPlan. Preserve the complete active SDK path catalog separately
    // from the minimal set of providers linked directly into this generated crate: a copied compiled artifact can
    // still carry a non-descriptor Cargo edge to an active provider supplied transitively or unused by this consumer.
    for provider in provider_plan.active_sdk_records() {
        if let Some(artifact) = provider.artifact.as_ref() {
            merge_requirement_dependency(
                &mut requirements.sdk_path_dependencies,
                artifact.to_dependency_spec(),
                format!("active SDK provider `{}`", provider.identity.name),
            )?;
        }
        for requirement in provider_plan.selected_backend_requirements(provider) {
            let BackendImplementationRequirement::CargoDependency { dependency } = requirement else {
                continue;
            };
            if matches!(dependency.source, ProviderCargoDependencySource::Toolchain { .. }) {
                merge_requirement_dependency(
                    &mut requirements.sdk_path_dependencies,
                    provider_cargo_dependency_spec(&dependency),
                    format!("active SDK provider `{}` toolchain dependency", provider.identity.name),
                )?;
            }
        }
    }
    for provider in provider_plan.active_records() {
        if matches!(provider.authority, crate::provider::NamespaceAuthority::SdkReserved)
            && !sdk_providers.contains(&provider.identity.stable_key())
        {
            continue;
        }
        let Some(artifact) = provider.artifact.as_ref() else {
            continue;
        };
        let mut provider_dependency = artifact.to_dependency_spec();
        if matches!(provider.authority, crate::provider::NamespaceAuthority::SdkReserved) {
            // Checked private SDK edges freeze an exact feature projection and never inherit the provider crate's
            // conventional Cargo defaults. Emit that contract explicitly so future artifacts need no legacy repair.
            provider_dependency.default_features = false;
        }
        merge_requirement_dependency(
            &mut requirements.dependencies,
            provider_dependency,
            format!("compiled provider `{}`", provider.identity.name),
        )?;
        for requirement in provider_plan.selected_backend_requirements(provider) {
            if let BackendImplementationRequirement::CargoDependency { dependency } = requirement {
                let dependency_spec = provider_cargo_dependency_spec(&dependency);
                if matches!(dependency.source, ProviderCargoDependencySource::Toolchain { .. }) {
                    merge_requirement_dependency(
                        &mut requirements.sdk_path_dependencies,
                        dependency_spec.clone(),
                        format!("compiled provider `{}` toolchain dependency", provider.identity.name),
                    )?;
                }
                merge_requirement_dependency(
                    &mut requirements.dependencies,
                    dependency_spec,
                    format!("compiled provider `{}` implementation facet", provider.identity.name),
                )?;
            }
        }
        for requirement in provider_plan.selected_backend_requirements(provider) {
            let BackendImplementationRequirement::CargoFeature { crate_name, feature } = requirement else {
                continue;
            };
            if crate_name == INCAN_STDLIB_CRATE_NAME {
                requirements.stdlib_features.push(feature);
                continue;
            }
            let Some(dependency) = requirements
                .dependencies
                .iter_mut()
                .find(|dependency| dependency.crate_name == crate_name)
            else {
                return Err(CliError::failure(format!(
                    "provider `{}` implementation facet selects Cargo feature `{crate_name}/{feature}` without declaring that dependency",
                    provider.identity.name
                )));
            };
            dependency.features.push(feature);
            dependency.features.sort();
            dependency.features.dedup();
        }
    }
    requirements
        .sdk_dependency_rebindings
        .extend_from_slice(provider_plan.sdk_dependency_rebindings());
    normalize_sdk_dependency_rebindings(&mut requirements.sdk_dependency_rebindings);
    requirements
        .sdk_artifact_projections
        .extend_from_slice(provider_plan.sdk_artifact_projections());
    normalize_sdk_artifact_projections(&mut requirements.sdk_artifact_projections);
    requirements.stdlib_features.sort();
    requirements.stdlib_features.dedup();
    Ok(())
}

/// Sort and de-duplicate physical SDK projections independently of module-group traversal order.
fn normalize_sdk_dependency_rebindings(rebindings: &mut Vec<SdkDependencyRebinding>) {
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
}

/// Sort and de-duplicate projected artifacts by their immutable compiled crate root.
fn normalize_sdk_artifact_projections(projections: &mut Vec<SdkArtifactProjection>) {
    projections.sort_by(|left, right| left.artifact.crate_root.cmp(&right.artifact.crate_root));
    projections.dedup_by(|left, right| left.artifact.crate_root == right.artifact.crate_root);
}

/// Translate relocatable provider-owned Cargo metadata at the final Rust-backend boundary.
fn provider_cargo_dependency_spec(dependency: &ProviderCargoDependency) -> DependencySpec {
    let source = match &dependency.source {
        ProviderCargoDependencySource::Registry => DependencySource::Registry,
        ProviderCargoDependencySource::Toolchain { relative_path } => DependencySource::Path {
            path: crate::toolchain_layout::resolve_toolchain_relative_path(Path::new(relative_path)),
        },
    };
    DependencySpec {
        crate_name: dependency.crate_name.clone(),
        version: dependency.version.clone(),
        features: dependency.features.iter().cloned().collect(),
        default_features: dependency.default_features,
        source,
        optional: false,
        package: dependency.package.clone(),
    }
    .normalized()
}

/// Collect canonical provider module use from resolved source modules and authored import edges.
pub(crate) fn provider_used_module_paths(modules: &[ParsedModule]) -> BTreeSet<Vec<String>> {
    let mut used = BTreeSet::new();
    if !modules.is_empty() && env::var_os(SDK_PROVIDER_BUILD_ENV).is_none() {
        // Every ordinary compilation consumes the implicit language prelude. Recording that compiler requirement
        // keeps the mandatory core provider linked even when generated support such as iterator adapters is the only
        // emitted path into `std.derives.*`.
        used.insert(vec![stdlib::STDLIB_ROOT.to_string(), "prelude".to_string()]);
    }
    for module in modules {
        if module.path_segments.first().map(String::as_str) == Some(stdlib::INCAN_STD_NAMESPACE) {
            let mut canonical = vec![stdlib::STDLIB_ROOT.to_string()];
            canonical.extend(module.path_segments.iter().skip(1).cloned());
            used.insert(canonical);
        }
        for declaration in &module.ast.declarations {
            let crate::frontend::ast::Declaration::Import(import) = &declaration.node else {
                continue;
            };
            let path = match &import.kind {
                ImportKind::Module(path) | ImportKind::From { module: path, .. }
                    if path.parent_levels == 0
                        && !path.is_absolute
                        && path.segments.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) =>
                {
                    Some(path.segments.clone())
                }
                _ => None,
            };
            used.extend(path);
        }
    }
    used
}

/// Resolve the reserved namespace roots granted to the SDK component currently being compiled from source.
pub(crate) fn sdk_provider_bootstrap_namespace_roots(project_root: &Path) -> CliResult<BTreeSet<String>> {
    let Some(component_marker) = env::var_os(SDK_PROVIDER_BUILD_ENV).filter(|value| !value.is_empty()) else {
        return Ok(BTreeSet::new());
    };
    let stdlib_root = crate::cli::prelude::find_stdlib_dir()
        .ok_or_else(|| CliError::failure("cannot locate the SDK source catalog while compiling an SDK provider"))?;
    let catalog = SdkSourceCatalog::read_from_path(&stdlib_root.join(SDK_SOURCE_CATALOG_FILE))
        .map_err(|error| CliError::failure(error.to_string()))?;
    let component_marker = component_marker.to_string_lossy();
    let canonical_project_root = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let component = catalog.components.get(component_marker.as_ref()).or_else(|| {
        catalog.components.values().find(|component| {
            fs::canonicalize(&component.project_root).unwrap_or_else(|_| component.project_root.clone())
                == canonical_project_root
        })
    });
    let component = component.ok_or_else(|| {
        CliError::failure(format!(
            "SDK provider bootstrap marker `{component_marker}` does not match a component in {}",
            stdlib_root.join(SDK_SOURCE_CATALOG_FILE).display()
        ))
    })?;
    Ok(component.namespace_roots.clone())
}

/// Discover a project manifest and materialize explicit RFC 077 inheritance before compiler stages consume it.
///
/// This is the single-project compatibility boundary: projects outside a workspace retain their parsed manifest, while
/// a member receives the graph-owned effective manifest. A dangling `{ workspace = true }` request is always an error
/// instead of being silently treated as an absent local dependency.
pub(crate) fn discover_effective_project_manifest(start_dir: &Path) -> CliResult<Option<ProjectManifest>> {
    let Some(manifest) = ProjectManifest::discover(start_dir).map_err(|error| CliError::failure(error.to_string()))?
    else {
        return Ok(None);
    };
    let workspace =
        WorkspaceGraph::discover(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?;
    let Some(workspace) = workspace else {
        if manifest.has_workspace_inherited_dependencies() {
            return Err(CliError::failure(format!(
                "{} declares {{ workspace = true }} dependencies but is not a member of an active workspace",
                manifest.path().display()
            )));
        }
        return Ok(Some(manifest));
    };
    let canonical_root = std::fs::canonicalize(manifest.project_root()).map_err(|error| {
        CliError::failure(format!(
            "failed to canonicalize project root {}: {error}",
            manifest.project_root().display()
        ))
    })?;
    let member = workspace.member_for_root(&canonical_root).ok_or_else(|| {
        CliError::failure(format!(
            "project {} is not a member of the active workspace at {}",
            manifest.path().display(),
            workspace.root().display()
        ))
    })?;
    workspace
        .effective_member_manifest(member)
        .map(Some)
        .map_err(|error| CliError::failure(error.to_string()))
}

/// Checked products produced by one [`CompilationSession`] analysis pass.
///
/// The current Rust-source backend still lowers from [`TypeCheckInfo`], while compiler-facing consumers use the
/// portable snapshot. Keeping both products in one analysis result prevents a CLI command from independently checking
/// the same sources and then treating its second result as authoritative.
///
/// The `TypeCheckInfo` half is a transition bridge. Remove it when Body IR owns every lowering query tracked by #225.
#[derive(Debug, Clone)]
pub(crate) struct CompilationAnalysis {
    type_info_by_path: BTreeMap<PathBuf, TypeCheckInfo>,
    type_info_by_module_path: BTreeMap<Vec<String>, TypeCheckInfo>,
    semantic_snapshots_by_path: BTreeMap<PathBuf, incan_semantics_core::SemanticModuleSnapshot>,
    stdlib_cache: StdlibAstCache,
}

impl CompilationAnalysis {
    /// Return the lowering input for one collected source file.
    pub(crate) fn type_info_for_path(&self, path: &Path) -> Option<&TypeCheckInfo> {
        self.type_info_by_path.get(path)
    }

    /// Return the lowering input for one compiler module identity.
    ///
    /// Identity is distinct from a source file path because the test runner may create multiple compiler modules rooted
    /// at the same file.
    pub(crate) fn type_info_for_module_path(&self, path: &[String]) -> Option<&TypeCheckInfo> {
        self.type_info_by_module_path.get(path)
    }

    /// Return portable HIR and semantic fact snapshots keyed by source path.
    pub(crate) fn semantic_snapshots(&self) -> &BTreeMap<PathBuf, incan_semantics_core::SemanticModuleSnapshot> {
        &self.semantic_snapshots_by_path
    }

    /// Return the source-backed stdlib metadata accumulated by this analysis.
    ///
    /// Lowering currently queries this cache for source-defined trait and type metadata. It stays part of the session
    /// result until those queries move to portable semantic facts and Body IR (#225).
    pub(crate) fn stdlib_cache(&self) -> &StdlibAstCache {
        &self.stdlib_cache
    }
}

/// Shared source-analysis context for CLI commands and the LSP.
///
/// This owns the project-level inputs that affect context-sensitive parsing and typechecking so entrypoints do not
/// independently rediscover manifests, library vocabulary, provider surfaces, or checked contract metadata.
#[derive(Debug, Clone)]
pub(crate) struct CompilationSession {
    pub manifest: Option<ProjectManifest>,
    pub source_root: PathBuf,
    pub library_manifest_index: LibraryManifestIndex,
    /// Immutable provider projection shared by compiler stages for ordinary package dependencies and SDK providers.
    pub provider_plan: Arc<ProviderPlan>,
    /// Integrity-checked active SDK catalog, when this toolchain is component-aware.
    pub sdk_inventory: Option<Arc<SdkInventory>>,
    /// Project-selected SDK component closure, when an inventory is active.
    pub sdk_components: Option<ResolvedSdkComponents>,
    /// Typed additive feature closure for the root package and active path dependencies.
    pub package_feature_plan: Option<PackageFeaturePlan>,
    /// Active root-package features used to project compilation-unit `when feature(...)` declarations.
    pub active_features: BTreeSet<String>,
    /// Declared root-package features used for source diagnostics before projection.
    pub declared_features: BTreeSet<String>,
    pub library_imported_vocab: parser::ImportedLibraryVocab,
    pub library_imported_dsl_surfaces: parser::ImportedLibraryDslSurfaces,
    pub contract_model_bundles: Vec<CanonicalModelBundle>,
}

impl CompilationSession {
    /// Discover project-level compilation context for an explicit Incan package-feature selection.
    pub(crate) fn discover_with_feature_selection(
        entry_path: &Path,
        feature_selection: &FeatureSelection,
    ) -> CliResult<Self> {
        Self::discover_with_selections(entry_path, feature_selection, None)
    }

    /// Discover project context for explicit package-feature and transient SDK-profile selections.
    pub(crate) fn discover_with_selections(
        entry_path: &Path,
        feature_selection: &FeatureSelection,
        sdk_profile_override: Option<&str>,
    ) -> CliResult<Self> {
        Self::discover_with_dependency_mode(
            entry_path,
            DependencyManifestMode::FullArtifacts,
            feature_selection,
            sdk_profile_override,
        )
    }

    /// Discover project-level parsing context without preparing full dependency artifacts.
    pub(crate) fn discover_for_collection(entry_path: &Path) -> CliResult<Self> {
        Self::discover_for_collection_with_feature_selection(entry_path, &FeatureSelection::default())
    }

    /// Discover parser-only project context for an explicit Incan package-feature selection.
    pub(crate) fn discover_for_collection_with_feature_selection(
        entry_path: &Path,
        feature_selection: &FeatureSelection,
    ) -> CliResult<Self> {
        Self::discover_for_collection_with_selections(entry_path, feature_selection, None)
    }

    /// Discover parser-only project context for explicit package-feature and transient SDK-profile selections.
    pub(crate) fn discover_for_collection_with_selections(
        entry_path: &Path,
        feature_selection: &FeatureSelection,
        sdk_profile_override: Option<&str>,
    ) -> CliResult<Self> {
        Self::discover_with_dependency_mode(
            entry_path,
            DependencyManifestMode::ParserOnly,
            feature_selection,
            sdk_profile_override,
        )
    }

    /// Discover project context with either full dependency artifacts or parser-only dependency metadata.
    fn discover_with_dependency_mode(
        entry_path: &Path,
        dependency_mode: DependencyManifestMode,
        feature_selection: &FeatureSelection,
        sdk_profile_override: Option<&str>,
    ) -> CliResult<Self> {
        let inferred_project_root = resolve_project_root(entry_path);
        let manifest = discover_effective_project_manifest(&inferred_project_root)?;
        let project_root = manifest
            .as_ref()
            .map(|manifest| manifest.project_root().to_path_buf())
            .unwrap_or(inferred_project_root);
        let source_root = resolve_source_root(&project_root, manifest.as_ref());
        let sdk_inventory = prepare_or_discover_sdk_inventory()?;
        let package_feature_plan = manifest
            .as_ref()
            .map(|manifest| {
                PackageFeaturePlan::resolve_with_sdk_inventory(manifest, feature_selection, sdk_inventory.as_deref())
            })
            .transpose()
            .map_err(|error| CliError::failure(error.to_string()))?;
        let root_feature_state = package_feature_plan
            .as_ref()
            .and_then(|plan| plan.package(&project_root));
        let active_dependencies = root_feature_state
            .map(|state| state.active_dependencies.clone())
            .unwrap_or_default();
        let active_features = root_feature_state
            .map(|state| state.features.active_features.clone())
            .unwrap_or_default();
        let declared_features = manifest
            .as_ref()
            .map(PackageFeatureGraph::from_manifest)
            .transpose()
            .map_err(|error| CliError::failure(error.to_string()))?
            .map(|graph| graph.declared_features().map(str::to_string).collect())
            .unwrap_or_default();
        if let Some(manifest) = manifest.as_ref()
            && dependency_mode == DependencyManifestMode::FullArtifacts
        {
            prepare_library_dependency_artifacts(manifest, package_feature_plan.as_ref(), &active_dependencies)?;
        }
        let library_manifest_index = match (manifest.as_ref(), dependency_mode) {
            (Some(manifest), DependencyManifestMode::FullArtifacts) if !active_dependencies.is_empty() => {
                LibraryManifestIndex::from_project_manifest_dependencies(
                    manifest,
                    active_dependencies.iter().map(String::as_str),
                )
            }
            (Some(manifest), DependencyManifestMode::ParserOnly) if !active_dependencies.is_empty() => {
                parser_only_library_manifest_index(manifest, &active_dependencies)?
            }
            _ => LibraryManifestIndex::default(),
        };
        let library_imported_vocab = library_manifest_index.library_imported_vocab();
        let library_imported_dsl_surfaces = library_manifest_index.library_imported_dsl_surfaces();
        let contract_model_bundles = manifest
            .as_ref()
            .map(|manifest| read_project_model_bundles(&project_root, &manifest.contract_model_bundle_paths()))
            .transpose()
            .map_err(|error| CliError::failure(error.to_string()))?
            .unwrap_or_default();
        // Collection and execution must resolve the same SDK catalog. In a source checkout there is no installed
        // inventory to discover, so a parser-only collection that skipped publication silently fell back to the
        // legacy monolithic stdlib. Besides losing component-aware diagnostics, that made a transient SDK profile
        // fail during collection and allowed the lock projection to drift before execution prepared the artifacts.
        // Publication is content-addressed and reused; parser-only mode still avoids preparing ordinary dependencies.
        validate_component_inventory_selection(manifest.as_ref(), sdk_profile_override, sdk_inventory.as_deref())?;
        let sdk_selection =
            SdkComponentSelection::from_manifest_with_profile_override(manifest.as_ref(), sdk_profile_override);
        let sdk_components = sdk_inventory
            .as_ref()
            .map(|inventory| {
                resolve_sdk_component_selection(
                    inventory,
                    &sdk_selection,
                    manifest.as_ref(),
                    sdk_profile_override,
                    false,
                )
            })
            .transpose()?;
        let bootstrap_sdk_namespace_roots = sdk_provider_bootstrap_namespace_roots(&project_root)?;
        let provider_plan = Arc::new(
            ProviderPlan::from_resolved_inputs(
                library_manifest_index.clone(),
                package_feature_plan.as_ref(),
                sdk_inventory.as_deref(),
                sdk_components.as_ref(),
                std::iter::empty(),
            )
            .map_err(|error| CliError::failure(error.to_string()))?
            .with_bootstrap_sdk_namespace_roots(bootstrap_sdk_namespace_roots),
        );

        Ok(Self {
            manifest,
            source_root,
            library_manifest_index,
            provider_plan,
            sdk_inventory,
            sdk_components,
            package_feature_plan,
            active_features,
            declared_features,
            library_imported_vocab,
            library_imported_dsl_surfaces,
            contract_model_bundles,
        })
    }

    /// Resolve module participation from this session's immutable provider, feature, and SDK inputs.
    pub(crate) fn provider_plan_for_modules(&self, modules: &[ParsedModule]) -> CliResult<Arc<ProviderPlan>> {
        ProviderPlan::from_resolved_inputs(
            self.provider_plan.library_manifest_index().clone(),
            self.package_feature_plan.as_ref(),
            self.sdk_inventory.as_deref(),
            self.sdk_components.as_ref(),
            provider_used_module_paths(modules),
        )
        .map(|plan| {
            plan.with_bootstrap_sdk_namespace_roots(self.provider_plan.bootstrap_sdk_namespace_roots().cloned())
        })
        .map(Arc::new)
        .map_err(|error| CliError::failure(error.to_string()))
    }

    /// Analyze one collected module graph exactly once.
    ///
    /// This is the v0.5 session bridge: code generation receives the checked lowering inputs it currently needs, and
    /// codegraph/LSP-facing callers receive compiler-owned semantic facts from the same pass. No command may re-run
    /// typechecking merely to derive its own authority.
    pub(crate) fn analyze_modules(
        &self,
        modules: &[ParsedModule],
        #[cfg(feature = "rust_inspect")] rust_inspect_manifest_dir: Option<&Path>,
    ) -> Result<CompilationAnalysis, CliDiagnosticFailure> {
        let provider_plan = self.provider_plan_for_modules(modules).map_err(|error| {
            let module = modules.last();
            CliDiagnosticFailure::single(
                module
                    .map(|module| module.file_path.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                module.map(|module| module.source.clone()).unwrap_or_default(),
                diagnostics::CompileError::new(error.message, Span::default()),
                diagnostics::DiagnosticPhase::Import,
            )
        })?;
        let typecheck_artifacts = typecheck_modules_with_import_graph_artifacts(
            modules,
            self.manifest.as_ref(),
            &provider_plan,
            #[cfg(feature = "rust_inspect")]
            rust_inspect_manifest_dir,
        )?;
        let mut type_info_by_path = BTreeMap::new();
        let mut type_info_by_module_path = BTreeMap::new();
        let mut semantic_snapshots_by_path = BTreeMap::new();

        for (module, type_info) in modules.iter().zip(typecheck_artifacts.type_infos) {
            let snapshot = build_semantic_module_snapshot_v0(&module.ast, &module.path_segments, &type_info);
            type_info_by_path.insert(module.file_path.clone(), type_info.clone());
            type_info_by_module_path.insert(module.path_segments.clone(), type_info);
            semantic_snapshots_by_path.insert(module.file_path.clone(), snapshot);
        }

        Ok(CompilationAnalysis {
            type_info_by_path,
            type_info_by_module_path,
            semantic_snapshots_by_path,
            stdlib_cache: typecheck_artifacts.stdlib_cache,
        })
    }

    /// Resolve the test runner's marker contract from the same checked provider plan used by compilation.
    ///
    /// A source fallback remains only for legacy/development sessions without an SDK catalog. Component-aware SDKs
    /// must supply this contract through the compiled `std.testing` provider manifest.
    pub(crate) fn testing_marker_semantics(&self) -> CliResult<Option<TestingMarkerSemantics>> {
        let module = [stdlib::STDLIB_ROOT.to_string(), "testing".to_string()];
        match self.provider_plan.resolve_module(&module) {
            ProviderModuleResolution::Active(provider) => {
                let manifest = provider.manifest.as_deref().ok_or_else(|| {
                    CliError::failure(format!(
                        "active std.testing provider `{}` has no checked manifest",
                        provider.identity.name
                    ))
                })?;
                testing_marker_semantics_from_manifest(manifest).map_err(|error| CliError::failure(error.to_string()))
            }
            ProviderModuleResolution::Disabled(_) | ProviderModuleResolution::Unavailable(_) => Ok(None),
            ProviderModuleResolution::Unknown if self.provider_plan.has_sdk_catalog() => Ok(None),
            ProviderModuleResolution::Unknown => load_testing_marker_semantics()
                .map(Some)
                .map_err(|error| CliError::failure(error.to_string())),
        }
    }

    /// Require `std.testing` marker semantics and retain the component-specific remedy in the failure.
    pub(crate) fn require_testing_marker_semantics(&self) -> CliResult<TestingMarkerSemantics> {
        if let Some(semantics) = self.testing_marker_semantics()? {
            return Ok(semantics);
        }

        let module = [stdlib::STDLIB_ROOT.to_string(), "testing".to_string()];
        let message = match self.provider_plan.resolve_module(&module) {
            ProviderModuleResolution::Disabled(provider) => match &provider.provenance {
                ProviderProvenance::Sdk { component_id, .. } => format!(
                    "std.testing marker syntax requires disabled SDK component `{component_id}`; enable it under [sdk] components"
                ),
                _ => "std.testing marker syntax requires a disabled provider".to_string(),
            },
            ProviderModuleResolution::Unavailable(provider) => match &provider.provenance {
                ProviderProvenance::Sdk { component_id, .. } => format!(
                    "std.testing marker syntax requires SDK component `{component_id}`, but its artifact is not installed"
                ),
                _ => "std.testing marker syntax requires an unavailable provider artifact".to_string(),
            },
            _ => "std.testing marker syntax requires the compiled std.testing provider".to_string(),
        };
        Err(CliError::failure(message))
    }

    /// Return the Rust crate names declared by the project manifest, or an empty set outside a project.
    #[cfg(feature = "lsp")]
    pub(crate) fn declared_crate_names(&self) -> HashSet<String> {
        self.manifest
            .as_ref()
            .map(ProjectManifest::declared_rust_crate_names)
            .unwrap_or_default()
    }

    /// Lex and parse one source file using the project-aware vocabulary surfaces, without running desugarers or
    /// compile-time materialization passes.
    pub(crate) fn parse_source_for_collection(
        &self,
        file_path: &Path,
        source: &str,
    ) -> Result<Program, Vec<diagnostics::CompileError>> {
        let tokens = lexer::lex(source)?;
        let file_path_display = file_path.to_string_lossy();
        parser::parse_with_context_and_surfaces(
            &tokens,
            Some(file_path_display.as_ref()),
            Some(&self.library_imported_vocab),
            Some(&self.library_imported_dsl_surfaces),
        )
    }

    /// Lex, parse, vocab-desugar, and optionally materialize checked contract models for one source file.
    pub(crate) fn parse_source(
        &self,
        file_path: &Path,
        source: &str,
        materialize_models: bool,
    ) -> Result<Program, Vec<diagnostics::CompileError>> {
        let parsed = self.parse_source_unprojected(file_path, source, materialize_models)?;
        self.project_parsed_program(parsed)
    }

    /// Parse and desugar one source file while retaining inactive compile-time feature declarations for tooling.
    pub(crate) fn parse_source_unprojected(
        &self,
        file_path: &Path,
        source: &str,
        materialize_models: bool,
    ) -> Result<Program, Vec<diagnostics::CompileError>> {
        let parsed = self.parse_source_for_collection(file_path, source)?;
        let mut ast = parsed;
        let file_path_display = file_path.to_string_lossy();
        vocab_desugar_pass::desugar_program_vocab_blocks(
            &mut ast,
            Some(file_path_display.as_ref()),
            &self.library_manifest_index,
        )?;
        if materialize_models && let Err(error) = materialize_contract_models(&mut ast, &self.contract_model_bundles) {
            return Err(vec![diagnostics::CompileError::new(
                format!("Invalid checked contract metadata: {error}"),
                Span::default(),
            )]);
        }
        Ok(ast)
    }

    /// Validate and project one already-parsed source program through this session's active package features.
    pub(crate) fn project_parsed_program(&self, parsed: Program) -> Result<Program, Vec<diagnostics::CompileError>> {
        self.validate_parsed_program_features(&parsed)?;
        Ok(parsed.projected_for_features(&self.active_features))
    }

    /// Validate compile-time feature names while retaining the complete unprojected source program.
    pub(crate) fn validate_parsed_program_features(
        &self,
        parsed: &Program,
    ) -> Result<(), Vec<diagnostics::CompileError>> {
        let mut feature_errors = Vec::new();
        for declaration in &parsed.declarations {
            self.validate_declaration_feature_requirements(declaration, &mut feature_errors);
        }
        if !feature_errors.is_empty() {
            return Err(feature_errors);
        }
        Ok(())
    }

    /// Validate one declaration and any inline-test declarations nested inside it against the package feature graph.
    fn validate_declaration_feature_requirements(
        &self,
        declaration: &crate::frontend::ast::Spanned<crate::frontend::ast::Declaration>,
        errors: &mut Vec<diagnostics::CompileError>,
    ) {
        for feature in &declaration.required_features {
            if !self.declared_features.contains(feature) {
                errors.push(diagnostics::CompileError::new(
                    format!("Unknown package feature `{feature}` in compile-time condition"),
                    declaration.span,
                ));
            }
        }
        if let crate::frontend::ast::Declaration::TestModule(module) = &declaration.node {
            for nested in &module.body {
                self.validate_declaration_feature_requirements(nested, errors);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DependencyManifestMode {
    FullArtifacts,
    ParserOnly,
}

/// Build a parser-only dependency manifest index for formatting and other collection-only entrypoints.
///
/// This deliberately does not write `.incnlib` artifacts. A source-derived parser manifest contains vocab
/// registrations and soft-keyword activations only, because collection parsing needs syntax context but not generated
/// Rust artifacts, checked exports, Rust ABI metadata, or a packaged desugarer.
fn parser_only_library_manifest_index(
    manifest: &ProjectManifest,
    active_dependencies: &BTreeSet<String>,
) -> CliResult<LibraryManifestIndex> {
    let existing_index = LibraryManifestIndex::from_project_manifest_dependencies(
        manifest,
        active_dependencies.iter().map(String::as_str),
    );
    let mut entries = HashMap::new();

    for dependency_key in active_dependencies {
        let Some(dependency) = manifest.library_dependencies().get(dependency_key) else {
            continue;
        };
        match existing_index.get(dependency_key) {
            Some(LibraryManifestIndexEntry::Loaded { .. }) => {
                let Some(entry) = existing_index.get(dependency_key) else {
                    continue;
                };
                entries.insert(dependency_key.clone(), entry.clone());
            }
            Some(LibraryManifestIndexEntry::Failed(failure))
                if failure.kind == LibraryManifestFailureKind::ArtifactMissing
                    && dependency.path.join(MANIFEST_FILENAME).is_file() =>
            {
                entries.insert(
                    dependency_key.clone(),
                    parser_only_library_manifest_entry(dependency_key, &dependency.path)?,
                );
            }
            Some(entry) => {
                entries.insert(dependency_key.clone(), entry.clone());
            }
            None => {}
        }
    }

    Ok(LibraryManifestIndex::from_entries(entries))
}

/// Derive the parser-visible portion of one source dependency's library manifest without writing package artifacts.
fn parser_only_library_manifest_entry(
    dependency_key: &str,
    dependency_root: &Path,
) -> CliResult<LibraryManifestIndexEntry> {
    let dependency_root = fs::canonicalize(dependency_root).unwrap_or_else(|_| dependency_root.to_path_buf());
    let manifest_path = dependency_root.join(MANIFEST_FILENAME);
    let manifest_content = fs::read_to_string(&manifest_path)
        .map_err(|error| CliError::failure(format!("failed to read {}: {error}", manifest_path.display())))?;
    let dependency_manifest = ProjectManifest::from_str(&manifest_content, &manifest_path)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let project_root = dependency_manifest.project_root().to_path_buf();
    let project_name = dependency_manifest
        .project
        .as_ref()
        .and_then(|project| project.name.clone())
        .or_else(|| {
            project_root
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| dependency_key.to_string());
    let project_version = dependency_manifest
        .project
        .as_ref()
        .and_then(|project| project.version.clone())
        .unwrap_or_else(|| "0.1.0".to_string());
    let mut manifest = LibraryManifest::new(project_name.clone(), project_version);

    let generated_cargo_target_dir = env::var_os(GENERATED_CARGO_TARGET_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    if let Some(vocab_extraction) = collect_library_vocab_metadata_for_parser(
        &dependency_manifest,
        &project_root,
        generated_cargo_target_dir.as_deref(),
    )? {
        manifest.vocab = Some(vocab_extraction.payload);
        manifest.soft_keywords.activations = vocab_extraction.compatibility_activations;
    }

    let metadata =
        LibraryArtifactMetadata::for_parser_source(dependency_key.to_string(), project_name, dependency_root);
    Ok(LibraryManifestIndexEntry::Loaded {
        manifest: Box::new(manifest),
        metadata,
    })
}

/// Ensure clean check/format/test entrypoints see the same public dependency manifests as warmed worktrees.
pub(crate) fn prepare_library_dependency_artifacts(
    manifest: &ProjectManifest,
    feature_plan: Option<&PackageFeaturePlan>,
    active_dependencies: &BTreeSet<String>,
) -> CliResult<()> {
    if active_dependencies.is_empty() {
        return Ok(());
    }

    let initial_index = LibraryManifestIndex::from_project_manifest_dependencies(
        manifest,
        active_dependencies.iter().map(String::as_str),
    );
    let mut required = Vec::new();
    for dependency_key in active_dependencies {
        let Some(dependency) = manifest.library_dependencies().get(dependency_key) else {
            continue;
        };
        let expected_features = feature_plan
            .and_then(|plan| plan.package(&dependency.path))
            .map(|package| package.features.active_features.clone())
            .unwrap_or_default();
        let has_source_manifest = dependency.path.join(MANIFEST_FILENAME).is_file();
        let needs_build = match initial_index.get(dependency_key) {
            Some(LibraryManifestIndexEntry::Loaded {
                manifest: artifact_manifest,
                metadata,
            }) => {
                let actual_features = &artifact_manifest.contract_metadata.provider.active_features;
                if actual_features != &expected_features && !has_source_manifest {
                    return Err(CliError::failure(format!(
                        "compiled dependency `pub::{dependency_key}` at {} was built with package features [{}], but this consumer requires [{}]; the producer source manifest is unavailable, so install or publish an artifact with the exact requested feature projection",
                        metadata.manifest_path.display(),
                        actual_features.iter().cloned().collect::<Vec<_>>().join(", "),
                        expected_features.iter().cloned().collect::<Vec<_>>().join(", "),
                    )));
                }
                actual_features != &expected_features
            }
            Some(LibraryManifestIndexEntry::Failed(failure)) => {
                failure.kind == LibraryManifestFailureKind::ArtifactMissing
            }
            None => false,
        };
        if needs_build && has_source_manifest {
            required.push((dependency_key.clone(), dependency.path.clone(), expected_features));
        }
    }

    for (dependency_key, dependency_root, active_features) in required {
        prepare_library_dependency_artifact(&dependency_key, &dependency_root, &active_features)?;
    }

    Ok(())
}

/// Prepare one missing `pub::` dependency artifact through the existing library-mode compiler path.
fn prepare_library_dependency_artifact(
    dependency_key: &str,
    dependency_root: &Path,
    active_features: &BTreeSet<String>,
) -> CliResult<()> {
    let canonical_root = fs::canonicalize(dependency_root).unwrap_or_else(|_| dependency_root.to_path_buf());
    {
        let prepared = PREPARED_LIBRARY_DEPENDENCIES
            .lock()
            .map_err(|_| CliError::failure("failed to lock prepared library dependency set"))?;
        if prepared.get(&canonical_root) == Some(active_features) {
            return Ok(());
        }
    }

    eprintln!(
        "Preparing missing pub::{dependency_key} dependency artifact with `incan build --lib` in {}",
        dependency_root.display()
    );
    let current_exe = env::current_exe()
        .map_err(|error| CliError::failure(format!("failed to resolve current incan executable: {error}")))?;
    let mut command = Command::new(current_exe);
    command
        .args(["build", "--lib", "--no-default-features"])
        .current_dir(dependency_root)
        .env_remove(INTERNAL_MANIFEST_OVERRIDE_ENV)
        .env_remove(INTERNAL_PROJECT_ROOT_OVERRIDE_ENV)
        .env(INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV, "1");
    if !active_features.is_empty() {
        command
            .arg("--features")
            .arg(active_features.iter().cloned().collect::<Vec<_>>().join(","));
    }
    let status = command.status().map_err(|error| {
        CliError::failure(format!(
            "failed to run `incan build --lib` for pub::{dependency_key} dependency at {}: {error}",
            dependency_root.display()
        ))
    })?;

    if !status.success() {
        return Err(CliError::failure(format!(
            "failed to prepare pub::{dependency_key} dependency artifact at {}",
            dependency_root.display()
        )));
    }

    let mut prepared = PREPARED_LIBRARY_DEPENDENCIES
        .lock()
        .map_err(|_| CliError::failure("failed to lock prepared library dependency set"))?;
    prepared.insert(canonical_root, active_features.clone());
    Ok(())
}

/// Collect a unified set of project requirements from source imports and loaded provider manifests.
pub(crate) fn collect_project_requirements(
    modules: &[ParsedModule],
    library_manifest_index: &LibraryManifestIndex,
) -> CliResult<ProjectRequirements> {
    let mut stdlib_namespaces = HashSet::new();
    if env::var_os(SDK_PROVIDER_BUILD_ENV).is_some() {
        for module in modules {
            for decl in &module.ast.declarations {
                let crate::frontend::ast::Declaration::Import(import) = &decl.node else {
                    continue;
                };
                let path = match &import.kind {
                    ImportKind::From { module, .. } => {
                        if module.parent_levels > 0 || module.is_absolute {
                            continue;
                        }
                        &module.segments
                    }
                    ImportKind::Module(path) => {
                        if path.parent_levels > 0 || path.is_absolute {
                            continue;
                        }
                        &path.segments
                    }
                    _ => continue,
                };
                if path.len() >= 2 && path[0] == stdlib::STDLIB_ROOT {
                    stdlib_namespaces.insert(path[1].clone());
                }
            }
        }
    }

    // The compiler-owned legacy bare `json_stringify` builtin can still be used without a provider import. Keep its
    // runtime requirement explicit until that compatibility surface is removed.
    let needs_legacy_serde_runtime = modules.iter().any(|module| detect_serde_non_import_usage(&module.ast));
    if needs_legacy_serde_runtime {
        stdlib_namespaces.insert("serde".to_string());
    }

    let mut stdlib_features: BTreeSet<String> = BTreeSet::new();
    for namespace_name in &stdlib_namespaces {
        let Some(namespace) = stdlib::find_namespace(namespace_name) else {
            continue;
        };
        if let Some(feature) = namespace.feature {
            stdlib_features.insert(feature.to_string());
        }
    }
    for feature in library_manifest_index.merged_provider_required_stdlib_features() {
        stdlib_features.insert(feature);
    }

    let mut requirements = ProjectRequirements {
        stdlib_features: stdlib_features.into_iter().collect(),
        dependencies: Vec::new(),
        sdk_dependency_rebindings: Vec::new(),
        sdk_path_dependencies: Vec::new(),
        sdk_artifact_projections: Vec::new(),
    };
    for namespace_name in &stdlib_namespaces {
        let Some(namespace) = stdlib::find_namespace(namespace_name) else {
            continue;
        };
        for dep in namespace.extra_crate_deps {
            let spec = dependency_spec_from_stdlib_dep(dep);
            if matches!(spec.source, DependencySource::Path { .. }) {
                merge_requirement_dependency(
                    &mut requirements.sdk_path_dependencies,
                    spec.clone(),
                    format!("stdlib namespace `std.{namespace_name}` toolchain path"),
                )?;
            }
            merge_requirement_dependency(
                &mut requirements.dependencies,
                spec,
                format!("stdlib namespace `std.{namespace_name}`"),
            )?;
        }
    }

    let needs_serde_runtime = needs_legacy_serde_runtime || stdlib_namespaces.contains("serde");
    if needs_serde_runtime {
        let serde = dependency_spec_from_stdlib_extra_crate("serde")?;
        if matches!(serde.source, DependencySource::Path { .. }) {
            merge_requirement_dependency(
                &mut requirements.sdk_path_dependencies,
                serde.clone(),
                "std.serde toolchain path".to_string(),
            )?;
        }
        merge_requirement_dependency(
            &mut requirements.dependencies,
            serde,
            "std.serde usage in source".to_string(),
        )?;
    }

    for spec in library_manifest_index.cargo_path_dependencies() {
        merge_requirement_dependency(
            &mut requirements.dependencies,
            spec,
            "pub:: dependency artifact".to_string(),
        )?;
    }
    for spec in library_manifest_index
        .merged_provider_required_dependencies()
        .map_err(|err| CliError::failure(format!("failed to merge provider requirements: {err}")))?
    {
        merge_requirement_dependency(
            &mut requirements.dependencies,
            spec,
            "provider manifest requirement".to_string(),
        )?;
    }

    Ok(requirements)
}

/// Return the exact compiler-owned path catalog used only for semantic generated-artifact identity.
pub(crate) fn semantic_sdk_path_dependencies(requirements: &ProjectRequirements) -> Vec<DependencySpec> {
    let mut dependencies = requirements.sdk_path_dependencies.clone();
    for crate_name in GENERATED_TOOLCHAIN_SUPPORT_CRATES {
        if dependencies
            .iter()
            .any(|dependency| dependency.crate_name == crate_name)
        {
            continue;
        }
        dependencies.push(compiler_support_dependency_spec(crate_name));
    }
    dependencies.sort_by(|left, right| {
        (&left.crate_name, left.package.as_deref()).cmp(&(&right.crate_name, right.package.as_deref()))
    });
    dependencies
}

/// Describe one support crate emitted into every generated Cargo project as an exact compiler-owned path.
fn compiler_support_dependency_spec(crate_name: &str) -> DependencySpec {
    DependencySpec {
        crate_name: crate_name.to_string(),
        version: None,
        features: Vec::new(),
        default_features: true,
        source: DependencySource::Path {
            path: crate::toolchain_layout::resolve_toolchain_crate_path(crate_name),
        },
        optional: false,
        package: None,
    }
}

/// Build a dependency specification from a stdlib extra crate requirement.
fn dependency_spec_from_stdlib_extra_crate(crate_name: &str) -> CliResult<DependencySpec> {
    let dep = stdlib::find_extra_crate_dep(crate_name).ok_or_else(|| {
        CliError::failure(format!(
            "stdlib dependency metadata for `{crate_name}` is missing from the registry"
        ))
    })?;
    Ok(dependency_spec_from_stdlib_dep(dep))
}

/// Build a dependency specification from a stdlib dependency requirement.
fn dependency_spec_from_stdlib_dep(dep: &StdlibExtraCrateDep) -> DependencySpec {
    match dep.source {
        StdlibExtraCrateSource::Version(version) => DependencySpec {
            crate_name: dep.crate_name.to_string(),
            version: Some(version.to_string()),
            features: dep.features.iter().map(|feature| (*feature).to_string()).collect(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: stdlib::extra_crate_package_alias(dep.crate_name).map(str::to_string),
        },
        StdlibExtraCrateSource::Path(relative_path) => DependencySpec {
            crate_name: dep.crate_name.to_string(),
            version: None,
            features: dep.features.iter().map(|feature| (*feature).to_string()).collect(),
            default_features: true,
            source: DependencySource::Path {
                path: crate::toolchain_layout::resolve_toolchain_relative_path(Path::new(relative_path)),
            },
            optional: false,
            package: None,
        },
    }
    .normalized()
}

/// Merge a dependency requirement into a collection of requirements.
///
/// Existing entries with the same crate name must be compatible.
fn merge_requirement_dependency(
    merged: &mut Vec<DependencySpec>,
    candidate: DependencySpec,
    source_label: String,
) -> CliResult<()> {
    if let Some(existing) = merged.iter().find(|dep| dep.crate_name == candidate.crate_name) {
        if !dependency_specs_match(existing, &candidate) {
            return Err(CliError::failure(format!(
                "dependency requirement `{}` conflicts with existing collected requirements ({source_label})",
                candidate.crate_name
            )));
        }
        return Ok(());
    }
    merged.push(candidate);
    merged.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    Ok(())
}

/// Compare dependency specs while treating equivalent path spellings as the same dependency.
fn dependency_specs_match(left: &DependencySpec, right: &DependencySpec) -> bool {
    if left == right {
        return true;
    }
    let mut left = left.clone();
    let mut right = right.clone();
    for spec in [&mut left, &mut right] {
        if let DependencySource::Path { path } = &mut spec.source {
            *path = fs::canonicalize(&*path).unwrap_or_else(|_| path.clone());
        }
    }
    left == right
}

/// Merge collected requirement dependencies into resolved dependency sets.
///
/// Existing entries with the same crate name must be compatible.
pub(crate) fn merge_project_requirement_dependencies(
    resolved: &mut ResolvedDependencies,
    requirements: &ProjectRequirements,
) -> CliResult<()> {
    for required in &requirements.dependencies {
        let already_in_dependencies = resolved
            .dependencies
            .iter()
            .find(|spec| spec.crate_name == required.crate_name);
        if let Some(existing) = already_in_dependencies {
            if !dependency_specs_match(existing, required) {
                return Err(CliError::failure(format!(
                    "dependency `{}` conflicts between resolved imports and collected project requirements",
                    required.crate_name
                )));
            }
            continue;
        }
        let already_in_dev = resolved
            .dev_dependencies
            .iter()
            .find(|spec| spec.crate_name == required.crate_name);
        if let Some(existing) = already_in_dev {
            if existing != required {
                return Err(CliError::failure(format!(
                    "dependency `{}` conflicts between dev dependencies and collected project requirements",
                    required.crate_name
                )));
            }
            continue;
        }
        resolved.dependencies.push(required.clone());
    }
    resolved
        .dependencies
        .sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    Ok(())
}

/// Merge project-level dependency requirements into the resolved dependency set.
pub(crate) fn merge_project_requirements(
    current: &ProjectRequirements,
    extra: &ProjectRequirements,
) -> CliResult<ProjectRequirements> {
    let stdlib_features = current
        .stdlib_features
        .iter()
        .chain(extra.stdlib_features.iter())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut dependencies = current.dependencies.clone();
    for candidate in &extra.dependencies {
        if let Some(existing) = dependencies.iter().find(|dep| dep.crate_name == candidate.crate_name) {
            if existing != candidate {
                return Err(CliError::failure(format!(
                    "dependency requirement `{}` conflicts between project requirement contexts",
                    candidate.crate_name
                )));
            }
            continue;
        }
        dependencies.push(candidate.clone());
    }
    dependencies.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    let mut sdk_dependency_rebindings = current.sdk_dependency_rebindings.clone();
    sdk_dependency_rebindings.extend(extra.sdk_dependency_rebindings.iter().cloned());
    normalize_sdk_dependency_rebindings(&mut sdk_dependency_rebindings);
    let mut sdk_path_dependencies = current.sdk_path_dependencies.clone();
    for candidate in &extra.sdk_path_dependencies {
        merge_requirement_dependency(
            &mut sdk_path_dependencies,
            candidate.clone(),
            "merged SDK/toolchain path context".to_string(),
        )?;
    }
    let mut sdk_artifact_projections = current.sdk_artifact_projections.clone();
    sdk_artifact_projections.extend(extra.sdk_artifact_projections.iter().cloned());
    normalize_sdk_artifact_projections(&mut sdk_artifact_projections);

    Ok(ProjectRequirements {
        stdlib_features,
        dependencies,
        sdk_dependency_rebindings,
        sdk_path_dependencies,
        sdk_artifact_projections,
    })
}

#[cfg(feature = "rust_inspect")]
const RUST_INSPECT_WORKSPACE_FINGERPRINT_FILE: &str = ".incan_rust_inspect_fingerprint";

#[cfg(feature = "rust_inspect")]
const RUST_INSPECT_WORKSPACE_FINGERPRINT_PREFIX: &str = "v1:";

#[cfg(feature = "rust_inspect")]
const RUST_INSPECT_OUT_DIRS_FINGERPRINT_FILE: &str = ".incan_rust_inspect_out_dirs_fingerprint";

/// Counts how many times each rust-inspect stub workspace is fully regenerated instead of skipped via fingerprint.
///
/// Full lib tests run in parallel and other tests can legitimately create unrelated rust-inspect workspaces, so this
/// instrumentation is keyed by generated workspace path instead of using one process-wide counter.
#[cfg(all(test, feature = "rust_inspect"))]
static TEST_RUST_INSPECT_WORKSPACE_GENERATIONS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::BTreeMap<PathBuf, u64>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::BTreeMap::new()));

/// Records a full rust-inspect workspace regeneration for the generated workspace path under test.
#[cfg(all(test, feature = "rust_inspect"))]
fn record_test_rust_inspect_workspace_generation(workspace_dir: &Path) {
    let mut counts = TEST_RUST_INSPECT_WORKSPACE_GENERATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *counts.entry(workspace_dir.to_path_buf()).or_default() += 1;
}

/// Returns the number of full rust-inspect workspace regenerations recorded for a generated workspace path.
#[cfg(all(test, feature = "rust_inspect"))]
fn test_rust_inspect_workspace_generations(workspace_dir: &Path) -> u64 {
    let counts = TEST_RUST_INSPECT_WORKSPACE_GENERATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    counts.get(workspace_dir).copied().unwrap_or(0)
}

#[cfg(feature = "rust_inspect")]
fn normalized_stdlib_features_for_rust_inspect_fingerprint(features: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = features
        .iter()
        .map(|feature| feature.trim().to_string())
        .filter(|feature| !feature.is_empty())
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

#[cfg(feature = "rust_inspect")]
fn hash_dependency_spec_for_rust_inspect(hasher: &mut Sha256, spec: &DependencySpec) {
    use crate::manifest::GitReference;

    hasher.update(spec.crate_name.as_bytes());
    hasher.update(b"\0");
    match &spec.version {
        Some(v) => {
            hasher.update(b"ver\0");
            hasher.update(v.as_bytes());
            hasher.update(b"\0");
        }
        None => hasher.update(b"nover\0"),
    }
    let mut feats = spec.features.clone();
    feats.sort();
    for f in feats {
        hasher.update(f.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update([if spec.default_features { 1 } else { 0 }]);
    hasher.update([if spec.optional { 1 } else { 0 }]);
    match &spec.package {
        Some(p) => {
            hasher.update(b"pkg\0");
            hasher.update(p.as_bytes());
            hasher.update(b"\0");
        }
        None => hasher.update(b"nopkg\0"),
    }
    match &spec.source {
        DependencySource::Registry => hasher.update(b"src_reg\0"),
        DependencySource::Git { url, reference } => {
            hasher.update(b"src_git\0");
            hasher.update(url.as_bytes());
            hasher.update(b"\0");
            match reference {
                GitReference::Branch(s) => {
                    hasher.update(b"git_br\0");
                    hasher.update(s.as_bytes());
                    hasher.update(b"\0");
                }
                GitReference::Tag(s) => {
                    hasher.update(b"git_tag\0");
                    hasher.update(s.as_bytes());
                    hasher.update(b"\0");
                }
                GitReference::Rev(s) => {
                    hasher.update(b"git_rev\0");
                    hasher.update(s.as_bytes());
                    hasher.update(b"\0");
                }
            }
        }
        DependencySource::Path { path } => {
            hasher.update(b"src_path\0");
            hasher.update(path.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
        }
    }
    hasher.update(b"|dep|\0");
}

/// Stable fingerprint for inputs that define one generated rust-inspect Cargo workspace.
#[cfg(feature = "rust_inspect")]
#[allow(clippy::too_many_arguments)]
fn rust_inspect_workspace_fingerprint(
    project_name: &str,
    cargo_package_name: &str,
    rust_edition: Option<&str>,
    resolved: &ResolvedDependencies,
    stdlib_features: &[String],
    sdk_dependency_rebindings: &[SdkDependencyRebinding],
    sdk_path_dependencies: &[DependencySpec],
    sdk_artifact_projections: &[SdkArtifactProjection],
    cargo_lock_payload: Option<&str>,
    cargo_lock_projection_root: Option<&str>,
    clear_cargo_lock: bool,
    cargo_target_dir: &Path,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"incan_rust_inspect_workspace/3\0");
    hasher.update(project_name.as_bytes());
    hasher.update(b"\0");
    hasher.update(cargo_package_name.as_bytes());
    hasher.update(b"\0");
    if clear_cargo_lock {
        hasher.update(b"clear_cargo_lock\0");
    }
    hasher.update(b"cargo_target_dir\0");
    hasher.update(cargo_target_dir.as_os_str().as_encoded_bytes());
    hasher.update(b"\0");
    if let Some(root) = cargo_lock_projection_root {
        hasher.update(b"lock_projection\0");
        hasher.update(root.as_bytes());
        hasher.update(b"\0");
    }
    match rust_edition {
        Some(e) => {
            hasher.update(b"ed\0");
            hasher.update(e.as_bytes());
            hasher.update(b"\0");
        }
        None => hasher.update(b"noed\0"),
    }
    // Matches `ProjectGenerator::new(..., is_binary: true)` + `set_include_dev_dependencies(true)` for this workspace.
    hasher.update(b"layout_bin_devdeps\0");

    let stdlib = normalized_stdlib_features_for_rust_inspect_fingerprint(stdlib_features);
    for f in &stdlib {
        hasher.update(f.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(b"|\0");

    let mut projections = sdk_artifact_projections.to_vec();
    normalize_sdk_artifact_projections(&mut projections);
    hasher.update(b"sdk_artifact_projections\0");
    for projection in projections {
        hasher.update(projection.artifact.crate_root.as_os_str().as_encoded_bytes());
        hasher.update(b"\0");
        match digest_provider_artifact(&projection.artifact.crate_root) {
            Ok(digest) => hasher.update(digest.as_bytes()),
            Err(error) => hasher.update(error.to_string().as_bytes()),
        }
        hasher.update(b"\0");
    }
    hasher.update(b"|\0");

    let mut rebindings = sdk_dependency_rebindings.to_vec();
    normalize_sdk_dependency_rebindings(&mut rebindings);
    hasher.update(b"sdk_dependency_rebindings\0");
    for rebinding in rebindings {
        hasher.update(rebinding.containing_artifact.crate_root.as_os_str().as_encoded_bytes());
        hasher.update(b"\0");
        hasher.update(rebinding.provider_name.as_bytes());
        hasher.update(b"\0");
        hasher.update(rebinding.dependency_key.as_bytes());
        hasher.update(b"\0");
        hasher.update(rebinding.source_crate_root.as_os_str().as_encoded_bytes());
        hasher.update(b"\0");
        hasher.update(rebinding.active_crate_root.as_os_str().as_encoded_bytes());
        hasher.update(b"\0");
        match digest_provider_artifact(&rebinding.active_crate_root) {
            Ok(digest) => hasher.update(digest.as_bytes()),
            Err(error) => hasher.update(error.to_string().as_bytes()),
        }
        hasher.update(b"\0");
    }
    hasher.update(b"|\0");

    let mut sdk_paths = sdk_path_dependencies.to_vec();
    sdk_paths.sort_by(|left, right| {
        (&left.crate_name, left.package.as_deref()).cmp(&(&right.crate_name, right.package.as_deref()))
    });
    hasher.update(b"sdk_path_dependencies\0");
    for dependency in &sdk_paths {
        hash_dependency_spec_for_rust_inspect(&mut hasher, dependency);
        if let DependencySource::Path { path } = &dependency.source {
            match digest_provider_artifact(path) {
                Ok(digest) => hasher.update(digest.as_bytes()),
                Err(error) => hasher.update(error.to_string().as_bytes()),
            }
            hasher.update(b"\0");
        }
    }
    hasher.update(b"|\0");

    let mut deps = resolved.dependencies.clone();
    deps.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));
    for dep in &mut deps {
        *dep = dep.clone().normalized();
    }
    hasher.update(b"deps\0");
    for dep in &deps {
        hash_dependency_spec_for_rust_inspect(&mut hasher, dep);
    }
    hasher.update(b"|\0");

    let mut dev_deps = resolved.dev_dependencies.clone();
    dev_deps.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));
    for dep in &mut dev_deps {
        *dep = dep.clone().normalized();
    }
    hasher.update(b"dev_deps\0");
    for dep in &dev_deps {
        hash_dependency_spec_for_rust_inspect(&mut hasher, dep);
    }
    hasher.update(b"|\0");

    match cargo_lock_payload {
        Some(lock) => {
            hasher.update(b"lock\0");
            hasher.update(lock.as_bytes());
        }
        None => hasher.update(b"nolock\0"),
    }

    format!(
        "{}{}",
        RUST_INSPECT_WORKSPACE_FINGERPRINT_PREFIX,
        hex::encode(hasher.finalize())
    )
}

/// Return the workspace directory used for Rust inspection metadata.
#[cfg(feature = "rust_inspect")]
fn rust_inspect_workspace_dir(project_root: &Path, project_name: &str, fingerprint: &str) -> PathBuf {
    let mut safe_name = project_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe_name.is_empty() {
        safe_name.push_str("project");
    }
    let suffix = fingerprint
        .rsplit_once(':')
        .map(|(_, hash)| hash)
        .unwrap_or(fingerprint)
        .chars()
        .take(16)
        .collect::<String>();
    crate::lockfile::compiler_lock_state_dir(project_root)
        .join("rust_inspect")
        .join(format!("{safe_name}-{suffix}"))
}

#[cfg(feature = "rust_inspect")]
/// Build a deterministic fingerprint for generated build-script metadata prewarm inputs and requested Rust paths.
fn rust_inspect_out_dirs_fingerprint(
    manifest_dir: &Path,
    target_dir: &Path,
    query_paths: &[String],
) -> CliResult<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"incan_rust_inspect_out_dirs/1\0");
    hasher.update(target_dir.as_os_str().as_encoded_bytes());
    hasher.update(b"\0");
    for relative in ["Cargo.toml", "Cargo.lock", "src/main.rs"] {
        let path = manifest_dir.join(relative);
        match fs::read(&path) {
            Ok(bytes) => {
                hasher.update(relative.as_bytes());
                hasher.update(b"\0");
                hasher.update(bytes);
                hasher.update(b"\0");
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && relative == "Cargo.lock" => {}
            Err(err) => {
                return Err(CliError::failure(format!(
                    "Failed to fingerprint rust-inspect out-dir prewarm input {}: {err}",
                    path.display()
                )));
            }
        }
    }
    let mut sorted_paths = query_paths.to_vec();
    sorted_paths.sort();
    sorted_paths.dedup();
    for query_path in sorted_paths {
        hasher.update(query_path.as_bytes());
        hasher.update(b"\0");
    }
    Ok(format!(
        "{}{}",
        RUST_INSPECT_OUT_DIRS_FINGERPRINT_FILE,
        hex::encode(hasher.finalize())
    ))
}

#[cfg(feature = "rust_inspect")]
/// Return whether the stored out-dir prewarm stamp matches the current fingerprint and the shared target still exists.
fn rust_inspect_out_dirs_stamp_matches(stamp_path: &Path, fingerprint: &str, target_dir: &Path) -> bool {
    target_dir.is_dir()
        && fs::read_to_string(stamp_path)
            .map(|existing| existing.trim() == fingerprint)
            .unwrap_or(false)
}

#[cfg(feature = "rust_inspect")]
/// Write a Cargo config that points the generated rust-inspect workspace at the shared target directory.
fn write_rust_inspect_cargo_config(manifest_dir: &Path, target_dir: &Path) -> CliResult<()> {
    let cargo_dir = manifest_dir.join(".cargo");
    fs::create_dir_all(&cargo_dir).map_err(|err| {
        CliError::failure(format!(
            "Failed to create rust-inspect Cargo config directory {}: {err}",
            cargo_dir.display()
        ))
    })?;
    let escaped_target_dir = target_dir.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    fs::write(
        cargo_dir.join("config.toml"),
        format!("[build]\ntarget-dir = \"{escaped_target_dir}\"\n"),
    )
    .map_err(|err| {
        CliError::failure(format!(
            "Failed to write rust-inspect Cargo config {}: {err}",
            cargo_dir.join("config.toml").display()
        ))
    })
}

#[cfg(feature = "rust_inspect")]
/// Point one generated rust-inspect workspace at its selected Cargo target.
pub(crate) fn configure_rust_inspect_cargo_target(manifest_dir: &Path, target_dir: &Path) -> CliResult<()> {
    write_rust_inspect_cargo_config(manifest_dir, target_dir)
}

#[cfg(feature = "rust_inspect")]
/// Detect Cargo's stale-lockfile failure so prewarm can retry with an offline lock refresh instead of silently
/// skipping.
fn rust_inspect_locked_prewarm_needs_lock_update(stderr: &str) -> bool {
    stderr.contains("--locked was passed")
        && stderr.contains("lock file")
        && (stderr.contains("cannot update") || stderr.contains("needs to be updated"))
}

#[cfg(feature = "rust_inspect")]
/// Run the Cargo command that warms generated build-script output for rust-inspect metadata extraction.
fn run_rust_inspect_out_dirs_prewarm_command(
    manifest_dir: &Path,
    target_dir: &Path,
    mode: RustInspectPrewarmCargoMode,
) -> CliResult<std::process::Output> {
    let mut command = crate::backend::project::runner::cargo_command();
    crate::backend::project::runner::configure_cargo_target(&mut command, target_dir);
    command.arg("check");
    command.arg("--manifest-path");
    command.arg(manifest_dir.join("Cargo.toml"));
    match mode {
        RustInspectPrewarmCargoMode::Locked if manifest_dir.join("Cargo.lock").is_file() => {
            command.arg("--locked");
        }
        RustInspectPrewarmCargoMode::Offline => {
            command.arg("--offline");
        }
        RustInspectPrewarmCargoMode::Locked => {}
    }
    command
        .env_remove("SSL_CERT_FILE")
        .env_remove("SSL_CERT_DIR")
        .env_remove("CURL_CA_BUNDLE")
        .env_remove("REQUESTS_CA_BUNDLE")
        .env_remove("CARGO_HTTP_CAINFO")
        .output()
        .map_err(|err| CliError::failure(format!("Failed to run rust-inspect build-script prewarm: {err}")))
}

#[cfg(feature = "rust_inspect")]
#[derive(Debug, Clone, Copy)]
enum RustInspectPrewarmCargoMode {
    Locked,
    Offline,
}

#[cfg(feature = "rust_inspect")]
/// Prewarm generated build-script output directories for rust-inspect lookups and stamp successful runs for reuse.
fn prewarm_rust_inspect_out_dirs(manifest_dir: &Path, target_dir: &Path, query_paths: &[String]) -> CliResult<()> {
    write_rust_inspect_cargo_config(manifest_dir, target_dir)?;
    let fingerprint = rust_inspect_out_dirs_fingerprint(manifest_dir, target_dir, query_paths)?;
    let stamp_path = manifest_dir.join(RUST_INSPECT_OUT_DIRS_FINGERPRINT_FILE);
    if rust_inspect_out_dirs_stamp_matches(&stamp_path, &fingerprint, target_dir) {
        return Ok(());
    }

    eprintln!(
        "rust-inspect build-script prewarm: checking generated metadata workspace into {}",
        target_dir.display()
    );
    let mut output =
        run_rust_inspect_out_dirs_prewarm_command(manifest_dir, target_dir, RustInspectPrewarmCargoMode::Locked)?;
    if !output.status.success()
        && rust_inspect_locked_prewarm_needs_lock_update(String::from_utf8_lossy(&output.stderr).as_ref())
    {
        eprintln!("rust-inspect build-script prewarm: generated Cargo.lock is stale; retrying offline lock refresh");
        output =
            run_rust_inspect_out_dirs_prewarm_command(manifest_dir, target_dir, RustInspectPrewarmCargoMode::Offline)?;
    }

    if !output.status.success() {
        return Err(CliError::failure(format!(
            "rust-inspect build-script prewarm failed with status {}:\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    fs::write(&stamp_path, &fingerprint).map_err(|err| {
        CliError::failure(format!(
            "Failed to write rust-inspect out-dir prewarm fingerprint {}: {err}",
            stamp_path.display()
        ))
    })?;
    Ok(())
}

/// Generate the rust-inspect workspace that semantic Rust extraction should query for this project.
///
/// The generated workspace intentionally uses the Rust import spelling for dependency keys, while preserving the
/// published Cargo package name separately when the two differ.
///
/// When the same inputs are seen again (for example across multiple `incan test` cases in one package), regeneration is
/// skipped if the namespaced workspace fingerprint matches the computed digest and expected artifacts exist.
#[cfg(feature = "rust_inspect")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn ensure_rust_inspect_workspace(
    project_root: &Path,
    project_name: &str,
    rust_edition: Option<String>,
    resolved: &ResolvedDependencies,
    project_requirements: &ProjectRequirements,
    cargo_lock_payload: Option<String>,
    cargo_target_dir: &Path,
    cargo_policy_flags: &[String],
) -> CliResult<PathBuf> {
    ensure_rust_inspect_workspace_with_cargo_package_name(
        project_root,
        project_name,
        project_name,
        rust_edition,
        resolved,
        project_requirements,
        cargo_lock_payload,
        None,
        false,
        cargo_target_dir,
        cargo_policy_flags,
    )
}

/// Generate a rust-inspect workspace whose Cargo package identity matches the canonical lock owner.
#[cfg(feature = "rust_inspect")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn ensure_rust_inspect_workspace_with_cargo_package_name(
    project_root: &Path,
    project_name: &str,
    cargo_package_name: &str,
    rust_edition: Option<String>,
    resolved: &ResolvedDependencies,
    project_requirements: &ProjectRequirements,
    cargo_lock_payload: Option<String>,
    cargo_lock_projection_root: Option<&str>,
    clear_cargo_lock: bool,
    cargo_target_dir: &Path,
    cargo_policy_flags: &[String],
) -> CliResult<PathBuf> {
    let fingerprint = rust_inspect_workspace_fingerprint(
        project_name,
        cargo_package_name,
        rust_edition.as_deref(),
        resolved,
        &project_requirements.stdlib_features,
        &project_requirements.sdk_dependency_rebindings,
        &project_requirements.sdk_path_dependencies,
        &project_requirements.sdk_artifact_projections,
        cargo_lock_payload.as_deref(),
        cargo_lock_projection_root,
        clear_cargo_lock,
        cargo_target_dir,
    );
    let rust_inspect_manifest_dir = rust_inspect_workspace_dir(project_root, project_name, &fingerprint);
    let fingerprint_path = rust_inspect_manifest_dir.join(RUST_INSPECT_WORKSPACE_FINGERPRINT_FILE);
    let cargo_toml_path = rust_inspect_manifest_dir.join("Cargo.toml");
    let main_rs_path = rust_inspect_manifest_dir.join("src").join("main.rs");

    let fingerprint_matches = match fs::read_to_string(&fingerprint_path) {
        Ok(existing) => existing.trim() == fingerprint.as_str(),
        Err(_) => false,
    };

    if cargo_toml_path.is_file()
        && main_rs_path.is_file()
        && fingerprint_matches
        && project_requirements.sdk_artifact_projections.is_empty()
    {
        return Ok(rust_inspect_manifest_dir);
    }

    let mut generator = ProjectGenerator::new(&rust_inspect_manifest_dir, project_name, true);
    generator.set_package_name(Some(cargo_package_name.to_string()));
    generator.set_dependencies(resolved.dependencies.clone());
    generator.set_dev_dependencies(resolved.dev_dependencies.clone());
    generator.set_include_dev_dependencies(true);
    generator.set_stdlib_features(project_requirements.stdlib_features.clone());
    generator.set_sdk_dependency_rebindings(project_requirements.sdk_dependency_rebindings.clone());
    generator.set_sdk_path_dependencies(project_requirements.sdk_path_dependencies.clone());
    generator.set_sdk_artifact_projections(project_requirements.sdk_artifact_projections.clone());
    generator.set_rust_edition(rust_edition);
    generator.set_cargo_lock_payload(cargo_lock_payload);
    generator.set_cargo_lock_projection_root(cargo_lock_projection_root.map(ToOwned::to_owned));
    generator.set_clear_cargo_lock(clear_cargo_lock);
    generator.set_cargo_policy_flags(cargo_policy_flags.to_vec());
    let mut referenced_crates = std::collections::BTreeSet::new();
    for dep in resolved.dependencies.iter().chain(resolved.dev_dependencies.iter()) {
        referenced_crates.insert(dep.crate_name.replace('-', "_"));
    }
    let mut rust_inspect_stub = String::new();
    for crate_name in referenced_crates {
        rust_inspect_stub.push_str(format!("use {crate_name} as _;\n").as_str());
    }
    rust_inspect_stub.push_str("fn main() {}");

    #[cfg(all(test, feature = "rust_inspect"))]
    record_test_rust_inspect_workspace_generation(&rust_inspect_manifest_dir);

    generator.generate(rust_inspect_stub.as_str()).map_err(|e| {
        CliError::failure(format!(
            "Failed to generate rust-inspect lock project at {}: {e}",
            rust_inspect_manifest_dir.display()
        ))
    })?;
    generator.materialize_cargo_lock_projection().map_err(|error| {
        CliError::failure(format!(
            "Failed to project rust-inspect Cargo.lock at {}: {error}",
            rust_inspect_manifest_dir.display()
        ))
    })?;

    if let Err(err) = fs::write(&fingerprint_path, &fingerprint) {
        return Err(CliError::failure(format!(
            "Failed to write rust-inspect workspace fingerprint {}: {err}",
            fingerprint_path.display()
        )));
    }

    Ok(rust_inspect_manifest_dir)
}

/// Collect canonical rust-inspect query paths from parsed `rust::` imports.
#[cfg(feature = "rust_inspect")]
pub(crate) fn collect_rust_inspect_query_paths(modules: &[ParsedModule]) -> Vec<String> {
    fn env_flag_enabled(name: &str) -> bool {
        std::env::var_os(name).is_some_and(|value| {
            let value = value.to_string_lossy();
            matches!(value.as_ref(), "1" | "true" | "TRUE" | "on" | "ON")
        })
    }

    // Default policy: prewarm explicit non-stdlib `from rust::... import Item` imports. These are the exact paths
    // semantic/codegen hot paths may query later, including Rust types with uppercase names.
    //
    // We still avoid crate/module imports and `incan_stdlib::*` by default. Full eager prewarm can force broad
    // rust-analyzer walks and persist negative module lookups that are not safe metadata items.
    // Set `INCAN_RUST_INSPECT_PREWARM_ALL=1` to restore full eager prewarm for debugging/regressions.
    let prewarm_all = env_flag_enabled("INCAN_RUST_INSPECT_PREWARM_ALL");
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for module in modules {
        for decl in &module.ast.declarations {
            let crate::frontend::ast::Declaration::Import(import) = &decl.node else {
                continue;
            };
            match &import.kind {
                ImportKind::RustCrate { crate_name, path, .. } if prewarm_all => {
                    let mut segments = Vec::with_capacity(path.len() + 1);
                    segments.push(crate_name.replace('-', "_"));
                    segments.extend(path.iter().cloned());
                    if !segments.is_empty() {
                        paths.insert(segments.join("::"));
                    }
                }
                ImportKind::RustCrate { .. } => {}
                ImportKind::RustFrom {
                    crate_name,
                    path,
                    items,
                    ..
                } => {
                    let mut base = Vec::with_capacity(path.len() + 1);
                    base.push(crate_name.replace('-', "_"));
                    base.extend(path.iter().cloned());
                    let base = base.join("::");
                    if base.is_empty() {
                        continue;
                    }
                    if !prewarm_all && base.starts_with("incan_stdlib::") {
                        continue;
                    }
                    let primitive_ns = matches!(base.as_str(), "std::primitive" | "core::primitive");
                    for item in items {
                        if !primitive_ns {
                            paths.insert(format!("{base}::{}", item.name));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    paths.into_iter().collect()
}

/// Return whether rust-inspect prewarm should run for the supplied environment value.
#[cfg(feature = "rust_inspect")]
fn parse_rust_inspect_prewarm_env(raw: Option<&str>) -> bool {
    let Some(raw) = raw else {
        return false;
    };
    matches!(raw.trim(), "1" | "true" | "TRUE" | "on" | "ON" | "yes" | "YES")
}

/// Return whether Rust inspection prewarming is enabled.
#[cfg(feature = "rust_inspect")]
fn rust_inspect_prewarm_enabled() -> bool {
    parse_rust_inspect_prewarm_env(std::env::var("INCAN_RUST_INSPECT_PREWARM").ok().as_deref())
}

/// Return whether rust-inspect should eagerly run Cargo to materialize every generated build-script `OUT_DIR`.
#[cfg(feature = "rust_inspect")]
fn parse_rust_inspect_eager_out_dirs_prewarm_env(raw: Option<&str>) -> bool {
    raw.is_some_and(|raw| matches!(raw.trim(), "1" | "true" | "TRUE" | "on" | "ON" | "yes" | "YES"))
}

/// Return whether rust-inspect should eagerly run Cargo to materialize every generated build-script `OUT_DIR`.
#[cfg(feature = "rust_inspect")]
fn rust_inspect_eager_out_dirs_prewarm_enabled() -> bool {
    parse_rust_inspect_eager_out_dirs_prewarm_env(
        std::env::var("INCAN_RUST_INSPECT_EAGER_OUT_DIRS_PREWARM")
            .ok()
            .as_deref(),
    )
}

/// Surface rust-inspect preparation progress from explicit CLI prewarm phases.
#[cfg(feature = "rust_inspect")]
fn print_rust_inspect_prewarm_progress(message: String) {
    if message.starts_with("rust-inspect prewarm") {
        eprintln!("{message}");
    }
}

/// Prepare rust-inspect metadata access before typechecking/codegen hot paths.
///
/// Metadata extraction now defaults to lazy lookup because eager rust-analyzer extraction across every imported Rust
/// path can dominate cold downstream builds before the real generated Rust build starts. The Cargo target configuration
/// is still prepared up front so lazy build-script `OUT_DIR` routes share the generated-project target directory. Set
/// `INCAN_RUST_INSPECT_PREWARM=1` to opt into eager metadata prewarm, and set
/// `INCAN_RUST_INSPECT_EAGER_OUT_DIRS_PREWARM=1` only when debugging a suspected out-dir cache regression.
#[cfg(feature = "rust_inspect")]
pub(crate) fn prewarm_rust_inspect_workspace(
    manifest_dir: &Path,
    target_dir: &Path,
    query_paths: &[String],
) -> CliResult<()> {
    configure_rust_inspect_cargo_target(manifest_dir, target_dir)?;
    if query_paths.is_empty() {
        return Ok(());
    }
    if !rust_inspect_prewarm_enabled() {
        return Ok(());
    }
    if rust_inspect_eager_out_dirs_prewarm_enabled() {
        prewarm_rust_inspect_out_dirs(manifest_dir, target_dir, query_paths)?;
    }
    let inspector = Inspector::new(InspectorConfig::new(manifest_dir.to_path_buf()));
    inspector
        .prewarm(query_paths.iter().cloned(), &print_rust_inspect_prewarm_progress)
        .map_err(|err| {
            CliError::failure(format!(
                "failed to prewarm rust-inspect cache from {}: {err}",
                manifest_dir.display()
            ))
        })
}

/// Resolve the source path for a stdlib module path (e.g. `["std", "testing"]`).
pub(crate) fn resolve_stdlib_module_source_path(module_path: &[String]) -> CliResult<PathBuf> {
    let Some(relative_stub_path) = stdlib::stdlib_stub_path(module_path) else {
        return Err(CliError::failure(format!(
            "Cannot resolve source for non-stdlib module path '{}'.",
            module_path.join(".")
        )));
    };

    let stdlib_relative = relative_stub_path
        .strip_prefix("stdlib/")
        .unwrap_or(relative_stub_path.as_str());
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(stdlib_dir) = crate::cli::prelude::find_stdlib_dir() {
        candidates.push(stdlib_dir.join(stdlib_relative));
    }
    candidates.push(PathBuf::from(&relative_stub_path));
    candidates.push(PathBuf::from("crates/incan_stdlib").join(&relative_stub_path));

    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(CliError::failure(format!(
        "Cannot resolve source file for '{}'; expected '{}' under stdlib search roots.",
        module_path.join("."),
        relative_stub_path
    )))
}

/// Read source file contents.
///
/// ## Errors
///
/// Returns an error if:
/// - The file cannot be read (I/O error)
/// - The file exceeds `MAX_SOURCE_SIZE` (100 MB)
pub fn read_source(file_path: &str) -> CliResult<String> {
    read_source_checked(file_path).map_err(|failure| CliError::failure(failure.message))
}

/// Read source for the stable diagnostic path, converting file-system failures into tooling diagnostics.
fn read_source_for_diagnostics(file_path: &str) -> Result<String, CliDiagnosticFailure> {
    read_source_checked(file_path).map_err(|failure| {
        CliDiagnosticFailure::single(
            file_path,
            "",
            diagnostics::CompileError::new(failure.message, Span::default()),
            diagnostics::DiagnosticPhase::Tooling,
        )
    })
}

/// Read source once behind both legacy text errors and structured diagnostic reporting.
fn read_source_checked(file_path: &str) -> Result<String, SourceReadFailure> {
    let metadata = fs::metadata(file_path).map_err(|error| SourceReadFailure {
        message: format!("Cannot access file '{}': {}", file_path, error),
    })?;
    if metadata.len() > MAX_SOURCE_SIZE {
        return Err(SourceReadFailure {
            message: format!(
                "Source file '{}' is too large ({} bytes, max {} bytes)",
                file_path,
                metadata.len(),
                MAX_SOURCE_SIZE
            ),
        });
    }
    fs::read_to_string(file_path).map_err(|error| SourceReadFailure {
        message: format!("Error reading file '{}': {}", file_path, error),
    })
}

/// Return whether a parsed module uses RFC 088 iterator surface methods that require stdlib adapter modules.
pub(crate) fn uses_iterator_adapter_surface(program: &Program) -> bool {
    ast_walk::any_expr_in_program(program, |expr| match expr {
        crate::frontend::ast::Expr::MethodCall(_, method, _, _) => matches!(
            method.as_str(),
            "iter"
                | "map"
                | "filter"
                | "enumerate"
                | "zip"
                | "take"
                | "skip"
                | "take_while"
                | "skip_while"
                | "chain"
                | "flat_map"
                | "batch"
                | "collect"
                | "count"
                | "reduce"
                | "fold"
                | "any"
                | "all"
                | "find"
                | "for_each"
                | "sum"
        ),
        _ => false,
    })
}

/// Return whether a parsed module uses RFC 070 Result combinators backed by std.result helpers.
pub(crate) fn uses_result_combinator_surface(program: &Program) -> bool {
    ast_walk::any_expr_in_program(program, |expr| match expr {
        crate::frontend::ast::Expr::MethodCall(_, method, _, _) => result_methods::from_str(method).is_some(),
        _ => false,
    })
}

/// Collect and parse the entry file and all its dependencies.
///
/// # Note on Prelude
///
/// The stdlib root prelude (`stdlib/prelude.incn`) exists, but it is not auto-imported into every compilation unit.
/// Unmigrated source-backed stdlib trait modules and builtin fallback traits are still discovered explicitly when the
/// parsed AST needs them. Migrated modules are resolved through the compiled built-in artifact instead.
pub fn collect_modules(entry_path: &str) -> CliResult<Vec<ParsedModule>> {
    collect_modules_detailed(entry_path).map_err(|failure| CliError::failure(failure.render_human()))
}

/// Collect and parse one compilation graph using an explicit Incan package-feature selection.
pub(crate) fn collect_modules_with_feature_selection(
    entry_path: &str,
    feature_selection: &FeatureSelection,
) -> CliResult<Vec<ParsedModule>> {
    collect_modules_detailed_with_feature_selection(entry_path, feature_selection)
        .map_err(|failure| CliError::failure(failure.render_human()))
}

/// Return whether the SDK catalog claims a module that source collection must never materialize locally.
///
/// Disabled and unavailable providers still own their namespace claims. Their imports must reach provider-aware
/// diagnostics instead of silently loading a nearby stdlib checkout and producing cascaded errors from the wrong
/// source graph.
fn sdk_catalog_claims_module_for_collection(provider_plan: &ProviderPlan, module_path: &[String]) -> bool {
    !matches!(
        provider_plan.resolve_module(module_path),
        ProviderModuleResolution::Unknown
    )
}

/// Collect and parse the entry file and all its dependencies, preserving structured diagnostic context.
pub(crate) fn collect_modules_detailed(entry_path: &str) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    collect_modules_detailed_with_feature_selection(entry_path, &FeatureSelection::default())
}

/// Collect and parse the entry file and all dependencies for one explicit Incan package-feature projection.
pub(crate) fn collect_modules_detailed_with_feature_selection(
    entry_path: &str,
    feature_selection: &FeatureSelection,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    collect_modules_detailed_with_selections(entry_path, feature_selection, None)
}

/// Collect and parse the entry file and all dependencies for one package-feature and SDK-profile projection.
pub(crate) fn collect_modules_detailed_with_selections(
    entry_path: &str,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    let path = if Path::new(entry_path).is_absolute() {
        PathBuf::from(entry_path)
    } else {
        std::env::current_dir()
            .map_err(|error| {
                CliDiagnosticFailure::single(
                    entry_path,
                    "",
                    diagnostics::CompileError::new(
                        format!("failed to determine current directory: {error}"),
                        Span::default(),
                    ),
                    diagnostics::DiagnosticPhase::Tooling,
                )
            })?
            .join(entry_path)
    };
    let session = match sdk_profile_override {
        Some(profile) => CompilationSession::discover_with_selections(&path, feature_selection, Some(profile)),
        None => CompilationSession::discover_with_feature_selection(&path, feature_selection),
    }
    .map_err(|error| {
        CliDiagnosticFailure::single(
            path.to_string_lossy(),
            "",
            diagnostics::CompileError::new(error.message, Span::default()),
            diagnostics::DiagnosticPhase::Import,
        )
    })?;
    collect_modules_detailed_with_session(path, &session)
}

/// Collect one source graph through an already-resolved compilation session.
pub(crate) fn collect_modules_detailed_with_session(
    path: PathBuf,
    session: &CompilationSession,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    let base_dir = path.parent().unwrap_or(Path::new("."));
    let mut modules = Vec::new();
    let mut processed = HashSet::new();
    let mut dependency_edges: HashMap<String, HashSet<String>> = HashMap::new();
    let mut incan_source_stdlib_module_paths: HashMap<String, PathBuf> = HashMap::new();
    let compiling_sdk_provider = env::var_os(SDK_PROVIDER_BUILD_ENV).is_some();
    let stdlib_module_segments = |module_path: &[String]| {
        if compiling_sdk_provider {
            module_path.iter().skip(1).cloned().collect()
        } else {
            let mut segments = vec![stdlib::INCAN_STD_NAMESPACE.to_string()];
            segments.extend(module_path.iter().skip(1).cloned());
            segments
        }
    };
    // (file_path, module_name, path_segments)
    let mut to_process: Vec<(String, String, Vec<String>)> = vec![(
        path.to_string_lossy().to_string(),
        "main".to_string(),
        vec!["main".to_string()],
    )];

    while let Some((file_path, module_name, path_segments)) = to_process.pop() {
        if processed.contains(&file_path) {
            continue;
        }
        processed.insert(file_path.clone());
        dependency_edges.entry(file_path.clone()).or_default();

        let source = read_source_for_diagnostics(&file_path)?;
        let file_path_obj = Path::new(&file_path);
        let is_incan_source_stdlib_module = path_segments
            .first()
            .is_some_and(|segment| segment == stdlib::INCAN_STD_NAMESPACE);
        let ast = match session.parse_source(file_path_obj, &source, !is_incan_source_stdlib_module) {
            Ok(a) => {
                // Surface any non-fatal parser warnings (e.g. RFC 005 dot-notation nudges) immediately,
                // so they reach the user regardless of which build/run/debug command was invoked.
                for warn in &a.warnings {
                    eprint!("{}", diagnostics::format_error(&file_path, &source, warn));
                }
                a
            }
            Err(errs) => {
                return Err(CliDiagnosticFailure::from_errors(
                    file_path,
                    source,
                    errs,
                    diagnostics::DiagnosticPhase::Parse,
                ));
            }
        };

        let current_base = file_path_obj.parent().unwrap_or(base_dir);
        if uses_iterator_adapter_surface(&ast) {
            let module_path = vec![
                stdlib::STDLIB_ROOT.to_string(),
                "derives".to_string(),
                "collection".to_string(),
            ];
            if !sdk_catalog_claims_module_for_collection(&session.provider_plan, &module_path) {
                let source_path = resolve_stdlib_module_source_path(&module_path)?;
                let module_segments = stdlib_module_segments(&module_path);
                let module_name = module_segments.join("_");
                let dep_path_str = source_path.to_string_lossy().to_string();
                if !processed.contains(&dep_path_str) {
                    to_process.push((dep_path_str.clone(), module_name, module_segments));
                }
                dependency_edges
                    .entry(file_path.clone())
                    .or_default()
                    .insert(dep_path_str);
            }
        }
        if uses_result_combinator_surface(&ast) {
            let module_path = vec![stdlib::STDLIB_ROOT.to_string(), "result".to_string()];
            if !sdk_catalog_claims_module_for_collection(&session.provider_plan, &module_path) {
                let source_path = resolve_stdlib_module_source_path(&module_path)?;
                let module_segments = stdlib_module_segments(&module_path);
                let module_name = module_segments.join("_");
                let dep_path_str = source_path.to_string_lossy().to_string();
                if !processed.contains(&dep_path_str) {
                    to_process.push((dep_path_str.clone(), module_name, module_segments));
                }
                dependency_edges
                    .entry(file_path.clone())
                    .or_default()
                    .insert(dep_path_str);
            }
        }
        for resolved in resolve_program_source_imports(&ast, current_base, Some(&session.source_root)) {
            match resolved.resolution {
                SourceModuleImportResolution::Stdlib { module_path } => {
                    if stdlib::stdlib_stub_path(&module_path).is_none() {
                        continue;
                    }
                    if sdk_catalog_claims_module_for_collection(&session.provider_plan, &module_path) {
                        continue;
                    }
                    let stdlib_key = module_path.join(".");
                    let source_path = if let Some(cached_path) = incan_source_stdlib_module_paths.get(&stdlib_key) {
                        cached_path.clone()
                    } else {
                        let resolved = resolve_stdlib_module_source_path(&module_path)?;
                        incan_source_stdlib_module_paths.insert(stdlib_key, resolved.clone());
                        resolved
                    };

                    let module_segments = stdlib_module_segments(&module_path);
                    let module_name = module_segments.join("_");
                    let dep_path_str = source_path.to_string_lossy().to_string();
                    if !processed.contains(&dep_path_str) {
                        to_process.push((dep_path_str.clone(), module_name, module_segments));
                    }
                    dependency_edges
                        .entry(file_path.clone())
                        .or_default()
                        .insert(dep_path_str);
                }
                SourceModuleImportResolution::Local(module_ref) => {
                    let dep_path_str = module_ref.file_path.to_string_lossy().to_string();
                    let module_segments = canonicalize_source_module_segments(&module_ref.path_segments);
                    let module_name = module_segments.join("_");
                    if !processed.contains(&dep_path_str) {
                        to_process.push((dep_path_str.clone(), module_name, module_segments));
                    }
                    dependency_edges
                        .entry(file_path.clone())
                        .or_default()
                        .insert(dep_path_str);
                }
                SourceModuleImportResolution::External => {}
            }
        }

        modules.push(ParsedModule {
            name: module_name,
            path_segments,
            file_path: PathBuf::from(&file_path),
            source,
            ast,
        });
    }

    Ok(topologically_sort_modules(modules, &dependency_edges)?)
}

/// Return modules in stable topological order (dependencies first).
///
/// Discovery traversal uses a stack, which is not guaranteed to produce dependency-safe ordering for siblings.
/// This explicit sort guarantees each module appears only after its direct and transitive dependencies for acyclic
/// portions of the graph. For cyclic components (for example stdlib prelude re-export loops), we keep deterministic
/// fallback ordering rather than hard-failing in collection.
pub(crate) fn topologically_sort_modules(
    modules: Vec<ParsedModule>,
    dependency_edges: &HashMap<String, HashSet<String>>,
) -> CliResult<Vec<ParsedModule>> {
    if modules.is_empty() {
        return Ok(modules);
    }

    let mut module_by_path: HashMap<String, ParsedModule> = HashMap::new();
    let mut order_index: HashMap<String, usize> = HashMap::new();
    for (idx, module) in modules.into_iter().enumerate() {
        let key = module.file_path.to_string_lossy().to_string();
        order_index.insert(key.clone(), idx);
        module_by_path.insert(key, module);
    }

    let mut indegree: HashMap<String, usize> = module_by_path.keys().cloned().map(|key| (key, 0usize)).collect();
    let mut reverse_adj: HashMap<String, Vec<String>> = HashMap::new();

    for (module_path, deps) in dependency_edges {
        if !module_by_path.contains_key(module_path) {
            continue;
        }
        for dep in deps {
            if !module_by_path.contains_key(dep) {
                continue;
            }
            if let Some(value) = indegree.get_mut(module_path) {
                *value += 1;
            }
            reverse_adj.entry(dep.clone()).or_default().push(module_path.clone());
        }
    }

    let mut ready: BTreeSet<(usize, String)> = indegree
        .iter()
        .filter_map(|(path, &degree)| {
            (degree == 0).then_some((order_index.get(path).copied().unwrap_or(usize::MAX), path.clone()))
        })
        .collect();

    let mut sorted = Vec::new();
    while let Some((_, next)) = ready.pop_first() {
        let Some(module) = module_by_path.remove(&next) else {
            continue;
        };
        sorted.push(module);

        if let Some(dependents) = reverse_adj.get(&next) {
            for dependent in dependents {
                if let Some(value) = indegree.get_mut(dependent)
                    && *value > 0
                {
                    *value -= 1;
                    if *value == 0 {
                        ready.insert((
                            order_index.get(dependent).copied().unwrap_or(usize::MAX),
                            dependent.clone(),
                        ));
                    }
                }
            }
        }
    }

    if !module_by_path.is_empty() {
        // Kahn's algorithm leaves cycle members (and dependents blocked by them) unresolved.
        // Preserve deterministic behavior by appending unresolved modules in reverse discovery order, which matches the
        // previous `modules.reverse()` shape that existing stdlib integration tests rely on.
        let mut unresolved: Vec<(usize, ParsedModule)> = module_by_path
            .into_iter()
            .map(|(path, module)| (order_index.get(&path).copied().unwrap_or(usize::MAX), module))
            .collect();
        unresolved.sort_by_key(|(idx, _)| std::cmp::Reverse(*idx));
        sorted.extend(unresolved.into_iter().map(|(_, module)| module));
    }

    Ok(sorted)
}

/// Resolve the project root from a source file path.
///
/// If the file is inside a `src/` directory (e.g. `src/main.incn` or `projects/foo/src/main.incn`), the project root
/// is the parent of `src/`. Otherwise, the project root is the file's parent directory.
///
/// Returns `"."` when the computed root would be empty (which happens for relative paths like `src/main.incn` where
/// the parent of `"src"` is `""`).
pub(crate) fn resolve_project_root(file_path: &Path) -> PathBuf {
    file_path
        .parent()
        .and_then(|p| {
            if p.file_name().is_some_and(|name| name == "src") {
                p.parent()
            } else {
                Some(p)
            }
        })
        .map(|p| {
            if p.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                p.to_path_buf()
            }
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Resolve the source root directory for a project.
///
/// The source root is where user module imports are resolved from. Resolution order:
///
/// 1. Explicit `[build] source-root` in the manifest (e.g. `source-root = "lib"`)
/// 2. Convention: `src/` directory exists relative to project root
/// 3. Fallback: project root itself (flat layout)
///
/// This is used by both the build pipeline and the test runner so that `from greet import greet` resolves to the same
/// file everywhere.
pub(crate) fn resolve_source_root(project_root: &Path, manifest: Option<&ProjectManifest>) -> PathBuf {
    // ---- Explicit configuration ----
    if let Some(source_root) = manifest
        .and_then(|m| m.build.as_ref())
        .and_then(|b| b.source_root.as_deref())
    {
        return project_root.join(source_root);
    }

    // ---- Convention: src/ directory ----
    let src_dir = project_root.join("src");
    if src_dir.is_dir() {
        return src_dir;
    }

    // ---- Fallback: project root (flat layout) ----
    project_root.to_path_buf()
}

/// Validate the output directory to prevent path traversal attacks.
///
/// This function ensures:
/// - The path doesn't contain `..` components
/// - The path doesn't start with `/` (absolute path outside workspace) unless it starts with a known safe prefix
pub(crate) fn validate_output_dir(out_dir: &str) -> CliResult<()> {
    let path = Path::new(out_dir);

    // Check for path traversal attempts
    for component in path.components() {
        if let std::path::Component::ParentDir = component {
            return Err(CliError::failure(format!(
                "Output directory '{}' contains path traversal (..)",
                out_dir
            )));
        }
    }

    // Warn about absolute paths (but allow them for flexibility)
    if path.is_absolute() {
        tracing::warn!(
            "Using absolute output path: {}. Consider using a relative path.",
            out_dir
        );
    }

    Ok(())
}

/// Format a Rust import base path like `rust::serde_json` or `rust::chrono::naive::date`.
pub(crate) fn format_rust_import_base_path(crate_name: &str, path: &[String]) -> String {
    if path.is_empty() {
        format!("rust::{}", crate_name)
    } else {
        format!("rust::{}::{}", crate_name, path.join("::"))
    }
}

/// Format a Rust from-import path like `from rust::serde_json import from_str, to_string`.
pub(crate) fn format_rust_from_import_path(crate_name: &str, path: &[String], imported: &[String]) -> String {
    format!(
        "from {} import {}",
        format_rust_import_base_path(crate_name, path),
        imported.join(", ")
    )
}

/// Build an inline Rust import record for dependency resolution.
pub(crate) fn build_inline_rust_import(
    crate_name: &str,
    import_path: String,
    version: &Option<String>,
    features: &[String],
    span: Span,
    file_path: &Path,
    is_test_context: bool,
) -> InlineRustImport {
    InlineRustImport {
        crate_name: crate_name.to_string(),
        import_path,
        version: version.clone(),
        features: features.to_vec(),
        span,
        file_path: file_path.to_path_buf(),
        is_test_context,
    }
}

/// Extract inline Rust crate imports from a parsed module.
pub(crate) fn collect_inline_rust_imports(module: &ParsedModule, is_test_context: bool) -> Vec<InlineRustImport> {
    let mut imports = Vec::new();

    for decl in &module.ast.declarations {
        let crate::frontend::ast::Declaration::Import(import) = &decl.node else {
            continue;
        };

        match &import.kind {
            ImportKind::RustCrate {
                crate_name,
                path,
                version,
                features,
                ..
            } => {
                let import_path = format_rust_import_base_path(crate_name, path);
                imports.push(build_inline_rust_import(
                    crate_name,
                    import_path,
                    version,
                    features,
                    decl.span,
                    &module.file_path,
                    is_test_context,
                ));
            }
            ImportKind::RustFrom {
                crate_name,
                path,
                items,
                version,
                features,
                ..
            } => {
                let imported = items.iter().map(|item| item.name.clone()).collect::<Vec<_>>();
                let import_path = format_rust_from_import_path(crate_name, path, &imported);
                imports.push(build_inline_rust_import(
                    crate_name,
                    import_path,
                    version,
                    features,
                    decl.span,
                    &module.file_path,
                    is_test_context,
                ));
            }
            _ => {}
        }
    }

    imports
}

/// Extract all Rust dependency uses from a parsed module.
pub(crate) fn collect_rust_dependency_uses(module: &ParsedModule, is_test_context: bool) -> Vec<InlineRustImport> {
    let mut imports = collect_inline_rust_imports(module, is_test_context);
    let Some(rust_module_path) = &module.ast.rust_module_path else {
        return imports;
    };
    let Some(crate_name) = rust_module_path.node.split("::").next().filter(|name| !name.is_empty()) else {
        return imports;
    };
    if crate_name == stdlib::STDLIB_ROOT || stdlib::is_path_extra_crate_dep(crate_name) {
        return imports;
    }

    imports.push(build_inline_rust_import(
        crate_name,
        format!("rust.module(\"{}\")", rust_module_path.node),
        &None,
        &[],
        rust_module_path.span,
        &module.file_path,
        is_test_context,
    ));
    imports
}

/// Build a map of file paths to source contents for error reporting.
pub(crate) fn build_source_map(modules: &[ParsedModule]) -> HashMap<PathBuf, String> {
    let mut sources = HashMap::new();
    for module in modules {
        sources.insert(module.file_path.clone(), module.source.clone());
    }
    sources
}

/// Format a dependency resolution error with source-file context.
pub(crate) fn format_dependency_error(error: &DependencyError, sources: &HashMap<PathBuf, String>) -> String {
    let file_path = error.file_path.to_string_lossy();
    if let Some(source) = sources.get(&error.file_path) {
        return diagnostics::format_error(&file_path, source, &error.error);
    }
    if let Ok(source) = fs::read_to_string(&error.file_path) {
        return diagnostics::format_error(&file_path, &source, &error.error);
    }

    format!("error: {}\n  --> {}\n", error.error.message, error.file_path.display())
}

/// Build Cargo policy flags (`--offline` / `--locked` / `--frozen`).
pub(crate) fn cargo_policy_flags(policy: &CargoPolicy) -> Vec<String> {
    if policy.frozen {
        return vec!["--frozen".to_string()];
    }

    let mut flags = Vec::new();
    if policy.offline {
        flags.push("--offline".to_string());
    }
    if policy.locked {
        flags.push("--locked".to_string());
    }
    flags
}

/// Build Cargo feature-selection flags without policy or arbitrary extra args.
fn cargo_feature_flags(cargo_features: &CargoFeatureSelection) -> Vec<String> {
    let mut flags = Vec::new();
    if cargo_features.cargo_all_features {
        flags.push("--all-features".to_string());
    }
    if cargo_features.cargo_no_default_features {
        flags.push("--no-default-features".to_string());
    }
    if !cargo_features.cargo_features.is_empty() {
        flags.push("--features".to_string());
        flags.push(cargo_features.cargo_features.join(","));
    }
    flags
}

/// Build flags for lockfile-oriented Cargo commands.
pub(crate) fn cargo_lockfile_flags(policy: &CargoPolicy, cargo_features: &CargoFeatureSelection) -> Vec<String> {
    let mut flags = cargo_policy_flags(policy);
    flags.extend(cargo_feature_flags(cargo_features));
    flags
}

/// Build Cargo command flags (policy flags + feature flags + extra Cargo args).
pub(crate) fn cargo_command_flags(policy: &CargoPolicy, cargo_features: &CargoFeatureSelection) -> Vec<String> {
    let mut flags = cargo_lockfile_flags(policy, cargo_features);
    flags.extend(policy.extra_args.clone());
    flags
}

/// Build a lookup map from canonical module key (`a_b_c`) to module index in `collect_modules` output.
pub(crate) fn module_key_index(modules: &[ParsedModule]) -> HashMap<String, usize> {
    let mut module_idx_by_key: HashMap<String, usize> = HashMap::new();
    for (idx, module) in modules.iter().enumerate() {
        let key = canonicalize_source_module_segments(&module.path_segments).join("_");
        module_idx_by_key.insert(key, idx);
    }
    module_idx_by_key
}

/// Resolve imported source-module dependencies for one collected module using a precomputed module key index.
///
/// Public signatures in a directly imported module may reference types from that module's own imports, so the
/// typechecker needs the transitive source-module dependency closure rather than just the immediate import list.
/// This helper preserves stable module ordering by returning dependencies in collected-module index order.
/// Bare sibling paths and absolute `crate.*` paths are both local source-module edges and must contribute to the same
/// closure.
///
/// Use this variant inside per-module loops to avoid rebuilding the module key map on every iteration.
pub(crate) fn imported_module_deps_for_with_index<'m>(
    modules: &'m [ParsedModule],
    module_index: usize,
    module_idx_by_key: &HashMap<String, usize>,
) -> Vec<(&'m str, &'m Program)> {
    // ---- Context: bounds and setup ----
    if module_index >= modules.len() {
        return Vec::new();
    }

    // ---- Context: walk the transitive local source-module import closure ----
    /// Collect immediate local source dependencies for one module from both bare and absolute `crate.*` imports.
    fn direct_local_dep_indexes(
        modules: &[ParsedModule],
        module_index: usize,
        module_idx_by_key: &HashMap<String, usize>,
    ) -> BTreeSet<usize> {
        /// Resolve one import path to the exact collected source module, including a safe nested-entry fallback.
        fn resolve_local_dep_index(
            current_module_path: &[String],
            path: &ImportPath,
            module_idx_by_key: &HashMap<String, usize>,
        ) -> Option<usize> {
            let exact = logical_source_import_candidates(current_module_path, path)
                .into_iter()
                .find_map(|candidate| {
                    let key = canonicalize_source_module_segments(&candidate).join("_");
                    module_idx_by_key.get(&key).copied()
                });
            if exact.is_some() || path.is_absolute || path.parent_levels > 0 {
                return exact;
            }

            // CLI entrypoints retain the synthetic logical name `main` even when their file lives in a nested source
            // directory. The on-disk resolver has already admitted the sibling into this module set; recover that
            // canonical identity only when the bare import suffix identifies exactly one collected module.
            let suffix = canonicalize_source_module_segments(&path.segments).join("_");
            if suffix.is_empty() {
                return None;
            }
            let suffix = format!("_{suffix}");
            let mut matches = module_idx_by_key
                .iter()
                .filter_map(|(key, index)| key.ends_with(&suffix).then_some(*index))
                .collect::<Vec<_>>();
            matches.sort_unstable();
            matches.dedup();
            match matches.as_slice() {
                [index] => Some(*index),
                _ => None,
            }
        }

        let mut dep_indexes: BTreeSet<usize> = BTreeSet::new();
        for decl in &modules[module_index].ast.declarations {
            let crate::frontend::ast::Declaration::Import(import) = &decl.node else {
                continue;
            };
            match &import.kind {
                ImportKind::From { module, .. } => {
                    if let Some(dep_idx) =
                        resolve_local_dep_index(&modules[module_index].path_segments, module, module_idx_by_key)
                        && dep_idx != module_index
                    {
                        dep_indexes.insert(dep_idx);
                    }
                }
                ImportKind::Module(path) => {
                    let dep_idx = resolve_local_dep_index(
                        &modules[module_index].path_segments,
                        path,
                        module_idx_by_key,
                    )
                    .or_else(|| {
                        let mut parent_path = path.clone();
                        parent_path.segments.pop();
                        resolve_local_dep_index(&modules[module_index].path_segments, &parent_path, module_idx_by_key)
                    });
                    if let Some(dep_idx) = dep_idx
                        && dep_idx != module_index
                    {
                        dep_indexes.insert(dep_idx);
                    }
                }
                _ => {}
            }
        }
        dep_indexes
    }

    let mut dep_indexes: BTreeSet<usize> = BTreeSet::new();
    let mut pending: Vec<usize> = direct_local_dep_indexes(modules, module_index, module_idx_by_key)
        .into_iter()
        .collect();
    while let Some(dep_idx) = pending.pop() {
        if dep_idx == module_index || !dep_indexes.insert(dep_idx) {
            continue;
        }
        pending.extend(direct_local_dep_indexes(modules, dep_idx, module_idx_by_key));
    }

    // ---- Context: materialize dependency pairs for typechecker.check_with_imports ----
    dep_indexes
        .into_iter()
        .map(|idx| (modules[idx].name.as_str(), &modules[idx].ast))
        .collect()
}

/// Typecheck all collected modules in dependency-safe order using shared CLI diagnostics formatting.
///
/// This helper centralizes the per-module checker setup used by `build` and `check` paths so warning/error rendering
/// stays consistent across command flows.
#[cfg(test)]
pub(crate) fn typecheck_modules_with_import_graph(
    modules: &[ParsedModule],
    manifest: Option<&ProjectManifest>,
    provider_plan: &Arc<ProviderPlan>,
    #[cfg(feature = "rust_inspect")] rust_inspect_manifest_dir: Option<&Path>,
) -> CliResult<()> {
    typecheck_modules_with_import_graph_detailed(
        modules,
        manifest,
        provider_plan,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir,
    )
    .map_err(|failure| CliError::failure(failure.render_human()))
}

/// Typecheck all collected modules and preserve diagnostics as structured data for stable reporting.
pub(crate) fn typecheck_modules_with_import_graph_detailed(
    modules: &[ParsedModule],
    manifest: Option<&ProjectManifest>,
    provider_plan: &Arc<ProviderPlan>,
    #[cfg(feature = "rust_inspect")] rust_inspect_manifest_dir: Option<&Path>,
) -> Result<(), CliDiagnosticFailure> {
    typecheck_modules_with_import_graph_info(
        modules,
        manifest,
        provider_plan,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir,
    )
    .map(|_| ())
}

/// Typecheck all collected modules and return reusable typechecker artifacts for successfully checked modules.
pub(crate) fn typecheck_modules_with_import_graph_info(
    modules: &[ParsedModule],
    manifest: Option<&ProjectManifest>,
    provider_plan: &Arc<ProviderPlan>,
    #[cfg(feature = "rust_inspect")] rust_inspect_manifest_dir: Option<&Path>,
) -> Result<BTreeMap<PathBuf, TypeCheckInfo>, CliDiagnosticFailure> {
    let typecheck_artifacts = typecheck_modules_with_import_graph_artifacts(
        modules,
        manifest,
        provider_plan,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir,
    )?;
    Ok(modules
        .iter()
        .zip(typecheck_artifacts.type_infos)
        .map(|(module, type_info)| (module.file_path.clone(), type_info))
        .collect())
}

/// Products retained from one dependency-safe typechecking pass.
struct TypecheckModuleArtifacts {
    type_infos: Vec<TypeCheckInfo>,
    stdlib_cache: StdlibAstCache,
}

/// Typecheck a collected graph in dependency-safe order and retain one result for every input module plus the
/// source-backed stdlib metadata lowering needs.
///
/// Ordering is intentional: session analysis also needs an identity-keyed representation for synthetic modules that
/// share a source file path.
fn typecheck_modules_with_import_graph_artifacts(
    modules: &[ParsedModule],
    manifest: Option<&ProjectManifest>,
    provider_plan: &Arc<ProviderPlan>,
    #[cfg(feature = "rust_inspect")] rust_inspect_manifest_dir: Option<&Path>,
) -> Result<TypecheckModuleArtifacts, CliDiagnosticFailure> {
    let declared = manifest.map(|m| m.declared_rust_crate_names());
    let module_idx_by_key = module_key_index(modules);
    let mut diagnostics_out = Vec::new();
    let mut type_infos = Vec::with_capacity(modules.len());
    let mut stdlib_cache = StdlibAstCache::new();

    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module = imported_module_deps_for_with_index(modules, idx, &module_idx_by_key);

        let mut checker = typechecker::TypeChecker::new();
        checker.stdlib_cache = stdlib_cache.clone();
        if let Some(names) = declared.clone() {
            checker.set_declared_crate_names(names);
        }
        checker.set_current_module_path(Some(module.path_segments.clone()));
        checker.set_provider_plan(Arc::clone(provider_plan));
        #[cfg(feature = "rust_inspect")]
        if let Some(rust_inspect_manifest_dir) = rust_inspect_manifest_dir {
            checker.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir.to_path_buf());
        }

        // A provider producer checks its complete source package before publishing the public checked facade.
        let check_result = if provider_plan.bootstrap_sdk_namespace_roots().next().is_some() {
            checker.check_with_imports_allow_private(&module.ast, &deps_for_module)
        } else {
            checker.check_with_imports(&module.ast, &deps_for_module)
        };
        match check_result {
            Ok(()) => {
                for warn in checker.warnings() {
                    eprint!(
                        "{}",
                        diagnostics::format_error(module.file_path.to_string_lossy().as_ref(), &module.source, warn)
                    );
                }
                type_infos.push(checker.type_info().clone());
                stdlib_cache = checker.stdlib_cache.clone();
            }
            Err(errs) => {
                stdlib_cache = checker.stdlib_cache.clone();
                diagnostics_out.extend(errs.into_iter().map(|error| CliDiagnostic {
                    file_path: module.file_path.to_string_lossy().to_string(),
                    source: module.source.clone(),
                    phase: typecheck_diagnostic_phase(module, error.span),
                    error,
                }));
            }
        }
    }

    if diagnostics_out.is_empty() {
        Ok(TypecheckModuleArtifacts {
            type_infos,
            stdlib_cache,
        })
    } else {
        Err(CliDiagnosticFailure {
            diagnostics: diagnostics_out,
        })
    }
}

/// Classify diagnostics that are still emitted by the typechecker but originate from an import declaration span.
fn typecheck_diagnostic_phase(module: &ParsedModule, span: Span) -> diagnostics::DiagnosticPhase {
    diagnostics::phase_for_typecheck_span(&module.ast, span)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::typechecker::{self, IdentKind};
    use crate::library_manifest::{LibraryManifest, ProviderFeatureMetadata, ProviderModuleClaim, VocabExports};
    use std::path::Path;

    fn parsed_module_for_test(source: &str) -> Result<ParsedModule, Box<dyn std::error::Error>> {
        let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
        Ok(ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("main.incn"),
            source: source.to_string(),
            ast,
        })
    }

    #[test]
    fn sdk_provider_builder_selects_the_real_cli_for_tests_and_utilities() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let cargo_cli = temp_dir.path().join("incan-cli");
        fs::write(&cargo_cli, "test binary")?;
        assert_eq!(
            sdk_provider_builder_executable(Some(cargo_cli.clone()), PathBuf::from("/tmp/integration-test"),)?,
            cargo_cli
        );

        let target_dir = temp_dir.path().join("target/debug");
        fs::create_dir_all(&target_dir)?;
        let mut sibling_cli = target_dir.join("incan");
        sibling_cli.set_extension(std::env::consts::EXE_EXTENSION);
        fs::write(&sibling_cli, "cli binary")?;
        assert_eq!(
            sdk_provider_builder_executable(None, target_dir.join("generate_feature_inventory"))?,
            sibling_cli
        );

        let direct_cli = temp_dir.path().join("incan");
        fs::write(&direct_cli, "installed cli")?;
        assert_eq!(
            sdk_provider_builder_executable(Some(PathBuf::from("/tmp/stale-incan-cli")), direct_cli.clone(),)?,
            direct_cli
        );

        let deps_dir = target_dir.join("deps");
        fs::create_dir_all(&deps_dir)?;
        assert_eq!(
            sdk_provider_builder_executable(None, deps_dir.join("incan-abc123"))?,
            sibling_cli
        );
        Ok(())
    }

    #[test]
    fn sdk_provider_builder_rejects_a_utility_without_a_sibling_cli() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let utility = temp_dir.path().join("generate_feature_inventory");
        fs::write(&utility, "utility binary")?;

        let error = match sdk_provider_builder_executable(None, utility) {
            Err(error) => error,
            Ok(path) => return Err(format!("missing sibling CLI unexpectedly resolved to {}", path.display()).into()),
        };
        assert!(error.message.contains("requires the incan CLI executable"));
        Ok(())
    }

    #[test]
    fn sdk_provider_build_uses_transaction_local_cargo_target() {
        let mut command = Command::new("incan");
        let target = Path::new("/staging/.cargo-target");
        configure_sdk_provider_build_environment(&mut command, "stdlib-core", target, None);
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
        );
        let configured_target = command
            .get_envs()
            .find_map(|(name, value)| (name == GENERATED_CARGO_TARGET_DIR_ENV).then_some(value))
            .flatten();
        assert_eq!(configured_target, Some(caller_target.as_os_str()));
    }

    #[test]
    fn explicit_sdk_selection_rejects_legacy_inventoryless_toolchains() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let manifest_path = project.path().join("incan.toml");
        fs::write(
            &manifest_path,
            "[project]\nname = \"demo\"\n\n[sdk]\nprofile = \"minimal\"\n",
        )?;
        let manifest = ProjectManifest::load(&manifest_path)?;
        let error = validate_component_inventory_selection(Some(&manifest), None, None)
            .err()
            .ok_or("expected explicit SDK selection to require an inventory")?;

        assert!(error.message.contains("no component inventory"));
        assert!(
            error.message.contains("incan.toml:4:1"),
            "expected the explicit SDK table location, got: {}",
            error.message
        );
        assert!(validate_component_inventory_selection(None, None, None).is_ok());
        Ok(())
    }

    #[test]
    fn sdk_selection_errors_retain_manifest_or_command_provenance() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let manifest_path = project.path().join("incan.toml");
        fs::write(
            &manifest_path,
            "[project]\nname = \"demo\"\n\n[sdk]\nprofile = \"minimal\"\ncomponents = [\"stdlib-web\"]\n",
        )?;
        let manifest = ProjectManifest::load(&manifest_path)?;
        let selection = SdkComponentSelection::from_manifest(Some(&manifest));
        let component_error = SdkResolutionError::UnknownComponent {
            component: "stdlib-web".to_string(),
            sdk_identity: "incan@0.5.0".to_string(),
        };

        let rendered = format_sdk_selection_error(&component_error, &selection, Some(&manifest), None);
        assert!(
            rendered.contains("incan.toml:6:15"),
            "expected exact SDK component location, got: {rendered}"
        );

        let profile_error = SdkResolutionError::UnknownProfile {
            profile: "tiny".to_string(),
            sdk_identity: "incan@0.5.0".to_string(),
        };
        let rendered = format_sdk_selection_error(&profile_error, &selection, Some(&manifest), Some("tiny"));
        assert!(
            rendered.contains("current command's `--sdk-profile` override"),
            "expected transient profile provenance, got: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn sdk_provider_build_uses_enclosing_workspace_lock() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let workspace = tmp.path().join("workspace");
        let stdlib_root = workspace.join("crates/incan_stdlib/stdlib");
        let artifact_root = stdlib_root.join("target/lib");
        fs::create_dir_all(&stdlib_root)?;
        fs::write(workspace.join("Cargo.lock"), "workspace lock payload")?;

        let workspace_lock = sdk_provider_workspace_lock(&stdlib_root);
        seed_sdk_provider_workspace_lock(workspace_lock.as_deref(), &artifact_root)?;

        assert_eq!(
            fs::read_to_string(artifact_root.join("Cargo.lock"))?,
            "workspace lock payload"
        );
        Ok(())
    }

    #[test]
    fn sdk_provider_store_identity_tracks_source_and_lock_inputs() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let stdlib_root = temp_dir.path().join("stdlib");
        fs::create_dir_all(stdlib_root.join("nested"))?;
        fs::write(stdlib_root.join("incan.toml"), "[project]\nname = \"stdlib\"\n")?;
        fs::write(
            stdlib_root.join("nested").join("module.incn"),
            "pub def value() -> int:\n  return 1\n",
        )?;
        let workspace_lock = temp_dir.path().join("Cargo.lock");
        fs::write(&workspace_lock, "first lock closure")?;
        let executable = temp_dir.path().join("compiler-a");
        fs::write(&executable, "compiler payload")?;

        let initial = sdk_provider_store_identity(&stdlib_root, &executable, Some(&workspace_lock), "full")?;
        let relocated_executable = temp_dir.path().join("relocated").join("compiler-b");
        fs::create_dir_all(
            relocated_executable
                .parent()
                .ok_or("relocated compiler had no parent")?,
        )?;
        fs::copy(&executable, &relocated_executable)?;
        let relocated =
            sdk_provider_store_identity(&stdlib_root, &relocated_executable, Some(&workspace_lock), "full")?;
        assert_eq!(
            initial, relocated,
            "identical compiler bytes must reuse provider artifacts across paths"
        );
        let changed_executable = temp_dir.path().join("compiler-changed");
        fs::write(&changed_executable, "different compiler payload")?;
        let compiler_changed =
            sdk_provider_store_identity(&stdlib_root, &changed_executable, Some(&workspace_lock), "full")?;
        assert_ne!(
            initial, compiler_changed,
            "changing compiler bytes must invalidate provider artifacts"
        );
        fs::write(
            stdlib_root.join("nested").join("module.incn"),
            "pub def value() -> int:\n  return 2\n",
        )?;
        let source_changed = sdk_provider_store_identity(&stdlib_root, &executable, Some(&workspace_lock), "full")?;
        assert_ne!(
            initial, source_changed,
            "changing a stdlib source must invalidate its artifact identity"
        );

        fs::write(&workspace_lock, "second lock closure")?;
        let lock_changed = sdk_provider_store_identity(&stdlib_root, &executable, Some(&workspace_lock), "full")?;
        assert_ne!(
            source_changed, lock_changed,
            "changing the resolved Cargo closure must invalidate its artifact identity"
        );
        let minimal = sdk_provider_store_identity(&stdlib_root, &executable, Some(&workspace_lock), "minimal")?;
        assert_ne!(
            lock_changed, minimal,
            "distribution profiles must not share provider-store identities"
        );
        Ok(())
    }

    #[test]
    fn restricted_sdk_profile_retains_unavailable_provider_catalog_facts() -> Result<(), Box<dyn std::error::Error>> {
        let catalog_path = Path::new(env!("CARGO_MANIFEST_DIR"))
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
    fn sdk_provider_store_defaults_to_the_shared_incan_cache() {
        let stdlib_root = Path::new("/workspace/stdlib");
        assert_eq!(
            default_sdk_provider_store(stdlib_root, Some("/opt/incan-home".into()), Some("/home/user".into())),
            Path::new("/opt/incan-home/cache/providers/sdk-v2")
        );
        assert_eq!(
            default_sdk_provider_store(stdlib_root, None, Some("/home/user".into())),
            Path::new("/home/user/.incan/cache/providers/sdk-v2")
        );
        assert_eq!(
            default_sdk_provider_store(stdlib_root, None, None),
            Path::new("/workspace/stdlib/target/incan_sdk_components")
        );
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

    fn write_minimal_library_artifact(
        root: &Path,
        dependency_key: &str,
        manifest_name: &str,
        manifest: &LibraryManifest,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = root.join("deps").join(dependency_key).join("target").join("lib");
        std::fs::create_dir_all(artifact_root.join("src"))?;
        std::fs::write(
            artifact_root.join("Cargo.toml"),
            format!("[package]\nname = \"{manifest_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )?;
        std::fs::write(artifact_root.join("src/lib.rs"), "")?;
        manifest.write_to_path(&artifact_root.join(format!("{manifest_name}.incnlib")))?;
        Ok(())
    }

    #[test]
    fn collect_rust_dependency_uses_includes_rust_module_root() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test("rust.module(\"datafusion::prelude\")\n\ndef main() -> None:\n  pass\n")?;

        let imports = collect_rust_dependency_uses(&module, false);

        assert!(
            imports.iter().any(|import| import.crate_name == "datafusion"
                && import.import_path == "rust.module(\"datafusion::prelude\")"),
            "rust.module roots should participate in dependency resolution: {imports:?}"
        );
        Ok(())
    }

    #[test]
    fn collect_rust_dependency_uses_skips_stdlib_path_extra_crate_roots() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test("rust.module(\"incan_web_macros\")\n\ndef main() -> None:\n  pass\n")?;

        let imports = collect_rust_dependency_uses(&module, false);

        assert!(
            imports.iter().all(|import| import.crate_name != "incan_web_macros"),
            "stdlib-managed path crates should come from project requirements, not rust.module dependency uses: {imports:?}"
        );
        Ok(())
    }

    #[test]
    fn compilation_session_parses_with_imported_library_vocab() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"deps/widgets\" }\n",
        )?;

        let mut manifest = LibraryManifest::new("widgets_core", "0.1.0");
        manifest.vocab = Some(VocabExports {
            crate_path: "widgets_vocab_companion".to_string(),
            package_name: "widgets_vocab_companion".to_string(),
            keyword_registrations: vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "widgets.dsl".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec::new(
                    "assert",
                    incan_vocab::KeywordSurfaceKind::ControlFlow,
                )],
                valid_decorators: Vec::new(),
            }],
            dsl_surfaces: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest::default(),
            desugarer_artifact: None,
        });
        std::fs::create_dir_all(project_root.join("deps/widgets"))?;
        std::fs::write(
            project_root.join("deps/widgets/incan.toml"),
            "[project]\nname = \"widgets\"\nversion = \"0.1.0\"\n",
        )?;
        write_minimal_library_artifact(project_root, "widgets", "widgets_core", &manifest)?;

        let main_path = project_root.join("src/main.incn");
        let source = "import pub::widgets\n\ndef main() -> None:\n  assert true\n";
        std::fs::write(&main_path, source)?;

        let session = CompilationSession::discover_with_feature_selection(&main_path, &FeatureSelection::default())?;
        session
            .parse_source(&main_path, source, false)
            .map_err(|errors| format!("expected session parse to use imported vocab: {errors:?}"))?;

        Ok(())
    }

    // ---- resolve_project_root ----

    #[test]
    fn project_root_from_relative_src_is_dot_not_empty() {
        // Regression: `src/main.incn` used to yield "" instead of ".", causing
        // `Command::current_dir("")` to fail with ENOENT.
        let root = resolve_project_root(Path::new("src/main.incn"));
        assert_eq!(root, PathBuf::from("."));
    }

    #[test]
    fn project_root_from_nested_src_path() {
        let root = resolve_project_root(Path::new("projects/greeter/src/main.incn"));
        assert_eq!(root, PathBuf::from("projects/greeter"));
    }

    #[test]
    fn project_root_from_absolute_src_path() {
        let root = resolve_project_root(Path::new("/home/user/project/src/main.incn"));
        assert_eq!(root, PathBuf::from("/home/user/project"));
    }

    #[test]
    fn cargo_policy_resolves_env_defaults_and_frozen_implication() {
        let policy = CargoPolicy::from_sources(
            CargoPolicyCliFlags::default(),
            Vec::new(),
            Vec::new(),
            |name| match name {
                "INCAN_FROZEN" => Some("1".to_string()),
                "INCAN_CARGO_ARGS" => Some("--timings --verbose".to_string()),
                _ => None,
            },
        );

        assert!(policy.frozen);
        assert!(policy.offline);
        assert!(policy.locked);
        assert_eq!(policy.extra_args, vec!["--timings", "--verbose"]);
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_prewarm_env_defaults_to_disabled() {
        assert!(!parse_rust_inspect_prewarm_env(None));
        assert!(!parse_rust_inspect_prewarm_env(Some("")));
        assert!(parse_rust_inspect_prewarm_env(Some("1")));
        assert!(parse_rust_inspect_prewarm_env(Some("true")));
        assert!(parse_rust_inspect_prewarm_env(Some("on")));
        assert!(parse_rust_inspect_prewarm_env(Some("YES")));
        assert!(!parse_rust_inspect_prewarm_env(Some("0")));
        assert!(!parse_rust_inspect_prewarm_env(Some("false")));
        assert!(!parse_rust_inspect_prewarm_env(Some(" OFF ")));
        assert!(!parse_rust_inspect_prewarm_env(Some("no")));
        assert!(!parse_rust_inspect_prewarm_env(Some("unexpected")));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_eager_out_dir_prewarm_env_defaults_to_disabled() {
        assert!(!parse_rust_inspect_eager_out_dirs_prewarm_env(None));
        assert!(!parse_rust_inspect_eager_out_dirs_prewarm_env(Some("")));
        assert!(parse_rust_inspect_eager_out_dirs_prewarm_env(Some("1")));
        assert!(parse_rust_inspect_eager_out_dirs_prewarm_env(Some("true")));
        assert!(parse_rust_inspect_eager_out_dirs_prewarm_env(Some("ON")));
        assert!(parse_rust_inspect_eager_out_dirs_prewarm_env(Some("yes")));
        assert!(!parse_rust_inspect_eager_out_dirs_prewarm_env(Some("0")));
        assert!(!parse_rust_inspect_eager_out_dirs_prewarm_env(Some("false")));
        assert!(!parse_rust_inspect_eager_out_dirs_prewarm_env(Some("unexpected")));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_query_paths_include_explicit_rust_item_imports() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
from rust::datafusion::execution::context import SessionContext
from rust::datafusion::prelude import CsvReadOptions, read_csv
from rust::incan_stdlib::async::runtime import block_on
from rust::std::fs import metadata
from rust::std::primitive import i64 as RustI64
"#,
        )?;

        let paths = collect_rust_inspect_query_paths(&[module]);

        assert_eq!(
            paths,
            vec![
                "datafusion::execution::context::SessionContext".to_string(),
                "datafusion::prelude::CsvReadOptions".to_string(),
                "datafusion::prelude::read_csv".to_string(),
                "std::fs::metadata".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn cargo_policy_uses_cli_extra_args_before_env_extra_args() {
        let policy = CargoPolicy::from_sources(
            CargoPolicyCliFlags {
                offline: true,
                ..CargoPolicyCliFlags::default()
            },
            vec!["--features".to_string(), "cli".to_string()],
            vec!["--no-default-features".to_string()],
            |name| match name {
                "INCAN_CARGO_ARGS" => Some("--features env".to_string()),
                _ => None,
            },
        );

        assert!(policy.offline);
        assert_eq!(policy.extra_args, vec!["--features", "cli", "--no-default-features"]);
    }

    #[test]
    fn cargo_policy_cli_disable_flags_override_env_defaults() {
        let policy = CargoPolicy::from_sources(
            CargoPolicyCliFlags {
                no_offline: true,
                no_locked: true,
                no_frozen: true,
                ..CargoPolicyCliFlags::default()
            },
            Vec::new(),
            Vec::new(),
            |name| match name {
                "INCAN_OFFLINE" | "INCAN_LOCKED" | "INCAN_FROZEN" => Some("1".to_string()),
                _ => None,
            },
        );

        assert!(!policy.offline);
        assert!(!policy.locked);
        assert!(!policy.frozen);
    }

    #[test]
    fn cargo_command_flags_order_policy_features_then_extra_args() {
        let policy = CargoPolicy::explicit(
            true,
            true,
            false,
            vec!["--timings".to_string(), "--color=always".to_string()],
        );
        let features = CargoFeatureSelection {
            cargo_features: vec!["json".to_string(), "web".to_string()],
            cargo_no_default_features: true,
            cargo_all_features: false,
        };

        assert_eq!(
            cargo_command_flags(&policy, &features),
            vec![
                "--offline",
                "--locked",
                "--no-default-features",
                "--features",
                "json,web",
                "--timings",
                "--color=always"
            ]
        );
    }

    #[test]
    fn project_root_when_file_is_not_in_src() {
        // File directly in a directory, not in src/
        let root = resolve_project_root(Path::new("main.incn"));
        assert_eq!(root, PathBuf::from("."));
    }

    #[test]
    fn project_root_from_non_src_subdirectory() {
        let root = resolve_project_root(Path::new("lib/utils.incn"));
        assert_eq!(root, PathBuf::from("lib"));
    }

    // ---- resolve_source_root ----

    #[test]
    fn source_root_uses_src_convention() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("myproject");
        fs::create_dir_all(project.join("src"))?;

        let root = resolve_source_root(&project, None);
        assert_eq!(root, project.join("src"));
        Ok(())
    }

    #[test]
    fn source_root_falls_back_to_project_root_when_no_src() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("flat_project");
        fs::create_dir_all(&project)?;

        let root = resolve_source_root(&project, None);
        assert_eq!(root, project);
        Ok(())
    }

    #[test]
    fn source_root_respects_explicit_manifest_config() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("custom_src");
        fs::create_dir_all(project.join("src"))?; // src/ exists but should be overridden

        let manifest_content = r#"
[build]
source-root = "lib"
"#;
        let manifest = ProjectManifest::from_str(manifest_content, &project.join("incan.toml"))?;

        let root = resolve_source_root(&project, Some(&manifest));
        assert_eq!(root, project.join("lib"));
        Ok(())
    }

    #[test]
    fn collect_project_requirements_defers_sdk_namespace_features_to_provider_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
import std.async
from std.math import sqrt
"#,
        )?;

        let requirements = collect_project_requirements(&[module], &LibraryManifestIndex::default())?;
        assert!(requirements.stdlib_features.is_empty());
        assert!(requirements.dependencies.is_empty());
        Ok(())
    }

    #[test]
    fn collect_project_requirements_defers_imported_serde_runtime_to_provider_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
from std.serde import json

@derive(json)
model User:
    name: str
"#,
        )?;

        let requirements = collect_project_requirements(&[module], &LibraryManifestIndex::default())?;
        assert!(requirements.stdlib_features.is_empty());
        assert!(requirements.dependencies.is_empty());
        Ok(())
    }

    #[test]
    fn collect_modules_canonicalizes_directory_entrypoints() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )?;

        let src_dir = project_root.join("src");
        std::fs::create_dir_all(src_dir.join("dataset"))?;
        std::fs::write(
            src_dir.join("lib.incn"),
            "from dataset.mod import DataSet\nfrom dataset.ops import filter_ds\n",
        )?;
        std::fs::write(
            src_dir.join("dataset").join("mod.incn"),
            "pub trait DataSet[T]:\n    pass\n",
        )?;
        std::fs::write(
            src_dir.join("dataset").join("ops.incn"),
            "from dataset.mod import DataSet\npub def filter_ds[T](ds: DataSet[T]) -> DataSet[T]:\n    return ds\n",
        )?;

        let entry = src_dir.join("lib.incn");
        let entry_str = entry
            .to_str()
            .ok_or("entry path should be valid utf-8 for collect_modules test")?;
        let modules = collect_modules(entry_str)?;

        let dataset_mod = modules
            .iter()
            .find(|module| module.file_path.ends_with(Path::new("dataset").join("mod.incn")))
            .ok_or("expected dataset/mod.incn to be collected")?;
        assert_eq!(dataset_mod.path_segments, vec!["dataset".to_string()]);
        assert_ne!(
            dataset_mod.path_segments,
            vec!["dataset".to_string(), "mod".to_string()]
        );

        let dataset_ops = modules
            .iter()
            .find(|module| module.file_path.ends_with(Path::new("dataset").join("ops.incn")))
            .ok_or("expected dataset/ops.incn to be collected")?;
        assert_eq!(
            dataset_ops.path_segments,
            vec!["dataset".to_string(), "ops".to_string()]
        );

        Ok(())
    }

    #[test]
    fn collect_modules_keeps_migrated_stdlib_sources_out_of_consumer_graphs() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let entry = tmp.path().join("main.incn");
        std::fs::write(
            &entry,
            "from std.environ import get_str\n\ndef main() -> None:\n    get_str(\"HOME\")\n",
        )?;

        let modules = collect_modules(&entry.to_string_lossy())?;
        assert_eq!(
            modules.len(),
            1,
            "migrated stdlib imports must be supplied by the artifact, not source modules"
        );
        assert_eq!(modules[0].path_segments, ["main"]);
        Ok(())
    }

    #[test]
    fn collect_modules_supports_init_directory_entrypoints() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )?;

        let src_dir = project_root.join("src");
        std::fs::create_dir_all(src_dir.join("dataset"))?;
        std::fs::write(src_dir.join("lib.incn"), "from dataset import DataSet\n")?;
        std::fs::write(
            src_dir.join("dataset").join("__init__.incn"),
            "pub trait DataSet[T]:\n    pass\n",
        )?;

        let entry = src_dir.join("lib.incn");
        let entry_str = entry
            .to_str()
            .ok_or("entry path should be valid utf-8 for collect_modules test")?;
        let modules = collect_modules(entry_str)?;

        let dataset_init = modules
            .iter()
            .find(|module| module.file_path.ends_with(Path::new("dataset").join("__init__.incn")))
            .ok_or("expected dataset/__init__.incn to be collected")?;
        assert_eq!(dataset_init.path_segments, vec!["dataset".to_string()]);

        Ok(())
    }

    #[test]
    fn source_requirements_do_not_rediscover_math_provider_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
from std.math import sqrt
"#,
        )?;
        let requirements = collect_project_requirements(&[module], &LibraryManifestIndex::default())?;
        let mut resolved = ResolvedDependencies {
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
        };

        merge_project_requirement_dependencies(&mut resolved, &requirements)?;

        assert!(resolved.dependencies.is_empty());
        Ok(())
    }

    #[test]
    fn source_requirements_do_not_rediscover_io_provider_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
from std.io import BytesIO
"#,
        )?;
        let requirements = collect_project_requirements(&[module], &LibraryManifestIndex::default())?;
        let mut resolved = ResolvedDependencies {
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
        };

        merge_project_requirement_dependencies(&mut resolved, &requirements)?;

        assert!(resolved.dependencies.is_empty());
        Ok(())
    }

    #[test]
    fn collect_modules_skips_unknown_stdlib_source_resolution() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let src_dir = tmp.path().join("src");
        std::fs::create_dir_all(&src_dir)?;
        let entry = src_dir.join("main.incn");
        std::fs::write(&entry, "from std.unknown_module import thing\n")?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        assert_eq!(modules.len(), 1, "unknown std.* imports should not queue source stubs");
        Ok(())
    }

    #[test]
    fn collect_modules_resolves_source_root_for_examples_entrypoints() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        let examples_dir = project_root.join("examples");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&examples_dir)?;

        std::fs::write(
            src_dir.join("dataset.incn"),
            r#"pub trait DataSet[T]:
    pass
"#,
        )?;
        let entry = examples_dir.join("trait_hierarchy.incn");
        std::fs::write(
            &entry,
            r#"from dataset import DataSet

def main() -> None:
    pass
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        assert_eq!(modules.len(), 2, "example entrypoint should pull source-root imports");
        assert!(
            modules.iter().any(|m| m.file_path.ends_with("src/dataset.incn")),
            "expected dataset module to resolve from source root"
        );
        Ok(())
    }

    #[test]
    fn collect_modules_orders_dependencies_before_dependents() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "dep_order_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            src_dir.join("substrait_model.incn"),
            r#"pub model SubstraitPlan:
    rels: list[str]
"#,
        )?;
        std::fs::write(
            src_dir.join("substrait_builder.incn"),
            r#"from substrait_model import SubstraitPlan

pub def plan_from_named_table(name: str) -> SubstraitPlan:
    _ = name
    return SubstraitPlan(rels=[])
"#,
        )?;
        let entry = src_dir.join("lib.incn");
        std::fs::write(
            &entry,
            r#"from substrait_builder import plan_from_named_table
from substrait_model import SubstraitPlan

pub def probe() -> SubstraitPlan:
    return plan_from_named_table(str("orders"))
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let mut model_idx = None;
        let mut builder_idx = None;
        let mut entry_idx = None;
        for (idx, module) in modules.iter().enumerate() {
            if module.file_path.ends_with("src/substrait_model.incn") {
                model_idx = Some(idx);
            } else if module.file_path.ends_with("src/substrait_builder.incn") {
                builder_idx = Some(idx);
            } else if module.file_path.ends_with("src/lib.incn") {
                entry_idx = Some(idx);
            }
        }

        let Some(model_idx) = model_idx else {
            panic!("expected substrait_model module");
        };
        let Some(builder_idx) = builder_idx else {
            panic!("expected substrait_builder module");
        };
        let Some(entry_idx) = entry_idx else {
            panic!("expected entry module");
        };

        assert!(
            model_idx < builder_idx,
            "dependency module must be ordered before dependent module"
        );
        assert!(
            builder_idx < entry_idx,
            "entry module must be ordered after imported modules"
        );
        Ok(())
    }

    #[test]
    fn collect_modules_order_keeps_imported_types_resolved_during_typecheck() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "dep_check_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            src_dir.join("substrait_model.incn"),
            r#"@derive(Clone)
pub model SubstraitRelNode:
    rel_id: str

@derive(Clone)
pub model SubstraitPlan:
    plan_id: str
    root_rel_id: str
    rels: list[SubstraitRelNode]
    profile_tags: list[str]

pub def empty_substrait_plan() -> SubstraitPlan:
    return SubstraitPlan(plan_id=str("p"), root_rel_id=str(""), rels=[], profile_tags=[])
"#,
        )?;
        std::fs::write(
            src_dir.join("substrait_builder.incn"),
            r#"from substrait_model import SubstraitPlan, SubstraitRelNode, empty_substrait_plan

pub def build_one() -> SubstraitPlan:
    plan = empty_substrait_plan()
    mut rels = plan.rels
    rel = SubstraitRelNode(rel_id=str("r1"))
    rels.append(rel)
    return SubstraitPlan(plan_id=plan.plan_id, root_rel_id=rel.rel_id, rels=rels, profile_tags=plan.profile_tags)
"#,
        )?;
        let entry = src_dir.join("lib.incn");
        std::fs::write(
            &entry,
            r#"from substrait_builder import build_one
from substrait_model import SubstraitPlan

pub def probe() -> SubstraitPlan:
    return build_one()
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let module_idx_by_key = module_key_index(&modules);
        for (idx, module) in modules.iter().enumerate() {
            let deps = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
            let mut checker = typechecker::TypeChecker::new();
            if let Err(errs) = checker.check_with_imports(&module.ast, &deps) {
                return Err(format!(
                    "typecheck failed for module {}: {:?}",
                    module.file_path.display(),
                    errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn imported_module_deps_preserve_bare_sibling_class_privacy_issue886() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let src_dir = tmp.path().join("src");
        let pkg_dir = src_dir.join("pkg");
        std::fs::create_dir_all(&pkg_dir)?;
        std::fs::write(
            tmp.path().join("incan.toml"),
            "[project]\nname = \"sibling_private_class\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            pkg_dir.join("vaults.incn"),
            "pub class Vault:\n    secret: str = \"sealed\"\n    pub label: str\n",
        )?;
        let consumer_path = pkg_dir.join("consumer.incn");
        std::fs::write(
            &consumer_path,
            r#"from vaults import Vault

def leak() -> str:
    value = Vault(label="visible")
    return value.secret
"#,
        )?;

        let modules = collect_modules(consumer_path.to_string_lossy().as_ref())?;
        let consumer_index = modules
            .iter()
            .position(|module| module.file_path == consumer_path)
            .ok_or("expected nested consumer module")?;
        let module_idx_by_key = module_key_index(&modules);
        let dependencies = imported_module_deps_for_with_index(&modules, consumer_index, &module_idx_by_key);
        assert!(
            dependencies.iter().any(|(name, _)| *name == "pkg_vaults"),
            "bare sibling imports must retain the canonical nested dependency; modules={:?}, dependencies={:?}",
            modules
                .iter()
                .map(|module| (module.name.clone(), module.path_segments.clone()))
                .collect::<Vec<_>>(),
            dependencies
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect::<Vec<_>>()
        );

        let mut checker = typechecker::TypeChecker::new();
        checker.set_current_module_path(Some(modules[consumer_index].path_segments.clone()));
        let errors = match checker.check_with_imports(&modules[consumer_index].ast, &dependencies) {
            Ok(()) => return Err("private sibling field access must fail typechecking".into()),
            Err(errors) => errors,
        };
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("Field 'secret' on 'Vault' is private")),
            "expected private-field diagnostic, got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        );
        Ok(())
    }

    /// Verifies that absolute from-imports and module imports both contribute local dependency metadata before
    /// typechecking.
    #[test]
    fn imported_module_deps_preserve_absolute_crate_public_type_metadata_issue882()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "absolute_crate_public_types"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            src_dir.join("types.incn"),
            r#"pub enum Access:
    Allowed
    Denied


pub model Decision:
    pub admitted: bool
    pub reason: str
"#,
        )?;
        std::fs::write(
            src_dir.join("consumer.incn"),
            r#"from crate.types import Access, Decision


pub def allowed() -> Access:
    return Access.Allowed


pub def explain(decision: Decision) -> str:
    if decision.admitted:
        return decision.reason
    return "denied"
"#,
        )?;
        std::fs::write(src_dir.join("module_consumer.incn"), "import crate.types\n")?;
        let entry = src_dir.join("lib.incn");
        std::fs::write(
            &entry,
            r#"pub from crate.consumer import allowed, explain
pub from crate.types import Access, Decision
import crate.module_consumer
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let module_idx_by_key = module_key_index(&modules);
        let consumer_idx = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/consumer.incn"))
            .ok_or("expected src/consumer.incn module")?;
        let consumer_deps = imported_module_deps_for_with_index(&modules, consumer_idx, &module_idx_by_key);
        assert!(
            consumer_deps.iter().any(|(name, _)| *name == "types"),
            "expected absolute from-import dependency `consumer -> types`, got: {:?}",
            consumer_deps
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect::<Vec<_>>()
        );

        let module_consumer_idx = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/module_consumer.incn"))
            .ok_or("expected src/module_consumer.incn module")?;
        let module_consumer_deps =
            imported_module_deps_for_with_index(&modules, module_consumer_idx, &module_idx_by_key);
        assert!(
            module_consumer_deps.iter().any(|(name, _)| *name == "types"),
            "expected absolute module-import dependency `module_consumer -> types`, got: {:?}",
            module_consumer_deps
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect::<Vec<_>>()
        );

        for (idx, module) in modules.iter().enumerate() {
            let deps = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
            let mut checker = typechecker::TypeChecker::new();
            if let Err(errs) = checker.check_with_imports(&module.ast, &deps) {
                return Err(format!(
                    "typecheck failed for module {}: {:?}",
                    module.file_path.display(),
                    errs.iter().map(|error| error.message.clone()).collect::<Vec<_>>()
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn imported_module_deps_for_includes_forward_edge_in_cycle() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "cycle_dep_resolver_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("a.incn"),
            r#"from b import pong

pub def ping() -> int:
    return pong()
"#,
        )?;
        std::fs::write(
            src_dir.join("b.incn"),
            r#"from a import ping

pub def pong() -> int:
    return 1
"#,
        )?;
        let entry = src_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"from a import ping

pub def main() -> int:
    return ping()
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let Some(b_index) = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/b.incn"))
        else {
            panic!("expected src/b.incn module");
        };
        let module_idx_by_key = module_key_index(&modules);
        let deps = imported_module_deps_for_with_index(&modules, b_index, &module_idx_by_key);
        assert!(
            deps.iter().any(|(name, _)| *name == "a"),
            "expected cyclic forward dependency `b -> a` to be resolved, got: {:?}",
            deps.iter().map(|(name, _)| (*name).to_string()).collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn imported_module_deps_for_includes_transitive_signature_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "transitive_signature_dep_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("dataset.incn"),
            r#"pub class LazyFrame[T]:
    def clone(self) -> Self:
        return self
"#,
        )?;
        std::fs::write(
            src_dir.join("session.incn"),
            r#"from dataset import LazyFrame

pub class Session:
    def read_csv[T](self) -> Result[LazyFrame[T], str]:
        return Err(str("not implemented"))
"#,
        )?;
        let entry = src_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"from session import Session

def main() -> Result[None, str]:
    session = Session()
    lines = session.read_csv[int]()?
    lines.clone()
    return Ok(None)
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let Some(main_index) = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/main.incn"))
        else {
            return Err("expected src/main.incn module".into());
        };
        let module_idx_by_key = module_key_index(&modules);
        let deps = imported_module_deps_for_with_index(&modules, main_index, &module_idx_by_key);
        assert!(
            deps.iter().any(|(name, _)| *name == "dataset"),
            "expected transitive dependency `dataset` to be included for imported signature resolution, got: {:?}",
            deps.iter().map(|(name, _)| (*name).to_string()).collect::<Vec<_>>()
        );

        let mut checker = typechecker::TypeChecker::new();
        if let Err(errs) = checker.check_with_imports(&modules[main_index].ast, &deps) {
            return Err(format!(
                "typecheck failed: {:?}",
                errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn session_analysis_keeps_crate_root_facade_class_reexports_as_types() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let source_root = project_root.join("src");
        let session_root = source_root.join("session");
        std::fs::create_dir_all(&session_root)?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"crate_root_facade\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(session_root.join("types.incn"), "pub class Session:\n    pub id: int\n")?;
        std::fs::write(
            session_root.join("mod.incn"),
            "pub from crate.session.types import Session\n",
        )?;
        let main_path = source_root.join("main.incn");
        let main_source = "from session import Session\n\ndef main() -> None:\n    session = Session(id=1)\n";
        std::fs::write(&main_path, main_source)?;

        let session = CompilationSession::discover_with_feature_selection(&main_path, &FeatureSelection::default())?;
        let modules = collect_modules_detailed_with_session(main_path.clone(), &session)
            .map_err(|failure| failure.render_human())?;
        let module_idx_by_key = module_key_index(&modules);
        let facade_index = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/session/mod.incn"))
            .ok_or("expected the session facade module")?;
        let facade_dependencies = imported_module_deps_for_with_index(&modules, facade_index, &module_idx_by_key);
        assert!(
            facade_dependencies.iter().any(|(name, _)| *name == "session_types"),
            "crate-root imports must contribute their source dependency to the session analysis closure; got: {:?}",
            facade_dependencies
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect::<Vec<_>>()
        );

        let analysis = session
            .analyze_modules(
                &modules,
                #[cfg(feature = "rust_inspect")]
                None,
            )
            .map_err(|failure| failure.render_human())?;
        let callee_start = main_source.find("Session(id=1)").ok_or("expected constructor call")?;
        assert_eq!(
            analysis
                .type_info_for_path(&main_path)
                .ok_or("expected main session analysis")?
                .ident_kind(Span::new(callee_start, callee_start + "Session".len())),
            Some(IdentKind::TypeName),
            "a public facade re-export of a crate-root class must stay a class constructor in shared session facts"
        );
        Ok(())
    }

    #[test]
    fn dependency_closure_includes_crate_root_module_imports() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let source_root = project_root.join("src");
        let types_root = source_root.join("types");
        std::fs::create_dir_all(&types_root)?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"crate_root_module_import\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(types_root.join("user.incn"), "pub class User:\n    pub id: int\n")?;
        let consumer_path = source_root.join("consumer.incn");
        std::fs::write(
            &consumer_path,
            "import crate.types.user\n\npub def consume() -> None:\n    pass\n",
        )?;
        let main_path = source_root.join("main.incn");
        std::fs::write(
            &main_path,
            "from consumer import consume\n\ndef main() -> None:\n    consume()\n",
        )?;

        let session = CompilationSession::discover_with_feature_selection(&main_path, &FeatureSelection::default())?;
        let modules = collect_modules_detailed_with_session(main_path.clone(), &session)
            .map_err(|failure| failure.render_human())?;
        let module_idx_by_key = module_key_index(&modules);
        let consumer_index = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/consumer.incn"))
            .ok_or("expected the consumer module")?;
        let dependencies = imported_module_deps_for_with_index(&modules, consumer_index, &module_idx_by_key);
        assert!(
            dependencies.iter().any(|(name, _)| *name == "types_user"),
            "crate-root module imports must contribute their source dependency to the closure; got: {:?}",
            dependencies
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn collect_modules_supports_example_entry_with_cyclic_src_interfaces() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "example_cycle_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        let examples_dir = project_root.join("examples");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&examples_dir)?;
        std::fs::write(
            src_dir.join("functions.incn"),
            r#"from dataset import DataFrame, DataSet

pub def display[T](data: DataSet[T]) -> None:
    pass

pub def sink[T](data: DataFrame[T]) -> None:
    pass
"#,
        )?;
        std::fs::write(
            src_dir.join("session.incn"),
            r#"from dataset import DataFrame, LazyFrame

pub model SessionError:
    pub message: str

pub class Session:
    @staticmethod
    def default() -> Session:
        return Session()

    def read_csv[T](self, _logical_name: str, _uri: str) -> Result[LazyFrame[T], SessionError]:
        return Err(SessionError(message=str("not implemented")))

    def activate(self) -> None:
        pass

pub def collect_with_active_session[T](data: LazyFrame[T]) -> Result[DataFrame[T], SessionError]:
    return Err(SessionError(message=str("not implemented")))
"#,
        )?;
        std::fs::write(
            src_dir.join("dataset.incn"),
            r#"from session import SessionError, collect_with_active_session

pub trait DataSet[T]:
    pass

pub class DataFrame[T] with DataSet:
    def clone(self) -> Self:
        return self

pub class LazyFrame[T] with DataSet:
    def clone(self) -> Self:
        return self

    def collect(self) -> Result[DataFrame[T], SessionError]:
        return collect_with_active_session[T](self.clone())
"#,
        )?;
        let entry = examples_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"from functions import display
from session import Session, SessionError

def main() -> Result[None, SessionError]:
    mut session = Session.default()
    lines = session.read_csv[int](str("orders"), str("input.csv"))?
    transformed = lines.clone()
    session.activate()
    df = transformed.clone().collect()?
    display(df)
    return Ok(None)
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let module_idx_by_key = module_key_index(&modules);
        for (idx, module) in modules.iter().enumerate() {
            let deps = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
            let mut checker = typechecker::TypeChecker::new();
            if let Err(errs) = checker.check_with_imports(&module.ast, &deps) {
                return Err(format!(
                    "typecheck failed for module {}: {:?}",
                    module.file_path.display(),
                    errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn collect_modules_supports_directory_module_cycles_from_example_entry() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "example_directory_cycle_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        let dataset_dir = src_dir.join("dataset");
        let examples_dir = project_root.join("examples");
        std::fs::create_dir_all(&dataset_dir)?;
        std::fs::create_dir_all(&examples_dir)?;
        std::fs::write(
            src_dir.join("session.incn"),
            r#"from dataset import DataFrame, LazyFrame

pub model SessionError:
    pub message: str

pub class Session:
    @staticmethod
    def default() -> Session:
        return Session()

    def read_csv[T with Clone](self, _logical_name: str, _uri: str) -> Result[LazyFrame[T], SessionError]:
        return Err(SessionError(message=str("not implemented")))

pub def collect_with_active_session[T with Clone](data: LazyFrame[T]) -> Result[DataFrame[T], SessionError]:
    return Err(SessionError(message=str("not implemented")))
"#,
        )?;
        std::fs::write(
            dataset_dir.join("mod.incn"),
            r#"from session import SessionError, collect_with_active_session

pub trait DataSet[T with Clone]:
    pass

pub class DataFrame[T with Clone] with DataSet:
    def clone(self) -> Self:
        return self

pub class LazyFrame[T with Clone] with DataSet:
    def clone(self) -> Self:
        return self

    def collect(self) -> Result[DataFrame[T], SessionError]:
        return collect_with_active_session[T](self.clone())
"#,
        )?;
        let entry = examples_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"from session import Session, SessionError

@derive(Clone)
pub model OrderLine:
    pub sku: str

def main() -> Result[None, SessionError]:
    session = Session.default()
    lines = session.read_csv[OrderLine](str("orders"), str("input.csv"))?
    df = lines.clone().collect()?
    df.clone()
    return Ok(None)
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let module_idx_by_key = module_key_index(&modules);
        for (idx, module) in modules.iter().enumerate() {
            let deps = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
            let mut checker = typechecker::TypeChecker::new();
            if let Err(errs) = checker.check_with_imports(&module.ast, &deps) {
                return Err(format!(
                    "typecheck failed for module {}: {:?}",
                    module.file_path.display(),
                    errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn collect_modules_cycle_falls_back_to_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "cycle_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            src_dir.join("a.incn"),
            r#"from b import pong

pub def ping() -> int:
    return pong()
"#,
        )?;
        std::fs::write(
            src_dir.join("b.incn"),
            r#"from a import ping

pub def pong() -> int:
    return 1
"#,
        )?;
        let entry = src_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"from a import ping

pub def main() -> int:
    return ping()
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        assert_eq!(modules.len(), 3, "expected all modules to be collected even with cycle");
        assert!(modules[0].file_path.ends_with("src/b.incn"));
        assert!(modules[1].file_path.ends_with("src/a.incn"));
        assert!(modules[2].file_path.ends_with("src/main.incn"));
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_workspace_fingerprint_is_deterministic() {
        let requirements = ProjectRequirements::default();
        let resolved = ResolvedDependencies {
            dependencies: vec![DependencySpec {
                crate_name: "serde".to_string(),
                version: Some("1".to_string()),
                features: vec!["derive".to_string()],
                default_features: true,
                source: DependencySource::Registry,
                optional: false,
                package: None,
            }],
            dev_dependencies: Vec::new(),
        };
        let fp_a = super::rust_inspect_workspace_fingerprint(
            "probe",
            "probe",
            Some("2021"),
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            Some("lock-bytes"),
            None,
            false,
            Path::new("/cache/target"),
        );
        let fp_b = super::rust_inspect_workspace_fingerprint(
            "probe",
            "probe",
            Some("2021"),
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            Some("lock-bytes"),
            None,
            false,
            Path::new("/cache/target"),
        );
        let workspace_fp = super::rust_inspect_workspace_fingerprint(
            "probe",
            "incan_workspace",
            Some("2021"),
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            Some("lock-bytes"),
            None,
            false,
            Path::new("/cache/target"),
        );
        let target_fp = super::rust_inspect_workspace_fingerprint(
            "probe",
            "probe",
            Some("2021"),
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            Some("lock-bytes"),
            None,
            false,
            Path::new("/cache/other-target"),
        );
        assert_eq!(fp_a, fp_b);
        assert_ne!(fp_a, workspace_fp);
        assert_ne!(fp_a, target_fp);
        assert!(fp_a.starts_with(super::RUST_INSPECT_WORKSPACE_FINGERPRINT_PREFIX));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_workspace_fingerprint_changes_when_lock_payload_changes() {
        let requirements = ProjectRequirements::default();
        let resolved = ResolvedDependencies {
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
        };
        let fp_one = super::rust_inspect_workspace_fingerprint(
            "p",
            "p",
            None,
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            Some("lock-a"),
            None,
            false,
            Path::new("/cache/target"),
        );
        let fp_two = super::rust_inspect_workspace_fingerprint(
            "p",
            "p",
            None,
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            Some("lock-b"),
            None,
            false,
            Path::new("/cache/target"),
        );
        assert_ne!(fp_one, fp_two);
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_projection_receives_frozen_cargo_policy() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let requirements = ProjectRequirements::default();
        let resolved = ResolvedDependencies {
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
        };
        let canonical = format!(
            "version = 4\n\n[[package]]\nname = \"incan_workspace\"\nversion = \"{}\"\n",
            crate::version::INCAN_VERSION
        );
        let fingerprint = super::rust_inspect_workspace_fingerprint(
            "policy_probe",
            "caller",
            None,
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            Some(&canonical),
            Some("incan_workspace"),
            false,
            &tmp.path().join("cargo-target"),
        );
        let output_dir = super::rust_inspect_workspace_dir(tmp.path(), "policy_probe", &fingerprint);
        let flags = vec!["--frozen".to_string()];

        let result = ensure_rust_inspect_workspace_with_cargo_package_name(
            tmp.path(),
            "policy_probe",
            "caller",
            None,
            &resolved,
            &requirements,
            Some(canonical),
            Some("incan_workspace"),
            false,
            &tmp.path().join("cargo-target"),
            &flags,
        );
        assert!(
            result.is_err(),
            "the deliberately incomplete canonical fixture must fail closed"
        );
        assert_eq!(
            crate::backend::project::runner::test_projection_cargo_policy(&output_dir),
            Some(flags),
            "rust-inspect must set frozen policy before attempting Cargo lock projection"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_fingerprint_tracks_same_path_projection_rebuild_issue911() -> Result<(), Box<dyn std::error::Error>>
    {
        let workspace = tempfile::tempdir()?;
        let artifact = workspace.path().join("compiled");
        fs::create_dir_all(artifact.join("src"))?;
        fs::write(
            artifact.join("Cargo.toml"),
            "[package]\nname = \"issue911_compiled\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(artifact.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n")?;
        let requirements = ProjectRequirements {
            sdk_artifact_projections: vec![SdkArtifactProjection {
                artifact: LibraryArtifactMetadata::from_crate_root("issue911_compiled", "issue911_compiled", &artifact),
            }],
            ..ProjectRequirements::default()
        };
        let resolved = ResolvedDependencies {
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
        };
        let before = super::rust_inspect_workspace_fingerprint(
            "probe",
            "probe",
            None,
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            None,
            None,
            false,
            &workspace.path().join("cargo-target"),
        );
        fs::write(artifact.join("src/lib.rs"), "pub fn value() -> u8 { 2 }\n")?;
        let after = super::rust_inspect_workspace_fingerprint(
            "probe",
            "probe",
            None,
            &resolved,
            &requirements.stdlib_features,
            &requirements.sdk_dependency_rebindings,
            &requirements.sdk_path_dependencies,
            &requirements.sdk_artifact_projections,
            None,
            None,
            false,
            &workspace.path().join("cargo-target"),
        );

        assert_ne!(before, after);
        Ok(())
    }

    #[test]
    fn helper_requirements_keep_unused_active_sdk_path_targets_issue911() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let artifact = workspace.path().join("unused-sdk-provider");
        let record = crate::provider::ProviderRecord {
            identity: crate::provider::ProviderIdentity {
                name: "incan_issue911_unused_sdk".to_string(),
                version: "0.5.0".to_string(),
                digest: "sha256:issue911-unused".to_string(),
                feature_projection: BTreeSet::new(),
            },
            provenance: crate::provider::ProviderProvenance::Sdk {
                sdk_identity: "incan@0.5.0".to_string(),
                component_id: "issue911-unused".to_string(),
                inventory_path: None,
            },
            authority: crate::provider::NamespaceAuthority::SdkReserved,
            namespace_claims: BTreeSet::from([vec!["std".to_string(), "issue911_unused".to_string()]]),
            available: true,
            enabled: true,
            manifest: Some(Arc::new(LibraryManifest::new("incan_issue911_unused_sdk", "0.5.0"))),
            artifact: Some(LibraryArtifactMetadata::from_crate_root(
                "incan_issue911_unused_sdk",
                "incan_issue911_unused_sdk",
                &artifact,
            )),
            implementation_facets: Vec::new(),
        };
        let plan = ProviderPlan::new(
            crate::frontend::library_manifest_index::LibraryManifestIndex::default(),
            vec![record.clone()],
            std::iter::empty(),
        )?;
        assert!(
            plan.sdk_link_roots().is_empty(),
            "unused SDK provider must not become a direct link root"
        );

        let mut requirements = ProjectRequirements::default();
        extend_requirements_with_provider_plan(&mut requirements, &plan)?;

        assert!(
            requirements.dependencies.is_empty(),
            "unused provider must not be linked directly"
        );
        assert_eq!(requirements.sdk_path_dependencies.len(), 1);
        assert!(matches!(
            &requirements.sdk_path_dependencies[0].source,
            DependencySource::Path { path } if path == &artifact
        ));

        let used_plan = ProviderPlan::new(
            crate::frontend::library_manifest_index::LibraryManifestIndex::default(),
            vec![record],
            [vec!["std".to_string(), "issue911_unused".to_string()]],
        )?;
        let mut used_requirements = ProjectRequirements::default();
        extend_requirements_with_provider_plan(&mut used_requirements, &used_plan)?;
        assert_eq!(used_requirements.dependencies.len(), 1);
        assert!(
            !used_requirements.dependencies[0].default_features,
            "new direct SDK edges must render explicit default-features = false"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_helper_materializes_sdk_projection_issue911() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let artifact = workspace.path().join("compiled");
        let absent_sdk = workspace.path().join("sdk-cache-a/runtime");
        let active_sdk = workspace.path().join("sdk-cache-b/runtime");
        for root in [&artifact, &active_sdk] {
            fs::create_dir_all(root.join("src"))?;
        }
        fs::write(
            artifact.join("Cargo.toml"),
            format!(
                "[package]\nname = \"issue911_compiled\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies.issue911_runtime]\npath = {:?}\ndefault-features = false\n",
                absent_sdk.to_string_lossy()
            ),
        )?;
        fs::write(
            artifact.join("src/lib.rs"),
            "pub fn value() -> u8 { issue911_runtime::value() }\n",
        )?;
        fs::write(
            active_sdk.join("Cargo.toml"),
            "[package]\nname = \"issue911_runtime\"\nversion = \"0.5.0\"\nedition = \"2024\"\n\n[workspace]\n",
        )?;
        fs::write(active_sdk.join("src/lib.rs"), "pub fn value() -> u8 { 3 }\n")?;
        let mut manifest = LibraryManifest::new("issue911_compiled", "0.1.0");
        manifest.contract_metadata.provider.provider_dependencies.push(
            crate::library_manifest::ProviderDependencyMetadata {
                kind: crate::library_manifest::ProviderDependencyKind::PrivateImplementation,
                dependency_key: "issue911_runtime".to_string(),
                provider_name: "issue911_runtime".to_string(),
                provider_version: "0.5.0".to_string(),
                artifact_digest: digest_provider_artifact(&active_sdk)?,
                relative_artifact_path: "../sdk-cache-a/runtime".to_string(),
                requested_features: BTreeSet::new(),
                default_features: false,
                optional: false,
            },
        );
        let manifest_path = artifact.join("issue911_compiled.incnlib");
        manifest.write_to_path(&manifest_path)?;
        let metadata = LibraryArtifactMetadata::from_manifest_path(
            "issue911_compiled",
            "issue911_compiled",
            manifest_path,
            artifact.clone(),
        );
        let requirements = ProjectRequirements {
            sdk_dependency_rebindings: vec![SdkDependencyRebinding {
                containing_artifact: metadata.clone(),
                source_crate_root: absent_sdk.clone(),
                provider_name: "issue911_runtime".to_string(),
                dependency_key: "issue911_runtime".to_string(),
                active_crate_root: active_sdk,
            }],
            sdk_artifact_projections: vec![SdkArtifactProjection { artifact: metadata }],
            ..ProjectRequirements::default()
        };
        let resolved = ResolvedDependencies {
            dependencies: vec![DependencySpec {
                crate_name: "issue911_compiled".to_string(),
                version: None,
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Path { path: artifact },
                optional: false,
                package: None,
            }],
            dev_dependencies: Vec::new(),
        };

        let generated = ensure_rust_inspect_workspace_with_cargo_package_name(
            workspace.path(),
            "issue911_probe",
            "issue911_probe",
            Some("2024".to_string()),
            &resolved,
            &requirements,
            None,
            None,
            false,
            &workspace.path().join("cargo-target"),
            &[],
        )?;

        let cargo_manifest = fs::read_to_string(generated.join("Cargo.toml"))?;
        assert!(cargo_manifest.contains(".incan-sdk-rebound"));
        assert!(!cargo_manifest.contains(absent_sdk.to_string_lossy().as_ref()));
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let output = Command::new(cargo)
            .arg("check")
            .arg("--offline")
            .arg("--manifest-path")
            .arg(generated.join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", workspace.path().join("cargo-target"))
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "rust-inspect projected Cargo graph failed:\n{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let projection_parent = generated
            .parent()
            .ok_or("rust-inspect workspace has no parent")?
            .join(".incan-sdk-rebound");
        let shadow_root = fs::read_dir(&projection_parent)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.is_dir())
            .ok_or("missing rust-inspect projected artifact")?;
        fs::write(shadow_root.join("src/lib.rs"), "pub fn corrupt() {}\n")?;
        let regenerated = ensure_rust_inspect_workspace_with_cargo_package_name(
            workspace.path(),
            "issue911_probe",
            "issue911_probe",
            Some("2024".to_string()),
            &resolved,
            &requirements,
            None,
            None,
            false,
            &workspace.path().join("cargo-target"),
            &[],
        )?;
        assert_eq!(generated, regenerated);
        assert!(fs::read_to_string(shadow_root.join("src/lib.rs"))?.contains("value"));
        assert!(!absent_sdk.exists());
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_workspace_dir_is_namespaced_by_input_fingerprint() {
        let root = Path::new("/workspace");
        let first = super::rust_inspect_workspace_dir(root, "demo", "v1:aaaaaaaaaaaaaaaaaaaaaaaa");
        let second = super::rust_inspect_workspace_dir(root, "demo", "v1:bbbbbbbbbbbbbbbbbbbbbbbb");

        assert_ne!(first, second);
        assert!(first.ends_with(Path::new("target/incan_lock/rust_inspect/demo-aaaaaaaaaaaaaaaa")));
        assert!(second.ends_with(Path::new("target/incan_lock/rust_inspect/demo-bbbbbbbbbbbbbbbb")));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_out_dirs_fingerprint_tracks_query_surface() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let manifest_dir = tmp.path();
        fs::create_dir_all(manifest_dir.join("src"))?;
        fs::write(
            manifest_dir.join("Cargo.toml"),
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(manifest_dir.join("src").join("main.rs"), "fn main() {}\n")?;
        let target_dir = manifest_dir.join("target");

        let one = super::rust_inspect_out_dirs_fingerprint(manifest_dir, &target_dir, &["demo::One".to_string()])?;
        let two = super::rust_inspect_out_dirs_fingerprint(manifest_dir, &target_dir, &["demo::Two".to_string()])?;

        assert_ne!(
            one, two,
            "rust-inspect out-dir prewarm must rerun when the inspected ABI query surface changes"
        );
        assert!(one.starts_with(super::RUST_INSPECT_OUT_DIRS_FINGERPRINT_FILE));
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_locked_prewarm_detects_stale_generated_lockfile() {
        let cannot_update = "error: cannot update the lock file /tmp/target/incan_lock/rust_inspect/demo/Cargo.lock because --locked was passed to prevent this";
        assert!(super::rust_inspect_locked_prewarm_needs_lock_update(cannot_update));

        let needs_update = "error: the lock file /tmp/target/incan_lock/rust_inspect/demo/Cargo.lock needs to be updated but --locked was passed to prevent this";
        assert!(super::rust_inspect_locked_prewarm_needs_lock_update(needs_update));

        assert!(!super::rust_inspect_locked_prewarm_needs_lock_update(
            "error: failed to select a version for `demo`"
        ));
        assert!(!super::rust_inspect_locked_prewarm_needs_lock_update(
            "error: package selected but no lock file policy was involved"
        ));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn ensure_rust_inspect_workspace_uses_rust_safe_dependency_keys() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let requirements = ProjectRequirements::default();
        let resolved = ResolvedDependencies {
            dependencies: vec![DependencySpec {
                crate_name: "datafusion-substrait".to_string(),
                version: Some("53".to_string()),
                features: vec!["protoc".to_string()],
                default_features: true,
                source: DependencySource::Registry,
                optional: false,
                package: None,
            }],
            dev_dependencies: Vec::new(),
        };

        let out_dir = ensure_rust_inspect_workspace(
            tmp.path(),
            "metadata_probe",
            Some("2021".to_string()),
            &resolved,
            &requirements,
            Some("[[package]]\nname = \"metadata_probe\"\n".to_string()),
            &tmp.path().join("cargo-target"),
            &[],
        )?;
        assert_eq!(
            super::test_rust_inspect_workspace_generations(&out_dir),
            1,
            "expected one rust-inspect workspace generation"
        );

        let cargo_toml = fs::read_to_string(out_dir.join("Cargo.toml"))?;
        let cargo_lock = fs::read_to_string(out_dir.join("Cargo.lock"))?;
        let main_rs = fs::read_to_string(out_dir.join("src").join("main.rs"))?;

        assert!(
            cargo_toml.contains("[dependencies.datafusion_substrait]"),
            "expected rust-safe dependency key in generated rust-inspect workspace, got:\n{cargo_toml}"
        );
        assert!(
            cargo_toml.contains("package = \"datafusion-substrait\""),
            "expected original package name preserved in generated rust-inspect workspace, got:\n{cargo_toml}"
        );
        assert!(
            cargo_lock.contains("metadata_probe"),
            "expected rust-inspect workspace to write the provided Cargo.lock payload"
        );
        assert!(
            main_rs.contains("use datafusion_substrait as _;"),
            "expected rust-inspect workspace stub to reference the aliased dependency crate, got:\n{main_rs}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn ensure_rust_inspect_workspace_skips_regeneration_when_unchanged() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let requirements = ProjectRequirements::default();
        let resolved = ResolvedDependencies {
            dependencies: vec![DependencySpec {
                crate_name: "serde".to_string(),
                version: Some("1".to_string()),
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Registry,
                optional: false,
                package: None,
            }],
            dev_dependencies: Vec::new(),
        };
        let lock = Some("[[package]]\nname = \"skip_probe\"\n".to_string());

        let out_dir = ensure_rust_inspect_workspace(
            tmp.path(),
            "skip_probe",
            Some("2021".to_string()),
            &resolved,
            &requirements,
            lock.clone(),
            &tmp.path().join("cargo-target"),
            &[],
        )?;
        assert_eq!(
            super::test_rust_inspect_workspace_generations(&out_dir),
            1,
            "first call should generate the workspace"
        );

        ensure_rust_inspect_workspace(
            tmp.path(),
            "skip_probe",
            Some("2021".to_string()),
            &resolved,
            &requirements,
            lock,
            &tmp.path().join("cargo-target"),
            &[],
        )?;
        assert_eq!(
            super::test_rust_inspect_workspace_generations(&out_dir),
            1,
            "second call with identical inputs should skip regeneration"
        );

        Ok(())
    }

    #[test]
    fn typecheck_modules_with_import_graph_accepts_valid_program() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
def main() -> None:
    pass
"#,
        )?;

        typecheck_modules_with_import_graph(
            &[module],
            None,
            &Arc::new(ProviderPlan::default()),
            #[cfg(feature = "rust_inspect")]
            None,
        )?;

        Ok(())
    }

    #[test]
    fn typecheck_modules_with_import_graph_reports_errors() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
def main() -> None:
    missing_symbol()
"#,
        )?;

        let result = typecheck_modules_with_import_graph(
            &[module],
            None,
            &Arc::new(ProviderPlan::default()),
            #[cfg(feature = "rust_inspect")]
            None,
        );
        assert!(result.is_err(), "expected unresolved symbol to fail typecheck");

        Ok(())
    }

    #[test]
    fn compilation_session_projects_declared_package_features() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let source_root = tmp.path().join("src");
        std::fs::create_dir_all(&source_root)?;
        std::fs::write(
            tmp.path().join("incan.toml"),
            "[project]\nname = \"feature_projection\"\n\n[project.features]\ndefault = [\"json\"]\njson = []\n",
        )?;
        let source_path = source_root.join("main.incn");
        let source = "when feature(\"json\"):\n    const JSON_ENABLED = true\n\nconst ALWAYS = true\n";
        std::fs::write(&source_path, source)?;

        let default_session =
            CompilationSession::discover_with_feature_selection(&source_path, &FeatureSelection::default())?;
        let default_program = default_session
            .parse_source(&source_path, source, false)
            .map_err(|errors| std::io::Error::other(format!("default feature parse failed: {errors:?}")))?;
        assert_eq!(default_program.declarations.len(), 2);

        let selection = FeatureSelection {
            no_default_features: true,
            ..FeatureSelection::default()
        };
        let minimal_session = CompilationSession::discover_with_feature_selection(&source_path, &selection)?;
        let tooling_program = minimal_session
            .parse_source_unprojected(&source_path, source, false)
            .map_err(|errors| std::io::Error::other(format!("tooling feature parse failed: {errors:?}")))?;
        assert_eq!(
            tooling_program.declarations.len(),
            2,
            "tooling must retain inactive declarations before semantic projection"
        );
        let minimal_program = minimal_session
            .parse_source(&source_path, source, false)
            .map_err(|errors| std::io::Error::other(format!("minimal feature parse failed: {errors:?}")))?;
        assert_eq!(minimal_program.declarations.len(), 1);

        let unknown_source = "when feature(\"missing\"):\n    const VALUE = true\n";
        let errors = minimal_session
            .parse_source(&source_path, unknown_source, false)
            .err()
            .ok_or("unknown feature should fail source projection")?;
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("Unknown package feature `missing`"))
        );
        Ok(())
    }

    #[test]
    fn artifact_only_dependency_rejects_a_stale_feature_projection() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dependency_root = tmp.path().join("feature_library");
        let artifact_root = dependency_root.join("target/lib");
        fs::create_dir_all(artifact_root.join("src"))?;
        fs::write(
            artifact_root.join("Cargo.toml"),
            "[package]\nname = \"feature_library\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(artifact_root.join("src/lib.rs"), "pub fn alpha() {}\n")?;
        let mut artifact = LibraryManifest::new("feature_library", "0.1.0");
        artifact.contract_metadata.provider.public_features = BTreeMap::from([
            ("alpha".to_string(), ProviderFeatureMetadata::default()),
            ("beta".to_string(), ProviderFeatureMetadata::default()),
        ]);
        artifact.contract_metadata.provider.active_features = BTreeSet::from(["alpha".to_string()]);
        artifact.write_to_path(&artifact_root.join("feature_library.incnlib"))?;

        let consumer_root = tmp.path().join("consumer");
        fs::create_dir_all(&consumer_root)?;
        fs::write(
            consumer_root.join(MANIFEST_FILENAME),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nfeature_library = { path = \"../feature_library\", features = [\"beta\"], default-features = false }\n",
        )?;
        let consumer = ProjectManifest::discover(&consumer_root)?.ok_or("missing consumer manifest")?;
        let error = PackageFeaturePlan::resolve(&consumer, &FeatureSelection::default())
            .err()
            .ok_or("stale artifact-only feature projection should fail")?;
        let message = error.to_string();

        assert!(message.contains("was built with package features [alpha]"));
        assert!(message.contains("requires [beta]"));
        Ok(())
    }

    #[test]
    fn compilation_session_analysis_bundles_lowering_inputs_with_semantic_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let source_root = project_root.join("src");
        std::fs::create_dir_all(&source_root)?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"analysis_consumer\"\n",
        )?;
        let main_path = source_root.join("main.incn");
        std::fs::write(
            &main_path,
            "from std.testing import assert_eq\n\ndef helper() -> int:\n  return 1\n\ndef main() -> int:\n  assert_eq(helper(), 1)\n  return helper()\n",
        )?;

        let session = CompilationSession::discover_with_feature_selection(&main_path, &FeatureSelection::default())?;
        let modules = collect_modules_detailed_with_session(main_path.clone(), &session)
            .map_err(|failure| failure.render_human())?;
        let analysis = session
            .analyze_modules(
                &modules,
                #[cfg(feature = "rust_inspect")]
                None,
            )
            .map_err(|failure| failure.render_human())?;
        let snapshot = analysis
            .semantic_snapshots()
            .get(&main_path)
            .ok_or("expected a session semantic snapshot for the entry module")?;

        assert!(analysis.type_info_for_path(&main_path).is_some());
        assert!(snapshot.render_snapshot().contains("decl:main::helper type=() -> int"));
        assert!(
            snapshot
                .render_snapshot()
                .contains("symbol_target=function:main::helper")
        );
        let mut stdlib_cache = analysis.stdlib_cache().clone();
        assert!(
            stdlib_cache
                .lookup_function_symbol(&["std".to_string(), "testing".to_string()], "assert_eq")
                .is_some(),
            "session analysis must retain source-backed stdlib metadata for lowering"
        );
        Ok(())
    }

    #[test]
    fn compilation_session_analysis_preserves_same_file_module_identities() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let source_root = tmp.path().join("src");
        std::fs::create_dir_all(&source_root)?;
        std::fs::write(
            tmp.path().join("incan.toml"),
            "[project]\nname = \"identity_consumer\"\n",
        )?;
        let shared_path = source_root.join("shared.incn");
        std::fs::write(&shared_path, "def value() -> int:\n  return 1\n")?;

        let mut first = parsed_module_for_test("def first() -> int:\n  return 1\n")?;
        first.name = "first".to_string();
        first.path_segments = vec!["first".to_string()];
        first.file_path = shared_path.clone();
        let mut second = parsed_module_for_test("def second() -> int:\n  return 2\n")?;
        second.name = "second".to_string();
        second.path_segments = vec!["second".to_string()];
        second.file_path = shared_path.clone();

        let analysis = CompilationSession::discover_with_feature_selection(&shared_path, &FeatureSelection::default())?
            .analyze_modules(
                &[first, second],
                #[cfg(feature = "rust_inspect")]
                None,
            )
            .map_err(|failure| failure.render_human())?;

        assert!(analysis.type_info_for_module_path(&["first".to_string()]).is_some());
        assert!(analysis.type_info_for_module_path(&["second".to_string()]).is_some());
        Ok(())
    }
}
