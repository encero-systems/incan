//! Compiler-owned Loafs for the bounded Oven Alpha envelope.
//!
//! A loaf is an immutable direct-`rustc` closure shipped with the active Incan toolchain. It is deliberately
//! separate from a generated-project receipt: one loaf can satisfy compatible clean worktrees, while each generated
//! source tree keeps its own receipt and final output. Normal commands select a verified compiler Loaf directly, or
//! a receipt-bound project Loaf from the bounded Oven store; neither path inspects a Cargo target or accepts a
//! project-selected native-artifact directory.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::interop::{OVEN_INTEROP_EXECUTION_RECEIPT_INPUT, OVEN_INTEROP_PLAN_SCHEMA_INPUT};
use super::legacy_cargo::{
    OvenLegacyCargoDirectDependencyClosure, OvenLegacyCargoError, OvenLegacyCargoInspectionPackage,
    OvenLegacyCargoInspectionSource, OvenLegacyCargoPrepareRequest, OvenLegacyCargoPublicationKind,
    canonicalize_supporting_artifacts, copy_regular_directory_tree, direct_rustc_compile_environment,
    materialized_files_from_directory, prepare_direct_rustc_plan,
};
use super::process::{isolate_process_group, terminate_process_group};
use super::rustc::{
    OvenRegistryLeafAuthority, OvenRustcArtifactExtern, OvenRustcArtifactManifest, OvenRustcArtifactPlan,
    OvenRustcAuxiliaryTarget, OvenRustcError, OvenRustcRegistryLeaf, OvenRustcRegistrySource,
    OvenRustcRegistrySourcePackage, OvenRustcSupportingArtifact, clear_inherited_cargo_environment,
    validate_sealed_registry_leaf,
};
use super::store::{OvenArtifactKind, OvenStore, OvenStoreError};
use super::{OvenReceipt, digest_bytes};
use crate::manifest::{DependencySource, DependencySpec, ProjectManifest};
use crate::version::{INCAN_VERSION, SDK_PROVIDER_CODEGEN_REVISION};

/// Current wire format for one compiler-shipped Oven Loaf.
pub const OVEN_LOAF_SCHEMA_VERSION: u32 = 13;
/// Current wire format for the atomically committed Loaf-envelope manifest.
pub const OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION: u32 = 3;
/// Internal marker enabled only while the named legacy publisher creates a compiler-owned Loaf.
///
/// This is deliberately distinct from normal Oven command selection: it grants compiler source emission the same
/// trusted standard-provider identity as the SDK publisher, but it never authorizes Cargo for a caller command.
pub(crate) const OVEN_LOAF_ENV: &str = "INCAN_OVEN_LOAF";
/// Actionable user guidance for a normal-command miss without turning it into a compatibility-baker fallback.
pub const OVEN_LOAF_MISS_GUIDANCE: &str = "Action: run `incan oven bake --project <project-root>`. That explicit command records generated-project receipts for every conventional project target, reuses a compatible closure when present, or performs one bounded compatibility bake. Normal build, run, and test remain Cargo-free and will not invoke the baker automatically.";
const TOOLCHAIN_LOAF_RELATIVE_ROOT: &str = "share/incan/oven/loafs";
static LOAF_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const OVEN_LOAF_ENVELOPE_LOCK_FILE: &str = ".envelope.lock";

/// Built-in compiler-owned Loaf set prepared by the explicit baker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OvenLoafEnvelope {
    /// Coherent compiled closures with source authority shipped in a release toolchain.
    Release,
    /// The same complete standard-provider closure for compiler-suite debug and release execution.
    CompilerSuite,
}

/// Normal compiler action used only to derive a receipt and generated project for a checked Loaf fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OvenLoafFixtureAction {
    /// Compile the fixture with normal `incan build` semantics.
    Build,
    /// Compile the fixture with normal `incan run` semantics.
    Run,
}

/// The independent authority a typed Loaf member contributes to one release-version envelope.
///
/// Compiled closures remain feature-unified direct-`rustc` inputs. Source-authority members carry the locked registry
/// source trees needed during Rust inspection, so those sources are shared without turning unrelated rlibs into one
/// interchangeable catalog. A checked fixture may deliberately contribute both authorities when its one coherent
/// closure genuinely owns them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OvenLoafMemberRole {
    /// A coherent direct-`rustc` closure that can be materialized for normal execution.
    CompiledClosure,
    /// A source-inspection authority selected independently from a linkable closure.
    SourceAuthority,
    /// One coherent closure that is intentionally both linkable and source-authoritative.
    CompiledClosureAndSourceAuthority,
}

impl OvenLoafMemberRole {
    /// Return whether normal direct-`rustc` execution may select this member.
    pub(crate) const fn provides_compiled_closure(self) -> bool {
        matches!(self, Self::CompiledClosure | Self::CompiledClosureAndSourceAuthority)
    }

    /// Return whether Rust inspection may select this member's sealed source catalog.
    pub(crate) const fn provides_source_authority(self) -> bool {
        matches!(self, Self::SourceAuthority | Self::CompiledClosureAndSourceAuthority)
    }
}

/// One checked Incan fixture in a built-in Loaf envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OvenLoafSpecification {
    /// Stable human-readable family label used in progress and evidence.
    pub label: &'static str,
    /// Project name that determines the generated-project directory.
    pub project_name: &'static str,
    /// Debug or release profile selected for this Loaf.
    pub profile: &'static str,
    /// Normal compiler action used to derive the authorizing receipt.
    pub action: OvenLoafFixtureAction,
    /// Checked Incan source embedded in the compiler binary.
    pub source: &'static str,
    /// Checked Incan project manifest embedded in the compiler binary.
    pub manifest: &'static str,
    /// Checked registry-source inventory embedded separately from the generated fixture manifest.
    ///
    /// Source-only members use this to seal every supported stdlib package without compiling those packages again.
    pub inspection_manifest: &'static str,
    /// The independent authority this immutable member contributes to the envelope.
    pub role: OvenLoafMemberRole,
    /// Whether this linkable closure keeps every registry leaf emitted by its checked fixture.
    pub retain_complete_registry_leaves: bool,
    /// Whether this compiler-owned Loaf keeps every checked fixture dependency as a direct Rustc extern.
    ///
    /// A `stdlib` Loaf is a complete standard-library closure, not a scenario-shaped subset. Compiler-suite Loafs
    /// additionally seal vocabulary support in a target-specific auxiliary closure, so compiler-only roots never
    /// become a second direct-Rustc authority for ordinary generated programs.
    pub retain_checked_direct_dependencies: bool,
}

impl OvenLoafSpecification {
    /// Return the exact registry packages whose Rust source this checked fixture may inspect.
    pub fn inspection_packages(&self) -> Result<Vec<OvenLegacyCargoInspectionPackage>, String> {
        let path = Path::new("src/oven/fixtures").join(format!("{}.toml", self.project_name));
        inspection_packages_from_manifest(self.inspection_manifest, &path, self.label)
    }
}

/// Parse one checked manifest into the registry selectors resolved only by the explicit Loaf baker.
fn inspection_packages_from_manifest(
    contents: &str,
    path: &Path,
    label: &str,
) -> Result<Vec<OvenLegacyCargoInspectionPackage>, String> {
    let manifest = ProjectManifest::from_str(contents, path)
        .map_err(|error| format!("invalid checked Rust source manifest for `{label}`: {error}"))?;
    let mut packages = manifest
        .rust_dependencies()
        .values()
        .filter(|dependency| matches!(dependency.source, DependencySource::Registry))
        .map(|dependency| {
            let version_requirement = dependency.version.clone().ok_or_else(|| {
                format!(
                    "checked Loaf manifest `{}` omits a registry version for `{}`",
                    label, dependency.crate_name
                )
            })?;
            Ok(OvenLegacyCargoInspectionPackage {
                package: dependency
                    .package
                    .clone()
                    .unwrap_or_else(|| dependency.crate_name.clone()),
                version_requirement,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    packages.sort();
    packages.dedup();
    Ok(packages)
}

/// One atomically committed generation of a typed Loaf envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenLoafEnvelopeManifest {
    /// Manifest wire-schema version.
    pub schema_version: u32,
    /// Built-in envelope name (`release` or `compiler-suite`).
    pub envelope: String,
    /// Content identity of the complete generation and its release-family compatibility evidence.
    pub generation_identity: String,
    /// Canonical release-family compatibility evidence, excluding per-executable baker provenance.
    pub evidence: BTreeMap<String, String>,
    /// Complete typed member list for this generation.
    pub loafs: Vec<OvenLoafEnvelopeMember>,
}

/// One Loaf referenced by a committed envelope generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenLoafEnvelopeMember {
    /// Stable member label from the typed envelope definition.
    pub label: String,
    /// Debug or release profile.
    pub profile: String,
    /// Checked fixture action used to derive its receipt.
    pub action: String,
    /// The authority this member contributes to the envelope.
    pub role: OvenLoafMemberRole,
    /// Receipt compatibility identity stored inside the Loaf.
    pub build_unit_identity: String,
    /// Digest of the canonical Loaf metadata, including every declared artifact digest.
    pub loaf_identity: String,
    /// Digest of the sealed direct-Rustc plan encoded by this Loaf manifest.
    pub plan_identity: String,
    /// Logical bytes measured by the publishing generation.
    pub logical_bytes: u64,
    /// Physical allocation measured by the publishing generation.
    pub physical_bytes: u64,
    /// Relative `generations/<identity>/<identity>.loaf/loaf.json` path.
    pub path: PathBuf,
}

const COMPILER_SUITE_LOAFS: [OvenLoafSpecification; 2] = [
    OvenLoafSpecification {
        label: "stdlib",
        project_name: "oven_compiler_suite_foundation",
        profile: "debug",
        action: OvenLoafFixtureAction::Run,
        source: include_str!("fixtures/compiler_suite_foundation.incn"),
        manifest: include_str!("fixtures/compiler_suite_foundation.toml"),
        inspection_manifest: include_str!("fixtures/compiler_suite_foundation.toml"),
        role: OvenLoafMemberRole::CompiledClosureAndSourceAuthority,
        retain_complete_registry_leaves: true,
        retain_checked_direct_dependencies: true,
    },
    OvenLoafSpecification {
        label: "stdlib",
        project_name: "oven_compiler_suite_foundation",
        profile: "release",
        action: OvenLoafFixtureAction::Run,
        source: include_str!("fixtures/compiler_suite_foundation.incn"),
        manifest: include_str!("fixtures/compiler_suite_foundation.toml"),
        inspection_manifest: include_str!("fixtures/compiler_suite_foundation.toml"),
        role: OvenLoafMemberRole::CompiledClosureAndSourceAuthority,
        retain_complete_registry_leaves: true,
        retain_checked_direct_dependencies: true,
    },
];

const RELEASE_LOAFS: [OvenLoafSpecification; 2] = [
    OvenLoafSpecification {
        label: "stdlib",
        project_name: "oven_release_stdlib",
        profile: "debug",
        action: OvenLoafFixtureAction::Run,
        source: include_str!("fixtures/release_stdlib.incn"),
        manifest: include_str!("fixtures/release_stdlib.toml"),
        inspection_manifest: include_str!("fixtures/release_stdlib.toml"),
        role: OvenLoafMemberRole::CompiledClosureAndSourceAuthority,
        retain_complete_registry_leaves: true,
        retain_checked_direct_dependencies: true,
    },
    OvenLoafSpecification {
        label: "stdlib",
        project_name: "oven_release_stdlib",
        profile: "release",
        action: OvenLoafFixtureAction::Build,
        source: include_str!("fixtures/release_stdlib.incn"),
        manifest: include_str!("fixtures/release_stdlib.toml"),
        inspection_manifest: include_str!("fixtures/release_stdlib.toml"),
        role: OvenLoafMemberRole::CompiledClosureAndSourceAuthority,
        retain_complete_registry_leaves: true,
        retain_checked_direct_dependencies: true,
    },
];

/// Return the complete checked specification for one built-in Loaf envelope.
#[must_use]
pub fn loaf_envelope_specifications(envelope: OvenLoafEnvelope) -> &'static [OvenLoafSpecification] {
    match envelope {
        OvenLoafEnvelope::Release => &RELEASE_LOAFS,
        OvenLoafEnvelope::CompilerSuite => &COMPILER_SUITE_LOAFS,
    }
}

/// Return the registry-source selectors declared by one built-in envelope's checked fixtures.
///
/// The compiler-suite baker additionally seals its complete locked compiler graph. That graph comes from the
/// canonical compiler manifest, features, and lock rather than a second hand-maintained dependency list.
pub fn loaf_envelope_inspection_packages(
    envelope: OvenLoafEnvelope,
) -> Result<Vec<OvenLegacyCargoInspectionPackage>, String> {
    let mut packages = loaf_envelope_specifications(envelope)
        .iter()
        .filter(|specification| specification.role.provides_source_authority())
        .map(OvenLoafSpecification::inspection_packages)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    packages.sort();
    packages.dedup();
    Ok(packages)
}

/// Owner-scoped staging directory that is removed unless a verified loaf is atomically published from it.
pub(crate) struct LoafTemporaryDirectory {
    path: PathBuf,
    keep: bool,
}

impl LoafTemporaryDirectory {
    /// Create a unique owner-scoped Loaf staging directory below `parent`.
    pub(crate) fn create(parent: &Path, prefix: &str) -> io::Result<Self> {
        for _ in 0..128 {
            let sequence = LOAF_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("{prefix}{}-{sequence}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path, keep: false }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "could not allocate unique Oven Loaf staging directory below {}",
                parent.display()
            ),
        ))
    }

    /// Return the staging directory path while this owner retains cleanup responsibility.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Retain the staging directory after its caller has atomically published it.
    pub(crate) fn persist(mut self) -> PathBuf {
        self.keep = true;
        self.path.clone()
    }
}

impl Drop for LoafTemporaryDirectory {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Move every obsolete generation out of the authoritative envelope tree before scratch reclamation.
pub(crate) fn retire_unreferenced_loaf_generations(
    output: &Path,
    generation_identity: &str,
    scratch: &Path,
) -> Result<(), OvenLoafError> {
    let generations_root = output.join("generations");
    if !generations_root.is_dir() {
        return Ok(());
    }
    let active_name = generation_identity
        .strip_prefix("sha256:")
        .unwrap_or(generation_identity);
    let active_generation = generations_root.join(active_name);
    let retired_root = scratch.join("retired");
    fs::create_dir_all(&retired_root).map_err(|source| OvenLoafError::Io {
        path: retired_root.clone(),
        source,
    })?;
    for entry in fs::read_dir(&generations_root).map_err(|source| OvenLoafError::Io {
        path: generations_root.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| OvenLoafError::Io {
            path: generations_root.clone(),
            source,
        })?;
        let path = entry.path();
        if path != active_generation {
            let destination = retired_root.join(entry.file_name());
            fs::rename(&path, &destination).map_err(|source| OvenLoafError::Io { path, source })?;
        }
    }
    Ok(())
}

/// Durably publish one staged generation before atomically switching the envelope authority.
pub(crate) fn commit_loaf_generation(
    output: &Path,
    generations_root: &Path,
    generation_output: &Path,
    staged_root: &Path,
    manifest: &OvenLoafEnvelopeManifest,
    scratch: &Path,
    before_manifest_commit: impl FnOnce() -> io::Result<()>,
) -> Result<(), OvenLoafError> {
    super::store::sync_directory_tree(staged_root)?;
    if generation_output.exists() {
        let abandoned = scratch.join("abandoned-generation");
        fs::rename(generation_output, &abandoned).map_err(|source| OvenLoafError::Io {
            path: generation_output.to_path_buf(),
            source,
        })?;
    }
    fs::rename(staged_root, generation_output).map_err(|source| OvenLoafError::Io {
        path: generation_output.to_path_buf(),
        source,
    })?;
    super::store::sync_directory(generations_root.to_path_buf())?;
    let staged_manifest = scratch.join("envelope.json");
    let payload = serde_json::to_vec_pretty(manifest).map_err(|error| OvenLoafError::Preparation {
        message: format!("could not encode Loaf envelope manifest: {error}"),
    })?;
    fs::write(&staged_manifest, payload).map_err(|source| OvenLoafError::Io {
        path: staged_manifest.clone(),
        source,
    })?;
    File::open(&staged_manifest)
        .and_then(|file| file.sync_all())
        .map_err(|source| OvenLoafError::Io {
            path: staged_manifest.clone(),
            source,
        })?;
    before_manifest_commit().map_err(|source| OvenLoafError::Io {
        path: staged_manifest.clone(),
        source,
    })?;
    let manifest_path = output.join("envelope.json");
    fs::rename(&staged_manifest, &manifest_path).map_err(|source| OvenLoafError::Io {
        path: manifest_path,
        source,
    })?;
    super::store::sync_directory(output.to_path_buf())?;
    Ok(())
}

/// Immutable direct-`rustc` closure shipped with one compiler/toolchain distribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenLoaf {
    /// Loaf wire-schema version.
    pub schema_version: u32,
    /// Source-independent identity of the provider/dependency unit this loaf can materialize.
    pub build_unit_identity: String,
    /// Stable provenance for the explicit baker transition that produced this Loaf.
    pub provenance: OvenLoafProvenance,
    /// Payload accounting captured before the self-describing manifest is written.
    pub accounting: OvenLoafAccounting,
    /// Explicit compiler-owned runtime capabilities that may authorize a narrower standard-provider request.
    ///
    /// This is deliberately more restrictive than a generic dependency solver: every runtime input other than
    /// provider selection stays exact. The provider-subset policy is callable only after normal-command routing has
    /// rejected every caller-owned external `rust::` import. Rust's own `rust::std` is compiler-supplied, while a
    /// selected standard provider may contribute its checked transitive Rust closure; only standard-provider modules
    /// and facets may otherwise be subsets of this loaf.
    #[serde(default)]
    pub compatibility: OvenLoafCompatibility,
    /// Exact registry package artifacts emitted by the named Loaf publisher.
    ///
    /// Normal consumers may select only these records; this is deliberately not a Cargo cache, package index, or
    /// source resolver.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub registry_leaves: Vec<OvenRustcRegistryLeaf>,
    /// Direct-`rustc` compiler input closure relative to the loaf file's parent directory.
    pub plan: OvenRustcArtifactManifest,
}

/// Portable provenance carried by one Alpha Loaf.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenLoafProvenance {
    /// Incan compiler release that authored the Loaf contract.
    pub compiler_version: String,
    /// Exact Rust toolchain selected by the authorizing receipt.
    pub rust_toolchain: String,
    /// Compiler-owned SDK provider code-generation contract revision.
    pub sdk_provider_codegen_revision: String,
    /// Explicit baker boundary; normal commands never use this as a backend selector.
    pub baker: String,
}

/// Filesystem accounting for the immutable payload beside `loaf.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenLoafAccounting {
    /// Logical bytes in the copied direct-`rustc` payload before manifest bytes are added.
    pub payload_logical_bytes: u64,
    /// Host filesystem allocation for that payload at bake time.
    pub payload_physical_bytes: u64,
}

/// A receipt-authorized complete standard-library closure resolved from immutable toolchain data.
///
/// The release ships this Loaf once per target/profile. Both ordinary consumers and compiler-suite children retain
/// its generation lock while executing direct `rustc`; neither path copies the same full stdlib closure into every
/// project store. Project-specific compatibility closures remain separately bounded store Loafs.
#[derive(Debug)]
pub struct OvenToolchainLoaf {
    /// Stable identity of the compiler-shipped loaf selected for this receipt.
    pub loaf_build_unit_identity: String,
    /// Receipt-compatible direct-Rustc manifest retained by the loaf.
    pub artifacts: OvenRustcArtifactManifest,
    /// Exact registry leaves sealed with this Loaf.
    pub registry_leaves: Vec<OvenRustcRegistryLeaf>,
    /// Immutable compiler-data directory containing the manifest's declared files.
    pub artifact_root: PathBuf,
    /// Trusted direct-Rustc invocation inputs resolved from that immutable compiler data.
    pub artifact_plan: OvenRustcArtifactPlan,
    /// Shared publication boundary retained while direct consumers use this generation.
    _generation_lock: Option<OvenLoafGenerationLock>,
}

#[derive(Debug)]
pub(crate) struct OvenLoafGenerationLock {
    file: File,
}

impl Drop for OvenLoafGenerationLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Acquire exclusive authority to validate, switch, and retire generations below one Loaf envelope root.
pub(crate) fn acquire_exclusive_loaf_generation_lock(root: &Path) -> Result<OvenLoafGenerationLock, OvenLoafError> {
    fs::create_dir_all(root).map_err(|source| OvenLoafError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let path = root.join(OVEN_LOAF_ENVELOPE_LOCK_FILE);
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|source| OvenLoafError::Io {
            path: path.clone(),
            source,
        })?;
    file.lock().map_err(|source| OvenLoafError::Io { path, source })?;
    Ok(OvenLoafGenerationLock { file })
}

impl OvenToolchainLoaf {
    #[must_use]
    /// Expose this unit's registry leaves with only its verified transitive metadata directories.
    pub(crate) fn registry_leaf_authority(&self) -> OvenRegistryLeafAuthority {
        OvenRegistryLeafAuthority::new_with_trusted_dependency_search_paths(
            self.artifact_root.clone(),
            self.registry_leaves.clone(),
            self.artifact_plan.dependency_search_paths.clone(),
        )
    }
}

