//! Hidden `legacy_cargo` Loaf baker for Oven Alpha compatibility preparation.
//!
//! This is deliberately not an execution backend. It may be invoked only through the named `legacy_cargo` command
//! while direct closure materialization is being completed for #1005/#975. It bakes typed `.loaf/` envelopes and
//! publishes receipt-bound compiler-suite plans into the bounded Oven store, then removes every private Cargo target
//! before returning. Normal Oven build, run, and test code neither calls this module nor receives a Cargo target path.

mod cargo_json;
pub mod cargo_process;
pub mod loaf_bake;

// Cargo's own JSON shapes live beside this file rather than inside it. They are deserialization targets with no
// publisher behavior, and every path stays where callers expect it through this re-export, so this is a move.
use cargo_json::*;

mod compiler_suite_catalog;
mod compiler_suite_targets;
mod inspection_sources;
mod lock;
mod registry_sources;
mod rustc_trace;
mod sdk_staging;
mod selected_graph_projection;
mod selected_unit_capture;
mod workspace_authority;

pub use compiler_suite_catalog::*;
pub use compiler_suite_targets::*;
pub use inspection_sources::*;
pub use lock::*;
pub use registry_sources::*;
pub use rustc_trace::*;
pub use sdk_staging::*;
pub use selected_graph_projection::*;
pub use selected_unit_capture::*;
pub use workspace_authority::*;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use oven_model::compiler_identity::CompilerIdentity;

use oven_store::OvenProviderHooks;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// The wire contract the native route reads is declared in `oven_rustc::native_contract`; this crate produces
// it and re-exports it so callers keep one spelling for the baker and its output.
pub use oven_rustc::native_contract::{
    OVEN_COMPILER_TEST_SUITE_FOUNDATION_SCHEMA_VERSION, OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION,
    OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION, OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION_V1,
    OVEN_COMPILER_TEST_SUITE_TOOLCHAIN_DATA_SCHEMA_VERSION, OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION,
    OVEN_PROVIDER_COMPILATION_KEY, OvenCompilerTestSuiteArtifactClosure, OvenCompilerTestSuiteFoundationPayload,
    OvenCompilerTestSuiteFoundationReference, OvenCompilerTestSuitePayload, OvenCompilerTestSuiteShardPayload,
    OvenCompilerTestSuiteShardReference, OvenCompilerTestSuiteTarget, OvenCompilerTestSuiteTargetKey,
    OvenCompilerTestSuiteToolchainDataPayload, OvenCompilerTestSuiteToolchainDataReference,
    OvenCompilerTestSuiteToolchainLoafGenerationReference, OvenCompilerWorkspaceLibrary,
    OvenCompilerWorkspaceLibraryKey, OvenLegacyCargoInspectionPackage, OvenLegacyCargoInspectionSource,
    OvenLegacyCargoInspectionSourceMember, OvenProjectExtensionPayload, OvenProjectRegistrySourceDependency,
};

use serde::Serialize;

use oven_rustc::rustc::{
    OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH, OvenRustcArtifactExtern,
    OvenRustcArtifactManifest, OvenRustcRegistryLeaf, OvenRustcRegistrySource, OvenRustcRegistrySourcePackage,
    OvenRustcSupportingArtifact, clear_inherited_cargo_environment, rerooted_artifact_staging_source,
    rustc_host_and_target_cfg_snapshots, rustc_host_target, rustc_identity, select_direct_rustc_plan_identity,
    validate_project_extension_payload_against_base,
};
use oven_store::process::{isolate_process_group, terminate_process_group};
use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedDirectory, OvenArtifactMaterializedFile, OvenArtifactPublishRequest,
    OvenStore, OvenStoreError,
};
use oven_store::{DEFAULT_OVEN_PUBLISHER_STAGING_FLOOR_BYTES, digest_bytes, digest_source_tree};
use oven_store::{
    OVEN_COMPILER_TEST_PROFILE, OvenBuildIntent, OvenCompatibilityKind, OvenReceipt, compiler_suite_source_evidence_key,
};

/// Wire format retained as an immutable supporting artifact alongside every `legacy_cargo`-prepared closure.
pub const OVEN_LEGACY_CARGO_PROVENANCE_SCHEMA_VERSION: u32 = 2;
/// Reserve payload and manifest headroom when splitting a closure by the logical domain policy.
///
/// The publisher still asks the store to make the authoritative admission decision. This small deterministic margin
/// lets a foundation describe its selected artifacts without accidentally placing a near-limit group over policy.
const COMPILER_TEST_SUITE_FOUNDATION_METADATA_HEADROOM_BYTES: u64 = 64 * 1024;
/// Minimum idle interval between full private-staging capacity scans while the explicit publisher is compiling.
///
/// A scan has to walk Cargo's whole transient tree in order to preserve physical-byte accounting and catch output
/// outside the immediate target subdirectory. Polling it every 25 ms made that guard compete with large dependency
/// compiles and turn one cold publish into repeated multi-gigabyte tree walks. A quarter-second floor keeps small
/// overrun fixtures prompt; longer scans must yield for at least their own duration so the guard cannot monopolize
/// the publisher after the private tree reaches gigabytes.
pub const PUBLISHER_CAPACITY_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Return the idle time after one complete capacity scan.
///
/// The physical-byte policy must inspect every file under private staging because a child can write outside its
/// declared Cargo target. The monitor therefore yields for the scan duration when a large tree makes that scan more
/// expensive than the ordinary prompt-poll floor. This bounds the monitor to at most half of one CPU rather than
/// beginning the next full walk immediately after the preceding one.
pub fn publisher_capacity_probe_delay(scan_elapsed: Duration) -> Duration {
    PUBLISHER_CAPACITY_POLL_INTERVAL.max(scan_elapsed)
}
/// Cargo-profile definition mirrored by every publisher-owned compatibility project that receives the suite receipt.
///
/// The direct-Rustc consumer applies the same contract independently. Keeping this as a single manifest fragment
/// prevents a small publisher helper from accepting an `oven-test` receipt without defining its profile.
const OVEN_COMPILER_TEST_CARGO_PROFILE_MANIFEST: &str =
    "\n[profile.oven-test]\ninherits = \"dev\"\ndebug = 0\nincremental = false\n";

/// The one explicit publisher action authorized for an immutable direct-rustc plan.
///
/// The distinction is recorded at publication time because a library-test closure contains dev-dependencies and is
/// not interchangeable with an executable closure even when Cargo.toml is shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OvenLegacyCargoPublicationKind {
    /// Build one generated executable/library root without test-only dependencies.
    Executable,
    /// Build only a generated executable's publisher-only companion library before native interop is sealed.
    ///
    /// This preserves ordinary executable Cargo topology: only the explicit interop bootstrap emits and selects the
    /// companion `src/main.rs` library target, so Cargo never links a package-owned native library before Oven has
    /// sealed that library into the final direct-rustc plan.
    InteropBootstrap,
    /// Build one library's libtest inputs with Cargo only at this explicit publisher boundary.
    LibraryTests,
}

/// The direct Rust dependency surface sealed by one explicit publisher transaction.
///
/// Ordinary generated projects retain only dependencies their generated Rust source can name. A compiler-owned
/// standard-library Loaf instead retains every dependency declared by its checked fixture manifest: generated
/// standard-library modules are compiled alongside a consuming project and may name a facade's upstream Rust crate
/// even when the small fixture root did not itself exercise that facade. This remains a bounded, checked manifest
/// closure; it is never a scan of an ambient Cargo target directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OvenLegacyCargoDirectDependencyClosure {
    /// Seal only generated-source roots and documented compiler macro-expansion roots.
    GeneratedSource,
    /// Seal every direct dependency declared by the checked publisher manifest.
    CheckedDeclared,
}

/// A compiler-owned macro dependency required by an already checked provider compilation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenCompilerMacroDependency {
    /// Dependency alias retained by the provider's generated manifest.
    pub alias: String,
    /// Declared package name, before the publisher binds its locked package ID.
    pub package: String,
    /// Canonical compiler-owned source root authenticated by the provider and consumer runtime facts.
    #[serde(skip)]
    pub source_root: PathBuf,
    /// Checked macro source content identity, independent of its installed location.
    pub source_digest: String,
    /// Checked compiler-core source used by the macro implementation.
    pub core_source_digest: String,
    /// Checked runtime lock content that owns the macro's private dependency closure.
    pub runtime_lock_digest: String,
}

/// Hash only macro dependency content; provider bodies, publication labels and physical roots do not enter reuse.
pub fn provider_compilation_requirements_digest(
    requirements: &[OvenCompilerMacroDependency],
) -> Result<String, OvenLegacyCargoError> {
    let records = requirements
        .iter()
        .map(serde_json::to_vec)
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|error| OvenLegacyCargoError::Plan(format!("cannot encode provider macro requirements: {error}")))?;
    serde_json::to_vec(&records)
        .map(|bytes| digest_bytes(&bytes))
        .map_err(|error| OvenLegacyCargoError::Plan(format!("cannot encode provider macro requirement set: {error}")))
}

/// Require the publication receipt to own the exact content-bound provider request before warm selection or effects.
fn validate_provider_compilation_requirements(
    receipt: &OvenReceipt,
    requirements: &[OvenCompilerMacroDependency],
) -> Result<(), OvenLegacyCargoError> {
    if requirements.is_empty() {
        return Ok(());
    }
    let digest = provider_compilation_requirements_digest(requirements)?;
    if receipt
        .sources
        .build_unit_inputs
        .get("provider-compilation-requirements")
        != Some(&digest)
    {
        return Err(OvenLegacyCargoError::ReceiptMismatch {
            message: "provider compilation requirements differ from the publication receipt".to_string(),
        });
    }
    Ok(())
}

/// Preserve consumer roots and expose the checked macro set only to provider compilations.
fn provider_compilation_externs(
    requirements: &[OvenCompilerMacroDependency],
    consumer: &BTreeMap<String, String>,
    all_dependencies: &BTreeMap<String, String>,
    consumer_key: &str,
) -> Result<BTreeMap<String, Vec<String>>, OvenLegacyCargoError> {
    if requirements.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut provider_names = consumer.keys().cloned().collect::<BTreeSet<_>>();
    for dependency in requirements {
        if all_dependencies.get(&dependency.alias) != Some(&dependency.package) {
            return Err(OvenLegacyCargoError::Plan(format!(
                "provider compilation has no selected macro {}",
                dependency.alias
            )));
        }
        provider_names.insert(dependency.alias.clone());
    }
    Ok(BTreeMap::from([
        (consumer_key.to_string(), consumer.keys().cloned().collect()),
        (
            OVEN_PROVIDER_COMPILATION_KEY.to_string(),
            provider_names.into_iter().collect(),
        ),
    ]))
}

/// Bind required compiler macros to the exact declared package and actual publisher-reported host artifacts.
fn validate_provider_macro_artifacts(
    providers: &[OvenCompilerMacroDependency],
    metadata: &CargoMetadata,
    resolved: &BTreeMap<String, ResolvedDirectDependency>,
    outputs: &[CargoInvocationOutput],
    staging: &Path,
    externs: &[OvenRustcArtifactExtern],
    base: Option<&OvenLegacyCargoBaseLoaf<'_>>,
) -> Result<(), OvenLegacyCargoError> {
    for dependency in providers {
        let selected = resolved.get(&dependency.alias).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!("required provider macro {} is unresolved", dependency.alias))
        })?;
        let package = metadata
            .packages
            .iter()
            .find(|package| package.id == selected.package_id)
            .ok_or_else(|| {
                OvenLegacyCargoError::Plan("selected macro package is absent from publisher metadata".to_string())
            })?;
        let package_root = package
            .manifest_path
            .parent()
            .ok_or_else(|| OvenLegacyCargoError::Plan("selected macro manifest has no source root".to_string()))?;
        if package.name != dependency.package
            || canonical_directory(package_root, "selected macro source root")? != dependency.source_root
        {
            return Err(OvenLegacyCargoError::Plan(format!(
                "provider macro {} does not resolve to its checked compiler-owned source",
                dependency.alias
            )));
        }
        let named = externs
            .iter()
            .find(|artifact| artifact.crate_name == dependency.alias)
            .ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "required provider macro {} has no named artifact",
                    dependency.alias
                ))
            })?;
        if base.is_some_and(|base| base.artifacts.externs.iter().any(|artifact| artifact == named)) {
            // The existing base admission already owns this exact named artifact and its compatibility closure.
            // Never replace it with a separately emitted package copy merely to obtain a new report.
            continue;
        }
        let source_relative =
            rerooted_artifact_staging_source(&named.relative_path).unwrap_or_else(|| named.relative_path.clone());
        let actual_path = verified_regular_file(&staging.join(source_relative), "selected provider macro")?;
        let mut matched = false;
        for output in outputs {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let Ok(artifact) = serde_json::from_str::<CargoCompilerArtifact>(line) else {
                    continue;
                };
                if artifact.reason != "compiler-artifact"
                    || artifact.package_id != selected.package_id
                    || !artifact.target.kind.iter().any(|kind| kind == "proc-macro")
                {
                    continue;
                }
                for filename in artifact.filenames {
                    if verified_regular_file(&filename, "reported provider macro")? == actual_path {
                        matched = true;
                    }
                }
            }
        }
        if !matched {
            return Err(OvenLegacyCargoError::Plan(format!(
                "required provider macro {} has no matching reported proc-macro artifact",
                dependency.alias
            )));
        }
    }
    Ok(())
}

/// Explicit input to the hidden `legacy_cargo` publisher.
pub struct OvenLegacyCargoPrepareRequest<'a> {
    /// The compiler this publication runs under; its identity is sealed into the compatibility inputs.
    pub compiler: CompilerIdentity,
    /// The provider facts the publisher asks the compiler for while staging the SDK.
    pub provider_hooks: Arc<dyn OvenProviderHooks>,
    /// Bounded Oven store that will own the immutable result.
    pub store: &'a OvenStore,
    /// Generated-project receipt that authorizes the generated Rust root and direct-rustc intent.
    pub receipt: OvenReceipt,
    /// Caller-owned generated Rust project containing `Cargo.toml` and `src/main.rs`.
    pub generated_project: PathBuf,
    /// Explicit Cargo executable. Normal Oven commands never discover or invoke this tool.
    pub cargo: PathBuf,
    /// Explicit Rust compiler used by Cargo and later direct-rustc execution.
    pub rustc: PathBuf,
    /// Exact prebuilt SDK inventory supplied by the Loaf baker for compiler-suite publication.
    ///
    /// Standalone transitional callers may omit this and use the installed-toolchain discovery contract. Normal
    /// consumer commands never construct this publisher request.
    pub sdk_inventory: Option<PathBuf>,
    /// Explicit committed compiler Loaf envelope required by a compiler-suite publication.
    ///
    /// The named baker supplies this root after it atomically publishes the release-family envelope.  Requiring the
    /// exact root prevents the suite publisher from silently selecting a similarly shaped Loaf tree beside another
    /// compiler executable.
    pub compiler_loaf_root: Option<PathBuf>,
    /// Stable compatibility-domain policy bucket for the stored closure.
    pub domain: String,
    /// Explicit publisher operation; normal Oven consumers never receive this authority.
    pub publication_kind: OvenLegacyCargoPublicationKind,
    /// Named receipt source digest that authorizes the root later passed to direct rustc.
    pub source_evidence_key: String,
    /// Deterministic compile-time metadata required by the authorized root after ambient Cargo state is cleared.
    pub compile_environment: BTreeMap<String, String>,
    /// Exact checked Rust-inspection surface to seal into this plan.
    ///
    /// `Some` limits a narrow Loaf to its checked fixture surface, including an empty surface. `None` retains every
    /// registry rlib actually emitted into a broad compiler-suite foundation closure. A caller project partitioned
    /// against a base Loaf additionally seals its complete locked registry-source graph; neither form invents
    /// artifacts or resolves anything beyond this explicit publisher invocation. Normal Oven consumers never
    /// construct this request.
    pub inspection_packages: Option<Vec<OvenLegacyCargoInspectionPackage>>,
    /// Compiler-owned direct dependency closure to retain from the checked generated-project manifest.
    ///
    /// This controls the Rustc `--extern` surface only at the named publisher boundary. Normal consumers merely
    /// select the already sealed plan and never inspect a generated Cargo manifest or target directory.
    pub direct_dependency_closure: OvenLegacyCargoDirectDependencyClosure,
    /// Checked provider compilation requirements whose native artifacts this publisher must expose privately.
    pub provider_compilations: &'a [OvenCompilerMacroDependency],
    /// Whether the named Loaf publisher may omit debug information from a debug-profile dependency closure.
    ///
    /// This affects only private `legacy_cargo` publisher artifacts; direct-rustc receipt identity and normal command
    /// semantics remain unchanged. It prevents compiler-shipped sealed Loaf data from consuming policy capacity with
    /// linker-irrelevant debug sections.
    pub compact_debug_info: bool,
    /// Whether this explicit source-built project publication must seal the compiler-owned vocabulary helper.
    ///
    /// This is admitted only by the source-built compiler's explicit Oven bake. The normal build, run, and test
    /// paths select the helper from the already sealed plan and never receive Cargo authority.
    pub source_compiler_vocab_support: bool,
    /// Optional immutable standard-library base selected before an explicit project bake.
    ///
    /// When present, the publisher substitutes the exact release-owned dependency cohort—compiler runtime,
    /// overlapping locked registry units, and vocabulary auxiliaries—then retains only the project's locked
    /// third-party and provider delta. The caller holds the base Loaf generation lock for the transaction; this
    /// request carries the immutable identity and plan evidence needed to partition those responsibilities.
    pub base_loaf: Option<OvenLegacyCargoBaseLoaf<'a>>,
}

/// One exact compiler-shipped Loaf selected as the base for a project-extension publication.
pub struct OvenLegacyCargoBaseLoaf<'a> {
    /// Content address of the selected `loaf.json`, not a crate or filesystem name.
    pub loaf_identity: String,
    /// Compatibility identity that authorized the selected base for this receipt.
    pub build_unit_identity: String,
    /// Verified complete direct-Rustc closure retained by the immutable base.
    pub artifacts: &'a OvenRustcArtifactManifest,
    /// Immutable directory containing every digest-verified base artifact.
    ///
    /// The publisher reads the sealed registry lock from this root before Cargo resolves a project extension. The
    /// root is request-local authority retained under the caller's generation lock; it is never serialized into the
    /// project payload.
    pub artifact_root: &'a Path,
}

/// Outcome from a successful explicit `legacy_cargo` publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenLegacyCargoPrepareResult {
    /// Identity of the immutable receipt-bound Oven Loaf published by this transaction.
    pub plan_identity: String,
    /// Cargo version observed only at the explicit publisher boundary.
    pub cargo_version: String,
    /// Digest of the generated `Cargo.toml` used by the publisher.
    pub cargo_manifest_digest: String,
    /// Digest of the generated `Cargo.lock` written or verified by the publisher.
    pub cargo_lock_digest: String,
    /// Exact registry package artifacts observed by this named publisher invocation.
    ///
    /// The Loaf exporter seals this small catalog beside the copied direct-Rustc closure. It is never a
    /// normal-command Cargo resolution result.
    pub registry_leaves: Vec<OvenRustcRegistryLeaf>,
    /// Physical Cargo-selected units captured at this publisher boundary; absent only for exact store reuse.
    pub selected_units: Option<OvenLegacyCargoSelectedUnitCapture>,
    /// Conservative transient publisher allocation high-water mark; this directory is removed before success returns.
    pub transient_reservation_bytes: u64,
    /// Inactive store entries evicted, oldest first, so this bake could reserve its staging floor (#1230). Empty when
    /// the store already had room; never names an entry that was under a live lease.
    #[serde(default)]
    pub reclaimed_store_entries: Vec<String>,
}

/// Publisher-private foundation payload and its exact staged files, ready for separately bounded admission.
struct OvenCompilerTestSuiteFoundationPlan {
    payload: OvenCompilerTestSuiteFoundationPayload,
    materialized_files: Vec<OvenArtifactMaterializedFile>,
}

/// Publisher-private Loaf partition and its exact staged files, ready for separately bounded admission.
#[cfg(test)]
struct OvenCompilerTestSuiteToolchainDataPlan {
    materialized_files: Vec<OvenArtifactMaterializedFile>,
}

/// Split one publisher-verified compiler closure into deterministic foundation entries.
///
/// A foundation owns every byte it declares. Root shards retain the complete logical dependency declaration, but
/// receive only these exact foundations at execution time; they never recover a Cargo target or a copied composite
/// directory. Partitions retain their parent suite's one compatibility domain, so the store refuses the complete
/// closure when its aggregate exceeds the configured allowance rather than treating each label as a separate cache.
fn compiler_suite_foundation_plans(
    closure: &OvenCompilerTestSuiteArtifactClosure,
    materialized_files: &[OvenArtifactMaterializedFile],
    max_domain_logical_bytes: u64,
) -> Result<Vec<OvenCompilerTestSuiteFoundationPlan>, OvenLegacyCargoError> {
    let content_limit = max_domain_logical_bytes
        .checked_sub(COMPILER_TEST_SUITE_FOUNDATION_METADATA_HEADROOM_BYTES)
        .ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler foundation logical allowance {max_domain_logical_bytes} leaves no payload metadata headroom"
            ))
        })?;
    let mut files_by_relative_path = BTreeMap::new();
    for file in materialized_files {
        let metadata = fs::symlink_metadata(&file.source_path).map_err(|source| OvenLegacyCargoError::Io {
            path: file.source_path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite foundation artifact",
                message: format!("{} must be a regular non-symlink file", file.source_path.display()),
            });
        }
        if files_by_relative_path
            .insert(file.relative_path.clone(), (file.clone(), metadata.len()))
            .is_some()
        {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite foundation artifact",
                message: format!("duplicates materialized path `{}`", file.relative_path),
            });
        }
    }

    let mut foundations = Vec::new();
    let mut current_artifacts = Vec::new();
    let mut current_files = Vec::new();
    let mut current_bytes = 0_u64;
    let mut used_paths = BTreeSet::new();
    for artifact in &closure.supporting_artifacts {
        let (file, logical_bytes) = files_by_relative_path.get(&artifact.relative_path).ok_or_else(|| {
            OvenLegacyCargoError::MissingDirectArtifact {
                crate_name: "compiler-suite foundation closure".to_string(),
                path: PathBuf::from(&artifact.relative_path),
            }
        })?;
        if !used_paths.insert(artifact.relative_path.clone()) {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite foundation closure",
                message: format!("declares duplicate artifact `{}`", artifact.relative_path),
            });
        }
        if *logical_bytes > content_limit {
            return Err(OvenLegacyCargoError::Plan(format!(
                "compiler foundation artifact `{}` is {} bytes, exceeding the {}-byte logical content allowance",
                artifact.relative_path, logical_bytes, content_limit
            )));
        }
        if !current_artifacts.is_empty() && current_bytes.saturating_add(*logical_bytes) > content_limit {
            foundations.push(OvenCompilerTestSuiteFoundationPlan {
                payload: OvenCompilerTestSuiteFoundationPayload {
                    schema_version: OVEN_COMPILER_TEST_SUITE_FOUNDATION_SCHEMA_VERSION,
                    label: format!("foundation-{:04}", foundations.len()),
                    artifact_closure: compiler_suite_foundation_closure(
                        closure,
                        std::mem::take(&mut current_artifacts),
                    ),
                },
                materialized_files: std::mem::take(&mut current_files),
            });
            current_bytes = 0;
        }
        current_bytes = current_bytes.saturating_add(*logical_bytes);
        current_artifacts.push(artifact.clone());
        current_files.push(file.clone());
    }
    if current_artifacts.is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "compiler foundation closure has no supporting artifacts to partition".to_string(),
        ));
    }
    if used_paths.len() != files_by_relative_path.len() {
        let unassigned = files_by_relative_path
            .keys()
            .filter(|path| !used_paths.contains(*path))
            .cloned()
            .collect::<Vec<_>>();
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite foundation closure",
            message: format!("does not declare materialized artifact(s): {}", unassigned.join(", ")),
        });
    }
    foundations.push(OvenCompilerTestSuiteFoundationPlan {
        payload: OvenCompilerTestSuiteFoundationPayload {
            schema_version: OVEN_COMPILER_TEST_SUITE_FOUNDATION_SCHEMA_VERSION,
            label: format!("foundation-{:04}", foundations.len()),
            artifact_closure: compiler_suite_foundation_closure(closure, current_artifacts),
        },
        materialized_files: current_files,
    });
    Ok(foundations)
}

/// Split installed compiler-Loaf directories into deterministic schema-13 suite inputs.
///
/// This reader remains only to execute already-published schema-13 suite entries. New schema-14-and-later entries
/// record one exact, lease-held compiler Loaf generation instead of copying these directories into the receipt-bound
/// store.
#[cfg(test)]
fn compiler_suite_toolchain_data_plans(
    data_root: &Path,
    max_domain_logical_bytes: u64,
    expected_runtime_inputs: &BTreeMap<String, String>,
) -> Result<Vec<OvenCompilerTestSuiteToolchainDataPlan>, OvenLegacyCargoError> {
    compiler_suite_toolchain_data_plans_from_loaf_root(
        &data_root.join("share/incan/oven/loafs"),
        max_domain_logical_bytes,
        expected_runtime_inputs,
    )
}

/// Split one explicit committed compiler Loaf envelope into deterministic suite inputs.
#[cfg(test)]
fn compiler_suite_toolchain_data_plans_from_loaf_root(
    loafs: &Path,
    max_domain_logical_bytes: u64,
    expected_runtime_inputs: &BTreeMap<String, String>,
) -> Result<Vec<OvenCompilerTestSuiteToolchainDataPlan>, OvenLegacyCargoError> {
    let content_limit = max_domain_logical_bytes
        .checked_sub(COMPILER_TEST_SUITE_FOUNDATION_METADATA_HEADROOM_BYTES)
        .ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler Loaf logical allowance {max_domain_logical_bytes} leaves no payload metadata headroom"
            ))
        })?;
    if !loafs.is_dir() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler Loaf data",
            message: format!("{} is not a directory", loafs.display()),
        });
    }
    let committed = oven_rustc::loaf::acquire_committed_loaf_generation(loafs)
        .map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "compiler Loaf data",
            message: error.to_string(),
        })?
        .ok_or_else(|| OvenLegacyCargoError::Plan("compiler Loaf data has no committed envelope".to_string()))?;
    let mut loaf_groups = Vec::new();
    for loaf_manifest in committed.paths() {
        let loaf_directory = loaf_manifest
            .parent()
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: format!("{} has no parent directory", loaf_manifest.display()),
            })?
            .to_path_buf();
        let loaf_name = loaf_directory
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: format!("{} has a non-UTF-8 Loaf directory name", loaf_directory.display()),
            })?
            .to_string();
        let metadata = fs::symlink_metadata(&loaf_directory).map_err(|source| OvenLegacyCargoError::Io {
            path: loaf_directory.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: format!("{} must be a non-symlink Loaf directory", loaf_directory.display()),
            });
        }
        let loaf_metadata = fs::symlink_metadata(loaf_manifest).map_err(|source| OvenLegacyCargoError::Io {
            path: loaf_manifest.clone(),
            source,
        })?;
        if loaf_metadata.file_type().is_symlink() || !loaf_metadata.is_file() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: format!("{} must contain a regular loaf.json", loaf_directory.display()),
            });
        }
        let loaf = serde_json::from_slice::<oven_rustc::loaf::OvenLoaf>(&regular_file_bytes(loaf_manifest)?).map_err(
            |error| OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: format!("{} is not a valid sealed Loaf: {error}", loaf_manifest.display()),
            },
        )?;
        oven_rustc::loaf::validate_stored_loaf(loaf_manifest, &loaf.build_unit_identity).map_err(|error| {
            OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: error.to_string(),
            }
        })?;
        validate_compiler_suite_loaf_runtime_inputs(&loaf_name, &loaf, expected_runtime_inputs)?;
        let relative_directory =
            loaf_directory
                .strip_prefix(loafs)
                .map_err(|_| OvenLegacyCargoError::InvalidInput {
                    field: "compiler Loaf data",
                    message: format!("{} escapes compiler data root", loaf_directory.display()),
                })?;
        let relative_root = Path::new("share/incan/oven/loafs")
            .join(relative_directory)
            .to_string_lossy()
            .to_string();
        let files = materialized_files_from_directory(&loaf_directory, &relative_root, "compiler-owned Loaf data")?;
        let logical_bytes = files.iter().try_fold(0_u64, |total, file| {
            let metadata = fs::symlink_metadata(&file.source_path).map_err(|source| OvenLegacyCargoError::Io {
                path: file.source_path.clone(),
                source,
            })?;
            Ok::<_, OvenLegacyCargoError>(total.saturating_add(metadata.len()))
        })?;
        if logical_bytes > content_limit {
            return Err(OvenLegacyCargoError::Plan(format!(
                "compiler Loaf `{loaf_name}` is {logical_bytes} bytes, exceeding the {content_limit}-byte logical content allowance"
            )));
        }
        loaf_groups.push((files, logical_bytes));
    }
    if loaf_groups.is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "compiler Loaf data has no sealed .loaf directories to partition".to_string(),
        ));
    }

    let control_files = [loafs.join("envelope.json"), loafs.join(".envelope.lock")]
        .into_iter()
        .map(|source_path| {
            let relative = source_path
                .strip_prefix(loafs)
                .map_err(|_| OvenLegacyCargoError::InvalidInput {
                    field: "compiler Loaf data",
                    message: format!("{} escapes compiler data root", source_path.display()),
                })?;
            let relative_path = Path::new("share/incan/oven/loafs")
                .join(relative)
                .to_string_lossy()
                .to_string();
            Ok(OvenArtifactMaterializedFile {
                source_path,
                relative_path,
            })
        })
        .collect::<Result<Vec<_>, OvenLegacyCargoError>>()?;
    loaf_groups[0].0.extend(control_files);

    let mut plans = Vec::new();
    let mut current_files = Vec::new();
    let mut current_bytes = 0_u64;
    for (files, logical_bytes) in loaf_groups {
        if !current_files.is_empty() && current_bytes.saturating_add(logical_bytes) > content_limit {
            plans.push(OvenCompilerTestSuiteToolchainDataPlan {
                materialized_files: std::mem::take(&mut current_files),
            });
            current_bytes = 0;
        }
        current_bytes = current_bytes.saturating_add(logical_bytes);
        current_files.extend(files);
    }
    plans.push(OvenCompilerTestSuiteToolchainDataPlan {
        materialized_files: current_files,
    });
    Ok(plans)
}

/// Validate the compiler-owned standard-library generation that a schema-14-or-later suite will lease at execution
/// time.
///
/// The suite index records this immutable generation identity instead of republishing its 1+ GiB contents into the
/// receipt-bound store.  The same checks that protected schema-13 copied partitions run here before the index is
/// committed, and the runner repeats generation selection while retaining the shared envelope lock.
fn compiler_suite_toolchain_loaf_generation_reference(
    loaf_root: &Path,
    expected_runtime_inputs: &BTreeMap<String, String>,
) -> Result<OvenCompilerTestSuiteToolchainLoafGenerationReference, OvenLegacyCargoError> {
    let committed = oven_rustc::loaf::acquire_committed_loaf_generation(loaf_root)
        .map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "compiler Loaf data",
            message: error.to_string(),
        })?
        .ok_or_else(|| OvenLegacyCargoError::Plan("compiler Loaf data has no committed envelope".to_string()))?;
    if committed.paths().is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "compiler Loaf data has no sealed .loaf directories".to_string(),
        ));
    }
    for loaf_manifest in committed.paths() {
        let loaf = serde_json::from_slice::<oven_rustc::loaf::OvenLoaf>(&regular_file_bytes(loaf_manifest)?).map_err(
            |error| OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: format!("{} is not a valid sealed Loaf: {error}", loaf_manifest.display()),
            },
        )?;
        oven_rustc::loaf::validate_stored_loaf(loaf_manifest, &loaf.build_unit_identity).map_err(|error| {
            OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: error.to_string(),
            }
        })?;
        let loaf_name = loaf_manifest
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: format!("{} has a non-UTF-8 Loaf directory name", loaf_manifest.display()),
            })?;
        validate_compiler_suite_loaf_runtime_inputs(loaf_name, &loaf, expected_runtime_inputs)?;
    }
    Ok(OvenCompilerTestSuiteToolchainLoafGenerationReference {
        generation_identity: committed.generation_identity().to_string(),
    })
}

/// Derive the Loaf compatibility inputs from the runtime closure sealed in this suite's SDK inventory.
///
/// The compiler-suite publisher stages that runtime closure as a self-contained immutable input. Its Loafs must
/// describe the same lockfile and compiler-runtime source trees; accepting a nearby toolchain's Loaf would make child
/// selection depend on ambient state and later fail closed only after the suite was admitted.
fn compiler_suite_staged_runtime_inputs(
    staged_sdk_root: &Path,
    compiler: &CompilerIdentity,
) -> Result<BTreeMap<String, String>, OvenLegacyCargoError> {
    let runtime_root = staged_sdk_root.join("runtime");
    let runtime_lock = runtime_root.join("Cargo.lock");
    let mut inputs = BTreeMap::new();
    inputs.insert("compiler-version".to_string(), compiler.version.clone());
    inputs.insert(
        "sdk-provider-codegen-revision".to_string(),
        compiler.sdk_provider_codegen_revision.to_string(),
    );
    // One receipt input per compiler-owned runtime crate, so a change to any facet's source re-keys the suite.
    for crate_name in oven_model::toolchain_layout::SDK_RUNTIME_CRATES {
        let input_name = format!("runtime-source-{}", crate_name.replace('_', "-"));
        let source_root = runtime_root.join("crates").join(crate_name);
        let digest = oven_rustc::loaf::digest_runtime_crate_source(&source_root).map_err(|message| {
            OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite SDK runtime closure",
                message,
            }
        })?;
        inputs.insert(input_name, digest);
    }
    let lock_bytes = regular_file_bytes(&runtime_lock)?;
    inputs.insert("runtime-lock".to_string(), digest_bytes(&lock_bytes));
    Ok(inputs)
}

/// Refuse Loaf data from a different compiler runtime than the staged SDK inventory.
///
/// Compatibility is intentionally exact here. Provider modules can be an explicitly authorized Loaf superset, but
/// a runtime source or lockfile mismatch would combine two compiler/package worlds and cannot be repaired by runtime
/// selection. The named baker must regenerate the Loaf with the sealed SDK inventory instead.
fn validate_compiler_suite_loaf_runtime_inputs(
    loaf_name: &str,
    loaf: &oven_rustc::loaf::OvenLoaf,
    expected_runtime_inputs: &BTreeMap<String, String>,
) -> Result<(), OvenLegacyCargoError> {
    if loaf.compatibility.runtime_inputs == *expected_runtime_inputs {
        return Ok(());
    }
    let mismatched = expected_runtime_inputs
        .iter()
        .filter_map(|(key, expected)| {
            let actual = loaf.compatibility.runtime_inputs.get(key);
            (actual != Some(expected)).then(|| {
                format!(
                    "{key}: expected {expected}, found {}",
                    actual.map_or("<missing>", String::as_str)
                )
            })
        })
        .collect::<Vec<_>>();
    let unexpected = loaf
        .compatibility
        .runtime_inputs
        .keys()
        .filter(|key| !expected_runtime_inputs.contains_key(*key))
        .map(|key| format!("{key}: unexpected"))
        .collect::<Vec<_>>();
    let details = mismatched.into_iter().chain(unexpected).collect::<Vec<_>>().join(", ");
    Err(OvenLegacyCargoError::Plan(format!(
        "compiler Loaf `{loaf_name}` is incompatible with the staged SDK runtime closure ({details}); regenerate it through the internal compatibility publisher with the same SDK inventory"
    )))
}

/// Restrict a foundation's direct-rustc search directories to paths that actually contain its selected files.
///
/// A composed runner passes every foundation root separately to Rustc. Retaining an empty sibling `deps` directory
/// would make the strict trusted materializer accept an undeclared directory rather than a real dependency input.
fn compiler_suite_foundation_closure(
    closure: &OvenCompilerTestSuiteArtifactClosure,
    supporting_artifacts: Vec<OvenRustcSupportingArtifact>,
) -> OvenCompilerTestSuiteArtifactClosure {
    let contains_artifact = |directory: &str| {
        let prefix = format!("{}/", directory.trim_end_matches('/'));
        supporting_artifacts
            .iter()
            .any(|artifact| artifact.relative_path.starts_with(&prefix))
    };
    OvenCompilerTestSuiteArtifactClosure {
        dependency_search_paths: closure
            .dependency_search_paths
            .iter()
            .filter(|directory| contains_artifact(directory))
            .cloned()
            .collect(),
        native_search_paths: closure
            .native_search_paths
            .iter()
            .filter(|directory| contains_artifact(directory))
            .cloned()
            .collect(),
        supporting_artifacts,
    }
}

/// Successful explicit publication of a compiler libtest runtime pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenLegacyCargoCompilerSuiteResult {
    /// Identity of the bounded, immutable compiler-suite runtime artifact.
    pub suite_identity: String,
    /// Cargo version observed only at the explicit publisher boundary.
    pub cargo_version: String,
    /// Digest of the compiler Cargo.toml observed by the publisher.
    pub cargo_manifest_digest: String,
    /// Digest of the compiler Cargo.lock observed by the publisher.
    pub cargo_lock_digest: String,
    /// Conservative transient publisher allocation high-water mark; the Cargo target is removed before success
    /// returns.
    pub transient_reservation_bytes: u64,
    /// Product-owned phase timing for the compiler-suite publisher.
    pub timing: OvenLegacyCargoCompilerSuiteTiming,
}

/// Attribution for the explicit compiler-suite publisher after its enclosing Loaf family is available.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct OvenLegacyCargoCompilerSuiteTiming {
    /// Receipt validation, private staging, sealed SDK materialization, and toolchain-data planning.
    pub preflight_and_sdk_elapsed_ms: u128,
    /// Locked Cargo unit-graph discovery only; Cargo does not compile roots in this phase.
    pub unit_graph_elapsed_ms: u128,
    /// The one permitted third-party foundation compilation.
    pub foundation_build_elapsed_ms: u128,
    /// Direct-Rustc root planning, foundation partitioning, and immutable request construction.
    pub direct_plan_elapsed_ms: u128,
    /// Capacity admission and atomic store publication of the complete suite closure.
    pub store_publication_elapsed_ms: u128,
}

/// Failure while preparing a bounded Oven closure through the temporary explicit Cargo boundary.
#[derive(Debug, thiserror::Error)]
pub enum OvenLegacyCargoError {
    /// The caller supplied an invalid publisher input.
    #[error("invalid Oven internal compatibility publisher {field}: {message}")]
    InvalidInput { field: &'static str, message: String },
    /// A receipt does not authorize the requested generated source.
    #[error("Oven internal compatibility publisher receipt mismatch: {message}")]
    ReceiptMismatch { message: String },
    /// Filesystem access failed at an explicit publisher path.
    #[error("Oven internal compatibility publisher I/O failed at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    /// The compiler's provider hooks could not stage the SDK providers the publisher needs.
    #[error("Oven internal compatibility publisher provider hook failed: {0}")]
    ProviderHook(#[from] oven_store::OvenProviderHookError),
    /// Cargo failed while the explicitly named publisher was running.
    #[error("Oven internal compatibility publisher failed: {output}")]
    CargoFailed { output: String },
    /// Temporary publisher storage reached its enforced compatibility-domain allowance.
    #[error(
        "Oven internal compatibility publisher physical reservation at {path} reached {observed_physical_bytes} bytes, exceeding the transient compatibility allowance of {limit_bytes} bytes"
    )]
    TransientCapacityExceeded {
        /// Publisher-owned target or prepared-staging path measured without following links.
        path: PathBuf,
        /// Conservative physical reservation observed when the publisher failed closed.
        observed_physical_bytes: u64,
        /// Enforced compatibility-domain physical allowance.
        limit_bytes: u64,
    },
    /// An expected compiled direct dependency was not available in Cargo's fresh target.
    #[error("Oven internal compatibility publisher could not materialize direct dependency `{crate_name}` from {path}")]
    MissingDirectArtifact { crate_name: String, path: PathBuf },
    /// The bounded Oven store rejected or could not publish the immutable result.
    #[error("Oven internal compatibility publisher store failure: {0}")]
    Store(#[from] OvenStoreError),
    /// A direct-rustc plan could not be serialized or validated.
    #[error("Oven internal compatibility publisher direct-rustc plan failure: {0}")]
    Plan(String),
}

/// Select an already published plan through the exact rule for this request's publication shape.
fn select_existing_direct_rustc_plan_identity(
    request: &OvenLegacyCargoPrepareRequest<'_>,
) -> Result<Option<String>, OvenLegacyCargoError> {
    match request.base_loaf.as_ref() {
        Some(base) => select_existing_project_extension_identity(request.store, &request.receipt, base),
        None => match select_direct_rustc_plan_identity(request.store, &request.receipt) {
            Ok(plan_identity) => {
                if request.source_compiler_vocab_support
                    && !stored_plan_supplies_source_compiler_vocab_support(
                        request.store,
                        &plan_identity,
                        &request.receipt.intent,
                    )?
                {
                    return Ok(None);
                }
                Ok(Some(plan_identity))
            }
            Err(oven_rustc::rustc::OvenRustcError::PlanSelection { message, .. })
                if message == "no compatible stored direct-rustc plan is available" =>
            {
                Ok(None)
            }
            Err(error) => Err(OvenLegacyCargoError::Plan(error.to_string())),
        },
    }
}

/// Return whether a reusable direct plan already seals the host vocabulary closure required by this source build.
///
/// The base plan selector intentionally shares a build-unit identity across generated source revisions. A
/// pre-patch source-built plan may therefore remain otherwise receipt-compatible while lacking the compiler-owned
/// helper. Only the explicit publisher uses this capability check; normal consumers select an already prepared plan
/// and never use it to acquire Cargo authority.
fn stored_plan_supplies_source_compiler_vocab_support(
    store: &OvenStore,
    plan_identity: &str,
    intent: &OvenBuildIntent,
) -> Result<bool, OvenLegacyCargoError> {
    let (manifest, _artifact_root, payload, _lease) = store.select_payload_for_execution(plan_identity)?;
    if manifest.kind != OvenArtifactKind::DirectRustcPlan || manifest.intent != *intent {
        return Ok(false);
    }
    let plan = serde_json::from_slice::<OvenRustcArtifactManifest>(&payload).map_err(|error| {
        OvenLegacyCargoError::Plan(format!(
            "stored direct-rustc plan `{plan_identity}` has an invalid payload while checking vocabulary support: {error}"
        ))
    })?;
    Ok(plan.vocab_auxiliary_targets.iter().any(|target| {
        target.target == intent.target
            && ["incan_vocab", "serde_json"]
                .into_iter()
                .all(|crate_name| target.externs.iter().any(|artifact| artifact.crate_name == crate_name))
    }))
}

/// Return the observable no-publisher result for one exact existing plan.
fn reused_direct_rustc_plan_result(plan_identity: String) -> OvenLegacyCargoPrepareResult {
    OvenLegacyCargoPrepareResult {
        plan_identity,
        cargo_version: "not-run-existing-plan".to_string(),
        cargo_manifest_digest: "not-run-existing-plan".to_string(),
        cargo_lock_digest: "not-run-existing-plan".to_string(),
        registry_leaves: Vec::new(),
        selected_units: None,
        transient_reservation_bytes: 0,
        reclaimed_store_entries: Vec::new(),
    }
}

/// Return the compiler checkout whose checked workspace lock owns a source-built vocabulary helper bake.
///
/// The checkout root is compiled into the executable, so this does not accept a caller-selected source directory.
/// A packaged compiler does not carry the checked workspace layout and must instead select a release-cohort Loaf.
fn source_compiler_vocab_support_root() -> Result<PathBuf, OvenLegacyCargoError> {
    let root = canonical_directory(
        &oven_model::toolchain_layout::development_root(),
        "compiler source root",
    )?;
    if !source_compiler_vocab_support_is_available() {
        return Err(OvenLegacyCargoError::Plan(format!(
            "source-built compiler vocabulary support requires the checked compiler workspace at {}; no release-cohort Loaf is available",
            root.display()
        )));
    }
    Ok(root)
}

/// Return whether this executable was built from the checked compiler workspace that owns `incan_vocab`.
///
/// This is a compiler-origin capability rather than caller input. It distinguishes a source-built compiler, which
/// may seal its checked helper closure at the explicit publisher boundary, from a packaged compiler that must rely
/// on a shipped release-cohort Loaf.
pub fn source_compiler_vocab_support_is_available() -> bool {
    let Ok(root) = fs::canonicalize(oven_model::toolchain_layout::development_root()) else {
        return false;
    };
    let Ok(executable) = std::env::current_exe().and_then(fs::canonicalize) else {
        return false;
    };
    source_compiler_vocab_support_paths_are_available(&root, &executable)
}

/// Return whether `executable` is a source-build binary beneath the checked compiler workspace at `root`.
fn source_compiler_vocab_support_paths_are_available(root: &Path, executable: &Path) -> bool {
    root.join("Cargo.lock").is_file()
        && oven_model::toolchain_layout::support_crate_dir_in(root, "incan_vocab")
            .join("Cargo.toml")
            .is_file()
        && executable.starts_with(root.join("target"))
}

/// Prepare and publish exactly one receipt-bound direct-rustc closure through the hidden `legacy_cargo` boundary.
///
/// Publication is idempotent: a plain direct plan may reuse the compatible generated-project unit it was published
/// for, while a base-partitioned project extension must match its exact receipt and base. A broad compiler Loaf can
/// share a generated build-unit identity with a caller project without owning that project's complete dependency
/// closure. If multiple valid candidates match the applicable rule, the publisher refuses to guess.
pub fn prepare_direct_rustc_plan(
    request: &OvenLegacyCargoPrepareRequest<'_>,
) -> Result<OvenLegacyCargoPrepareResult, OvenLegacyCargoError> {
    request
        .receipt
        .verify_identity()
        .map_err(|error| OvenLegacyCargoError::ReceiptMismatch {
            message: error.to_string(),
        })?;
    validate_provider_compilation_requirements(&request.receipt, request.provider_compilations)?;
    let supported_compatibility = matches!(
        request.receipt.compatibility.kind,
        OvenCompatibilityKind::GeneratedIncanProject | OvenCompatibilityKind::NativeCompilerTestSuite
    );
    if !supported_compatibility
        || (request.receipt.compatibility.cargo_input_only
            && request.receipt.compatibility.kind != OvenCompatibilityKind::NativeCompilerTestSuite)
    {
        return Err(OvenLegacyCargoError::ReceiptMismatch {
            message: "compatibility publication requires a generated-project or native compiler-suite receipt"
                .to_string(),
        });
    }
    let library_tests = request.publication_kind == OvenLegacyCargoPublicationKind::LibraryTests;
    if (request.receipt.compatibility.kind == OvenCompatibilityKind::NativeCompilerTestSuite) != library_tests {
        return Err(OvenLegacyCargoError::ReceiptMismatch {
            message: "native compiler-suite receipts require the explicit library-test publisher; generated-project receipts require an executable or interop-bootstrap publisher".to_string(),
        });
    }
    let rustc_identity = rustc_identity(&request.rustc).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "rustc",
        message: error.to_string(),
    })?;
    if rustc_identity != request.receipt.intent.toolchain {
        return Err(OvenLegacyCargoError::ReceiptMismatch {
            message: format!(
                "receipt requires Rust compiler `{}`, but --rustc reports `{rustc_identity}`",
                request.receipt.intent.toolchain
            ),
        });
    }
    if let Some(plan_identity) = select_existing_direct_rustc_plan_identity(request)? {
        return Ok(reused_direct_rustc_plan_result(plan_identity));
    }

    let generated_project = canonical_directory(&request.generated_project, "generated project")?;
    let cargo_manifest = generated_project.join("Cargo.toml");
    let cargo_manifest_bytes = regular_file_bytes(&cargo_manifest)?;
    let staged_metadata = if let Some(base) = request.base_loaf.as_ref() {
        let base_lock = verified_release_cohort_registry_lock(base)?;
        Some(stage_release_cohort_project_lock(
            &request.cargo,
            &generated_project,
            &base_lock,
            &request.receipt.intent.features,
        )?)
    } else {
        None
    };
    let _ =
        receipt_authorized_generated_root_bytes(&generated_project, &request.receipt, &request.source_evidence_key)?;
    let cargo_version = tool_version(&request.cargo, "cargo")?;
    let declared_direct_dependencies = cargo_direct_dependency_names(
        &cargo_manifest_bytes,
        request.publication_kind == OvenLegacyCargoPublicationKind::LibraryTests,
    )?;
    let mut direct_dependencies = publisher_direct_dependencies(
        &generated_project,
        declared_direct_dependencies.clone(),
        request.publication_kind,
        request.direct_dependency_closure,
    )?;
    let consumer_direct_dependencies = direct_dependencies.clone();
    for dependency in request.provider_compilations {
        if declared_direct_dependencies.get(&dependency.alias) != Some(&dependency.package) {
            return Err(OvenLegacyCargoError::Plan(format!(
                "provider compilation requires undeclared compiler macro {} from package {}",
                dependency.alias, dependency.package,
            )));
        }
        direct_dependencies.insert(dependency.alias.clone(), dependency.package.clone());
    }
    let staging_parent = request.store.root().join("legacy-cargo-staging");
    let publisher_lock = acquire_publisher_lock(&staging_parent)?;
    // The fast lookup above deliberately precedes manifest inspection. Recheck after taking the cross-process
    // publisher lock so a concurrent winner cannot make this request rebuild and publish a second byte-distinct
    // extension for the same exact receipt and build unit.
    if let Some(plan_identity) = select_existing_direct_rustc_plan_identity(request)? {
        return Ok(reused_direct_rustc_plan_result(plan_identity));
    }
    reclaim_stale_publisher_staging(&staging_parent)?;
    let publisher_reservation = request
        .store
        .reserve_legacy_cargo_publisher_capacity(&request.domain, DEFAULT_OVEN_PUBLISHER_STAGING_FLOOR_BYTES)
        .map_err(OvenLegacyCargoError::Store)?;
    let staging = create_publisher_staging(&staging_parent)?;
    let cleanup = PublisherStagingCleanup { path: staging.clone() };
    let target = staging.join("target");
    let transient_limit = publisher_reservation.transient_limit_bytes;
    let reclaimed_store_entries = publisher_reservation.prune_report.removed_entries;
    // Normal release publication uses stable Cargo. `--unit-graph` is an unstable Cargo interface and remains
    // confined to the separately provisioned compiler-suite producer. The stable publisher instead joins Cargo's
    // artifact/build-script messages to exact successful rustc invocations, retaining physical unit edges without
    // adding a nightly requirement to installed release tooling.
    let cargo_outputs = run_legacy_cargo(
        &request.cargo,
        &request.rustc,
        &cargo_manifest,
        &target,
        &request.receipt.intent.target,
        &request.receipt.intent.profile,
        &request.receipt.intent.features,
        transient_limit,
        request.publication_kind,
        request.compact_debug_info,
        request.base_loaf.is_some(),
    )?;
    let cargo_lock = generated_project.join("Cargo.lock");
    let cargo_lock_bytes = regular_file_bytes(&cargo_lock)?;
    let profile_directory = cargo_profile_directory(&request.receipt.intent.profile)?;
    let requested_target_deps = target
        .join(&request.receipt.intent.target)
        .join(profile_directory)
        .join("deps");
    // Cross-target Cargo builds put target libraries and host-side procedural macros in separate dependency
    // directories. Direct rustc needs both: the target directory supplies root `--extern` inputs, and the host
    // directory supplies the proc-macro dylibs needed while expanding dependency metadata.
    let host_deps = target.join(profile_directory).join("deps");
    let rustc_host = rustc_host_target(&request.rustc)
        .map_err(|error| OvenLegacyCargoError::Plan(format!("cannot identify publisher Rust host target: {error}")))?;
    // Cargo is allowed to omit the redundant target-triple directory for a host-target build. Treat that layout as
    // the target closure only when the receipt target is exactly the publisher host; a cross-target request still
    // fails closed if it did not produce its target-specific output directory.
    let target_deps = if requested_target_deps.is_dir() {
        requested_target_deps
    } else if request.receipt.intent.target == rustc_host && host_deps.is_dir() {
        host_deps.clone()
    } else {
        requested_target_deps
    };
    // A dependency-free generated root can leave Cargo with no `deps` directory at all. Its empty direct-rustc
    // closure is still valid: the later source compilation needs neither `-L dependency` nor `--extern`. Create a
    // private empty directory solely so the shared closure reader can represent that zero-dependency case without
    // weakening the missing-artifact check for a declared direct dependency.
    if direct_dependencies.is_empty() && !target_deps.exists() {
        fs::create_dir_all(&target_deps).map_err(|source| OvenLegacyCargoError::Io {
            path: target_deps.clone(),
            source,
        })?;
    }
    let mut dependency_directories = vec![target_deps.clone()];
    if host_deps.is_dir() && host_deps != target_deps {
        dependency_directories.push(host_deps);
    }
    let metadata = match staged_metadata {
        Some(metadata) => metadata,
        None => read_legacy_cargo_metadata(&request.cargo, &cargo_manifest, &request.receipt.intent.features)?,
    };
    let mut selected_units = if outputs_have_rustc_trace(&cargo_outputs) {
        let mut capture =
            capture_legacy_cargo_selected_units_from_trace(&metadata, &cargo_outputs, &request.rustc, &rustc_host)?;
        let (host_cfg, target_cfg) =
            rustc_host_and_target_cfg_snapshots(&request.rustc, &request.receipt.intent.target)
                .map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
        capture.compiler = Some(OvenLegacyCargoSelectedCompilerContext {
            host: rustc_host.clone(),
            target: request.receipt.intent.target.clone(),
            toolchain: request.receipt.intent.toolchain.clone(),
            rustc_identity: rustc_identity.clone(),
            host_cfg,
            target_cfg,
        });
        Some(capture)
    } else {
        None
    };
    let resolved_direct_dependencies = resolve_direct_dependency_packages(&metadata, &direct_dependencies)?;
    let reported_artifact_files = publisher_output_artifact_paths(&cargo_outputs, &request.receipt.intent.profile)?;
    let (dependency_search_paths, externs, mut supporting_artifacts) = if reported_artifact_files.is_empty() {
        // Cargo's JSON protocol is the normal authority for an explicit bake. Retain the directory reader only as
        // a compatibility path for a dependency-free build whose Cargo version emits no compiler-artifact messages.
        artifact_closure(
            &staging,
            &target_deps,
            &dependency_directories,
            &direct_dependencies,
            request.publication_kind == OvenLegacyCargoPublicationKind::LibraryTests,
        )?
    } else {
        artifact_closure_from_reported_paths(
            &staging,
            &request.receipt.intent.target,
            &request.receipt.intent.profile,
            &resolved_direct_dependencies,
            request.publication_kind == OvenLegacyCargoPublicationKind::LibraryTests,
            &cargo_outputs,
        )?
    };
    if let Some(selected_units) = selected_units.as_mut() {
        supporting_artifacts.extend(retain_legacy_cargo_selected_generated_outputs(
            selected_units,
            &staging,
        )?);
    }
    let provider_entrypoints = provider_compilation_externs(
        request.provider_compilations,
        &consumer_direct_dependencies,
        &direct_dependencies,
        &request.source_evidence_key,
    )?;
    let (registry_leaves, registry_source_artifacts) =
        publisher_registry_leaf_catalog(PublisherRegistryLeafCatalogRequest {
            outputs: &cargo_outputs,
            metadata: &metadata,
            cargo_lock: &cargo_lock_bytes,
            staging: &staging,
            intent: &request.receipt.intent,
            rustc_host: &rustc_host,
            externs: &externs,
            supporting_artifacts: &supporting_artifacts,
            selected_units: selected_units.as_ref(),
            inspection_packages: request.inspection_packages.as_deref(),
        })?;
    supporting_artifacts.extend(registry_source_artifacts);
    // The complete-graph source catalog only runs when sealing against a base release Loaf (`base_loaf.is_some()`).
    // Fetch Cargo's own platform-filtered resolve for the receipt's exact target so that closure reflects what this
    // target's build actually requires, not every platform's locked dependencies.
    let platform_filtered_metadata = if request.base_loaf.is_some() {
        Some(read_legacy_cargo_metadata_for_platform(
            &request.cargo,
            &cargo_manifest,
            &request.receipt.intent.features,
            true,
            Some(request.receipt.intent.target.as_str()),
        )?)
    } else {
        None
    };
    let (registry_sources, transitive_registry_source_artifacts) = publisher_registry_source_catalog(
        &metadata,
        &cargo_lock_bytes,
        &staging,
        request.inspection_packages.as_deref(),
        &registry_leaves,
        request.base_loaf.is_some(),
        platform_filtered_metadata.as_ref(),
    )?;
    supporting_artifacts.extend(transitive_registry_source_artifacts);
    if !registry_sources.is_empty() {
        let registry_lock_path = staging.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
        if let Some(parent) = registry_lock_path.parent() {
            fs::create_dir_all(parent).map_err(|source| OvenLegacyCargoError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(&registry_lock_path, &cargo_lock_bytes).map_err(|source| OvenLegacyCargoError::Io {
            path: registry_lock_path,
            source,
        })?;
    }
    let provenance_path = staging.join("provenance/legacy-cargo.json");
    let provenance = OvenLegacyCargoProvenance {
        schema_version: OVEN_LEGACY_CARGO_PROVENANCE_SCHEMA_VERSION,
        boundary: "legacy_cargo".to_string(),
        cargo_version: cargo_version.clone(),
        cargo_manifest_digest: digest_bytes(&cargo_manifest_bytes),
        cargo_lock_digest: digest_bytes(&cargo_lock_bytes),
        publication_kind: request.publication_kind,
        target: request.receipt.intent.target.clone(),
        toolchain: request.receipt.intent.toolchain.clone(),
        profile: request.receipt.intent.profile.clone(),
    };
    write_provenance(&provenance_path, &provenance)?;
    let has_registry_sources = !registry_sources.is_empty();
    let mut plan = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: request.receipt.intent.clone(),
        dependency_search_paths,
        native_search_paths: Vec::new(),
        externs,
        entrypoint_dependency_search_paths: BTreeMap::new(),
        entrypoint_externs: provider_entrypoints,
        registry_leaves: registry_leaves.clone(),
        registry_sources,
        compile_environment: request.compile_environment.clone(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts,
    };
    if request.source_compiler_vocab_support {
        if request.base_loaf.is_some() {
            return Err(OvenLegacyCargoError::Plan(
                "a project extension cannot bake a source compiler vocabulary helper beside its selected release-cohort Loaf"
                    .to_string(),
            ));
        }
        let compiler_root = source_compiler_vocab_support_root()?;
        let compiler_support_target = staging.join("compiler-vocab-target");
        crate::loaf_bake::bake_source_compiler_vocab_support(oven_rustc::loaf::OvenSourceCompilerVocabSupportRequest {
            plan: &mut plan,
            loaf_staging: &staging,
            compiler_root: &compiler_root,
            cargo: &request.cargo,
            rustc: &request.rustc,
            cargo_target: &compiler_support_target,
            capacity_roots: &[&staging],
            transient_limit,
        })
        .map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
    }
    canonicalize_supporting_artifacts(&mut plan.supporting_artifacts)?;
    // Capture each source role's search closure only once the manifest's artifact set is final.
    //
    // A closure records its members by relative path and digest, and `bind_source_search_roles` later accepts a
    // directory as a member's binding only when the role claims *every* artifact in it. Capturing before the
    // vocabulary helper can add artifacts, and before `canonicalize_supporting_artifacts` rewrites their paths,
    // leaves the closure describing a manifest that no longer exists: the directory inventory then holds members
    // the role never claimed, and the bake refuses with "cannot isolate selected member ... its canonical directory
    // has a missing or co-resident unselected artifact and no clean admitted alternative".
    //
    // Nothing after this point adds an artifact. The project-extension composition below re-derives its own roles
    // by merging the base's closures, so it needs these present rather than deferred further.
    let source_closure = plan
        .capture_source_search_closure(&plan.dependency_search_paths)
        .map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
    plan.entrypoint_dependency_search_paths = plan
        .entrypoint_externs
        .keys()
        .map(|key| (key.clone(), source_closure.clone()))
        .collect();
    let (kind, payload, materialized_plan) = if let Some(base) = request.base_loaf.as_ref() {
        if base.artifacts.intent != request.receipt.intent {
            return Err(OvenLegacyCargoError::ReceiptMismatch {
                message: "selected project-extension base Loaf has an incompatible direct-Rustc intent".to_string(),
            });
        }
        let root_registry_packages = declared_direct_dependencies
            .values()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let complete_plan = plan
            .with_release_cohort_from_base(base.artifacts, &root_registry_packages)
            .map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
        validate_provider_macro_artifacts(
            request.provider_compilations,
            &metadata,
            &resolved_direct_dependencies,
            &cargo_outputs,
            &staging,
            &complete_plan.externs,
            Some(base),
        )?;
        let partition = complete_plan
            .partition_against_base(base.artifacts)
            .map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
        let registry_source_dependencies = project_registry_source_dependencies(
            &metadata,
            &declared_direct_dependencies,
            &complete_plan.registry_sources,
        )?;
        let cargo_manifest =
            std::str::from_utf8(&cargo_manifest_bytes).map_err(|error| OvenLegacyCargoError::InvalidInput {
                field: "Cargo.toml",
                message: format!("must be UTF-8: {error}"),
            })?;
        let cargo_manifest =
            toml::from_str::<toml::Value>(cargo_manifest).map_err(|error| OvenLegacyCargoError::InvalidInput {
                field: "Cargo.toml",
                message: format!("must be valid TOML: {error}"),
            })?;
        let dev_registry_source_dependencies = project_registry_source_dependencies(
            &metadata,
            &direct_dependency_aliases(&cargo_manifest, "dev-dependencies"),
            &complete_plan.registry_sources,
        )?;
        if partition.base_paths.is_empty() {
            return Err(OvenLegacyCargoError::Plan(
                "selected standard-library Loaf shares no byte-identical artifacts with this project closure; refuse a duplicate whole-closure project bake"
                    .to_string(),
            ));
        }
        if partition.extension_paths.is_empty() {
            return Err(OvenLegacyCargoError::Plan(
                "explicit project bake produced no non-stdlib artifacts; consume the selected standard-library Loaf directly"
                    .to_string(),
            ));
        }
        // ---- Stage re-rooted collision artifacts before atomic copying ----
        // Cohort composition moves a salted extension artifact whose filename collides with the base's execution
        // closure into its `extension-deps` sibling directory, but the built file still sits in the Cargo `deps`
        // output. Materialization resolves every source as the staging root joined with the recorded relative path,
        // so link each re-rooted artifact into its recorded home here.
        for relative_path in &partition.extension_paths {
            let Some(source) = rerooted_artifact_staging_source(relative_path) else {
                continue;
            };
            let source_path = staging.join(&source);
            let target_path = staging.join(relative_path);
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).map_err(|source| OvenLegacyCargoError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            if fs::hard_link(&source_path, &target_path).is_err() {
                fs::copy(&source_path, &target_path).map_err(|source| OvenLegacyCargoError::Io {
                    path: target_path.clone(),
                    source,
                })?;
            }
        }
        let materialized_plan = complete_plan
            .artifact_fragment(&partition.extension_paths)
            .map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
        let payload = serde_json::to_vec(&OvenProjectExtensionPayload {
            schema_version: OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION,
            base_loaf_identity: base.loaf_identity.clone(),
            base_build_unit_identity: base.build_unit_identity.clone(),
            publisher_plan: plan,
            complete_plan,
            registry_source_dependencies,
            dev_registry_source_dependencies,
            extension_paths: partition.extension_paths.into_iter().collect(),
        })
        .map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
        (OvenArtifactKind::ProjectPayload, payload, materialized_plan)
    } else {
        validate_provider_macro_artifacts(
            request.provider_compilations,
            &metadata,
            &resolved_direct_dependencies,
            &cargo_outputs,
            &staging,
            &plan.externs,
            None,
        )?;
        let payload = serde_json::to_vec(&plan).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
        (OvenArtifactKind::DirectRustcPlan, payload, plan)
    };
    let mut materialized_files = materialized_plan
        .materialized_artifacts(&staging, &request.receipt.intent)
        .map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?
        .into_iter()
        .map(|artifact| OvenArtifactMaterializedFile {
            source_path: artifact.source_path,
            relative_path: artifact.relative_path,
        })
        .collect::<Vec<_>>();
    materialize_sealed_registry_lock(&staging, has_registry_sources, &mut materialized_files)?;
    // Publisher metadata belongs in the immutable Loaf, but it is not a direct-Rustc input. Keeping it out of the
    // executable artifact plan lets a project extension share the base closure even though its own Cargo lock and
    // publication receipt naturally differ. The store manifest still digests and verifies this copied file.
    materialized_files.push(OvenArtifactMaterializedFile {
        source_path: provenance_path,
        relative_path: "provenance/legacy-cargo.json".to_string(),
    });
    let publication = OvenArtifactPublishRequest {
        receipt: request.receipt.clone(),
        domain: request.domain.clone(),
        kind,
        payload,
        materialized_files,
        materialized_directories: Vec::new(),
    };
    let transient_reservation_bytes = conservative_directory_reservation(&staging)?;
    request
        .store
        .ensure_legacy_cargo_batch_physical_capacity(&staging, std::slice::from_ref(&publication))?;
    let artifact = request.store.publish_from_legacy_cargo(&publication)?;
    drop(cleanup);
    drop(publisher_lock);
    Ok(OvenLegacyCargoPrepareResult {
        plan_identity: artifact.identity,
        cargo_version,
        cargo_manifest_digest: digest_bytes(&cargo_manifest_bytes),
        cargo_lock_digest: digest_bytes(&cargo_lock_bytes),
        registry_leaves,
        selected_units,
        transient_reservation_bytes,
        reclaimed_store_entries,
    })
}

/// Return a reusable project extension only when its exact project receipt and selected standard-library base match.
///
/// `DirectRustcPlan` entries are intentionally excluded. They may be valid compiler-owned Loafs for a compatible
/// generated unit, yet omit a provider's transitive source/metadata authority. Accepting one here would suppress
/// the only publisher transaction that can seal the caller's complete closure.
fn select_existing_project_extension_identity(
    store: &OvenStore,
    receipt: &OvenReceipt,
    base: &OvenLegacyCargoBaseLoaf<'_>,
) -> Result<Option<String>, OvenLegacyCargoError> {
    let candidates = store.select_payloads_matching_for_execution(|manifest| {
        manifest.kind == OvenArtifactKind::ProjectPayload
            && manifest.receipt_identity == receipt.identity
            && manifest.build_unit_identity == receipt.build_unit_identity
            && manifest.intent == receipt.intent
    })?;
    let mut identities = Vec::new();
    for candidate in candidates {
        let identity = candidate.manifest.identity;
        let payload = serde_json::from_slice::<OvenProjectExtensionPayload>(&candidate.payload).map_err(|error| {
            OvenLegacyCargoError::Plan(format!(
                "stored project extension candidate {identity} has an invalid payload: {error}"
            ))
        })?;
        match validate_project_extension_payload_against_base(
            &payload,
            &base.loaf_identity,
            &base.build_unit_identity,
            base.artifacts,
        ) {
            Ok(_) => identities.push(identity),
            // A receipt-exact extension that no longer validates against the selected base is a publisher
            // decision worth seeing: the caller will seal a second extension for the same receipt, and every later
            // selection for that receipt then has two candidates.
            Err(error) => tracing::debug!(
                "stored project extension {identity} is not reusable against base {}: {error}",
                base.loaf_identity
            ),
        }
    }
    match identities.as_slice() {
        [] => Ok(None),
        [identity] => Ok(Some(identity.clone())),
        _ => Err(OvenLegacyCargoError::Plan(format!(
            "multiple receipt-exact project extensions are authorized by one selected standard-library Loaf: {}",
            identities.join(", ")
        ))),
    }
}

/// Canonicalize the union of compiled artifacts and independently discovered sealed source files.
///
/// One linkable registry leaf and the complete inspection-source catalog may deliberately name the same staged
/// source file. Identical declarations collapse to one manifest record; conflicting digests remain a hard publisher
/// error instead of allowing one authority surface to overwrite the other.
pub fn canonicalize_supporting_artifacts(
    artifacts: &mut Vec<OvenRustcSupportingArtifact>,
) -> Result<(), OvenLegacyCargoError> {
    artifacts.sort_by(|left, right| (&left.relative_path, &left.digest).cmp(&(&right.relative_path, &right.digest)));
    if let Some(pair) = artifacts
        .windows(2)
        .find(|pair| pair[0].relative_path == pair[1].relative_path && pair[0].digest != pair[1].digest)
    {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "direct-rustc supporting artifacts",
            message: format!(
                "staged artifact `{}` has conflicting digests {} and {}",
                pair[0].relative_path, pair[0].digest, pair[1].digest
            ),
        });
    }
    artifacts.dedup_by(|left, right| left.relative_path == right.relative_path);
    Ok(())
}

/// Publish the compiler workspace's direct-rustc test-target plan and its CLI fixture through the one explicit Cargo
/// boundary.
///
/// Cargo's private unit graph is observed only at this publisher boundary, then converted into receipt-bound direct
/// targets and an exact immutable dependency closure. Later compiler-suite invocations acquire a lease and compile
/// and execute that verified plan without invoking Cargo or reading a Cargo target directory.
pub fn prepare_compiler_test_suite(
    request: &OvenLegacyCargoPrepareRequest<'_>,
) -> Result<OvenLegacyCargoCompilerSuiteResult, OvenLegacyCargoError> {
    let suite_started = Instant::now();
    request
        .receipt
        .verify_identity()
        .map_err(|error| OvenLegacyCargoError::ReceiptMismatch {
            message: error.to_string(),
        })?;
    if request.publication_kind != OvenLegacyCargoPublicationKind::LibraryTests {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler suite publication kind",
            message: "must use the explicit library-tests publisher boundary".to_string(),
        });
    }
    let rustc_identity =
        rustc_identity(&request.rustc).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
    if rustc_identity != request.receipt.intent.toolchain {
        return Err(OvenLegacyCargoError::ReceiptMismatch {
            message: format!(
                "receipt requires Rust compiler `{}`, but --rustc reports `{rustc_identity}`",
                request.receipt.intent.toolchain
            ),
        });
    }
    if let Some(suite_identity) = select_or_import_compiler_test_suite_identity(request.store, &request.receipt)? {
        return Ok(OvenLegacyCargoCompilerSuiteResult {
            suite_identity,
            cargo_version: "not-run-existing-suite".to_string(),
            cargo_manifest_digest: "not-run-existing-suite".to_string(),
            cargo_lock_digest: "not-run-existing-suite".to_string(),
            transient_reservation_bytes: 0,
            timing: OvenLegacyCargoCompilerSuiteTiming {
                preflight_and_sdk_elapsed_ms: suite_started.elapsed().as_millis(),
                ..OvenLegacyCargoCompilerSuiteTiming::default()
            },
        });
    }

    let generated_project = canonical_directory(&request.generated_project, "generated project")?;
    let cargo_manifest = generated_project.join("Cargo.toml");
    let cargo_manifest_bytes = regular_file_bytes(&cargo_manifest)?;
    let _ =
        receipt_authorized_generated_root_bytes(&generated_project, &request.receipt, &request.source_evidence_key)?;
    let cargo_version = tool_version(&request.cargo, "cargo")?;
    let staging_parent = request.store.root().join("legacy-cargo-staging");
    let publisher_lock = acquire_publisher_lock(&staging_parent)?;
    reclaim_stale_publisher_staging(&staging_parent)?;
    // The compiler suite owns its store and its 16 GiB policy; nothing of another project's is worth evicting there.
    let publisher_reservation = request
        .store
        .reserve_legacy_cargo_publisher_capacity(&request.domain, 0)
        .map_err(OvenLegacyCargoError::Store)?;
    let staging = create_publisher_staging(&staging_parent)?;
    let cleanup = PublisherStagingCleanup { path: staging.clone() };
    // The suite publisher copies an already prepared, read-only SDK inventory into the immutable entry. Rebuilding
    // source components here used the ordinary `incan build --lib` helper, which can recurse into generated-Cargo
    // work and turn the hidden Loaf baker into an unbounded second build system. A missing inventory is an
    // explicit Oven preparation miss, never authority to launch that helper or create a hidden Cargo cache.
    let prepared_sdk_root = request
        .provider_hooks
        .sdk_provider_root(request.sdk_inventory.as_deref())?;
    // The component crates retain path dependencies on compiler runtime crates.  Copying only the provider tree
    // would leave those paths pointing back to the publisher checkout, which is both an SDK leak and a later Cargo
    // failure.  Rebase that small compiler-owned runtime source closure inside the immutable provider tree before
    // recording any materialized files.
    let staged_sdk_root =
        stage_self_contained_sdk_provider_tree(&prepared_sdk_root, &staging, request.provider_hooks.as_ref())?;
    let staged_runtime_inputs = compiler_suite_staged_runtime_inputs(&staged_sdk_root, &request.compiler)?;
    let sdk_inventory_path = staged_sdk_root.join(request.provider_hooks.sdk_inventory_file());
    let sdk_inventory_digest = digest_bytes(&regular_file_bytes(&sdk_inventory_path)?);
    let mut index_materialized_files =
        materialized_files_from_directory(&staged_sdk_root, "providers", "SDK provider inventory")?;
    // Direct-rustc children consume the compiler-owned standard-library family under a shared generation lease.
    // Copying this already sealed release artifact into every receipt-bound compiler-suite store doubled retained
    // bytes and made durable publication dominate cold bakes.  The suite instead records the exact committed
    // generation; the scheduler verifies and holds that generation before it starts its first child.
    let compiler_loaf_root = request.compiler_loaf_root.as_deref().ok_or_else(|| {
        OvenLegacyCargoError::Plan(
            "compiler-suite publication requires the explicit committed compiler Loaf envelope selected by its baker"
                .to_string(),
        )
    })?;
    let toolchain_loaf_generation =
        compiler_suite_toolchain_loaf_generation_reference(compiler_loaf_root, &staged_runtime_inputs)?;
    // A compiler-suite publisher's private selection target is bounded by the store's aggregate physical ceiling,
    // not charged as retained bytes in one compatibility domain. Prepared foundation inputs are checked against the
    // same aggregate ceiling as their related immutable publication. The retained suite closure is then admitted
    // against its one compatibility-domain allowance, so no partition label can hide an oversized suite.
    let publisher_transient_limit = publisher_reservation.transient_limit_bytes;
    let prepared_related_limit = publisher_reservation.transient_limit_bytes;
    // Cargo exposes its resolved test-unit graph only at this explicit publisher boundary. The graph is converted
    // below into Oven target plans; it is never retained as a normal-command dependency or target directory.
    let unit_graph_started = Instant::now();
    let unit_graph_target = staging.join("unit-graph-target");
    let unit_graph_output = run_legacy_cargo_invocation(
        &request.cargo,
        &request.rustc,
        &cargo_manifest,
        &unit_graph_target,
        &staging,
        &request.receipt.intent.target,
        &request.receipt.intent.profile,
        &request.receipt.intent.features,
        publisher_transient_limit,
        "test",
        &OvenLegacyCargoInvocationTarget::WorkspaceTests,
        true,
        false,
        false,
    )?;
    let unit_graph_elapsed_ms = unit_graph_started.elapsed().as_millis();
    let unit_graph = parse_compiler_suite_unit_graph(&unit_graph_output)?;
    // Inspect first: a publisher must fail before materializing even the bounded root compatibility closure if its
    // declared suite contains a target class Oven cannot execute without Cargo. The full workspace graph remains
    // planning evidence only; it is never compiled through Cargo as the ordinary test substrate.
    validate_compiler_suite_unit_graph(&generated_project, &unit_graph)?;
    let metadata = read_legacy_cargo_metadata(&request.cargo, &cargo_manifest, &request.receipt.intent.features)?;
    let foundation_build_started = Instant::now();
    let foundation_dependencies = compiler_suite_foundation_dependencies(&generated_project, &unit_graph, &metadata)?;
    let foundation_manifest = compiler_suite_foundation_manifest(&foundation_dependencies)?;
    let third_party_foundation_manifest = stage_compiler_suite_foundation_manifest(
        &staging,
        &foundation_manifest,
        &generated_project.join("Cargo.lock"),
        &foundation_dependencies,
    )?;
    reclaim_unmaterialized_compiler_suite_target_files(&unit_graph_target, &[])?;

    // The only compilation Cargo is authorized to perform is this sealed third-party foundation. Its copied lock
    // file preserves the compiler workspace's exact registry resolution, while its private root has no path
    // dependency on the compiler workspace. Every compiler library, proc macro, CLI and test root below is then
    // rebuilt from receipt-authorized source by the direct-Rustc scheduler.
    let foundation_target = staging.join("third-party-foundation-target");
    let foundation_output = run_legacy_cargo_invocation(
        &request.cargo,
        &request.rustc,
        &third_party_foundation_manifest,
        &foundation_target,
        &staging,
        &request.receipt.intent.target,
        &request.receipt.intent.profile,
        &[],
        publisher_transient_limit,
        "build",
        &OvenLegacyCargoInvocationTarget::PackageLibrary,
        false,
        false,
        false,
    )?;
    // This phase is the one explicitly permitted Cargo compilation.  Keep the manifest preparation and the child
    // process under one clock: reporting only the setup above would misleadingly classify Cargo's actual foundation
    // work as unaccounted suite time.
    let foundation_build_elapsed_ms = foundation_build_started.elapsed().as_millis();
    let profile_directory = cargo_profile_directory(&request.receipt.intent.profile)?;
    let target_deps = foundation_target
        .join(&request.receipt.intent.target)
        .join(profile_directory)
        .join("deps");
    let host_deps = foundation_target.join(profile_directory).join("deps");
    let dependency_directories = compiler_suite_dependency_directories(target_deps, host_deps);
    // The private foundation root itself may be the only Cargo-reported direct-Rustc artifact on a compatible
    // host/profile layout. Keep every exact compiler-artifact path that Cargo reported, rather than relying on
    // the conventional `deps/` directories to exist. The catalog still rejects paths outside publisher staging.
    let foundation_direct_artifact_files = compiler_suite_output_artifact_paths(&foundation_output)?;
    let foundation_catalog =
        compiler_suite_artifact_catalog(&staging, &dependency_directories, &foundation_direct_artifact_files)?;
    let foundation_artifact_index = compiler_suite_artifact_index(&foundation_output, &request.receipt.intent.target)?;
    let mut root_indices = Vec::new();
    for root_index in &unit_graph.roots {
        let root = unit_graph.units.get(*root_index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite unit graph root index {root_index} is outside its unit list"
            ))
        })?;
        if matches!(root.mode.as_str(), "test" | "doctest")
            && compiler_suite_unit_is_in_workspace(&generated_project, root)?
        {
            root_indices.push(*root_index);
        }
    }
    if root_indices.is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "compiler-suite graph has no workspace test roots for direct-Rustc materialization".to_string(),
        ));
    }
    let direct_plan_started = Instant::now();
    let mut shard_references = Vec::new();
    let mut foundation_references = Vec::new();
    let mut foundation_requests = Vec::new();
    let mut shard_requests = Vec::new();
    for foundation in compiler_suite_foundation_plans(
        &foundation_catalog.closure,
        &foundation_catalog.materialized_files,
        request.store.limits().max_domain_logical_bytes,
    )? {
        let foundation_payload =
            serde_json::to_vec(&foundation.payload).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
        let foundation_request = OvenArtifactPublishRequest {
            receipt: request.receipt.clone(),
            domain: request.domain.clone(),
            kind: OvenArtifactKind::CompilerTestSuiteFoundation,
            payload: foundation_payload,
            materialized_files: foundation.materialized_files,
            materialized_directories: Vec::new(),
        };
        let manifest = request.store.manifest_for_publication(&foundation_request)?;
        foundation_references.push(OvenCompilerTestSuiteFoundationReference {
            identity: manifest.identity,
            label: foundation.payload.label,
        });
        foundation_requests.push(foundation_request);
    }
    foundation_references.sort();
    foundation_references.dedup();
    for root_index in root_indices {
        let (mut shard, _materialized_files) = compiler_suite_direct_target_shard_from_catalog(
            &generated_project,
            &request.receipt,
            &unit_graph,
            root_index,
            &foundation_artifact_index,
            &foundation_catalog,
        )?;
        shard.foundation_references = foundation_references.clone();
        let payload = serde_json::to_vec(&shard).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
        let shard_request = OvenArtifactPublishRequest {
            receipt: request.receipt.clone(),
            domain: request.domain.clone(),
            kind: OvenArtifactKind::CompilerTestSuiteShard,
            payload,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        };
        let manifest = request.store.manifest_for_publication(&shard_request)?;
        shard_references.push(OvenCompilerTestSuiteShardReference {
            identity: manifest.identity,
            target: shard.target_key(),
            source_bytes: compiler_suite_verified_target_source_bytes(
                &generated_project,
                &request.receipt,
                &shard.target,
            )?,
        });
        shard_requests.push(shard_request);
    }
    shard_references.sort_by(|left, right| left.target.cmp(&right.target));

    // Cargo supplies only the compiler CLI's resolved unit graph here. It does not compile `src/main.rs` or the
    // workspace library; direct Rustc receives their source plans from the sealed foundation catalog below.
    let cli_unit_graph_target = staging.join("cli-unit-graph-target");
    let cli_unit_graph_output = run_legacy_cargo_invocation(
        &request.cargo,
        &request.rustc,
        &cargo_manifest,
        &cli_unit_graph_target,
        &staging,
        &request.receipt.intent.target,
        &request.receipt.intent.profile,
        &request.receipt.intent.features,
        publisher_transient_limit,
        "build",
        &OvenLegacyCargoInvocationTarget::CompilerCli,
        true,
        false,
        false,
    )?;
    let cli_unit_graph = parse_compiler_suite_unit_graph(&cli_unit_graph_output)?;
    let (cli_target, cli_workspace_libraries) = compiler_suite_cli_target_from_artifact_index(
        &generated_project,
        &request.receipt,
        &cli_unit_graph,
        &foundation_artifact_index,
        &foundation_catalog,
    )?;
    reclaim_unmaterialized_compiler_suite_target_files(&cli_unit_graph_target, &[])?;
    enforce_compiler_suite_prepared_staging_capacity(&staging, prepared_related_limit)?;
    // Schema 12 no longer launches a second Cargo build for `generated_rust_warning_clean`.  The scheduler derives
    // its facet Rustc plans from a selected shard's already-receipted workspace-library DAG and the sealed
    // foundations below.  This keeps one publisher staging root physically bounded instead of holding a second
    // mostly-duplicate Cargo target beside the third-party foundation.
    let warning_check_artifacts = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: request.receipt.intent.clone(),
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: Vec::new(),
        registry_sources: Vec::new(),
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: Vec::new(),
    };
    let cargo_lock = generated_project.join("Cargo.lock");
    let cargo_lock_bytes = regular_file_bytes(&cargo_lock)?;
    let provenance_path = staging.join("provenance/legacy-cargo.json");
    let provenance = OvenLegacyCargoProvenance {
        schema_version: OVEN_LEGACY_CARGO_PROVENANCE_SCHEMA_VERSION,
        boundary: "legacy_cargo".to_string(),
        cargo_version: cargo_version.clone(),
        cargo_manifest_digest: digest_bytes(&cargo_manifest_bytes),
        cargo_lock_digest: digest_bytes(&cargo_lock_bytes),
        publication_kind: request.publication_kind,
        target: request.receipt.intent.target.clone(),
        toolchain: request.receipt.intent.toolchain.clone(),
        profile: request.receipt.intent.profile.clone(),
    };
    write_provenance(&provenance_path, &provenance)?;
    let cli_foundation_references = foundation_references.clone();
    let payload = OvenCompilerTestSuitePayload {
        schema_version: OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION,
        test_targets: Vec::new(),
        shard_references,
        foundation_references,
        toolchain_data_references: Vec::new(),
        toolchain_loaf_generation: Some(toolchain_loaf_generation),
        binary_targets: Vec::new(),
        test_artifact_closure: None,
        cli_artifact_closure: Some(foundation_catalog.closure.clone()),
        cli_foundation_references,
        cli_target: Some(cli_target),
        cli_workspace_libraries,
        sdk_inventory_relative_path: format!("providers/{}", request.provider_hooks.sdk_inventory_file()),
        sdk_inventory_digest,
        toolchain_data_relative_root: None,
        warning_check_artifacts,
    };
    let payload_bytes = serde_json::to_vec(&payload).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
    index_materialized_files.extend([OvenArtifactMaterializedFile {
        source_path: provenance_path,
        relative_path: "provenance/legacy-cargo.json".to_string(),
    }]);
    enforce_compiler_suite_prepared_staging_capacity(&staging, prepared_related_limit)?;
    let transient_reservation_bytes = conservative_directory_reservation(&staging)?;
    let index_request = OvenArtifactPublishRequest {
        receipt: request.receipt.clone(),
        domain: request.domain.clone(),
        kind: OvenArtifactKind::CompilerTestSuite,
        payload: payload_bytes,
        materialized_files: index_materialized_files,
        materialized_directories: Vec::new(),
    };
    let mut batch = Vec::with_capacity(
        foundation_requests
            .len()
            .saturating_add(shard_requests.len())
            .saturating_add(1),
    );
    batch.extend(foundation_requests);
    batch.extend(shard_requests);
    // The suite index is its sole selection authority. Keep it last in the publisher request as well as the store's
    // durable commit order so future publication refactors cannot accidentally expose it before its members.
    batch.push(index_request);
    let direct_plan_elapsed_ms = direct_plan_started.elapsed().as_millis();
    let store_publication_started = Instant::now();
    request
        .store
        .ensure_legacy_cargo_batch_physical_capacity(&staging, &batch)?;
    let artifacts = request.store.publish_batch_from_legacy_cargo(&batch)?;
    let artifact = artifacts
        .iter()
        .find(|artifact| artifact.kind == OvenArtifactKind::CompilerTestSuite)
        .ok_or_else(|| {
            OvenLegacyCargoError::Plan("compiler-suite batch publication returned no index manifest".to_string())
        })?;
    let store_publication_elapsed_ms = store_publication_started.elapsed().as_millis();
    drop(cleanup);
    drop(publisher_lock);
    Ok(OvenLegacyCargoCompilerSuiteResult {
        suite_identity: artifact.identity.clone(),
        cargo_version,
        cargo_manifest_digest: digest_bytes(&cargo_manifest_bytes),
        cargo_lock_digest: digest_bytes(&cargo_lock_bytes),
        transient_reservation_bytes,
        timing: OvenLegacyCargoCompilerSuiteTiming {
            preflight_and_sdk_elapsed_ms: unit_graph_started.duration_since(suite_started).as_millis(),
            unit_graph_elapsed_ms,
            foundation_build_elapsed_ms,
            direct_plan_elapsed_ms,
            store_publication_elapsed_ms,
        },
    })
}

/// Map the explicit publisher's supported receipt profiles to Cargo's output directory names.
fn cargo_profile_directory(profile: &str) -> Result<&'static str, OvenLegacyCargoError> {
    match profile {
        "debug" => Ok("debug"),
        "release" => Ok("release"),
        OVEN_COMPILER_TEST_PROFILE => Ok(OVEN_COMPILER_TEST_PROFILE),
        _ => Err(OvenLegacyCargoError::InvalidInput {
            field: "receipt profile",
            message: format!(
                "the internal compatibility publisher supports only debug, release, or {OVEN_COMPILER_TEST_PROFILE}, got `{profile}`"
            ),
        }),
    }
}

/// Select the compiler suite locally, and on a miss admit the whole family from any configured mirror first.
///
/// The family — foundation, shards, and the suite record — is what a cold runner spends its first quarter hour
/// rebuilding through Cargo. A mirror in store layout (`INCAN_OVEN_MIRRORS`) turns that miss into a verified copy;
/// nothing is trusted from it, and the bake below still runs when no mirror has a compatible family. The import is
/// keyed by this receipt's build unit and intent, so a mirror baked for another compiler or target contributes
/// nothing.
fn select_or_import_compiler_test_suite_identity(
    store: &OvenStore,
    receipt: &OvenReceipt,
) -> Result<Option<String>, OvenLegacyCargoError> {
    if let Some(identity) = select_compiler_test_suite_identity(store, receipt)? {
        return Ok(Some(identity));
    }
    let mirrors = oven_store::store_mirror::configured_mirrors(|name| std::env::var_os(name));
    if mirrors.is_empty() {
        return Ok(None);
    }
    let imported =
        oven_store::store_mirror::import_matching_from_mirrors(store, &mirrors, Some(receipt), |manifest| {
            matches!(
                manifest.kind,
                OvenArtifactKind::CompilerTestSuite
                    | OvenArtifactKind::CompilerTestSuiteFoundation
                    | OvenArtifactKind::CompilerTestSuiteShard
            ) && manifest.build_unit_identity == receipt.build_unit_identity
                && manifest.intent == receipt.intent
        })?;
    if imported.is_empty() {
        return Ok(None);
    }
    select_compiler_test_suite_identity(store, receipt)
}

/// Select one immutable compiler suite only when it is uniquely authorized by the exact receipt build unit.
fn select_compiler_test_suite_identity(
    store: &OvenStore,
    receipt: &OvenReceipt,
) -> Result<Option<String>, OvenLegacyCargoError> {
    let candidates = store.select_payloads_matching_for_execution(|manifest| {
        manifest.kind == OvenArtifactKind::CompilerTestSuite
            && manifest.build_unit_identity == receipt.build_unit_identity
            && manifest.intent == receipt.intent
    })?;
    let mut identities = Vec::new();
    for candidate in candidates {
        let identity = candidate.manifest.identity;
        let payload = serde_json::from_slice::<OvenCompilerTestSuitePayload>(&candidate.payload).map_err(|error| {
            OvenLegacyCargoError::Plan(format!(
                "stored compiler-suite candidate {identity} has an invalid payload: {error}"
            ))
        })?;
        // A prior schema remains readable only for an already-selected historical execution. It is never a
        // publisher reuse hit: a new publisher version must replace it so newly required receipt evidence cannot
        // be silently absent from a supposedly current immutable suite.
        if payload.schema_version == OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION
            && payload.test_targets.is_empty()
            && payload.test_artifact_closure.is_none()
            && !payload.shard_references.is_empty()
            && payload
                .shard_references
                .iter()
                .all(|reference| reference.source_bytes > 0)
            && payload.cli_target.is_some()
            && payload.cli_artifact_closure.is_some()
            && !payload.foundation_references.is_empty()
            && payload.toolchain_data_relative_root.is_none()
            && payload.toolchain_data_references.is_empty()
            && payload.toolchain_loaf_generation.is_some()
        {
            identities.push(identity);
        }
    }
    match identities.as_slice() {
        [] => Ok(None),
        [identity] => Ok(Some(identity.clone())),
        _ => Err(OvenLegacyCargoError::Plan(format!(
            "multiple compiler test suites are authorized by one build unit: {}",
            identities.join(", ")
        ))),
    }
}

/// Metadata search boundary used while resolving one explicit Rust-inspection surface.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InspectionPackageScope {
    /// Select only packages named by the generated project's root dependency edges.
    DirectRoot,
    /// Select any exact package in the already locked compiler graph.
    ResolvedGraph,
    /// Seal every package in the already locked compiler graph.
    CompleteResolvedGraph,
}

/// Resolve an explicit inspection surface and its transitive package-ID closure from locked Cargo metadata.
fn inspection_package_closure_ids(
    metadata: &CargoMetadata,
    requested: &[OvenLegacyCargoInspectionPackage],
    scope: InspectionPackageScope,
) -> Result<BTreeSet<String>, OvenLegacyCargoError> {
    if requested.is_empty() && scope != InspectionPackageScope::CompleteResolvedGraph {
        return Ok(BTreeSet::new());
    }
    let resolve = metadata.resolve.as_ref().ok_or_else(|| {
        OvenLegacyCargoError::Plan(
            "locked Cargo metadata omitted the resolve graph required by the Rust-inspection surface".to_string(),
        )
    })?;
    let nodes = resolve
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let packages = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    let candidate_ids = match scope {
        InspectionPackageScope::DirectRoot => {
            let root = resolve.root.as_deref().ok_or_else(|| {
                OvenLegacyCargoError::Plan(
                    "locked Cargo metadata omitted the generated-project root package".to_string(),
                )
            })?;
            let root_node = nodes.get(root).ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "locked Cargo metadata root package `{root}` has no resolve node"
                ))
            })?;
            root_node.dependencies.iter().map(String::as_str).collect::<Vec<_>>()
        }
        InspectionPackageScope::ResolvedGraph | InspectionPackageScope::CompleteResolvedGraph => {
            resolve.nodes.iter().map(|node| node.id.as_str()).collect()
        }
    };
    if scope == InspectionPackageScope::CompleteResolvedGraph {
        return Ok(candidate_ids.into_iter().map(str::to_string).collect());
    }
    let mut selected = BTreeSet::new();
    for request in requested {
        let requirement = semver::VersionReq::parse(&request.version_requirement).map_err(|error| {
            OvenLegacyCargoError::Plan(format!(
                "invalid Rust-inspection version requirement `{}` for `{}`: {error}",
                request.version_requirement, request.package
            ))
        })?;
        let mut matches = candidate_ids
            .iter()
            .filter_map(|id| packages.get(id).copied())
            .filter(|package| {
                package.name == request.package
                    && package
                        .source
                        .as_deref()
                        .is_some_and(|source| source.starts_with("registry+"))
                    && semver::Version::parse(&package.version).is_ok_and(|version| requirement.matches(&version))
            })
            .map(|package| package.id.clone())
            .collect::<Vec<_>>();
        matches.sort();
        matches.dedup();
        match matches.as_slice() {
            [package_id] => {
                selected.insert(package_id.clone());
            }
            [] => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "locked Cargo metadata does not resolve Rust-inspection package `{}` `{}` in the selected scope",
                    request.package, request.version_requirement
                )));
            }
            _ => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "locked Cargo metadata resolves Rust-inspection package `{}` `{}` ambiguously: {}",
                    request.package,
                    request.version_requirement,
                    matches.join(", ")
                )));
            }
        }
    }
    let mut pending = selected.iter().cloned().collect::<Vec<_>>();
    while let Some(package_id) = pending.pop() {
        let node = nodes.get(package_id.as_str()).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "Rust-inspection package `{package_id}` has no locked resolve node"
            ))
        })?;
        for dependency in &node.dependencies {
            if selected.insert(dependency.clone()) {
                pending.push(dependency.clone());
            }
        }
    }
    Ok(selected)
}

/// Path to the publisher-authored source authority inherited by a cold baker fixture child.
pub const OVEN_LEGACY_CARGO_INSPECTION_AUTHORITY_ENV: &str = "INCAN_OVEN_LEGACY_CARGO_INSPECTION_AUTHORITY";

/// Registry leaf evidence collected before its immutable source tree is staged.
struct PendingRegistryLeaf {
    selected_unit_identity: Option<String>,
    package: String,
    version: String,
    crate_name: String,
    features: Vec<String>,
    artifact: OvenRustcArtifactExtern,
    registry: String,
    checksum: String,
    source_root: PathBuf,
    target_artifact: bool,
}

/// One external package selected by the frozen compiler unit graph for a sealed foundation publication.
///
/// Workspace sources intentionally do not appear here: Oven materializes those as caller-owned direct-Rustc
/// libraries or procedural macros. The eventual Cargo invocation therefore need not build the compiler root merely
/// to recover its third-party closure.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CompilerSuiteFoundationDependency {
    alias: String,
    package: String,
    version: String,
    source: Option<String>,
    features: Vec<String>,
    path: Option<PathBuf>,
}

/// Evidence retained in the immutable closure proving the only Cargo use was at the named publisher boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct OvenLegacyCargoProvenance {
    schema_version: u32,
    boundary: String,
    cargo_version: String,
    cargo_manifest_digest: String,
    cargo_lock_digest: String,
    publication_kind: OvenLegacyCargoPublicationKind,
    target: String,
    toolchain: String,
    profile: String,
}

/// Hold a unique private publisher directory and delete it on every normal return or error path.
struct PublisherStagingCleanup {
    path: PathBuf,
}

impl Drop for PublisherStagingCleanup {
    fn drop(&mut self) {
        // A just-killed Cargo child can briefly retain a directory handle on macOS. The staging root is task-owned
        // and guarded by the publisher lock, so retry a few times rather than silently retaining a failed
        // compatibility-domain allocation for later inspection or admission.
        for attempt in 0..3 {
            if fs::remove_dir_all(&self.path).is_ok() || !self.path.exists() {
                return;
            }
            if attempt < 2 {
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Retain an advisory lock for all stale-staging reclamation and one publisher transaction.
struct PublisherLock {
    file: File,
}

impl Drop for PublisherLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Run the explicit Cargo compiler while bounding transient output by the compatibility-domain allowance.
#[allow(clippy::too_many_arguments)]
fn run_legacy_cargo(
    cargo: &Path,
    rustc: &Path,
    cargo_manifest: &Path,
    target: &Path,
    target_triple: &str,
    profile: &str,
    features: &[String],
    transient_limit: u64,
    publication_kind: OvenLegacyCargoPublicationKind,
    compact_debug_info: bool,
    distinct_extension_identities: bool,
) -> Result<Vec<CargoInvocationOutput>, OvenLegacyCargoError> {
    let first = run_legacy_cargo_invocation(
        cargo,
        rustc,
        cargo_manifest,
        target,
        target,
        target_triple,
        profile,
        features,
        transient_limit,
        match publication_kind {
            OvenLegacyCargoPublicationKind::Executable | OvenLegacyCargoPublicationKind::InteropBootstrap => "build",
            OvenLegacyCargoPublicationKind::LibraryTests => "test",
        },
        &match publication_kind {
            // The interop bootstrap alone emits the companion target. Publishing the library validates the exact
            // Rust closure without prematurely linking the package-owned native artifact. On the compiler workspace,
            // whose root is virtual, the same selection builds every member's library tests: their normal and dev
            // dependencies are the closure the direct-rustc roots consume, at the feature sets the workspace test
            // build resolves.
            OvenLegacyCargoPublicationKind::InteropBootstrap | OvenLegacyCargoPublicationKind::LibraryTests => {
                OvenLegacyCargoInvocationTarget::PackageLibrary
            }
            OvenLegacyCargoPublicationKind::Executable => OvenLegacyCargoInvocationTarget::None,
        },
        false,
        compact_debug_info,
        distinct_extension_identities,
    )?;
    let mut outputs = vec![first];
    if publication_kind == OvenLegacyCargoPublicationKind::LibraryTests {
        // The same explicit publisher also materializes the compiler CLI and its own library in the first call's
        // target. Cargo can report the already-built closure as fresh here; the capture accepts those records only
        // because the earlier output in this returned transaction retains their exact traced invocations. No normal
        // test command receives this Cargo authority or target path.
        outputs.push(run_legacy_cargo_invocation(
            cargo,
            rustc,
            cargo_manifest,
            target,
            target,
            target_triple,
            profile,
            features,
            transient_limit,
            "build",
            &OvenLegacyCargoInvocationTarget::CompilerCli,
            false,
            compact_debug_info,
            distinct_extension_identities,
        )?);
    }
    Ok(outputs)
}

/// Captured output from one explicitly named Cargo publisher invocation.
pub struct CargoInvocationOutput {
    stdout: Vec<u8>,
}

impl CargoInvocationOutput {
    /// Decode physical selected-unit facts from this publisher's structured JSON stream.
    pub fn selected_unit_facts(
        &self,
        graph: &CargoUnitGraph,
        metadata: &CargoMetadata,
    ) -> Result<OvenLegacyCargoSelectedUnitCapture, OvenLegacyCargoError> {
        capture_legacy_cargo_selected_units(graph, metadata, std::slice::from_ref(self))
    }
}

/// One publisher-only Cargo target selection used to bake a bounded compiler-suite build unit.
///
/// This never describes a normal Incan command. The resulting executable is discarded; only the dependency
/// artifacts, converted receipt plan, and immutable Oven entry survive the transition.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum OvenLegacyCargoInvocationTarget {
    None,
    PackageLibrary,
    CompilerCli,
    WorkspaceTests,
    /// One package library or proc-macro root. This is intentionally narrower than `--workspace --lib`: a later
    /// Oven shard publisher can own and reclaim this selection without inheriting every workspace library output.
    #[cfg_attr(not(test), allow(dead_code))]
    WorkspacePackageLibrary(String),
    /// One package binary root, retained separately because integration tests can depend on its caller-owned output.
    #[cfg_attr(not(test), allow(dead_code))]
    WorkspacePackageBinary {
        package: String,
        target: String,
    },
    /// One package integration-test root.
    #[cfg_attr(not(test), allow(dead_code))]
    WorkspacePackageIntegrationTest {
        package: String,
        target: String,
    },
    /// One package's Rustdoc roots.
    #[cfg_attr(not(test), allow(dead_code))]
    WorkspacePackageDoctests(String),
}

/// Select the non-workspace package closure that a sealed third-party foundation must provide.
///
/// This consumes the planning-only full-suite unit graph. It deliberately excludes compiler workspace libraries and
/// proc macros: those are the direct-Rustc DAG this Alpha path must bake itself, not a reason to make Cargo compile
/// the `incan` root again. Cargo metadata is used only to turn the opaque graph package identity into an exact
/// package/version dependency in the named publisher manifest.
fn compiler_suite_foundation_dependencies(
    compiler_root: &Path,
    graph: &CargoUnitGraph,
    metadata: &CargoMetadata,
) -> Result<Vec<CompilerSuiteFoundationDependency>, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let packages = metadata
        .packages
        .iter()
        .cloned()
        .map(|package| (package.id.clone(), package))
        .collect::<BTreeMap<_, _>>();
    let mut selected = BTreeMap::<String, (CargoMetadataPackage, BTreeSet<String>)>::new();
    let mut reachable = BTreeSet::new();
    let mut pending = graph.roots.clone();
    while let Some(index) = pending.pop() {
        if !reachable.insert(index) {
            continue;
        }
        let unit = graph.units.get(index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite foundation unit graph index {index} is outside its unit list"
            ))
        })?;
        pending.extend(unit.dependencies.iter().map(|dependency| dependency.index));
    }
    for index in reachable {
        let unit = graph.units.get(index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite foundation unit graph index {index} is outside its unit list"
            ))
        })?;
        // `proc_macro` is a Rust compiler/sysroot facility, not a package the explicit publisher may try to fetch
        // or materialize. Custom-build units become available while Cargo builds their selected library package.
        if unit.target.name == "proc_macro"
            || !unit
                .target
                .kind
                .iter()
                .any(|kind| matches!(kind.as_str(), "lib" | "proc-macro"))
        {
            continue;
        }
        let package = packages.get(&unit.pkg_id).cloned().ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite foundation unit `{}` is absent from publisher Cargo metadata",
                unit.pkg_id
            ))
        })?;
        let patch_path = compiler_suite_foundation_patch_path(&compiler_root, &package)?;
        if package.source.is_none() && patch_path.is_none() {
            let manifest = verified_regular_file(&package.manifest_path, "compiler workspace Cargo.toml")?;
            if manifest.starts_with(&compiler_root) {
                // Compiler sources—including ordinary workspace crates—remain caller-owned direct-Rustc nodes.
                continue;
            }
            return Err(OvenLegacyCargoError::Plan(format!(
                "compiler-suite path dependency `{}` is outside the compiler root or approved loaves/third_party patch directory",
                package.name
            )));
        }
        if package.source.is_some() {
            let source = package.source.as_deref().ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "compiler-suite external unit `{}` has no immutable registry source or approved third-party patch path",
                    package.name
                ))
            })?;
            if !source.starts_with("registry+") {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "compiler-suite external unit `{}` uses unsupported source `{source}`; a sealed Alpha foundation requires an explicit registry source or approved third-party patch",
                    package.name,
                )));
            }
        }
        let entry = selected
            .entry(package.id.clone())
            .or_insert_with(|| (package, BTreeSet::new()));
        entry.1.extend(unit.features.iter().cloned());
    }
    let mut dependencies = selected
        .into_values()
        .map(|(package, features)| (package, features.into_iter().collect::<Vec<_>>()))
        .collect::<Vec<_>>();
    dependencies.sort_by(|(left, _), (right, _)| {
        (&left.name, &left.version, &left.id).cmp(&(&right.name, &right.version, &right.id))
    });
    dependencies
        .into_iter()
        .enumerate()
        .map(|(index, (package, features))| {
            let path = compiler_suite_foundation_patch_path(&compiler_root, &package)?;
            Ok(CompilerSuiteFoundationDependency {
                alias: format!("oven_foundation_{index:04}"),
                package: package.name,
                version: package.version,
                source: package.source,
                features,
                path,
            })
        })
        .collect()
}

/// Return the one class of compiler-tree source Cargo may retain in the sealed third-party foundation.
///
/// Registry patches under `loaves/third_party` preserve the checked-in lock graph when an upstream package enables a
/// yanked or otherwise unacceptable optional dependency. They are dependency provenance, never normal Incan source
/// execution: only the named publisher sees this path and Oven retains its verified output thereafter.
fn compiler_suite_foundation_patch_path(
    compiler_root: &Path,
    package: &CargoMetadataPackage,
) -> Result<Option<PathBuf>, OvenLegacyCargoError> {
    if package.source.is_some() {
        return Ok(None);
    }
    let manifest = verified_regular_file(&package.manifest_path, "compiler third-party patch Cargo.toml")?;
    let package_root = manifest.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
        field: "compiler third-party patch Cargo.toml",
        message: format!("{} has no package directory", manifest.display()),
    })?;
    if package_root.starts_with(compiler_root.join("loaves/third_party")) {
        return Ok(Some(package_root.to_path_buf()));
    }
    Ok(None)
}

/// Render the deterministic private manifest for the named third-party foundation publisher.
///
/// This manifest is staging input only. The publisher later retains only independently verified Oven foundation
/// artifacts, never this Cargo project or target directory.
fn compiler_suite_foundation_manifest(
    dependencies: &[CompilerSuiteFoundationDependency],
) -> Result<String, OvenLegacyCargoError> {
    if dependencies.is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "compiler-suite third-party foundation has no external package dependencies".to_string(),
        ));
    }
    let mut manifest = concat!(
        "[package]\n",
        "name = \"oven-compiler-foundation\"\n",
        "version = \"0.0.0\"\n",
        "edition = \"2024\"\n",
        "publish = false\n\n",
        "[workspace]\n\n",
        "[lib]\n",
        "path = \"src/lib.rs\"\n\n",
        "[dependencies]\n"
    )
    .to_string();
    let patch_dependencies = dependencies
        .iter()
        .filter_map(|dependency| dependency.path.as_ref().map(|path| (&dependency.package, path)))
        .collect::<BTreeMap<_, _>>();
    for dependency in dependencies {
        let package = toml::Value::String(dependency.package.clone()).to_string();
        let features = dependency
            .features
            .iter()
            .filter(|feature| feature.as_str() != "default")
            .map(|feature| toml::Value::String(feature.clone()).to_string())
            .collect::<Vec<_>>();
        let default_features = dependency.features.iter().any(|feature| feature == "default");
        manifest.push_str(&format!("{} = {{ package = {package}", dependency.alias));
        match &dependency.path {
            Some(path) => {
                let path = toml::Value::String(path.display().to_string()).to_string();
                manifest.push_str(&format!(", path = {path}"));
            }
            None => {
                let version = toml::Value::String(format!("={}", dependency.version)).to_string();
                manifest.push_str(&format!(", version = {version}"));
            }
        }
        manifest.push_str(&format!(", default-features = {default_features}"));
        if !features.is_empty() {
            manifest.push_str(&format!(", features = [{}]", features.join(", ")));
        }
        manifest.push_str(" }\n");
    }
    if !patch_dependencies.is_empty() {
        manifest.push_str("\n[patch.crates-io]\n");
        for (package, path) in patch_dependencies {
            let path = toml::Value::String(path.display().to_string()).to_string();
            manifest.push_str(&format!("{package} = {{ path = {path} }}\n"));
        }
    }
    // The compiler-suite Loaf baker uses the named receipt profile for every publisher action. The private
    // foundation manifest must declare the same profile rather than silently compiling its sealed dependency
    // artifacts with Cargo's `dev` defaults. Keep this in lockstep with the root manifest and the direct-rustc
    // test-runner contract: this is a publisher-only compatibility setting, never a normal-command fallback.
    manifest.push_str(OVEN_COMPILER_TEST_CARGO_PROFILE_MANIFEST);
    Ok(manifest)
}

/// Stage the private manifest for a future sealed third-party foundation compilation.
///
/// The staging project is deliberately not a normal output and contains no generated compiler root. It is prepared
/// before any compiler-root bootstrap is allowed, then removed with the publisher staging on every failure path.
fn stage_compiler_suite_foundation_manifest(
    staging: &Path,
    manifest: &str,
    source_lock: &Path,
    dependencies: &[CompilerSuiteFoundationDependency],
) -> Result<PathBuf, OvenLegacyCargoError> {
    let root = staging.join("third-party-foundation");
    let source_directory = root.join("src");
    fs::create_dir_all(&source_directory).map_err(|source| OvenLegacyCargoError::Io {
        path: source_directory.clone(),
        source,
    })?;
    let manifest_path = root.join("Cargo.toml");
    fs::write(&manifest_path, manifest).map_err(|source| OvenLegacyCargoError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let lock_path = root.join("Cargo.lock");
    let lock = compiler_suite_foundation_lock(&regular_file_bytes(source_lock)?, dependencies)?;
    fs::write(&lock_path, lock).map_err(|source| OvenLegacyCargoError::Io {
        path: lock_path,
        source,
    })?;
    let source_path = source_directory.join("lib.rs");
    fs::write(
        &source_path,
        "//! Private Oven third-party foundation publisher root.\n",
    )
    .map_err(|source| OvenLegacyCargoError::Io {
        path: source_path,
        source,
    })?;
    Ok(manifest_path)
}

/// Add the private foundation root to the compiler's already-resolved lock without changing its package graph.
///
/// Copying the compiler lock alone is insufficient: Cargo treats a different root package as an unlocked graph and
/// may select newer transitive versions that happen to exist in the ambient cache. The synthetic root names every
/// exact foundation package selected from the compiler unit graph, after which `--locked` can enforce that no
/// publisher dependency moves independently of the checked compiler lock.
fn compiler_suite_foundation_lock(
    source_lock: &[u8],
    dependencies: &[CompilerSuiteFoundationDependency],
) -> Result<Vec<u8>, OvenLegacyCargoError> {
    let source_lock = std::str::from_utf8(source_lock).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "compiler Cargo.lock",
        message: error.to_string(),
    })?;
    let mut document =
        toml::from_str::<toml::Value>(source_lock).map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "compiler Cargo.lock",
            message: error.to_string(),
        })?;
    let packages = document
        .get_mut("package")
        .and_then(toml::Value::as_array_mut)
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler Cargo.lock",
            message: "must contain a package array".to_string(),
        })?;
    packages.retain(|package| package.get("name").and_then(toml::Value::as_str) != Some("oven-compiler-foundation"));
    let locked_packages = packages
        .iter()
        .filter_map(|package| {
            Some((
                package.get("name")?.as_str()?.to_string(),
                package.get("version")?.as_str()?.to_string(),
                package.get("source").and_then(toml::Value::as_str).map(str::to_string),
            ))
        })
        .collect::<Vec<_>>();
    let mut root_dependencies = Vec::with_capacity(dependencies.len());
    for dependency in dependencies {
        let matching = locked_packages
            .iter()
            .filter(|(name, version, source)| {
                name == &dependency.package && version == &dependency.version && source == &dependency.source
            })
            .count();
        if matching != 1 {
            return Err(OvenLegacyCargoError::Plan(format!(
                "compiler lock must contain exactly one `{}` {} package from {:?}, found {matching}",
                dependency.package, dependency.version, dependency.source
            )));
        }
        let same_name = locked_packages
            .iter()
            .filter(|(name, _, _)| name == &dependency.package)
            .count();
        let same_name_version = locked_packages
            .iter()
            .filter(|(name, version, _)| name == &dependency.package && version == &dependency.version)
            .count();
        let identity = if same_name == 1 {
            dependency.package.clone()
        } else if same_name_version == 1 {
            format!("{} {}", dependency.package, dependency.version)
        } else if let Some(source) = &dependency.source {
            format!("{} {} ({source})", dependency.package, dependency.version)
        } else {
            format!("{} {}", dependency.package, dependency.version)
        };
        root_dependencies.push(toml::Value::String(identity));
    }
    root_dependencies.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    root_dependencies.dedup();
    let mut root = toml::map::Map::new();
    root.insert(
        "name".to_string(),
        toml::Value::String("oven-compiler-foundation".to_string()),
    );
    root.insert("version".to_string(), toml::Value::String("0.0.0".to_string()));
    root.insert("dependencies".to_string(), toml::Value::Array(root_dependencies));
    packages.push(toml::Value::Table(root));
    prune_lock_to_package(&mut document, "oven-compiler-foundation", "0.0.0", None)?;
    toml::to_string_pretty(&document)
        .map(String::into_bytes)
        .map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "compiler Cargo.lock",
            message: error.to_string(),
        })
}

/// Stage a generated Loaf fixture with the compiler's exact registry lock and its explicit local path packages.
///
/// Checked fixture manifests intentionally exercise a subset of the compiler and SDK closure. Letting Cargo resolve
/// that subset from an ambient offline index can choose a newer cached transitive than the compiler itself uses.
/// This helper carries the compiler's locked registry graph forward, adds only the generated root and local path
/// package records Cargo requires, and makes the subsequent named publisher invocation unconditionally locked.
pub fn stage_locked_loaf_fixture(
    cargo: &Path,
    generated_project: &Path,
    compiler_lock: &Path,
) -> Result<(), OvenLegacyCargoError> {
    let generated_project = canonical_directory(generated_project, "generated Loaf fixture")?;
    let manifest_path = generated_project.join("Cargo.toml");
    let manifest = regular_file_bytes(&manifest_path)?;
    let source_lock = regular_file_bytes(compiler_lock)?;
    let lock = locked_generated_project(&manifest_path, &manifest, &source_lock)?;
    let lock_path = generated_project.join("Cargo.lock");
    fs::write(&lock_path, lock).map_err(|source| OvenLegacyCargoError::Io {
        path: lock_path.clone(),
        source,
    })?;

    // Cargo owns local feature unification and therefore the dependency lists attached to path-package lock
    // records. Let it normalize only that local graph while offline, then reject the result unless every registry
    // identity and checksum is a member of the checked compiler lock. All later compilation sees the normalized
    // file and is unconditionally `--locked`.
    let cargo = canonical_tool_file(cargo, "cargo")?;
    let mut command = Command::new(&cargo);
    command
        .current_dir(&generated_project)
        .arg("metadata")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .args(["--offline", "--format-version", "1"]);
    clear_inherited_cargo_environment(&mut command);
    let output = command
        .output()
        .map_err(|source| OvenLegacyCargoError::Io { path: cargo, source })?;
    if !output.status.success() {
        return Err(OvenLegacyCargoError::CargoFailed {
            output: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    validate_generated_registry_lock(&source_lock, &regular_file_bytes(&lock_path)?)
}

/// Seed an explicit project publisher from one release cohort without excluding project-only registry packages.
///
/// The selected release lock pins every retained release identity and dependency edge. Cargo may add project-owned
/// package identities, including alternate versions of packages that also occur in the release graph, but it may not
/// mutate the release graph itself. The normalized lock is written before the actual publisher build, so that build
/// is unconditionally `--locked`.
fn stage_release_cohort_project_lock(
    cargo: &Path,
    generated_project: &Path,
    release_lock: &Path,
    features: &[String],
) -> Result<CargoMetadata, OvenLegacyCargoError> {
    let generated_project = canonical_directory(generated_project, "generated release-cohort project")?;
    let manifest_path = generated_project.join("Cargo.toml");
    let manifest = regular_file_bytes(&manifest_path)?;
    let release_lock_bytes = regular_file_bytes(release_lock)?;
    let lock = release_cohort_generated_project_lock(&manifest_path, &manifest, &release_lock_bytes)?;
    let lock_path = generated_project.join("Cargo.lock");
    fs::write(&lock_path, &lock).map_err(|source| OvenLegacyCargoError::Io {
        path: lock_path.clone(),
        source,
    })?;

    // Normalize local and newly introduced project-only edges while preserving every compatible release pin already
    // present in the seed. This is the sole unlocked metadata invocation and remains inside the explicit baker.
    let metadata = read_legacy_cargo_metadata_with_lock_policy(cargo, &manifest_path, features, false)?;
    validate_release_cohort_registry_lock(&release_lock_bytes, &lock, &regular_file_bytes(&lock_path)?)?;
    Ok(metadata)
}

/// Run one named Cargo publisher invocation while continuously enforcing its enclosing transient allocation allowance.
#[allow(clippy::too_many_arguments)]
fn run_legacy_cargo_invocation(
    cargo: &Path,
    rustc: &Path,
    cargo_manifest: &Path,
    target: &Path,
    capacity_root: &Path,
    target_triple: &str,
    profile: &str,
    features: &[String],
    transient_limit: u64,
    command_name: &'static str,
    target_selection: &OvenLegacyCargoInvocationTarget,
    unit_graph: bool,
    compact_debug_info: bool,
    distinct_extension_identities: bool,
) -> Result<CargoInvocationOutput, OvenLegacyCargoError> {
    let cargo = canonical_tool_file(cargo, "cargo")?;
    let rustc = canonical_tool_file(rustc, "rustc")?;
    let package_root = cargo_manifest
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "Cargo manifest",
            message: format!("{} has no package directory", cargo_manifest.display()),
        })?;
    let mut command = Command::new(&cargo);
    command
        .current_dir(package_root)
        .arg(command_name)
        .arg("--manifest-path")
        .arg(cargo_manifest)
        .arg("--target")
        .arg(target_triple)
        .arg("--target-dir")
        .arg(target)
        .arg("--message-format=json-render-diagnostics");
    // Existing locks are immutable publisher authority. The explicit project publisher alone may resolve a missing
    // first lock; once Cargo writes it, every remaining publisher action is locked and offline. Compiler-root and
    // synthetic foundation projects always arrive with locks and therefore cannot silently re-resolve their graph.
    if package_root.join("Cargo.lock").is_file() {
        command.arg("--offline");
        command.arg("--locked");
    }
    if !features.is_empty() {
        command.arg("--features").arg(features.join(","));
    }
    match target_selection {
        OvenLegacyCargoInvocationTarget::None => {}
        OvenLegacyCargoInvocationTarget::PackageLibrary => {
            command.arg("--lib");
            if command_name == "test" {
                command.arg("--no-run");
            }
        }
        OvenLegacyCargoInvocationTarget::CompilerCli => {
            // The compiler command line is a workspace member of its own; the binary name is the fact this
            // selection knows, never the package that carries it, so the whole workspace is searched for it.
            command.args(["--workspace", "--bin", "incan"]);
        }
        OvenLegacyCargoInvocationTarget::WorkspaceTests => {
            command.args(["--all", "--no-run"]);
        }
        OvenLegacyCargoInvocationTarget::WorkspacePackageLibrary(package) => {
            command.args(["--package", package, "--lib", "--no-run"]);
        }
        OvenLegacyCargoInvocationTarget::WorkspacePackageBinary { package, target } => {
            command.args(["--package", package, "--bin", target, "--no-run"]);
        }
        OvenLegacyCargoInvocationTarget::WorkspacePackageIntegrationTest { package, target } => {
            command.args(["--package", package, "--test", target, "--no-run"]);
        }
        OvenLegacyCargoInvocationTarget::WorkspacePackageDoctests(package) => {
            command.args(["--package", package, "--doc", "--no-run"]);
        }
    }
    if unit_graph {
        command.args(["-Z", "unstable-options", "--unit-graph"]);
    }
    let _ = cargo_profile_directory(profile)?;
    if profile == "release" {
        command.arg("--release");
    } else if profile == OVEN_COMPILER_TEST_PROFILE {
        command.args(["--profile", OVEN_COMPILER_TEST_PROFILE]);
    }
    fs::create_dir_all(target).map_err(|source| OvenLegacyCargoError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    let capture_stem = format!(".oven-cargo-{}-{}", std::process::id(), command_name);
    let stdout_path = target.join(format!("{capture_stem}.stdout"));
    let stderr_path = target.join(format!("{capture_stem}.stderr"));
    let rustc_trace_path = target.join(format!("{capture_stem}.rustc.jsonl"));
    // A unit-graph query describes compilation without executing rustc. Only the later compilation invocation
    // can supply the physical trace; requiring one here rejects the compiler-suite publisher before it builds.
    let rustc_wrapper = if unit_graph {
        None
    } else {
        current_rustc_trace_wrapper()?
    };
    if rustc_wrapper.is_some() {
        match fs::remove_file(&rustc_trace_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(OvenLegacyCargoError::Io {
                    path: rustc_trace_path.clone(),
                    source,
                });
            }
        }
    }
    let stdout = File::create(&stdout_path).map_err(|source| OvenLegacyCargoError::Io {
        path: stdout_path.clone(),
        source,
    })?;
    let stderr = File::create(&stderr_path).map_err(|source| OvenLegacyCargoError::Io {
        path: stderr_path.clone(),
        source,
    })?;
    clear_inherited_cargo_environment(&mut command);
    // ---- Deterministic path remapping for reproducible unit bytes ----
    // Cargo's `-<hash>` extra-filename and rustc's StableCrateId summarize declared unit inputs, so the same locked
    // unit compiles to the same identity on every machine — but the strict version hash also reflects absolute
    // source paths (registry checkouts under the Cargo home, the staged package, the target directory). Left
    // unmapped, a release base baked on one machine and an extension baked on another publish the same identity
    // with different bytes, and rustc refuses to load both halves of that split in one crate graph (colliding
    // StableCrateId values). Remapping every machine-variant root to a stable virtual prefix makes identical units
    // byte-identical everywhere, so shared leaves reconcile by digest instead. RUSTFLAGS do not enter the
    // extra-filename hash, so these per-machine flag strings never fork unit identities.
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")));
    let mut remap_flags: Vec<String> = Vec::new();
    if let Some(cargo_home) = &cargo_home {
        remap_flags.push(format!(
            "--remap-path-prefix={}=/incan/cargo-home",
            cargo_home.display()
        ));
    }
    remap_flags.push(format!("--remap-path-prefix={}=/incan/package", package_root.display()));
    remap_flags.push(format!("--remap-path-prefix={}=/incan/target", target.display()));
    // Standard-library spans leak through inlined core/alloc generics. A toolchain with the `rust-src` component
    // resolves them to its real sysroot checkout while one without emits the virtual `/rustc/<commit>` form, so the
    // same unit compiles to different bytes depending on which components happen to be installed. Remap the source
    // checkout onto the exact virtual form so every toolchain agrees; on a src-less toolchain the prefix never
    // matches and the flag is inert.
    if let Some(toolchain_root) = rustc.parent().and_then(Path::parent)
        && let Some(commit) = oven_rustc::rustc::rustc_commit_hash(&rustc)
    {
        remap_flags.push(format!(
            "--remap-path-prefix={}=/rustc/{commit}",
            toolchain_root.join("lib/rustlib/src/rust").display()
        ));
    }
    // ---- Distinct extension crate identities ----
    // rustc refuses to load two crates with one StableCrateId, and byte-identity across machines is unattainable
    // because rustc folds its own install path into the strict version hash. A project extension therefore salts
    // every unit's `-C metadata` set (rustc hashes all values together with Cargo's own entries), giving extension
    // units StableCrateIds distinct from the sealed base's twins: both copies of a shared interior unit may then
    // legally coexist in one crate graph, and rustc selects each dependent's copy by recorded hash. Declared
    // root-linked crates still substitute onto the base copy by package semantics, so trait identities that can
    // cross the boundary keep unifying. Base loaf bakes never salt — theirs are the canonical identities.
    if distinct_extension_identities {
        remap_flags.push("-C".to_string());
        remap_flags.push("metadata=incan-extension".to_string());
    }
    command.env("CARGO_ENCODED_RUSTFLAGS", remap_flags.join("\u{1f}"));
    command
        .env("RUSTC", &rustc)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    if let Some(rustc_wrapper) = rustc_wrapper.as_ref() {
        command
            .env("RUSTC_WRAPPER", rustc_wrapper)
            .env(OVEN_RUSTC_TRACE_WRAPPER_ENV, "1")
            .env(OVEN_RUSTC_TRACE_PATH_ENV, &rustc_trace_path);
    }
    if compact_debug_info && profile == "debug" {
        command.env("CARGO_PROFILE_DEV_DEBUG", "0");
    }
    isolate_process_group(&mut command);
    let mut child = command.spawn().map_err(|source| OvenLegacyCargoError::Io {
        path: cargo.clone(),
        source,
    })?;
    let output = loop {
        match child.try_wait().map_err(|source| OvenLegacyCargoError::Io {
            path: cargo.clone(),
            source,
        })? {
            Some(_) => {
                break child.wait_with_output().map_err(|source| OvenLegacyCargoError::Io {
                    path: cargo.clone(),
                    source,
                })?;
            }
            None => {
                let scan_started = Instant::now();
                let reservation = if capacity_root.exists() {
                    conservative_directory_reservation(capacity_root)?
                } else {
                    0
                };
                if reservation > transient_limit {
                    terminate_process_group(&mut child).map_err(|source| OvenLegacyCargoError::Io {
                        path: cargo.clone(),
                        source,
                    })?;
                    return Err(OvenLegacyCargoError::TransientCapacityExceeded {
                        path: capacity_root.to_path_buf(),
                        observed_physical_bytes: reservation,
                        limit_bytes: transient_limit,
                    });
                }
                thread::sleep(publisher_capacity_probe_delay(scan_started.elapsed()));
            }
        }
    };
    let mut stdout = fs::read(&stdout_path).map_err(|source| OvenLegacyCargoError::Io {
        path: stdout_path.clone(),
        source,
    })?;
    let stderr = fs::read(&stderr_path).map_err(|source| OvenLegacyCargoError::Io {
        path: stderr_path.clone(),
        source,
    })?;
    let _ = fs::remove_file(&stdout_path);
    let _ = fs::remove_file(&stderr_path);
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&stdout);
        let stderr = String::from_utf8_lossy(&stderr);
        return Err(OvenLegacyCargoError::CargoFailed {
            output: format!("{stdout}\n{stderr}").trim().to_string(),
        });
    }
    if rustc_wrapper.is_some() {
        append_rustc_trace(&mut stdout, &rustc_trace_path)?;
        let _ = fs::remove_file(&rustc_trace_path);
    }
    let reservation = conservative_directory_reservation(capacity_root)?;
    if reservation > transient_limit {
        return Err(OvenLegacyCargoError::TransientCapacityExceeded {
            path: capacity_root.to_path_buf(),
            observed_physical_bytes: reservation,
            limit_bytes: transient_limit,
        });
    }
    Ok(CargoInvocationOutput { stdout })
}

type PublisherArtifactClosure = (
    Vec<String>,
    Vec<OvenRustcArtifactExtern>,
    Vec<OvenRustcSupportingArtifact>,
);

/// Derive direct `--extern` inputs and the complete declared dependency search directory closure.
///
/// A cross-target Cargo build has one target dependency directory plus a host dependency directory for procedural
/// macros. Target artifacts satisfy ordinary generated-program root `--extern` arguments; a host dynamic library can
/// satisfy a procedural-macro root when Cargo emitted no target artifact for that direct dependency.
fn artifact_closure(
    staging: &Path,
    target_deps: &Path,
    dependency_directories: &[PathBuf],
    direct_dependencies: &BTreeMap<String, String>,
    permit_absent_declared_dependencies: bool,
) -> Result<PublisherArtifactClosure, OvenLegacyCargoError> {
    let target_deps = canonical_directory(target_deps, "target Cargo dependency output")?;
    let mut target_files = BTreeMap::new();
    let mut host_dynamic_files = BTreeMap::new();
    let mut supporting = BTreeMap::new();
    let mut dependency_search_paths = Vec::new();
    for directory in dependency_directories {
        let directory = canonical_directory(directory, "Cargo dependency output")?;
        let directory_relative = relative_path(staging, &directory)?;
        let mut contained_artifact = false;
        for entry in fs::read_dir(&directory).map_err(|source| OvenLegacyCargoError::Io {
            path: directory.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| OvenLegacyCargoError::Io {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| OvenLegacyCargoError::Io {
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "Cargo dependency output",
                    message: format!("{} must contain regular non-symlink files only", path.display()),
                });
            }
            let name =
                path.file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                        field: "Cargo dependency output",
                        message: format!("{} has a non-UTF-8 file name", path.display()),
                    })?;
            if !is_direct_rustc_artifact(name) {
                continue;
            }
            contained_artifact = true;
            let digest = digest_bytes(&regular_file_bytes(&path)?);
            let relative_path = format!("{directory_relative}/{name}");
            if directory == target_deps {
                target_files.insert(name.to_string(), (relative_path.clone(), digest.clone()));
            } else if is_dynamic_rustc_artifact(name) {
                // Cargo compiles procedural macros for the host, not the cross target. Keep this separate from
                // target artifacts so a host `.rlib` can never be selected for an ordinary generated-program root.
                host_dynamic_files.insert(name.to_string(), (relative_path.clone(), digest.clone()));
            }
            supporting.insert(relative_path, digest);
        }
        if contained_artifact {
            dependency_search_paths.push(directory_relative);
        }
    }
    let mut externs = Vec::new();
    let mut selected = BTreeSet::new();
    for (dependency, package) in direct_dependencies {
        let Some(artifact) = select_direct_artifact(&target_files, package)
            .or_else(|| select_direct_proc_macro_artifact(&host_dynamic_files, package))
        else {
            if permit_absent_declared_dependencies {
                continue;
            }
            return Err(OvenLegacyCargoError::MissingDirectArtifact {
                crate_name: dependency.clone(),
                path: target_deps.clone(),
            });
        };
        selected.insert(artifact.0.clone());
        externs.push(OvenRustcArtifactExtern {
            crate_name: dependency.clone(),
            relative_path: artifact.0,
            digest: artifact.1,
        });
    }
    let supporting_artifacts = supporting
        .into_iter()
        .filter(|(relative_path, _)| !selected.contains(relative_path))
        .map(|(relative_path, digest)| OvenRustcSupportingArtifact { relative_path, digest })
        .collect();
    Ok((dependency_search_paths, externs, supporting_artifacts))
}

/// Derive one direct-rustc closure from the exact compiler artifacts the explicit Cargo invocation reported.
///
/// Cargo's target layout is an implementation detail: recent Cargo versions can place a dependency library below
/// `target/<triple>/<profile>/build/<package>/<identity>/out`, while older versions use `deps/`. The JSON message
/// stream is the stable publisher authority in both cases. Every path remains confined to private staging, must be
/// a regular non-symlink file, and must have Cargo's crate-and-identity-shaped build-output form before it enters a
/// Loaf.
fn artifact_closure_from_reported_paths(
    staging: &Path,
    target_triple: &str,
    profile: &str,
    direct_dependencies: &BTreeMap<String, ResolvedDirectDependency>,
    permit_absent_declared_dependencies: bool,
    outputs: &[CargoInvocationOutput],
) -> Result<PublisherArtifactClosure, OvenLegacyCargoError> {
    let canonical_staging = canonical_directory(staging, "publisher staging")?;
    let mut target_artifacts = Vec::new();
    let mut host_dynamic_artifacts = Vec::new();
    let mut supporting = BTreeMap::new();
    let mut dependency_search_paths = BTreeSet::new();
    let mut searchable_artifacts = BTreeSet::new();

    for output in outputs {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let Ok(artifact) = serde_json::from_str::<CargoCompilerArtifact>(line) else {
                continue;
            };
            if artifact.reason != "compiler-artifact" {
                continue;
            }
            for path in artifact.filenames {
                let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                    return Err(OvenLegacyCargoError::InvalidInput {
                        field: "Cargo-reported publisher artifact",
                        message: format!("{} has a non-UTF-8 file name", path.display()),
                    });
                };
                if !is_direct_rustc_artifact(file_name)
                    || !cargo_reported_direct_artifact(profile, &artifact.target.name, &path)
                {
                    continue;
                }
                let metadata = fs::symlink_metadata(&path).map_err(|source| OvenLegacyCargoError::Io {
                    path: path.clone(),
                    source,
                })?;
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(OvenLegacyCargoError::InvalidInput {
                        field: "Cargo-reported publisher artifact",
                        message: format!("{} must be a regular non-symlink file", path.display()),
                    });
                }
                let source_path = verified_regular_file(&path, "Cargo-reported publisher artifact")?;
                if !source_path.starts_with(&canonical_staging) {
                    return Err(OvenLegacyCargoError::InvalidInput {
                        field: "Cargo-reported publisher artifact",
                        message: format!("{} escapes publisher staging", source_path.display()),
                    });
                }
                let target_artifact = compiler_artifact_platform(&source_path, target_triple).is_some();
                if !target_artifact && !is_dynamic_rustc_artifact(file_name) {
                    // A cross-target direct-rustc plan must not accidentally retain a host `.rlib`. Host dynamic
                    // artifacts are the only supported host-side inputs because procedural macros execute there.
                    continue;
                }
                let parent = source_path.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                    field: "Cargo-reported publisher artifact",
                    message: format!("{} has no parent directory", source_path.display()),
                })?;
                let artifact_relative_path = relative_path(&canonical_staging, &source_path)?;
                // A `deps` directory or Cargo 1.99's identity-owned `out` directory is flat after Oven materializes
                // its recorded files. A profile root is provenance only, never a Rustc dependency search directory.
                if parent
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| matches!(name, "deps" | "out"))
                {
                    dependency_search_paths.insert(relative_path(&canonical_staging, parent)?);
                    searchable_artifacts.insert(artifact_relative_path.clone());
                }
                let digest = digest_bytes(&regular_file_bytes(&source_path)?);
                let retained = PublisherReportedArtifact {
                    package_id: artifact.package_id.clone(),
                    relative_path: artifact_relative_path.clone(),
                    digest: digest.clone(),
                };
                if target_artifact {
                    target_artifacts.push(retained);
                } else {
                    host_dynamic_artifacts.push(retained);
                }
                if let Some(previous) = supporting.insert(artifact_relative_path.clone(), digest.clone())
                    && previous != digest
                {
                    return Err(OvenLegacyCargoError::InvalidInput {
                        field: "Cargo-reported publisher artifact",
                        message: format!("multiple artifacts share path `{artifact_relative_path}`"),
                    });
                }
            }
        }
    }

    let mut externs = Vec::new();
    let mut selected = BTreeSet::new();
    for (dependency, package) in direct_dependencies {
        let artifact =
            match select_reported_direct_artifact(&target_artifacts, package, &["rlib", "dylib", "so", "dll"])? {
                Some(artifact) => Some(artifact),
                None => select_reported_direct_artifact(&host_dynamic_artifacts, package, &["dylib", "so", "dll"])?,
            };
        let Some(artifact) = artifact else {
            if permit_absent_declared_dependencies {
                continue;
            }
            return Err(OvenLegacyCargoError::MissingDirectArtifact {
                crate_name: dependency.clone(),
                path: canonical_staging.clone(),
            });
        };
        if !searchable_artifacts.contains(&artifact.0) {
            return Err(OvenLegacyCargoError::Plan(format!(
                "named publisher selected direct dependency `{dependency}` from {} outside a sealed Rustc search directory",
                artifact.0
            )));
        }
        selected.insert(artifact.0.clone());
        externs.push(OvenRustcArtifactExtern {
            crate_name: dependency.clone(),
            relative_path: artifact.0,
            digest: artifact.1,
        });
    }
    let supporting_artifacts = supporting
        .into_iter()
        .filter(|(relative_path, _)| !selected.contains(relative_path))
        .map(|(relative_path, digest)| OvenRustcSupportingArtifact { relative_path, digest })
        .collect();
    Ok((
        dependency_search_paths.into_iter().collect(),
        externs,
        supporting_artifacts,
    ))
}

/// One direct-rustc artifact reported by Cargo together with the opaque package identity that emitted it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PublisherReportedArtifact {
    package_id: String,
    relative_path: String,
    digest: String,
}

/// Select the one artifact emitted by Cargo for a particular resolved package instance.
fn select_reported_direct_artifact(
    artifacts: &[PublisherReportedArtifact],
    dependency: &ResolvedDirectDependency,
    extensions: &[&str],
) -> Result<Option<(String, String)>, OvenLegacyCargoError> {
    for extension in extensions {
        let suffix = format!(".{extension}");
        let mut candidates = artifacts
            .iter()
            .filter(|artifact| artifact.package_id == dependency.package_id)
            .filter(|artifact| artifact.relative_path.ends_with(&suffix))
            .map(|artifact| (artifact.relative_path.clone(), artifact.digest.clone()))
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.dedup();
        match candidates.as_slice() {
            [] => continue,
            [artifact] => return Ok(Some(artifact.clone())),
            _ => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "named publisher resolved direct dependency `{}` to multiple `{extension}` artifacts for Cargo package `{}`: {}",
                    dependency.package,
                    dependency.package_id,
                    candidates
                        .iter()
                        .map(|(path, _)| path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
    }
    Ok(None)
}

/// Read every exact direct-rustc artifact from the named Cargo publisher's stable JSON records.
fn publisher_output_artifact_paths(
    outputs: &[CargoInvocationOutput],
    profile: &str,
) -> Result<Vec<PathBuf>, OvenLegacyCargoError> {
    let mut paths = BTreeSet::new();
    for output in outputs {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let Ok(artifact) = serde_json::from_str::<CargoCompilerArtifact>(line) else {
                continue;
            };
            if artifact.reason != "compiler-artifact" {
                continue;
            }
            for path in artifact.filenames {
                if !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_direct_rustc_artifact)
                    || !cargo_reported_direct_artifact(profile, &artifact.target.name, &path)
                {
                    continue;
                }
                let path = fs::canonicalize(&path).map_err(|source| OvenLegacyCargoError::Io {
                    path: path.clone(),
                    source,
                })?;
                paths.insert(path);
            }
        }
    }
    Ok(paths.into_iter().collect())
}

/// Select one `.rlib` or dynamic-library artifact for a direct dependency from a fresh Cargo target directory.
fn select_direct_artifact(files: &BTreeMap<String, (String, String)>, dependency: &str) -> Option<(String, String)> {
    select_direct_artifact_with_extensions(files, dependency, &["rlib", "dylib", "so", "dll"])
}

/// Select a host-built procedural macro only after no target artifact can satisfy the direct dependency.
fn select_direct_proc_macro_artifact(
    files: &BTreeMap<String, (String, String)>,
    dependency: &str,
) -> Option<(String, String)> {
    select_direct_artifact_with_extensions(files, dependency, &["dylib", "so", "dll"])
}

/// Select one artifact with an allowed direct-rustc extension for a dependency name.
fn select_direct_artifact_with_extensions(
    files: &BTreeMap<String, (String, String)>,
    dependency: &str,
    extensions: &[&str],
) -> Option<(String, String)> {
    // Cargo generally replaces package hyphens with underscores in an artifact crate name, but packages may expose a
    // different library name (for example `md-5` exposes `md5`). Preserve both deterministic normalizations until
    // the publisher gains a Cargo-unit-graph reader of its own.
    let crate_names = [dependency.replace('-', "_"), dependency.replace('-', "")];
    crate_names.into_iter().find_map(|crate_name| {
        let prefix = format!("lib{crate_name}-");
        extensions.iter().find_map(|extension| {
            files
                .keys()
                .find(|name| name.starts_with(&prefix) && name.ends_with(&format!(".{extension}")))
                .and_then(|name| files.get(name))
                .cloned()
        })
    })
}

/// Return whether an artifact filename is a dynamically loaded rustc library.
fn is_dynamic_rustc_artifact(name: &str) -> bool {
    [".dylib", ".so", ".dll"]
        .iter()
        .any(|extension| name.ends_with(extension))
}

/// Retain direct Rustc crate inputs, never Cargo's object files, dep-info files, or a prior executable.
///
/// `.rmeta` sidecars are load-bearing, not redundant: since Rust 1.98 Cargo can emit an `.rlib` that carries only a
/// metadata stub (an archive with a `lib.rmeta-link` member), with the crate's real metadata living solely in the
/// sibling `.rmeta` file that rustc discovers next to the rlib. Dropping the sidecar makes every direct-rustc
/// consumer of such a closure fail with E0463 "can't find crate", so the sealed closure must ship both files.
/// Extern selection still names only `.rlib`/dynamic libraries; the sidecar rides along as a supporting artifact.
fn is_direct_rustc_artifact(name: &str) -> bool {
    [".rlib", ".rmeta", ".dylib", ".so", ".dll", ".a", ".lib"]
        .iter()
        .any(|extension| name.ends_with(extension))
}

/// Identify Cargo's `oven-test/build/<package>/<identity>/out` output layout.
///
/// An arbitrary file below this path is still build-script implementation detail, never a direct-Rustc crate input.
/// Newer Cargo releases can report a real compiler artifact in the same layout, so callers admitting a Cargo JSON
/// record must additionally prove its package, target, and identity-shaped filename in
/// `compiler_suite_cargo_reported_direct_artifact`. The compiler-suite publisher always uses the named `oven-test`
/// profile, so fence the match to that profile rather than treating an unrelated checkout component named `build` as
/// a Cargo implementation detail. Cargo can add an identity directory between the package and `out`, so require the
/// final parent to be `out` instead of assuming a fixed number of path components.
fn cargo_reported_build_output(profile: &str, path: &Path) -> bool {
    let components = path.components().collect::<Vec<_>>();
    let Some((_, parent_components)) = components.split_last() else {
        return false;
    };
    parent_components
        .last()
        .is_some_and(|component| component.as_os_str() == "out")
        && parent_components
            .windows(2)
            .any(|components| components[0].as_os_str() == profile && components[1].as_os_str() == "build")
}

/// Identify compiler-suite `oven-test/build/<package>/<identity>/out` output without duplicating the generic
/// publisher predicate used by ordinary project Loafs.
fn compiler_suite_cargo_build_output(path: &Path) -> bool {
    cargo_reported_build_output(OVEN_COMPILER_TEST_PROFILE, path)
}

/// Accept one direct-Rustc compiler artifact Cargo explicitly reported from its newer build-output layout.
///
/// Cargo 1.99 writes normal dependency libraries as
/// `oven-test/build/<package>/<identity>/out/lib<target>-<identity>.<linker-extension>`. A build script may also
/// write files below `out`, but it cannot pass this target-and-identity-shaped filename test unless Cargo has
/// presented it as that target's compiler artifact. The package directory need not match the library target (for
/// example, package `coreaudio-rs` emits target `coreaudio`). Directory scans keep rejecting all `build` output;
/// this predicate is used only for the JSON-recorded file list from the named publisher.
fn cargo_reported_direct_artifact(profile: &str, target_name: &str, path: &Path) -> bool {
    if !cargo_reported_build_output(profile, path) {
        return true;
    }
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(parent) = path.parent() else {
        return false;
    };
    let parent_components = parent.components().collect::<Vec<_>>();
    let Some(out_index) = parent_components
        .iter()
        .rposition(|component| component.as_os_str() == "out")
    else {
        return false;
    };
    if out_index + 1 != parent_components.len() || out_index < 3 {
        return false;
    }
    let identity = parent_components[out_index - 1].as_os_str().to_str();
    let Some(identity) = identity else {
        return false;
    };
    if identity.is_empty() {
        return false;
    }
    [".rlib", ".dylib", ".so", ".dll", ".a", ".lib"]
        .iter()
        .any(|extension| file_name == format!("lib{}-{identity}{extension}", target_name.replace('-', "_")))
}

/// Keep compiler-suite callers on the named profile while sharing the exact build-output proof with project Loafs.
fn compiler_suite_cargo_reported_direct_artifact(target_name: &str, path: &Path) -> bool {
    cargo_reported_direct_artifact(OVEN_COMPILER_TEST_PROFILE, target_name, path)
}

/// Read direct dependency aliases from the generated root `Cargo.toml` without asking Cargo for metadata.
fn cargo_direct_dependency_names(
    cargo_manifest: &[u8],
    include_dev_dependencies: bool,
) -> Result<BTreeMap<String, String>, OvenLegacyCargoError> {
    let content = std::str::from_utf8(cargo_manifest).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "Cargo.toml",
        message: format!("must be UTF-8: {error}"),
    })?;
    let document = toml::from_str::<toml::Value>(content).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "Cargo.toml",
        message: format!("must be valid TOML: {error}"),
    })?;
    let mut dependencies = direct_dependency_aliases(&document, "dependencies");
    if include_dev_dependencies {
        dependencies.extend(direct_dependency_aliases(&document, "dev-dependencies"));
    }
    Ok(dependencies)
}

/// Return Cargo dependency aliases mapped to the package artifact name Cargo emits into `target/.../deps`.
fn direct_dependency_aliases(document: &toml::Value, section: &str) -> BTreeMap<String, String> {
    document
        .get(section)
        .and_then(toml::Value::as_table)
        .map(|dependencies| {
            dependencies
                .iter()
                .map(|(alias, specification)| {
                    let package = specification
                        .as_table()
                        .and_then(|table| table.get("package"))
                        .and_then(toml::Value::as_str)
                        .unwrap_or(alias)
                        .to_string();
                    // Cargo package keys may contain hyphens, while both Rust paths and `rustc --extern` use the
                    // corresponding underscore identifier (`wasmtime-wasi` -> `wasmtime_wasi`).
                    (alias.replace('-', "_"), package)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Keep generated-code roots and documented compiler-owned macro-expansion roots for direct rustc.
///
/// Cargo compiles src/main.rs or src/lib.rs together with every nested generated module. Restricting this scan to
/// the root silently omits dependencies used only by a child module, such as the compiler-owned incan_derive
/// procedural macro emitted for a model declaration. The explicit publisher therefore reads every regular .rs file
/// below the generated src tree before it freezes the immutable plan. A proc macro can also create root paths after
/// that scan, so documented compiler-owned macro expansion roots are retained from the same declared manifest.
/// Normal Oven consumers still inspect neither this generated Cargo project nor any Cargo target directory.
fn generated_project_direct_dependencies(
    generated_project: &Path,
    declared_dependencies: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, OvenLegacyCargoError> {
    let source_root = canonical_directory(&generated_project.join("src"), "generated Rust source tree")?;
    let mut pending = vec![source_root];
    let mut generated_sources = String::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(|source| OvenLegacyCargoError::Io {
            path: directory.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| OvenLegacyCargoError::Io {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| OvenLegacyCargoError::Io {
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "generated Rust source tree",
                    message: format!("{} must not contain symlinks", path.display()),
                });
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "generated Rust source tree",
                    message: format!("{} must contain only regular files and directories", path.display()),
                });
            }
            if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                let source = regular_file_bytes(&path)?;
                let source = std::str::from_utf8(&source).map_err(|error| OvenLegacyCargoError::InvalidInput {
                    field: "generated Rust source",
                    message: format!("{} must be UTF-8: {error}", path.display()),
                })?;
                generated_sources.push_str(source);
                generated_sources.push('\n');
            }
        }
    }
    let mut selected = declared_dependencies
        .iter()
        .filter(|(dependency, _)| {
            let crate_name = dependency.replace('-', "_");
            generated_sources.contains(&format!("{crate_name}::"))
        })
        .map(|(alias, package)| (alias.clone(), package.clone()))
        .collect::<BTreeMap<_, _>>();
    for (marker, expansion_roots) in GENERATED_PROC_MACRO_EXPANSION_ROOTS {
        if !generated_sources.contains(marker) {
            continue;
        }
        for crate_name in *expansion_roots {
            let package = declared_dependencies.get(*crate_name).ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "generated macro {marker} requires declared direct dependency {crate_name}"
                ))
            })?;
            selected.insert((*crate_name).to_string(), package.clone());
        }
    }
    Ok(selected)
}

/// Choose the bounded manifest dependencies that become named `rustc --extern` inputs.
///
/// A regular generated root needs only source-reachable crates. Library tests and complete standard-library Loafs
/// compile source trees beyond their small root, so their checked manifest is the authoritative complete direct
/// dependency surface. Cargo may omit a conditionally declared package; the artifact reader preserves that
/// compatibility behavior by tolerating the corresponding absent artifact where appropriate.
fn publisher_direct_dependencies(
    generated_project: &Path,
    declared_dependencies: BTreeMap<String, String>,
    publication_kind: OvenLegacyCargoPublicationKind,
    closure: OvenLegacyCargoDirectDependencyClosure,
) -> Result<BTreeMap<String, String>, OvenLegacyCargoError> {
    if publication_kind == OvenLegacyCargoPublicationKind::LibraryTests
        || closure == OvenLegacyCargoDirectDependencyClosure::CheckedDeclared
    {
        return Ok(declared_dependencies);
    }
    generated_project_direct_dependencies(generated_project, &declared_dependencies)
}

/// Compiler-owned procedural macros whose generated Rust introduces root crate paths absent from pre-expansion source.
///
/// This is deliberately a finite publisher contract, rather than inferring arbitrary third-party macro behavior or
/// broadening a native plan to every Cargo dependency. Each root must already be a declared generated-project direct
/// dependency, then passes the same sealed artifact/digest path as ordinary direct externs.
const GENERATED_PROC_MACRO_EXPANSION_ROOTS: &[(&str, &[&str])] =
    &[("#[incan_web_macros::route", &["inventory", "axum"])];

/// Write portable publisher provenance inside the temporary closure without retaining local paths.
fn write_provenance(path: &Path, provenance: &OvenLegacyCargoProvenance) -> Result<(), OvenLegacyCargoError> {
    let parent = path.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
        field: "provenance path",
        message: "must have a parent directory".to_string(),
    })?;
    fs::create_dir_all(parent).map_err(|source| OvenLegacyCargoError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let payload =
        serde_json::to_vec_pretty(provenance).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
    fs::write(path, payload).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Create and lock the publisher staging root before reclaiming stale successful-or-interrupted staging directories.
fn acquire_publisher_lock(parent: &Path) -> Result<PublisherLock, OvenLegacyCargoError> {
    fs::create_dir_all(parent).map_err(|source| OvenLegacyCargoError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let path = parent.join(".publisher.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|source| OvenLegacyCargoError::Io { path, source })?;
    file.lock().map_err(|source| OvenLegacyCargoError::Io {
        path: parent.join(".publisher.lock"),
        source,
    })?;
    Ok(PublisherLock { file })
}

/// Reclaim only publisher-owned stale directories while the publisher lock is held.
fn reclaim_stale_publisher_staging(parent: &Path) -> Result<(), OvenLegacyCargoError> {
    for entry in fs::read_dir(parent).map_err(|source| OvenLegacyCargoError::Io {
        path: parent.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| OvenLegacyCargoError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(".legacy-cargo-") {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(&path).map_err(|source| OvenLegacyCargoError::Io { path, source })?;
        }
    }
    Ok(())
}

/// Allocate a collision-resistant publisher-owned staging root below the Oven store.
fn create_publisher_staging(parent: &Path) -> Result<PathBuf, OvenLegacyCargoError> {
    let parent = fs::canonicalize(parent).map_err(|source| OvenLegacyCargoError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "system clock",
            message: error.to_string(),
        })?
        .as_nanos();
    for sequence in 0_u32..128 {
        let path = parent.join(format!(".legacy-cargo-{}-{timestamp}-{sequence}", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(OvenLegacyCargoError::Io { path, source }),
        }
    }
    Err(OvenLegacyCargoError::InvalidInput {
        field: "publisher staging",
        message: "could not allocate a unique staging directory".to_string(),
    })
}

/// Read a non-symlink regular file in the publisher closure.
fn regular_file_bytes(path: &Path) -> Result<Vec<u8>, OvenLegacyCargoError> {
    let path = verified_regular_file(path, "publisher input")?;
    fs::read(&path).map_err(|source| OvenLegacyCargoError::Io { path, source })
}

/// Locate the generated Cargo entrypoint authorized by the receipt without assuming a binary target.
///
/// Normal Oven build/run uses `src/main.rs`; the native test harness is a library target at `src/lib.rs`. Both are
/// compatible inputs for the explicit publisher, but exactly one must match the receipt-bound generated-root digest.
fn receipt_authorized_generated_root_bytes(
    generated_project: &Path,
    receipt: &OvenReceipt,
    source_evidence_key: &str,
) -> Result<Vec<u8>, OvenLegacyCargoError> {
    let expected = receipt
        .sources
        .supplemental_digests
        .get(source_evidence_key)
        .ok_or_else(|| OvenLegacyCargoError::ReceiptMismatch {
            message: format!("receipt does not declare {source_evidence_key} source evidence"),
        })?;
    // The compiler workspace has no generated root: its receipt names the workspace manifest, and that is the one
    // file the library-tests publication is bound to.
    if receipt.compatibility.kind == OvenCompatibilityKind::NativeCompilerTestSuite {
        let manifest = generated_project.join("Cargo.toml");
        let bytes = regular_file_bytes(&manifest)?;
        if &digest_bytes(&bytes) != expected {
            return Err(OvenLegacyCargoError::ReceiptMismatch {
                message: format!("Cargo.toml does not match {source_evidence_key} receipt digest {expected}"),
            });
        }
        return Ok(bytes);
    }
    let mut matching = Vec::new();
    for relative in ["src/main.rs", "src/lib.rs"] {
        let candidate = generated_project.join(relative);
        if !candidate.exists() {
            continue;
        }
        let bytes = regular_file_bytes(&candidate)?;
        let actual = digest_bytes(&bytes);
        if &actual == expected {
            matching.push((candidate, bytes));
        }
    }
    match matching.len() {
        1 => Ok(matching.remove(0).1),
        0 => Err(OvenLegacyCargoError::ReceiptMismatch {
            message: format!(
                "none of src/main.rs or src/lib.rs matches {source_evidence_key} receipt digest {expected}"
            ),
        }),
        _ => Err(OvenLegacyCargoError::ReceiptMismatch {
            message: "multiple generated roots match the receipt; publisher refuses an ambiguous target".to_string(),
        }),
    }
}

/// Validate one non-symlink regular file and return its canonical path for subsequent containment checks.
///
/// The non-symlink check happens before canonicalization, so a caller cannot smuggle a link through the publisher
/// boundary. Returning the canonical form keeps path containment sound on platforms that expose the same temporary
/// directory through aliases such as macOS `/var` and `/private/var`.
fn verified_regular_file(path: &Path, field: &'static str) -> Result<PathBuf, OvenLegacyCargoError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field,
            message: format!("{} must be a regular non-symlink file", path.display()),
        });
    }
    fs::canonicalize(path).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Validate one non-symlink directory and canonicalize it for publisher input isolation.
fn canonical_directory(path: &Path, field: &'static str) -> Result<PathBuf, OvenLegacyCargoError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field,
            message: format!("{} must be a non-symlink directory", path.display()),
        });
    }
    fs::canonicalize(path).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Read the named tool's stable version report without relying on a shell or Cargo metadata.
fn tool_version(path: &Path, field: &'static str) -> Result<String, OvenLegacyCargoError> {
    let path = canonical_tool_file(path, field)?;
    let output = Command::new(&path)
        .arg("--version")
        .output()
        .map_err(|source| OvenLegacyCargoError::Io {
            path: path.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field,
            message: "must report a successful --version identity".to_string(),
        });
    }
    let version = String::from_utf8(output.stdout).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field,
        message: format!("reported non-UTF-8 --version output: {error}"),
    })?;
    let version = version.trim();
    if version.is_empty() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field,
            message: "reported an empty --version identity".to_string(),
        });
    }
    Ok(version.to_string())
}

/// Validate a conventional toolchain-manager entrypoint while preserving its original executable name.
///
/// `cargo` is commonly a `rustup-init` symlink. Canonicalizing and executing its terminal file changes `argv[0]` to
/// `rustup-init`, so that shim no longer dispatches Cargo subcommands. Verify the terminal regular file, but return
/// the absolute caller entrypoint so `Command` retains the approved `cargo` or `rustc` invocation identity.
fn canonical_tool_file(path: &Path, field: &'static str) -> Result<PathBuf, OvenLegacyCargoError> {
    let invocation = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| OvenLegacyCargoError::Io {
                path: path.to_path_buf(),
                source,
            })?
            .join(path)
    };
    let canonical = fs::canonicalize(path).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let _ = verified_regular_file(&canonical, field)?;
    Ok(invocation)
}

/// Return a safe path below `root` with forward slashes for an Oven manifest.
fn relative_path(root: &Path, path: &Path) -> Result<String, OvenLegacyCargoError> {
    let root = canonical_directory(root, "artifact root")?;
    let path = fs::canonicalize(path).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| OvenLegacyCargoError::InvalidInput {
            field: "artifact path",
            message: format!("{} is not below {}", path.display(), root.display()),
        })?;
    let value = relative.to_string_lossy().replace('\\', "/");
    if value.is_empty() || value.starts_with('/') || value.contains("../") {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "artifact path",
            message: format!("{} is not a portable relative artifact path", relative.display()),
        });
    }
    Ok(value)
}

/// Copy one shard's verified closure out of a disposable Cargo target before the next selection starts.
///
/// The publisher must not keep every selection target alive until `OvenStore::publish_batch` can receive the final
/// suite. Each selection instead gets a private prepared directory below the publisher staging root. The returned
/// paths preserve the immutable entry-relative names used by the shard payload, while their sources now point at the
/// publisher-owned prepared copy. That lets the caller reclaim the closed Cargo target without turning the prepared
/// suite into a Cargo cache.
#[cfg(test)]
fn stage_compiler_suite_shard_files(
    staging: &Path,
    shard_index: usize,
    materialized_files: &[OvenArtifactMaterializedFile],
) -> Result<Vec<OvenArtifactMaterializedFile>, OvenLegacyCargoError> {
    stage_compiler_suite_files_at(
        &staging.join("prepared-shards").join(format!("{shard_index:04}")),
        materialized_files,
    )
}

/// Link a verified direct-rustc closure into one newly-created publisher-owned prepared directory.
///
/// The prepared directory and disposable selection target are siblings below the same Oven-owned store root. A hard
/// link therefore survives reclamation of the Cargo target without allocating a second physical copy, while the
/// final bounded store publisher still verifies and owns its immutable files. This is not a Cargo cache: the target
/// link is removed immediately after staging and normal commands never receive either path.
#[cfg(test)]
fn stage_compiler_suite_files_at(
    prepared_root: &Path,
    materialized_files: &[OvenArtifactMaterializedFile],
) -> Result<Vec<OvenArtifactMaterializedFile>, OvenLegacyCargoError> {
    let prepared_parent = prepared_root
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite prepared artifact",
            message: format!("{} has no parent staging directory", prepared_root.display()),
        })?;
    fs::create_dir_all(prepared_parent).map_err(|source| OvenLegacyCargoError::Io {
        path: prepared_parent.to_path_buf(),
        source,
    })?;
    fs::create_dir(prepared_root).map_err(|source| OvenLegacyCargoError::Io {
        path: prepared_root.to_path_buf(),
        source,
    })?;

    let staged = (|| {
        let mut staged = Vec::with_capacity(materialized_files.len());
        let mut relative_paths = BTreeSet::new();
        for materialized in materialized_files {
            let source = verified_regular_file(&materialized.source_path, "compiler-suite shard artifact")?;
            let relative = compiler_suite_shard_relative_path(&materialized.relative_path)?;
            if !relative_paths.insert(materialized.relative_path.clone()) {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "compiler-suite shard artifact",
                    message: format!(
                        "declares duplicate prepared artifact path {}",
                        materialized.relative_path
                    ),
                });
            }
            let destination = prepared_root.join(&relative);
            let destination_parent = destination.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite shard artifact",
                message: format!("cannot derive parent for prepared artifact {}", destination.display()),
            })?;
            fs::create_dir_all(destination_parent).map_err(|source_error| OvenLegacyCargoError::Io {
                path: destination_parent.to_path_buf(),
                source: source_error,
            })?;
            match fs::symlink_metadata(&destination) {
                Ok(_) => {
                    return Err(OvenLegacyCargoError::InvalidInput {
                        field: "compiler-suite shard artifact",
                        message: format!("prepared artifact path already exists: {}", destination.display()),
                    });
                }
                Err(source_error) if source_error.kind() == io::ErrorKind::NotFound => {}
                Err(source_error) => {
                    return Err(OvenLegacyCargoError::Io {
                        path: destination,
                        source: source_error,
                    });
                }
            }
            fs::hard_link(&source, &destination).map_err(|source_error| OvenLegacyCargoError::Io {
                path: destination.clone(),
                source: source_error,
            })?;
            staged.push(OvenArtifactMaterializedFile {
                source_path: destination,
                relative_path: materialized.relative_path.clone(),
            });
        }
        Ok(staged)
    })();
    if staged.is_err() {
        let _ = fs::remove_dir_all(prepared_root);
    }
    staged
}

/// Validate the portable name that will be recreated below a publisher-owned prepared-shard directory.
#[cfg(test)]
fn compiler_suite_shard_relative_path(value: &str) -> Result<PathBuf, OvenLegacyCargoError> {
    let path = Path::new(value);
    if value.trim().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite shard artifact path",
            message: "must be a non-empty normalized relative path".to_string(),
        });
    }
    Ok(path.to_path_buf())
}

/// Drop Cargo-only files from the publisher's private target after the direct-rustc closure is fully declared.
///
/// The target directory is created beneath `legacy-cargo-staging` by this publisher and is no longer observed by
/// Cargo at this point. Keeping only source paths passed to `OvenStore::publish` prevents temporary object files
/// from consuming a second compatibility-domain allocation while the immutable store entry is copied and verified.
fn reclaim_unmaterialized_compiler_suite_target_files(
    target: &Path,
    materialized_files: &[OvenArtifactMaterializedFile],
) -> Result<(), OvenLegacyCargoError> {
    let target = canonical_directory(target, "compiler-suite transient target")?;
    let mut retained = BTreeSet::new();
    for materialized in materialized_files {
        let source = verified_regular_file(&materialized.source_path, "compiler-suite retained artifact")?;
        if source.starts_with(&target) {
            retained.insert(source);
        }
    }
    reclaim_unmaterialized_compiler_suite_target_directory(&target, &target, &retained)
}

/// Recursively reclaim a closed publisher target without following links or crossing its staging boundary.
fn reclaim_unmaterialized_compiler_suite_target_directory(
    target: &Path,
    directory: &Path,
    retained: &BTreeSet<PathBuf>,
) -> Result<(), OvenLegacyCargoError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|source| OvenLegacyCargoError::Io {
            path: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenLegacyCargoError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenLegacyCargoError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            // This removes the link itself; it never follows a Cargo-produced link outside publisher staging.
            fs::remove_file(&path).map_err(|source| OvenLegacyCargoError::Io { path, source })?;
            continue;
        }
        if metadata.is_dir() {
            reclaim_unmaterialized_compiler_suite_target_directory(target, &path, retained)?;
            let mut remaining = fs::read_dir(&path).map_err(|source| OvenLegacyCargoError::Io {
                path: path.clone(),
                source,
            })?;
            match remaining.next() {
                None => {
                    fs::remove_dir(&path).map_err(|source| OvenLegacyCargoError::Io {
                        path: path.clone(),
                        source,
                    })?;
                }
                Some(Ok(_)) => {}
                Some(Err(source)) => {
                    return Err(OvenLegacyCargoError::Io { path, source });
                }
            }
            continue;
        }
        if !metadata.is_file() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite transient target",
                message: format!("{} is neither a regular file nor directory", path.display()),
            });
        }
        let canonical = fs::canonicalize(&path).map_err(|source| OvenLegacyCargoError::Io {
            path: path.clone(),
            source,
        })?;
        if !canonical.starts_with(target) {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite transient target",
                message: format!("{} escapes publisher staging", canonical.display()),
            });
        }
        if !retained.contains(&canonical) {
            fs::remove_file(&path).map_err(|source| OvenLegacyCargoError::Io { path, source })?;
        }
    }
    Ok(())
}

/// Measure a conservative physical reservation for transient staging without following links or relying on logical
/// bytes as physical allocation.
///
/// Cargo can create short-lived symlinks in its target directory on platforms that expose dynamic libraries. They
/// count as one allocation apiece here, but their targets are never followed. Later artifact admission still rejects
/// every symlink, so no link can become a retained Oven input.
#[cfg(unix)]
pub fn conservative_directory_reservation(root: &Path) -> Result<u64, OvenLegacyCargoError> {
    let mut seen_files = BTreeSet::new();
    conservative_directory_reservation_with_seen_files(root, &mut seen_files)
}

/// Treat hard-linked publisher inputs as one physical allocation while still rejecting links that could escape the
/// explicit publisher root. Prepared shard roots use this after their disposable selection target has been reclaimed.
#[cfg(unix)]
fn conservative_directory_reservation_with_seen_files(
    root: &Path,
    seen_files: &mut BTreeSet<(u64, u64)>,
) -> Result<u64, OvenLegacyCargoError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::symlink_metadata(root).map_err(|source| OvenLegacyCargoError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Ok(round_physical(metadata.len()));
    }
    if metadata.is_file() {
        let identity = (metadata.dev(), metadata.ino());
        return Ok(if seen_files.insert(identity) {
            round_physical(metadata.len())
        } else {
            0
        });
    }
    if !metadata.is_dir() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "publisher staging",
            message: format!("{} is neither a regular file nor directory", root.display()),
        });
    }
    let mut total = 4096_u64;
    for entry in fs::read_dir(root).map_err(|source| OvenLegacyCargoError::Io {
        path: root.to_path_buf(),
        source,
    })? {
        let entry = match entry {
            Ok(entry) => entry,
            // Cargo atomically replaces transient archives while publishing a fresh dependency. The next quota poll
            // sees the replacement; this one must not fail the publisher merely because the old directory entry lost
            // the race after `read_dir` yielded it.
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(OvenLegacyCargoError::Io {
                    path: root.to_path_buf(),
                    source,
                });
            }
        };
        let path = entry.path();
        match conservative_directory_reservation_with_seen_files(&path, seen_files) {
            Ok(reservation) => total = total.saturating_add(reservation),
            Err(OvenLegacyCargoError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(total)
}

/// Platforms without inode identity retain conservative per-directory accounting.
#[cfg(not(unix))]
pub fn conservative_directory_reservation(root: &Path) -> Result<u64, OvenLegacyCargoError> {
    let metadata = fs::symlink_metadata(root).map_err(|source| OvenLegacyCargoError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Ok(round_physical(metadata.len()));
    }
    if metadata.is_file() {
        return Ok(round_physical(metadata.len()));
    }
    if !metadata.is_dir() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "publisher staging",
            message: format!("{} is neither a regular file nor directory", root.display()),
        });
    }
    let mut total = 4096_u64;
    for entry in fs::read_dir(root).map_err(|source| OvenLegacyCargoError::Io {
        path: root.to_path_buf(),
        source,
    })? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(OvenLegacyCargoError::Io {
                    path: root.to_path_buf(),
                    source,
                });
            }
        };
        let path = entry.path();
        match conservative_directory_reservation(&path) {
            Ok(reservation) => total = total.saturating_add(reservation),
            Err(OvenLegacyCargoError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(total)
}

/// Refuse prepared publisher staging once its retained shard/index inputs exceed the compatibility-domain allowance.
///
/// A selection target has already been reclaimed when this runs, so the measurement is the bounded immutable batch
/// input rather than private Cargo cache state. The final store batch performs its own physical measurement and is the
/// only operation that makes the artifacts visible.
fn enforce_compiler_suite_prepared_staging_capacity(
    staging: &Path,
    transient_limit: u64,
) -> Result<(), OvenLegacyCargoError> {
    let reservation = conservative_directory_reservation(staging)?;
    if reservation > transient_limit {
        return Err(OvenLegacyCargoError::TransientCapacityExceeded {
            path: staging.to_path_buf(),
            observed_physical_bytes: reservation,
            limit_bytes: transient_limit,
        });
    }
    Ok(())
}

/// Reserve a full 4 KiB block for each transient regular file, matching store admission's conservative accounting.
fn round_physical(bytes: u64) -> u64 {
    const BLOCK: u64 = 4096;
    bytes.saturating_add(BLOCK - 1) / BLOCK * BLOCK
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use oven_model::compiler_identity::CompilerIdentity;

    /// The ordinary compatibility publisher must retain an explicitly observed empty OUT_DIR through Store
    /// publication, mirror import and acquisition. This is a production-boundary regression: the generated root uses
    /// the same owner-relative spelling as stable capture, while the publication path is the one used before the
    /// runtime-foundation asset is assembled.
    #[test]
    fn empty_generated_output_survives_publish_mirror_and_acquire() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source = project.path().join("src/main.rs");
        fs::create_dir_all(source.parent().ok_or("fixture source has no parent")?)?;
        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"empty_generated_output\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(&source, "fn main() {}\n")?;
        let request = OvenGeneratedProjectRequest::new(
            project.path(),
            "empty_generated_output",
            "0.1.0",
            "fixture-target",
            "fixture-rustc",
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", &source);
        let receipt = receipt_generated_project(&request)?;

        let staging = tempfile::tempdir()?;
        let empty_digest = oven_rustc::rustc::selected_graph_generated_input_digest(&[])?;
        let relative_root = format!(
            "generated-outputs/{}",
            empty_digest.strip_prefix("sha256:").unwrap_or(&empty_digest)
        );
        fs::create_dir_all(staging.path().join(&relative_root))?;
        fs::write(staging.path().join("foundation.json"), b"{}")?;
        let materialized_files = materialized_files_from_directory(
            staging.path(),
            "foundation",
            "empty generated output publication fixture",
        )?;

        let mirror_root = tempfile::tempdir()?;
        let mirror = OvenStore::new(
            mirror_root.path(),
            OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000),
        );
        let published = mirror.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "empty-generated-output".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: b"fixture plan".to_vec(),
            materialized_files,
            materialized_directories: vec![OvenArtifactMaterializedDirectory {
                source_path: staging.path().join(&relative_root),
                relative_path: format!("foundation/{relative_root}"),
            }],
        })?;

        let local_root = tempfile::tempdir()?;
        let local = OvenStore::new(local_root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let imported = oven_store::store_mirror::import_matching_from_mirrors(
            &local,
            &[mirror_root.path().to_path_buf()],
            Some(&receipt),
            |manifest| manifest.identity == published.identity,
        )?;
        assert_eq!(imported.len(), 1);
        let (acquired, _payload, _lease) = local.select_payload(&published.identity)?;
        assert!(
            acquired
                .materialized_root()
                .join("foundation")
                .join(&relative_root)
                .is_dir(),
            "the admitted empty generated-output root must survive cold publication, mirror import and acquisition"
        );
        let acquired_empty_root = acquired.materialized_root().join("foundation").join(&relative_root);
        let unexpected = acquired_empty_root.join("unexpected.txt");
        fs::write(&unexpected, b"tampered")?;
        assert!(
            acquired.verify_admitted_payload().is_err(),
            "adding content beneath a declared empty generated-output root must invalidate the admitted entry"
        );
        fs::remove_file(unexpected)?;
        acquired.verify_admitted_payload()?;
        fs::remove_dir(acquired_empty_root)?;
        assert!(
            acquired.verify_admitted_payload().is_err(),
            "removing a declared empty generated-output root must invalidate the admitted entry"
        );
        Ok(())
    }

    /// One registry unit resolved twice contributes its staged tree once, keeping every feature either asked for.
    ///
    /// The publisher refuses a manifest that declares one relative artifact path more than once, and a repeated
    /// resolution is how that happened: the staged directory is a digest of registry, package, version and
    /// checksum, so both resolutions name the same tree and both contribute its files.
    #[test]
    fn a_registry_unit_resolved_twice_is_folded_with_its_features_unioned() {
        let source = |features: &[&str]| super::OvenLegacyCargoInspectionSource {
            package: "serde".to_string(),
            version: "1.0.0".to_string(),
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: "sha256:serde".to_string(),
            features: features.iter().map(|feature| (*feature).to_string()).collect(),
            source_root: std::path::PathBuf::from("registry-sources/serde"),
            source_digest: "sha256:serde-tree".to_string(),
            members: Vec::new(),
        };
        let mut other = source(&["std"]);
        other.package = "itoa".to_string();
        other.checksum = "sha256:itoa".to_string();

        let folded = super::fold_repeated_inspection_sources(vec![
            source(&["derive"]),
            source(&["std", "derive"]),
            other.clone(),
        ]);

        assert_eq!(folded.len(), 2, "the repeated unit must contribute one staged tree");
        assert_eq!(folded[0].package, "serde");
        assert_eq!(folded[0].features, vec!["derive".to_string(), "std".to_string()]);
        assert_eq!(folded[1].package, "itoa");

        // A unit that differs in any identifying field names a different staged tree and must survive on its own.
        let mut rekeyed = source(&["derive"]);
        rekeyed.version = "1.0.1".to_string();
        assert_eq!(
            super::fold_repeated_inspection_sources(vec![source(&["derive"]), rekeyed]).len(),
            2
        );
    }
    use super::{
        OVEN_PROVIDER_COMPILATION_KEY, OvenCompilerMacroDependency, provider_compilation_externs,
        provider_compilation_requirements_digest, validate_provider_compilation_requirements,
        validate_provider_macro_artifacts,
    };

    use super::{
        CargoInvocationOutput, CargoMetadata, CargoMetadataPackage, CargoMetadataResolve,
        CargoMetadataResolveDependency, CargoMetadataResolveNode, CargoUnitGraph, CargoUnitGraphDependency,
        CargoUnitGraphTarget, CargoUnitGraphUnit, CompilerSuiteArtifactCatalog, InspectionPackageScope,
        OVEN_COMPILER_TEST_PROFILE, OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION,
        OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION, OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION,
        OvenCompilerTestSuiteArtifactClosure, OvenCompilerTestSuiteFoundationReference, OvenCompilerTestSuitePayload,
        OvenCompilerTestSuiteShardPayload, OvenCompilerTestSuiteShardReference, OvenCompilerTestSuiteTarget,
        OvenCompilerTestSuiteToolchainLoafGenerationReference, OvenLegacyCargoBaseLoaf,
        OvenLegacyCargoDirectDependencyClosure, OvenLegacyCargoInspectionPackage, OvenLegacyCargoInvocationTarget,
        OvenLegacyCargoPrepareRequest, OvenLegacyCargoPublicationKind, OvenProjectExtensionPayload,
        ResolvedDirectDependency, artifact_closure, artifact_closure_from_reported_paths,
        canonicalize_supporting_artifacts, compiler_suite_artifact_catalog, compiler_suite_artifact_index,
        compiler_suite_cargo_build_output, compiler_suite_cli_target_from_artifact_index,
        compiler_suite_dependency_artifact, compiler_suite_dependency_directories, compiler_suite_direct_cli_plan,
        compiler_suite_direct_target_plan, compiler_suite_direct_target_shard_from_catalog,
        compiler_suite_direct_target_shard_plan, compiler_suite_foundation_dependencies,
        compiler_suite_foundation_lock, compiler_suite_foundation_manifest, compiler_suite_foundation_plans,
        compiler_suite_output_artifact_paths, compiler_suite_target_externs, compiler_suite_target_from_unit,
        compiler_suite_target_runner, compiler_suite_target_selection_features, compiler_suite_target_selection_groups,
        compiler_suite_target_selections, compiler_suite_toolchain_data_plans,
        compiler_suite_toolchain_loaf_generation_reference, compiler_suite_verified_target_source_bytes,
        compiler_suite_workspace_libraries_for_roots, conservative_directory_reservation, create_publisher_staging,
        digest_local_cargo_workspace_authority, direct_rustc_compile_environment,
        direct_rustc_reusable_project_plan_environment, explicit_project_bake_inspection_sources,
        generated_project_direct_dependencies, inspection_package_closure_ids, legacy_cargo_inspection_sources,
        locked_generated_project, materialize_sealed_registry_lock, materialized_files_from_directory,
        prepare_compiler_test_suite, prepare_direct_rustc_plan, project_registry_source_dependencies,
        publisher_direct_dependencies, publisher_registry_source_catalog, read_legacy_cargo_metadata_with_lock_policy,
        reclaim_unmaterialized_compiler_suite_target_files, release_cohort_generated_project_lock,
        resolve_direct_dependency_packages, run_legacy_cargo, run_legacy_cargo_invocation,
        select_compiler_test_suite_identity, select_existing_project_extension_identity,
        source_compiler_vocab_support_paths_are_available, stage_compiler_suite_shard_files,
        stage_registry_source_directory, stage_self_contained_sdk_provider_tree, validate_compiler_suite_unit_graph,
        validate_generated_registry_lock, validate_release_cohort_registry_lock,
    };
    use oven_rustc::loaf::{
        OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION, OVEN_LOAF_SCHEMA_VERSION, OvenLoaf, OvenLoafEnvelopeManifest,
        OvenLoafEnvelopeMember, OvenLoafMemberRole,
    };
    use oven_rustc::rustc::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH, OvenRustcArtifactExtern,
        OvenRustcArtifactManifest, OvenRustcRegistrySource, OvenRustcRegistrySourcePackage,
        OvenRustcSupportingArtifact, rustc_host_target, rustc_identity,
    };
    use oven_store::store::{
        OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreLimits,
    };
    use oven_store::{
        OvenBuildIntent, OvenCompilerSuiteRequest, OvenGeneratedProjectRequest, digest_bytes, digest_source_tree,
        receipt_generated_project, receipt_native_compiler_suite,
    };
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        process::Command,
    };

    /// A declared macro uses content identities independently of installation or provider source coordinates.
    fn provider_macro_requirement(root: &Path) -> OvenCompilerMacroDependency {
        OvenCompilerMacroDependency {
            alias: "incan_derive".to_string(),
            package: "incan_derive".to_string(),
            source_root: root.to_path_buf(),
            source_digest: "sha256:macro-source".to_string(),
            core_source_digest: "sha256:core-source".to_string(),
            runtime_lock_digest: "sha256:runtime-lock".to_string(),
        }
    }

    /// Body/publication data is absent from foundation identity, and a zero-macro graph adds no publisher roots.
    #[test]
    fn provider_macro_requirements_preserve_reuse_and_consumer_visibility() -> Result<(), Box<dyn std::error::Error>> {
        let original = provider_macro_requirement(Path::new("/publisher/one"));
        let digest = provider_compilation_requirements_digest(std::slice::from_ref(&original))?;
        let mut relocated = original.clone();
        relocated.source_root = PathBuf::from("/publisher/two");
        assert_eq!(
            provider_compilation_requirements_digest(&[original.clone(), relocated])?,
            digest
        );
        let mut changed = original.clone();
        changed.source_digest.push_str("-changed");
        assert_ne!(provider_compilation_requirements_digest(&[changed])?, digest);
        let consumer = BTreeMap::from([("incan_std_core".to_string(), "incan_std_core".to_string())]);
        let mut selected = consumer.clone();
        selected.insert("incan_derive".to_string(), "incan_derive".to_string());
        let projections =
            provider_compilation_externs(std::slice::from_ref(&original), &consumer, &selected, "generated-root")?;
        assert_eq!(projections["generated-root"], ["incan_std_core"]);
        assert_eq!(
            projections[OVEN_PROVIDER_COMPILATION_KEY],
            ["incan_derive", "incan_std_core"]
        );
        assert!(
            provider_compilation_externs(std::slice::from_ref(&original), &consumer, &consumer, "generated-root")
                .is_err()
        );
        assert!(provider_compilation_externs(&[], &consumer, &selected, "generated-root")?.is_empty());
        let project = tempfile::tempdir()?;
        let source = project.path().join("main.rs");
        fs::write(&source, "fn main() {}")?;
        let request = OvenGeneratedProjectRequest::new(
            project.path(),
            "consumer",
            "0.1.0",
            "target",
            "rustc",
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", &source)
        .with_build_unit_input("provider-compilation-requirements", digest);
        let receipt = receipt_generated_project(&request)?;
        fs::write(&source, "fn main() { /* body changed */ }")?;
        let edited = receipt_generated_project(&request)?;
        assert_ne!(receipt.identity, edited.identity);
        assert_eq!(receipt.build_unit_identity, edited.build_unit_identity);
        validate_provider_compilation_requirements(&edited, std::slice::from_ref(&original))?;
        let mut wrong_lock = original;
        wrong_lock.runtime_lock_digest.push_str("-changed");
        assert!(validate_provider_compilation_requirements(&edited, &[wrong_lock]).is_err());
        Ok(())
    }

    /// The publisher must associate the named macro with its locked package, source and actual proc-macro report.
    #[test]
    fn provider_macro_artifact_requires_exact_reported_package_kind_and_path() -> Result<(), Box<dyn std::error::Error>>
    {
        let staging = tempfile::tempdir()?;
        let package_root = fs::canonicalize(staging.path())?;
        let requirement = provider_macro_requirement(&package_root);
        let artifact_path = staging.path().join("reported-macro");
        let other_path = staging.path().join("other-macro");
        fs::write(&artifact_path, b"reported host macro")?;
        fs::write(&other_path, b"different host macro")?;
        let metadata = CargoMetadata {
            packages: vec![CargoMetadataPackage {
                id: "locked-macro-id".to_string(),
                name: "incan_derive".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: package_root.join("Cargo.toml"),
                source: None,
            }],
            resolve: None,
        };
        let resolved = BTreeMap::from([(
            "incan_derive".to_string(),
            ResolvedDirectDependency {
                package: "incan_derive".to_string(),
                package_id: "locked-macro-id".to_string(),
            },
        )]);
        let externs = vec![OvenRustcArtifactExtern {
            crate_name: "incan_derive".to_string(),
            relative_path: "reported-macro".to_string(),
            digest: digest_bytes(b"reported host macro"),
        }];
        let report =
            |package_id: &str, kind: &str, path: &Path| -> Result<Vec<CargoInvocationOutput>, serde_json::Error> {
                Ok(vec![CargoInvocationOutput {
                    stdout: serde_json::to_vec(&serde_json::json!({
                        "reason": "compiler-artifact", "package_id": package_id,
                        "target": { "name": "incan_derive", "kind": [kind] }, "filenames": [path],
                    }))?,
                }])
            };
        let outputs = report("locked-macro-id", "proc-macro", &artifact_path)?;
        validate_provider_macro_artifacts(
            std::slice::from_ref(&requirement),
            &metadata,
            &resolved,
            &outputs,
            staging.path(),
            &externs,
            None,
        )?;
        for bad in [
            report("another-package-id", "proc-macro", &artifact_path)?,
            report("locked-macro-id", "lib", &artifact_path)?,
            report("locked-macro-id", "proc-macro", &other_path)?,
        ] {
            assert!(
                validate_provider_macro_artifacts(
                    std::slice::from_ref(&requirement),
                    &metadata,
                    &resolved,
                    &bad,
                    staging.path(),
                    &externs,
                    None
                )
                .is_err()
            );
        }
        let base_plan = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: OvenBuildIntent {
                target: "target".to_string(),
                toolchain: "rustc".to_string(),
                profile: "debug".to_string(),
                features: Vec::new(),
            },
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: externs.clone(),
            entrypoint_dependency_search_paths: Default::default(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let base = OvenLegacyCargoBaseLoaf {
            loaf_identity: "sha256:admitted-base".to_string(),
            build_unit_identity: "sha256:base-inputs".to_string(),
            artifacts: &base_plan,
            artifact_root: staging.path(),
        };
        validate_provider_macro_artifacts(
            std::slice::from_ref(&requirement),
            &metadata,
            &resolved,
            &[],
            &staging.path().join("absent-new-staging"),
            &externs,
            Some(&base),
        )?;
        let mut different = externs.clone();
        different[0].digest = digest_bytes(b"another macro");
        assert!(
            validate_provider_macro_artifacts(
                std::slice::from_ref(&requirement),
                &metadata,
                &resolved,
                &[],
                staging.path(),
                &different,
                Some(&base),
            )
            .is_err()
        );
        let mut wrong_source = requirement.clone();
        wrong_source.source_root = package_root.join("other-source");
        assert!(
            validate_provider_macro_artifacts(
                &[wrong_source],
                &metadata,
                &resolved,
                &outputs,
                staging.path(),
                &externs,
                None
            )
            .is_err()
        );
        assert!(
            validate_provider_macro_artifacts(
                &[requirement],
                &metadata,
                &resolved,
                &outputs,
                staging.path(),
                &[],
                None
            )
            .is_err()
        );
        validate_provider_macro_artifacts(&[], &metadata, &BTreeMap::new(), &[], staging.path(), &[], None)?;
        Ok(())
    }

    #[test]
    fn source_compiler_vocab_support_requires_a_binary_beneath_the_source_target()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = tempfile::tempdir()?;
        let source_root = source.path();
        fs::create_dir_all(source_root.join("crates/incan_vocab"))?;
        fs::create_dir_all(source_root.join("target/release"))?;
        fs::write(source_root.join("Cargo.lock"), "# fixture\n")?;
        fs::write(
            source_root.join("crates/incan_vocab/Cargo.toml"),
            "[package]\nname = \"incan_vocab\"\n",
        )?;

        let source_binary = source_root.join("target/release/incan");
        assert!(source_compiler_vocab_support_paths_are_available(
            source_root,
            &source_binary
        ));

        let installed_binary = tempfile::tempdir()?.path().join("toolchains/0.5.1-rc2/bin/incan");
        assert!(!source_compiler_vocab_support_paths_are_available(
            source_root,
            &installed_binary
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn transient_reservation_counts_a_symlink_without_following_its_target() -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let target = outside.path().join("large-external-payload");
        fs::write(&target, vec![0_u8; 1024 * 1024])?;
        std::os::unix::fs::symlink(&target, staging.path().join("cargo-transient-link"))?;

        let reservation = conservative_directory_reservation(staging.path())?;
        assert!(
            reservation >= 2 * 4096,
            "the staging root and link must be accounted for"
        );
        assert!(
            reservation < 1024 * 1024,
            "transient reservation must not follow a symlink outside publisher staging"
        );
        Ok(())
    }

    fn write_test_loaf_envelope(
        loafs: &std::path::Path,
        members: Vec<OvenLoafEnvelopeMember>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        fs::write(loafs.join(".envelope.lock"), "")?;
        fs::write(
            loafs.join("envelope.json"),
            serde_json::to_vec(&OvenLoafEnvelopeManifest {
                schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
                envelope: "compiler-suite".to_string(),
                generation_identity: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
                evidence: BTreeMap::new(),
                loafs: members,
                release_store_member: None,
                runtime_foundation: None,
            })?,
        )?;
        Ok(())
    }

    #[test]
    fn inspection_surface_keeps_declared_registry_closure_but_not_provider_sources()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = Some("registry+https://example.invalid/index".to_string());
        let package = |id: &str, name: &str, version: &str, source: Option<String>| CargoMetadataPackage {
            id: id.to_string(),
            name: name.to_string(),
            version: version.to_string(),
            manifest_path: PathBuf::from(format!("/{name}/Cargo.toml")),
            source,
        };
        let node = |id: &str, dependencies: &[&str]| CargoMetadataResolveNode {
            id: id.to_string(),
            features: Vec::new(),
            dependencies: dependencies
                .iter()
                .map(|dependency| (*dependency).to_string())
                .collect(),
            deps: Vec::new(),
        };
        let metadata = CargoMetadata {
            packages: vec![
                package("root", "fixture", "0.1.0", None),
                package("declared", "declared-rust", "1.2.3", registry.clone()),
                package("transitive", "declared-transitive", "2.0.0", registry.clone()),
                package("provider", "provider-runtime", "9.0.0", registry),
            ],
            resolve: Some(CargoMetadataResolve {
                root: Some("root".to_string()),
                nodes: vec![
                    node("root", &["declared", "provider"]),
                    node("declared", &["transitive"]),
                    node("transitive", &[]),
                    node("provider", &[]),
                ],
            }),
        };

        let selected = inspection_package_closure_ids(
            &metadata,
            &[OvenLegacyCargoInspectionPackage {
                package: "declared-rust".to_string(),
                version_requirement: "1".to_string(),
            }],
            InspectionPackageScope::DirectRoot,
        )?;

        assert_eq!(
            selected,
            ["declared".to_string(), "transitive".to_string()].into_iter().collect()
        );
        assert!(!selected.contains("provider"));

        let complete = inspection_package_closure_ids(&metadata, &[], InspectionPackageScope::CompleteResolvedGraph)?;
        assert_eq!(
            complete,
            ["declared", "provider", "root", "transitive"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
        Ok(())
    }

    #[test]
    fn project_extension_catalog_keeps_every_locked_source_without_fabricating_a_leaf()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = tempfile::tempdir()?;
        let staging = fixture.path().join("staging");
        fs::create_dir_all(&staging)?;
        let registry = "registry+https://example.invalid/index";
        let package = |id: &str, name: &str, dependencies: &[&str]| -> Result<_, Box<dyn std::error::Error>> {
            let root = fixture.path().join(name);
            fs::create_dir_all(root.join("src"))?;
            fs::write(
                root.join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
            )?;
            fs::write(root.join("src/lib.rs"), "pub fn sealed() {}\n")?;
            Ok((
                CargoMetadataPackage {
                    id: id.to_string(),
                    name: name.to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: root.join("Cargo.toml"),
                    source: Some(registry.to_string()),
                },
                CargoMetadataResolveNode {
                    id: id.to_string(),
                    features: Vec::new(),
                    dependencies: dependencies
                        .iter()
                        .map(|dependency| (*dependency).to_string())
                        .collect(),
                    deps: Vec::new(),
                },
            ))
        };
        let (serde_json, serde_json_node) = package("serde-json", "serde_json", &["serde"])?;
        let (serde, serde_node) = package("serde", "serde", &["serde-derive"])?;
        let (serde_derive, serde_derive_node) = package("serde-derive", "serde_derive", &[])?;
        let (uninspected, uninspected_node) = package("uninspected", "uninspected", &[])?;
        let metadata = CargoMetadata {
            packages: vec![
                CargoMetadataPackage {
                    id: "root".to_string(),
                    name: "fixture".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: fixture.path().join("Cargo.toml"),
                    source: None,
                },
                serde_json,
                serde,
                serde_derive,
                uninspected,
            ],
            resolve: Some(CargoMetadataResolve {
                root: Some("root".to_string()),
                nodes: vec![
                    CargoMetadataResolveNode {
                        id: "root".to_string(),
                        features: Vec::new(),
                        dependencies: vec!["serde-json".to_string()],
                        deps: Vec::new(),
                    },
                    serde_json_node,
                    serde_node,
                    serde_derive_node,
                    uninspected_node,
                ],
            }),
        };
        let lock = format!(
            "version = 4\n\n{}",
            ["serde_json", "serde", "serde_derive", "uninspected"]
                .into_iter()
                .map(|name| format!(
                    "[[package]]\nname = \"{name}\"\nversion = \"1.0.0\"\nsource = \"{registry}\"\nchecksum = \"{name}-checksum\"\n\n"
                ))
                .collect::<String>()
        );

        let (sources, source_artifacts) =
            publisher_registry_source_catalog(&metadata, lock.as_bytes(), &staging, None, &[], true, None)?;

        assert_eq!(
            sources.iter().map(|source| source.package.as_str()).collect::<Vec<_>>(),
            ["serde", "serde_derive", "serde_json", "uninspected"]
        );
        assert!(sources.iter().any(|source| source.package == "serde_derive"));
        assert!(
            source_artifacts
                .iter()
                .any(|artifact| artifact.relative_path.ends_with("/Cargo.toml"))
        );
        Ok(())
    }

    #[test]
    fn supporting_artifact_union_collapses_identical_source_records_and_rejects_conflicts()
    -> Result<(), Box<dyn std::error::Error>> {
        let artifact = |digest: &str| OvenRustcSupportingArtifact {
            relative_path: "registry-sources/fixture/Cargo.toml".to_string(),
            digest: digest.to_string(),
        };
        let mut identical = vec![artifact("sha256:fixture"), artifact("sha256:fixture")];
        canonicalize_supporting_artifacts(&mut identical)?;
        assert_eq!(identical, vec![artifact("sha256:fixture")]);

        let mut conflicting = vec![artifact("sha256:first"), artifact("sha256:second")];
        let error = canonicalize_supporting_artifacts(&mut conflicting)
            .err()
            .ok_or("conflicting source records must fail closed")?;
        assert!(error.to_string().contains("conflicting digests"));
        Ok(())
    }

    #[test]
    fn inspection_surface_rejects_an_ambiguous_locked_package() -> Result<(), Box<dyn std::error::Error>> {
        let registry = Some("registry+https://example.invalid/index".to_string());
        let metadata = CargoMetadata {
            packages: vec![
                CargoMetadataPackage {
                    id: "root".to_string(),
                    name: "compiler".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: PathBuf::from("/compiler/Cargo.toml"),
                    source: None,
                },
                CargoMetadataPackage {
                    id: "duplicate-1".to_string(),
                    name: "duplicate".to_string(),
                    version: "1.2.0".to_string(),
                    manifest_path: PathBuf::from("/duplicate-1/Cargo.toml"),
                    source: registry.clone(),
                },
                CargoMetadataPackage {
                    id: "duplicate-2".to_string(),
                    name: "duplicate".to_string(),
                    version: "1.3.0".to_string(),
                    manifest_path: PathBuf::from("/duplicate-2/Cargo.toml"),
                    source: registry,
                },
            ],
            resolve: Some(CargoMetadataResolve {
                root: Some("root".to_string()),
                nodes: vec![
                    CargoMetadataResolveNode {
                        id: "root".to_string(),
                        features: Vec::new(),
                        dependencies: vec!["duplicate-1".to_string(), "duplicate-2".to_string()],
                        deps: Vec::new(),
                    },
                    CargoMetadataResolveNode {
                        id: "duplicate-1".to_string(),
                        features: Vec::new(),
                        dependencies: Vec::new(),
                        deps: Vec::new(),
                    },
                    CargoMetadataResolveNode {
                        id: "duplicate-2".to_string(),
                        features: Vec::new(),
                        dependencies: Vec::new(),
                        deps: Vec::new(),
                    },
                ],
            }),
        };

        let error = match inspection_package_closure_ids(
            &metadata,
            &[OvenLegacyCargoInspectionPackage {
                package: "duplicate".to_string(),
                version_requirement: "1".to_string(),
            }],
            InspectionPackageScope::DirectRoot,
        ) {
            Ok(selected) => return Err(format!("ambiguous package unexpectedly selected: {selected:?}").into()),
            Err(error) => error,
        };
        assert!(error.to_string().contains("ambiguously"), "unexpected error: {error}");
        Ok(())
    }

    #[test]
    fn compiler_suite_dependency_directories_preserves_a_host_only_foundation() -> Result<(), Box<dyn std::error::Error>>
    {
        let staging = tempfile::tempdir()?;
        let target_deps = staging.path().join("target/aarch64-apple-darwin/oven-test/deps");
        let host_deps = staging.path().join("target/oven-test/deps");
        fs::create_dir_all(&host_deps)?;
        fs::write(host_deps.join("libfixture-abc.dylib"), "host-only foundation")?;

        let directories = compiler_suite_dependency_directories(target_deps.clone(), host_deps.clone());

        assert_eq!(directories, vec![host_deps.clone()]);
        let catalog = compiler_suite_artifact_catalog(staging.path(), &directories, &[])?;
        assert_eq!(catalog.closure.dependency_search_paths, vec!["target/oven-test/deps"]);
        assert_eq!(catalog.materialized_files.len(), 1);
        assert!(!target_deps.exists());
        Ok(())
    }

    #[test]
    fn compiler_suite_catalog_uses_a_reported_foundation_artifact_without_a_deps_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let artifact = staging
            .path()
            .join("third-party-foundation-target/aarch64-apple-darwin/oven-test/liboven_compiler_foundation.rlib");
        fs::create_dir_all(artifact.parent().ok_or("foundation artifact parent missing")?)?;
        fs::write(&artifact, "foundation artifact")?;
        let output = CargoInvocationOutput {
            stdout: format!(
                "{}\n",
                serde_json::json!({
                    "reason": "compiler-artifact",
                    "package_id": "oven-compiler-foundation 0.0.0",
                    "target": { "name": "oven_compiler_foundation" },
                    "filenames": [artifact],
                })
            )
            .into_bytes(),
        };

        let reported_artifacts = compiler_suite_output_artifact_paths(&output)?;
        let catalog = compiler_suite_artifact_catalog(staging.path(), &[], &reported_artifacts)?;

        assert_eq!(reported_artifacts, vec![fs::canonicalize(&artifact)?]);
        assert_eq!(catalog.closure.dependency_search_paths, Vec::<String>::new());
        assert_eq!(catalog.materialized_files.len(), 1);
        assert_eq!(
            catalog.materialized_files[0].relative_path,
            "third-party-foundation-target/aarch64-apple-darwin/oven-test/liboven_compiler_foundation.rlib"
        );
        Ok(())
    }

    #[test]
    fn publisher_staging_canonicalizes_a_relative_store_path() -> Result<(), Box<dyn std::error::Error>> {
        let parent = tempfile::tempdir_in(".")?;
        let relative_parent =
            std::path::PathBuf::from(parent.path().file_name().ok_or("temporary parent has no name")?);

        let staging = create_publisher_staging(&relative_parent)?;

        assert!(staging.is_absolute());
        assert!(staging.starts_with(fs::canonicalize(parent.path())?));
        Ok(())
    }

    #[test]
    fn publisher_selects_a_host_proc_macro_dylib_as_a_missing_direct_extern() -> Result<(), Box<dyn std::error::Error>>
    {
        let staging = tempfile::tempdir()?;
        let target_deps = staging.path().join("target/aarch64-apple-darwin/debug/deps");
        let host_deps = staging.path().join("target/debug/deps");
        fs::create_dir_all(&target_deps)?;
        fs::create_dir_all(&host_deps)?;
        fs::write(host_deps.join("libincan_web_macros-abc.dylib"), "host proc macro")?;

        let direct_dependencies = BTreeMap::from([("incan_web_macros".to_string(), "incan_web_macros".to_string())]);
        let (_, externs, supporting) = artifact_closure(
            staging.path(),
            &target_deps,
            &[target_deps.clone(), host_deps],
            &direct_dependencies,
            false,
        )?;

        assert_eq!(externs.len(), 1);
        assert_eq!(externs[0].crate_name, "incan_web_macros");
        assert_eq!(
            externs[0].relative_path,
            "target/debug/deps/libincan_web_macros-abc.dylib"
        );
        assert!(
            supporting
                .iter()
                .all(|artifact| artifact.relative_path != externs[0].relative_path)
        );
        Ok(())
    }

    #[test]
    fn publisher_prefers_a_target_direct_extern_over_a_matching_host_dylib() -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let target_deps = staging.path().join("target/aarch64-apple-darwin/debug/deps");
        let host_deps = staging.path().join("target/debug/deps");
        fs::create_dir_all(&target_deps)?;
        fs::create_dir_all(&host_deps)?;
        fs::write(target_deps.join("libfixture-abc.rlib"), "target library")?;
        fs::write(host_deps.join("libfixture-def.dylib"), "host dylib")?;

        let direct_dependencies = BTreeMap::from([("fixture".to_string(), "fixture".to_string())]);
        let (_, externs, _) = artifact_closure(
            staging.path(),
            &target_deps,
            &[target_deps.clone(), host_deps],
            &direct_dependencies,
            false,
        )?;

        assert_eq!(externs.len(), 1);
        assert_eq!(
            externs[0].relative_path,
            "target/aarch64-apple-darwin/debug/deps/libfixture-abc.rlib"
        );
        Ok(())
    }

    #[test]
    fn reported_artifact_closure_keeps_the_direct_package_instance_when_versions_share_a_crate_name()
    -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let target_triple = "aarch64-apple-darwin";
        let target_dependencies = staging.path().join(format!("target/{target_triple}/debug/deps"));
        fs::create_dir_all(&target_dependencies)?;
        let transitive = target_dependencies.join("libsubstrait-aaaa.rlib");
        let direct = target_dependencies.join("libsubstrait-zzzz.rlib");
        fs::write(&transitive, "substrait 0.62 transitive")?;
        fs::write(&direct, "substrait 0.63 direct")?;
        let compiler_artifact = |package_id: &str, artifact: &Path| {
            serde_json::json!({
                "reason": "compiler-artifact",
                "package_id": package_id,
                "target": { "name": "substrait" },
                "filenames": [artifact],
            })
            .to_string()
        };
        let output = CargoInvocationOutput {
            stdout: format!(
                "{}\n{}\n",
                compiler_artifact("substrait 0.62.2 (registry+https://example.invalid)", &transitive),
                compiler_artifact("substrait 0.63.0 (registry+https://example.invalid)", &direct),
            )
            .into_bytes(),
        };
        let dependencies = BTreeMap::from([(
            "substrait".to_string(),
            ResolvedDirectDependency {
                package: "substrait".to_string(),
                package_id: "substrait 0.63.0 (registry+https://example.invalid)".to_string(),
            },
        )]);

        let (_, externs, _) = artifact_closure_from_reported_paths(
            staging.path(),
            target_triple,
            "debug",
            &dependencies,
            false,
            &[output],
        )?;

        assert_eq!(externs.len(), 1);
        assert_eq!(externs[0].crate_name, "substrait");
        assert!(
            externs[0].relative_path.ends_with("libsubstrait-zzzz.rlib"),
            "the root --extern must use the direct 0.63 package, not its 0.62 transitive namesake: {}",
            externs[0].relative_path
        );
        Ok(())
    }

    #[test]
    fn reported_artifact_closure_retains_a_renamed_library_target_from_cargo_build_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let target_triple = "aarch64-apple-darwin";
        let output_directory = staging
            .path()
            .join(format!("target/{target_triple}/debug/build/coreaudio-rs/abc123/out"));
        fs::create_dir_all(&output_directory)?;
        let library = output_directory.join("libcoreaudio-abc123.rlib");
        fs::write(&library, "coreaudio-rs library target")?;
        let compiler_artifact = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "coreaudio-rs 0.2.16 (registry+https://example.invalid)",
            "target": { "name": "coreaudio" },
            "filenames": [library],
        });
        let output = CargoInvocationOutput {
            stdout: format!("{compiler_artifact}\n").into_bytes(),
        };

        let (search_paths, _, supporting) = artifact_closure_from_reported_paths(
            staging.path(),
            target_triple,
            "debug",
            &BTreeMap::new(),
            false,
            &[output],
        )?;

        assert_eq!(
            search_paths,
            vec!["target/aarch64-apple-darwin/debug/build/coreaudio-rs/abc123/out".to_string()]
        );
        assert_eq!(supporting.len(), 1);
        assert!(supporting[0].relative_path.ends_with("libcoreaudio-abc123.rlib"));
        Ok(())
    }

    #[test]
    fn resolved_direct_dependency_packages_preserve_the_root_alias_to_package_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let metadata = CargoMetadata {
            packages: vec![
                CargoMetadataPackage {
                    id: "root".to_string(),
                    name: "fixture".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: PathBuf::from("/fixture/Cargo.toml"),
                    source: None,
                },
                CargoMetadataPackage {
                    id: "substrait 0.62".to_string(),
                    name: "substrait".to_string(),
                    version: "0.62.2".to_string(),
                    manifest_path: PathBuf::from("/registry/substrait-0.62/Cargo.toml"),
                    source: Some("registry+https://example.invalid".to_string()),
                },
                CargoMetadataPackage {
                    id: "substrait 0.63".to_string(),
                    name: "substrait".to_string(),
                    version: "0.63.0".to_string(),
                    manifest_path: PathBuf::from("/registry/substrait-0.63/Cargo.toml"),
                    source: Some("registry+https://example.invalid".to_string()),
                },
            ],
            resolve: Some(CargoMetadataResolve {
                root: Some("root".to_string()),
                nodes: vec![
                    CargoMetadataResolveNode {
                        id: "root".to_string(),
                        features: Vec::new(),
                        dependencies: vec!["substrait 0.63".to_string()],
                        deps: vec![CargoMetadataResolveDependency {
                            name: "substrait".to_string(),
                            pkg: "substrait 0.63".to_string(),
                        }],
                    },
                    CargoMetadataResolveNode {
                        id: "substrait 0.62".to_string(),
                        features: Vec::new(),
                        dependencies: Vec::new(),
                        deps: Vec::new(),
                    },
                    CargoMetadataResolveNode {
                        id: "substrait 0.63".to_string(),
                        features: Vec::new(),
                        dependencies: vec!["substrait 0.62".to_string()],
                        deps: Vec::new(),
                    },
                ],
            }),
        };

        let resolved = resolve_direct_dependency_packages(
            &metadata,
            &BTreeMap::from([("substrait".to_string(), "substrait".to_string())]),
        )?;

        assert_eq!(resolved["substrait"].package_id, "substrait 0.63");
        Ok(())
    }

    #[test]
    fn project_registry_source_dependencies_preserve_two_compatible_root_aliases()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = "registry+https://example.invalid";
        let metadata = CargoMetadata {
            packages: vec![
                CargoMetadataPackage {
                    id: "root".to_string(),
                    name: "fixture".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: PathBuf::from("/fixture/Cargo.toml"),
                    source: None,
                },
                CargoMetadataPackage {
                    id: "shared 1.2".to_string(),
                    name: "shared".to_string(),
                    version: "1.2.0".to_string(),
                    manifest_path: PathBuf::from("/registry/shared-1.2/Cargo.toml"),
                    source: Some(registry.to_string()),
                },
                CargoMetadataPackage {
                    id: "shared 1.8".to_string(),
                    name: "shared".to_string(),
                    version: "1.8.0".to_string(),
                    manifest_path: PathBuf::from("/registry/shared-1.8/Cargo.toml"),
                    source: Some(registry.to_string()),
                },
            ],
            resolve: Some(CargoMetadataResolve {
                root: Some("root".to_string()),
                nodes: vec![CargoMetadataResolveNode {
                    id: "root".to_string(),
                    features: Vec::new(),
                    dependencies: vec!["shared 1.2".to_string(), "shared 1.8".to_string()],
                    deps: vec![
                        CargoMetadataResolveDependency {
                            name: "shared_old".to_string(),
                            pkg: "shared 1.2".to_string(),
                        },
                        CargoMetadataResolveDependency {
                            name: "shared_new".to_string(),
                            pkg: "shared 1.8".to_string(),
                        },
                    ],
                }],
            }),
        };
        let source = |version: &str, checksum: &str| OvenRustcRegistrySourcePackage {
            package: "shared".to_string(),
            version: version.to_string(),
            features: Vec::new(),
            source: OvenRustcRegistrySource {
                registry: registry.to_string(),
                checksum: checksum.to_string(),
                relative_root: format!("registry-sources/shared-{version}"),
                digest: format!("sha256:shared-{version}"),
            },
        };
        let sources = vec![source("1.2.0", "old-checksum"), source("1.8.0", "new-checksum")];
        let dependencies = BTreeMap::from([
            ("shared_new".to_string(), "shared".to_string()),
            ("shared_old".to_string(), "shared".to_string()),
        ]);

        let selected = project_registry_source_dependencies(&metadata, &dependencies, &sources)?;

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].alias, "shared_new");
        assert_eq!(selected[0].version, "1.8.0");
        assert_eq!(selected[0].checksum, "new-checksum");
        assert_eq!(selected[1].alias, "shared_old");
        assert_eq!(selected[1].version, "1.2.0");
        assert_eq!(selected[1].checksum, "old-checksum");

        let Err(error) = project_registry_source_dependencies(&metadata, &dependencies, &sources[..1]) else {
            return Err(std::io::Error::other("missing exact source record was accepted").into());
        };
        assert!(error.to_string().contains("has 0 exact sealed source records"));
        Ok(())
    }

    #[test]
    fn publisher_recovers_one_sealed_target_library_when_cargo_json_omits_its_artifact_family()
    -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let source = staging.path().join("registry/blake2/src/lib.rs");
        let target_dependencies = staging
            .path()
            .join("third-party-foundation-target/x86_64-unknown-linux-gnu/oven-test/deps");
        fs::create_dir_all(source.parent().ok_or("registry source parent missing")?)?;
        fs::create_dir_all(&target_dependencies)?;
        fs::write(&source, "pub fn fixture() {}\n")?;
        let resolved_library = target_dependencies.join("libblake2-resolved.rlib");
        fs::write(&resolved_library, "resolved blake2 library")?;
        let unit = CargoUnitGraphUnit {
            pkg_id: "blake2 0.10.6 (registry+https://example.invalid)".to_string(),
            target: CargoUnitGraphTarget {
                kind: vec!["lib".to_string()],
                crate_types: vec!["lib".to_string()],
                name: "blake2".to_string(),
                src_path: source.clone(),
                edition: "2024".to_string(),
            },
            mode: "build".to_string(),
            platform: Some("x86_64-unknown-linux-gnu".to_string()),
            features: Vec::new(),
            dependencies: Vec::new(),
        };
        let catalog = compiler_suite_artifact_catalog(staging.path(), std::slice::from_ref(&target_dependencies), &[])?;
        let selected = compiler_suite_dependency_artifact(
            &unit,
            &BTreeMap::new(),
            &catalog,
            "blake2",
            Some("x86_64-unknown-linux-gnu"),
        )?;
        assert_eq!(
            selected.relative_path,
            "third-party-foundation-target/x86_64-unknown-linux-gnu/oven-test/deps/libblake2-resolved.rlib"
        );

        // The sealed-catalog recovery is deliberately unique: two receipt-target libraries remain an explicit
        // publisher error rather than an arbitrary direct-Rustc selection.
        fs::write(
            target_dependencies.join("libblake2-second.rlib"),
            "second incompatible blake2 library",
        )?;
        let ambiguous_catalog = compiler_suite_artifact_catalog(staging.path(), &[target_dependencies], &[])?;
        assert!(matches!(
            compiler_suite_dependency_artifact(
                &unit,
                &BTreeMap::new(),
                &ambiguous_catalog,
                "blake2",
                Some("x86_64-unknown-linux-gnu"),
            ),
            Err(super::OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite unit graph",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn publisher_materializes_a_complete_provider_tree_with_portable_paths() -> Result<(), Box<dyn std::error::Error>> {
        let provider = tempfile::tempdir()?;
        fs::create_dir_all(provider.path().join("components/core"))?;
        fs::write(
            provider.path().join("sdk-inventory.json"),
            "{\n  \"schema_version\": 2,\n  \"sdk_id\": \"fixture\",\n  \"sdk_version\": \"0.1.0\",\n  \"compiler_requirement\": \">=0.1.0\",\n  \"provider_codegen_revision\": 5,\n  \"components\": {},\n  \"profiles\": {\"default\": []}\n}\n",
        )?;
        fs::write(provider.path().join("components/core/provider.incnlib"), "provider")?;
        fs::write(
            provider.path().join("components.rs"),
            "root file sorts before nested component",
        )?;

        let files = materialized_files_from_directory(provider.path(), "providers", "SDK provider inventory")?;
        let relative_paths = files.into_iter().map(|file| file.relative_path).collect::<Vec<_>>();
        assert_eq!(
            relative_paths,
            vec![
                "providers/components.rs".to_string(),
                "providers/components/core/provider.incnlib".to_string(),
                "providers/sdk-inventory.json".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn publisher_materializes_the_sealed_registry_lock_with_registry_sources() -> Result<(), Box<dyn std::error::Error>>
    {
        let staging = tempfile::tempdir()?;
        let sealed_lock = staging.path().join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
        let parent = sealed_lock.parent().ok_or("registry lock has no parent")?;
        fs::create_dir_all(parent)?;
        fs::write(&sealed_lock, "version = 4\n")?;

        let mut files = Vec::new();
        materialize_sealed_registry_lock(staging.path(), true, &mut files)?;

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
        assert_eq!(files[0].source_path, fs::canonicalize(&sealed_lock)?);
        Ok(())
    }

    /// The stdlib ring's version line as the checkout declares it, so the assertion below follows a bump.
    fn incan_stdlib_version_line() -> Result<String, Box<dyn std::error::Error>> {
        let manifest = oven_model::toolchain_layout::development_root()
            .join(oven_model::toolchain_layout::development_support_crate_dir(
                "incan_std_core",
            ))
            .join("Cargo.toml");
        let text = fs::read_to_string(&manifest)?;
        text.lines()
            .find_map(|line| {
                line.strip_prefix("version = \"")
                    .and_then(|rest| rest.strip_suffix('"'))
            })
            .map(str::to_string)
            .ok_or_else(|| format!("{} declares no explicit version line", manifest.display()).into())
    }

    #[test]
    fn publisher_seals_sdk_component_runtime_paths_inside_the_suite_entry() -> Result<(), Box<dyn std::error::Error>> {
        let provider = tempfile::tempdir()?;
        let component = provider.path().join("components/stdlib-core");
        let inherited_runtime = provider.path().join("runtime");
        fs::create_dir_all(&component)?;
        fs::create_dir_all(&inherited_runtime)?;
        fs::write(
            provider.path().join("sdk-inventory.json"),
            "{\n  \"schema_version\": 2,\n  \"sdk_id\": \"fixture\",\n  \"sdk_version\": \"0.1.0\",\n  \"compiler_requirement\": \">=0.1.0\",\n  \"provider_codegen_revision\": 5,\n  \"components\": {},\n  \"profiles\": {\"default\": []}\n}\n",
        )?;
        fs::write(
            component.join("Cargo.toml"),
            "[package]\nname = \"fixture_provider\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies.incan_derive]\npath = \"../../../../outside/incan_derive\"\n\n[dependencies.incan_std_core]\npath = \"../../../../outside/incan_std_core\"\n",
        )?;
        let inherited_lock = inherited_runtime.join("Cargo.lock");
        fs::write(&inherited_lock, "stale sealed runtime lock\n")?;
        fs::write(inherited_runtime.join("obsolete-runtime-file"), "stale")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&inherited_lock, fs::Permissions::from_mode(0o444))?;
        }
        let staging = tempfile::tempdir()?;

        let staged =
            stage_self_contained_sdk_provider_tree(provider.path(), staging.path(), &oven_store::NoProviderHooks)?;
        let manifest = fs::read_to_string(staged.join("components/stdlib-core/Cargo.toml"))?;

        assert!(manifest.contains("path = \"../../runtime/crates/incan_derive\""));
        assert!(manifest.contains("path = \"../../runtime/crates/incan_std_core\""));
        assert!(staged.join("runtime/Cargo.toml").is_file());
        assert!(staged.join("runtime/Cargo.lock").is_file());
        assert_ne!(
            fs::read_to_string(staged.join("runtime/Cargo.lock"))?,
            "stale sealed runtime lock\n"
        );
        let runtime_workspace: toml::Value = toml::from_str(&fs::read_to_string(staged.join("runtime/Cargo.toml"))?)?;
        let runtime_dependencies = runtime_workspace
            .get("workspace")
            .and_then(|workspace| workspace.get("dependencies"))
            .and_then(toml::Value::as_table)
            .ok_or("staged runtime workspace has no [workspace.dependencies] table")?;
        for crate_name in oven_model::toolchain_layout::SDK_RUNTIME_CRATES {
            assert_eq!(
                runtime_dependencies
                    .get(crate_name)
                    .and_then(|dependency| dependency.get("path"))
                    .and_then(toml::Value::as_str),
                Some(format!("crates/{crate_name}").as_str()),
                "staged runtime workspace must name {crate_name} at its copied location"
            );
        }
        assert_eq!(
            runtime_dependencies
                .get("incan_std_core")
                .and_then(|dependency| dependency.get("version"))
                .and_then(toml::Value::as_str),
            Some(incan_stdlib_version_line()?.as_str()),
            "staged runtime workspace must keep the stdlib ring's version requirement"
        );
        assert!(
            runtime_dependencies.values().all(|dependency| {
                dependency
                    .get("path")
                    .is_none_or(|path| toml::Value::as_str(path).is_some_and(|path| path.starts_with("crates/")))
            }),
            "staged runtime workspace must not name a checkout path it does not ship: {runtime_dependencies:?}"
        );
        assert!(!staged.join("runtime/obsolete-runtime-file").exists());
        assert!(staged.join("runtime/crates/incan_lang/src/lib.rs").is_file());
        assert!(staged.join("runtime/crates/incan_derive/src/lib.rs").is_file());
        for facet in [
            "incan_std_core",
            "incan_std_data",
            "incan_std_async",
            "incan_std_web",
            "incan_std_testing",
        ] {
            assert!(staged.join(format!("runtime/crates/{facet}/src/lib.rs")).is_file());
        }
        assert!(staged.join("runtime/crates/incan_web_macros/src/lib.rs").is_file());

        let files = materialized_files_from_directory(&staged, "providers", "SDK provider inventory")?;
        assert!(
            files
                .iter()
                .any(|file| file.relative_path == "providers/runtime/crates/incan_std_core/src/lib.rs")
        );
        Ok(())
    }

    #[test]
    fn publisher_keeps_dependency_used_only_by_nested_generated_module() -> Result<(), Box<dyn std::error::Error>> {
        let generated = tempfile::tempdir()?;
        fs::create_dir_all(generated.path().join("src/models"))?;
        fs::write(generated.path().join("src/lib.rs"), "pub mod models;\n")?;
        fs::write(
            generated.path().join("src/models/generated.rs"),
            "#[derive(incan_derive::IncanModel)]\npub struct Model;\n",
        )?;
        let declared: BTreeMap<_, _> = [
            ("incan_derive".to_string(), "incan_derive".to_string()),
            ("unused_crate".to_string(), "unused_crate".to_string()),
        ]
        .into_iter()
        .collect();

        let selected = generated_project_direct_dependencies(generated.path(), &declared)?;

        assert_eq!(
            selected,
            [("incan_derive".to_string(), "incan_derive".to_string())]
                .into_iter()
                .collect()
        );
        Ok(())
    }

    #[test]
    fn complete_stdlib_closure_keeps_checked_dependency_not_exercised_by_fixture()
    -> Result<(), Box<dyn std::error::Error>> {
        let generated = tempfile::tempdir()?;
        fs::create_dir_all(generated.path().join("src"))?;
        fs::write(generated.path().join("src/main.rs"), "fn main() {}\n")?;
        let declared: BTreeMap<_, _> = [
            ("byteorder".to_string(), "byteorder".to_string()),
            ("incan_std_core".to_string(), "incan_std_core".to_string()),
        ]
        .into_iter()
        .collect();

        let selected = publisher_direct_dependencies(
            generated.path(),
            declared.clone(),
            OvenLegacyCargoPublicationKind::Executable,
            OvenLegacyCargoDirectDependencyClosure::CheckedDeclared,
        )?;

        assert_eq!(selected, declared);
        Ok(())
    }

    #[test]
    fn publisher_keeps_compiler_owned_route_macro_expansion_roots() -> Result<(), Box<dyn std::error::Error>> {
        let generated = tempfile::tempdir()?;
        fs::create_dir_all(generated.path().join("src"))?;
        fs::write(
            generated.path().join("src/main.rs"),
            "#[incan_web_macros::route(\"/health\")]\nfn health() {}\n",
        )?;
        let declared = [
            ("axum".to_string(), "axum".to_string()),
            ("incan_web_macros".to_string(), "incan_web_macros".to_string()),
            ("inventory".to_string(), "inventory".to_string()),
            ("unused_crate".to_string(), "unused_crate".to_string()),
        ]
        .into_iter()
        .collect();

        let selected = generated_project_direct_dependencies(generated.path(), &declared)?;

        assert_eq!(
            selected,
            [
                ("axum".to_string(), "axum".to_string()),
                ("incan_web_macros".to_string(), "incan_web_macros".to_string()),
                ("inventory".to_string(), "inventory".to_string()),
            ]
            .into_iter()
            .collect()
        );
        Ok(())
    }

    #[test]
    fn compiler_foundation_manifest_excludes_workspace_sources_and_preserves_resolved_features()
    -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let external_root = tempfile::tempdir()?;
        let unused_root = tempfile::tempdir()?;
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::create_dir_all(external_root.path().join("src"))?;
        fs::create_dir_all(unused_root.path().join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"compiler\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(compiler_root.path().join("src/lib.rs"), "pub fn compiler() {}\n")?;
        fs::write(external_root.path().join("src/lib.rs"), "pub fn dependency() {}\n")?;
        fs::write(unused_root.path().join("src/lib.rs"), "pub fn unused() {}\n")?;
        let graph = CargoUnitGraph {
            version: 1,
            units: vec![
                CargoUnitGraphUnit {
                    pkg_id: "compiler 0.1.0 (path+file:///compiler)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "compiler".to_string(),
                        src_path: compiler_root.path().join("src/lib.rs"),
                        edition: "2024".to_string(),
                    },
                    mode: "test".to_string(),
                    platform: Some("aarch64-apple-darwin".to_string()),
                    features: Vec::new(),
                    dependencies: vec![CargoUnitGraphDependency {
                        index: 1,
                        extern_crate_name: Some("external_dep".to_string()),
                    }],
                },
                CargoUnitGraphUnit {
                    pkg_id: "external_dep 1.2.3 (registry+https://example.invalid/index)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "external_dep".to_string(),
                        src_path: external_root.path().join("src/lib.rs"),
                        edition: "2021".to_string(),
                    },
                    mode: "build".to_string(),
                    platform: Some("aarch64-apple-darwin".to_string()),
                    features: vec!["default".to_string(), "serde".to_string()],
                    dependencies: Vec::new(),
                },
                CargoUnitGraphUnit {
                    pkg_id: "unused 9.9.9 (registry+https://example.invalid/index)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "unused".to_string(),
                        src_path: unused_root.path().join("src/lib.rs"),
                        edition: "2021".to_string(),
                    },
                    mode: "build".to_string(),
                    platform: Some("aarch64-apple-darwin".to_string()),
                    features: Vec::new(),
                    dependencies: Vec::new(),
                },
            ],
            roots: vec![0],
        };
        let metadata = CargoMetadata {
            resolve: None,
            packages: vec![
                CargoMetadataPackage {
                    id: "compiler 0.1.0 (path+file:///compiler)".to_string(),
                    name: "compiler".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: compiler_root.path().join("Cargo.toml"),
                    source: None,
                },
                CargoMetadataPackage {
                    id: "external_dep 1.2.3 (registry+https://example.invalid/index)".to_string(),
                    name: "external-dep".to_string(),
                    version: "1.2.3".to_string(),
                    manifest_path: external_root.path().join("Cargo.toml"),
                    source: Some("registry+https://example.invalid/index".to_string()),
                },
                CargoMetadataPackage {
                    id: "unused 9.9.9 (registry+https://example.invalid/index)".to_string(),
                    name: "unused".to_string(),
                    version: "9.9.9".to_string(),
                    manifest_path: unused_root.path().join("Cargo.toml"),
                    source: Some("registry+https://example.invalid/index".to_string()),
                },
            ],
        };

        let dependencies = compiler_suite_foundation_dependencies(compiler_root.path(), &graph, &metadata)?;
        assert_eq!(dependencies.len(), 1, "foundation dependencies: {dependencies:?}");
        assert_eq!(dependencies[0].alias, "oven_foundation_0000");
        assert_eq!(dependencies[0].package, "external-dep");
        assert_eq!(dependencies[0].version, "1.2.3");
        assert_eq!(
            dependencies[0].source.as_deref(),
            Some("registry+https://example.invalid/index")
        );
        assert_eq!(dependencies[0].features, ["default", "serde"]);
        assert_eq!(dependencies[0].path, None);
        let manifest = compiler_suite_foundation_manifest(&dependencies)?;
        assert!(manifest.contains("package = \"external-dep\""));
        assert!(manifest.contains("version = \"=1.2.3\""));
        assert!(manifest.contains("default-features = true"));
        assert!(manifest.contains("features = [\"serde\"]"));
        assert!(manifest.contains("[profile.oven-test]\ninherits = \"dev\"\ndebug = 0\nincremental = false"));
        assert!(!manifest.contains("unused"));
        assert!(!manifest.contains("compiler 0.1.0"));
        let lock = compiler_suite_foundation_lock(
            br#"version = 4

[[package]]
name = "compiler"
version = "0.1.0"

[[package]]
name = "external-dep"
version = "1.2.3"
source = "registry+https://example.invalid/index"
checksum = "fixture"
"#,
            &dependencies,
        )?;
        let lock = toml::from_str::<toml::Value>(std::str::from_utf8(&lock)?)?;
        let foundation = lock["package"]
            .as_array()
            .and_then(|packages| {
                packages
                    .iter()
                    .find(|package| package["name"].as_str() == Some("oven-compiler-foundation"))
            })
            .ok_or_else(|| std::io::Error::other("foundation lock package"))?;
        assert_eq!(foundation["version"].as_str(), Some("0.0.0"));
        let locked_dependencies = foundation["dependencies"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("foundation lock dependencies"))?;
        assert_eq!(locked_dependencies.len(), 1);
        assert_eq!(locked_dependencies[0].as_str(), Some("external-dep"));
        Ok(())
    }

    #[test]
    fn compiler_foundation_manifest_preserves_checked_in_third_party_patch_resolution()
    -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let patch_root = compiler_root.path().join("loaves/third_party/registry_patch");
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::create_dir_all(patch_root.join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"compiler\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(compiler_root.path().join("src/lib.rs"), "pub fn compiler() {}\n")?;
        fs::write(
            patch_root.join("Cargo.toml"),
            "[package]\nname = \"registry-patch\"\nversion = \"1.2.3\"\nedition = \"2024\"\n",
        )?;
        fs::write(patch_root.join("src/lib.rs"), "pub fn patched() {}\n")?;
        let graph = CargoUnitGraph {
            version: 1,
            units: vec![
                CargoUnitGraphUnit {
                    pkg_id: "compiler 0.1.0 (path+file:///compiler)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "compiler".to_string(),
                        src_path: compiler_root.path().join("src/lib.rs"),
                        edition: "2024".to_string(),
                    },
                    mode: "test".to_string(),
                    platform: None,
                    features: Vec::new(),
                    dependencies: vec![CargoUnitGraphDependency {
                        index: 1,
                        extern_crate_name: Some("registry_patch".to_string()),
                    }],
                },
                CargoUnitGraphUnit {
                    pkg_id: "registry-patch 1.2.3 (path+file:///registry-patch)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "registry_patch".to_string(),
                        src_path: patch_root.join("src/lib.rs"),
                        edition: "2024".to_string(),
                    },
                    mode: "build".to_string(),
                    platform: None,
                    features: vec!["default".to_string(), "alloc".to_string()],
                    dependencies: Vec::new(),
                },
            ],
            roots: vec![0],
        };
        let metadata = CargoMetadata {
            resolve: None,
            packages: vec![
                CargoMetadataPackage {
                    id: "compiler 0.1.0 (path+file:///compiler)".to_string(),
                    name: "compiler".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: compiler_root.path().join("Cargo.toml"),
                    source: None,
                },
                CargoMetadataPackage {
                    id: "registry-patch 1.2.3 (path+file:///registry-patch)".to_string(),
                    name: "registry-patch".to_string(),
                    version: "1.2.3".to_string(),
                    manifest_path: patch_root.join("Cargo.toml"),
                    source: None,
                },
            ],
        };
        let dependencies = compiler_suite_foundation_dependencies(compiler_root.path(), &graph, &metadata)?;
        let canonical_patch_root = fs::canonicalize(&patch_root)?;
        assert_eq!(dependencies.len(), 1);
        assert_eq!(dependencies[0].package, "registry-patch");
        assert_eq!(dependencies[0].features, ["alloc", "default"]);
        assert_eq!(dependencies[0].path.as_deref(), Some(canonical_patch_root.as_path()));
        let manifest = compiler_suite_foundation_manifest(&dependencies)?;
        assert!(manifest.contains("[patch.crates-io]"));
        assert!(manifest.contains("registry-patch = { path ="));
        assert!(manifest.contains(&canonical_patch_root.display().to_string()));
        assert!(!manifest.contains("package = \"compiler\""));
        Ok(())
    }

    #[test]
    fn generated_loaf_lock_keeps_compiler_registry_versions_and_adds_local_packages()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let component = project.path().join("component");
        let compiler_component = project.path().join("compiler-component");
        fs::create_dir_all(component.join("src"))?;
        fs::create_dir_all(compiler_component.join("src"))?;
        let root_manifest = concat!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n",
            "[dependencies]\nserde = \"1\"\ncomponent = { path = \"component\" }\n",
            "compiler_component = { package = \"compiler-component\", path = \"compiler-component\" }\n",
        );
        let root_manifest_path = project.path().join("Cargo.toml");
        fs::write(&root_manifest_path, root_manifest)?;
        fs::write(
            component.join("Cargo.toml"),
            "[package]\nname = \"component\"\nversion = \"0.2.0\"\nedition = \"2024\"\n\n[dependencies]\nserde = \"1\"\n",
        )?;
        fs::write(component.join("src/lib.rs"), "pub fn component() {}\n")?;
        fs::write(
            compiler_component.join("Cargo.toml"),
            "[package]\nname = \"compiler-component\"\nversion.workspace = true\nedition.workspace = true\n",
        )?;
        fs::write(
            compiler_component.join("src/lib.rs"),
            "pub fn compiler_component() {}\n",
        )?;
        let compiler_lock = br#"version = 4

[[package]]
name = "compiler-component"
version = "0.5.0"

[[package]]
name = "serde"
version = "0.9.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "old"

[[package]]
name = "serde"
version = "1.0.228"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "selected"
"#;

        let lock = locked_generated_project(&root_manifest_path, root_manifest.as_bytes(), compiler_lock)?;
        validate_generated_registry_lock(compiler_lock, &lock)?;
        let mismatched_lock =
            String::from_utf8(lock.clone())?.replace("checksum = \"selected\"", "checksum = \"drift\"");
        let Err(error) = validate_generated_registry_lock(compiler_lock, mismatched_lock.as_bytes()) else {
            return Err(std::io::Error::other("mismatched registry checksum was accepted").into());
        };
        assert!(error.to_string().contains("disagrees with the checked compiler lock"));
        let lock = toml::from_str::<toml::Value>(std::str::from_utf8(&lock)?)?;
        let packages = lock["package"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("generated package array"))?;
        let fixture = packages
            .iter()
            .find(|package| package["name"].as_str() == Some("fixture"))
            .ok_or_else(|| std::io::Error::other("generated fixture lock root"))?;
        let component = packages
            .iter()
            .find(|package| package["name"].as_str() == Some("component"))
            .ok_or_else(|| std::io::Error::other("generated component lock package"))?;
        let compiler_component = packages
            .iter()
            .find(|package| package["name"].as_str() == Some("compiler-component"))
            .ok_or_else(|| std::io::Error::other("generated detached compiler component lock package"))?;
        let fixture_dependencies = fixture["dependencies"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("generated fixture dependencies"))?;
        assert!(
            fixture_dependencies
                .iter()
                .any(|dependency| dependency.as_str() == Some("component 0.2.0"))
        );
        assert!(
            fixture_dependencies
                .iter()
                .any(|dependency| dependency.as_str() == Some("compiler-component 0.5.0"))
        );
        assert!(
            fixture_dependencies
                .iter()
                .any(|dependency| dependency.as_str() == Some("serde 1.0.228"))
        );
        assert_eq!(
            component["dependencies"]
                .as_array()
                .and_then(|dependencies| dependencies.first())
                .and_then(toml::Value::as_str),
            Some("serde 1.0.228")
        );
        assert_eq!(
            compiler_component["version"].as_str(),
            Some("0.5.0"),
            "a detached compiler-owned package retains the checked local lock version fallback"
        );
        assert_eq!(
            packages
                .iter()
                .filter(|package| package["name"].as_str() == Some("serde"))
                .count(),
            1,
            "unreachable compiler packages are pruned without changing the selected registry version"
        );
        Ok(())
    }

    #[test]
    fn generated_loaf_lock_qualifies_same_named_local_package_versions() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let generated = project.path().join("generated");
        let foo_v1 = project.path().join("foo-v1");
        let foo_v2 = project.path().join("foo-v2");
        fs::create_dir_all(generated.join("src"))?;
        fs::create_dir_all(foo_v1.join("src"))?;
        fs::create_dir_all(foo_v2.join("src"))?;
        let generated_manifest = concat!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n",
            "[dependencies]\n",
            "foo_old = { package = \"foo\", path = \"../foo-v1\" }\n",
            "foo_new = { package = \"foo\", path = \"../foo-v2\" }\n",
        );
        let generated_manifest_path = generated.join("Cargo.toml");
        fs::write(&generated_manifest_path, generated_manifest)?;
        fs::write(generated.join("src/lib.rs"), "pub fn fixture() {}\n")?;
        fs::write(
            foo_v1.join("Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"1.0.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(foo_v1.join("src/lib.rs"), "pub fn foo_v1() {}\n")?;
        fs::write(
            foo_v2.join("Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"2.0.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(foo_v2.join("src/lib.rs"), "pub fn foo_v2() {}\n")?;
        let compiler_lock = br#"version = 4

[[package]]
name = "compiler-only"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "compiler"
"#;

        let lock = locked_generated_project(&generated_manifest_path, generated_manifest.as_bytes(), compiler_lock)?;
        let lock = toml::from_slice::<toml::Value>(&lock)?;
        let packages = lock["package"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("generated package array"))?;
        let fixture = packages
            .iter()
            .find(|package| package["name"].as_str() == Some("fixture"))
            .ok_or_else(|| std::io::Error::other("generated fixture lock root"))?;
        let dependencies = fixture["dependencies"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("generated fixture dependencies"))?
            .iter()
            .filter_map(toml::Value::as_str)
            .collect::<Vec<_>>();
        assert_eq!(dependencies, ["foo 1.0.0", "foo 2.0.0"]);
        assert_eq!(
            packages
                .iter()
                .filter(|package| package["name"].as_str() == Some("foo"))
                .count(),
            2
        );
        Ok(())
    }

    #[test]
    fn generated_lock_inherits_nearest_workspace_package_and_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let workspace = project.path().join("vendor");
        let member = workspace.join("member");
        let support = workspace.join("support");
        let generated = project.path().join("generated");
        fs::create_dir_all(member.join("src"))?;
        fs::create_dir_all(support.join("src"))?;
        fs::create_dir_all(generated.join("src"))?;
        let workspace_manifest_path = workspace.join("Cargo.toml");
        fs::write(
            &workspace_manifest_path,
            concat!(
                "[workspace]\nmembers = [\"member\", \"support\"]\n\n",
                "[workspace.package]\nversion = \"1.2.3\"\n\n",
                "[workspace.dependencies]\n",
                "serde = { version = \"=1.0.228\", features = [\"derive\"] }\n",
                "support = { path = \"support\" }\n",
            ),
        )?;
        let member_manifest = concat!(
            "[package]\nname = \"member\"\nversion.workspace = true\nedition = \"2024\"\n\n",
            "[dependencies]\n",
            "serde = { workspace = true, features = [\"alloc\"] }\n",
            "support.workspace = true\n",
        );
        let member_manifest_path = member.join("Cargo.toml");
        fs::write(&member_manifest_path, member_manifest)?;
        fs::write(member.join("src/lib.rs"), "pub fn member() {}\n")?;
        fs::write(
            support.join("Cargo.toml"),
            "[package]\nname = \"support\"\nversion = \"0.4.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(support.join("src/lib.rs"), "pub fn support() {}\n")?;
        let generated_manifest = concat!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n",
            "[dependencies]\nmember = { path = \"../vendor/member\" }\n",
        );
        let generated_manifest_path = generated.join("Cargo.toml");
        fs::write(&generated_manifest_path, generated_manifest)?;
        fs::write(generated.join("src/lib.rs"), "pub fn fixture() {}\n")?;
        let compiler_lock = br#"version = 4

[[package]]
name = "member"
version = "1.2.3"
dependencies = [
 "serde 1.0.228",
]

[[package]]
name = "serde"
version = "1.0.228"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "first"

[[package]]
name = "serde"
version = "1.0.229"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "second"
"#;

        let first_authority = digest_local_cargo_workspace_authority(&member)?
            .ok_or_else(|| std::io::Error::other("first inherited workspace authority digest"))?;
        let first = locked_generated_project(&generated_manifest_path, generated_manifest.as_bytes(), compiler_lock)?;
        let first_lock = toml::from_slice::<toml::Value>(&first)?;
        let first_packages = first_lock["package"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("first generated workspace package array"))?;
        let first_member = first_packages
            .iter()
            .find(|package| package["name"].as_str() == Some("member"))
            .ok_or_else(|| std::io::Error::other("first generated workspace member"))?;
        assert_eq!(first_member["version"].as_str(), Some("1.2.3"));
        assert!(
            first_packages
                .iter()
                .any(|package| package["name"].as_str() == Some("support")),
            "workspace-relative path dependency was not added to the effective local graph"
        );
        assert!(
            first_packages.iter().any(|package| {
                package["name"].as_str() == Some("serde") && package["version"].as_str() == Some("1.0.228")
            }),
            "workspace-selected registry dependency was not retained"
        );

        fs::write(
            &workspace_manifest_path,
            concat!(
                "[workspace]\nmembers = [\"member\", \"support\"]\n\n",
                "[workspace.package]\nversion = \"1.2.3\"\n\n",
                "[workspace.dependencies]\n",
                "serde = { version = \"=1.0.229\", features = [\"derive\"] }\n",
                "support = { path = \"support\" }\n",
            ),
        )?;
        let second_authority = digest_local_cargo_workspace_authority(&member)?
            .ok_or_else(|| std::io::Error::other("second inherited workspace authority digest"))?;
        assert_ne!(
            first_authority, second_authority,
            "selected workspace dependency authority must change the normal source identity"
        );
        let second = locked_generated_project(&generated_manifest_path, generated_manifest.as_bytes(), compiler_lock)?;
        assert_ne!(
            first, second,
            "selected workspace dependency authority must change the generated lock identity"
        );
        let second_lock = toml::from_slice::<toml::Value>(&second)?;
        let second_packages = second_lock["package"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("second generated workspace package array"))?;
        assert!(
            second_packages.iter().any(|package| {
                package["name"].as_str() == Some("serde") && package["version"].as_str() == Some("1.0.229")
            }),
            "updated workspace registry authority was not selected"
        );
        Ok(())
    }

    #[test]
    fn generated_lock_reports_missing_workspace_authority_without_compiler_lock_fallback()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let member = workspace.path().join("member");
        fs::create_dir_all(member.join("src"))?;
        let workspace_manifest_path = workspace.path().join("Cargo.toml");
        fs::write(&workspace_manifest_path, "[workspace]\nmembers = [\"member\"]\n")?;
        let member_manifest = "[package]\nname = \"member\"\nversion.workspace = true\n";
        let member_manifest_path = member.join("Cargo.toml");
        fs::write(&member_manifest_path, member_manifest)?;
        fs::write(member.join("src/lib.rs"), "pub fn member() {}\n")?;
        let compiler_lock = b"version = 4\n\n[[package]]\nname = \"member\"\nversion = \"9.9.9\"\n";

        let Err(error) = locked_generated_project(&member_manifest_path, member_manifest.as_bytes(), compiler_lock)
        else {
            return Err(std::io::Error::other("missing workspace package version was accepted").into());
        };
        let workspace_manifest_path = fs::canonicalize(&workspace_manifest_path)?;
        let diagnostic = error.to_string();
        assert!(diagnostic.contains(&workspace_manifest_path.display().to_string()));
        assert!(diagnostic.contains("[workspace.package].version"));
        assert!(
            !diagnostic.contains("9.9.9"),
            "a containing workspace must not fall back to a compiler-lock coincidence"
        );

        fs::write(
            &workspace_manifest_path,
            "[workspace]\nmembers = [\"member\"]\n\n[workspace.package]\nversion = \"1.2.3\"\n",
        )?;
        let member_manifest = concat!(
            "[package]\nname = \"member\"\nversion.workspace = true\n\n",
            "[dependencies]\nserde.workspace = true\n",
        );
        fs::write(&member_manifest_path, member_manifest)?;
        let Err(error) = locked_generated_project(&member_manifest_path, member_manifest.as_bytes(), compiler_lock)
        else {
            return Err(std::io::Error::other("missing workspace dependency was accepted").into());
        };
        let diagnostic = error.to_string();
        assert!(diagnostic.contains(&workspace_manifest_path.display().to_string()));
        assert!(diagnostic.contains("[workspace.dependencies].serde"));
        Ok(())
    }

    #[test]
    fn release_cohort_lock_pins_overlap_while_permitting_project_only_packages()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let manifest_path = project.path().join("Cargo.toml");
        let manifest = concat!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n",
            "[dependencies]\nserde = \"1\"\nproject-only = \"2\"\n",
        );
        fs::write(&manifest_path, manifest)?;
        let release_lock = br#"version = 4

[[package]]
name = "serde"
version = "1.0.228"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-serde"
"#;

        let seed = release_cohort_generated_project_lock(&manifest_path, manifest.as_bytes(), release_lock)?;
        let seed_text = std::str::from_utf8(&seed)?;
        assert!(seed_text.contains("name = \"serde\""));
        assert!(seed_text.contains("version = \"1.0.228\""));
        assert!(!seed_text.contains("project-only"));

        let normalized = br#"version = 4

[[package]]
name = "fixture"
version = "0.1.0"
dependencies = [
 "project-only",
 "serde",
]

[[package]]
name = "project-only"
version = "2.4.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "project-only-checksum"

[[package]]
name = "serde"
version = "1.0.228"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-serde"
"#;
        validate_release_cohort_registry_lock(release_lock, &seed, normalized)?;

        let tampered = String::from_utf8(normalized.to_vec())?.replace("release-serde", "tampered");
        let Err(error) = validate_release_cohort_registry_lock(release_lock, &seed, tampered.as_bytes()) else {
            return Err(std::io::Error::other("tampered release-owned checksum was accepted").into());
        };
        assert!(error.to_string().contains("changed the release-derived checksum"));
        Ok(())
    }

    #[test]
    fn release_cohort_lock_allows_cargo_to_prune_an_unreachable_release_node() -> Result<(), Box<dyn std::error::Error>>
    {
        let release_lock = br#"version = 4

[[package]]
name = "atomic-waker"
version = "1.1.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-atomic-waker"
"#;
        let seeded_lock = br#"version = 4

[[package]]
name = "fixture"
version = "0.1.0"

[[package]]
name = "atomic-waker"
version = "1.1.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-atomic-waker"
"#;
        let generated_lock = br#"version = 4

[[package]]
name = "fixture"
version = "0.1.0"
"#;

        validate_release_cohort_registry_lock(release_lock, seeded_lock, generated_lock)?;
        Ok(())
    }

    #[test]
    fn release_cohort_lock_allows_project_only_versions_and_registry_edge_normalization()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let manifest_path = project.path().join("Cargo.toml");
        let manifest = concat!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n",
            "[dependencies]\nshared = \"1\"\ndatafusion-shaped = \"1\"\n",
        );
        fs::write(&manifest_path, manifest)?;
        let release_lock = br#"version = 4

[[package]]
name = "shared"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-shared"
dependencies = ["transitive"]

[[package]]
name = "transitive"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-transitive"
"#;
        let seed = release_cohort_generated_project_lock(&manifest_path, manifest.as_bytes(), release_lock)?;
        let normalized = br#"version = 4

[[package]]
name = "fixture"
version = "0.1.0"
dependencies = [
 "datafusion-shaped",
 "shared",
]

[[package]]
name = "datafusion-shaped"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "project-datafusion"
dependencies = ["transitive 2.0.0"]

[[package]]
name = "shared"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-shared"
dependencies = ["transitive 1.0.0"]

[[package]]
name = "transitive"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-transitive"

[[package]]
name = "transitive"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "project-transitive"
"#;

        validate_release_cohort_registry_lock(release_lock, &seed, normalized)?;

        let normalized_registry_edges = String::from_utf8(normalized.to_vec())?.replace(
            "dependencies = [\"transitive 1.0.0\"]",
            "dependencies = [\"transitive 2.0.0\"]",
        );
        validate_release_cohort_registry_lock(release_lock, &seed, normalized_registry_edges.as_bytes())?;
        Ok(())
    }

    #[test]
    fn release_cohort_lock_allows_pruned_local_edges_but_rejects_new_release_identity_edges()
    -> Result<(), Box<dyn std::error::Error>> {
        let release_lock = br#"version = 4

[[package]]
name = "release-local"
version = "1.0.0"
dependencies = ["shared"]

[[package]]
name = "shared"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-shared"
"#;
        let seeded_lock = br#"version = 4

[[package]]
name = "fixture"
version = "0.1.0"
dependencies = ["release-local"]

[[package]]
name = "release-local"
version = "1.0.0"
dependencies = ["shared"]

[[package]]
name = "shared"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-shared"
"#;
        let generated_lock = br#"version = 4

[[package]]
name = "fixture"
version = "0.1.0"
dependencies = ["release-local"]

[[package]]
name = "project-only"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "project-only"

[[package]]
name = "release-local"
version = "1.0.0"
dependencies = ["project-only", "shared"]

[[package]]
name = "shared"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-shared"
"#;

        let pruned_local_lock = br#"version = 4

[[package]]
name = "fixture"
version = "0.1.0"
dependencies = ["release-local"]

[[package]]
name = "release-local"
version = "1.0.0"
"#;
        validate_release_cohort_registry_lock(release_lock, seeded_lock, pruned_local_lock)?;

        let Err(error) = validate_release_cohort_registry_lock(release_lock, seeded_lock, generated_lock) else {
            return Err(std::io::Error::other("new edge on a retained local release package was accepted").into());
        };
        assert!(
            error
                .to_string()
                .contains("changed the release-derived dependency edges for `release-local` 1.0.0")
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn explicit_project_inspection_reuses_the_release_lock_metadata_walk() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let project = tempfile::tempdir()?;
        let manifest = project.path().join("Cargo.toml");
        fs::write(
            &manifest,
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nlocal = { path = \"local\" }\n",
        )?;
        let local = project.path().join("local");
        fs::create_dir_all(local.join("src"))?;
        fs::write(
            local.join("Cargo.toml"),
            "[package]\nname = \"local\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nhidden = \"1\"\n",
        )?;
        fs::write(local.join("src/lib.rs"), "pub fn local() {}\n")?;
        let registry_source = "registry+https://example.invalid/index";
        let checksum = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let hidden = project.path().join("registry-hidden");
        fs::create_dir_all(hidden.join("src"))?;
        fs::write(
            hidden.join("Cargo.toml"),
            "[package]\nname = \"hidden\"\nversion = \"1.0.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(hidden.join("src/lib.rs"), "pub fn hidden() {}\n")?;
        let release_lock = project.path().join("release-Cargo.lock");
        fs::write(
            &release_lock,
            format!(
                "version = 4\n\n[[package]]\nname = \"hidden\"\nversion = \"1.0.0\"\nsource = \"{registry_source}\"\nchecksum = \"{checksum}\"\n"
            ),
        )?;
        let metadata = project.path().join("metadata.json");
        fs::write(
            &metadata,
            serde_json::to_vec(&serde_json::json!({
                "packages": [{
                    "id": "fixture 0.1.0",
                    "name": "fixture",
                    "version": "0.1.0",
                    "manifest_path": manifest,
                }, {
                    "id": "local 0.1.0",
                    "name": "local",
                    "version": "0.1.0",
                    "manifest_path": local.join("Cargo.toml"),
                }, {
                    "id": "hidden 1.0.0",
                    "name": "hidden",
                    "version": "1.0.0",
                    "manifest_path": hidden.join("Cargo.toml"),
                    "source": registry_source,
                }],
                "resolve": {
                    "root": "fixture 0.1.0",
                    "nodes": [{
                        "id": "fixture 0.1.0",
                        "dependencies": ["local 0.1.0"],
                        "deps": [{ "name": "local", "pkg": "local 0.1.0" }],
                    }, {
                        "id": "local 0.1.0",
                        "dependencies": ["hidden 1.0.0"],
                        "deps": [{ "name": "hidden", "pkg": "hidden 1.0.0" }],
                    }, {
                        "id": "hidden 1.0.0",
                        "dependencies": [],
                        "deps": [],
                    }],
                },
            }))?,
        )?;
        let marker = project.path().join("metadata-invocations");
        let cargo = project.path().join("cargo");
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\nprintf x >> '{}'\ncat '{}'\n",
                marker.display(),
                metadata.display()
            ),
        )?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;
        let staging = tempfile::tempdir()?;

        let sources =
            explicit_project_bake_inspection_sources(&cargo, &manifest, &[], &[], staging.path(), Some(&release_lock))?;

        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].package, "hidden");
        assert_eq!(sources[0].checksum, checksum);
        assert_eq!(
            fs::read(&marker)?,
            b"x",
            "metadata must run once per explicit inspection workspace"
        );
        Ok(())
    }

    #[test]
    fn compiler_suite_source_footprint_is_bound_to_receipt_evidence() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
        let source_text = "pub fn fixture() {}\n";
        fs::write(compiler_root.path().join("src/lib.rs"), source_text)?;
        fs::write(compiler_root.path().join("src/main.rs"), "fn main() {}\n")?;
        let receipt = receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
            compiler_root.path(),
            "compiler-suite-fixture",
            "aarch64-apple-darwin",
            "rustc fixture",
            "debug",
            Vec::new(),
        ))?;
        let target = OvenCompilerTestSuiteTarget {
            package_name: "fixture".to_string(),
            target_name: "fixture".to_string(),
            target_kind: "lib".to_string(),
            runner: "rustc-test".to_string(),
            source_relative_path: "src/lib.rs".to_string(),
            source_evidence_key: "compiler-suite-source:src/lib.rs".to_string(),
            crate_name: "fixture".to_string(),
            edition: "2024".to_string(),
            features: Vec::new(),
            compile_environment: BTreeMap::new(),
            binary_dependencies: Vec::new(),
            workspace_library_dependencies: Vec::new(),
            externs: Vec::new(),
        };

        assert_eq!(
            compiler_suite_verified_target_source_bytes(compiler_root.path(), &receipt, &target)?,
            u64::try_from(source_text.len())?
        );

        fs::write(compiler_root.path().join("src/lib.rs"), "pub fn changed() {}\n")?;
        let error = compiler_suite_verified_target_source_bytes(compiler_root.path(), &receipt, &target)
            .err()
            .ok_or("receipt-mismatched source footprint unexpectedly succeeded")?;
        assert!(error.to_string().contains("does not match its receipt evidence"));
        Ok(())
    }

    #[test]
    fn compiler_target_plan_keeps_workspace_library_edges_out_of_the_cargo_artifact_closure()
    -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let helper_root = compiler_root.path().join("crates/helper");
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::create_dir_all(helper_root.join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"compiler\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
        fs::write(compiler_root.path().join("src/lib.rs"), "pub fn compiler() {}\n")?;
        fs::write(compiler_root.path().join("src/main.rs"), "fn main() {}\n")?;
        fs::write(
            helper_root.join("Cargo.toml"),
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(helper_root.join("src/lib.rs"), "pub fn helper() {}\n")?;
        let receipt = receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
            compiler_root.path(),
            "compiler-suite-fixture",
            "aarch64-apple-darwin",
            "rustc fixture",
            "debug",
            Vec::new(),
        ))?;
        let graph = CargoUnitGraph {
            version: 1,
            units: vec![
                CargoUnitGraphUnit {
                    pkg_id: "helper 0.1.0 (path+file:///helper)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "helper".to_string(),
                        src_path: helper_root.join("src/lib.rs"),
                        edition: "2024".to_string(),
                    },
                    mode: "build".to_string(),
                    platform: Some("aarch64-apple-darwin".to_string()),
                    features: vec!["serde".to_string()],
                    dependencies: Vec::new(),
                },
                CargoUnitGraphUnit {
                    pkg_id: "compiler 0.1.0 (path+file:///compiler)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "compiler".to_string(),
                        src_path: compiler_root.path().join("src/lib.rs"),
                        edition: "2024".to_string(),
                    },
                    mode: "test".to_string(),
                    platform: Some("aarch64-apple-darwin".to_string()),
                    features: Vec::new(),
                    dependencies: vec![CargoUnitGraphDependency {
                        index: 0,
                        extern_crate_name: Some("helper".to_string()),
                    }],
                },
            ],
            roots: vec![1],
        };
        let empty_catalog = CompilerSuiteArtifactCatalog {
            closure: OvenCompilerTestSuiteArtifactClosure {
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
            materialized_files: Vec::new(),
            by_source_path: Default::default(),
        };
        let target = compiler_suite_target_from_unit(
            compiler_root.path(),
            &receipt,
            &graph.units[1],
            &graph,
            &Default::default(),
            &empty_catalog,
        )?;
        assert!(target.externs.is_empty());
        assert_eq!(target.workspace_library_dependencies.len(), 1);
        assert_eq!(target.workspace_library_dependencies[0].package_name, "helper");
        assert_eq!(target.workspace_library_dependencies[0].crate_name, "helper");
        assert_eq!(target.workspace_library_dependencies[0].target_kind, "lib");
        assert_eq!(
            target.workspace_library_dependencies[0].source_relative_path,
            "crates/helper/src/lib.rs"
        );
        assert_eq!(target.workspace_library_dependencies[0].features, ["serde"]);
        let workspace_libraries = compiler_suite_workspace_libraries_for_roots(
            compiler_root.path(),
            &receipt,
            &graph,
            &[1],
            &Default::default(),
            &empty_catalog,
        )?;
        assert_eq!(workspace_libraries.len(), 1);
        assert_eq!(workspace_libraries[0].key, target.workspace_library_dependencies[0]);
        assert_eq!(
            workspace_libraries[0].source_evidence_key,
            "compiler-suite-source:crates/helper/src/lib.rs"
        );
        assert!(workspace_libraries[0].externs.is_empty());
        assert!(workspace_libraries[0].dependencies.is_empty());
        Ok(())
    }

    #[test]
    fn compiler_foundation_partition_is_deterministic_and_leaves_manifest_headroom()
    -> Result<(), Box<dyn std::error::Error>> {
        let files = tempfile::tempdir()?;
        let first = files.path().join("a.rlib");
        let second = files.path().join("b.rlib");
        fs::write(&first, vec![b'a'; 30_000])?;
        fs::write(&second, vec![b'b'; 30_000])?;
        let closure = OvenCompilerTestSuiteArtifactClosure {
            dependency_search_paths: vec!["deps".to_string()],
            native_search_paths: Vec::new(),
            supporting_artifacts: vec![
                OvenRustcSupportingArtifact {
                    relative_path: "deps/a.rlib".to_string(),
                    digest: "sha256:a".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "deps/b.rlib".to_string(),
                    digest: "sha256:b".to_string(),
                },
            ],
        };
        let plans = compiler_suite_foundation_plans(
            &closure,
            &[
                OvenArtifactMaterializedFile {
                    source_path: first,
                    relative_path: "deps/a.rlib".to_string(),
                },
                OvenArtifactMaterializedFile {
                    source_path: second,
                    relative_path: "deps/b.rlib".to_string(),
                },
            ],
            100_000,
        )?;

        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].payload.label, "foundation-0000");
        assert_eq!(plans[1].payload.label, "foundation-0001");
        assert_eq!(plans[0].materialized_files[0].relative_path, "deps/a.rlib");
        assert_eq!(plans[1].materialized_files[0].relative_path, "deps/b.rlib");
        assert!(plans.iter().all(|plan| plan.materialized_files.len() == 1));
        Ok(())
    }

    #[test]
    fn publisher_materializes_compiler_loaf_data_below_its_own_prefix() -> Result<(), Box<dyn std::error::Error>> {
        let toolchain = tempfile::tempdir()?;
        let loafs = toolchain.path().join("share/incan/oven/loafs");
        let loaf = loafs.join("fixture.loaf/loaf.json");
        fs::create_dir_all(loaf.parent().ok_or("Loaf parent missing")?)?;
        fs::write(&loaf, "sealed Loaf")?;

        let files = materialized_files_from_directory(
            &loafs,
            "toolchain-data/share/incan/oven/loafs",
            "compiler-owned Loaf data",
        )?;

        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].relative_path,
            "toolchain-data/share/incan/oven/loafs/fixture.loaf/loaf.json"
        );
        Ok(())
    }

    #[test]
    fn staged_registry_source_excludes_mutable_package_target_output() -> Result<(), Box<dyn std::error::Error>> {
        let source = tempfile::tempdir()?;
        let staging = tempfile::tempdir()?;
        fs::create_dir_all(source.path().join("src"))?;
        fs::create_dir_all(source.path().join("target/debug"))?;
        fs::write(
            source.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"1.0.0\"\n",
        )?;
        fs::write(source.path().join("src/lib.rs"), "pub fn fixture() {}\n")?;
        fs::write(
            source.path().join("target/debug/libfixture.rlib"),
            "mutable cache output",
        )?;

        let (staged, first_digest, _) = stage_registry_source_directory(
            staging.path(),
            "fixture",
            "1.0.0",
            "registry+https://example.invalid/index",
            "fixture-checksum",
            source.path(),
        )?;

        assert!(staged.join("Cargo.toml").is_file());
        assert!(staged.join("src/lib.rs").is_file());
        assert!(!staged.join("target").exists());
        fs::write(source.path().join("target/debug/another.rlib"), "more mutable output")?;
        assert_eq!(first_digest, digest_source_tree(&staged)?);
        Ok(())
    }

    #[test]
    fn compiler_suite_toolchain_loaf_generation_covers_debug_and_release_variants()
    -> Result<(), Box<dyn std::error::Error>> {
        let toolchain = tempfile::tempdir()?;
        let loafs = toolchain.path().join("share/incan/oven/loafs");
        let mut members = Vec::new();
        for (name, profile) in [("debug", "debug"), ("release", "release")] {
            let loaf = OvenLoaf {
                schema_version: OVEN_LOAF_SCHEMA_VERSION,
                build_unit_identity: format!("sha256:{name}"),
                provenance: Default::default(),
                accounting: Default::default(),
                compatibility: Default::default(),
                registry_leaves: Vec::new(),
                plan: OvenRustcArtifactManifest {
                    schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                    intent: OvenBuildIntent {
                        target: "fixture-target".to_string(),
                        toolchain: "fixture-rustc".to_string(),
                        profile: profile.to_string(),
                        features: Vec::new(),
                    },
                    dependency_search_paths: Vec::new(),
                    native_search_paths: Vec::new(),
                    externs: Vec::new(),
                    entrypoint_dependency_search_paths: Default::default(),
                    entrypoint_externs: Default::default(),
                    registry_leaves: Vec::new(),
                    registry_sources: Vec::new(),
                    compile_environment: Default::default(),
                    vocab_auxiliary_targets: Vec::new(),
                    supporting_artifacts: Vec::new(),
                },
            };
            let loaf_identity = oven_store::digest_bytes(&serde_json::to_vec_pretty(&loaf)?);
            let relative = PathBuf::from(format!(
                "generations/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/{}.loaf/loaf.json",
                loaf_identity.strip_prefix("sha256:").unwrap_or(&loaf_identity)
            ));
            let loaf_path = loafs.join(&relative);
            fs::create_dir_all(loaf_path.parent().ok_or("Loaf parent missing")?)?;
            fs::write(loaf_path, serde_json::to_vec_pretty(&loaf)?)?;
            members.push(OvenLoafEnvelopeMember {
                label: name.to_string(),
                profile: profile.to_string(),
                action: "run".to_string(),
                role: OvenLoafMemberRole::CompiledClosure,
                build_unit_identity: format!("sha256:{name}"),
                loaf_identity,
                plan_identity: oven_store::digest_bytes(&serde_json::to_vec(&loaf.plan)?),
                logical_bytes: serde_json::to_vec_pretty(&loaf)?.len() as u64,
                physical_bytes: 0,
                path: relative,
            });
        }
        write_test_loaf_envelope(&loafs, members)?;

        let reference = compiler_suite_toolchain_loaf_generation_reference(&loafs, &BTreeMap::new())?;
        assert_eq!(
            reference.generation_identity,
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );

        // Schema-13 data copies remain readable only for already-stored suite entries. New publication records the
        // generation reference above and does not publish these files again.
        let plans = compiler_suite_toolchain_data_plans(toolchain.path(), 1024 * 1024, &BTreeMap::new())?;
        let paths = plans
            .into_iter()
            .flat_map(|plan| plan.materialized_files)
            .map(|file| file.relative_path)
            .collect::<Vec<_>>();

        assert_eq!(paths.iter().filter(|path| path.ends_with("loaf.json")).count(), 2);
        Ok(())
    }

    #[test]
    fn compiler_suite_toolchain_data_rejects_loaf_for_a_different_sealed_runtime()
    -> Result<(), Box<dyn std::error::Error>> {
        let toolchain = tempfile::tempdir()?;
        let loafs = toolchain.path().join("share/incan/oven/loafs");
        let mut loaf = OvenLoaf {
            schema_version: OVEN_LOAF_SCHEMA_VERSION,
            build_unit_identity: "sha256:fixture".to_string(),
            provenance: Default::default(),
            accounting: Default::default(),
            compatibility: Default::default(),
            registry_leaves: Vec::new(),
            plan: OvenRustcArtifactManifest {
                schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: OvenBuildIntent {
                    target: "fixture-target".to_string(),
                    toolchain: "fixture-rustc".to_string(),
                    profile: "debug".to_string(),
                    features: Vec::new(),
                },
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_dependency_search_paths: Default::default(),
                entrypoint_externs: Default::default(),
                registry_leaves: Vec::new(),
                registry_sources: Vec::new(),
                compile_environment: Default::default(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
        };
        loaf.compatibility
            .runtime_inputs
            .insert("runtime-lock".to_string(), "sha256:old".to_string());
        let loaf_identity = oven_store::digest_bytes(&serde_json::to_vec_pretty(&loaf)?);
        let relative = PathBuf::from(format!(
            "generations/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/{}.loaf/loaf.json",
            loaf_identity.strip_prefix("sha256:").unwrap_or(&loaf_identity)
        ));
        let loaf_path = loafs.join(&relative);
        fs::create_dir_all(loaf_path.parent().ok_or("Loaf parent missing")?)?;
        fs::write(&loaf_path, serde_json::to_vec_pretty(&loaf)?)?;
        write_test_loaf_envelope(
            &loafs,
            vec![OvenLoafEnvelopeMember {
                label: "fixture".to_string(),
                profile: "debug".to_string(),
                action: "run".to_string(),
                role: OvenLoafMemberRole::CompiledClosure,
                build_unit_identity: "sha256:fixture".to_string(),
                loaf_identity,
                plan_identity: oven_store::digest_bytes(&serde_json::to_vec(&loaf.plan)?),
                logical_bytes: serde_json::to_vec_pretty(&loaf)?.len() as u64,
                physical_bytes: 0,
                path: relative,
            }],
        )?;
        let expected = BTreeMap::from([("runtime-lock".to_string(), "sha256:new".to_string())]);

        let error = match compiler_suite_toolchain_loaf_generation_reference(&loafs, &expected) {
            Ok(_) => return Err("a Loaf from another staged SDK runtime must be refused".into()),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("runtime-lock: expected sha256:new, found sha256:old")
        );
        assert!(
            error
                .to_string()
                .contains("regenerate it through the internal compatibility publisher")
        );
        Ok(())
    }

    #[test]
    fn publisher_reclaims_cargo_only_target_files_before_store_materialization()
    -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let target = staging.path().join("target");
        let retained = target.join("aarch64-apple-darwin/debug/deps/libfixture.rlib");
        let discarded_object = target.join("aarch64-apple-darwin/debug/deps/fixture.o");
        let discarded_dep_info = target.join("aarch64-apple-darwin/debug/deps/fixture.d");
        let discarded_profile_file = target.join("aarch64-apple-darwin/debug/fixture");
        fs::create_dir_all(retained.parent().ok_or("retained parent missing")?)?;
        fs::write(&retained, "retained direct-rustc artifact")?;
        fs::write(&discarded_object, "Cargo object")?;
        fs::write(&discarded_dep_info, "Cargo dep-info")?;
        fs::write(&discarded_profile_file, "Cargo executable")?;

        reclaim_unmaterialized_compiler_suite_target_files(
            &target,
            &[OvenArtifactMaterializedFile {
                source_path: retained.clone(),
                relative_path: "target/aarch64-apple-darwin/debug/deps/libfixture.rlib".to_string(),
            }],
        )?;

        assert!(retained.is_file());
        assert!(!discarded_object.exists());
        assert!(!discarded_dep_info.exists());
        assert!(!discarded_profile_file.exists());
        Ok(())
    }

    #[test]
    fn publisher_stages_a_shard_before_reclaiming_its_transient_target() -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let source = staging.path().join("selection-target/debug/deps/libfixture.rlib");
        fs::create_dir_all(source.parent().ok_or("artifact parent missing")?)?;
        fs::write(&source, "verified direct-rustc input")?;
        let materialized = vec![OvenArtifactMaterializedFile {
            source_path: source.clone(),
            relative_path: "target/debug/deps/libfixture.rlib".to_string(),
        }];

        let staged = stage_compiler_suite_shard_files(staging.path(), 7, &materialized)?;

        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].relative_path, materialized[0].relative_path);
        assert!(
            staged[0]
                .source_path
                .starts_with(staging.path().join("prepared-shards/0007"))
        );
        fs::remove_file(source)?;
        assert_eq!(fs::read(&staged[0].source_path)?, b"verified direct-rustc input");
        assert!(fs::symlink_metadata(&staged[0].source_path)?.is_file());
        Ok(())
    }

    #[test]
    fn publisher_reuses_only_a_complete_current_schema_suite() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
        fs::write(compiler_root.path().join("src/lib.rs"), "pub fn fixture() {}\n")?;
        fs::write(compiler_root.path().join("src/main.rs"), "fn main() {}\n")?;
        let receipt = receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
            compiler_root.path(),
            "compiler-suite-fixture",
            "aarch64-apple-darwin",
            "rustc fixture",
            "debug",
            Vec::new(),
        ))?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(store_root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let target = OvenCompilerTestSuiteTarget {
            package_name: "fixture".to_string(),
            target_name: "fixture".to_string(),
            target_kind: "lib".to_string(),
            runner: "rustc-test".to_string(),
            source_relative_path: "src/lib.rs".to_string(),
            source_evidence_key: "compiler-suite-source:src/lib.rs".to_string(),
            crate_name: "fixture".to_string(),
            edition: "2024".to_string(),
            features: vec!["unit-graph-test-mode".to_string()],
            compile_environment: Default::default(),
            binary_dependencies: Vec::new(),
            workspace_library_dependencies: Vec::new(),
            externs: Vec::new(),
        };
        let empty_artifacts = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_dependency_search_paths: Default::default(),
            entrypoint_externs: Default::default(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: Default::default(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let empty_closure = OvenCompilerTestSuiteArtifactClosure {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let schema_eight = OvenCompilerTestSuitePayload {
            schema_version: 8,
            test_targets: vec![target.clone()],
            shard_references: Vec::new(),
            foundation_references: Vec::new(),
            toolchain_data_references: Vec::new(),
            toolchain_loaf_generation: None,
            binary_targets: Vec::new(),
            test_artifact_closure: Some(empty_closure.clone()),
            cli_artifact_closure: None,
            cli_foundation_references: Vec::new(),
            cli_target: None,
            cli_workspace_libraries: Vec::new(),
            sdk_inventory_relative_path: "providers/sdk-inventory.json".to_string(),
            sdk_inventory_digest: "fixture".to_string(),
            toolchain_data_relative_root: None,
            warning_check_artifacts: empty_artifacts.clone(),
        };
        let _ = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "compiler-suite".to_string(),
            kind: OvenArtifactKind::CompilerTestSuite,
            payload: serde_json::to_vec(&schema_eight)?,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })?;
        assert_eq!(select_compiler_test_suite_identity(&store, &receipt)?, None);

        let schema_nine = OvenCompilerTestSuitePayload {
            schema_version: 9,
            test_targets: Vec::new(),
            shard_references: vec![OvenCompilerTestSuiteShardReference {
                identity: "sha256:fixture-shard".to_string(),
                target: target.key(),
                source_bytes: 0,
            }],
            foundation_references: Vec::new(),
            toolchain_data_references: Vec::new(),
            toolchain_loaf_generation: None,
            binary_targets: Vec::new(),
            test_artifact_closure: None,
            cli_artifact_closure: Some(empty_closure.clone()),
            cli_foundation_references: Vec::new(),
            cli_target: Some(target.clone()),
            cli_workspace_libraries: Vec::new(),
            sdk_inventory_relative_path: "providers/sdk-inventory.json".to_string(),
            sdk_inventory_digest: "fixture".to_string(),
            toolchain_data_relative_root: None,
            warning_check_artifacts: empty_artifacts.clone(),
        };
        let _ = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "compiler-suite".to_string(),
            kind: OvenArtifactKind::CompilerTestSuite,
            payload: serde_json::to_vec(&schema_nine)?,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })?;
        assert_eq!(select_compiler_test_suite_identity(&store, &receipt)?, None);

        let schema_fifteen = OvenCompilerTestSuitePayload {
            schema_version: OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION,
            test_targets: Vec::new(),
            shard_references: vec![OvenCompilerTestSuiteShardReference {
                identity: "sha256:fixture-shard".to_string(),
                target: target.key(),
                source_bytes: 1,
            }],
            foundation_references: vec![OvenCompilerTestSuiteFoundationReference {
                identity: "sha256:fixture-foundation".to_string(),
                label: "foundation-0000".to_string(),
            }],
            toolchain_data_references: Vec::new(),
            toolchain_loaf_generation: Some(OvenCompilerTestSuiteToolchainLoafGenerationReference {
                generation_identity: "sha256:fixture-generation".to_string(),
            }),
            binary_targets: Vec::new(),
            test_artifact_closure: None,
            cli_artifact_closure: Some(empty_closure),
            cli_foundation_references: Vec::new(),
            cli_target: Some(target),
            cli_workspace_libraries: Vec::new(),
            sdk_inventory_relative_path: "providers/sdk-inventory.json".to_string(),
            sdk_inventory_digest: "fixture".to_string(),
            toolchain_data_relative_root: None,
            warning_check_artifacts: empty_artifacts,
        };
        let current_manifest = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "compiler-suite".to_string(),
            kind: OvenArtifactKind::CompilerTestSuite,
            payload: serde_json::to_vec(&schema_fifteen)?,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })?;
        assert_eq!(
            select_compiler_test_suite_identity(&store, &receipt)?,
            Some(current_manifest.identity)
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn compiler_suite_prepare_reuses_a_current_suite_without_invoking_cargo() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::unix::fs::PermissionsExt;

        let compiler_root = tempfile::tempdir()?;
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
        fs::write(compiler_root.path().join("src/lib.rs"), "pub fn fixture() {}\n")?;
        fs::write(compiler_root.path().join("src/main.rs"), "fn main() {}\n")?;
        let rustc_output = Command::new("rustup").args(["which", "rustc"]).output()?;
        assert!(rustc_output.status.success(), "rustup which rustc failed");
        let rustc = PathBuf::from(String::from_utf8(rustc_output.stdout)?.trim());
        let receipt = receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
            compiler_root.path(),
            "compiler-suite-fixture",
            "aarch64-apple-darwin",
            rustc_identity(&rustc)?,
            "debug",
            Vec::new(),
        ))?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(store_root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let target = OvenCompilerTestSuiteTarget {
            package_name: "fixture".to_string(),
            target_name: "fixture".to_string(),
            target_kind: "lib".to_string(),
            runner: "rustc-test".to_string(),
            source_relative_path: "src/lib.rs".to_string(),
            source_evidence_key: "compiler-suite-source:src/lib.rs".to_string(),
            crate_name: "fixture".to_string(),
            edition: "2024".to_string(),
            features: Vec::new(),
            compile_environment: Default::default(),
            binary_dependencies: Vec::new(),
            workspace_library_dependencies: Vec::new(),
            externs: Vec::new(),
        };
        let empty_closure = OvenCompilerTestSuiteArtifactClosure {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let suite = OvenCompilerTestSuitePayload {
            schema_version: OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION,
            test_targets: Vec::new(),
            shard_references: vec![OvenCompilerTestSuiteShardReference {
                identity: "sha256:fixture-shard".to_string(),
                target: target.key(),
                source_bytes: 1,
            }],
            foundation_references: vec![OvenCompilerTestSuiteFoundationReference {
                identity: "sha256:fixture-foundation".to_string(),
                label: "foundation-0000".to_string(),
            }],
            toolchain_data_references: Vec::new(),
            toolchain_loaf_generation: Some(OvenCompilerTestSuiteToolchainLoafGenerationReference {
                generation_identity: "sha256:fixture-generation".to_string(),
            }),
            binary_targets: Vec::new(),
            test_artifact_closure: None,
            cli_artifact_closure: Some(empty_closure),
            cli_foundation_references: Vec::new(),
            cli_target: Some(target),
            cli_workspace_libraries: Vec::new(),
            sdk_inventory_relative_path: "providers/sdk-inventory.json".to_string(),
            sdk_inventory_digest: "fixture".to_string(),
            toolchain_data_relative_root: None,
            warning_check_artifacts: OvenRustcArtifactManifest {
                schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: receipt.intent.clone(),
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_dependency_search_paths: Default::default(),
                entrypoint_externs: Default::default(),
                registry_leaves: Vec::new(),
                registry_sources: Vec::new(),
                compile_environment: Default::default(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
        };
        let stored = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "compiler-suite".to_string(),
            kind: OvenArtifactKind::CompilerTestSuite,
            payload: serde_json::to_vec(&suite)?,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })?;
        fs::write(
            compiler_root.path().join("src/lib.rs"),
            "pub fn fixture_changed_without_graph_change() {}\n",
        )?;
        let current_receipt = receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
            compiler_root.path(),
            "compiler-suite-fixture",
            "aarch64-apple-darwin",
            rustc_identity(&rustc)?,
            "debug",
            Vec::new(),
        ))?;
        assert_ne!(receipt.identity, current_receipt.identity);
        assert_eq!(receipt.build_unit_identity, current_receipt.build_unit_identity);
        let fixture = tempfile::tempdir()?;
        let cargo_marker = fixture.path().join("unexpected-cargo-invocation");
        let cargo = fixture.path().join("cargo");
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"{}\"\nexit 97\n",
                cargo_marker.display()
            ),
        )?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;

        let result = prepare_compiler_test_suite(&OvenLegacyCargoPrepareRequest {
            compiler: CompilerIdentity::new("0.0.0-test", 0),
            provider_hooks: Arc::new(oven_store::NoProviderHooks),
            store: &store,
            receipt: current_receipt,
            generated_project: fixture.path().join("unused-generated-project"),
            cargo,
            rustc,
            sdk_inventory: None,
            compiler_loaf_root: None,
            domain: "compiler-suite".to_string(),
            publication_kind: OvenLegacyCargoPublicationKind::LibraryTests,
            source_evidence_key: oven_store::COMPILER_WORKSPACE_MANIFEST_EVIDENCE_KEY.to_string(),
            compile_environment: Default::default(),
            inspection_packages: Some(Vec::new()),
            direct_dependency_closure: OvenLegacyCargoDirectDependencyClosure::CheckedDeclared,
            provider_compilations: &[],
            compact_debug_info: false,
            source_compiler_vocab_support: false,
            base_loaf: None,
        })?;

        assert_eq!(result.suite_identity, stored.identity);
        assert_eq!(result.cargo_version, "not-run-existing-suite");
        assert_eq!(result.cargo_manifest_digest, "not-run-existing-suite");
        assert_eq!(result.cargo_lock_digest, "not-run-existing-suite");
        assert_eq!(result.transient_reservation_bytes, 0);
        assert_eq!(result.timing.unit_graph_elapsed_ms, 0);
        assert_eq!(result.timing.foundation_build_elapsed_ms, 0);
        assert_eq!(result.timing.direct_plan_elapsed_ms, 0);
        assert_eq!(result.timing.store_publication_elapsed_ms, 0);
        let timing = serde_json::to_value(&result)?;
        assert!(timing["timing"].get("preflight_and_sdk_elapsed_ms").is_some());
        assert!(
            !cargo_marker.exists(),
            "a compatible stored suite must return before invoking the supplied Cargo executable"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn project_extension_reuse_requires_a_complete_current_payload() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let generated_root = project.path().join("src/main.rs");
        fs::create_dir_all(generated_root.parent().ok_or("generated source parent missing")?)?;
        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"oven_schema_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(&generated_root, "fn main() {}\n")?;
        let rustc_output = Command::new("rustup").args(["which", "rustc"]).output()?;
        assert!(rustc_output.status.success(), "rustup which rustc failed");
        let rustc = PathBuf::from(String::from_utf8(rustc_output.stdout)?.trim());
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "oven_schema_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated_root),
        )?;
        let base_plan = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: vec!["deps".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![OvenRustcArtifactExtern {
                crate_name: "incan_std_core".to_string(),
                relative_path: "deps/libincan_std_core-release.rlib".to_string(),
                digest: digest_bytes(b"release stdlib"),
            }],
            entrypoint_dependency_search_paths: Default::default(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let mut publisher_plan = base_plan.clone();
        publisher_plan.externs = vec![
            OvenRustcArtifactExtern {
                crate_name: "incan_std_core".to_string(),
                relative_path: "deps/libincan_std_core-project.rlib".to_string(),
                digest: digest_bytes(b"project stdlib"),
            },
            OvenRustcArtifactExtern {
                crate_name: "project_dep".to_string(),
                relative_path: "deps/libproject_dep.rlib".to_string(),
                digest: digest_bytes(b"project dependency"),
            },
        ];
        let complete_plan =
            publisher_plan.with_release_cohort_from_base(&base_plan, &std::collections::BTreeSet::new())?;
        let partition = complete_plan.partition_against_base(&base_plan)?;
        assert!(!partition.base_paths.is_empty());
        assert!(!partition.extension_paths.is_empty());
        let base_identity = "sha256:release-base".to_string();
        let base = OvenLegacyCargoBaseLoaf {
            loaf_identity: base_identity.clone(),
            build_unit_identity: receipt.build_unit_identity.clone(),
            artifacts: &base_plan,
            artifact_root: project.path(),
        };
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(store_root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let payload = |schema_version| OvenProjectExtensionPayload {
            schema_version,
            base_loaf_identity: base_identity.clone(),
            base_build_unit_identity: receipt.build_unit_identity.clone(),
            publisher_plan: publisher_plan.clone(),
            complete_plan: complete_plan.clone(),
            registry_source_dependencies: Vec::new(),
            dev_registry_source_dependencies: Vec::new(),
            extension_paths: partition.extension_paths.iter().cloned().collect(),
        };
        let mut stale_payload = serde_json::to_value(payload(OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION - 1))?;
        let _ = stale_payload
            .as_object_mut()
            .ok_or("project extension fixture payload is not an object")?
            .remove("registry_source_dependencies");
        let _stale = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "incan-release-fixture".to_string(),
            kind: OvenArtifactKind::ProjectPayload,
            payload: serde_json::to_vec(&stale_payload)?,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })?;

        assert_eq!(
            select_existing_project_extension_identity(&store, &receipt, &base)?,
            None
        );

        let mut complete_plan_drift = payload(OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION);
        let project_dependency = complete_plan_drift
            .complete_plan
            .externs
            .iter_mut()
            .find(|artifact| artifact.crate_name == "project_dep")
            .ok_or("project dependency missing from complete fixture plan")?;
        project_dependency.digest = digest_bytes(b"drifted project dependency");
        store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "incan-release-fixture".to_string(),
            kind: OvenArtifactKind::ProjectPayload,
            payload: serde_json::to_vec(&complete_plan_drift)?,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })?;
        let mut extension_paths_drift = payload(OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION);
        let _ = extension_paths_drift.extension_paths.pop();
        store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "incan-release-fixture".to_string(),
            kind: OvenArtifactKind::ProjectPayload,
            payload: serde_json::to_vec(&extension_paths_drift)?,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })?;
        assert_eq!(
            select_existing_project_extension_identity(&store, &receipt, &base)?,
            None,
            "current-schema payload drift must trigger a replacement bake"
        );

        let current = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "incan-release-fixture".to_string(),
            kind: OvenArtifactKind::ProjectPayload,
            payload: serde_json::to_vec(&payload(OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION))?,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })?;
        assert_eq!(
            select_existing_project_extension_identity(&store, &receipt, &base)?,
            Some(current.identity)
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn generated_project_prepare_reuses_a_current_plan_without_invoking_cargo() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::unix::fs::PermissionsExt;

        let project = tempfile::tempdir()?;
        let generated_root = project.path().join("src/main.rs");
        fs::create_dir_all(generated_root.parent().ok_or("generated source parent missing")?)?;
        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"oven_reuse_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(&generated_root, "fn main() {}\n")?;
        let rustc_output = Command::new("rustup").args(["which", "rustc"]).output()?;
        assert!(rustc_output.status.success(), "rustup which rustc failed");
        let rustc = PathBuf::from(String::from_utf8(rustc_output.stdout)?.trim());
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "oven_reuse_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated_root),
        )?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(store_root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let plan = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_dependency_search_paths: Default::default(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: Default::default(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let stored = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "incan-release-fixture".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&plan)?,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })?;
        let fixture = tempfile::tempdir()?;
        let cargo_marker = fixture.path().join("unexpected-cargo-invocation");
        let cargo = fixture.path().join("cargo");
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"{}\"\nexit 97\n",
                cargo_marker.display()
            ),
        )?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;

        let result = prepare_direct_rustc_plan(&OvenLegacyCargoPrepareRequest {
            compiler: CompilerIdentity::new("0.0.0-test", 0),
            provider_hooks: Arc::new(oven_store::NoProviderHooks),
            store: &store,
            receipt,
            generated_project: project.path().to_path_buf(),
            cargo,
            rustc,
            sdk_inventory: None,
            compiler_loaf_root: None,
            domain: "incan-release-fixture".to_string(),
            publication_kind: OvenLegacyCargoPublicationKind::Executable,
            source_evidence_key: "generated-root".to_string(),
            compile_environment: Default::default(),
            inspection_packages: None,
            direct_dependency_closure: OvenLegacyCargoDirectDependencyClosure::GeneratedSource,
            provider_compilations: &[],
            compact_debug_info: false,
            source_compiler_vocab_support: false,
            base_loaf: None,
        })?;

        assert_eq!(result.plan_identity, stored.identity);
        assert_eq!(result.cargo_version, "not-run-existing-plan");
        assert_eq!(result.transient_reservation_bytes, 0);
        assert!(
            !cargo_marker.exists(),
            "a compatible stored project Loaf must return before invoking the supplied Cargo executable"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stable_release_cargo_invocation_omits_unit_graph_flags() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let fixture = tempfile::tempdir()?;
        let manifest = fixture.path().join("Cargo.toml");
        fs::write(&manifest, "[package]\nname='fixture'\nversion='0.1.0'\n")?;
        let log = fixture.path().join("cargo-args");
        let cargo = fixture.path().join("cargo");
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\nprintf '%s\\n' '{{\"reason\":\"incan-rustc-invocation\",\"rustc\":\"rustc\",\"arguments\":[\"--crate-name\",\"fixture\"],\"environment\":{{}}}}' > \"$INCAN_OVEN_RUSTC_TRACE_PATH\"\nprintf '%s\\n' '{{\"reason\":\"build-finished\",\"success\":true}}'\n",
                log.display()
            ),
        )?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;
        let rustc = fixture.path().join("rustc");
        fs::write(&rustc, "#!/bin/sh\nexit 0\n")?;
        fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755))?;
        let target = fixture.path().join("target");
        let staging = fixture.path().join("staging");
        fs::create_dir(&staging)?;

        run_legacy_cargo_invocation(
            &cargo,
            &rustc,
            &manifest,
            &target,
            &staging,
            "aarch64-apple-darwin",
            "debug",
            &[],
            u64::MAX,
            "build",
            &OvenLegacyCargoInvocationTarget::None,
            false,
            false,
            false,
        )?;

        let arguments = fs::read_to_string(log)?;
        assert!(!arguments.contains("-Z"));
        assert!(!arguments.contains("--unit-graph"));
        Ok(())
    }

    #[test]
    fn direct_rustc_environment_captures_generated_package_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source = project.path().join("src/main.rs");
        fs::create_dir_all(source.parent().ok_or("source parent missing")?)?;
        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"native_seed\"\nversion = \"7.2.1\"\n",
        )?;
        fs::write(&source, "fn main() {}\n")?;

        let environment = direct_rustc_compile_environment(project.path(), &source)?;

        assert_eq!(
            environment.get("CARGO_MANIFEST_DIR"),
            Some(&"@oven-source-ancestor:2".to_string())
        );
        assert_eq!(environment.get("CARGO_PKG_NAME"), Some(&"native_seed".to_string()));
        assert_eq!(environment.get("CARGO_PKG_VERSION"), Some(&"7.2.1".to_string()));
        Ok(())
    }

    #[test]
    fn reusable_project_plan_environment_excludes_generated_package_metadata() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        let source = project.path().join("src/main.rs");
        fs::create_dir_all(source.parent().ok_or("source parent missing")?)?;
        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"shared_extension\"\nversion = \"8.1.0\"\n",
        )?;
        fs::write(&source, "fn main() {}\n")?;

        let environment = direct_rustc_reusable_project_plan_environment(project.path(), &source)?;

        assert_eq!(
            environment.get("CARGO_MANIFEST_DIR"),
            Some(&"@oven-source-ancestor:2".to_string())
        );
        assert!(!environment.contains_key("CARGO_PKG_NAME"));
        assert!(!environment.contains_key("CARGO_PKG_VERSION"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn interop_bootstrap_publisher_compiles_the_companion_library_before_native_interop_is_sealed()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = tempfile::tempdir()?;
        let project = fixture.path().join("generated-project");
        let source = project.join("src/main.rs");
        fs::create_dir_all(source.parent().ok_or("source parent missing")?)?;
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\nname = \"fixture\"\npath = \"src/main.rs\"\n\n[[bin]]\nname = \"fixture\"\npath = \"src/main.rs\"\n",
        )?;
        // A binary target would need this absent native library at link time. The compatibility publisher must
        // instead compile the companion library target and leave native linking to the sealed interop plan.
        fs::write(
            &source,
            "#[link(name = \"not-yet-sealed-native\")]\nunsafe extern \"C\" {}\nfn main() {}\n",
        )?;
        fs::write(
            project.join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        )?;
        let cargo_output = Command::new("rustup").args(["which", "cargo"]).output()?;
        assert!(cargo_output.status.success(), "rustup which cargo failed");
        let cargo = PathBuf::from(String::from_utf8(cargo_output.stdout)?.trim());
        let rustc_output = Command::new("rustup").args(["which", "rustc"]).output()?;
        assert!(rustc_output.status.success(), "rustup which rustc failed");
        let rustc = PathBuf::from(String::from_utf8(rustc_output.stdout)?.trim());

        let outputs = run_legacy_cargo(
            &cargo,
            &rustc,
            &project.join("Cargo.toml"),
            &fixture.path().join("target"),
            &rustc_host_target(&rustc)?,
            "debug",
            &[],
            1024 * 1024,
            OvenLegacyCargoPublicationKind::InteropBootstrap,
            false,
            false,
        )?;

        assert_eq!(outputs.len(), 1);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn publisher_uses_network_only_for_a_fresh_explicit_project_resolution() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let fixture = tempfile::tempdir()?;
        let project = fixture.path().join("generated-project");
        let source = project.join("src/main.rs");
        let cargo = fixture.path().join("cargo");
        let rustc = fixture.path().join("rustc");
        let observed_directory = fixture.path().join("cargo-current-directory");
        let observed_debug_setting = fixture.path().join("cargo-debug-setting");
        let observed_incremental_setting = fixture.path().join("cargo-incremental-setting");
        let observed_arguments = fixture.path().join("cargo-arguments");
        fs::create_dir_all(source.parent().ok_or("source parent missing")?)?;
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(&source, "fn main() {}\n")?;
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\npwd > \"{}\"\nprintf '%s' \"$CARGO_PROFILE_DEV_DEBUG\" > \"{}\"\nprintf '%s' \"$CARGO_INCREMENTAL\" > \"{}\"\nprintf '%s\\n' \"$@\" > \"{}\"\n",
                observed_directory.display(),
                observed_debug_setting.display(),
                observed_incremental_setting.display(),
                observed_arguments.display(),
            ),
        )?;
        fs::write(&rustc, "#!/bin/sh\nexit 0\n")?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755))?;

        let _ = run_legacy_cargo_invocation(
            &cargo,
            &rustc,
            &project.join("Cargo.toml"),
            &fixture.path().join("target"),
            fixture.path(),
            "aarch64-apple-darwin",
            OVEN_COMPILER_TEST_PROFILE,
            &[],
            1024 * 1024,
            "build",
            &OvenLegacyCargoInvocationTarget::None,
            false,
            false,
            false,
        )?;

        assert_eq!(
            fs::canonicalize(&project)?,
            fs::canonicalize(fs::read_to_string(observed_directory)?.trim())?
        );
        assert_eq!(fs::read_to_string(observed_debug_setting)?, "");
        assert_eq!(fs::read_to_string(observed_incremental_setting)?, "");
        let arguments = fs::read_to_string(&observed_arguments)?;
        assert!(arguments.lines().any(|argument| argument == "--profile"));
        assert!(arguments.lines().any(|argument| argument == OVEN_COMPILER_TEST_PROFILE));
        assert!(
            !arguments.lines().any(|argument| argument == "--offline"),
            "a fresh explicit project bake must be able to establish its initial Cargo.lock"
        );
        assert!(
            !arguments.lines().any(|argument| argument == "--locked"),
            "a fresh explicit project bake cannot require a lock before Cargo has created it"
        );

        fs::write(project.join("Cargo.lock"), "version = 4\n")?;
        let _ = run_legacy_cargo_invocation(
            &cargo,
            &rustc,
            &project.join("Cargo.toml"),
            &fixture.path().join("target"),
            fixture.path(),
            "aarch64-apple-darwin",
            OVEN_COMPILER_TEST_PROFILE,
            &[],
            1024 * 1024,
            "build",
            &OvenLegacyCargoInvocationTarget::None,
            false,
            false,
            false,
        )?;
        let locked_arguments = fs::read_to_string(&observed_arguments)?;
        assert!(locked_arguments.lines().any(|argument| argument == "--offline"));
        assert!(locked_arguments.lines().any(|argument| argument == "--locked"));
        Ok(())
    }

    #[test]
    fn explicit_project_bake_creates_and_reuses_a_local_cargo_lock_issue1196() -> Result<(), Box<dyn std::error::Error>>
    {
        let fixture = tempfile::tempdir()?;
        let project = fixture.path().join("generated-project");
        let helper = project.join("helper");
        fs::create_dir_all(project.join("src"))?;
        fs::create_dir_all(helper.join("src"))?;
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nhelper = { path = \"helper\" }\n",
        )?;
        fs::write(project.join("src/main.rs"), "fn main() { helper::marker(); }\n")?;
        fs::write(
            helper.join("Cargo.toml"),
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(helper.join("src/lib.rs"), "pub fn marker() {}\n")?;

        let cargo = crate::cargo_process::resolved_cargo_executable()?;
        let manifest = project.join("Cargo.toml");
        let staging = fixture.path().join("staging");
        let initial_sources = explicit_project_bake_inspection_sources(&cargo, &manifest, &[], &[], &staging, None)?;
        assert!(
            initial_sources.is_empty(),
            "the local-only fixture has no registry sources to stage"
        );
        assert!(
            project.join("Cargo.lock").is_file(),
            "the one explicit bake boundary must establish the missing Cargo.lock"
        );

        let replayed_sources = legacy_cargo_inspection_sources(&cargo, &manifest, &[], &[], &staging)?;
        assert!(
            replayed_sources.is_empty(),
            "the newly created lock must support the locked publisher replay"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn fresh_explicit_project_metadata_uses_online_then_locked_policy_issue1196()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let fixture = tempfile::tempdir()?;
        let project = fixture.path().join("generated-project");
        let cargo = fixture.path().join("cargo");
        let observed_arguments = fixture.path().join("cargo-metadata-arguments");
        fs::create_dir_all(&project)?;
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nif [ ! -f Cargo.lock ]; then printf 'version = 4\\n' > Cargo.lock; fi\nprintf '%s\\n' '{{\"packages\":[],\"resolve\":null}}'\n",
                observed_arguments.display(),
            ),
        )?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;
        let manifest = project.join("Cargo.toml");

        let _ = read_legacy_cargo_metadata_with_lock_policy(&cargo, &manifest, &[], false)?;
        let initial_arguments = fs::read_to_string(&observed_arguments)?;
        assert!(
            !initial_arguments.contains("--offline") && !initial_arguments.contains("--locked"),
            "the first explicit project metadata resolve must be allowed to establish Cargo.lock: {initial_arguments}"
        );
        assert!(project.join("Cargo.lock").is_file());

        let _ = read_legacy_cargo_metadata_with_lock_policy(&cargo, &manifest, &[], true)?;
        let invocations = fs::read_to_string(&observed_arguments)?;
        let replay_arguments = invocations.lines().last().ok_or("missing replay metadata invocation")?;
        assert!(
            replay_arguments.contains("--offline") && replay_arguments.contains("--locked"),
            "every later publisher metadata read must be locked and offline: {replay_arguments}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn cargo_monitor_counts_the_whole_private_publisher_staging_root() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        let fixture = tempfile::tempdir()?;
        let cargo = fixture.path().join("cargo");
        let rustc = fixture.path().join("rustc");
        let manifest = fixture.path().join("Cargo.toml");
        let staging = fixture.path().join("legacy-cargo-staging");
        let target = staging.join("current-target");
        let retained_output = staging.join("earlier-target/overflow");
        let descendant_pid = fixture.path().join("cargo-descendant-pid");
        fs::create_dir_all(retained_output.parent().ok_or("retained output parent missing")?)?;
        fs::write(&manifest, "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n")?;
        // Zero-filled files can remain sparse or compressed on APFS, which does not exercise the physical-byte limit.
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\nsleep 30 &\nprintf '%s\\n' \"$!\" > \"{}\"\ndd if=/dev/urandom of=\"{}\" bs=131072 count=1 2>/dev/null\nwait\n",
                descendant_pid.display(),
                retained_output.display(),
            ),
        )?;
        fs::write(&rustc, "#!/bin/sh\nexit 0\n")?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755))?;

        let started = Instant::now();
        let result = run_legacy_cargo_invocation(
            &cargo,
            &rustc,
            &manifest,
            &target,
            &staging,
            "aarch64-apple-darwin",
            "debug",
            &[],
            64 * 1024,
            "build",
            &OvenLegacyCargoInvocationTarget::None,
            false,
            false,
            false,
        );

        assert!(matches!(
            result,
            Err(super::OvenLegacyCargoError::TransientCapacityExceeded { path, .. }) if path == staging
        ));
        // The fixture's Cargo sleeps 30s, so this asserts the monitor aborted on the capacity breach rather than
        // waiting for the child. The bound is deliberately loose: it only has to separate "aborted" from "waited",
        // and a tight one measures machine load instead. At 2s this failed under the parallel suite while taking
        // 0.26s in isolation, which tested the host rather than the monitor.
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(10),
            "monitor should abort on the capacity breach, not wait for the 30s child; took {elapsed:?}",
        );
        fs::remove_dir_all(&staging)?;
        assert!(
            !staging.exists(),
            "capacity-aborted publisher staging was not removable"
        );
        let pid = fs::read_to_string(descendant_pid)?.trim().parse::<u32>()?;
        for _ in 0..100 {
            if !oven_store::process::process_is_running(pid)? {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Err("capacity abort left the fake-Cargo descendant running".into())
    }

    #[test]
    fn publisher_capacity_monitor_yields_after_a_long_full_tree_scan() {
        assert_eq!(
            super::publisher_capacity_probe_delay(std::time::Duration::from_millis(5)),
            super::PUBLISHER_CAPACITY_POLL_INTERVAL
        );
        assert_eq!(
            super::publisher_capacity_probe_delay(std::time::Duration::from_secs(2)),
            std::time::Duration::from_secs(2)
        );
    }

    #[cfg(unix)]
    #[test]
    fn compiler_suite_bootstrap_builds_the_declared_workspace_test_units() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let fixture = tempfile::tempdir()?;
        let cargo = fixture.path().join("cargo");
        let rustc = fixture.path().join("rustc");
        let arguments = fixture.path().join("cargo-arguments");
        fs::write(
            &cargo,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n", arguments.display()),
        )?;
        fs::write(&rustc, "#!/bin/sh\nexit 0\n")?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755))?;
        let manifest = fixture.path().join("Cargo.toml");
        fs::write(&manifest, "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n")?;

        let _ = run_legacy_cargo_invocation(
            &cargo,
            &rustc,
            &manifest,
            &fixture.path().join("target"),
            fixture.path(),
            "aarch64-apple-darwin",
            "debug",
            &[],
            1024 * 1024,
            "test",
            &OvenLegacyCargoInvocationTarget::WorkspaceTests,
            false,
            false,
            false,
        )?;
        let arguments = fs::read_to_string(arguments)?;
        assert!(arguments.lines().any(|argument| argument == "--all"));
        assert!(arguments.lines().any(|argument| argument == "--no-run"));
        assert!(!arguments.lines().any(|argument| argument == "--lib"));
        assert!(!arguments.lines().any(|argument| argument == "--bin"));
        Ok(())
    }

    #[test]
    fn publisher_retains_transitive_external_proc_macro_externs() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let staging = tempfile::tempdir()?;
        let root_source = compiler_root.path().join("src/lib.rs");
        fs::create_dir_all(root_source.parent().ok_or("root source parent missing")?)?;
        fs::write(&root_source, "pub fn fixture() {}\n")?;

        let serde_source = staging.path().join("registry/serde/src/lib.rs");
        let serde_core_source = staging.path().join("registry/serde_core/src/lib.rs");
        let serde_derive_source = staging.path().join("registry/serde_derive/src/lib.rs");
        let macro_build_input_source = staging.path().join("registry/macro_build_input/src/lib.rs");
        for source in [
            &serde_source,
            &serde_core_source,
            &serde_derive_source,
            &macro_build_input_source,
        ] {
            fs::create_dir_all(source.parent().ok_or("registry source parent missing")?)?;
            fs::write(source, "pub fn fixture() {}\n")?;
        }
        let target_dependencies = staging.path().join("target/aarch64-apple-darwin/debug/deps");
        let host_dependencies = staging.path().join("target/debug/deps");
        fs::create_dir_all(&target_dependencies)?;
        fs::create_dir_all(&host_dependencies)?;
        let serde_artifact = target_dependencies.join("libserde-abc123.rlib");
        let host_serde_artifact = host_dependencies.join("libserde-host456.rlib");
        let serde_core_artifact = target_dependencies.join("libserde_core-abc123.rlib");
        let serde_derive_artifact = host_dependencies.join("libserde_derive-abc123.dylib");
        let macro_build_input_artifact = host_dependencies.join("libmacro_build_input-abc123.dylib");
        fs::write(&serde_artifact, "serde library")?;
        fs::write(&host_serde_artifact, "host serde library")?;
        fs::write(&serde_core_artifact, "serde core library")?;
        fs::write(&serde_derive_artifact, "serde derive proc macro")?;
        fs::write(&macro_build_input_artifact, "proc-macro compiler-only input")?;

        let graph = CargoUnitGraph {
            version: 1,
            units: vec![
                CargoUnitGraphUnit {
                    pkg_id: "fixture 0.1.0 (path+file:///fixture)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "fixture".to_string(),
                        src_path: root_source.clone(),
                        edition: "2024".to_string(),
                    },
                    mode: "test".to_string(),
                    platform: Some("aarch64-apple-darwin".to_string()),
                    features: Vec::new(),
                    dependencies: vec![CargoUnitGraphDependency {
                        index: 1,
                        extern_crate_name: Some("serde".to_string()),
                    }],
                },
                CargoUnitGraphUnit {
                    pkg_id: "serde 1.0.0 (registry+https://example.invalid)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "serde".to_string(),
                        src_path: serde_source.clone(),
                        edition: "2024".to_string(),
                    },
                    mode: "build".to_string(),
                    // Cargo can describe this unit as host-side when it is reached through a proc-macro root even
                    // though Oven recompiles the workspace library with its explicit receipt target.
                    platform: None,
                    features: vec!["derive".to_string()],
                    dependencies: vec![
                        CargoUnitGraphDependency {
                            index: 2,
                            extern_crate_name: Some("serde_core".to_string()),
                        },
                        CargoUnitGraphDependency {
                            index: 3,
                            extern_crate_name: Some("serde_derive".to_string()),
                        },
                    ],
                },
                CargoUnitGraphUnit {
                    pkg_id: "serde_core 1.0.0 (registry+https://example.invalid)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "serde_core".to_string(),
                        src_path: serde_core_source.clone(),
                        edition: "2024".to_string(),
                    },
                    mode: "build".to_string(),
                    platform: Some("aarch64-apple-darwin".to_string()),
                    features: Vec::new(),
                    dependencies: Vec::new(),
                },
                CargoUnitGraphUnit {
                    pkg_id: "serde_derive 1.0.0 (registry+https://example.invalid)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["proc-macro".to_string()],
                        crate_types: vec!["proc-macro".to_string()],
                        name: "serde_derive".to_string(),
                        src_path: serde_derive_source.clone(),
                        edition: "2024".to_string(),
                    },
                    mode: "build".to_string(),
                    platform: None,
                    features: Vec::new(),
                    dependencies: vec![CargoUnitGraphDependency {
                        index: 4,
                        extern_crate_name: Some("macro_build_input".to_string()),
                    }],
                },
                CargoUnitGraphUnit {
                    pkg_id: "macro_build_input 1.0.0 (registry+https://example.invalid)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["proc-macro".to_string()],
                        crate_types: vec!["proc-macro".to_string()],
                        name: "macro_build_input".to_string(),
                        src_path: macro_build_input_source.clone(),
                        edition: "2024".to_string(),
                    },
                    mode: "build".to_string(),
                    platform: None,
                    features: Vec::new(),
                    dependencies: Vec::new(),
                },
            ],
            roots: vec![0],
        };
        let serde_artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "serde 1.0.0 (registry+https://example.invalid)",
            "target": { "name": "serde", "src_path": serde_source },
            "features": ["derive"],
            "filenames": [serde_artifact],
            "profile": { "test": false },
        });
        let serde_core_artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "serde_core 1.0.0 (registry+https://example.invalid)",
            "target": { "name": "serde_core", "src_path": serde_core_source },
            "features": [],
            "filenames": [serde_core_artifact],
            "profile": { "test": false },
        });
        let host_serde_artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "serde 1.0.0 (registry+https://example.invalid)",
            "target": { "name": "serde", "src_path": serde_source },
            "features": ["derive"],
            "filenames": [host_serde_artifact],
            "profile": { "test": false },
        });
        let serde_derive_artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "serde_derive 1.0.0 (registry+https://example.invalid)",
            "target": { "name": "serde_derive", "src_path": serde_derive_source },
            "features": [],
            "filenames": [serde_derive_artifact],
            "profile": { "test": false },
        });
        let macro_build_input_artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "macro_build_input 1.0.0 (registry+https://example.invalid)",
            "target": { "name": "macro_build_input", "src_path": macro_build_input_source },
            "features": [],
            "filenames": [macro_build_input_artifact],
            "profile": { "test": false },
        });
        let output = CargoInvocationOutput {
            stdout: format!(
                "{serde_artifact_message}\n{host_serde_artifact_message}\n{serde_core_artifact_message}\n{serde_derive_artifact_message}\n{macro_build_input_artifact_message}\n"
            )
            .into_bytes(),
        };
        let artifact_index = compiler_suite_artifact_index(&output, "aarch64-apple-darwin")?;
        let catalog = compiler_suite_artifact_catalog(staging.path(), &[target_dependencies, host_dependencies], &[])?;

        let (externs, workspace_dependencies) = compiler_suite_target_externs(
            &graph.units[0],
            &graph,
            compiler_root.path(),
            &artifact_index,
            &catalog,
            "aarch64-apple-darwin",
        )?;

        assert!(workspace_dependencies.is_empty());
        assert_eq!(
            externs
                .iter()
                .map(|artifact| artifact.crate_name.as_str())
                .collect::<Vec<_>>(),
            ["serde", "serde_derive"]
        );
        assert!(externs[0].relative_path.ends_with("libserde-abc123.rlib"));
        assert!(externs[1].relative_path.ends_with("libserde_derive-abc123.dylib"));
        Ok(())
    }

    #[test]
    fn publisher_excludes_unmatched_build_output_but_retains_cargo_reported_compiler_artifacts()
    -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        // The parent `build` component is deliberately unrelated to Cargo's build-script layout. The direct
        // dependency must remain admissible while both supported nested build-script `out` layouts are excluded.
        let workspace_root = staging.path().join("workspace/build");
        let serde_source = workspace_root.join("registry/serde/src/lib.rs");
        fs::create_dir_all(serde_source.parent().ok_or("serde source parent missing")?)?;
        fs::write(&serde_source, "pub fn fixture() {}\n")?;
        let dependency_directory = workspace_root.join("target/x86_64-unknown-linux-gnu/oven-test/deps");
        let build_output_directory =
            workspace_root.join("target/x86_64-unknown-linux-gnu/oven-test/build/serde-fixture/out");
        let build_output_with_identity_directory =
            workspace_root.join("target/x86_64-unknown-linux-gnu/oven-test/build/serde-fixture/abc123/out");
        let cargo_reported_output_directory =
            workspace_root.join("target/x86_64-unknown-linux-gnu/oven-test/build/serde/real123/out");
        fs::create_dir_all(&dependency_directory)?;
        fs::create_dir_all(&build_output_directory)?;
        fs::create_dir_all(&build_output_with_identity_directory)?;
        fs::create_dir_all(&cargo_reported_output_directory)?;
        let resolved_library = dependency_directory.join("libserde-resolved.rlib");
        let build_output_library = build_output_directory.join("libserde-build-output.rlib");
        let build_output_with_identity_library =
            build_output_with_identity_directory.join("libserde-build-output-with-identity.rlib");
        let cargo_reported_library = cargo_reported_output_directory.join("libserde-real123.rlib");
        let unrelated_build_output = workspace_root.join("fixtures/out/libfixture.rlib");
        assert!(!compiler_suite_cargo_build_output(&unrelated_build_output));
        assert!(compiler_suite_cargo_build_output(&build_output_library));
        assert!(compiler_suite_cargo_build_output(&build_output_with_identity_library));
        fs::write(&resolved_library, "resolved serde library")?;
        fs::write(&build_output_library, "not a Cargo dependency artifact")?;
        fs::write(
            &build_output_with_identity_library,
            "not a Cargo dependency artifact with an identity directory",
        )?;
        fs::write(&cargo_reported_library, "Cargo-reported serde compiler artifact")?;
        let artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "serde 1.0.0 (registry+https://example.invalid)",
            "target": { "name": "serde", "src_path": serde_source },
            "features": ["derive"],
            "filenames": [
                resolved_library,
                build_output_library,
                build_output_with_identity_library,
                cargo_reported_library,
            ],
            "profile": { "test": false },
        });
        let output = CargoInvocationOutput {
            stdout: format!("{artifact_message}\n").into_bytes(),
        };

        let artifact_index = compiler_suite_artifact_index(&output, "x86_64-unknown-linux-gnu")?;
        let indexed = artifact_index.values().flatten().collect::<Vec<_>>();
        let canonical_resolved_library = fs::canonicalize(&resolved_library)?;
        let canonical_cargo_reported_library = fs::canonicalize(&cargo_reported_library)?;
        let canonical_build_output_library = fs::canonicalize(&build_output_library)?;
        let canonical_build_output_with_identity_library = fs::canonicalize(&build_output_with_identity_library)?;
        assert_eq!(
            indexed,
            vec![&canonical_cargo_reported_library, &canonical_resolved_library]
        );
        let reported = compiler_suite_output_artifact_paths(&output)?;
        assert_eq!(
            reported,
            vec![canonical_cargo_reported_library.clone(), canonical_resolved_library]
        );
        let catalog = compiler_suite_artifact_catalog(&workspace_root, &[dependency_directory], &reported)?;
        assert_eq!(catalog.materialized_files.len(), 2);
        assert!(
            catalog
                .closure
                .dependency_search_paths
                .contains(&"target/x86_64-unknown-linux-gnu/oven-test/build/serde/real123/out".to_string())
        );
        assert!(
            catalog
                .materialized_files
                .iter()
                .any(|artifact| artifact.source_path == canonical_cargo_reported_library)
        );
        assert!(catalog.materialized_files.iter().all(|artifact| {
            artifact.source_path != canonical_build_output_library
                && artifact.source_path != canonical_build_output_with_identity_library
        }));
        Ok(())
    }

    #[test]
    fn publisher_converts_workspace_unit_graph_into_direct_rustc_targets() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let staging = tempfile::tempdir()?;
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
        let library_source = compiler_root.path().join("src/lib.rs");
        let cli_source = compiler_root.path().join("src/main.rs");
        let binary_source = compiler_root.path().join("src/bin/generate_fixture.rs");
        fs::write(&library_source, "pub fn fixture() {}\n")?;
        fs::write(&cli_source, "fn main() {}\n")?;
        fs::create_dir_all(binary_source.parent().ok_or("binary source parent missing")?)?;
        fs::write(&binary_source, "fn main() {}\n")?;
        let receipt = receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
            compiler_root.path(),
            "compiler-suite-fixture",
            "aarch64-apple-darwin",
            "rustc fixture",
            "debug",
            Vec::new(),
        ))?;
        let dependency_source = staging.path().join("fixture_dep.rs");
        fs::write(&dependency_source, "pub fn dependency() {}\n")?;
        let dependency_directory = staging.path().join("target/aarch64-apple-darwin/debug/deps");
        let host_dependency_directory = staging.path().join("target/debug/deps");
        let cli_library_directory = staging.path().join("target/aarch64-apple-darwin/debug");
        fs::create_dir_all(&dependency_directory)?;
        fs::create_dir_all(&host_dependency_directory)?;
        fs::create_dir_all(&cli_library_directory)?;
        let dependency_artifact = dependency_directory.join("libfixture_dep-abc123.rlib");
        let cli_dependency_artifact = cli_library_directory.join("libfixture_dep.rlib");
        let host_dependency_artifact = host_dependency_directory.join("libfixture_dep-host456.rlib");
        fs::write(&dependency_artifact, "fixture dependency artifact")?;
        fs::write(&cli_dependency_artifact, "fixture CLI dependency artifact")?;
        fs::write(&host_dependency_artifact, "fixture host dependency artifact")?;

        let dependency_unit = CargoUnitGraphUnit {
            pkg_id: "fixture_dep 0.1.0 (path+file:///fixture-dep)".to_string(),
            target: CargoUnitGraphTarget {
                kind: vec!["lib".to_string()],
                crate_types: vec!["lib".to_string()],
                name: "fixture_dep".to_string(),
                src_path: dependency_source.clone(),
                edition: "2024".to_string(),
            },
            // Cargo's unit graph may describe a dependency under its test-root mode even when Cargo's JSON message
            // correctly reports the emitted dependency library with `profile.test = false`.
            mode: "test".to_string(),
            platform: Some("aarch64-apple-darwin".to_string()),
            features: vec!["root-feature".to_string()],
            dependencies: Vec::new(),
        };
        let test_unit = CargoUnitGraphUnit {
            pkg_id: "fixture 0.1.0 (path+file:///fixture)".to_string(),
            target: CargoUnitGraphTarget {
                kind: vec!["lib".to_string()],
                crate_types: vec!["lib".to_string()],
                name: "fixture".to_string(),
                src_path: library_source.clone(),
                edition: "2024".to_string(),
            },
            mode: "test".to_string(),
            platform: Some("aarch64-apple-darwin".to_string()),
            features: vec!["default".to_string()],
            dependencies: vec![
                CargoUnitGraphDependency {
                    index: 0,
                    extern_crate_name: Some("fixture_dep".to_string()),
                },
                CargoUnitGraphDependency {
                    index: 0,
                    extern_crate_name: Some("fixture_dep".to_string()),
                },
                CargoUnitGraphDependency {
                    index: 1,
                    extern_crate_name: Some("generate_fixture".to_string()),
                },
            ],
        };
        let binary_unit = CargoUnitGraphUnit {
            pkg_id: "fixture 0.1.0 (path+file:///fixture)".to_string(),
            target: CargoUnitGraphTarget {
                kind: vec!["bin".to_string()],
                crate_types: vec!["bin".to_string()],
                name: "generate_fixture".to_string(),
                src_path: binary_source,
                edition: "2024".to_string(),
            },
            mode: "build".to_string(),
            platform: Some("aarch64-apple-darwin".to_string()),
            features: vec!["default".to_string()],
            dependencies: vec![CargoUnitGraphDependency {
                index: 0,
                extern_crate_name: Some("fixture_dep".to_string()),
            }],
        };
        let cli_unit = CargoUnitGraphUnit {
            pkg_id: "fixture 0.1.0 (path+file:///fixture)".to_string(),
            target: CargoUnitGraphTarget {
                kind: vec!["bin".to_string()],
                crate_types: vec!["bin".to_string()],
                name: "incan".to_string(),
                src_path: cli_source.clone(),
                edition: "2024".to_string(),
            },
            mode: "build".to_string(),
            platform: Some("aarch64-apple-darwin".to_string()),
            features: vec!["default".to_string()],
            dependencies: vec![CargoUnitGraphDependency {
                index: 0,
                extern_crate_name: Some("fixture_dep".to_string()),
            }],
        };
        let doctest_unit = CargoUnitGraphUnit {
            mode: "doctest".to_string(),
            ..test_unit.clone()
        };
        let test_graph = CargoUnitGraph {
            version: 1,
            units: vec![dependency_unit, binary_unit, test_unit, doctest_unit],
            roots: vec![2, 3],
        };
        let cli_graph = CargoUnitGraph {
            version: 1,
            units: vec![
                CargoUnitGraphUnit {
                    pkg_id: "fixture_dep 0.1.0 (path+file:///fixture-dep)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "fixture_dep".to_string(),
                        src_path: dependency_source.clone(),
                        edition: "2024".to_string(),
                    },
                    mode: "build".to_string(),
                    platform: Some("aarch64-apple-darwin".to_string()),
                    features: vec!["root-feature".to_string()],
                    dependencies: Vec::new(),
                },
                cli_unit,
            ],
            roots: vec![1],
        };
        let test_artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "fixture_dep 0.1.0 (path+file:///fixture-dep)",
            "target": {
                "name": "fixture_dep",
                "src_path": dependency_source,
            },
            "features": [],
            "filenames": [dependency_artifact],
            "profile": { "test": false },
        });
        let host_artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "fixture_dep 0.1.0 (path+file:///fixture-dep)",
            "target": {
                "name": "fixture_dep",
                "src_path": dependency_source,
            },
            "features": [],
            "filenames": [host_dependency_artifact],
            "profile": { "test": false },
        });
        let cli_artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "fixture_dep 0.1.0 (path+file:///fixture-dep)",
            "target": {
                "name": "fixture_dep",
                "src_path": dependency_source,
            },
            "features": [],
            "filenames": [cli_dependency_artifact],
            "profile": { "test": false },
        });
        let test_output = CargoInvocationOutput {
            stdout: format!("{test_artifact_message}\n{host_artifact_message}\n").into_bytes(),
        };
        let cli_output = CargoInvocationOutput {
            stdout: format!("{cli_artifact_message}\n").into_bytes(),
        };
        let target = staging.path().join("target");
        let isolated_selection_output = CargoInvocationOutput {
            stdout: format!("{test_artifact_message}\n{host_artifact_message}\n").into_bytes(),
        };
        let (isolated_shard, isolated_files) = compiler_suite_direct_target_shard_plan(
            compiler_root.path(),
            &receipt,
            staging.path(),
            &target,
            &test_graph,
            2,
            &isolated_selection_output,
        )?;
        assert_eq!(isolated_shard.target.key().package_name, "fixture");
        assert_eq!(isolated_shard.target.runner, "rustc-test");
        assert_eq!(isolated_shard.binary_targets.len(), 1);
        assert_eq!(isolated_shard.binary_targets[0].target_name, "generate_fixture");
        assert_eq!(isolated_shard.artifact_closure.supporting_artifacts.len(), 2);
        assert_eq!(isolated_files.len(), 2);
        let sealed_foundation_catalog = compiler_suite_artifact_catalog(
            staging.path(),
            &[dependency_directory.clone(), host_dependency_directory.clone()],
            &[],
        )?;
        let sealed_foundation_index =
            compiler_suite_artifact_index(&isolated_selection_output, &receipt.intent.target)?;
        let (sealed_foundation_shard, sealed_foundation_files) = compiler_suite_direct_target_shard_from_catalog(
            compiler_root.path(),
            &receipt,
            &test_graph,
            2,
            &sealed_foundation_index,
            &sealed_foundation_catalog,
        )?;
        assert_eq!(sealed_foundation_shard.target, isolated_shard.target);
        assert_eq!(sealed_foundation_shard.binary_targets, isolated_shard.binary_targets);
        assert_eq!(
            sealed_foundation_shard.workspace_libraries,
            isolated_shard.workspace_libraries
        );
        assert_eq!(sealed_foundation_files.len(), isolated_files.len());
        assert_eq!(
            sealed_foundation_files
                .iter()
                .map(|file| file.relative_path.as_str())
                .collect::<Vec<_>>(),
            isolated_files
                .iter()
                .map(|file| file.relative_path.as_str())
                .collect::<Vec<_>>()
        );
        let (isolated_cli, isolated_cli_workspace_libraries, isolated_cli_closure, isolated_cli_files) =
            compiler_suite_direct_cli_plan(
                compiler_root.path(),
                &receipt,
                staging.path(),
                &target,
                &cli_graph,
                std::slice::from_ref(&cli_output),
            )?;
        assert_eq!(isolated_cli.source_relative_path, "src/main.rs");
        assert_eq!(isolated_cli.runner, "rustc-run");
        assert!(isolated_cli_workspace_libraries.is_empty());
        assert_eq!(isolated_cli_closure.supporting_artifacts.len(), 3);
        assert_eq!(isolated_cli_files.len(), 3);
        let (sealed_foundation_cli, sealed_foundation_cli_libraries) = compiler_suite_cli_target_from_artifact_index(
            compiler_root.path(),
            &receipt,
            &cli_graph,
            &sealed_foundation_index,
            &sealed_foundation_catalog,
        )?;
        assert_eq!(sealed_foundation_cli.key(), isolated_cli.key());
        assert_eq!(sealed_foundation_cli.features, isolated_cli.features);
        assert_eq!(
            sealed_foundation_cli.compile_environment,
            isolated_cli.compile_environment
        );
        assert_eq!(sealed_foundation_cli.externs.len(), 1);
        assert!(
            sealed_foundation_cli.externs[0]
                .relative_path
                .ends_with("libfixture_dep-abc123.rlib")
        );
        assert_eq!(sealed_foundation_cli_libraries, isolated_cli_workspace_libraries);
        let (targets, binary_targets, cli_target, closure, materialized) = compiler_suite_direct_target_plan(
            compiler_root.path(),
            &receipt,
            staging.path(),
            &target,
            &test_graph,
            &[test_output],
            &cli_graph,
            &cli_output,
        )?;

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].binary_dependencies, ["generate_fixture"]);
        assert_eq!(targets[0].package_name, "fixture");
        assert_eq!(binary_targets.len(), 1);
        assert_eq!(binary_targets[0].target_name, "generate_fixture");
        assert_eq!(binary_targets[0].runner, "rustc-run");
        assert_eq!(targets[0].source_relative_path, "src/lib.rs");
        assert_eq!(targets[0].runner, "rustc-test");
        assert_eq!(targets[0].features, vec!["default"]);
        assert_eq!(targets[0].externs.len(), 1);
        assert_eq!(targets[0].externs[0].crate_name, "fixture_dep");
        assert!(
            targets[0].externs[0]
                .relative_path
                .ends_with("libfixture_dep-abc123.rlib")
        );
        assert_eq!(targets[1].source_relative_path, "src/lib.rs");
        assert_eq!(targets[1].runner, "rustdoc-test");
        assert_eq!(cli_target.source_relative_path, "src/main.rs");
        assert_eq!(cli_target.runner, "rustc-run");
        assert!(
            cli_target.externs[0]
                .relative_path
                .ends_with("target/aarch64-apple-darwin/debug/libfixture_dep.rlib")
        );
        assert!(
            materialized
                .iter()
                .any(|file| { file.relative_path == "target/aarch64-apple-darwin/debug/libfixture_dep.rlib" })
        );
        // Cargo can emit matching package/target/profile records for the host and selected target. The target
        // unit must resolve its target-triple artifact, while the immutable closure retains both for host-side
        // proc-macro/build dependencies that may be named by other roots.
        assert_eq!(closure.supporting_artifacts.len(), 3);
        assert_eq!(materialized.len(), 3);
        let shard = OvenCompilerTestSuiteShardPayload {
            schema_version: OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION,
            target: targets[0].clone(),
            binary_targets: binary_targets.clone(),
            workspace_libraries: Vec::new(),
            foundation_references: Vec::new(),
            artifact_closure: closure.clone(),
        };
        let restored = serde_json::from_slice::<OvenCompilerTestSuiteShardPayload>(&serde_json::to_vec(&shard)?)?;
        assert_eq!(restored.schema_version, OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION);
        assert_eq!(restored.target_key(), targets[0].key());
        assert_eq!(restored.binary_targets, binary_targets);
        Ok(())
    }

    #[test]
    fn publisher_assigns_direct_rustdoc_to_doctest_roots() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(compiler_suite_target_runner("doctest")?, "rustdoc-test");
        Ok(())
    }

    #[test]
    fn publisher_accepts_doctest_roots_for_direct_rustdoc() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let source = compiler_root.path().join("src/lib.rs");
        fs::create_dir_all(source.parent().ok_or("lib source parent missing")?)?;
        fs::write(&source, "//! a doctest\n")?;
        let graph = CargoUnitGraph {
            version: 1,
            units: vec![CargoUnitGraphUnit {
                pkg_id: "fixture 0.1.0 (path+file:///fixture)".to_string(),
                target: CargoUnitGraphTarget {
                    kind: vec!["lib".to_string()],
                    crate_types: vec!["lib".to_string()],
                    name: "fixture".to_string(),
                    src_path: source,
                    edition: "2024".to_string(),
                },
                mode: "doctest".to_string(),
                platform: None,
                features: Vec::new(),
                dependencies: Vec::new(),
            }],
            roots: vec![0],
        };

        validate_compiler_suite_unit_graph(compiler_root.path(), &graph)?;
        Ok(())
    }

    #[test]
    fn publisher_accepts_proc_macro_test_roots_for_direct_rustc() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let source = compiler_root.path().join("crates/macros/src/lib.rs");
        fs::create_dir_all(source.parent().ok_or("macro source parent missing")?)?;
        fs::write(&source, "pub fn fixture() {}\n")?;
        let graph = CargoUnitGraph {
            version: 1,
            units: vec![CargoUnitGraphUnit {
                pkg_id: "fixture-macros 0.1.0 (path+file:///fixture-macros)".to_string(),
                target: CargoUnitGraphTarget {
                    kind: vec!["proc-macro".to_string()],
                    crate_types: vec!["proc-macro".to_string()],
                    name: "fixture_macros".to_string(),
                    src_path: source,
                    edition: "2024".to_string(),
                },
                mode: "test".to_string(),
                platform: None,
                features: Vec::new(),
                dependencies: Vec::new(),
            }],
            roots: vec![0],
        };

        validate_compiler_suite_unit_graph(compiler_root.path(), &graph)?;
        Ok(())
    }

    #[test]
    fn compiler_suite_plans_exact_package_qualified_root_selections() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let root_source = compiler_root.path().join("src/lib.rs");
        let root_binary = compiler_root.path().join("src/main.rs");
        let root_integration = compiler_root.path().join("tests/smoke.rs");
        let macro_source = compiler_root.path().join("crates/macros/src/lib.rs");
        let nested_integration = compiler_root.path().join("crates/other/tests/smoke.rs");
        fs::create_dir_all(root_source.parent().ok_or("root source parent missing")?)?;
        fs::create_dir_all(root_integration.parent().ok_or("root integration parent missing")?)?;
        fs::create_dir_all(macro_source.parent().ok_or("macro source parent missing")?)?;
        fs::create_dir_all(nested_integration.parent().ok_or("nested integration parent missing")?)?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"root-package\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(
            compiler_root.path().join("crates/macros/Cargo.toml"),
            "[package]\nname = \"fixture-macros\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(
            compiler_root.path().join("crates/other/Cargo.toml"),
            "[package]\nname = \"other-package\"\nversion = \"0.1.0\"\n",
        )?;
        for source in [
            &root_source,
            &root_binary,
            &root_integration,
            &macro_source,
            &nested_integration,
        ] {
            fs::write(source, "fn fixture() {}\n")?;
        }
        let root_library = CargoUnitGraphUnit {
            pkg_id: "opaque-root-id".to_string(),
            target: CargoUnitGraphTarget {
                kind: vec!["lib".to_string()],
                crate_types: vec!["lib".to_string()],
                name: "root_package".to_string(),
                src_path: root_source.clone(),
                edition: "2024".to_string(),
            },
            mode: "test".to_string(),
            platform: None,
            features: vec!["root-feature".to_string()],
            dependencies: Vec::new(),
        };
        let graph = CargoUnitGraph {
            version: 1,
            units: vec![
                root_library.clone(),
                CargoUnitGraphUnit {
                    pkg_id: "opaque-root-id".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["bin".to_string()],
                        crate_types: vec!["bin".to_string()],
                        name: "root-cli".to_string(),
                        src_path: root_binary,
                        edition: "2024".to_string(),
                    },
                    mode: "test".to_string(),
                    platform: None,
                    features: vec!["root-feature".to_string()],
                    dependencies: Vec::new(),
                },
                CargoUnitGraphUnit {
                    pkg_id: "opaque-root-id".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["test".to_string()],
                        crate_types: Vec::new(),
                        name: "smoke".to_string(),
                        src_path: root_integration,
                        edition: "2024".to_string(),
                    },
                    mode: "test".to_string(),
                    platform: None,
                    features: vec!["root-feature".to_string()],
                    dependencies: Vec::new(),
                },
                CargoUnitGraphUnit {
                    pkg_id: "opaque-macro-id".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["proc-macro".to_string()],
                        crate_types: vec!["proc-macro".to_string()],
                        name: "fixture_macros".to_string(),
                        src_path: macro_source,
                        edition: "2024".to_string(),
                    },
                    mode: "test".to_string(),
                    platform: None,
                    features: vec!["macro-feature".to_string()],
                    dependencies: Vec::new(),
                },
                CargoUnitGraphUnit {
                    mode: "doctest".to_string(),
                    ..root_library
                },
                CargoUnitGraphUnit {
                    pkg_id: "opaque-other-id".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["test".to_string()],
                        crate_types: Vec::new(),
                        name: "smoke".to_string(),
                        src_path: nested_integration,
                        edition: "2024".to_string(),
                    },
                    mode: "test".to_string(),
                    platform: None,
                    features: vec!["other-feature".to_string()],
                    dependencies: Vec::new(),
                },
            ],
            roots: vec![0, 1, 2, 3, 4, 5],
        };

        let selections = compiler_suite_target_selections(compiler_root.path(), &graph)?;

        assert_eq!(
            selections,
            vec![
                OvenLegacyCargoInvocationTarget::WorkspacePackageLibrary("fixture-macros".to_string()),
                OvenLegacyCargoInvocationTarget::WorkspacePackageLibrary("root-package".to_string()),
                OvenLegacyCargoInvocationTarget::WorkspacePackageBinary {
                    package: "root-package".to_string(),
                    target: "root-cli".to_string(),
                },
                OvenLegacyCargoInvocationTarget::WorkspacePackageIntegrationTest {
                    package: "other-package".to_string(),
                    target: "smoke".to_string(),
                },
                OvenLegacyCargoInvocationTarget::WorkspacePackageIntegrationTest {
                    package: "root-package".to_string(),
                    target: "smoke".to_string(),
                },
                OvenLegacyCargoInvocationTarget::WorkspacePackageDoctests("root-package".to_string()),
            ]
        );
        let groups = compiler_suite_target_selection_groups(compiler_root.path(), &graph)?;
        assert_eq!(
            groups
                .iter()
                .map(|(selection, _)| selection.clone())
                .collect::<Vec<_>>(),
            selections
        );
        assert_eq!(
            groups.iter().map(|(_, indices)| indices.clone()).collect::<Vec<_>>(),
            vec![vec![3], vec![0], vec![1], vec![5], vec![2], vec![4]]
        );
        assert_eq!(
            groups
                .iter()
                .map(|(_, indices)| compiler_suite_target_selection_features(&graph, indices))
                .collect::<Result<Vec<_>, _>>()?,
            vec![
                vec!["macro-feature".to_string()],
                vec!["root-feature".to_string()],
                vec!["root-feature".to_string()],
                vec!["other-feature".to_string()],
                vec!["root-feature".to_string()],
                vec!["root-feature".to_string()],
            ]
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn publisher_invokes_one_package_qualified_integration_target() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let fixture = tempfile::tempdir()?;
        let cargo = fixture.path().join("cargo");
        let rustc = fixture.path().join("rustc");
        let arguments = fixture.path().join("cargo-arguments");
        let manifest = fixture.path().join("Cargo.toml");
        fs::write(
            &cargo,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n", arguments.display()),
        )?;
        fs::write(&rustc, "#!/bin/sh\nexit 0\n")?;
        fs::write(&manifest, "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n")?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755))?;

        let _ = run_legacy_cargo_invocation(
            &cargo,
            &rustc,
            &manifest,
            &fixture.path().join("target"),
            fixture.path(),
            "aarch64-apple-darwin",
            "debug",
            &[],
            1024 * 1024,
            "test",
            &OvenLegacyCargoInvocationTarget::WorkspacePackageIntegrationTest {
                package: "fixture-package".to_string(),
                target: "smoke".to_string(),
            },
            false,
            false,
            false,
        )?;
        let arguments = fs::read_to_string(arguments)?;
        assert!(arguments.lines().any(|argument| argument == "--package"));
        assert!(arguments.lines().any(|argument| argument == "fixture-package"));
        assert!(arguments.lines().any(|argument| argument == "--test"));
        assert!(arguments.lines().any(|argument| argument == "smoke"));
        assert!(!arguments.lines().any(|argument| argument == "--workspace"));
        Ok(())
    }
}