/// Explicit runtime capability envelope for a compiler-owned Loaf.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenLoafCompatibility {
    /// Every receipt build-unit input other than provider selection and its derived feature set.
    ///
    /// Values such as the runtime source digests and lockfile must compare exactly before a loaf can satisfy another
    /// receipt. The resolved Rust-dependency digest remains in the receipt identity, but is intentionally excluded
    /// here: this policy runs only after caller-owned external Rust imports are refused. The standard-library feature
    /// digest is likewise represented by the selected provider modules and facets, which permits a verified provider
    /// superset to serve a narrower compiler-owned request without becoming a dependency resolver.
    pub runtime_inputs: BTreeMap<String, String>,
    /// Standard-provider modules, implementation facets, and direct rlib links compiled into the shipped closure.
    pub providers: Vec<OvenLoafProviderCapability>,
}

/// One provider capability compiled into a Loaf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenLoafProviderCapability {
    /// Stable provider identity supplied by the checked compiler provider plan.
    pub identity: String,
    /// Canonical standard-provider module paths covered by this closure.
    pub modules: Vec<String>,
    /// Exact implementation facets selected while the closure was published.
    pub facets: Vec<String>,
    /// Whether this provider's rlib is a required direct Rust link root even without a source-module import.
    #[serde(default)]
    pub direct_link: bool,
}

/// Authorization policy for compiler-shipped Loaf selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OvenLoafSelection {
    /// Require the loaf's complete build-unit identity to equal the caller receipt.
    Exact,
    /// Permit a compiler-owned standard-provider closure to satisfy a narrower request.
    ///
    /// Callers use this only after rejecting inline `rust::` imports. The relation still requires exact runtime
    /// inputs and loaf-superset provider modules/facets; it is not a dependency resolver or a Cargo fallback.
    CompilerOwnedProviderSuperset,
}

/// How much compiler-owned provider capability a loaf contributes beyond one requested receipt.
///
/// Multiple immutable loafs can safely authorize the same request. Prefer the narrowest one so adding a new
/// provider-family loaf does not make an otherwise valid core request ambiguous. The values are derived only after
/// the exact runtime-input check, so this is a deterministic efficiency choice, never dependency resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct OvenLoafProviderExcess {
    providers: usize,
    modules: usize,
    facets: usize,
    direct_links: usize,
}

/// A Loaf that has passed the narrow compiler-owned provider-subset authorization rule.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CompatibleLoaf {
    path: PathBuf,
    excess: OvenLoafProviderExcess,
}

impl OvenLoafCompatibility {
    /// Derive the explicit, portable compatibility envelope from one verified generated-project receipt.
    fn from_receipt(receipt: &OvenReceipt) -> Result<Self, OvenLoafError> {
        let mut runtime_inputs = receipt.sources.build_unit_inputs.clone();
        let provider_records = runtime_inputs.remove("providers").unwrap_or_default();
        let _ = runtime_inputs.remove("rust-dependencies");
        let _ = runtime_inputs.remove("stdlib-features");
        // The selected interop receipt proves package-owned archives and headers, not a compiler-owned runtime
        // capability. Its immutable final plan is independently reconstructed and verified before execution; using
        // it as a Loaf compatibility key would require one shipped Loaf per consumer package.
        let _ = runtime_inputs.remove(OVEN_INTEROP_EXECUTION_RECEIPT_INPUT);
        let _ = runtime_inputs.remove(OVEN_INTEROP_PLAN_SCHEMA_INPUT);
        let provider_plan = runtime_inputs
            .remove("provider-plan")
            .ok_or_else(|| OvenLoafError::Preparation {
                message: "Loaf receipt is missing its provider-plan input".to_string(),
            })?;
        let expected_provider_plan = digest_bytes(provider_records.as_bytes());
        if provider_plan != expected_provider_plan {
            return Err(OvenLoafError::Preparation {
                message: "Loaf receipt provider-plan digest does not match its provider records".to_string(),
            });
        }
        let providers = parse_provider_capabilities(&provider_records)?;
        Ok(Self {
            runtime_inputs,
            providers,
        })
    }

    /// Return the extra capability retained by this compatible loaf, or `None` when it cannot serve `receipt`.
    fn provider_subset_excess(&self, receipt: &OvenReceipt) -> Result<Option<OvenLoafProviderExcess>, OvenLoafError> {
        let requested = Self::from_receipt(receipt)?;
        if self.runtime_inputs != requested.runtime_inputs {
            return Ok(None);
        }
        let mut available = BTreeMap::new();
        for provider in &self.providers {
            if available.insert(provider.identity.as_str(), provider).is_some() {
                return Err(OvenLoafError::InvalidLoaf {
                    path: PathBuf::from("loaf compatibility"),
                    message: format!("declares provider `{}` more than once", provider.identity),
                });
            }
        }
        let requested_by_identity = requested
            .providers
            .iter()
            .map(|provider| (provider.identity.as_str(), provider))
            .collect::<BTreeMap<_, _>>();
        for required in &requested.providers {
            let Some(candidate) = available.get(required.identity.as_str()) else {
                return Ok(None);
            };
            if !required
                .modules
                .iter()
                .all(|module| candidate.modules.binary_search(module).is_ok())
                || !required
                    .facets
                    .iter()
                    .all(|facet| candidate.facets.binary_search(facet).is_ok())
                || (required.direct_link && !candidate.direct_link)
            {
                return Ok(None);
            }
        }
        let mut excess = OvenLoafProviderExcess {
            providers: 0,
            modules: 0,
            facets: 0,
            direct_links: 0,
        };
        for candidate in &self.providers {
            let Some(required) = requested_by_identity.get(candidate.identity.as_str()) else {
                excess.providers += 1;
                excess.modules += candidate.modules.len();
                excess.facets += candidate.facets.len();
                excess.direct_links += usize::from(candidate.direct_link);
                continue;
            };
            excess.modules += candidate
                .modules
                .iter()
                .filter(|module| required.modules.binary_search(module).is_err())
                .count();
            excess.facets += candidate
                .facets
                .iter()
                .filter(|facet| required.facets.binary_search(facet).is_err())
                .count();
            excess.direct_links += usize::from(candidate.direct_link && !required.direct_link);
        }
        Ok(Some(excess))
    }

    /// Return whether this shipped runtime closure can safely satisfy `receipt` under the narrow provider-subset rule.
    fn authorizes_provider_subset(&self, receipt: &OvenReceipt) -> Result<bool, OvenLoafError> {
        Ok(self.provider_subset_excess(receipt)?.is_some())
    }

    /// Return whether this independent source authority may inspect Rust metadata for `receipt`.
    ///
    /// Source inspection never authorizes a linkable direct-`rustc` closure, so provider modules and facets do not
    /// participate in this decision. Compiler/runtime source evidence, target, profile, toolchain, and every package
    /// feature still remain checked by the caller, the selected source catalog, and the plan intent.
    fn authorizes_source_authority(&self, receipt: &OvenReceipt) -> Result<bool, OvenLoafError> {
        Ok(self.runtime_inputs == Self::from_receipt(receipt)?.runtime_inputs)
    }
}

/// Parse the canonical provider-capability records sealed into a Loaf receipt.
fn parse_provider_capabilities(records: &str) -> Result<Vec<OvenLoafProviderCapability>, OvenLoafError> {
    let mut providers = Vec::new();
    for record in records.lines().filter(|record| !record.is_empty()) {
        let mut parts = record.split('|');
        let identity = parts.next().unwrap_or_default().trim();
        let modules = parts.next().unwrap_or_default();
        let facets = parts.next().unwrap_or_default();
        let direct_link = match parts.next() {
            None => false,
            Some("none") => false,
            Some("link") => true,
            Some(_) => {
                return Err(OvenLoafError::Preparation {
                    message: format!("Loaf provider record is not canonical: {record}"),
                });
            }
        };
        if identity.is_empty() || parts.next().is_some() {
            return Err(OvenLoafError::Preparation {
                message: format!("Loaf provider record is not canonical: {record}"),
            });
        }
        let mut modules = modules
            .split(',')
            .filter(|module| !module.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut facets = facets
            .split(',')
            .filter(|facet| !facet.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        modules.sort();
        modules.dedup();
        facets.sort();
        facets.dedup();
        providers.push(OvenLoafProviderCapability {
            identity: identity.to_string(),
            modules,
            facets,
            direct_link,
        });
    }
    providers.sort_by(|left, right| left.identity.cmp(&right.identity));
    if providers.windows(2).any(|pair| pair[0].identity == pair[1].identity) {
        return Err(OvenLoafError::Preparation {
            message: "Loaf provider records repeat one provider identity".to_string(),
        });
    }
    Ok(providers)
}

/// Result from a release-stage base-runtime loaf preparation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenLoafPreparation {
    /// Reusable native compatibility identity represented by the loaf.
    pub build_unit_identity: String,
    /// Content identity of the canonical Loaf metadata and its declared artifact digests.
    pub loaf_identity: String,
    /// Content identity of the final sealed direct-rustc plan stored in the Loaf.
    pub plan_identity: String,
    /// Logical bytes in the final compiler-shipped loaf directory, including its verified plan.
    pub logical_bytes: u64,
    /// Measured allocation in the final compiler-shipped loaf directory.
    pub physical_bytes: u64,
    /// Highest observed physical allocation in baker-owned transient state.
    pub transient_peak_physical_bytes: u64,
}

/// Explicit resources and bounded policy available to one hidden legacy-Cargo Loaf bake.
pub struct OvenLoafBakerContext<'a> {
    pub compiler_support_target: &'a Path,
    /// Every baker-owned persistent or transient root charged to the replacement high-water mark.
    pub capacity_roots: [&'a Path; 2],
    pub transient_limit: u64,
    pub cargo: &'a Path,
    pub rustc: &'a Path,
    /// Checked Rust dependency surface whose source is sealed into this one Loaf.
    pub inspection_packages: &'a [OvenLegacyCargoInspectionPackage],
    /// Locked source authority resolved once from the compiler root at the explicit baker boundary.
    ///
    /// Compiler-suite foundation Loafs retain this independently from their linkable generated-project leaves.
    pub inspection_sources: &'a [OvenLegacyCargoInspectionSource],
    /// Whether this broad foundation Loaf exposes every registry rlib actually emitted into its coherent closure.
    pub retain_complete_registry_leaves: bool,
    /// Whether the complete checked fixture dependency surface is direct-linkable by generated standard-library code.
    pub retain_checked_direct_dependencies: bool,
    pub limits: super::store::OvenStoreLimits,
}

/// Construct the portable runtime portion of a normal generated project's native build-unit identity.
///
/// The caller contributes normalized provider records, selected stdlib features, and the digest of resolved Rust
/// dependencies. Compiler-owned sources and the lockfile are resolved from the active toolchain layout so a packaged
/// compiler never depends on the checkout from which its binary happened to be built.
pub fn runtime_build_unit_inputs(
    provider_records: Vec<String>,
    stdlib_features: &[String],
    rust_dependencies_digest: String,
) -> Result<BTreeMap<String, String>, String> {
    let mut inputs = BTreeMap::new();
    inputs.insert("compiler-version".to_string(), INCAN_VERSION.to_string());
    inputs.insert(
        "sdk-provider-codegen-revision".to_string(),
        SDK_PROVIDER_CODEGEN_REVISION.to_string(),
    );
    for (name, crate_name) in [
        ("runtime-source-incan-core", "incan_core"),
        ("runtime-source-incan-derive", "incan_derive"),
        ("runtime-source-incan-stdlib", "incan_stdlib"),
    ] {
        let path = crate::toolchain_layout::resolve_toolchain_crate_path(crate_name);
        let digest = digest_runtime_crate_source(&path)?;
        inputs.insert(name.to_string(), digest);
    }
    let lock_path = crate::toolchain_layout::resolve_toolchain_runtime_lockfile();
    let lock = fs::read(&lock_path)
        .map_err(|error| format!("failed to read Oven runtime lock {}: {error}", lock_path.display()))?;
    inputs.insert("runtime-lock".to_string(), digest_bytes(&lock));
    inputs.insert(
        "provider-plan".to_string(),
        digest_bytes(provider_records.join("\n").as_bytes()),
    );
    if !provider_records.is_empty() {
        inputs.insert("providers".to_string(), provider_records.join("\n"));
    }
    inputs.insert(
        "stdlib-features".to_string(),
        digest_bytes(stdlib_features.join(",").as_bytes()),
    );
    inputs.insert("rust-dependencies".to_string(), rust_dependencies_digest);
    Ok(inputs)
}

/// Digest exactly the compiler runtime source closure retained by a suite publisher.
///
/// Runtime compatibility is determined by the package manifest and Rust sources that a generated provider can
/// compile against. Test fixtures, documentation, and nested build output are not runtime inputs and the suite
/// publisher deliberately does not retain them. Hashing the whole checkout crate here would make a native loaf
/// incompatible with the publisher's smaller immutable closure even when the compiled runtime is identical.
pub(crate) fn digest_runtime_crate_source(root: &Path) -> Result<String, String> {
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("failed to read runtime crate root {}: {error}", root.display()))?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err(format!(
            "runtime crate root {} must be a directory without symlink indirection",
            root.display()
        ));
    }
    let manifest = root.join("Cargo.toml");
    let manifest_metadata = fs::symlink_metadata(&manifest)
        .map_err(|error| format!("failed to read runtime manifest {}: {error}", manifest.display()))?;
    if !manifest_metadata.is_file() || manifest_metadata.file_type().is_symlink() {
        return Err(format!(
            "runtime manifest {} must be a regular file without symlink indirection",
            manifest.display()
        ));
    }
    let source_root = root.join("src");
    let source_metadata = fs::symlink_metadata(&source_root).map_err(|error| {
        format!(
            "failed to read runtime source directory {}: {error}",
            source_root.display()
        )
    })?;
    if !source_metadata.is_dir() || source_metadata.file_type().is_symlink() {
        return Err(format!(
            "runtime source directory {} must be a directory without symlink indirection",
            source_root.display()
        ));
    }

    let mut records = BTreeMap::new();
    records.insert(
        "Cargo.toml".to_string(),
        digest_bytes(
            &fs::read(&manifest)
                .map_err(|error| format!("failed to read runtime manifest {}: {error}", manifest.display()))?,
        ),
    );
    collect_runtime_source_records(&source_root, &source_root, &mut records)?;
    serde_json::to_vec(&records)
        .map(|payload| digest_bytes(&payload))
        .map_err(|error| {
            format!(
                "failed to serialize runtime source digest for {}: {error}",
                root.display()
            )
        })
}

/// Add the regular files below one runtime crate's `src/` tree to its portable source digest.
fn collect_runtime_source_records(
    source_root: &Path,
    current: &Path,
    records: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let mut entries = fs::read_dir(current)
        .map_err(|error| format!("failed to read runtime source directory {}: {error}", current.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read runtime source directory {}: {error}", current.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("failed to inspect runtime source {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!("runtime source {} must not contain symlinks", path.display()));
        }
        if metadata.is_dir() {
            collect_runtime_source_records(source_root, &path, records)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(format!(
                "runtime source {} must contain only regular files",
                path.display()
            ));
        }
        let relative = path
            .strip_prefix(source_root)
            .map_err(|_| format!("runtime source {} escaped {}", path.display(), source_root.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let key = format!("src/{relative}");
        let digest = digest_bytes(
            &fs::read(&path).map_err(|error| format!("failed to read runtime source {}: {error}", path.display()))?,
        );
        if records.insert(key.clone(), digest).is_some() {
            return Err(format!("runtime source contains duplicate portable path {key}"));
        }
    }
    Ok(())
}

/// Export one compiler-owned Loaf from an already receipted generated Incan project.
///
/// Release packaging first drives the compiler's ordinary Oven analysis for a small in-package Incan program. That
/// produces the same provider, SDK, feature, dependency, target, and toolchain identity that an everyday command
/// would use. This explicit publisher then converts only that exact generated project into an immutable direct-rustc
/// loaf; its temporary store is dropped before the release archive is created.
pub fn prepare_loaf_from_generated_project(
    loaf_root: &Path,
    context: &OvenLoafBakerContext<'_>,
    receipt: OvenReceipt,
    generated_project: &Path,
) -> Result<OvenLoafPreparation, OvenLoafError> {
    if loaf_root.exists() && !loaf_root.is_dir() {
        return Err(OvenLoafError::Preparation {
            message: format!("loaf root is not a directory: {}", loaf_root.display()),
        });
    }
    fs::create_dir_all(loaf_root).map_err(|source| OvenLoafError::Io {
        path: loaf_root.to_path_buf(),
        source,
    })?;
    let store_root =
        LoafTemporaryDirectory::create(loaf_root, ".incan-oven-loaf-store-").map_err(|source| OvenLoafError::Io {
            path: loaf_root.to_path_buf(),
            source,
        })?;
    let store = OvenStore::new(store_root.path(), context.limits);
    let generated_source = generated_project.join("src/main.rs");
    let compile_environment = direct_rustc_compile_environment(generated_project, &generated_source)?;
    let publication = prepare_direct_rustc_plan(&OvenLegacyCargoPrepareRequest {
        store: &store,
        receipt: receipt.clone(),
        generated_project: generated_project.to_path_buf(),
        cargo: context.cargo.to_path_buf(),
        rustc: context.rustc.to_path_buf(),
        sdk_inventory: None,
        domain: format!("toolchain-base-{}", receipt.intent.profile),
        publication_kind: OvenLegacyCargoPublicationKind::Executable,
        source_evidence_key: "generated-root".to_string(),
        compile_environment,
        inspection_packages: (!context.retain_complete_registry_leaves).then(|| context.inspection_packages.to_vec()),
        direct_dependency_closure: if context.retain_checked_direct_dependencies {
            OvenLegacyCargoDirectDependencyClosure::CheckedDeclared
        } else {
            OvenLegacyCargoDirectDependencyClosure::GeneratedSource
        },
        compact_debug_info: true,
    })?;
    let identity = receipt
        .build_unit_identity
        .strip_prefix("sha256:")
        .unwrap_or(receipt.build_unit_identity.as_str());
    let output_directory = loaf_root.join(format!(".building-{identity}.loaf"));
    if output_directory.exists() {
        return Err(OvenLoafError::Preparation {
            message: format!("loaf destination already exists: {}", output_directory.display()),
        });
    }
    let result = export_loaf(
        &store,
        &publication.plan_identity,
        &receipt,
        context,
        publication.transient_reservation_bytes,
        publication.registry_leaves,
        &output_directory,
    )?;
    let loaf_name = result
        .loaf_identity
        .strip_prefix("sha256:")
        .unwrap_or(&result.loaf_identity);
    let content_directory = loaf_root.join(format!("{loaf_name}.loaf"));
    if content_directory.exists() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "content-addressed Loaf destination already exists: {}",
                content_directory.display()
            ),
        });
    }
    fs::rename(&output_directory, &content_directory).map_err(|source| OvenLoafError::Io {
        path: content_directory,
        source,
    })?;
    Ok(result)
}

/// Loaf loading, validation, or store-publication failure.
#[derive(Debug, Error)]
pub enum OvenLoafError {
    /// A compiler-owned loaf file could not be read.
    #[error("failed to read Oven Loaf {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    /// A loaf payload is malformed or belongs to an unsupported schema.
    #[error("invalid Oven Loaf {path}: {message}")]
    InvalidLoaf { path: PathBuf, message: String },
    /// The declared closure is not a valid direct-rustc artifact plan.
    #[error(transparent)]
    Plan(#[from] OvenRustcError),
    /// Bounded publication refused the requested immutable closure.
    #[error(transparent)]
    Store(#[from] OvenStoreError),
    /// The release-stage publisher could not prepare its temporary direct-rustc closure.
    #[error(transparent)]
    Publisher(#[from] OvenLegacyCargoError),
    /// A release-stage Loaf could not be assembled safely.
    #[error("failed to prepare Oven Loaf: {message}")]
    Preparation { message: String },
}

/// Copy a fully verified temporary store entry into the compiler-owned loaf layout and report its accounting.
fn export_loaf(
    store: &OvenStore,
    plan_identity: &str,
    receipt: &OvenReceipt,
    context: &OvenLoafBakerContext<'_>,
    publisher_transient_peak: u64,
    registry_leaves: Vec<OvenRustcRegistryLeaf>,
    output_directory: &Path,
) -> Result<OvenLoafPreparation, OvenLoafError> {
    let inspection = store.inspect()?;
    let entry = inspection
        .entries
        .iter()
        .find(|entry| entry.manifest.identity == plan_identity)
        .ok_or_else(|| OvenLoafError::Preparation {
            message: format!("temporary Loaf plan {plan_identity} is absent after publication"),
        })?;
    if entry.manifest.kind != OvenArtifactKind::DirectRustcPlan
        || entry.manifest.build_unit_identity != receipt.build_unit_identity
        || entry.manifest.intent != receipt.intent
    {
        return Err(OvenLoafError::Preparation {
            message: "temporary Loaf plan does not match its base runtime receipt".to_string(),
        });
    }
    let (_manifest, artifact_root, payload, _lease) = store.select_payload_for_execution(plan_identity)?;
    let mut plan =
        serde_json::from_slice::<OvenRustcArtifactManifest>(&payload).map_err(|error| OvenLoafError::Preparation {
            message: format!("temporary Loaf payload is not a direct-rustc plan: {error}"),
        })?;
    discard_loaf_metadata_sidecars(&mut plan);
    record_generated_root_externs(&mut plan);
    promote_compiler_runtime_externs(&mut plan)?;
    plan.registry_leaves = registry_leaves.clone();
    let materialized_files = plan.materialized_artifacts(&artifact_root, &receipt.intent)?;
    let parent = output_directory.parent().ok_or_else(|| OvenLoafError::Preparation {
        message: format!("loaf destination has no parent: {}", output_directory.display()),
    })?;
    let staging = LoafTemporaryDirectory::create(parent, ".incan-oven-loaf-").map_err(|source| OvenLoafError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    for file in materialized_files {
        let destination = staging.path().join(&file.relative_path);
        let destination_parent = destination.parent().ok_or_else(|| OvenLoafError::Preparation {
            message: format!("loaf artifact has no parent: {}", file.relative_path),
        })?;
        fs::create_dir_all(destination_parent).map_err(|source_error| OvenLoafError::Io {
            path: destination_parent.to_path_buf(),
            source: source_error,
        })?;
        fs::copy(&file.source_path, &destination).map_err(|source_error| OvenLoafError::Io {
            path: file.source_path,
            source: source_error,
        })?;
    }
    merge_loaf_inspection_sources(&mut plan, staging.path(), context.inspection_sources)?;
    let vocab_transient_peak = bake_compiler_vocab_support(
        &mut plan,
        staging.path(),
        context.cargo,
        context.rustc,
        context.compiler_support_target,
        &context.capacity_roots,
        context.transient_limit,
    )?;
    plan.materialized_artifacts(staging.path(), &receipt.intent)?;
    let (payload_logical_bytes, payload_physical_bytes) = loaf_directory_byte_counts(staging.path())?;
    // Export rewrites publisher-store paths and adds compiler-owned runtime/vocabulary inputs. Report the identity
    // of this final sealed plan, which is also what exact warm validation observes, rather than the discarded
    // temporary store entry identity.
    let plan_identity = digest_bytes(&serde_json::to_vec(&plan).map_err(|error| OvenLoafError::Preparation {
        message: format!("could not encode sealed Loaf plan identity: {error}"),
    })?);
    let loaf = OvenLoaf {
        schema_version: OVEN_LOAF_SCHEMA_VERSION,
        build_unit_identity: receipt.build_unit_identity.clone(),
        provenance: OvenLoafProvenance {
            compiler_version: INCAN_VERSION.to_string(),
            rust_toolchain: receipt.intent.toolchain.clone(),
            sdk_provider_codegen_revision: SDK_PROVIDER_CODEGEN_REVISION.to_string(),
            baker: "legacy_cargo".to_string(),
        },
        accounting: OvenLoafAccounting {
            payload_logical_bytes,
            payload_physical_bytes,
        },
        compatibility: OvenLoafCompatibility::from_receipt(receipt)?,
        registry_leaves,
        plan,
    };
    let loaf_bytes = serde_json::to_vec_pretty(&loaf).map_err(|error| OvenLoafError::Preparation {
        message: format!("could not encode Loaf: {error}"),
    })?;
    let loaf_identity = digest_bytes(&loaf_bytes);
    let loaf_path = staging.path().join("loaf.json");
    fs::write(&loaf_path, loaf_bytes).map_err(|source| OvenLoafError::Io {
        path: loaf_path,
        source,
    })?;
    fs::rename(staging.path(), output_directory).map_err(|source| OvenLoafError::Io {
        path: output_directory.to_path_buf(),
        source,
    })?;
    let _ = staging.persist();
    let (logical_bytes, physical_bytes) = loaf_directory_byte_counts(output_directory)?;
    Ok(OvenLoafPreparation {
        build_unit_identity: receipt.build_unit_identity.clone(),
        loaf_identity,
        plan_identity,
        logical_bytes,
        physical_bytes,
        transient_peak_physical_bytes: publisher_transient_peak.max(vocab_transient_peak),
    })
}

/// Seal independently resolved registry sources into a Loaf without inventing linkable artifacts.
///
/// The compiler-suite source manifest may name packages that its small foundation program never imports. Those
/// packages still need immutable source authority for locked Rust-interoperability tests, but they must not appear as
/// externs or registry leaves. Existing source records are merged only when their complete identity agrees.
fn merge_loaf_inspection_sources(
    plan: &mut OvenRustcArtifactManifest,
    loaf_staging: &Path,
    sources: &[OvenLegacyCargoInspectionSource],
) -> Result<(), OvenLoafError> {
    for source in sources {
        let directory_name = source
            .source_root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| OvenLoafError::Preparation {
                message: format!(
                    "sealed registry source for `{}` {} has no portable directory identity",
                    source.package, source.version
                ),
            })?;
        let relative_root = format!("registry-sources/{directory_name}");
        let destination = loaf_staging.join(&relative_root);
        if destination.exists() {
            let digest = super::digest_source_tree(&destination).map_err(|message| OvenLoafError::Preparation {
                message: format!(
                    "could not verify existing sealed source for `{}` {}: {message}",
                    source.package, source.version
                ),
            })?;
            if digest != source.source_digest {
                return Err(OvenLoafError::Preparation {
                    message: format!(
                        "sealed registry source for `{}` {} conflicts with existing Loaf content",
                        source.package, source.version
                    ),
                });
            }
        } else {
            copy_regular_directory_tree(&source.source_root, &destination, "registry inspection source")?;
        }
        for file in materialized_files_from_directory(&destination, &relative_root, "registry inspection source")? {
            let bytes = fs::read(&file.source_path).map_err(|source_error| OvenLoafError::Io {
                path: file.source_path.clone(),
                source: source_error,
            })?;
            plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
                relative_path: file.relative_path,
                digest: digest_bytes(&bytes),
            });
        }

        let sealed = OvenRustcRegistrySource {
            registry: source.registry.clone(),
            checksum: source.checksum.clone(),
            relative_root,
            digest: source.source_digest.clone(),
        };
        if let Some(existing) = plan.registry_sources.iter_mut().find(|existing| {
            existing.package == source.package
                && existing.version == source.version
                && existing.source.registry == source.registry
        }) {
            if existing.source != sealed {
                return Err(OvenLoafError::Preparation {
                    message: format!(
                        "sealed registry source for `{}` {} disagrees with the generated-project authority",
                        source.package, source.version
                    ),
                });
            }
            existing.features.extend(source.features.iter().cloned());
            existing.features.sort();
            existing.features.dedup();
        } else {
            let mut features = source.features.clone();
            features.sort();
            features.dedup();
            plan.registry_sources.push(OvenRustcRegistrySourcePackage {
                package: source.package.clone(),
                version: source.version.clone(),
                features,
                source: sealed,
            });
        }
    }
    plan.registry_sources.sort_by(|left, right| {
        (&left.package, &left.version, &left.source.registry).cmp(&(
            &right.package,
            &right.version,
            &right.source.registry,
        ))
    });
    canonicalize_supporting_artifacts(&mut plan.supporting_artifacts)?;
    plan.validate_shape(&plan.intent)?;
    Ok(())
}

/// Preserve the publisher-selected generated-root dependency set before the loaf adds compiler-only helpers.
///
/// The ordinary Loaf is published from a minimal generated program, so its original direct externs are the
/// roots that generated caller code may receive. Later preparation adds compiler runtime and vocabulary capabilities
/// to the same immutable closure. Runtime roots are promoted into every declared entrypoint below, but the vocabulary
/// helper roots must remain private to vocabulary extraction: passing their independently built `serde` closure to a
/// generated library would make Rustc see two incompatible `serde` identities.
fn record_generated_root_externs(plan: &mut OvenRustcArtifactManifest) {
    plan.entrypoint_externs
        .entry("generated-root".to_string())
        .or_insert_with(|| {
            let mut crate_names = plan
                .externs
                .iter()
                .map(|artifact| artifact.crate_name.clone())
                .collect::<Vec<_>>();
            crate_names.sort();
            crate_names.dedup();
            crate_names
        });
}

/// Promote compiler runtime artifacts required by generated provider libraries to direct externs.
///
/// The minimal loaf program need not use models or provider metadata, while generated caller-owned libraries do.
/// `incan_derive` and `incan_core` are therefore promoted from the verified support closure. Leaving either only on
/// `-L dependency` relies on Cargo's implicit extern selection and makes a normal direct-Rustc consumer recompile
/// compiler source instead of linking the selected immutable plan.
fn promote_compiler_runtime_externs(plan: &mut OvenRustcArtifactManifest) -> Result<(), OvenLoafError> {
    promote_compiler_runtime_extern(plan, "incan_derive", is_incan_derive_artifact)?;
    promote_compiler_runtime_extern(plan, "incan_core", |relative_path| {
        is_named_rlib(relative_path, "incan_core")
    })
}

/// Promote one exact compiler-owned support artifact after confirming the loaf is unambiguous.
fn promote_compiler_runtime_extern(
    plan: &mut OvenRustcArtifactManifest,
    crate_name: &str,
    matches_artifact: impl Fn(&str) -> bool,
) -> Result<(), OvenLoafError> {
    if plan.externs.iter().any(|artifact| artifact.crate_name == crate_name) {
        return Ok(());
    }
    let candidates = plan
        .supporting_artifacts
        .iter()
        .enumerate()
        .filter(|(_, artifact)| matches_artifact(&artifact.relative_path))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let [index] = candidates.as_slice() else {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "native foundation must declare exactly one compiler `{crate_name}` direct-Rustc artifact; found {}",
                candidates.len()
            ),
        });
    };
    let artifact = plan.supporting_artifacts.remove(*index);
    plan.externs.push(OvenRustcArtifactExtern {
        crate_name: crate_name.to_string(),
        relative_path: artifact.relative_path,
        digest: artifact.digest,
    });
    plan.externs
        .sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    for crate_names in plan.entrypoint_externs.values_mut() {
        if !crate_names
            .iter()
            .any(|entrypoint_extern| entrypoint_extern == crate_name)
        {
            crate_names.push(crate_name.to_string());
            crate_names.sort();
        }
    }
    Ok(())
}

/// Run one baker-owned Cargo child while enforcing the aggregate transient physical allowance.
fn run_bounded_loaf_cargo(
    command: &mut Command,
    capacity_roots: &[&Path],
    transient_limit: u64,
    capture_root: &Path,
    label: &str,
) -> Result<(Vec<u8>, u64), OvenLoafError> {
    fs::create_dir_all(capture_root).map_err(|source| OvenLoafError::Io {
        path: capture_root.to_path_buf(),
        source,
    })?;
    let sequence = LOAF_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let stdout_path = capture_root.join(format!(".oven-loaf-cargo-{sequence}.stdout"));
    let stderr_path = capture_root.join(format!(".oven-loaf-cargo-{sequence}.stderr"));
    let stdout = File::create(&stdout_path).map_err(|source| OvenLoafError::Io {
        path: stdout_path.clone(),
        source,
    })?;
    let stderr = File::create(&stderr_path).map_err(|source| OvenLoafError::Io {
        path: stderr_path.clone(),
        source,
    })?;
    command.stdout(Stdio::from(stdout)).stderr(Stdio::from(stderr));
    isolate_process_group(command);
    let mut child = command.spawn().map_err(|source| OvenLoafError::Io {
        path: PathBuf::from(label),
        source,
    })?;
    let mut peak = 0_u64;
    let status = loop {
        let observed = capacity_roots.iter().try_fold(0_u64, |total, root| {
            super::legacy_cargo::conservative_directory_reservation(root).map(|bytes| total.saturating_add(bytes))
        })?;
        peak = peak.max(observed);
        if observed > transient_limit {
            terminate_process_group(&mut child).map_err(|source| OvenLoafError::Io {
                path: PathBuf::from(label),
                source,
            })?;
            return Err(OvenLoafError::Preparation {
                message: format!(
                    "{label} exceeded the {transient_limit}-byte Loaf transient allowance at {observed} bytes"
                ),
            });
        }
        if let Some(status) = child.try_wait().map_err(|source| OvenLoafError::Io {
            path: PathBuf::from(label),
            source,
        })? {
            break status;
        }
        thread::sleep(Duration::from_millis(25));
    };
    let output = fs::read(&stdout_path).map_err(|source| OvenLoafError::Io {
        path: stdout_path.clone(),
        source,
    })?;
    let diagnostics = fs::read(&stderr_path).map_err(|source| OvenLoafError::Io {
        path: stderr_path.clone(),
        source,
    })?;
    let _ = fs::remove_file(&stdout_path);
    let _ = fs::remove_file(&stderr_path);
    if !status.success() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "{label} publisher failed:\n{}",
                String::from_utf8_lossy(&diagnostics).trim()
            ),
        });
    }
    Ok((output, peak))
}

/// Build and seal the compiler-owned vocab registration closure into a native loaf.
///
/// A generated Incan program need not use JSON, while the compiler's vocab contract always serializes metadata.
/// Consequently, this compiler-owned closure cannot be inferred from a caller program's provider features. The
/// explicit `legacy_cargo` publisher builds only `incan_vocab` against the repository lockfile, copies its small
/// target-specific Rust closure into the immutable Loaf, and records the two helper roots as a host-target auxiliary
/// closure. Vocabulary extraction receives that closure; normal generated roots do not. This prevents a compiler
/// helper's separately compiled `serde_json` from becoming a second authority beside the full stdlib's `serde_json`.
/// No normal command can re-run this Cargo operation.
fn bake_compiler_vocab_support(
    plan: &mut OvenRustcArtifactManifest,
    loaf_staging: &Path,
    cargo: &Path,
    rustc: &Path,
    cargo_target: &Path,
    capacity_roots: &[&Path],
    transient_limit: u64,
) -> Result<u64, OvenLoafError> {
    const INCAN_VOCAB: &str = "incan_vocab";
    const VOCAB_DESUGARER_TARGET: &str = "wasm32-wasip1";
    if plan.externs.iter().any(|artifact| artifact.crate_name == INCAN_VOCAB) {
        return Err(OvenLoafError::Preparation {
            message: "native foundation unexpectedly declares the compiler-owned incan_vocab extern".to_string(),
        });
    }
    if !cargo.is_file() {
        return Err(OvenLoafError::Preparation {
            message: format!("native vocabulary publisher Cargo is not a file: {}", cargo.display()),
        });
    }
    if !rustc.is_file() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "native vocabulary publisher Rust compiler is not a file: {}",
                rustc.display()
            ),
        });
    }
    let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crate_root = source_root.join("crates/incan_vocab");
    let manifest = crate_root.join("Cargo.toml");
    if !manifest.is_file() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "native vocabulary publisher manifest is unavailable: {}",
                manifest.display()
            ),
        });
    }

    let support_root = loaf_staging.join("compiler-support");
    let mut command = Command::new(cargo);
    command
        .current_dir(&crate_root)
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--message-format=json-render-diagnostics")
        .arg("--target")
        .arg(&plan.intent.target)
        .arg("--target-dir")
        .arg(cargo_target)
        .arg("--locked")
        .arg("--offline");
    if plan.intent.profile == "release" {
        command.arg("--release");
    }
    clear_inherited_cargo_environment(&mut command);
    command.env("RUSTC", rustc).env("CARGO_NET_OFFLINE", "true");
    if plan.intent.profile == "debug" {
        command.env("CARGO_PROFILE_DEV_DEBUG", "0");
    }
    let (compile_stdout, native_peak) = run_bounded_loaf_cargo(
        &mut command,
        capacity_roots,
        transient_limit,
        cargo_target,
        "native vocabulary support",
    )?;

    // Vocabulary companions may ship a Wasm desugarer. Build its compiler-owned dependency closure here at the
    // explicit publisher boundary; a normal `incan build --lib` later invokes only direct Rustc against the copied,
    // digest-verified files. Keep this as a separate target directory so host artifacts can never be selected for
    // a Wasm command by accident.
    let mut wasm_command = Command::new(cargo);
    wasm_command
        .current_dir(&crate_root)
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--message-format=json-render-diagnostics")
        .arg("--target")
        .arg(VOCAB_DESUGARER_TARGET)
        .arg("--target-dir")
        .arg(cargo_target)
        .arg("--locked")
        .arg("--offline");
    if plan.intent.profile == "release" {
        wasm_command.arg("--release");
    }
    clear_inherited_cargo_environment(&mut wasm_command);
    wasm_command.env("RUSTC", rustc).env("CARGO_NET_OFFLINE", "true");
    if plan.intent.profile == "debug" {
        wasm_command.env("CARGO_PROFILE_DEV_DEBUG", "0");
    }
    let (wasm_stdout, wasm_peak) = run_bounded_loaf_cargo(
        &mut wasm_command,
        capacity_roots,
        transient_limit,
        cargo_target,
        "native vocabulary Wasm support",
    )?;

    let profile = if plan.intent.profile == "release" {
        "release"
    } else {
        "debug"
    };
    let artifact_directory = cargo_target.join(&plan.intent.target).join(profile).join("deps");
    // Cargo places procedural macros for the host compiler under the profile-only directory even when the native
    // build target equals the host. The target closure alone therefore cannot link a crate whose metadata names a
    // derive macro such as `serde_derive`.
    let host_artifact_directory = cargo_target.join(profile).join("deps");
    let loaf_directory = support_root.join("deps");
    let host_artifacts = compiler_artifact_paths_from_cargo_output(
        &compile_stdout,
        cargo_target,
        &[&artifact_directory, &host_artifact_directory],
        INCAN_VOCAB,
        &cargo_target.join(&plan.intent.target).join(profile),
        "native vocabulary support",
    )?;
    copy_compiler_vocab_support_artifacts(
        &host_artifacts,
        &artifact_directory,
        &cargo_target.join(&plan.intent.target).join(profile),
        &host_artifact_directory,
        &loaf_directory,
        plan,
    )?;
    let wasm_artifact_directory = cargo_target.join(VOCAB_DESUGARER_TARGET).join(profile).join("deps");
    let wasm_artifact_directory_canonical =
        fs::canonicalize(&wasm_artifact_directory).map_err(|source| OvenLoafError::Io {
            path: wasm_artifact_directory.clone(),
            source,
        })?;
    let wasm_primary_artifact_directory = cargo_target.join(VOCAB_DESUGARER_TARGET).join(profile);
    let wasm_primary_artifact_directory_canonical =
        fs::canonicalize(&wasm_primary_artifact_directory).map_err(|source| OvenLoafError::Io {
            path: wasm_primary_artifact_directory.clone(),
            source,
        })?;
    let wasm_artifacts = compiler_artifact_paths_from_cargo_output(
        &wasm_stdout,
        cargo_target,
        &[&wasm_artifact_directory, &host_artifact_directory],
        INCAN_VOCAB,
        &wasm_primary_artifact_directory,
        "native vocabulary Wasm support",
    )?
    .into_iter()
    .filter(|artifact| {
        artifact.starts_with(&wasm_artifact_directory_canonical)
            || artifact.parent() == Some(wasm_primary_artifact_directory_canonical.as_path())
    })
    .collect::<Vec<_>>();
    copy_compiler_vocab_auxiliary_target_artifacts(
        &wasm_artifacts,
        &support_root.join(VOCAB_DESUGARER_TARGET).join("deps"),
        VOCAB_DESUGARER_TARGET,
        plan,
    )?;
    plan.externs
        .sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    plan.validate_shape(&plan.intent)?;
    Ok(native_peak.max(wasm_peak))
}

/// Copy the sealed `incan_vocab` direct-Rustc support set produced by the compiler-owned package build.
///
/// The named publisher starts from an empty target directory, builds only `incan_vocab` against the checked lockfile,
/// and retains only the exact Rust-library paths in Cargo's `compiler-artifact` records for that invocation. The two
/// roots selected by vocabulary extraction (`incan_vocab` and `serde_json`) are host-target auxiliary externs; they
/// are deliberately not normal program externs. The remaining digested artifacts are their direct-Rustc support
/// closure, including host procedural macros. A stale or unrelated Cargo `deps` file is neither scanned nor admitted.
/// The normal guarded library-vocab regression exercises that sealed set and fails if a consumer attempts to launch
/// Cargo.
fn copy_compiler_vocab_support_artifacts(
    source_artifacts: &[PathBuf],
    target_artifact_directory: &Path,
    primary_artifact_directory: &Path,
    host_artifact_directory: &Path,
    loaf_directory: &Path,
    plan: &mut OvenRustcArtifactManifest,
) -> Result<(), OvenLoafError> {
    const INCAN_VOCAB: &str = "incan_vocab";
    const SERDE_JSON: &str = "serde_json";
    if !target_artifact_directory.is_dir() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "native vocabulary publisher produced no target dependency directory: {}",
                target_artifact_directory.display()
            ),
        });
    }
    if !primary_artifact_directory.is_dir() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "native vocabulary publisher produced no primary artifact directory: {}",
                primary_artifact_directory.display()
            ),
        });
    }
    if !host_artifact_directory.is_dir() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "native vocabulary publisher produced no host dependency directory: {}",
                host_artifact_directory.display()
            ),
        });
    }
    fs::create_dir_all(loaf_directory).map_err(|source| OvenLoafError::Io {
        path: loaf_directory.to_path_buf(),
        source,
    })?;
    let target_artifact_directory =
        fs::canonicalize(target_artifact_directory).map_err(|source| OvenLoafError::Io {
            path: target_artifact_directory.to_path_buf(),
            source,
        })?;
    let primary_artifact_directory =
        fs::canonicalize(primary_artifact_directory).map_err(|source| OvenLoafError::Io {
            path: primary_artifact_directory.to_path_buf(),
            source,
        })?;
    let host_artifact_directory = fs::canonicalize(host_artifact_directory).map_err(|source| OvenLoafError::Io {
        path: host_artifact_directory.to_path_buf(),
        source,
    })?;
    let mut copied = BTreeMap::new();
    let mut target_copied = BTreeMap::new();
    for source in source_artifacts {
        let target_artifacts = source.starts_with(&target_artifact_directory)
            || source.parent() == Some(primary_artifact_directory.as_path());
        if !target_artifacts && !source.starts_with(&host_artifact_directory) {
            return Err(OvenLoafError::Preparation {
                message: format!(
                    "native vocabulary compiler-artifact escaped its declared target or host dependency directory: {}",
                    source.display()
                ),
            });
        }
        let metadata = fs::symlink_metadata(source).map_err(|source_error| OvenLoafError::Io {
            path: source.clone(),
            source: source_error,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || !is_rust_library_artifact(source) {
            return Err(OvenLoafError::Preparation {
                message: format!(
                    "native vocabulary compiler-artifact is not a regular direct-Rustc library: {}",
                    source.display()
                ),
            });
        }
        let file_name = source
            .file_name()
            .ok_or_else(|| OvenLoafError::Preparation {
                message: format!("native vocabulary artifact has no filename: {}", source.display()),
            })?
            .to_string_lossy()
            .to_string();
        let bytes = fs::read(source).map_err(|source_error| OvenLoafError::Io {
            path: source.clone(),
            source: source_error,
        })?;
        let digest = digest_bytes(&bytes);
        if let Some(existing) = copied.insert(file_name.clone(), digest.clone())
            && existing != digest
        {
            return Err(OvenLoafError::Preparation {
                message: format!("native vocabulary target and host closures conflict on artifact `{file_name}`"),
            });
        }
        if target_artifacts {
            target_copied.insert(file_name.clone(), digest.clone());
        }
        let destination = loaf_directory.join(&file_name);
        fs::write(&destination, bytes).map_err(|source_error| OvenLoafError::Io {
            path: destination,
            source: source_error,
        })?;
    }
    if copied.is_empty() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "native vocabulary publisher retained no Rust artifacts from {}",
                target_artifact_directory.display()
            ),
        });
    }

    let relative_directory = "compiler-support/deps".to_string();
    let mut externs = Vec::new();
    for (file_name, digest) in &copied {
        let relative_path = format!("{relative_directory}/{file_name}");
        plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: relative_path.clone(),
            digest: digest.clone(),
        });
    }
    for crate_name in [INCAN_VOCAB, SERDE_JSON] {
        let matches = target_copied
            .iter()
            .filter(|(file_name, _)| is_named_rlib(file_name, crate_name))
            .collect::<Vec<_>>();
        let [(file_name, digest)] = matches.as_slice() else {
            return Err(OvenLoafError::Preparation {
                message: format!(
                    "native vocabulary publisher must retain exactly one `{crate_name}` rlib; found {}",
                    matches.len()
                ),
            });
        };
        let relative_path = format!("{relative_directory}/{file_name}");
        plan.supporting_artifacts
            .retain(|artifact| artifact.relative_path != relative_path);
        externs.push(OvenRustcArtifactExtern {
            crate_name: crate_name.to_string(),
            relative_path,
            digest: digest.to_string(),
        });
    }
    plan.vocab_auxiliary_targets.push(OvenRustcAuxiliaryTarget {
        target: plan.intent.target.clone(),
        dependency_search_paths: vec![relative_directory],
        externs,
    });
    plan.vocab_auxiliary_targets
        .sort_by(|left, right| left.target.cmp(&right.target));
    plan.supporting_artifacts
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(())
}

/// Return only Rust-library files explicitly emitted by one named publisher Cargo invocation.
///
/// Cargo's dependency directory is staging state, not an Oven input contract. The publisher requests structured
/// `compiler-artifact` output and this helper admits only listed regular files beneath the exact target/host
/// dependency directories supplied by the caller. Cargo reports the primary package's rlib only at the profile root
/// (with a dependency-directory rmeta): that one exact, named rlib is admitted as an explicit Rustc extern. Every
/// other Rust-library file outside the dependency directories is refused. A path outside the one publisher target
/// root is likewise refused. This keeps an unrelated retained `deps` artifact from becoming a silent immutable loaf
/// dependency while still retaining the publisher's real primary artifact.
fn compiler_artifact_paths_from_cargo_output(
    cargo_stdout: &[u8],
    publisher_target_root: &Path,
    allowed_directories: &[&Path],
    primary_crate_name: &str,
    primary_artifact_directory: &Path,
    publisher: &str,
) -> Result<Vec<PathBuf>, OvenLoafError> {
    let publisher_target_root = fs::canonicalize(publisher_target_root).map_err(|source| OvenLoafError::Io {
        path: publisher_target_root.to_path_buf(),
        source,
    })?;
    let allowed_directories = allowed_directories
        .iter()
        .map(|directory| {
            fs::canonicalize(directory).map_err(|source| OvenLoafError::Io {
                path: (*directory).to_path_buf(),
                source,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if allowed_directories.is_empty() {
        return Err(OvenLoafError::Preparation {
            message: format!("{publisher} publisher declared no Cargo artifact directories"),
        });
    }
    let primary_artifact_directory =
        fs::canonicalize(primary_artifact_directory).map_err(|source| OvenLoafError::Io {
            path: primary_artifact_directory.to_path_buf(),
            source,
        })?;
    let primary_artifact_filename = format!("lib{primary_crate_name}.rlib");
    let cargo_stdout = std::str::from_utf8(cargo_stdout).map_err(|error| OvenLoafError::Preparation {
        message: format!("{publisher} publisher emitted non-UTF-8 Cargo JSON: {error}"),
    })?;
    let mut artifacts = BTreeSet::new();
    for (line_number, line) in cargo_stdout.lines().enumerate() {
        let value = serde_json::from_str::<serde_json::Value>(line).map_err(|error| OvenLoafError::Preparation {
            message: format!(
                "{publisher} publisher emitted invalid Cargo JSON on line {}: {error}",
                line_number + 1
            ),
        })?;
        if value.get("reason").and_then(serde_json::Value::as_str) != Some("compiler-artifact") {
            continue;
        }
        let filenames = value
            .get("filenames")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| OvenLoafError::Preparation {
                message: format!(
                    "{publisher} publisher compiler-artifact on line {} has no filenames",
                    line_number + 1
                ),
            })?;
        let artifact_target_name = value
            .get("target")
            .and_then(|target| target.get("name"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| OvenLoafError::Preparation {
                message: format!(
                    "{publisher} publisher compiler-artifact on line {} has no target name",
                    line_number + 1
                ),
            })?;
        for filename in filenames {
            let filename = filename.as_str().ok_or_else(|| OvenLoafError::Preparation {
                message: format!(
                    "{publisher} publisher compiler-artifact on line {} has a non-string filename",
                    line_number + 1
                ),
            })?;
            let source = PathBuf::from(filename);
            if !is_rust_library_artifact(&source) {
                continue;
            }
            let metadata = fs::symlink_metadata(&source).map_err(|source_error| OvenLoafError::Io {
                path: source.clone(),
                source: source_error,
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(OvenLoafError::Preparation {
                    message: format!(
                        "{publisher} publisher compiler-artifact is not a regular file: {}",
                        source.display()
                    ),
                });
            }
            let source = fs::canonicalize(&source).map_err(|source_error| OvenLoafError::Io {
                path: source.clone(),
                source: source_error,
            })?;
            let in_dependency_directory = allowed_directories
                .iter()
                .any(|directory| source.starts_with(directory));
            let is_primary_profile_rlib = artifact_target_name == primary_crate_name
                && source.parent() == Some(primary_artifact_directory.as_path())
                && source.file_name().and_then(|name| name.to_str()) == Some(primary_artifact_filename.as_str());
            if in_dependency_directory || is_primary_profile_rlib {
                artifacts.insert(source);
            } else if !source.starts_with(&publisher_target_root) {
                return Err(OvenLoafError::Preparation {
                    message: format!(
                        "{publisher} publisher compiler-artifact escaped its target root: {}",
                        source.display()
                    ),
                });
            } else {
                return Err(OvenLoafError::Preparation {
                    message: format!(
                        "{publisher} publisher compiler-artifact escaped its declared dependency directories: {}",
                        source.display()
                    ),
                });
            }
        }
    }
    if artifacts.is_empty() {
        return Err(OvenLoafError::Preparation {
            message: format!("{publisher} publisher emitted no Rust compiler-artifact files"),
        });
    }
    Ok(artifacts.into_iter().collect())
}

/// Copy the target-only vocabulary support closure used to produce Wasm desugarers without Cargo.
///
/// The host closure remains an auxiliary search path because Rustc may need host procedural macros while compiling
/// target code. The target rlibs are retained separately and named explicitly, which prevents host and Wasm copies
/// of the same crate from occupying one ambiguous direct-Rustc search directory.
fn copy_compiler_vocab_auxiliary_target_artifacts(
    source_artifacts: &[PathBuf],
    loaf_directory: &Path,
    target: &str,
    plan: &mut OvenRustcArtifactManifest,
) -> Result<(), OvenLoafError> {
    const INCAN_VOCAB: &str = "incan_vocab";
    const SERDE_JSON: &str = "serde_json";
    fs::create_dir_all(loaf_directory).map_err(|source| OvenLoafError::Io {
        path: loaf_directory.to_path_buf(),
        source,
    })?;
    let mut artifacts = BTreeMap::new();
    for source in source_artifacts {
        let metadata = fs::symlink_metadata(source).map_err(|source_error| OvenLoafError::Io {
            path: source.clone(),
            source: source_error,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || !is_rust_library_artifact(source) {
            return Err(OvenLoafError::Preparation {
                message: format!(
                    "native vocabulary {target} compiler-artifact is not a regular direct-Rustc library: {}",
                    source.display()
                ),
            });
        }
        let file_name = source
            .file_name()
            .ok_or_else(|| OvenLoafError::Preparation {
                message: format!("native vocabulary artifact has no filename: {}", source.display()),
            })?
            .to_string_lossy()
            .to_string();
        let bytes = fs::read(source).map_err(|source_error| OvenLoafError::Io {
            path: source.clone(),
            source: source_error,
        })?;
        let digest = digest_bytes(&bytes);
        if artifacts.insert(file_name.clone(), digest.clone()).is_some() {
            return Err(OvenLoafError::Preparation {
                message: format!("native vocabulary {target} closure duplicates artifact `{file_name}`"),
            });
        }
        let destination = loaf_directory.join(&file_name);
        fs::write(&destination, bytes).map_err(|source_error| OvenLoafError::Io {
            path: destination,
            source: source_error,
        })?;
    }
    if artifacts.is_empty() {
        return Err(OvenLoafError::Preparation {
            message: format!("native vocabulary publisher retained no declared {target} Rust artifacts"),
        });
    }
    let relative_directory = format!("compiler-support/{target}/deps");
    let mut externs = Vec::new();
    for crate_name in [INCAN_VOCAB, SERDE_JSON] {
        let matches = artifacts
            .iter()
            .filter(|(file_name, _)| is_named_rlib(file_name, crate_name))
            .collect::<Vec<_>>();
        let [(file_name, digest)] = matches.as_slice() else {
            return Err(OvenLoafError::Preparation {
                message: format!(
                    "native vocabulary {target} publisher must retain exactly one `{crate_name}` rlib; found {}",
                    matches.len()
                ),
            });
        };
        externs.push(OvenRustcArtifactExtern {
            crate_name: crate_name.to_string(),
            relative_path: format!("{relative_directory}/{file_name}"),
            digest: digest.to_string(),
        });
    }
    for (file_name, digest) in artifacts {
        let relative_path = format!("{relative_directory}/{file_name}");
        if externs.iter().any(|artifact| artifact.relative_path == relative_path) {
            continue;
        }
        plan.supporting_artifacts
            .push(OvenRustcSupportingArtifact { relative_path, digest });
    }
    // `compiler-support/deps` holds host proc macros that Rustc may load while expanding the target closure.
    let mut dependency_search_paths = vec![relative_directory, "compiler-support/deps".to_string()];
    dependency_search_paths.sort();
    plan.vocab_auxiliary_targets.push(OvenRustcAuxiliaryTarget {
        target: target.to_string(),
        dependency_search_paths,
        externs,
    });
    plan.vocab_auxiliary_targets
        .sort_by(|left, right| left.target.cmp(&right.target));
    Ok(())
}

/// Return whether a file can participate in a direct Rustc dependency closure.
fn is_rust_library_artifact(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("rlib" | "dylib" | "so" | "dll")
    )
}

/// Return whether a retained artifact is the exact rlib for one compiler-owned crate.
fn is_named_rlib(relative_path: &str, crate_name: &str) -> bool {
    let Some(name) = Path::new(relative_path).file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name == format!("lib{crate_name}.rlib")
        || (name.starts_with(&format!("lib{crate_name}-")) && name.ends_with(".rlib"))
}

/// Return whether a manifest path is the dynamic compiler-owned `incan_derive` procedural macro.
fn is_incan_derive_artifact(relative_path: &str) -> bool {
    let Some(name) = Path::new(relative_path).file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.starts_with("libincan_derive-")
        && matches!(
            Path::new(name).extension().and_then(|extension| extension.to_str()),
            Some("dylib" | "so" | "dll")
        )
}

/// Measure the exact directory that will be copied into a toolchain archive.
///
/// This intentionally includes `loaf.json`: while the plan is control metadata rather than a link input, it is a
/// retained physical file and therefore belongs in the release accounting. Publisher source files are copied rather
/// than linked, so summing regular-file allocation gives a conservative, portable report for this final closure.
pub(crate) fn loaf_directory_byte_counts(root: &Path) -> Result<(u64, u64), OvenLoafError> {
    let metadata = fs::symlink_metadata(root).map_err(|source| OvenLoafError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(OvenLoafError::Preparation {
            message: format!("Loaf may not contain a symlink: {}", root.display()),
        });
    }
    if metadata.is_file() {
        return Ok((metadata.len(), loaf_file_physical_bytes(&metadata)));
    }
    if !metadata.is_dir() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "Loaf may contain only regular files and directories: {}",
                root.display()
            ),
        });
    }

    let mut logical_bytes = 0_u64;
    let mut physical_bytes = 0_u64;
    for child in fs::read_dir(root).map_err(|source| OvenLoafError::Io {
        path: root.to_path_buf(),
        source,
    })? {
        let child = child.map_err(|source| OvenLoafError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let (child_logical_bytes, child_physical_bytes) = loaf_directory_byte_counts(&child.path())?;
        logical_bytes = logical_bytes.saturating_add(child_logical_bytes);
        physical_bytes = physical_bytes.saturating_add(child_physical_bytes);
    }
    Ok((logical_bytes, physical_bytes))
}

/// Measure raw host allocation for the complete Loaf directory tree, including directory metadata.
///
/// Policy-accounted physical bytes deliberately cover retained regular artifacts. This second measurement follows
/// the ordinary `du` hard-link rule: one inode contributes its allocated blocks once even when multiple Loaf or store
/// paths name it. Reports therefore include directory and control-file allocation without inventing extra disk use for
/// the store's content-preserving hard links.
pub(crate) fn loaf_raw_disk_bytes(root: &Path) -> Result<u64, OvenLoafError> {
    #[cfg(unix)]
    {
        loaf_raw_disk_bytes_unix(root, &mut std::collections::BTreeSet::new())
    }

    #[cfg(not(unix))]
    {
        loaf_raw_disk_bytes_portable(root)
    }
}

/// Traverse a Unix Loaf tree while counting each allocated inode once.
#[cfg(unix)]
fn loaf_raw_disk_bytes_unix(
    root: &Path,
    seen: &mut std::collections::BTreeSet<(u64, u64)>,
) -> Result<u64, OvenLoafError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::symlink_metadata(root).map_err(|source| OvenLoafError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(OvenLoafError::Preparation {
            message: format!("Loaf may not contain a symlink: {}", root.display()),
        });
    }
    if !seen.insert((metadata.dev(), metadata.ino())) {
        return Ok(0);
    }
    if metadata.is_file() {
        return Ok(loaf_file_physical_bytes(&metadata));
    }
    if !metadata.is_dir() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "Loaf may contain only regular files and directories: {}",
                root.display()
            ),
        });
    }

    let mut bytes = loaf_file_physical_bytes(&metadata);
    for child in fs::read_dir(root).map_err(|source| OvenLoafError::Io {
        path: root.to_path_buf(),
        source,
    })? {
        let child = child.map_err(|source| OvenLoafError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        bytes = bytes.saturating_add(loaf_raw_disk_bytes_unix(&child.path(), seen)?);
    }
    Ok(bytes)
}

/// Traverse a Loaf tree on hosts that do not expose a stable device/inode identity.
#[cfg(not(unix))]
fn loaf_raw_disk_bytes_portable(root: &Path) -> Result<u64, OvenLoafError> {
    let metadata = fs::symlink_metadata(root).map_err(|source| OvenLoafError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(OvenLoafError::Preparation {
            message: format!("Loaf may not contain a symlink: {}", root.display()),
        });
    }
    if metadata.is_file() {
        return Ok(loaf_file_physical_bytes(&metadata));
    }
    if !metadata.is_dir() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "Loaf may contain only regular files and directories: {}",
                root.display()
            ),
        });
    }

    let mut bytes = loaf_file_physical_bytes(&metadata);
    for child in fs::read_dir(root).map_err(|source| OvenLoafError::Io {
        path: root.to_path_buf(),
        source,
    })? {
        let child = child.map_err(|source| OvenLoafError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        bytes = bytes.saturating_add(loaf_raw_disk_bytes_portable(&child.path())?);
    }
    Ok(bytes)
}

/// Return physical allocation for one compiler-shipped loaf file, preserving a portable fallback outside Unix.
#[cfg(unix)]
fn loaf_file_physical_bytes(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;

    metadata.blocks().saturating_mul(512)
}

/// Return logical bytes where the host cannot expose allocated Unix block counts.
#[cfg(not(unix))]
fn loaf_file_physical_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.len()
}

/// Remove Cargo's redundant Rust metadata sidecars from one compiler-shipped loaf.
///
/// The direct-rustc plan names its usable roots as `--extern` rlibs and retains all other required link inputs. A
/// standalone `.rmeta` sidecar is Cargo metadata for a companion rlib, not a direct-rustc input. Removing it here is
/// safe only because the reduced plan is immediately revalidated and copied from that plan; stored compiler-suite
/// closures retain their original artifact inventories.
fn discard_loaf_metadata_sidecars(plan: &mut OvenRustcArtifactManifest) {
    plan.supporting_artifacts
        .retain(|artifact| !artifact.relative_path.ends_with(".rmeta"));
}

/// Validate the committed envelope authority and return only its content-addressed Loaf manifests.
pub(crate) fn committed_loaf_paths(loaf_root: &Path) -> Result<Vec<PathBuf>, OvenLoafError> {
    let paths = committed_loaf_metadata_paths(loaf_root)?;
    for path in &paths {
        let loaf = read_loaf(path)?;
        validate_loaf_declared_file_set(&loaf, path)?;
    }
    Ok(paths)
}

/// Validate the typed committed-envelope authority without traversing every Loaf artifact tree.
///
/// Selection needs only content-addressed metadata and compatibility records to choose one closure. The final
/// selected Loaf is structurally audited by [`loaf_from_loaf_with_lock`] before any artifact path reaches Rustc.
/// Full-generation consumers use [`committed_loaf_paths`] instead.
fn committed_loaf_metadata_paths(loaf_root: &Path) -> Result<Vec<PathBuf>, OvenLoafError> {
    committed_loaf_metadata_paths_with_role(loaf_root, None)
}

/// Resolve only members that provide one authority while still validating the complete envelope.
fn committed_loaf_metadata_paths_for_authority(
    loaf_root: &Path,
    role: OvenLoafMemberRole,
) -> Result<Vec<PathBuf>, OvenLoafError> {
    committed_loaf_metadata_paths_with_role(loaf_root, Some(role))
}

/// Validate a committed envelope and retain the members selected by an optional authority role.
fn committed_loaf_metadata_paths_with_role(
    loaf_root: &Path,
    role: Option<OvenLoafMemberRole>,
) -> Result<Vec<PathBuf>, OvenLoafError> {
    let manifest_path = loaf_root.join("envelope.json");
    let manifest = match fs::read(&manifest_path) {
        Ok(bytes) => {
            serde_json::from_slice::<OvenLoafEnvelopeManifest>(&bytes).map_err(|error| OvenLoafError::InvalidLoaf {
                path: manifest_path.clone(),
                message: format!("invalid committed envelope manifest: {error}"),
            })?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(OvenLoafError::Io {
                path: manifest_path,
                source,
            });
        }
    };
    if manifest.schema_version != OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION {
        return Err(OvenLoafError::InvalidLoaf {
            path: manifest_path,
            message: format!("unsupported envelope manifest schema {}", manifest.schema_version),
        });
    }
    let generation_prefix = Path::new("generations").join(
        manifest
            .generation_identity
            .strip_prefix("sha256:")
            .unwrap_or(&manifest.generation_identity),
    );
    let mut paths = Vec::with_capacity(manifest.loafs.len());
    for member in &manifest.loafs {
        if member.path.is_absolute()
            || member.path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)
                )
            })
            || !member.path.starts_with(&generation_prefix)
            || member.path.file_name().and_then(|name| name.to_str()) != Some("loaf.json")
        {
            return Err(OvenLoafError::InvalidLoaf {
                path: manifest_path,
                message: format!("envelope member `{}` has an unsafe or foreign path", member.label),
            });
        }
        let path = loaf_root.join(&member.path);
        if !path.is_file() {
            return Err(OvenLoafError::InvalidLoaf {
                path,
                message: format!("committed envelope member `{}` is missing", member.label),
            });
        }
        let identity = loaf_file_identity(&path)?;
        let expected_name = format!(
            "{}.loaf",
            member
                .loaf_identity
                .strip_prefix("sha256:")
                .unwrap_or(&member.loaf_identity)
        );
        if identity != member.loaf_identity
            || path.parent().and_then(Path::file_name).and_then(|name| name.to_str()) != Some(expected_name.as_str())
        {
            return Err(OvenLoafError::InvalidLoaf {
                path,
                message: format!("committed envelope member `{}` is not content-addressed", member.label),
            });
        }
        if role.is_none_or(|expected| match expected {
            OvenLoafMemberRole::CompiledClosure => member.role.provides_compiled_closure(),
            OvenLoafMemberRole::SourceAuthority => member.role.provides_source_authority(),
            OvenLoafMemberRole::CompiledClosureAndSourceAuthority => {
                member.role == OvenLoafMemberRole::CompiledClosureAndSourceAuthority
            }
        }) {
            paths.push(path);
        }
    }
    Ok(paths)
}

/// Read and validate the one atomically committed typed envelope manifest.
fn committed_loaf_envelope_manifest(
    loaf_root: &Path,
    expected_envelope: &str,
) -> Result<(OvenLoafEnvelopeManifest, PathBuf), OvenLoafError> {
    let manifest_path = loaf_root.join("envelope.json");
    let bytes = fs::read(&manifest_path).map_err(|source| OvenLoafError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest =
        serde_json::from_slice::<OvenLoafEnvelopeManifest>(&bytes).map_err(|error| OvenLoafError::InvalidLoaf {
            path: manifest_path.clone(),
            message: format!("invalid committed envelope manifest: {error}"),
        })?;
    if manifest.schema_version != OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION {
        return Err(OvenLoafError::InvalidLoaf {
            path: manifest_path,
            message: format!("unsupported envelope manifest schema {}", manifest.schema_version),
        });
    }
    if manifest.envelope != expected_envelope {
        return Err(OvenLoafError::InvalidLoaf {
            path: manifest_path,
            message: format!(
                "committed envelope is `{}`, expected `{expected_envelope}`",
                manifest.envelope
            ),
        });
    }
    let generation_digest =
        manifest
            .generation_identity
            .strip_prefix("sha256:")
            .ok_or_else(|| OvenLoafError::InvalidLoaf {
                path: manifest_path.clone(),
                message: "committed envelope generation identity is not a SHA-256 digest".to_string(),
            })?;
    if generation_digest.len() != 64
        || !generation_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(OvenLoafError::InvalidLoaf {
            path: manifest_path,
            message: "committed envelope generation identity is not a canonical SHA-256 digest".to_string(),
        });
    }
    Ok((manifest, manifest_path))
}

/// Return the stable compiler-suite compatibility identity of the sealed member set.
///
/// A compiler-suite receipt needs to invalidate when a selected Loaf closure or direct-Rustc plan changes, but not
/// merely because envelope publication evidence changed. In particular, editing an `#[cfg(test)]` compiler source
/// can change the executable digest while leaving every lock/toolchain-bound member unchanged. Key the costly
/// compiler-suite foundation to its selected member identities, never to the enclosing generation path, accounting,
/// or evidence map.
pub(crate) fn committed_loaf_envelope_compatibility_identity(
    loaf_root: &Path,
    expected_envelope: &str,
) -> Result<String, OvenLoafError> {
    let (manifest, manifest_path) = committed_loaf_envelope_manifest(loaf_root, expected_envelope)?;
    let mut members = Vec::with_capacity(manifest.loafs.len());
    let mut variants = BTreeSet::new();
    for member in &manifest.loafs {
        if member.label.trim().is_empty()
            || member.profile.trim().is_empty()
            || member.action.trim().is_empty()
            || member.build_unit_identity.trim().is_empty()
            || member.loaf_identity.trim().is_empty()
            || member.plan_identity.trim().is_empty()
        {
            return Err(OvenLoafError::InvalidLoaf {
                path: manifest_path.clone(),
                message: "committed envelope has an incomplete compatibility member".to_string(),
            });
        }
        if !variants.insert((member.label.clone(), member.profile.clone())) {
            return Err(OvenLoafError::InvalidLoaf {
                path: manifest_path.clone(),
                message: format!(
                    "committed envelope repeats Loaf family `{}` for profile `{}`",
                    member.label, member.profile
                ),
            });
        }
        members.push((
            &member.label,
            &member.profile,
            &member.action,
            member.role,
            &member.build_unit_identity,
            &member.loaf_identity,
            &member.plan_identity,
        ));
    }
    members.sort_by(|left, right| left.0.cmp(right.0).then_with(|| left.1.cmp(right.1)));
    let encoded = serde_json::to_vec(&(manifest.schema_version, &manifest.envelope, members)).map_err(|error| {
        OvenLoafError::Preparation {
            message: format!("could not encode committed envelope compatibility identity: {error}"),
        }
    })?;
    Ok(digest_bytes(&encoded))
}

/// One committed Loaf generation whose paths remain stable for the lifetime of its shared lock.
pub(crate) struct OvenCommittedLoafGeneration {
    paths: Vec<PathBuf>,
    _lock: OvenLoafGenerationLock,
}

impl OvenCommittedLoafGeneration {
    /// Return the verified Loaf manifests protected by this generation's shared lock.
    pub(crate) fn paths(&self) -> &[PathBuf] {
        &self.paths
    }
}

/// Acquire a shared generation lock and resolve the complete currently committed Loaf set.
pub(crate) fn acquire_committed_loaf_generation(
    loaf_root: &Path,
) -> Result<Option<OvenCommittedLoafGeneration>, OvenLoafError> {
    if !loaf_root.join("envelope.json").is_file() {
        return Ok(None);
    }
    let generation_lock = acquire_loaf_generation_lock(loaf_root)?;
    let paths = committed_loaf_paths(loaf_root)?;
    Ok(Some(OvenCommittedLoafGeneration {
        paths,
        _lock: generation_lock,
    }))
}

/// Find the one committed Loaf whose exact build-unit identity matches `receipt`.
fn exact_committed_loaf_path(loaf_root: &Path, receipt: &OvenReceipt) -> Result<Option<PathBuf>, OvenLoafError> {
    for path in committed_loaf_metadata_paths_for_authority(loaf_root, OvenLoafMemberRole::CompiledClosure)? {
        let loaf = read_loaf(&path)?;
        if loaf.build_unit_identity == receipt.build_unit_identity {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Hold one committed generation stable while a consumer verifies and uses its Loafs.
fn acquire_loaf_generation_lock(loaf_root: &Path) -> Result<OvenLoafGenerationLock, OvenLoafError> {
    let path = loaf_root.join(OVEN_LOAF_ENVELOPE_LOCK_FILE);
    let file = File::open(&path).map_err(|source| OvenLoafError::Io {
        path: path.clone(),
        source,
    })?;
    file.lock_shared()
        .map_err(|source| OvenLoafError::Io { path, source })?;
    Ok(OvenLoafGenerationLock { file })
}

/// Check whether one already validated compiler-native loaf can supply all caller-visible registry imports from its
/// own exact catalog. A missing or incompatible leaf disqualifies this loaf; it never widens the caller to Cargo.
fn registry_dependencies_supported_by_loaf(
    native: &OvenToolchainLoaf,
    dependencies: &[&DependencySpec],
    profile: &str,
) -> bool {
    let authority = native.registry_leaf_authority();
    dependencies
        .iter()
        .all(|dependency| validate_sealed_registry_leaf(dependency, Some(&authority), profile).is_ok())
}

/// Check whether one immutable registry-source catalog contains exactly one compatible source authority per dependency.
///
/// This is intentionally independent of direct-`rustc` leaf selection. The catalog may be shared by multiple
/// feature-unified compiled closures, but package, version, and selected feature evidence remain exact at this
/// inspection boundary.
fn registry_source_dependencies_supported_by_catalog(
    sources: &[OvenRustcRegistrySourcePackage],
    dependencies: &[&DependencySpec],
) -> bool {
    dependencies.iter().all(|dependency| {
        let Some(requirement) = dependency
            .version
            .as_deref()
            .and_then(|version| semver::VersionReq::parse(version).ok())
        else {
            return false;
        };
        let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
        let required_features = dependency.features.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let matching = sources
            .iter()
            .filter(|source| {
                source.package == package
                    && semver::Version::parse(&source.version).is_ok_and(|version| requirement.matches(&version))
                    && required_features
                        .iter()
                        .all(|feature| source.features.iter().any(|selected| selected == *feature))
            })
            .count();
        matching == 1
    })
}

/// Registry capability required while choosing a compatible Loaf.
#[derive(Clone, Copy)]
enum OvenLoafRegistryRequirement {
    LinkableLeaf,
}

/// Resolve a compiler-owned full-stdlib Loaf for direct execution without copying it into a mutable Oven store.
///
/// Selection validates the immutable generation, full receipt compatibility, and every caller-visible registry root
/// before it returns a plan. It is not a dependency resolver: a project dependency absent from the sealed Loaf still
/// requires an explicit project bake.
pub fn resolve_toolchain_loaf(
    receipt: &OvenReceipt,
    selection: OvenLoafSelection,
) -> Result<Option<OvenToolchainLoaf>, OvenLoafError> {
    select_toolchain_loaf(receipt, selection, &[], OvenLoafRegistryRequirement::LinkableLeaf)
}

/// Resolve a compiler-owned full-stdlib Loaf whose linkable catalog satisfies every caller registry root.
pub fn resolve_toolchain_loaf_for_registry_dependencies(
    receipt: &OvenReceipt,
    selection: OvenLoafSelection,
    dependencies: &[DependencySpec],
) -> Result<Option<OvenToolchainLoaf>, OvenLoafError> {
    select_toolchain_loaf(
        receipt,
        selection,
        dependencies,
        OvenLoafRegistryRequirement::LinkableLeaf,
    )
}

/// Resolve a scheduler-held source-authority Loaf whose immutable source catalog satisfies every registry root.
///
/// Source inspection must not widen direct-`rustc` linkage. This selector therefore chooses only envelope members
/// explicitly marked [`OvenLoafMemberRole::SourceAuthority`], while normal commands continue to select one coherent
/// compiled closure through [`resolve_toolchain_loaf_for_registry_dependencies`].
pub fn resolve_toolchain_loaf_for_registry_sources(
    receipt: &OvenReceipt,
    dependencies: &[DependencySpec],
) -> Result<Option<OvenToolchainLoaf>, OvenLoafError> {
    let registry_dependencies = dependencies
        .iter()
        .filter(|dependency| matches!(dependency.source, DependencySource::Registry))
        .collect::<Vec<_>>();
    let loaf_root = crate::toolchain_layout::resolve_toolchain_data_path(Path::new(TOOLCHAIN_LOAF_RELATIVE_ROOT));
    if !loaf_root.join("envelope.json").is_file() {
        return Ok(None);
    }
    let generation_lock = acquire_loaf_generation_lock(&loaf_root)?;
    let mut candidates = Vec::new();
    for loaf_path in committed_loaf_metadata_paths_for_authority(&loaf_root, OvenLoafMemberRole::SourceAuthority)? {
        let loaf = read_loaf(&loaf_path)?;
        if loaf.schema_version != OVEN_LOAF_SCHEMA_VERSION
            || loaf.plan.intent != receipt.intent
            || !loaf.compatibility.authorizes_source_authority(receipt)?
            || !registry_source_dependencies_supported_by_catalog(&loaf.plan.registry_sources, &registry_dependencies)
        {
            continue;
        }
        candidates.push((loaf.plan.registry_sources.len(), loaf_path));
    }
    candidates.sort();
    let Some((_, loaf_path)) = candidates.into_iter().next() else {
        return Ok(None);
    };
    source_authority_loaf_from_loaf_with_lock(receipt, &loaf_path, Some(generation_lock)).map(Some)
}

/// Select one receipt-compatible compiler-owned Loaf under a shared generation lock.
///
/// Exact identity, provider-superset compatibility, registry-catalog admission, and deterministic tie-breaking live
/// here once. Materializing into a bounded caller store and consuming immutable toolchain data are downstream
/// ownership choices; neither may reinterpret compatibility.
fn select_toolchain_loaf(
    receipt: &OvenReceipt,
    selection: OvenLoafSelection,
    dependencies: &[DependencySpec],
    registry_requirement: OvenLoafRegistryRequirement,
) -> Result<Option<OvenToolchainLoaf>, OvenLoafError> {
    let registry_dependencies = dependencies
        .iter()
        .filter(|dependency| matches!(dependency.source, DependencySource::Registry))
        .collect::<Vec<_>>();
    let loaf_root = crate::toolchain_layout::resolve_toolchain_data_path(Path::new(TOOLCHAIN_LOAF_RELATIVE_ROOT));
    if !loaf_root.join("envelope.json").is_file() {
        return Ok(None);
    }
    let generation_lock = acquire_loaf_generation_lock(&loaf_root)?;
    if let Some(loaf_path) = exact_committed_loaf_path(&loaf_root, receipt)? {
        let native = loaf_from_loaf_with_lock(receipt, &loaf_path, OvenLoafSelection::Exact, Some(generation_lock))?;
        let supported = match registry_requirement {
            OvenLoafRegistryRequirement::LinkableLeaf => {
                registry_dependencies_supported_by_loaf(&native, &registry_dependencies, &receipt.intent.profile)
            }
        };
        return Ok(supported.then_some(native));
    }
    if selection == OvenLoafSelection::Exact {
        return Ok(None);
    }

    let candidates = compatible_loaf_paths(&loaf_root, receipt)?;
    let mut supported = Vec::new();
    for candidate in candidates {
        let native = loaf_from_loaf(
            receipt,
            &candidate.path,
            OvenLoafSelection::CompilerOwnedProviderSuperset,
        )?;
        let candidate_supported = match registry_requirement {
            OvenLoafRegistryRequirement::LinkableLeaf => {
                registry_dependencies_supported_by_loaf(&native, &registry_dependencies, &receipt.intent.profile)
            }
        };
        if candidate_supported {
            supported.push(candidate);
        }
    }
    let Some(candidate) = select_most_specific_compatible_loaf(supported) else {
        return Ok(None);
    };
    loaf_from_loaf_with_lock(
        receipt,
        &candidate.path,
        OvenLoafSelection::CompilerOwnedProviderSuperset,
        Some(generation_lock),
    )
    .map(Some)
}

/// Return every compiler-owned loaf that authorizes the narrow runtime-provider subset rule.
fn compatible_loaf_paths(loaf_root: &Path, receipt: &OvenReceipt) -> Result<Vec<CompatibleLoaf>, OvenLoafError> {
    let mut candidates = Vec::new();
    for loaf_path in committed_loaf_metadata_paths_for_authority(loaf_root, OvenLoafMemberRole::CompiledClosure)? {
        let loaf = read_loaf(&loaf_path)?;
        if loaf.schema_version != OVEN_LOAF_SCHEMA_VERSION || loaf.build_unit_identity == receipt.build_unit_identity {
            continue;
        }
        if loaf.plan.intent == receipt.intent
            && let Some(excess) = loaf.compatibility.provider_subset_excess(receipt)?
        {
            candidates.push(CompatibleLoaf {
                path: loaf_path,
                excess,
            });
        }
    }
    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(candidates)
}

/// Select the narrowest compatible compiler-owned loaf, with a path tie-breaker for reproducibility.
///
/// Every candidate has already matched all runtime inputs and contains every requested provider module/facet. The
/// tie-breaker therefore cannot widen the authority of the request; it only prevents independent shipped provider
/// families from making a no-provider request fail arbitrarily.
fn select_most_specific_compatible_loaf(mut candidates: Vec<CompatibleLoaf>) -> Option<CompatibleLoaf> {
    candidates.sort_by(|left, right| left.excess.cmp(&right.excess).then_with(|| left.path.cmp(&right.path)));
    candidates.into_iter().next()
}

/// Select the narrowest compatible Loaf after satisfying any caller-visible registry requirement.
///
/// A registry-free caller has no catalog predicate to prove, so it must not validate every compatible immutable
/// closure merely to evaluate an empty conjunction. The final caller validates only the selected Loaf before Rustc
/// receives any artifact path.
#[cfg(test)]
fn select_compatible_loaf_with_registry_requirement(
    candidates: Vec<CompatibleLoaf>,
    registry_dependencies: &[&DependencySpec],
    mut supports_registry_dependencies: impl FnMut(&CompatibleLoaf) -> Result<bool, OvenLoafError>,
) -> Result<Option<CompatibleLoaf>, OvenLoafError> {
    if registry_dependencies.is_empty() {
        return Ok(select_most_specific_compatible_loaf(candidates));
    }
    let mut supported = Vec::new();
    for candidate in candidates {
        if supports_registry_dependencies(&candidate)? {
            supported.push(candidate);
        }
    }
    Ok(select_most_specific_compatible_loaf(supported))
}

/// Verify that one loaf authorizes `receipt` and resolve its compiler-owned direct-Rustc closure.
fn loaf_from_loaf(
    receipt: &OvenReceipt,
    loaf_path: &Path,
    selection: OvenLoafSelection,
) -> Result<OvenToolchainLoaf, OvenLoafError> {
    loaf_from_loaf_with_lock(receipt, loaf_path, selection, None)
}

/// Resolve one receipt-authorized Loaf while retaining an optional generation-lifetime lock.
fn loaf_from_loaf_with_lock(
    receipt: &OvenReceipt,
    loaf_path: &Path,
    selection: OvenLoafSelection,
    generation_lock: Option<OvenLoafGenerationLock>,
) -> Result<OvenToolchainLoaf, OvenLoafError> {
    receipt.verify_identity().map_err(|error| OvenLoafError::InvalidLoaf {
        path: loaf_path.to_path_buf(),
        message: format!("requested receipt is invalid: {error}"),
    })?;
    let loaf = read_loaf(loaf_path)?;
    if loaf.schema_version != OVEN_LOAF_SCHEMA_VERSION {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: format!(
                "schema version {} is unsupported (expected {})",
                loaf.schema_version, OVEN_LOAF_SCHEMA_VERSION
            ),
        });
    }
    let exact_identity = loaf.build_unit_identity == receipt.build_unit_identity;
    let compatible_provider_subset = selection == OvenLoafSelection::CompilerOwnedProviderSuperset
        && loaf.compatibility.authorizes_provider_subset(receipt)?;
    if !exact_identity && !compatible_provider_subset {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "build-unit identity does not authorize the requested receipt or provider-subset runtime"
                .to_string(),
        });
    }
    if loaf.plan.intent != receipt.intent {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "direct-rustc intent does not authorize the requested receipt".to_string(),
        });
    }
    if loaf.plan.registry_leaves != loaf.registry_leaves {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "loaf registry catalog does not match its copied direct-rustc plan".to_string(),
        });
    }
    validate_registry_leaf_catalog(&loaf, loaf_path)?;
    validate_loaf_declared_file_set(&loaf, loaf_path)?;
    let artifact_root = loaf_path.parent().ok_or_else(|| OvenLoafError::InvalidLoaf {
        path: loaf_path.to_path_buf(),
        message: "loaf file has no parent directory".to_string(),
    })?;
    let artifact_plan = loaf.plan.materialize_trusted_store(artifact_root, &receipt.intent)?;
    Ok(OvenToolchainLoaf {
        loaf_build_unit_identity: loaf.build_unit_identity,
        artifacts: loaf.plan,
        registry_leaves: loaf.registry_leaves,
        artifact_root: artifact_root.to_path_buf(),
        artifact_plan,
        _generation_lock: generation_lock,
    })
}

/// Resolve a receipt-compatible source-authority Loaf without treating its catalog as a linkable closure.
///
/// The envelope has already restricted this path to a source-authority member. This second verification makes that
/// role meaningful at the trust boundary: source inspection accepts exact runtime provenance and intent, while
/// direct-`rustc` callers must still use [`loaf_from_loaf_with_lock`] and its provider/leaf compatibility checks.
fn source_authority_loaf_from_loaf_with_lock(
    receipt: &OvenReceipt,
    loaf_path: &Path,
    generation_lock: Option<OvenLoafGenerationLock>,
) -> Result<OvenToolchainLoaf, OvenLoafError> {
    receipt.verify_identity().map_err(|error| OvenLoafError::InvalidLoaf {
        path: loaf_path.to_path_buf(),
        message: format!("requested receipt is invalid: {error}"),
    })?;
    let loaf = read_loaf(loaf_path)?;
    if loaf.schema_version != OVEN_LOAF_SCHEMA_VERSION {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: format!(
                "schema version {} is unsupported (expected {})",
                loaf.schema_version, OVEN_LOAF_SCHEMA_VERSION
            ),
        });
    }
    if !loaf.compatibility.authorizes_source_authority(receipt)? {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "source authority does not authorize the requested runtime provenance".to_string(),
        });
    }
    if loaf.plan.intent != receipt.intent {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "source-authority direct-rustc intent does not authorize the requested receipt".to_string(),
        });
    }
    if loaf.plan.registry_leaves != loaf.registry_leaves {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "source-authority registry catalog does not match its copied direct-rustc plan".to_string(),
        });
    }
    validate_registry_leaf_catalog(&loaf, loaf_path)?;
    validate_loaf_declared_file_set(&loaf, loaf_path)?;
    let artifact_root = loaf_path.parent().ok_or_else(|| OvenLoafError::InvalidLoaf {
        path: loaf_path.to_path_buf(),
        message: "source-authority loaf file has no parent directory".to_string(),
    })?;
    let artifact_plan = loaf.plan.materialize_trusted_store(artifact_root, &receipt.intent)?;
    Ok(OvenToolchainLoaf {
        loaf_build_unit_identity: loaf.build_unit_identity,
        artifacts: loaf.plan,
        registry_leaves: loaf.registry_leaves,
        artifact_root: artifact_root.to_path_buf(),
        artifact_plan,
        _generation_lock: generation_lock,
    })
}

/// Validate a stored Loaf independently of a generated-project receipt.
///
/// The envelope manifest binds the Loaf identity to release-family SDK, toolchain, lock, and fixture evidence.
/// This check then verifies the immutable payload itself, allowing a complete warm baker invocation to avoid
/// rerunning compiler behaviour merely to rediscover an already-bound receipt.
pub(crate) fn validate_stored_loaf(
    loaf_path: &Path,
    expected_build_unit_identity: &str,
) -> Result<OvenLoafPreparation, OvenLoafError> {
    let loaf = read_loaf(loaf_path)?;
    let loaf_identity = validate_loaf_content_address(loaf_path)?;
    validate_loaf_declared_file_set(&loaf, loaf_path)?;
    if loaf.schema_version != OVEN_LOAF_SCHEMA_VERSION {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: format!(
                "schema version {} is unsupported (expected {})",
                loaf.schema_version, OVEN_LOAF_SCHEMA_VERSION
            ),
        });
    }
    if loaf.build_unit_identity != expected_build_unit_identity {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "build-unit identity does not match the envelope manifest".to_string(),
        });
    }
    if loaf.plan.registry_leaves != loaf.registry_leaves {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "loaf registry catalog does not match its copied direct-rustc plan".to_string(),
        });
    }
    validate_registry_leaf_catalog(&loaf, loaf_path)?;
    let artifact_root = loaf_path.parent().ok_or_else(|| OvenLoafError::InvalidLoaf {
        path: loaf_path.to_path_buf(),
        message: "loaf file has no parent directory".to_string(),
    })?;
    loaf.plan.materialize_trusted_store(artifact_root, &loaf.plan.intent)?;
    let plan_identity = digest_bytes(
        &serde_json::to_vec(&loaf.plan).map_err(|error| OvenLoafError::Preparation {
            message: format!("could not encode reused Loaf plan identity: {error}"),
        })?,
    );
    let (logical_bytes, physical_bytes) = loaf_directory_byte_counts(artifact_root)?;
    let manifest_logical_bytes = fs::symlink_metadata(loaf_path)
        .map_err(|source| OvenLoafError::Io {
            path: loaf_path.to_path_buf(),
            source,
        })?
        .len();
    let payload_logical_bytes = logical_bytes.saturating_sub(manifest_logical_bytes);
    if loaf.accounting.payload_logical_bytes != payload_logical_bytes {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: format!(
                "payload accounting declares {} logical bytes but the verified payload contains {payload_logical_bytes}",
                loaf.accounting.payload_logical_bytes
            ),
        });
    }
    Ok(OvenLoafPreparation {
        build_unit_identity: loaf.build_unit_identity,
        loaf_identity,
        plan_identity,
        logical_bytes,
        physical_bytes,
        transient_peak_physical_bytes: 0,
    })
}

/// Validate the small authority surface required for an exact default reuse decision.
///
/// This hashes `loaf.json`, checks its content-addressed directory name and typed identities, and verifies the plan
/// digest already committed by the envelope. It deliberately does not rehash every artifact. Any later selected Loaf
/// still performs full materialization verification while its generation lease is held; operators can likewise use
/// the explicit inspection path for an eager whole-envelope audit.
pub(crate) fn validate_stored_loaf_for_reuse(
    loaf_path: &Path,
    member: &OvenLoafEnvelopeMember,
) -> Result<OvenLoafPreparation, OvenLoafError> {
    let loaf = read_loaf(loaf_path)?;
    let loaf_identity = validate_loaf_content_address(loaf_path)?;
    if loaf_identity != member.loaf_identity {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "content identity does not match the envelope manifest".to_string(),
        });
    }
    if loaf.schema_version != OVEN_LOAF_SCHEMA_VERSION || loaf.build_unit_identity != member.build_unit_identity {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "schema or build-unit identity does not match the envelope manifest".to_string(),
        });
    }
    if loaf.plan.registry_leaves != loaf.registry_leaves {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "loaf registry catalog does not match its copied direct-rustc plan".to_string(),
        });
    }
    let plan_identity = digest_bytes(
        &serde_json::to_vec(&loaf.plan).map_err(|error| OvenLoafError::Preparation {
            message: format!("could not encode reused Loaf plan identity: {error}"),
        })?,
    );
    if plan_identity != member.plan_identity {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "plan identity does not match the envelope manifest".to_string(),
        });
    }
    Ok(OvenLoafPreparation {
        build_unit_identity: loaf.build_unit_identity,
        loaf_identity,
        plan_identity,
        logical_bytes: member.logical_bytes,
        physical_bytes: member.physical_bytes,
        transient_peak_physical_bytes: 0,
    })
}

/// Verify that a Loaf manifest's digest is also the name of its containing `.loaf` directory.
fn validate_loaf_content_address(loaf_path: &Path) -> Result<String, OvenLoafError> {
    let identity = loaf_file_identity(loaf_path)?;
    let expected_name = format!("{}.loaf", identity.strip_prefix("sha256:").unwrap_or(&identity));
    let actual_name = loaf_path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    if actual_name != Some(expected_name.as_str()) {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: format!("content identity is {identity}, but its directory is not named `{expected_name}`"),
        });
    }
    Ok(identity)
}

/// Reject undeclared, missing, non-portable, or symlinked files in one immutable Loaf directory.
fn validate_loaf_declared_file_set(loaf: &OvenLoaf, loaf_path: &Path) -> Result<(), OvenLoafError> {
    let artifact_root = loaf_path.parent().ok_or_else(|| OvenLoafError::InvalidLoaf {
        path: loaf_path.to_path_buf(),
        message: "loaf file has no parent directory".to_string(),
    })?;
    let mut expected = loaf
        .plan
        .declared_artifact_paths()?
        .into_iter()
        .collect::<BTreeSet<_>>();
    expected.insert("loaf.json".to_string());
    let mut pending = vec![artifact_root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(|source| OvenLoafError::Io {
            path: directory.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| OvenLoafError::Io {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| OvenLoafError::Io {
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() {
                return Err(OvenLoafError::InvalidLoaf {
                    path,
                    message: "Loaf contains an undeclared symbolic link".to_string(),
                });
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err(OvenLoafError::InvalidLoaf {
                    path,
                    message: "Loaf contains an unsupported filesystem entry".to_string(),
                });
            }
            let relative = path
                .strip_prefix(artifact_root)
                .ok()
                .and_then(Path::to_str)
                .map(|value| value.replace('\\', "/"))
                .ok_or_else(|| OvenLoafError::InvalidLoaf {
                    path: path.clone(),
                    message: "Loaf contains a non-portable file path".to_string(),
                })?;
            if !expected.remove(&relative) {
                return Err(OvenLoafError::InvalidLoaf {
                    path,
                    message: format!("Loaf contains undeclared file `{relative}`"),
                });
            }
        }
    }
    if let Some(missing) = expected.into_iter().next() {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: format!("Loaf is missing declared file `{missing}`"),
        });
    }
    Ok(())
}

/// Digest one regular `loaf.json` file into its canonical content identity.
fn loaf_file_identity(loaf_path: &Path) -> Result<String, OvenLoafError> {
    fs::read(loaf_path)
        .map(|bytes| digest_bytes(&bytes))
        .map_err(|source| OvenLoafError::Io {
            path: loaf_path.to_path_buf(),
            source,
        })
}

/// Reject registry catalog records that do not describe an artifact already sealed by the Loaf plan.
///
/// The direct-Rustc resolver may select a catalog leaf by package requirement. Binding every leaf to the plan keeps
/// that selection from becoming a second, less constrained artifact channel beside the receipt-owned closure.
fn validate_registry_leaf_catalog(loaf: &OvenLoaf, loaf_path: &Path) -> Result<(), OvenLoafError> {
    let mut plan_artifacts = BTreeMap::new();
    for artifact in loaf
        .plan
        .externs
        .iter()
        .map(|artifact| (&artifact.relative_path, &artifact.digest))
        .chain(
            loaf.plan
                .supporting_artifacts
                .iter()
                .map(|artifact| (&artifact.relative_path, &artifact.digest)),
        )
    {
        if plan_artifacts
            .insert(artifact.0.as_str(), artifact.1.as_str())
            .is_some()
        {
            return Err(OvenLoafError::InvalidLoaf {
                path: loaf_path.to_path_buf(),
                message: format!("direct-rustc plan declares artifact `{}` more than once", artifact.0),
            });
        }
    }

    let mut package_versions = BTreeSet::new();
    for leaf in &loaf.registry_leaves {
        if leaf.package.trim().is_empty() || leaf.version.trim().is_empty() {
            return Err(OvenLoafError::InvalidLoaf {
                path: loaf_path.to_path_buf(),
                message: "registry leaf package and version must not be empty".to_string(),
            });
        }
        if leaf.crate_name.trim().is_empty() || leaf.crate_name != leaf.artifact.crate_name {
            return Err(OvenLoafError::InvalidLoaf {
                path: loaf_path.to_path_buf(),
                message: format!(
                    "registry leaf `{}` `{}` has inconsistent crate identity",
                    leaf.package, leaf.version
                ),
            });
        }
        if !package_versions.insert((leaf.package.as_str(), leaf.version.as_str())) {
            return Err(OvenLoafError::InvalidLoaf {
                path: loaf_path.to_path_buf(),
                message: format!(
                    "registry catalog declares package `{}` version `{}` more than once",
                    leaf.package, leaf.version
                ),
            });
        }
        let mut features = BTreeSet::new();
        for feature in &leaf.features {
            if feature.trim().is_empty() || !features.insert(feature.as_str()) {
                return Err(OvenLoafError::InvalidLoaf {
                    path: loaf_path.to_path_buf(),
                    message: format!(
                        "registry leaf `{}` `{}` declares an empty or duplicate feature",
                        leaf.package, leaf.version
                    ),
                });
            }
        }
        if !leaf.source.registry.starts_with("registry+")
            || leaf.source.checksum.trim().is_empty()
            || leaf.source.digest.trim().is_empty()
        {
            return Err(OvenLoafError::InvalidLoaf {
                path: loaf_path.to_path_buf(),
                message: format!(
                    "registry leaf `{}` `{}` has incomplete registry source identity",
                    leaf.package, leaf.version
                ),
            });
        }
        let source_root = Path::new(&leaf.source.relative_root);
        if source_root.is_absolute()
            || source_root.as_os_str().is_empty()
            || source_root.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(OvenLoafError::InvalidLoaf {
                path: loaf_path.to_path_buf(),
                message: format!(
                    "registry leaf `{}` `{}` has an unsafe source root",
                    leaf.package, leaf.version
                ),
            });
        }
        let source_manifest = source_root.join("Cargo.toml").to_string_lossy().replace('\\', "/");
        if !plan_artifacts.contains_key(source_manifest.as_str()) {
            return Err(OvenLoafError::InvalidLoaf {
                path: loaf_path.to_path_buf(),
                message: format!(
                    "registry leaf `{}` `{}` source root is not declared by the direct-rustc plan",
                    leaf.package, leaf.version
                ),
            });
        }
        if Path::new(&leaf.artifact.relative_path)
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("rlib")
        {
            return Err(OvenLoafError::InvalidLoaf {
                path: loaf_path.to_path_buf(),
                message: format!(
                    "registry leaf `{}` `{}` must reference an rlib",
                    leaf.package, leaf.version
                ),
            });
        }
        match plan_artifacts.get(leaf.artifact.relative_path.as_str()) {
            Some(digest) if *digest == leaf.artifact.digest.as_str() => {}
            Some(_) => {
                return Err(OvenLoafError::InvalidLoaf {
                    path: loaf_path.to_path_buf(),
                    message: format!(
                        "registry leaf `{}` `{}` has a digest that disagrees with its sealed direct-rustc plan artifact",
                        leaf.package, leaf.version
                    ),
                });
            }
            None => {
                return Err(OvenLoafError::InvalidLoaf {
                    path: loaf_path.to_path_buf(),
                    message: format!(
                        "registry leaf `{}` `{}` references `{}`, which the sealed direct-rustc plan does not declare",
                        leaf.package, leaf.version, leaf.artifact.relative_path
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Read one Loaf and attach the source path to any decoding failure.
fn read_loaf(loaf_path: &Path) -> Result<OvenLoaf, OvenLoafError> {
    let bytes = fs::read(loaf_path).map_err(|source| OvenLoafError::Io {
        path: loaf_path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice::<OvenLoaf>(&bytes).map_err(|error| OvenLoafError::InvalidLoaf {
        path: loaf_path.to_path_buf(),
        message: format!("must be valid JSON: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::{
        CompatibleLoaf, OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION, OVEN_LOAF_SCHEMA_VERSION, OvenLoaf,
        OvenLoafCompatibility, OvenLoafEnvelope, OvenLoafEnvelopeManifest, OvenLoafEnvelopeMember, OvenLoafError,
        OvenLoafFixtureAction, OvenLoafMemberRole, OvenLoafSelection, acquire_exclusive_loaf_generation_lock,
        acquire_loaf_generation_lock, committed_loaf_envelope_compatibility_identity, committed_loaf_paths,
        digest_runtime_crate_source, loaf_envelope_inspection_packages, loaf_envelope_specifications, loaf_from_loaf,
        merge_loaf_inspection_sources, registry_source_dependencies_supported_by_catalog, run_bounded_loaf_cargo,
        select_most_specific_compatible_loaf,
    };
    use crate::manifest::{DependencySource, DependencySpec};
    use crate::oven::legacy_cargo::OvenLegacyCargoInspectionSource;
    use crate::oven::rustc::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactExtern, OvenRustcArtifactManifest,
        OvenRustcArtifactPlan, OvenRustcRegistryLeaf, OvenRustcRegistrySource, OvenRustcRegistrySourcePackage,
        OvenRustcSupportingArtifact,
    };
    use crate::oven::{OvenGeneratedProjectRequest, digest_bytes, digest_source_tree, receipt_generated_project};
    use incan_core::lang::stdlib::{self, StdlibExtraCrateSource};

    /// Return the canonical standard-library modules owned by checked SDK component sources.
    ///
    /// Component entrypoints are the source-of-truth provider surface. Normalizing their `*.prelude` implementation
    /// modules to their public facade mirrors provider publication, while `std.interop` is the intentionally
    /// source-less vocabulary-backed provider component.
    fn checked_stdlib_component_modules() -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
        let component_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib/components");
        let mut modules = BTreeSet::from(["std.interop".to_string()]);
        for entry in fs::read_dir(&component_root)? {
            let entry = entry?;
            let source = entry.path().join("src/lib.incn");
            if !source.is_file() {
                continue;
            }
            for line in fs::read_to_string(&source)?.lines() {
                let Some(import) = line.trim().strip_prefix("import ") else {
                    continue;
                };
                let module = import
                    .split_whitespace()
                    .next()
                    .ok_or("stdlib component import has no module path")?;
                let module = match module.strip_suffix(".prelude") {
                    Some(facade) => facade,
                    None => module,
                };
                modules.insert(format!("std.{module}"));
            }
        }
        Ok(modules)
    }

    /// Return the standard-library imports a checked complete-stdlib fixture declares.
    fn checked_stdlib_fixture_imports(source: &str) -> BTreeSet<String> {
        source
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let module = line
                    .strip_prefix("import ")
                    .or_else(|| line.strip_prefix("from "))?
                    .split_whitespace()
                    .next()?;
                module.starts_with("std.").then(|| module.to_string())
            })
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn loaf_capacity_abort_terminates_fake_cargo_descendants() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;
        use std::time::Instant;

        let fixture = tempfile::tempdir()?;
        let capacity_root = fixture.path().join("capacity");
        let capture_root = fixture.path().join("capture");
        let overflow = capacity_root.join("overflow");
        let descendant_pid = fixture.path().join("descendant-pid");
        let cargo = fixture.path().join("cargo");
        fs::create_dir_all(&capacity_root)?;
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\nsleep 30 &\nprintf '%s\\n' \"$!\" > \"{}\"\ndd if=/dev/zero of=\"{}\" bs=8192 count=1 2>/dev/null\nwait\n",
                descendant_pid.display(),
                overflow.display(),
            ),
        )?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;

        let mut command = Command::new(&cargo);
        let started = Instant::now();
        let result = run_bounded_loaf_cargo(&mut command, &[&capacity_root], 4 * 1024, &capture_root, "fake Cargo");
        assert!(matches!(result, Err(OvenLoafError::Preparation { .. })));
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "capacity abort waited for the fake Cargo descendant instead of terminating its process group"
        );
        fs::remove_dir_all(&capacity_root)?;
        fs::remove_dir_all(&capture_root)?;
        assert!(
            !capacity_root.exists(),
            "capacity-aborted Loaf staging was not removable"
        );
        assert!(
            !capture_root.exists(),
            "capacity-aborted Loaf capture output was not removable"
        );
        let pid = fs::read_to_string(descendant_pid)?.trim().parse::<u32>()?;
        for _ in 0..100 {
            if !crate::oven::process::process_is_running(pid)? {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(5));
        }
        Err("Loaf capacity abort left the fake-Cargo descendant running".into())
    }

    #[test]
    fn committed_envelope_ignores_unreferenced_generations_and_rejects_foreign_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let committed_loaf = OvenLoaf {
            schema_version: OVEN_LOAF_SCHEMA_VERSION,
            build_unit_identity: "sha256:one".to_string(),
            provenance: Default::default(),
            accounting: Default::default(),
            compatibility: Default::default(),
            registry_leaves: Vec::new(),
            plan: OvenRustcArtifactManifest {
                schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: crate::oven::OvenBuildIntent {
                    target: "fixture-target".to_string(),
                    toolchain: "fixture-rustc".to_string(),
                    profile: "release".to_string(),
                    features: Vec::new(),
                },
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_externs: BTreeMap::new(),
                registry_leaves: Vec::new(),
                registry_sources: Vec::new(),
                compile_environment: BTreeMap::new(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
        };
        let committed_bytes = serde_json::to_vec_pretty(&committed_loaf)?;
        let committed_identity = digest_bytes(&committed_bytes);
        let committed = PathBuf::from(format!(
            "generations/current/{}.loaf/loaf.json",
            committed_identity
                .strip_prefix("sha256:")
                .unwrap_or(&committed_identity)
        ));
        let stale = root.path().join("generations/stale/two.loaf/loaf.json");
        fs::create_dir_all(root.path().join(committed.parent().ok_or("committed parent missing")?))?;
        fs::create_dir_all(stale.parent().ok_or("stale parent missing")?)?;
        fs::write(root.path().join(&committed), &committed_bytes)?;
        fs::write(&stale, "{}")?;
        fs::write(
            root.path().join("envelope.json"),
            serde_json::to_vec(&OvenLoafEnvelopeManifest {
                schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
                envelope: "release".to_string(),
                generation_identity: "sha256:current".to_string(),
                evidence: BTreeMap::new(),
                loafs: vec![OvenLoafEnvelopeMember {
                    label: "one".to_string(),
                    profile: "release".to_string(),
                    action: "build".to_string(),
                    role: OvenLoafMemberRole::CompiledClosure,
                    build_unit_identity: "sha256:one".to_string(),
                    loaf_identity: committed_identity,
                    plan_identity: digest_bytes(&serde_json::to_vec(&committed_loaf.plan)?),
                    logical_bytes: committed_bytes.len() as u64,
                    physical_bytes: 0,
                    path: committed.clone(),
                }],
            })?,
        )?;
        assert_eq!(committed_loaf_paths(root.path())?, vec![root.path().join(&committed)]);

        fs::write(root.path().join(&committed), "{\"mutated\":true}")?;
        assert!(matches!(
            committed_loaf_paths(root.path()),
            Err(OvenLoafError::InvalidLoaf { .. })
        ));
        fs::write(root.path().join(&committed), &committed_bytes)?;
        let extra = root
            .path()
            .join(committed.parent().ok_or("committed parent missing")?)
            .join("undeclared.bin");
        fs::write(&extra, "undeclared")?;
        assert!(matches!(
            committed_loaf_paths(root.path()),
            Err(OvenLoafError::InvalidLoaf { .. })
        ));
        assert_eq!(
            super::committed_loaf_metadata_paths(root.path())?,
            vec![root.path().join(&committed)]
        );
        fs::remove_file(extra)?;

        let mut manifest: OvenLoafEnvelopeManifest =
            serde_json::from_slice(&fs::read(root.path().join("envelope.json"))?)?;
        manifest.loafs[0].path = PathBuf::from("../foreign.loaf/loaf.json");
        fs::write(root.path().join("envelope.json"), serde_json::to_vec(&manifest)?)?;
        assert!(matches!(
            committed_loaf_paths(root.path()),
            Err(OvenLoafError::InvalidLoaf { .. })
        ));
        Ok(())
    }

    #[test]
    fn committed_envelope_compatibility_tracks_members_not_generation_evidence()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let first_member = OvenLoafEnvelopeMember {
            label: "foundation-debug".to_string(),
            profile: "debug".to_string(),
            action: "run".to_string(),
            role: OvenLoafMemberRole::CompiledClosure,
            build_unit_identity: digest_bytes(b"foundation-build-unit"),
            loaf_identity: digest_bytes(b"foundation-loaf"),
            plan_identity: digest_bytes(b"foundation-plan"),
            logical_bytes: 1,
            physical_bytes: 1,
            path: PathBuf::from("generations/first/foundation.loaf/loaf.json"),
        };
        let write_manifest =
            |generation_identity: String, compiler_evidence: String, member: OvenLoafEnvelopeMember| {
                fs::write(
                    root.path().join("envelope.json"),
                    serde_json::to_vec(&OvenLoafEnvelopeManifest {
                        schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
                        envelope: "compiler-suite".to_string(),
                        generation_identity,
                        evidence: BTreeMap::from([("compiler_executable_digest".to_string(), compiler_evidence)]),
                        loafs: vec![member],
                    })?,
                )
            };

        write_manifest(
            digest_bytes(b"generation-one"),
            digest_bytes(b"compiler-one"),
            first_member.clone(),
        )?;
        let first = committed_loaf_envelope_compatibility_identity(root.path(), "compiler-suite")?;

        write_manifest(
            digest_bytes(b"generation-two"),
            digest_bytes(b"compiler-two"),
            first_member.clone(),
        )?;
        assert_eq!(
            committed_loaf_envelope_compatibility_identity(root.path(), "compiler-suite")?,
            first,
            "a changed compiler executable may require a new envelope generation but must not rebuild unchanged members"
        );

        let mut changed_member = first_member;
        changed_member.plan_identity = digest_bytes(b"changed-foundation-plan");
        write_manifest(
            digest_bytes(b"generation-three"),
            digest_bytes(b"compiler-three"),
            changed_member,
        )?;
        assert_ne!(
            committed_loaf_envelope_compatibility_identity(root.path(), "compiler-suite")?,
            first,
            "a changed sealed member plan must invalidate compiler-suite reuse"
        );
        Ok(())
    }

    #[test]
    fn active_generation_reader_blocks_replacement_until_release() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let exclusive = acquire_exclusive_loaf_generation_lock(root.path())?;
        drop(exclusive);
        let reader = acquire_loaf_generation_lock(root.path())?;
        let path = root.path().to_path_buf();
        let (sender, receiver) = mpsc::channel();
        let replacement = thread::spawn(move || {
            let lock = acquire_exclusive_loaf_generation_lock(&path);
            sender.send(lock.is_ok()).ok();
            lock
        });
        assert!(receiver.recv_timeout(Duration::from_millis(50)).is_err());
        drop(reader);
        assert!(receiver.recv_timeout(Duration::from_secs(2))?);
        drop(replacement.join().map_err(|_| "replacement thread panicked")??);
        Ok(())
    }

    #[test]
    fn built_in_envelopes_are_checked_complete_and_unambiguous() {
        for envelope in [OvenLoafEnvelope::Release, OvenLoafEnvelope::CompilerSuite] {
            let specifications = loaf_envelope_specifications(envelope);
            assert_eq!(specifications.len(), 2);
            let identities = specifications
                .iter()
                .map(|specification| (specification.label, specification.profile))
                .collect::<BTreeSet<_>>();
            assert_eq!(identities.len(), specifications.len());
            assert_eq!(identities, BTreeSet::from([("stdlib", "debug"), ("stdlib", "release")]));
            for specification in specifications {
                assert!(!specification.source.trim().is_empty());
                assert!(!specification.manifest.trim().is_empty());
                assert!(!specification.inspection_manifest.trim().is_empty());
                assert!(matches!(specification.profile, "debug" | "release"));
                assert!(
                    specification.profile != "debug" || specification.action != OvenLoafFixtureAction::Build,
                    "a debug Loaf fixture must use `run` so the canonical receipt records debug intent"
                );
                assert!(specification.manifest.contains(specification.project_name));
                assert_eq!(
                    specification.role,
                    OvenLoafMemberRole::CompiledClosureAndSourceAuthority
                );
                assert!(specification.retain_complete_registry_leaves);
                assert!(
                    specification.retain_checked_direct_dependencies,
                    "every `stdlib` Loaf must directly link the complete checked standard-library dependency surface"
                );
                for required_module in [
                    "std.async.channel",
                    "std.compression.zstd",
                    "std.datetime",
                    "std.datetime.runtime",
                    "std.encoding.base64",
                    "std.fs",
                    "std.interop",
                    "std.result",
                    "std.serde",
                    "std.telemetry",
                    "std.traits.callable",
                    "std.web",
                    "std.web.routing",
                ] {
                    assert!(
                        specification.source.contains(required_module),
                        "the full release-version provider Loaf must retain `{required_module}`"
                    );
                }
                for required_facade in ["std.datetime", "std.fs", "std.serde", "std.telemetry", "std.web"] {
                    assert!(
                        specification
                            .source
                            .lines()
                            .any(|line| line.trim() == format!("import {required_facade}")),
                        "the full release-version provider Loaf must activate the public `{required_facade}` facade"
                    );
                }
                assert!(
                    specification.source.contains("@route(\"/oven-loaf-provider\")"),
                    "{envelope:?}/{:?} must exercise the web proc-macro provider rather than merely importing it",
                    specification.label
                );
            }
        }
        let release = loaf_envelope_specifications(OvenLoafEnvelope::Release);
        let source_authority_profiles = release
            .iter()
            .filter(|specification| specification.role.provides_source_authority())
            .map(|specification| specification.profile)
            .collect::<BTreeSet<_>>();
        assert_eq!(source_authority_profiles, BTreeSet::from(["debug", "release"]));
        assert!(
            release
                .iter()
                .all(|specification| specification.role != OvenLoafMemberRole::SourceAuthority)
        );
    }

    #[test]
    fn complete_stdlib_loaf_fixtures_cover_every_checked_component_module() -> Result<(), Box<dyn std::error::Error>> {
        let expected_modules = checked_stdlib_component_modules()?;
        for envelope in [OvenLoafEnvelope::Release, OvenLoafEnvelope::CompilerSuite] {
            for specification in loaf_envelope_specifications(envelope) {
                let fixture_modules = checked_stdlib_fixture_imports(specification.source);
                let missing = expected_modules
                    .difference(&fixture_modules)
                    .cloned()
                    .collect::<Vec<_>>();
                if !missing.is_empty() {
                    return Err(format!(
                        "{envelope:?}/{}/{} complete stdlib fixture omits checked provider modules: {}",
                        specification.label,
                        specification.profile,
                        missing.join(", ")
                    )
                    .into());
                }
            }
        }
        Ok(())
    }

    #[test]
    fn checked_envelope_names_the_complete_declared_repository_test_inspection_surface()
    -> Result<(), Box<dyn std::error::Error>> {
        let specifications = loaf_envelope_specifications(OvenLoafEnvelope::CompilerSuite);
        let packages = loaf_envelope_inspection_packages(OvenLoafEnvelope::CompilerSuite)?;
        assert_eq!(
            loaf_envelope_inspection_packages(OvenLoafEnvelope::Release)?,
            packages,
            "the release and compiler-suite `stdlib` Loafs must declare one identical complete standard-library dependency surface"
        );
        let mut expected_packages = stdlib::extra_crate_deps()
            .filter(|dependency| matches!(dependency.source, StdlibExtraCrateSource::Version(_)))
            .map(|dependency| {
                stdlib::extra_crate_package_alias(dependency.crate_name)
                    .unwrap_or(dependency.crate_name)
                    .to_string()
            })
            .collect::<BTreeSet<_>>();
        expected_packages.extend([
            "bitflags".to_string(),
            "semver".to_string(),
            "serde".to_string(),
            "serde_json".to_string(),
            "uuid".to_string(),
        ]);
        assert_eq!(
            packages
                .iter()
                .map(|package| package.package.clone())
                .collect::<BTreeSet<_>>(),
            expected_packages
        );
        let provider = specifications
            .iter()
            .find(|specification| specification.label == "stdlib" && specification.profile == "debug")
            .ok_or("missing compiler-suite standard-provider Loaf")?;
        assert_eq!(
            provider
                .inspection_packages()?
                .iter()
                .map(|package| package.package.clone())
                .collect::<BTreeSet<_>>(),
            expected_packages
        );
        for imported_crate in ["rand", "uuid"] {
            assert!(
                provider.source.contains(&format!("from rust::{imported_crate}")),
                "the compiler-suite provider Loaf must retain an actual checked source import for `{imported_crate}` so reachable dependency resolution produces its direct-rustc leaf"
            );
        }
        assert!(
            provider.source.contains("std.serde"),
            "the compiler-suite provider Loaf must retain the checked stdlib serde surface that produces its derive-enabled direct-rustc leaf"
        );
        for reachable_use in ["Uuid.new_v4()", "thread_rng()", ".gen_range("] {
            assert!(
                provider.source.contains(reachable_use),
                "the compiler-suite provider Loaf must exercise `{reachable_use}` so its declared raw Rust dependency is usable rather than merely imported"
            );
        }
        assert!(provider.role.provides_source_authority());
        assert!(specifications.iter().all(|specification| {
            specification.role == OvenLoafMemberRole::CompiledClosureAndSourceAuthority
                && specification.retain_complete_registry_leaves
        }));
        Ok(())
    }

    #[test]
    fn envelope_source_authority_is_sealed_without_fabricating_a_linkable_leaf()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = tempfile::tempdir()?;
        fs::create_dir_all(source.path().join("src"))?;
        fs::write(
            source.path().join("Cargo.toml"),
            "[package]\nname = \"blake2\"\nversion = \"0.10.6\"\n",
        )?;
        fs::write(source.path().join("src/lib.rs"), "pub fn sealed() {}\n")?;
        let source_digest = digest_source_tree(source.path())?;
        let authority = OvenLegacyCargoInspectionSource {
            package: "blake2".to_string(),
            version: "0.10.6".to_string(),
            registry: "registry+https://example.invalid/index".to_string(),
            checksum: "blake2-checksum".to_string(),
            features: vec!["derive".to_string(), "std".to_string()],
            source_root: source.path().to_path_buf(),
            source_digest,
        };
        let staging = tempfile::tempdir()?;
        let receipt = runtime_receipt_for_plan()?;
        let mut plan = empty_manifest(&receipt);

        merge_loaf_inspection_sources(&mut plan, staging.path(), std::slice::from_ref(&authority))?;

        assert!(plan.registry_leaves.is_empty());
        assert_eq!(plan.registry_sources.len(), 1);
        assert!(plan.supporting_artifacts.iter().any(|artifact| {
            artifact.relative_path.starts_with("registry-sources/") && artifact.relative_path.ends_with("/Cargo.toml")
        }));

        fs::create_dir_all(staging.path().join("deps"))?;
        let artifact = b"sealed rlib";
        fs::write(staging.path().join("deps/libblake2.rlib"), artifact)?;
        plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: "deps/libblake2.rlib".to_string(),
            digest: digest_bytes(artifact),
        });
        plan.registry_leaves.push(OvenRustcRegistryLeaf {
            package: authority.package.clone(),
            version: authority.version.clone(),
            crate_name: "blake2".to_string(),
            features: vec!["std".to_string()],
            source: plan.registry_sources[0].source.clone(),
            artifact: OvenRustcArtifactExtern {
                crate_name: "blake2".to_string(),
                relative_path: "deps/libblake2.rlib".to_string(),
                digest: digest_bytes(artifact),
            },
        });
        plan.validate_shape(&receipt.intent)?;

        let conflicting = OvenLegacyCargoInspectionSource {
            checksum: "different-checksum".to_string(),
            ..authority
        };
        let error = merge_loaf_inspection_sources(&mut plan, staging.path(), &[conflicting])
            .err()
            .ok_or("conflicting source identity must fail closed")?;
        assert!(
            error
                .to_string()
                .contains("disagrees with the generated-project authority")
        );
        Ok(())
    }

    #[test]
    fn source_selection_does_not_require_a_fabricated_linkable_leaf() -> Result<(), Box<dyn std::error::Error>> {
        let receipt = runtime_receipt_for_plan()?;
        let mut artifacts = empty_manifest(&receipt);
        artifacts.registry_sources.push(OvenRustcRegistrySourcePackage {
            package: "regex".to_string(),
            version: "1.12.3".to_string(),
            features: vec!["perf".to_string(), "std".to_string()],
            source: OvenRustcRegistrySource {
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: "regex-checksum".to_string(),
                relative_root: "registry-sources/regex".to_string(),
                digest: "sha256:regex".to_string(),
            },
        });
        let native = super::OvenToolchainLoaf {
            loaf_build_unit_identity: receipt.build_unit_identity,
            artifacts,
            registry_leaves: Vec::new(),
            artifact_root: PathBuf::from("/sealed-loaf"),
            artifact_plan: OvenRustcArtifactPlan {
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                compile_environment: BTreeMap::new(),
                caller_owned_library_digests: BTreeMap::new(),
            },
            _generation_lock: None,
        };
        let dependency = DependencySpec {
            crate_name: "regex_alias".to_string(),
            version: Some("1".to_string()),
            features: vec!["std".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: Some("regex".to_string()),
        };

        assert!(registry_source_dependencies_supported_by_catalog(
            &native.artifacts.registry_sources,
            &[&dependency]
        ));
        assert!(native.registry_leaves.is_empty());
        Ok(())
    }

    #[test]
    fn runtime_source_digest_matches_the_staged_minimal_runtime_closure() -> Result<(), Box<dyn std::error::Error>> {
        let source = tempfile::tempdir()?;
        fs::create_dir_all(source.path().join("src/nested"))?;
        fs::create_dir_all(source.path().join("target/temporary"))?;
        fs::write(
            source.path().join("Cargo.toml"),
            "[package]\nname = \"runtime\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(source.path().join("src/lib.rs"), "pub mod nested;\n")?;
        fs::write(source.path().join("src/nested/mod.rs"), "pub fn value() {}\n")?;
        fs::write(source.path().join("README.md"), "not a runtime input\n")?;
        fs::write(source.path().join("target/temporary/artifact"), "not a runtime input\n")?;

        let staged = tempfile::tempdir()?;
        fs::create_dir_all(staged.path().join("src/nested"))?;
        for relative in ["Cargo.toml", "src/lib.rs", "src/nested/mod.rs"] {
            fs::copy(source.path().join(relative), staged.path().join(relative))?;
        }

        let digest = digest_runtime_crate_source(source.path())?;
        assert_eq!(digest, digest_source_tree(staged.path())?);

        fs::write(source.path().join("README.md"), "still not a runtime input\n")?;
        assert_eq!(digest_runtime_crate_source(source.path())?, digest);
        fs::write(source.path().join("src/nested/mod.rs"), "pub fn changed() {}\n")?;
        assert_ne!(digest_runtime_crate_source(source.path())?, digest);
        Ok(())
    }

    #[test]
    fn a_toolchain_loaf_serves_clean_project_receipts_without_a_store_copy() -> Result<(), Box<dyn std::error::Error>> {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        let loaf = tempfile::tempdir()?;
        fs::write(first.path().join("main.rs"), "fn main() {}\n")?;
        fs::write(second.path().join("main.rs"), "fn main() { println!(\"second\"); }\n")?;
        let receipt_for = |root: &Path| {
            receipt_generated_project(
                &OvenGeneratedProjectRequest::new(
                    root,
                    "seeded_fixture",
                    "0.1.0",
                    "aarch64-apple-darwin",
                    "rustc seeded-test",
                    "debug",
                    Vec::new(),
                )
                .with_generated_source("generated-root", root.join("main.rs")),
            )
        };
        let first_receipt = receipt_for(first.path())?;
        let second_receipt = receipt_for(second.path())?;
        assert_ne!(first_receipt.identity, second_receipt.identity);
        assert_eq!(first_receipt.build_unit_identity, second_receipt.build_unit_identity);
        let loaf_payload = OvenLoaf {
            schema_version: OVEN_LOAF_SCHEMA_VERSION,
            build_unit_identity: first_receipt.build_unit_identity.clone(),
            provenance: Default::default(),
            accounting: Default::default(),
            compatibility: OvenLoafCompatibility {
                runtime_inputs: BTreeMap::new(),
                providers: Vec::new(),
            },
            registry_leaves: Vec::new(),
            plan: OvenRustcArtifactManifest {
                schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: first_receipt.intent.clone(),
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_externs: BTreeMap::new(),
                registry_leaves: Vec::new(),
                registry_sources: Vec::new(),
                compile_environment: BTreeMap::new(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
        };
        let loaf_path = loaf.path().join("loaf.json");
        fs::write(&loaf_path, serde_json::to_vec(&loaf_payload)?)?;
        let resolved = loaf_from_loaf(&second_receipt, &loaf_path, OvenLoafSelection::Exact)?;
        assert_eq!(resolved.loaf_build_unit_identity, first_receipt.build_unit_identity);
        assert_eq!(resolved.artifact_root, loaf.path());
        assert!(resolved.artifact_plan.externs.is_empty());

        fs::write(loaf.path().join("unsealed-extra.bin"), "not part of the Loaf")?;
        let error = match loaf_from_loaf(&second_receipt, &loaf_path, OvenLoafSelection::Exact) {
            Ok(_) => return Err("selected Loaf with an undeclared file must fail closed".into()),
            Err(error) => error,
        };
        assert!(error.to_string().contains("undeclared file"));
        fs::remove_file(loaf.path().join("unsealed-extra.bin"))?;

        let first_selection = loaf_from_loaf(&first_receipt, &loaf_path, OvenLoafSelection::Exact)?;
        let second_selection = loaf_from_loaf(&second_receipt, &loaf_path, OvenLoafSelection::Exact)?;
        assert_eq!(first_selection.artifact_root, loaf.path());
        assert_eq!(second_selection.artifact_root, loaf.path());
        Ok(())
    }

    #[test]
    fn a_loaf_for_another_build_unit_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let loaf = tempfile::tempdir()?;
        let source = project.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "mismatch_fixture",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc seeded-test",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &source),
        )?;
        let loaf_payload = OvenLoaf {
            schema_version: OVEN_LOAF_SCHEMA_VERSION,
            build_unit_identity: digest_bytes(b"another-unit"),
            provenance: Default::default(),
            accounting: Default::default(),
            compatibility: OvenLoafCompatibility {
                runtime_inputs: BTreeMap::new(),
                providers: Vec::new(),
            },
            registry_leaves: Vec::new(),
            plan: OvenRustcArtifactManifest {
                schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: receipt.intent.clone(),
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_externs: BTreeMap::new(),
                registry_leaves: Vec::new(),
                registry_sources: Vec::new(),
                compile_environment: BTreeMap::new(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
        };
        let loaf_path = loaf.path().join("loaf.json");
        fs::write(&loaf_path, serde_json::to_vec(&loaf_payload)?)?;
        assert!(matches!(
            loaf_from_loaf(&receipt, &loaf_path, OvenLoafSelection::Exact),
            Err(OvenLoafError::InvalidLoaf { .. })
        ));
        Ok(())
    }

    #[test]
    fn native_loaf_rejects_registry_leaves_outside_its_declared_plan() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let loaf_root = tempfile::tempdir()?;
        let source = project.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let receipt = runtime_receipt(&source, "", "fixture-registry", "fixture-stdlib")?;
        let artifact_relative_path = "deps/libfixture.rlib".to_string();
        let artifact_path = loaf_root.path().join(&artifact_relative_path);
        fs::create_dir_all(artifact_path.parent().ok_or("registry artifact parent")?)?;
        let artifact_bytes = b"sealed fixture registry artifact";
        fs::write(&artifact_path, artifact_bytes)?;
        let artifact_digest = digest_bytes(artifact_bytes);
        let registry_source_relative_root = "registry-sources/fixture".to_string();
        let registry_source_root = loaf_root.path().join(&registry_source_relative_root);
        fs::create_dir_all(&registry_source_root)?;
        let registry_manifest_relative_path = format!("{registry_source_relative_root}/Cargo.toml");
        let registry_manifest = b"[package]\nname = \"fixture-registry\"\nversion = \"1.0.0\"\n";
        fs::write(registry_source_root.join("Cargo.toml"), registry_manifest)?;
        let registry_source_digest = digest_source_tree(&registry_source_root)?;
        let mut plan = empty_manifest(&receipt);
        plan.dependency_search_paths = vec!["deps".to_string()];
        plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: artifact_relative_path.clone(),
            digest: artifact_digest.clone(),
        });
        plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: registry_manifest_relative_path,
            digest: digest_bytes(registry_manifest),
        });
        let registry_leaf = OvenRustcRegistryLeaf {
            package: "fixture-registry".to_string(),
            version: "1.0.0".to_string(),
            crate_name: "fixture_registry".to_string(),
            features: vec!["std".to_string()],
            source: OvenRustcRegistrySource {
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: "fixture-checksum".to_string(),
                relative_root: registry_source_relative_root,
                digest: registry_source_digest,
            },
            artifact: OvenRustcArtifactExtern {
                crate_name: "fixture_registry".to_string(),
                relative_path: artifact_relative_path,
                digest: artifact_digest,
            },
        };
        plan.registry_leaves = vec![registry_leaf.clone()];
        plan.registry_sources = vec![OvenRustcRegistrySourcePackage {
            package: registry_leaf.package.clone(),
            version: registry_leaf.version.clone(),
            features: registry_leaf.features.clone(),
            source: registry_leaf.source.clone(),
        }];
        let mut loaf = OvenLoaf {
            schema_version: OVEN_LOAF_SCHEMA_VERSION,
            build_unit_identity: receipt.build_unit_identity.clone(),
            provenance: Default::default(),
            accounting: Default::default(),
            compatibility: OvenLoafCompatibility::default(),
            registry_leaves: vec![registry_leaf],
            plan,
        };
        let loaf_path = loaf_root.path().join("loaf.json");
        fs::write(&loaf_path, serde_json::to_vec(&loaf)?)?;
        let resolved = loaf_from_loaf(&receipt, &loaf_path, OvenLoafSelection::Exact)?;
        assert_eq!(resolved.registry_leaves.len(), 1);

        loaf.registry_leaves[0].artifact.relative_path = "deps/libunsealed.rlib".to_string();
        loaf.plan.registry_leaves = loaf.registry_leaves.clone();
        fs::write(&loaf_path, serde_json::to_vec(&loaf)?)?;
        let error = match loaf_from_loaf(&receipt, &loaf_path, OvenLoafSelection::Exact) {
            Ok(_) => return Err("a registry leaf outside the sealed plan must fail".into()),
            Err(error) => error,
        };
        assert!(matches!(error, OvenLoafError::InvalidLoaf { .. }));
        assert!(error.to_string().contains("sealed direct-rustc plan does not declare"));
        Ok(())
    }

    #[test]
    fn a_standard_testing_loaf_authorizes_the_core_provider_subset() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let loaf_root = tempfile::tempdir()?;
        let source = project.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let core = runtime_receipt(&source, "", "empty-rust-dependencies", "empty-stdlib-features")?;
        let testing = runtime_receipt(
            &source,
            "incan-stdlib|std.testing|testing",
            "empty-rust-dependencies",
            "fs,json,testing",
        )?;
        let unsupported_facet = runtime_receipt(
            &source,
            "incan-stdlib|std.testing|unsupported",
            "empty-rust-dependencies",
            "empty-stdlib-features",
        )?;
        assert_ne!(core.build_unit_identity, testing.build_unit_identity);

        let compatibility = OvenLoafCompatibility::from_receipt(&testing)?;
        assert!(compatibility.authorizes_provider_subset(&core)?);
        assert!(!OvenLoafCompatibility::from_receipt(&core)?.authorizes_provider_subset(&testing)?);
        assert!(!compatibility.authorizes_provider_subset(&unsupported_facet)?);

        let loaf_payload = OvenLoaf {
            schema_version: OVEN_LOAF_SCHEMA_VERSION,
            build_unit_identity: testing.build_unit_identity.clone(),
            provenance: Default::default(),
            accounting: Default::default(),
            compatibility,
            registry_leaves: Vec::new(),
            plan: empty_manifest(&testing),
        };
        let loaf_path = loaf_root.path().join("loaf.json");
        fs::write(&loaf_path, serde_json::to_vec(&loaf_payload)?)?;
        let selected = loaf_from_loaf(&core, &loaf_path, OvenLoafSelection::CompilerOwnedProviderSuperset)?;
        assert_eq!(selected.artifact_root, loaf_root.path());
        Ok(())
    }

    #[test]
    fn a_private_sdk_direct_link_requires_a_loaf_extern_capability() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source = project.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let ordinary = runtime_receipt(&source, "incan_stdlib_data|||none", "direct-link", "no-stdlib-features")?;
        let private_sdk_link =
            runtime_receipt(&source, "incan_stdlib_data|||link", "direct-link", "no-stdlib-features")?;

        let ordinary_compatibility = OvenLoafCompatibility::from_receipt(&ordinary)?;
        let linked_compatibility = OvenLoafCompatibility::from_receipt(&private_sdk_link)?;
        assert!(linked_compatibility.authorizes_provider_subset(&ordinary)?);
        assert!(
            !ordinary_compatibility.authorizes_provider_subset(&private_sdk_link)?,
            "a loaf without the direct SDK rlib cannot authorize a provider's private link root"
        );
        Ok(())
    }

    #[test]
    fn an_interop_execution_receipt_does_not_fragment_compiler_owned_loaf_compatibility()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source = project.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let base = runtime_receipt(
            &source,
            "incan-stdlib|std.interop|ffi",
            "empty-rust-dependencies",
            "interop",
        )?;
        let mut selected_interop = base.clone();
        selected_interop.sources.build_unit_inputs.insert(
            "oven-interop-execution-receipt".to_string(),
            "sha256:selected-package-interop-plan".to_string(),
        );
        selected_interop
            .sources
            .build_unit_inputs
            .insert("oven-interop-plan-schema".to_string(), "2".to_string());

        let compatibility = OvenLoafCompatibility::from_receipt(&base)?;
        assert!(compatibility.authorizes_provider_subset(&selected_interop)?);

        selected_interop
            .sources
            .build_unit_inputs
            .insert("unrelated-compiler-input".to_string(), "changed".to_string());
        assert!(!compatibility.authorizes_provider_subset(&selected_interop)?);
        Ok(())
    }

    #[test]
    fn loaf_selection_prefers_the_narrowest_compatible_provider_loaf() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source = project.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let core = runtime_receipt(&source, "", "empty-rust-dependencies", "empty-stdlib-features")?;
        let encoding = runtime_receipt(
            &source,
            "incan-stdlib|std.encoding.base64|codecs",
            "empty-rust-dependencies",
            "codecs",
        )?;
        let broad = runtime_receipt(
            &source,
            "incan-stdlib|std.async,std.testing|async,testing",
            "empty-rust-dependencies",
            "async,testing",
        )?;

        let encoding_excess = OvenLoafCompatibility::from_receipt(&encoding)?
            .provider_subset_excess(&core)?
            .ok_or("encoding loaf must authorize the core subset")?;
        let broad_excess = OvenLoafCompatibility::from_receipt(&broad)?
            .provider_subset_excess(&core)?
            .ok_or("broad loaf must authorize the core subset")?;
        assert!(encoding_excess < broad_excess);

        let selected = select_most_specific_compatible_loaf(vec![
            CompatibleLoaf {
                path: PathBuf::from("/toolchain/loafs/broad/loaf.json"),
                excess: broad_excess,
            },
            CompatibleLoaf {
                path: PathBuf::from("/toolchain/loafs/encoding/loaf.json"),
                excess: encoding_excess,
            },
        ])
        .ok_or("a compatible loaf must be selected")?;
        assert_eq!(selected.path, PathBuf::from("/toolchain/loafs/encoding/loaf.json"));
        Ok(())
    }

    #[test]
    fn registry_free_selection_skips_compatible_loaf_materialization() -> Result<(), Box<dyn std::error::Error>> {
        let narrow = CompatibleLoaf {
            path: PathBuf::from("/toolchain/loafs/narrow/loaf.json"),
            excess: super::OvenLoafProviderExcess {
                providers: 0,
                modules: 1,
                facets: 1,
                direct_links: 0,
            },
        };
        let broad = CompatibleLoaf {
            path: PathBuf::from("/toolchain/loafs/broad/loaf.json"),
            excess: super::OvenLoafProviderExcess {
                providers: 1,
                modules: 1,
                facets: 1,
                direct_links: 0,
            },
        };
        let materializations = std::cell::Cell::new(0_u8);

        let selected =
            super::select_compatible_loaf_with_registry_requirement(vec![broad, narrow.clone()], &[], |_| {
                materializations.set(materializations.get().saturating_add(1));
                Err(OvenLoafError::Preparation {
                    message: "a registry-free selection must not materialize a candidate".to_string(),
                })
            })?
            .ok_or("a compatible Loaf must be selected")?;

        assert_eq!(materializations.get(), 0);
        assert_eq!(selected, narrow);
        Ok(())
    }

    #[test]
    fn registry_selection_checks_each_compatible_loaf_before_tie_breaking() -> Result<(), Box<dyn std::error::Error>> {
        let narrow = CompatibleLoaf {
            path: PathBuf::from("/toolchain/loafs/narrow/loaf.json"),
            excess: super::OvenLoafProviderExcess {
                providers: 0,
                modules: 1,
                facets: 1,
                direct_links: 0,
            },
        };
        let broad = CompatibleLoaf {
            path: PathBuf::from("/toolchain/loafs/broad/loaf.json"),
            excess: super::OvenLoafProviderExcess {
                providers: 1,
                modules: 1,
                facets: 1,
                direct_links: 0,
            },
        };
        let dependency = DependencySpec {
            crate_name: "fixture_registry".to_string(),
            version: Some("1".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: Some("fixture-registry".to_string()),
        };
        let catalog_checks = std::cell::Cell::new(0_u8);

        let selected = super::select_compatible_loaf_with_registry_requirement(
            vec![narrow, broad.clone()],
            &[&dependency],
            |candidate| {
                catalog_checks.set(catalog_checks.get().saturating_add(1));
                Ok(candidate.path == broad.path)
            },
        )?
        .ok_or("the compatible registry Loaf must be selected")?;

        assert_eq!(catalog_checks.get(), 2);
        assert_eq!(selected, broad);
        Ok(())
    }

    #[test]
    fn a_native_loaf_drops_only_redundant_rmeta_sidecars() -> Result<(), Box<dyn std::error::Error>> {
        let receipt = runtime_receipt_for_plan()?;
        let mut plan = empty_manifest(&receipt);
        plan.supporting_artifacts = vec![
            crate::oven::rustc::OvenRustcSupportingArtifact {
                relative_path: "deps/libruntime.rlib".to_string(),
                digest: digest_bytes(b"runtime"),
            },
            crate::oven::rustc::OvenRustcSupportingArtifact {
                relative_path: "deps/libruntime.rmeta".to_string(),
                digest: digest_bytes(b"metadata"),
            },
            crate::oven::rustc::OvenRustcSupportingArtifact {
                relative_path: "provenance/legacy-cargo.json".to_string(),
                digest: digest_bytes(b"provenance"),
            },
        ];

        super::discard_loaf_metadata_sidecars(&mut plan);

        assert_eq!(
            plan.supporting_artifacts
                .iter()
                .map(|artifact| artifact.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec!["deps/libruntime.rlib", "provenance/legacy-cargo.json"]
        );
        Ok(())
    }

    #[test]
    fn native_loaf_promotes_compiler_runtime_externs_for_compatible_callers() -> Result<(), Box<dyn std::error::Error>>
    {
        let receipt = runtime_receipt_for_plan()?;
        let mut plan = empty_manifest(&receipt);
        plan.entrypoint_externs.insert("generated-root".to_string(), Vec::new());
        plan.supporting_artifacts = vec![
            crate::oven::rustc::OvenRustcSupportingArtifact {
                relative_path: "host/deps/libincan_derive-verified.dylib".to_string(),
                digest: digest_bytes(b"derive macro"),
            },
            crate::oven::rustc::OvenRustcSupportingArtifact {
                relative_path: "target/deps/libincan_core-verified.rlib".to_string(),
                digest: digest_bytes(b"compiler runtime"),
            },
            crate::oven::rustc::OvenRustcSupportingArtifact {
                relative_path: "target/deps/libruntime.rlib".to_string(),
                digest: digest_bytes(b"runtime"),
            },
        ];

        super::promote_compiler_runtime_externs(&mut plan)?;

        assert_eq!(
            plan.externs
                .iter()
                .map(|artifact| artifact.crate_name.as_str())
                .collect::<Vec<_>>(),
            vec!["incan_core", "incan_derive"]
        );
        assert!(plan.externs.iter().any(|artifact| {
            artifact.crate_name == "incan_derive"
                && artifact.relative_path == "host/deps/libincan_derive-verified.dylib"
        }));
        assert!(plan.externs.iter().any(|artifact| {
            artifact.crate_name == "incan_core" && artifact.relative_path == "target/deps/libincan_core-verified.rlib"
        }));
        assert_eq!(
            plan.supporting_artifacts
                .iter()
                .map(|artifact| artifact.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec!["target/deps/libruntime.rlib"]
        );
        assert_eq!(
            plan.entrypoint_externs.get("generated-root"),
            Some(&vec!["incan_core".to_string(), "incan_derive".to_string()])
        );
        Ok(())
    }

    #[test]
    fn native_loaf_keeps_compiler_vocab_helpers_off_generated_root() -> Result<(), Box<dyn std::error::Error>> {
        let receipt = runtime_receipt_for_plan()?;
        let mut plan = empty_manifest(&receipt);
        plan.externs.push(crate::oven::rustc::OvenRustcArtifactExtern {
            crate_name: "incan_stdlib".to_string(),
            relative_path: "target/deps/libincan_stdlib-verified.rlib".to_string(),
            digest: digest_bytes(b"stdlib runtime"),
        });
        plan.supporting_artifacts = vec![
            crate::oven::rustc::OvenRustcSupportingArtifact {
                relative_path: "host/deps/libincan_derive-verified.dylib".to_string(),
                digest: digest_bytes(b"derive macro"),
            },
            crate::oven::rustc::OvenRustcSupportingArtifact {
                relative_path: "target/deps/libincan_core-verified.rlib".to_string(),
                digest: digest_bytes(b"compiler runtime"),
            },
        ];

        super::record_generated_root_externs(&mut plan);
        super::promote_compiler_runtime_externs(&mut plan)?;
        plan.externs.extend([
            crate::oven::rustc::OvenRustcArtifactExtern {
                crate_name: "incan_vocab".to_string(),
                relative_path: "compiler-support/deps/libincan_vocab-verified.rlib".to_string(),
                digest: digest_bytes(b"compiler vocabulary"),
            },
            crate::oven::rustc::OvenRustcArtifactExtern {
                crate_name: "serde_json".to_string(),
                relative_path: "compiler-support/deps/libserde_json-verified.rlib".to_string(),
                digest: digest_bytes(b"compiler json"),
            },
        ]);
        plan.externs
            .sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
        plan.validate_shape(&receipt.intent)?;

        assert_eq!(
            plan.entrypoint_externs.get("generated-root"),
            Some(&vec![
                "incan_core".to_string(),
                "incan_derive".to_string(),
                "incan_stdlib".to_string(),
            ])
        );
        Ok(())
    }

    #[test]
    fn native_loaf_accounting_measures_the_final_loaf_directory() -> Result<(), Box<dyn std::error::Error>> {
        let loaf = tempfile::tempdir()?;
        fs::write(loaf.path().join("loaf.json"), b"plan")?;
        fs::create_dir(loaf.path().join("artifacts"))?;
        fs::write(loaf.path().join("artifacts/runtime.rlib"), b"runtime")?;

        let (logical_bytes, physical_bytes) = super::loaf_directory_byte_counts(loaf.path())?;

        assert_eq!(logical_bytes, 11);
        assert!(physical_bytes >= logical_bytes);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn raw_disk_accounting_counts_hard_linked_payload_once() -> Result<(), Box<dyn std::error::Error>> {
        let loaf = tempfile::tempdir()?;
        let original = loaf.path().join("original.rlib");
        let linked = loaf.path().join("linked.rlib");
        fs::write(&original, vec![0_u8; 8192])?;
        fs::hard_link(&original, &linked)?;

        let directory_bytes = super::loaf_file_physical_bytes(&fs::symlink_metadata(loaf.path())?);
        let payload_bytes = super::loaf_file_physical_bytes(&fs::symlink_metadata(&original)?);
        let raw_disk_bytes = super::loaf_raw_disk_bytes(loaf.path())?;

        assert_eq!(raw_disk_bytes, directory_bytes.saturating_add(payload_bytes));
        assert!(raw_disk_bytes < directory_bytes.saturating_add(payload_bytes.saturating_mul(2)));
        Ok(())
    }

    #[test]
    fn native_vocab_loaf_copies_only_cargo_reported_artifact_closure() -> Result<(), Box<dyn std::error::Error>> {
        let publisher = tempfile::tempdir()?;
        let target_deps = publisher.path().join("target/deps");
        let host_deps = publisher.path().join("host/deps");
        fs::create_dir_all(&target_deps)?;
        fs::create_dir_all(&host_deps)?;
        let reported = [
            ("serde_json", "libserde_json-reported.rlib", b"json".as_slice()),
            (
                "required_transitive",
                "librequired_transitive-reported.rlib",
                b"transitive".as_slice(),
            ),
        ];
        for (_, name, contents) in reported {
            fs::write(target_deps.join(name), contents)?;
        }
        let unreported = target_deps.join("libunrelated_cargo_residue.rlib");
        fs::write(&unreported, b"unreported")?;
        // Cargo reports both the hashed `deps` input and an unhashed convenience
        // copy at the profile root. The latter is publisher output, not a
        // direct-rustc input, and must not expand the sealed loaf closure.
        let profile_copy = publisher.path().join("target/libincan_vocab.rlib");
        fs::write(&profile_copy, b"profile copy")?;
        let profile_copy_canonical = fs::canonicalize(&profile_copy)?;
        let mut cargo_output = reported
            .iter()
            .map(|(crate_name, name, _)| {
                serde_json::json!({
                    "reason": "compiler-artifact",
                    "target": { "name": crate_name },
                    "filenames": [target_deps.join(name).display().to_string()],
                })
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        cargo_output.push('\n');
        cargo_output.push_str(
            &serde_json::json!({
                "reason": "compiler-artifact",
                "target": { "name": "incan_vocab" },
                "filenames": [profile_copy.display().to_string()],
            })
            .to_string(),
        );
        let artifacts = super::compiler_artifact_paths_from_cargo_output(
            cargo_output.as_bytes(),
            publisher.path(),
            &[target_deps.as_path(), host_deps.as_path()],
            "incan_vocab",
            publisher.path().join("target").as_path(),
            "native vocabulary fixture",
        )?;
        let loaf = tempfile::tempdir()?;
        let receipt = runtime_receipt_for_plan()?;
        let mut plan = empty_manifest(&receipt);
        super::copy_compiler_vocab_support_artifacts(
            &artifacts,
            &target_deps,
            &publisher.path().join("target"),
            &host_deps,
            &loaf.path().join("deps"),
            &mut plan,
        )?;

        assert!(!loaf.path().join("deps/libunrelated_cargo_residue.rlib").exists());
        assert!(
            artifacts.iter().any(|artifact| artifact == &profile_copy_canonical),
            "the named publisher's reported profile-root rlib must enter the direct-rustc closure"
        );
        assert!(
            plan.externs.is_empty(),
            "compiler-only vocab roots must not become program externs"
        );
        let host_support = plan
            .vocab_auxiliary_targets
            .iter()
            .find(|target| target.target == receipt.intent.target)
            .ok_or("missing host compiler vocabulary auxiliary closure")?;
        assert_eq!(
            host_support
                .externs
                .iter()
                .map(|artifact| artifact.crate_name.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["incan_vocab", "serde_json"]),
            "the host vocabulary helper must retain its exact direct-Rustc roots outside the normal program plan"
        );
        assert!(
            plan.supporting_artifacts
                .iter()
                .any(|artifact| artifact.relative_path.ends_with("librequired_transitive-reported.rlib"))
        );
        assert!(
            !plan
                .supporting_artifacts
                .iter()
                .any(|artifact| artifact.relative_path.ends_with("libunrelated_cargo_residue.rlib"))
        );
        Ok(())
    }

    #[test]
    fn native_vocab_loaf_rejects_unexpected_profile_root_compiler_artifact() -> Result<(), Box<dyn std::error::Error>> {
        let publisher = tempfile::tempdir()?;
        let target_deps = publisher.path().join("target/deps");
        let host_deps = publisher.path().join("host/deps");
        fs::create_dir_all(&target_deps)?;
        fs::create_dir_all(&host_deps)?;
        let unexpected = publisher.path().join("target/libunrelated.rlib");
        fs::write(&unexpected, b"must not become a sealed input")?;
        let cargo_output = serde_json::json!({
            "reason": "compiler-artifact",
            "target": { "name": "unrelated" },
            "filenames": [unexpected.display().to_string()],
        })
        .to_string();

        let error = match super::compiler_artifact_paths_from_cargo_output(
            cargo_output.as_bytes(),
            publisher.path(),
            &[target_deps.as_path(), host_deps.as_path()],
            "incan_vocab",
            publisher.path().join("target").as_path(),
            "native vocabulary fixture",
        ) {
            Ok(_) => return Err("unexpected profile-root artifact must fail closed".into()),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("escaped its declared dependency directories")
        );
        Ok(())
    }

    fn runtime_receipt(
        source: &Path,
        providers: &str,
        rust_dependencies: &str,
        stdlib_features: &str,
    ) -> Result<crate::oven::OvenReceipt, Box<dyn std::error::Error>> {
        let provider_plan = digest_bytes(providers.as_bytes());
        let mut request = OvenGeneratedProjectRequest::new(
            source.parent().ok_or("source has no parent")?,
            "runtime_fixture",
            "0.1.0",
            "aarch64-apple-darwin",
            "rustc seeded-test",
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", source)
        .with_build_unit_input("runtime-lock", "runtime-lock")
        .with_build_unit_input("rust-dependencies", rust_dependencies)
        .with_build_unit_input("stdlib-features", stdlib_features)
        .with_build_unit_input("provider-plan", provider_plan);
        if !providers.is_empty() {
            request = request.with_build_unit_input("providers", providers);
        }
        Ok(receipt_generated_project(&request)?)
    }

    fn runtime_receipt_for_plan() -> Result<crate::oven::OvenReceipt, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        // The receipt owns no filesystem path, so retaining only its value is valid after this helper drops the
        // temporary source tree.
        runtime_receipt(&source, "", "empty-rust-dependencies", "empty-stdlib-features")
    }

    fn empty_manifest(receipt: &crate::oven::OvenReceipt) -> OvenRustcArtifactManifest {
        OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        }
    }
}
