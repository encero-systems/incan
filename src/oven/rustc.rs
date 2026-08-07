//! Direct `rustc` execution for Oven Alpha's explicitly supported consumer envelope.
//!
//! The executor accepts a verified artifact manifest instead of scanning Cargo output or reproducing Cargo planning.
//! An explicit publisher-side `legacy_cargo` step may create the declared inputs, but this consumer path invokes only
//! the selected Rust compiler and refuses hidden Cargo state.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use super::process::{isolate_process_group, terminate_process_group};
use super::{OVEN_COMPILER_TEST_PROFILE, OvenBuildIntent, OvenReceipt, digest_bytes, digest_source_tree};
use crate::manifest::{DependencySource, DependencySpec};
use crate::oven::store::{OvenArtifactKind, OvenStore, OvenStoreError, OvenStoreExecutionPayload, OvenStoreLease};

/// Wire-format version for an Oven-owned direct-rustc artifact manifest.
/// Version 7 retains the exact registry-leaf catalog alongside the copied direct-rustc closure, so a normal
/// consumer never combines one leaf's transitive metadata with a different compatibility domain. Older payloads
/// are intentionally ignored during selection and re-materialized from the active toolchain Loaf.
pub const OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION: u32 = 7;
/// Schema version for caller-owned native-output reuse evidence.
const OVEN_DIRECT_RUSTC_OUTPUT_RECEIPT_SCHEMA_VERSION: u32 = 2;

/// One exact dependency artifact permitted in an Oven direct-rustc invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenRustcArtifactExtern {
    /// Crate name passed to `rustc --extern`.
    pub crate_name: String,
    /// Safe path relative to the immutable artifact root.
    pub relative_path: String,
    /// SHA-256 digest of the exact selected artifact bytes.
    pub digest: String,
}

/// One non-root artifact reachable through a declared dependency or native search directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenRustcSupportingArtifact {
    /// Safe path relative to the immutable artifact root.
    pub relative_path: String,
    /// SHA-256 digest of the exact selected artifact bytes.
    pub digest: String,
}

/// One compiler-owned cross-target Rust closure reserved for vocabulary desugarer packaging.
///
/// Normal Oven commands can compile caller-owned vocabulary companions to this target, but they cannot add targets
/// or select arbitrary dependency artifacts. The named publisher has already resolved, copied, and digested every
/// file named here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenRustcAuxiliaryTarget {
    /// Rust target triple for this isolated closure.
    pub target: String,
    /// Safe artifact-root-relative directories passed as `-L dependency` for this target only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependency_search_paths: Vec<String>,
    /// Exact target-specific dependency roots passed through `--extern`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub externs: Vec<OvenRustcArtifactExtern>,
}

/// Immutable direct-rustc dependency plan for one Oven compatibility domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenRustcArtifactManifest {
    /// Manifest wire-schema version.
    pub schema_version: u32,
    /// Target/toolchain/profile/features that own the selected artifacts.
    pub intent: OvenBuildIntent,
    /// Safe artifact-root-relative directories passed as `-L dependency`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependency_search_paths: Vec<String>,
    /// Safe artifact-root-relative directories passed as `-L native`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_search_paths: Vec<String>,
    /// Exact root dependency artifacts passed through `--extern`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub externs: Vec<OvenRustcArtifactExtern>,
    /// Exact root extern names authorized for each receipt source-evidence key.
    ///
    /// A package library test and its CLI can share one immutable dependency closure while requiring different direct
    /// `--extern` roots. The selected list prevents one target's test-only direct dependency from overriding another
    /// target's metadata-selected transitive dependency instance.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entrypoint_externs: BTreeMap<String, Vec<String>>,
    /// Exact registry package artifacts whose metadata closure was emitted with this immutable plan.
    ///
    /// The Loaf repeats this catalog for human inspection, while this copy travels with every bounded
    /// store entry so a selected plan resolves caller `rust::` imports only from its own compatibility domain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub registry_leaves: Vec<OvenRustcRegistryLeaf>,
    /// Deterministic compile-time environment explicitly required by the source closure.
    ///
    /// Ambient `CARGO_*` values are still removed before every consumer invocation. The only permitted replacements
    /// are package metadata captured by the publisher and `@oven-source-root`, which resolves to the caller-owned
    /// root derived from the receipt-authorized source path so compatible clean worktrees do not inherit another
    /// checkout's location.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub compile_environment: BTreeMap<String, String>,
    /// Compiler-owned target closures used only while packaging vocabulary Wasm desugarers.
    ///
    /// These are deliberately separate from the normal host plan: placing cross-target artifacts on every caller
    /// compilation's dependency search path would make Rustc's artifact selection target-ambiguous.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vocab_auxiliary_targets: Vec<OvenRustcAuxiliaryTarget>,
    /// Every other regular artifact reachable through declared search paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supporting_artifacts: Vec<OvenRustcSupportingArtifact>,
}

/// Materialized, content-verified inputs ready for a direct `rustc` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenRustcArtifactPlan {
    /// Verified directories passed as `-L dependency`.
    pub dependency_search_paths: Vec<PathBuf>,
    /// Verified directories passed as `-L native`.
    pub native_search_paths: Vec<PathBuf>,
    /// Verified root dependency artifacts passed as `--extern`.
    pub externs: Vec<(String, PathBuf)>,
    /// Verified compiler-owned environment replacements applied only while compiling the caller-owned source.
    pub compile_environment: BTreeMap<String, String>,
    /// Digests of caller-owned direct-Rustc libraries linked in addition to immutable Oven artifacts.
    ///
    /// A compiler-suite receipt covers its workspace source closure, but output reuse also records these exact
    /// library bytes so a changed intermediate cannot be mistaken for the previously linked root.
    pub(crate) caller_owned_library_digests: BTreeMap<String, String>,
}

/// Materialized compiler-owned cross-target closure available only to vocabulary extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenRustcAuxiliaryTargetPlan {
    pub(crate) dependency_search_paths: Vec<PathBuf>,
    pub(crate) externs: Vec<(String, PathBuf)>,
}

/// One caller-owned library or procedural-macro artifact made by an earlier Oven direct-Rustc materialization step.
///
/// This is deliberately an internal compiler-suite bridge, not an artifact-store entry. The library remains below
/// the caller's output root and carries its already-verified digest into the later output's reuse identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenCallerOwnedRustcLibrary {
    /// Rust crate name used by a consuming direct-Rustc root's `--extern` flag.
    pub crate_name: String,
    /// Regular caller-owned `.rlib` or dynamic procedural-macro output from the earlier direct-Rustc step.
    pub output: PathBuf,
    /// Digest established when Oven baked or revalidated this caller-owned output in the current execution.
    ///
    /// The output is created below this invocation's caller-owned directory, then its bytes are read once before it
    /// becomes a dependency. Later consumers reuse that verified identity rather than re-reading the same large
    /// workspace library for every downstream target.
    pub digest: String,
    /// Whether this output is a direct dependency of the current compilation root.
    ///
    /// Transitive caller-owned outputs still need a verified `-L dependency` search path and reuse evidence, but
    /// must not be presented as arbitrary public `--extern` names to the final consumer. That preserves the
    /// provider's receipt-authorized visibility graph instead of flattening it into the consuming package.
    pub expose_extern: bool,
}

/// One publisher-sealed registry package artifact that a Loaf may expose to a direct-Rustc consumer.
///
/// The catalog records an exact package version and the artifact Cargo emitted while the named publisher prepared the
/// Loaf. It is not a general resolver: a consumer may select only one compatible record already copied into
/// the immutable Loaf closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenRustcRegistryLeaf {
    /// Registry package name rather than a caller-local dependency alias.
    pub package: String,
    /// Exact publisher-resolved package version.
    pub version: String,
    /// Crate identifier encoded in the sealed Rust artifact metadata.
    pub crate_name: String,
    /// Publisher-resolved Cargo features compiled into this exact immutable artifact.
    ///
    /// A consumer may request only a subset. This represents the already unified Loaf closure; it does not
    /// run a feature resolver or permit a consumer to add a feature absent from the sealed leaf.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Digest-verified compiler artifact retained below the Loaf root.
    pub artifact: OvenRustcArtifactExtern,
}

/// One registry leaf and the immutable Loaf root that seals its relative artifact path.
#[derive(Debug, Clone)]
struct OvenRegistryLeafAuthorityEntry {
    artifact_root: PathBuf,
    leaf: OvenRustcRegistryLeaf,
    /// Already materialized immutable directories allowed to satisfy this leaf's transitive Rust metadata.
    ///
    /// A direct `--extern` names the immediate leaf, but Rustc still has to load the exact dependency artifacts
    /// recorded in that leaf's metadata. These directories come only from the same verified artifact plan that
    /// sealed the catalog; this is not an ambient Cargo search path.
    dependency_search_paths: Vec<PathBuf>,
}

/// Immutable registry-leaf authority supplied by receipt-compatible Loafs.
///
/// A caller receives this only from compiler Loafs that independently authorize the same receipt or the suite
/// scheduler's leased copies of those Loafs. Each entry retains its own root, so a narrow code plan may use a
/// registry leaf sealed by another compatible unit without treating either catalog as a Cargo-home, registry-index,
/// or download fallback.
#[derive(Debug, Clone)]
pub(crate) struct OvenRegistryLeafAuthority {
    entries: Vec<OvenRegistryLeafAuthorityEntry>,
}

impl OvenRegistryLeafAuthority {
    #[cfg(test)]
    #[must_use]
    /// Construct test-only authority without transitive metadata search directories.
    pub(crate) fn new(artifact_root: PathBuf, leaves: Vec<OvenRustcRegistryLeaf>) -> Self {
        Self::new_with_trusted_dependency_search_paths(artifact_root, leaves, Vec::new())
    }

    /// Construct a catalog whose transitive Rust metadata may be located only in already verified plan paths.
    #[must_use]
    pub(crate) fn new_with_trusted_dependency_search_paths(
        artifact_root: PathBuf,
        leaves: Vec<OvenRustcRegistryLeaf>,
        dependency_search_paths: Vec<PathBuf>,
    ) -> Self {
        Self {
            entries: leaves
                .into_iter()
                .map(|leaf| OvenRegistryLeafAuthorityEntry {
                    artifact_root: artifact_root.clone(),
                    leaf,
                    dependency_search_paths: dependency_search_paths.clone(),
                })
                .collect(),
        }
    }

    #[cfg(test)]
    #[must_use]
    /// Join test catalogs without granting production consumers cross-domain registry selection.
    pub(crate) fn aggregate(authorities: impl IntoIterator<Item = Self>) -> Self {
        Self {
            entries: authorities
                .into_iter()
                .flat_map(|authority| authority.entries)
                .collect(),
        }
    }
}

/// One digest-verified registry leaf plus the plan directories Rustc may use solely for its transitive metadata.
#[derive(Debug)]
struct ResolvedSealedRegistryLeaf {
    artifact: PathBuf,
    dependency_search_paths: Vec<PathBuf>,
}

/// Exact scheduler-selected path artifacts that a caller-owned direct-Rustc closure may reuse.
///
/// This authority is deliberately narrower than a path allowlist. A dependency must both reside below a
/// scheduler-owned immutable root and use a crate name that the selected plan already exposes. The artifact and
/// metadata search paths then come from that plan, rather than from the dependency's Cargo manifest or an ambient
/// Cargo target directory. This permits a caller-owned library to link a compiler-runtime dependency such as
/// `incan_stdlib` without re-materializing that runtime crate or interpreting its Cargo feature table.
#[derive(Debug, Clone)]
pub(crate) struct OvenSelectedPathRustcAuthority {
    owned_roots: Vec<PathBuf>,
    externs: BTreeMap<String, PathBuf>,
    dependency_search_paths: Vec<PathBuf>,
}

impl OvenSelectedPathRustcAuthority {
    /// Construct the authority from a scheduler-selected, already verified direct-Rustc plan.
    #[must_use]
    pub(crate) fn new(owned_roots: &[PathBuf], artifact_plan: &OvenRustcArtifactPlan) -> Self {
        let mut owned_roots = owned_roots.to_vec();
        owned_roots.sort();
        owned_roots.dedup();
        let mut dependency_search_paths = artifact_plan.dependency_search_paths.clone();
        dependency_search_paths.sort();
        dependency_search_paths.dedup();
        Self {
            owned_roots,
            externs: artifact_plan.externs.iter().cloned().collect(),
            dependency_search_paths,
        }
    }

    /// Return the selected artifact only for an exact compiler-runtime dependency under a leased scheduler root.
    fn resolve(&self, dependency: &DependencySpec) -> Option<PathBuf> {
        let DependencySource::Path { path } = &dependency.source else {
            return None;
        };
        let path = fs::canonicalize(path).ok()?;
        if !self.owned_roots.iter().any(|root| path.starts_with(root)) {
            return None;
        }
        self.externs.get(&dependency.crate_name.replace('-', "_")).cloned()
    }

    /// Return only the verified dependency directories paired with the selected externs.
    fn dependency_search_paths(&self) -> &[PathBuf] {
        &self.dependency_search_paths
    }

    /// Prefer an equivalent sealed registry artifact already present in this selected plan.
    ///
    /// A compatible Loaf catalog may live in the read-only toolchain envelope while the normal command has copied
    /// the same direct-Rustc closure into its actively leased Oven store. Linking the catalog copy as an additional
    /// `--extern` would expose Rustc to two physical copies of one StableCrateId. The caller has already validated
    /// the package, version, features, and digest against the sealed catalog; this method merely reuses the same
    /// metadata-bearing artifact name in one of the selected plan's verified dependency directories. Cargo can emit
    /// byte-distinct rlibs for the same compilation identity when separate publishers retain different non-semantic
    /// payload details; the sealed leaf resolver uses the same filename criterion when choosing equivalent catalog
    /// copies.
    fn matching_sealed_registry_artifact(&self, sealed_artifact: &Path) -> Option<PathBuf> {
        let filename = sealed_artifact.file_name()?;
        let mut matches = self
            .dependency_search_paths
            .iter()
            .filter_map(|directory| {
                let candidate = verified_regular_file(&directory.join(filename), "selected registry artifact").ok()?;
                Some(candidate)
            })
            .collect::<Vec<_>>();
        matches.sort();
        matches.dedup();
        matches.into_iter().next()
    }
}

/// Compile declared narrow Rust library closures with direct `rustc`, never with Cargo.
///
/// This bounded caller-dependency seam builds manifest-declared local path libraries and links registry leaves only
/// from a selected immutable Loaf catalog. It recursively follows only path-to-path edges and incorporates
/// each child output digest into its parent output identity. Git, optional/feature-driven roots, build scripts, and
/// unsealed registry closures remain explicit unsupported inputs.
#[cfg(test)]
pub(crate) fn materialize_declared_rust_libraries(
    output_root: &Path,
    rustc: &Path,
    target: &str,
    profile: &str,
    dependencies: &[DependencySpec],
    registry_authority: Option<&OvenRegistryLeafAuthority>,
) -> Result<Vec<OvenCallerOwnedRustcLibrary>, OvenRustcError> {
    materialize_declared_rust_libraries_with_selected_path_authority(
        output_root,
        rustc,
        target,
        profile,
        dependencies,
        registry_authority,
        None,
    )
}

/// Materialize caller-owned libraries while allowing a scheduler-selected path dependency to remain plan-owned.
///
/// Ordinary callers pass no selected-path authority and therefore keep the conservative manifest-shaped behavior.
/// The compiler-suite scheduler alone supplies this additional authority after leasing the immutable data roots.
pub(crate) fn materialize_declared_rust_libraries_with_selected_path_authority(
    output_root: &Path,
    rustc: &Path,
    target: &str,
    profile: &str,
    dependencies: &[DependencySpec],
    registry_authority: Option<&OvenRegistryLeafAuthority>,
    selected_path_authority: Option<&OvenSelectedPathRustcAuthority>,
) -> Result<Vec<OvenCallerOwnedRustcLibrary>, OvenRustcError> {
    if dependencies.is_empty() {
        return Ok(Vec::new());
    }
    fs::create_dir_all(output_root).map_err(|source| OvenRustcError::Io {
        path: output_root.to_path_buf(),
        source,
    })?;
    let mut state = PathRustcMaterializationState::default();
    let mut names = BTreeSet::new();
    let mut dependencies = dependencies.to_vec();
    dependencies.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    for dependency in dependencies {
        let crate_name = direct_rustc_crate_name(&dependency.crate_name)?;
        if !names.insert(crate_name.clone()) {
            return Err(OvenRustcError::InvalidInput {
                field: "Oven direct-rustc Rust dependency",
                message: format!("declares duplicate crate `{crate_name}`"),
            });
        }
        if dependency.optional {
            return Err(OvenRustcError::InvalidInput {
                field: "Oven direct-rustc Rust dependency",
                message: format!(
                    "`{}` is optional; prepare an explicit Oven-native closure",
                    dependency.crate_name
                ),
            });
        }
        if !matches!(dependency.source, DependencySource::Registry) && !dependency.features.is_empty() {
            return Err(OvenRustcError::InvalidInput {
                field: "Oven direct-rustc Rust dependency",
                message: format!(
                    "`{}` explicitly enables path-package Cargo features; prepare an explicit Oven-native closure",
                    dependency.crate_name
                ),
            });
        }
        if matches!(dependency.source, DependencySource::Registry) && !dependency.default_features {
            return Err(OvenRustcError::InvalidInput {
                field: "Oven direct-rustc Rust dependency",
                message: format!(
                    "`{}` disables registry default features; prepare an explicit Oven-native closure",
                    dependency.crate_name
                ),
            });
        }
        let package_root = match &dependency.source {
            DependencySource::Path { path } => {
                if selected_path_authority
                    .and_then(|authority| authority.resolve(&dependency))
                    .is_some()
                {
                    // The final direct-Rustc plan already attaches this selected compiler-runtime extern. Keeping it
                    // out of caller-owned outputs avoids a second `--extern` with the same name.
                    continue;
                }
                path.clone()
            }
            DependencySource::Registry => {
                let sealed = resolve_sealed_registry_leaf(&dependency, registry_authority, profile)?;
                let output = selected_path_authority
                    .and_then(|authority| authority.matching_sealed_registry_artifact(&sealed))
                    .unwrap_or(sealed);
                let digest = digest_regular_file(&output, "sealed registry Rust dependency")?;
                state.record_extern(crate_name, output, digest)?;
                continue;
            }
            DependencySource::Git { .. } => {
                return Err(OvenRustcError::InvalidInput {
                    field: "Oven direct-rustc Rust dependency",
                    message: format!(
                        "`{}` is a Git package; prepare an explicit Oven-native closure",
                        dependency.crate_name
                    ),
                });
            }
        };
        let output = materialize_path_rust_library(
            &package_root,
            output_root,
            rustc,
            target,
            profile,
            registry_authority,
            selected_path_authority,
            &dependency,
            &mut state,
        )?;
        let digest = digest_regular_file(&output, "materialized path Rust library")?;
        state.record_extern(crate_name, output, digest)?;
    }
    Ok(state
        .externs
        .into_iter()
        .map(|(crate_name, (output, digest))| OvenCallerOwnedRustcLibrary {
            crate_name,
            output,
            digest,
            expose_extern: true,
        })
        .collect())
}

/// Resolve one registry dependency from the selected Loaf's sealed catalog.
fn resolve_sealed_registry_leaf(
    dependency: &DependencySpec,
    authority: Option<&OvenRegistryLeafAuthority>,
    profile: &str,
) -> Result<PathBuf, OvenRustcError> {
    Ok(resolve_sealed_registry_leaf_with_search_paths(dependency, authority, profile)?.artifact)
}

/// Verify that a registry dependency already represented by a selected direct-Rustc extern is semantically valid.
///
/// A matching Rust crate name is not sufficient authority: the caller's package, version, and features still have to
/// match one digest-verified registry leaf from a receipt-compatible native catalog. This validates that contract
/// without compiling a duplicate caller-owned `--extern` or consulting Cargo state.
pub(crate) fn validate_sealed_registry_leaf(
    dependency: &DependencySpec,
    authority: Option<&OvenRegistryLeafAuthority>,
    profile: &str,
) -> Result<(), OvenRustcError> {
    let _ = select_sealed_registry_leaf(dependency, authority, profile)?;
    Ok(())
}

/// Select one semantically compatible sealed registry leaf without re-reading its artifact bytes.
///
/// The caller either uses this to validate a selected direct-Rustc extern that the immutable plan has already checked,
/// or [`resolve_sealed_registry_leaf_with_search_paths`] below to verify and attach a new caller-owned leaf.
fn select_sealed_registry_leaf<'a>(
    dependency: &DependencySpec,
    authority: Option<&'a OvenRegistryLeafAuthority>,
    profile: &str,
) -> Result<&'a OvenRegistryLeafAuthorityEntry, OvenRustcError> {
    let authority = authority.ok_or_else(|| OvenRustcError::InvalidInput {
        field: "Oven registry Rust dependency",
        message: format!(
            "`{}` has no receipt-bound Loaf registry catalog; prepare an explicit Oven-native closure",
            dependency.package.as_deref().unwrap_or(&dependency.crate_name)
        ),
    })?;
    let package_name = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
    let requirement_text = dependency
        .version
        .as_deref()
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!("`{package_name}` has no declared version requirement"),
        })?;
    let requirement = VersionReq::parse(requirement_text).map_err(|error| OvenRustcError::InvalidInput {
        field: "Oven registry Rust dependency",
        message: format!("`{package_name}` has invalid version requirement `{requirement_text}`: {error}"),
    })?;
    let requested_features = dependency.features.iter().collect::<BTreeSet<_>>();
    let mut candidates = authority
        .entries
        .iter()
        .filter_map(|entry| {
            let leaf = &entry.leaf;
            if leaf.package != package_name {
                return None;
            }
            let version = Version::parse(&leaf.version).ok()?;
            let available_features = leaf.features.iter().collect::<BTreeSet<_>>();
            (requirement.matches(&version) && requested_features.is_subset(&available_features))
                .then_some((version, entry))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|(left_version, left), (right_version, right)| {
        right_version
            .cmp(left_version)
            .then_with(|| left.leaf.artifact.relative_path.cmp(&right.leaf.artifact.relative_path))
            .then_with(|| left.artifact_root.cmp(&right.artifact_root))
    });
    // A suite ships separate debug and release Loaf catalogs. Prefer the matching profile whenever its sealed
    // catalog contains this dependency; fixtures use short synthetic paths, so retain the complete catalog when no
    // profile-qualified artifact exists.
    let profile_marker = format!("/{profile}/deps/");
    if candidates
        .iter()
        .any(|(_, entry)| entry.leaf.artifact.relative_path.contains(&profile_marker))
    {
        candidates.retain(|(_, entry)| entry.leaf.artifact.relative_path.contains(&profile_marker));
    }
    let Some((selected_version, selected)) = candidates.first().map(|(version, entry)| (version.clone(), *entry))
    else {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "`{package_name}` requirement `{requirement_text}` has no compatible receipt-bound Loaf registry leaf; prepare an explicit Oven-native closure"
            ),
        });
    };
    let selected_artifact_name = Path::new(&selected.leaf.artifact.relative_path)
        .file_name()
        .and_then(|name| name.to_str());
    let same_compilation = candidates
        .iter()
        .filter(|(version, _)| version == &selected_version)
        .all(|(_, entry)| {
            entry.leaf.crate_name == selected.leaf.crate_name
                && Path::new(&entry.leaf.artifact.relative_path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    == selected_artifact_name
        });
    if !same_compilation {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "`{package_name}` version `{selected_version}` resolves to multiple receipt-bound Loaf registry leaves; prepare an explicit Oven-native closure"
            ),
        });
    }
    Ok(selected)
}

/// Resolve a registry leaf and the pre-verified search directory required to load its transitive metadata.
fn resolve_sealed_registry_leaf_with_search_paths(
    dependency: &DependencySpec,
    authority: Option<&OvenRegistryLeafAuthority>,
    profile: &str,
) -> Result<ResolvedSealedRegistryLeaf, OvenRustcError> {
    let selected = select_sealed_registry_leaf(dependency, authority, profile)?;
    let package_name = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
    // A Cargo publisher can retain byte-distinct copies of one logical crate across independently sealed native
    // units. Rustc's metadata-bearing artifact name is the compilation identity available at this boundary. Once
    // the requested profile, crate name, version, features, and artifact name agree, choose the canonical sorted
    // copy rather than turning equivalent receipt-bound copies into a false ambiguity. A different artifact name
    // remains fail-closed above.
    let artifact = &selected.leaf.artifact;
    let artifact_path = safe_artifact_path(&selected.artifact_root, &artifact.relative_path, "registry leaf")?;
    let bytes = fs::read(&artifact_path).map_err(|source| OvenRustcError::Io {
        path: artifact_path.clone(),
        source,
    })?;
    if digest_bytes(&bytes) != artifact.digest {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "sealed registry leaf `{package_name}` at {} failed digest verification",
                artifact_path.display()
            ),
        });
    }
    let extension = artifact_path.extension().and_then(|extension| extension.to_str());
    if extension != Some("rlib") {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "sealed registry leaf `{package_name}` at {} is not an rlib",
                artifact_path.display()
            ),
        });
    }
    let artifact_parent = artifact_path.parent().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "Oven registry Rust dependency",
        message: format!(
            "sealed registry leaf `{package_name}` at {} has no dependency directory",
            artifact_path.display()
        ),
    })?;
    let mut dependency_search_paths = selected
        .dependency_search_paths
        .iter()
        .map(|path| canonical_directory(path, "registry leaf dependency search path"))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|path| path == artifact_parent)
        .collect::<Vec<_>>();
    dependency_search_paths.sort();
    dependency_search_paths.dedup();
    if !selected.dependency_search_paths.is_empty() && dependency_search_paths.is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "sealed registry leaf `{package_name}` at {} is outside its verified dependency search paths",
                artifact_path.display()
            ),
        });
    }
    Ok(ResolvedSealedRegistryLeaf {
        artifact: artifact_path,
        dependency_search_paths,
    })
}

/// Resolve one manifest-recorded Loaf artifact without allowing the leaf catalog to escape its sealed root.
fn safe_artifact_path(
    artifact_root: &Path,
    relative_path: &str,
    kind: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    let normalized = normalized_relative_path(relative_path, kind)?;
    let root = canonical_directory(artifact_root, "registry leaf artifact root")?;
    let artifact = verified_regular_file(&root.join(normalized), kind)?;
    if !artifact.starts_with(&root) {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!("{kind} artifact {} escapes its sealed root", artifact.display()),
        });
    }
    Ok(artifact)
}

#[derive(Default)]
struct PathRustcMaterializationState {
    outputs: BTreeMap<PathBuf, PathBuf>,
    externs: BTreeMap<String, (PathBuf, String)>,
    active: BTreeSet<PathBuf>,
}

impl PathRustcMaterializationState {
    /// Retain every direct dependency under the name its Rust source uses.
    ///
    /// Rust metadata for an outer rlib names its local path dependencies too, so the final consumer must receive
    /// their explicit `--extern` bindings as well as the top-level import. A name may repeat only when it resolves
    /// to the exact same already-materialized artifact.
    fn record_extern(&mut self, crate_name: String, output: PathBuf, digest: String) -> Result<(), OvenRustcError> {
        match self.externs.get(&crate_name) {
            Some((existing_output, existing_digest)) if existing_output == &output && existing_digest == &digest => {
                Ok(())
            }
            Some((existing_output, _)) => Err(OvenRustcError::InvalidInput {
                field: "path Rust dependency",
                message: format!(
                    "resolves Rust crate `{crate_name}` to both {} and {}",
                    existing_output.display(),
                    output.display()
                ),
            }),
            None => {
                self.externs.insert(crate_name, (output, digest));
                Ok(())
            }
        }
    }
}

/// Compile one Cargo-manifest-shaped local Rust library using only explicitly supplied direct artifacts.
#[allow(clippy::too_many_arguments)]
fn materialize_path_rust_library(
    package_root: &Path,
    output_root: &Path,
    rustc: &Path,
    target: &str,
    profile: &str,
    registry_authority: Option<&OvenRegistryLeafAuthority>,
    selected_path_authority: Option<&OvenSelectedPathRustcAuthority>,
    requested_dependency: &DependencySpec,
    state: &mut PathRustcMaterializationState,
) -> Result<PathBuf, OvenRustcError> {
    let package_root = canonical_directory(package_root, "path Rust dependency")?;
    if let Some(output) = state.outputs.get(&package_root) {
        return Ok(output.clone());
    }
    if !state.active.insert(package_root.clone()) {
        return Err(OvenRustcError::InvalidInput {
            field: "path Rust dependency",
            message: format!("contains a cyclic path dependency at {}", package_root.display()),
        });
    }
    let result = materialize_path_rust_library_inner(
        &package_root,
        output_root,
        rustc,
        target,
        profile,
        registry_authority,
        selected_path_authority,
        requested_dependency,
        state,
    );
    state.active.remove(&package_root);
    let output = result?;
    state.outputs.insert(package_root, output.clone());
    Ok(output)
}

/// Materialize one manifest-backed path library after its caller has established cycle and output ownership state.
#[allow(clippy::too_many_arguments)]
fn materialize_path_rust_library_inner(
    package_root: &Path,
    output_root: &Path,
    rustc: &Path,
    target: &str,
    profile: &str,
    registry_authority: Option<&OvenRegistryLeafAuthority>,
    selected_path_authority: Option<&OvenSelectedPathRustcAuthority>,
    requested_dependency: &DependencySpec,
    state: &mut PathRustcMaterializationState,
) -> Result<PathBuf, OvenRustcError> {
    let manifest_path = package_root.join("Cargo.toml");
    let manifest_bytes = fs::read(&manifest_path).map_err(|source| OvenRustcError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest_text = std::str::from_utf8(&manifest_bytes).map_err(|error| OvenRustcError::InvalidInput {
        field: "path Rust dependency Cargo.toml",
        message: format!("{} is not UTF-8: {error}", manifest_path.display()),
    })?;
    let manifest = toml::from_str::<toml::Value>(manifest_text).map_err(|error| OvenRustcError::InvalidInput {
        field: "path Rust dependency Cargo.toml",
        message: format!("{} is invalid TOML: {error}", manifest_path.display()),
    })?;
    if manifest.get("target").is_some() {
        return Err(OvenRustcError::InvalidInput {
            field: "path Rust dependency Cargo.toml",
            message: format!(
                "{} declares target-conditional dependencies; prepare an explicit Oven-native closure",
                manifest_path.display()
            ),
        });
    }
    let package =
        manifest
            .get("package")
            .and_then(toml::Value::as_table)
            .ok_or_else(|| OvenRustcError::InvalidInput {
                field: "path Rust dependency Cargo.toml",
                message: format!("{} has no [package] table", manifest_path.display()),
            })?;
    let package_name =
        package
            .get("name")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| OvenRustcError::InvalidInput {
                field: "path Rust dependency Cargo.toml",
                message: format!("{} has no package name", manifest_path.display()),
            })?;
    let lib = manifest.get("lib").and_then(toml::Value::as_table);
    let is_proc_macro = lib
        .and_then(|lib| lib.get("proc-macro"))
        .map(|value| {
            value.as_bool().ok_or_else(|| OvenRustcError::InvalidInput {
                field: "path Rust dependency Cargo.toml",
                message: format!(
                    "{} has a non-boolean lib.proc-macro declaration",
                    manifest_path.display()
                ),
            })
        })
        .transpose()?
        .unwrap_or(false);
    if lib
        .and_then(|lib| lib.get("crate-type"))
        .and_then(toml::Value::as_array)
        .is_some_and(|types| {
            types.iter().any(|kind| {
                !(matches!(kind.as_str(), Some("lib" | "rlib"))
                    || (is_proc_macro && matches!(kind.as_str(), Some("proc-macro"))))
            })
        })
    {
        return Err(OvenRustcError::InvalidInput {
            field: "path Rust dependency Cargo.toml",
            message: format!(
                "{} requests an unsupported crate type; prepare an explicit Oven-native closure",
                manifest_path.display()
            ),
        });
    }
    let crate_name = direct_rustc_crate_name(
        lib.and_then(|lib| lib.get("name"))
            .and_then(toml::Value::as_str)
            .unwrap_or(package_name),
    )?;
    let source_relative = lib
        .and_then(|lib| lib.get("path"))
        .and_then(toml::Value::as_str)
        .unwrap_or("src/lib.rs");
    let source = package_root.join(source_relative);
    let source = verified_regular_file(&source, "path Rust library source")?;
    let edition = package.get("edition").and_then(toml::Value::as_str).unwrap_or("2018");
    if !matches!(edition, "2018" | "2021" | "2024") {
        return Err(OvenRustcError::InvalidInput {
            field: "path Rust dependency Cargo.toml",
            message: format!("{} has unsupported edition `{edition}`", manifest_path.display()),
        });
    }
    if manifest.get("build").is_some() || package.get("build").is_some_and(|build| build.as_bool() != Some(false)) {
        return Err(OvenRustcError::InvalidInput {
            field: "path Rust dependency Cargo.toml",
            message: format!(
                "{} declares a build script; prepare an explicit Oven-native closure",
                manifest_path.display()
            ),
        });
    }
    validate_inactive_path_dependency_features(&manifest_path, &manifest, requested_dependency)?;

    let mut child_dependencies = Vec::new();
    let mut child_dependency_search_paths = BTreeSet::new();
    if let Some(dependencies) = manifest.get("dependencies").and_then(toml::Value::as_table) {
        for (name, specification) in dependencies {
            let Some(dependency) = path_rust_manifest_dependency(&manifest_path, package_root, name, specification)?
            else {
                continue;
            };
            let selected_path_artifact = selected_path_authority.and_then(|authority| authority.resolve(&dependency));
            let output = match &dependency.source {
                DependencySource::Path { path } => {
                    if let Some(output) = selected_path_artifact.clone() {
                        if let Some(authority) = selected_path_authority {
                            child_dependency_search_paths.extend(authority.dependency_search_paths().iter().cloned());
                        }
                        output
                    } else {
                        materialize_path_rust_library(
                            path,
                            output_root,
                            rustc,
                            target,
                            profile,
                            registry_authority,
                            selected_path_authority,
                            &dependency,
                            state,
                        )?
                    }
                }
                DependencySource::Registry => {
                    let resolved =
                        resolve_sealed_registry_leaf_with_search_paths(&dependency, registry_authority, profile)?;
                    if let Some(output) = selected_path_authority
                        .and_then(|authority| authority.matching_sealed_registry_artifact(&resolved.artifact))
                    {
                        if let Some(authority) = selected_path_authority {
                            child_dependency_search_paths.extend(authority.dependency_search_paths().iter().cloned());
                        }
                        output
                    } else {
                        child_dependency_search_paths.extend(resolved.dependency_search_paths);
                        resolved.artifact
                    }
                }
                DependencySource::Git { .. } => unreachable!("path manifest parser rejects Git dependencies"),
            };
            let child_name = direct_rustc_crate_name(name)?;
            if selected_path_artifact.is_none() {
                let digest = digest_regular_file(&output, "path Rust library dependency")?;
                state.record_extern(child_name.clone(), output.clone(), digest)?;
            }
            child_dependencies.push((child_name, output));
        }
    }
    child_dependencies.sort_by(|left, right| left.0.cmp(&right.0));

    let source_digest = digest_source_tree(package_root).map_err(|error| OvenRustcError::InvalidInput {
        field: "path Rust dependency",
        message: error.to_string(),
    })?;
    let child_digest_records = child_dependencies
        .iter()
        .map(|(name, output)| {
            let bytes = fs::read(output).map_err(|source| OvenRustcError::Io {
                path: output.clone(),
                source,
            })?;
            Ok(format!("{name}|{}", digest_bytes(&bytes)))
        })
        .collect::<Result<Vec<_>, OvenRustcError>>()?;
    let toolchain = rustc_identity(rustc)?;
    let identity = digest_bytes(
        format!(
            "{source_digest}\n{target}\n{profile}\n{toolchain}\n{}",
            child_digest_records.join("\n")
        )
        .as_bytes(),
    );
    let output_directory = output_root.join(identity.strip_prefix("sha256:").unwrap_or(identity.as_str()));
    let extension = if is_proc_macro {
        std::env::consts::DLL_SUFFIX
    } else {
        ".rlib"
    };
    let output = output_directory.join(format!("lib{crate_name}{extension}"));
    if output.is_file() {
        return verified_regular_file(&output, "materialized path Rust library");
    }
    fs::create_dir_all(&output_directory).map_err(|source| OvenRustcError::Io {
        path: output_directory.clone(),
        source,
    })?;
    let temporary = output_directory.join(format!(".lib{crate_name}.{}.tmp", std::process::id()));
    let mut command = Command::new(rustc);
    command
        .arg("--target")
        .arg(target)
        .arg(format!("--edition={edition}"))
        .arg("--crate-name")
        .arg(&crate_name)
        .arg("--error-format=json")
        .arg(&source)
        .arg("-o")
        .arg(&temporary);
    if is_proc_macro {
        command.args(["--crate-type", "proc-macro", "--extern", "proc_macro"]);
    } else {
        command.args(["--crate-type", "lib"]);
    }
    apply_oven_profile(&mut command, profile);
    clear_inherited_cargo_environment(&mut command);
    for dependency_search_path in &child_dependency_search_paths {
        command
            .arg("-L")
            .arg(format!("dependency={}", dependency_search_path.display()));
    }
    for (child_name, child_output) in &child_dependencies {
        command
            .arg("--extern")
            .arg(format!("{child_name}={}", child_output.display()));
    }
    let output_result = command.output().map_err(|source_error| OvenRustcError::Io {
        path: rustc.to_path_buf(),
        source: source_error,
    })?;
    if !output_result.status.success() {
        return Err(OvenRustcError::CompilationFailed {
            report: parse_rustc_diagnostics(&output_result.stdout, &output_result.stderr).with_invocation(&command),
        });
    }
    verified_regular_file(&temporary, "materialized path Rust library")?;
    fs::rename(&temporary, &output).map_err(|source| OvenRustcError::Io {
        path: output.clone(),
        source,
    })?;
    Ok(output)
}

/// Parse one unconditional local-manifest dependency into the same receipt-bound direct-Rustc representation used by
/// top-level caller dependencies. Optional dependencies are inactive because activated path features remain rejected;
/// registry dependencies must be satisfied by the supplied sealed authority, never Cargo.
fn path_rust_manifest_dependency(
    manifest_path: &Path,
    package_root: &Path,
    name: &str,
    specification: &toml::Value,
) -> Result<Option<DependencySpec>, OvenRustcError> {
    let invalid = |message: String| OvenRustcError::InvalidInput {
        field: "path Rust dependency Cargo.toml",
        message,
    };
    let dependency = match specification {
        toml::Value::String(version) => DependencySpec {
            crate_name: name.to_string(),
            version: Some(version.clone()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        },
        toml::Value::Table(table) => {
            if table.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                return Err(invalid(format!(
                    "{} dependency `{name}` inherits a Cargo workspace declaration; prepare an explicit Oven-native closure",
                    manifest_path.display()
                )));
            }
            if table.get("git").is_some() {
                return Err(invalid(format!(
                    "{} dependency `{name}` is Git-sourced; prepare an explicit Oven-native closure",
                    manifest_path.display()
                )));
            }
            let optional = table
                .get("optional")
                .map(|value| {
                    value.as_bool().ok_or_else(|| {
                        invalid(format!(
                            "{} dependency `{name}` has a non-boolean optional declaration",
                            manifest_path.display()
                        ))
                    })
                })
                .transpose()?
                .unwrap_or(false);
            if optional {
                return Ok(None);
            }
            let features = table
                .get("features")
                .map(|value| {
                    value
                        .as_array()
                        .ok_or_else(|| {
                            invalid(format!(
                                "{} dependency `{name}` has a non-array feature declaration",
                                manifest_path.display()
                            ))
                        })?
                        .iter()
                        .map(|feature| {
                            feature.as_str().map(str::to_string).ok_or_else(|| {
                                invalid(format!(
                                    "{} dependency `{name}` has a non-string feature",
                                    manifest_path.display()
                                ))
                            })
                        })
                        .collect::<Result<Vec<_>, OvenRustcError>>()
                })
                .transpose()?
                .unwrap_or_default();
            let default_features = table
                .get("default-features")
                .map(|value| {
                    value.as_bool().ok_or_else(|| {
                        invalid(format!(
                            "{} dependency `{name}` has a non-boolean default-features declaration",
                            manifest_path.display()
                        ))
                    })
                })
                .transpose()?
                .unwrap_or(true);
            let package = table
                .get("package")
                .map(|value| {
                    value.as_str().map(str::to_string).ok_or_else(|| {
                        invalid(format!(
                            "{} dependency `{name}` has a non-string package alias",
                            manifest_path.display()
                        ))
                    })
                })
                .transpose()?;
            let version = table
                .get("version")
                .map(|value| {
                    value.as_str().map(str::to_string).ok_or_else(|| {
                        invalid(format!(
                            "{} dependency `{name}` has a non-string version",
                            manifest_path.display()
                        ))
                    })
                })
                .transpose()?;
            let source = match table.get("path") {
                Some(path) => {
                    if !features.is_empty() {
                        return Err(invalid(format!(
                            "{} path dependency `{name}` explicitly enables Cargo features; prepare an explicit Oven-native closure",
                            manifest_path.display()
                        )));
                    }
                    let path = path.as_str().ok_or_else(|| {
                        invalid(format!(
                            "{} dependency `{name}` has a non-string path",
                            manifest_path.display()
                        ))
                    })?;
                    DependencySource::Path {
                        path: package_root.join(path),
                    }
                }
                None => {
                    if !default_features {
                        return Err(invalid(format!(
                            "{} registry dependency `{name}` disables default features; prepare an explicit Oven-native closure",
                            manifest_path.display()
                        )));
                    }
                    if version.is_none() {
                        return Err(invalid(format!(
                            "{} registry dependency `{name}` has no version requirement",
                            manifest_path.display()
                        )));
                    }
                    DependencySource::Registry
                }
            };
            DependencySpec {
                crate_name: name.to_string(),
                version,
                features,
                default_features,
                source,
                optional: false,
                package,
            }
        }
        _ => {
            return Err(invalid(format!(
                "{} dependency `{name}` has an unsupported Cargo manifest shape",
                manifest_path.display()
            )));
        }
    }
    .normalized();
    Ok(Some(dependency))
}

/// Permit a path package only when the requested direct-Rustc configuration activates no Cargo feature.
///
/// `default-features = false` is not itself a feature activation: Cargo would compile the dependency without any
/// `feature=...` cfg values. That is exactly the direct-Rustc configuration Oven emits, so accepting it avoids
/// rejecting generated SDK components whose projection disables an empty/default feature set. Explicit features and
/// non-empty default feature groups still need an Oven-native feature closure and remain fail-closed.
fn validate_inactive_path_dependency_features(
    manifest_path: &Path,
    manifest: &toml::Value,
    dependency: &DependencySpec,
) -> Result<(), OvenRustcError> {
    if !dependency.features.is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "path Rust dependency Cargo.toml",
            message: format!(
                "{} path dependency `{}` explicitly enables Cargo features; prepare an explicit Oven-native closure",
                manifest_path.display(),
                dependency.crate_name
            ),
        });
    }
    if !dependency.default_features {
        return Ok(());
    }
    let Some(features) = manifest.get("features") else {
        return Ok(());
    };
    let features = features.as_table().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "path Rust dependency Cargo.toml",
        message: format!("{} has a non-table [features] declaration", manifest_path.display()),
    })?;
    let Some(default) = features.get("default") else {
        return Ok(());
    };
    let default = default.as_array().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "path Rust dependency Cargo.toml",
        message: format!(
            "{} has a non-array default feature declaration",
            manifest_path.display()
        ),
    })?;
    if default.is_empty() {
        return Ok(());
    }
    Err(OvenRustcError::InvalidInput {
        field: "path Rust dependency Cargo.toml",
        message: format!(
            "{} path dependency `{}` activates default Cargo features; prepare an explicit Oven-native closure",
            manifest_path.display(),
            dependency.crate_name
        ),
    })
}

/// Normalize a declared package name to the direct-rustc crate identifier it exposes.
fn direct_rustc_crate_name(name: &str) -> Result<String, OvenRustcError> {
    let normalized = name.replace('-', "_");
    validate_rust_identifier(&normalized)?;
    Ok(normalized)
}

/// Add direct-Rustc workspace-library outputs to a previously selected immutable artifact plan.
///
/// The immutable plan continues to own third-party and native inputs. These libraries are the caller-owned bridge
/// between topologically ordered workspace compilation steps, so their already-verified digests are incorporated
/// into the consumer output's reuse identity instead of being mistaken for a Cargo target directory.
pub(crate) fn attach_caller_owned_rustc_libraries(
    plan: &mut OvenRustcArtifactPlan,
    libraries: &[OvenCallerOwnedRustcLibrary],
) -> Result<(), OvenRustcError> {
    let mut crate_names = plan
        .externs
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<BTreeSet<_>>();
    for library in libraries {
        validate_rust_identifier(&library.crate_name)?;
        if library.expose_extern && !crate_names.insert(library.crate_name.clone()) {
            return Err(OvenRustcError::InvalidInput {
                field: "caller-owned library",
                message: format!("duplicates direct-Rustc extern `{}`", library.crate_name),
            });
        }
        let output = verified_regular_file(&library.output, "caller-owned library")?;
        let extension = output.extension().and_then(|extension| extension.to_str());
        if !matches!(extension, Some("rlib" | "dylib" | "so" | "dll")) {
            return Err(OvenRustcError::InvalidInput {
                field: "caller-owned library",
                message: format!("{} must be a Rust library or procedural-macro output", output.display()),
            });
        }
        let parent = output.parent().ok_or_else(|| OvenRustcError::InvalidInput {
            field: "caller-owned library",
            message: format!("{} has no parent directory", output.display()),
        })?;
        if !plan.dependency_search_paths.contains(&parent.to_path_buf()) {
            plan.dependency_search_paths.push(parent.to_path_buf());
        }
        let evidence_key = if library.expose_extern {
            library.crate_name.clone()
        } else {
            format!("transitive:{}:{}", library.crate_name, library.digest)
        };
        if plan
            .caller_owned_library_digests
            .insert(evidence_key, library.digest.clone())
            .is_some()
        {
            return Err(OvenRustcError::InvalidInput {
                field: "caller-owned library",
                message: format!("duplicates reuse evidence for `{}`", library.crate_name),
            });
        }
        if library.expose_extern {
            plan.externs.push((library.crate_name.clone(), output));
        }
    }
    plan.dependency_search_paths.sort();
    plan.dependency_search_paths.dedup();
    Ok(())
}

/// One actively leased immutable root contributing a named fragment to a composed compiler-suite closure.
///
/// Every fragment is verified at publication and held by the scheduler before compilation starts. Direct Rustc
/// receives its paths directly from these roots, avoiding a copied aggregate directory that would itself evade
/// compatibility-domain policy.
pub(crate) struct OvenTrustedRustcArtifactRoot<'a> {
    /// Store-owned root of this selected foundation artifact.
    pub artifact_root: &'a Path,
    /// Direct-rustc closure fragment retained in this root.
    pub dependency_search_paths: &'a [String],
    /// Native-link search paths retained in this root.
    pub native_search_paths: &'a [String],
    /// Every artifact materialized in this root.
    pub supporting_artifacts: &'a [OvenRustcSupportingArtifact],
}

/// Publisher-owned artifact input that has passed manifest validation and may be copied into Oven storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenRustcMaterializedArtifact {
    /// Portable manifest path preserved beneath the store-owned artifact root.
    pub relative_path: String,
    /// Canonical publisher path whose bytes match the manifest digest at validation time.
    pub source_path: PathBuf,
}

/// Request to compile one receipt-bound test source with direct `rustc`.
#[derive(Debug, Clone)]
pub struct OvenDirectRustcTestRequest {
    /// Verified project/build-unit receipt that authorizes source and intent.
    pub receipt: OvenReceipt,
    /// Immutable artifact manifest selected for the receipt intent.
    pub artifacts: OvenRustcArtifactManifest,
    /// Immutable artifact root containing every path named by `artifacts`.
    pub artifact_root: PathBuf,
    /// Explicit Rust compiler executable; PATH lookup is not used as a hidden selector.
    pub rustc: PathBuf,
    /// Caller-owned generated Rust source file.
    pub source: PathBuf,
    /// Caller-owned final test executable output path.
    pub output: PathBuf,
    /// Rust crate name for the test harness.
    pub crate_name: String,
    /// Rust edition supplied to the compiler.
    pub edition: String,
    /// Receipt supplemental-digest key that authorizes the generated source content.
    pub source_evidence_key: String,
}

/// Request to compile one receipt-bound binary source with direct `rustc`.
#[derive(Debug, Clone)]
pub struct OvenDirectRustcRunRequest {
    /// Verified project/build-unit receipt that authorizes source and intent.
    pub receipt: OvenReceipt,
    /// Immutable artifact manifest selected for the receipt intent.
    pub artifacts: OvenRustcArtifactManifest,
    /// Immutable artifact root containing every path named by `artifacts`.
    pub artifact_root: PathBuf,
    /// Explicit Rust compiler executable; PATH lookup is not used as a hidden selector.
    pub rustc: PathBuf,
    /// Caller-owned generated Rust source file.
    pub source: PathBuf,
    /// Caller-owned final binary output path.
    pub output: PathBuf,
    /// Rust crate name for the binary.
    pub crate_name: String,
    /// Rust edition supplied to the compiler.
    pub edition: String,
    /// Receipt supplemental-digest key that authorizes the generated source content.
    pub source_evidence_key: String,
}

/// Request to compile a test through an immutable direct-rustc closure selected from the bounded Oven store.
pub struct OvenStoredDirectRustcTestRequest<'a> {
    /// Bounded Oven store that retains the selected immutable plan.
    pub store: &'a OvenStore,
    /// Exact store identity of the `direct_rustc_plan` artifact to execute.
    pub plan_identity: String,
    /// Verified project/build-unit receipt that authorizes source and intent.
    pub receipt: OvenReceipt,
    /// Explicit Rust compiler executable; PATH lookup is not used as a hidden selector.
    pub rustc: PathBuf,
    /// Caller-owned generated Rust source file.
    pub source: PathBuf,
    /// Caller-owned final test executable output path.
    pub output: PathBuf,
    /// Rust crate name for the test harness.
    pub crate_name: String,
    /// Rust edition supplied to the compiler.
    pub edition: String,
    /// Receipt supplemental-digest key that authorizes the generated source content.
    pub source_evidence_key: String,
}

/// Request to compile a binary through an immutable direct-rustc closure selected from the bounded Oven store.
pub struct OvenStoredDirectRustcRunRequest<'a> {
    /// Bounded Oven store that retains the selected immutable plan.
    pub store: &'a OvenStore,
    /// Exact store identity of the `direct_rustc_plan` artifact to execute.
    pub plan_identity: String,
    /// Verified project/build-unit receipt that authorizes source and intent.
    pub receipt: OvenReceipt,
    /// Explicit Rust compiler executable; PATH lookup is not used as a hidden selector.
    pub rustc: PathBuf,
    /// Caller-owned generated Rust source file.
    pub source: PathBuf,
    /// Caller-owned final binary output path.
    pub output: PathBuf,
    /// Rust crate name for the binary.
    pub crate_name: String,
    /// Supported Rust edition.
    pub edition: String,
    /// Receipt supplemental-digest key that authorizes the generated source content.
    pub source_evidence_key: String,
}

/// Request to compile a caller-owned Rust library through an immutable direct-rustc closure selected from the
/// bounded Oven store.
///
/// This is the normal-command counterpart to [`OvenStoredDirectRustcRunRequest`]. The generated library source and
/// final `.rlib` remain caller-owned, while the third-party/runtime closure is selected only through the receipt and
/// held under a store lease for the compilation.
pub struct OvenStoredDirectRustcLibraryRequest<'a> {
    /// Bounded Oven store that retains the selected immutable plan.
    pub store: &'a OvenStore,
    /// Exact store identity of the `direct_rustc_plan` artifact to execute.
    pub plan_identity: String,
    /// Verified project/build-unit receipt that authorizes source and intent.
    pub receipt: OvenReceipt,
    /// Explicit Rust compiler executable; PATH lookup is not used as a hidden selector.
    pub rustc: PathBuf,
    /// Caller-owned generated Rust source file.
    pub source: PathBuf,
    /// Caller-owned final Rust library output path.
    pub output: PathBuf,
    /// Rust crate name for the library.
    pub crate_name: String,
    /// Supported Rust edition.
    pub edition: String,
    /// Receipt supplemental-digest key that authorizes the generated source content.
    pub source_evidence_key: String,
}

/// Request to compile one target from an already selected, actively leased Oven suite artifact.
///
/// Compiler-suite execution holds the suite lease for the complete inventory and run, but its payload is not a
/// standalone `direct_rustc_plan` store entry. This narrow internal request reuses the trusted-store materialization
/// path without granting callers the ability to select an unleased artifact root.
pub(crate) struct OvenTrustedDirectRustcTargetRequest<'a> {
    /// Exact receipt that authorizes the workspace target source.
    pub receipt: &'a OvenReceipt,
    /// Target-specific direct-rustc closure declared by the immutable suite payload.
    pub artifacts: &'a OvenRustcArtifactManifest,
    /// Materialized root returned alongside the active suite lease.
    pub artifact_root: &'a Path,
    /// Optional plan composed from several separately leased compiler foundations.
    ///
    /// When absent, this request materializes the one legacy schema-8/9 artifact root. Schema 10 supplies an
    /// already validated composed plan and never asks this runner to rebuild a directory tree.
    pub artifact_plan: Option<&'a OvenRustcArtifactPlan>,
    /// Explicit compiler selected by the receipt.
    pub rustc: &'a Path,
    /// Caller-owned workspace test root.
    pub source: &'a Path,
    /// Caller-owned transient native test executable.
    pub output: &'a Path,
    /// Rust crate identifier for this test root.
    pub crate_name: &'a str,
    /// Rust edition declared by the target inventory.
    pub edition: &'a str,
    /// Receipt source-evidence key for this exact target root.
    pub source_evidence_key: &'a str,
    /// Resolved target feature set from the publisher's unit graph.
    pub features: &'a [String],
    /// Whether Cargo's publisher compiled this libtest root with `-C prefer-dynamic`.
    pub prefer_dynamic: bool,
}

/// Request to run one receipt-bound Rustdoc doctest root from an actively leased compiler-suite artifact.
///
/// Rustdoc owns the ephemeral doctest binaries, so Oven records the caller-owned temporary directory rather than
/// pretending there is a stable native executable to store or re-run.
pub(crate) struct OvenTrustedRustdocTestRequest<'a> {
    /// Exact receipt that authorizes the workspace target source.
    pub receipt: &'a OvenReceipt,
    /// Target-specific artifact closure declared by the immutable suite payload.
    pub artifacts: &'a OvenRustcArtifactManifest,
    /// Materialized root returned alongside the active suite lease.
    pub artifact_root: &'a Path,
    /// Optional plan composed from several separately leased compiler foundations.
    pub artifact_plan: Option<&'a OvenRustcArtifactPlan>,
    /// Explicit compiler selected by the receipt; its sibling Rustdoc is derived from the same sysroot.
    pub rustc: &'a Path,
    /// Caller-owned workspace Rustdoc source root.
    pub source: &'a Path,
    /// Caller-owned temporary directory for Rustdoc's ephemeral doctest binaries.
    pub temporary_directory: &'a Path,
    /// Rust crate identifier for this doctest root.
    pub crate_name: &'a str,
    /// Rust edition declared by the target inventory.
    pub edition: &'a str,
    /// Receipt source-evidence key for this exact source root.
    pub source_evidence_key: &'a str,
    /// Resolved target feature set from the publisher's unit graph.
    pub features: &'a [String],
    /// Whether Rustdoc must compile this root as a procedural-macro crate.
    pub is_proc_macro: bool,
    /// Whether the doctest root requires the selected toolchain dynamic library environment.
    pub prefer_dynamic: bool,
    /// Maximum wall-clock duration for the Rustdoc root and its generated doctest descendants.
    pub timeout: Option<Duration>,
}

/// Successful direct Rustdoc doctest execution transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenRustdocTestReport {
    /// Combined Rustdoc/doctest output for caller failure reporting.
    pub output: String,
}

/// Structured source span emitted by rustc's JSON diagnostic stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenRustcDiagnosticSpan {
    /// Rust source filename from rustc.
    pub file_name: String,
    /// One-based start line.
    pub line_start: u32,
    /// One-based start column.
    pub column_start: u32,
    /// One-based end line.
    pub line_end: u32,
    /// One-based end column.
    pub column_end: u32,
    /// Whether rustc identified this as the primary span.
    pub is_primary: bool,
}

/// One structured rustc diagnostic preserved without terminal-only parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenRustcDiagnostic {
    /// rustc severity level.
    pub level: String,
    /// Human-readable diagnostic message.
    pub message: String,
    /// Optional rustc error code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Structured source spans.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<OvenRustcDiagnosticSpan>,
    /// Optional rustc-rendered display form.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered: Option<String>,
}

/// Rustc diagnostic transcript for a failed direct Oven consumer compilation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenRustcDiagnosticReport {
    /// JSON diagnostics decoded from rustc output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<OvenRustcDiagnostic>,
    /// Non-JSON rustc output retained verbatim for diagnostics that lack a JSON record.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unstructured_output: String,
    /// Bounded direct-Rustc command evidence for a failed Oven compilation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation: Option<String>,
}

impl fmt::Display for OvenRustcDiagnosticReport {
    /// Render a bounded, actionable terminal summary while the complete structured report remains available to callers.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        const MAX_DIAGNOSTICS: usize = 12;
        const MAX_UNSTRUCTURED_CHARS: usize = 4_000;

        if self.diagnostics.is_empty() && self.unstructured_output.trim().is_empty() && self.invocation.is_none() {
            return formatter.write_str("rustc exited unsuccessfully without emitting diagnostics");
        }
        for (index, diagnostic) in self.diagnostics.iter().take(MAX_DIAGNOSTICS).enumerate() {
            if index > 0 {
                formatter.write_str("\n")?;
            }
            write!(formatter, "{}", diagnostic.level)?;
            if let Some(code) = &diagnostic.code {
                write!(formatter, "[{code}]")?;
            }
            write!(formatter, ": {}", diagnostic.message)?;
            if let Some(span) = diagnostic.spans.iter().find(|span| span.is_primary) {
                write!(
                    formatter,
                    " at {}:{}:{}",
                    span.file_name, span.line_start, span.column_start
                )?;
            }
        }
        if self.diagnostics.len() > MAX_DIAGNOSTICS {
            write!(
                formatter,
                "\n… {} additional rustc diagnostic(s) omitted from terminal summary",
                self.diagnostics.len() - MAX_DIAGNOSTICS
            )?;
        }
        let unstructured = self.unstructured_output.trim();
        if !unstructured.is_empty() {
            if !self.diagnostics.is_empty() {
                formatter.write_str("\n")?;
            }
            for character in unstructured.chars().take(MAX_UNSTRUCTURED_CHARS) {
                write!(formatter, "{character}")?;
            }
            if unstructured.chars().count() > MAX_UNSTRUCTURED_CHARS {
                formatter.write_str("\n… rustc unstructured output truncated")?;
            }
        }
        if let Some(invocation) = &self.invocation {
            if !self.diagnostics.is_empty() || !unstructured.is_empty() {
                formatter.write_str("\n")?;
            }
            write!(formatter, "direct rustc invocation: {invocation}")?;
        }
        Ok(())
    }
}

/// Successful direct-rustc consumer compilation evidence.
pub struct OvenDirectRustcBake {
    /// Source digest that was matched to receipt evidence before compiler invocation.
    pub source_digest: String,
    /// Final caller-owned test executable path.
    pub output: PathBuf,
    /// Digest of the regular caller-owned output, established before it is exposed to another direct-Rustc step.
    pub output_digest: String,
    /// The command cleared all Cargo process variables before starting rustc.
    pub cargo_process_started: bool,
    /// Whether the receipt- and plan-verified caller-owned native output was reused without launching rustc.
    pub reused: bool,
    /// Held for stored consumers until their caller finishes executing the native test binary.
    lease: Option<OvenStoreLease>,
}

/// The concrete caller-owned output Rustc must produce for one Oven materialization step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
enum OvenDirectRustcOutputKind {
    Binary,
    Library,
    Dylib,
    ProcMacro,
}

impl OvenDirectRustcOutputKind {
    /// Return the stable receipt spelling for this caller-owned output kind.
    const fn receipt_value(self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::Library => "library",
            Self::Dylib => "dylib",
            Self::ProcMacro => "proc-macro",
        }
    }
}

/// Return the receipt default for callers that predate an explicit output-kind field.
fn default_direct_rustc_output_kind() -> String {
    OvenDirectRustcOutputKind::Binary.receipt_value().to_string()
}

/// Small sidecar persisted beside a caller-owned native output so an unchanged normal command does not rebuild it.
///
/// The sidecar is not Oven store state and never authorizes execution by itself: the caller still verifies the
/// receipt, compiler identity, source digest, and immutable plan before this record can admit a reuse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OvenDirectRustcOutputReceipt {
    schema_version: u32,
    receipt_identity: String,
    artifact_manifest_digest: String,
    source_digest: String,
    crate_name: String,
    edition: String,
    #[serde(default)]
    features: Vec<String>,
    test_harness: bool,
    prefer_dynamic: bool,
    #[serde(default = "default_direct_rustc_output_kind")]
    output_kind: String,
    #[serde(default)]
    caller_owned_library_digests: BTreeMap<String, String>,
}

/// Backwards-compatible name for a direct-rustc libtest bake.
pub type OvenDirectRustcTestBake = OvenDirectRustcBake;

/// Errors while verifying direct-rustc inputs or compiling an Oven consumer.
#[derive(Debug, thiserror::Error)]
pub enum OvenRustcError {
    /// Artifact manifest schema differs from this executable's supported wire format.
    #[error("unsupported Oven Rust artifact manifest schema version {found}; expected {expected}")]
    UnsupportedSchema { found: u32, expected: u32 },
    /// Artifact plan intent differs from the selected Oven receipt.
    #[error("Oven direct-rustc artifact intent differs from the selected receipt")]
    IntentMismatch,
    /// A request field is blank or does not obey the narrow Alpha spelling contract.
    #[error("invalid Oven direct-rustc {field}: {message}")]
    InvalidInput { field: &'static str, message: String },
    /// A declared artifact path escapes the immutable artifact root or is not a regular file/directory.
    #[error("invalid Oven direct-rustc {kind} path {path}: {message}")]
    InvalidArtifactPath {
        kind: &'static str,
        path: PathBuf,
        message: String,
    },
    /// A declared artifact did not retain the digest recorded in the immutable artifact plan.
    #[error("Oven direct-rustc artifact digest mismatch at {path}: expected {expected}, got {actual}")]
    ArtifactDigestMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    /// A declared search directory contains an unrecorded or unsupported entry.
    #[error("Oven direct-rustc search directory has unrecorded input {path}")]
    UnrecordedSearchArtifact { path: PathBuf },
    /// Source content differs from the receipt-bound supplemental source evidence.
    #[error("Oven direct-rustc source evidence mismatch for `{key}`: expected {expected}, got {actual}")]
    SourceEvidenceMismatch {
        key: String,
        expected: String,
        actual: String,
    },
    /// The explicit compiler did not report the exact identity frozen in the receipt.
    #[error("Oven direct-rustc compiler identity mismatch: receipt requires `{expected}`, --rustc reports `{actual}`")]
    ToolchainMismatch { expected: String, actual: String },
    /// Reading or writing a direct-rustc input/output path failed.
    #[error("Oven direct-rustc I/O failed at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    /// Rustc returned a non-success status with structured diagnostic evidence.
    #[error("Oven direct-rustc compilation failed:\n{report}")]
    CompilationFailed { report: OvenRustcDiagnosticReport },
    /// Receipt-bound Rustdoc returned a non-success status while executing doctests.
    #[error("Oven direct Rustdoc doctest failed:\n{output}")]
    RustdocTestFailed { output: String },
    /// A bounded Oven store failed to select the immutable direct-rustc plan.
    #[error("Oven direct-rustc plan store failure: {0}")]
    Store(#[from] OvenStoreError),
    /// A selected store entry is not the receipt-bound direct-rustc plan expected by this executor.
    #[error("invalid Oven direct-rustc stored plan `{identity}`: {message}")]
    InvalidStoredPlan { identity: String, message: String },
    /// The bounded store does not retain a unique direct-rustc plan for the requested receipt.
    #[error("Oven direct-rustc selection failed for receipt `{receipt_identity}`: {message}")]
    PlanSelection { receipt_identity: String, message: String },
}

impl OvenRustcArtifactManifest {
    /// Return the complete artifact file set declared by this immutable plan without reading artifact bytes.
    pub(crate) fn declared_artifact_paths(&self) -> Result<BTreeSet<String>, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        Ok(expected_artifacts(self)?.into_keys().collect())
    }

    /// Verify and materialize exact compiler inputs without scanning Cargo output or resolving dependencies.
    pub fn materialize(
        &self,
        artifact_root: &Path,
        expected_intent: &OvenBuildIntent,
    ) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
        self.validate_shape(expected_intent)?;
        let root = canonical_directory(artifact_root, "artifact root")?;
        let expected = expected_artifacts(self)?;
        let dependency_search_paths =
            materialize_search_paths(&root, &self.dependency_search_paths, "dependency search", &expected)?;
        let native_search_paths =
            materialize_search_paths(&root, &self.native_search_paths, "native search", &expected)?;
        for auxiliary in &self.vocab_auxiliary_targets {
            let _ = materialize_search_paths(
                &root,
                &auxiliary.dependency_search_paths,
                "vocab auxiliary dependency search",
                &expected,
            )?;
            for artifact in &auxiliary.externs {
                let _ = verified_file(
                    &root,
                    &artifact.relative_path,
                    &artifact.digest,
                    "vocab auxiliary extern",
                )?;
            }
        }
        let externs = self
            .externs
            .iter()
            .map(|artifact| {
                validate_rust_identifier(&artifact.crate_name)?;
                let path = verified_file(&root, &artifact.relative_path, &artifact.digest, "extern")?;
                Ok((artifact.crate_name.clone(), path))
            })
            .collect::<Result<Vec<_>, OvenRustcError>>()?;
        for artifact in &self.supporting_artifacts {
            verified_file(&root, &artifact.relative_path, &artifact.digest, "supporting")?;
        }
        Ok(OvenRustcArtifactPlan {
            dependency_search_paths,
            native_search_paths,
            externs,
            compile_environment: validated_compile_environment(&self.compile_environment)?,
            caller_owned_library_digests: BTreeMap::new(),
        })
    }

    /// Return the complete verified publisher closure for atomic copying into a store-owned artifact root.
    pub fn materialized_artifacts(
        &self,
        artifact_root: &Path,
        expected_intent: &OvenBuildIntent,
    ) -> Result<Vec<OvenRustcMaterializedArtifact>, OvenRustcError> {
        self.validate_shape(expected_intent)?;
        let root = canonical_directory(artifact_root, "artifact root")?;
        let expected = expected_artifacts(self)?;
        validate_publisher_search_paths(&self.dependency_search_paths, "dependency search", &expected)?;
        validate_publisher_search_paths(&self.native_search_paths, "native search", &expected)?;
        for auxiliary in &self.vocab_auxiliary_targets {
            validate_publisher_search_paths(
                &auxiliary.dependency_search_paths,
                "vocab auxiliary dependency search",
                &expected,
            )?;
        }
        expected
            .into_iter()
            .map(|(relative_path, digest)| {
                Ok(OvenRustcMaterializedArtifact {
                    source_path: verified_file(&root, &relative_path, &digest, "materialized")?,
                    relative_path,
                })
            })
            .collect()
    }

    /// Materialize a plan that has already been atomically copied and digest-verified by the Oven store publisher.
    ///
    /// Normal consumers still verify the receipt, plan payload, artifact-root containment, regular-file shape, and
    /// active lease. They intentionally do not rehash every retained dependency: doing so turns a prepared test run
    /// into a multi-gigabyte integrity scan. `inspect oven` remains the explicit full-closure audit operation.
    pub(crate) fn materialize_trusted_store(
        &self,
        artifact_root: &Path,
        expected_intent: &OvenBuildIntent,
    ) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
        self.validate_shape(expected_intent)?;
        let root = canonical_directory(artifact_root, "artifact root")?;
        let expected = expected_artifacts(self)?;
        let dependency_search_paths =
            trusted_materialize_search_paths(&root, &self.dependency_search_paths, "dependency search", &expected)?;
        let native_search_paths =
            trusted_materialize_search_paths(&root, &self.native_search_paths, "native search", &expected)?;
        for auxiliary in &self.vocab_auxiliary_targets {
            let _ = trusted_materialize_search_paths(
                &root,
                &auxiliary.dependency_search_paths,
                "vocab auxiliary dependency search",
                &expected,
            )?;
            for artifact in &auxiliary.externs {
                let _ = trusted_file(&root, &artifact.relative_path, "vocab auxiliary extern")?;
            }
        }
        let externs = self
            .externs
            .iter()
            .map(|artifact| {
                Ok((
                    artifact.crate_name.clone(),
                    trusted_file(&root, &artifact.relative_path, "extern")?,
                ))
            })
            .collect::<Result<Vec<_>, OvenRustcError>>()?;
        for artifact in &self.supporting_artifacts {
            trusted_file(&root, &artifact.relative_path, "supporting")?;
        }
        Ok(OvenRustcArtifactPlan {
            dependency_search_paths,
            native_search_paths,
            externs,
            compile_environment: validated_compile_environment(&self.compile_environment)?,
            caller_owned_library_digests: BTreeMap::new(),
        })
    }

    /// Materialize one publisher-declared vocabulary cross-target closure from an already selected immutable root.
    ///
    /// This is intentionally not part of the ordinary [`OvenRustcArtifactPlan`]: normal host commands must never
    /// hand cross-target artifacts to Rustc. Vocabulary extraction names the target from its verified metadata and
    /// may receive only the exact matching auxiliary closure.
    pub(crate) fn materialize_trusted_vocab_auxiliary_target(
        &self,
        artifact_root: &Path,
        target: &str,
    ) -> Result<Option<OvenRustcAuxiliaryTargetPlan>, OvenRustcError> {
        let root = canonical_directory(artifact_root, "artifact root")?;
        let expected = expected_artifacts(self)?;
        let Some(auxiliary) = self
            .vocab_auxiliary_targets
            .iter()
            .find(|auxiliary| auxiliary.target == target)
        else {
            return Ok(None);
        };
        let dependency_search_paths = trusted_materialize_search_paths(
            &root,
            &auxiliary.dependency_search_paths,
            "vocab auxiliary dependency search",
            &expected,
        )?;
        let externs = auxiliary
            .externs
            .iter()
            .map(|artifact| {
                Ok((
                    artifact.crate_name.clone(),
                    trusted_file(&root, &artifact.relative_path, "vocab auxiliary extern")?,
                ))
            })
            .collect::<Result<Vec<_>, OvenRustcError>>()?;
        Ok(Some(OvenRustcAuxiliaryTargetPlan {
            dependency_search_paths,
            externs,
        }))
    }

    /// Materialize a complete direct-rustc plan from several separately bounded, actively leased Oven roots.
    ///
    /// The outer manifest remains the one target-specific execution contract. Each root contributes a disjoint
    /// publisher-declared fragment; this method rejects a missing, duplicate, or substituted path before passing
    /// any `-L` or `--extern` argument to Rustc.
    pub(crate) fn materialize_trusted_store_composed(
        &self,
        roots: &[OvenTrustedRustcArtifactRoot<'_>],
        expected_intent: &OvenBuildIntent,
    ) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
        self.validate_shape(expected_intent)?;
        if roots.is_empty() {
            return Err(OvenRustcError::InvalidInput {
                field: "composed artifact roots",
                message: "must contain at least one actively leased foundation".to_string(),
            });
        }
        let expected = expected_artifacts(self)?;
        let mut locations = BTreeMap::<String, PathBuf>::new();
        let mut dependency_search_paths = Vec::new();
        let mut native_search_paths = Vec::new();
        for fragment in roots {
            let root = canonical_directory(fragment.artifact_root, "composed artifact root")?;
            let mut fragment_expected = BTreeMap::new();
            for artifact in fragment.supporting_artifacts {
                let relative = normalized_relative_path(&artifact.relative_path, "composed supporting artifact")?;
                let expected_digest = expected.get(&relative).ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "composed artifact roots",
                    message: format!("declare unrecognized artifact `{relative}`"),
                })?;
                if expected_digest != &artifact.digest {
                    return Err(OvenRustcError::InvalidInput {
                        field: "composed artifact roots",
                        message: format!("declare mismatched digest for `{relative}`"),
                    });
                }
                let path = trusted_file(&root, &relative, "composed supporting artifact")?;
                if locations.insert(relative.clone(), path).is_some() {
                    return Err(OvenRustcError::InvalidInput {
                        field: "composed artifact roots",
                        message: format!("declare duplicate artifact `{relative}`"),
                    });
                }
                if fragment_expected.insert(relative, artifact.digest.clone()).is_some() {
                    return Err(OvenRustcError::InvalidInput {
                        field: "composed artifact roots",
                        message: "declare one artifact path more than once".to_string(),
                    });
                }
            }
            if fragment_expected.is_empty() {
                return Err(OvenRustcError::InvalidInput {
                    field: "composed artifact roots",
                    message: "must not contain an empty foundation fragment".to_string(),
                });
            }
            dependency_search_paths.extend(trusted_materialize_search_paths(
                &root,
                fragment.dependency_search_paths,
                "composed dependency search",
                &fragment_expected,
            )?);
            native_search_paths.extend(trusted_materialize_search_paths(
                &root,
                fragment.native_search_paths,
                "composed native search",
                &fragment_expected,
            )?);
        }
        let missing = expected
            .keys()
            .filter(|relative| !locations.contains_key(*relative))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(OvenRustcError::InvalidInput {
                field: "composed artifact roots",
                message: format!("omit required artifact(s): {}", missing.join(", ")),
            });
        }
        dependency_search_paths.sort();
        dependency_search_paths.dedup();
        native_search_paths.sort();
        native_search_paths.dedup();
        let externs = self
            .externs
            .iter()
            .map(|artifact| {
                let relative = normalized_relative_path(&artifact.relative_path, "extern")?;
                let path = locations.get(&relative).ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "composed artifact roots",
                    message: format!("omit extern `{relative}`"),
                })?;
                Ok((artifact.crate_name.clone(), path.clone()))
            })
            .collect::<Result<Vec<_>, OvenRustcError>>()?;
        Ok(OvenRustcArtifactPlan {
            dependency_search_paths,
            native_search_paths,
            externs,
            compile_environment: validated_compile_environment(&self.compile_environment)?,
            caller_owned_library_digests: BTreeMap::new(),
        })
    }

    /// Select the exact direct root externs authorized for one receipt source target.
    ///
    /// The unselected extern artifacts remain declared supporting inputs so strict search-directory completeness and
    /// publisher-time digest verification still cover the entire immutable compatibility closure.
    fn for_source_evidence(&self, source_evidence_key: &str) -> Result<Self, OvenRustcError> {
        let Some(allowed_names) = self.entrypoint_externs.get(source_evidence_key) else {
            return Ok(self.clone());
        };
        let allowed = allowed_names.iter().cloned().collect::<BTreeSet<_>>();
        let available = self
            .externs
            .iter()
            .map(|artifact| artifact.crate_name.clone())
            .collect::<BTreeSet<_>>();
        let missing = allowed
            .iter()
            .filter(|crate_name| !available.contains(*crate_name))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest entrypoint externs",
                message: format!(
                    "source evidence `{source_evidence_key}` names undeclared extern(s): {}",
                    missing.join(", ")
                ),
            });
        }
        let mut selected = self.clone();
        // A helper-only dependency directory may contain a second build of a crate that has the same stable Rust
        // identity as a generated-root dependency. Keeping that directory on `-L dependency` after its only direct
        // roots are projected away still lets Rustc discover the conflicting copy. Retain every mixed or
        // transitive-only directory, but remove a directory whose declared direct roots are all excluded.
        let excluded_dependency_search_paths = self
            .dependency_search_paths
            .iter()
            .filter(|search_path| {
                let direct_roots = self
                    .externs
                    .iter()
                    .filter(|artifact| artifact_is_below_search_path(&artifact.relative_path, search_path))
                    .collect::<Vec<_>>();
                !direct_roots.is_empty()
                    && direct_roots
                        .iter()
                        .all(|artifact| !allowed.contains(&artifact.crate_name))
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        let retained = selected
            .externs
            .iter()
            .filter(|artifact| !allowed.contains(&artifact.crate_name))
            .map(|artifact| OvenRustcSupportingArtifact {
                relative_path: artifact.relative_path.clone(),
                digest: artifact.digest.clone(),
            })
            .collect::<Vec<_>>();
        selected
            .externs
            .retain(|artifact| allowed.contains(&artifact.crate_name));
        selected
            .dependency_search_paths
            .retain(|search_path| !excluded_dependency_search_paths.contains(search_path));
        selected.supporting_artifacts.extend(retained);
        Ok(selected)
    }

    /// Validate receipt-independent manifest shape before a stored plan may be selected or republished.
    ///
    /// This deliberately avoids a full byte-by-byte artifact traversal. Publication and execution perform that
    /// stronger validation; selection uses this inexpensive gate so a legacy malformed payload cannot make a newly
    /// corrected plan ambiguous forever.
    pub fn validate_shape(&self, expected_intent: &OvenBuildIntent) -> Result<(), OvenRustcError> {
        if self.schema_version != OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION {
            return Err(OvenRustcError::UnsupportedSchema {
                found: self.schema_version,
                expected: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            });
        }
        if &self.intent != expected_intent {
            return Err(OvenRustcError::IntentMismatch);
        }
        let _ = validated_compile_environment(&self.compile_environment)?;
        let mut names = BTreeSet::new();
        for artifact in &self.externs {
            validate_rust_identifier(&artifact.crate_name)?;
            if !names.insert(artifact.crate_name.as_str()) {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest",
                    message: format!("declares duplicate extern crate `{}`", artifact.crate_name),
                });
            }
        }
        for (source_evidence_key, crate_names) in &self.entrypoint_externs {
            if source_evidence_key.trim().is_empty() {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest entrypoint externs",
                    message: "must not contain an empty source-evidence key".to_string(),
                });
            }
            let mut seen = BTreeSet::new();
            for crate_name in crate_names {
                validate_rust_identifier(crate_name)?;
                if !names.contains(crate_name.as_str()) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest entrypoint externs",
                        message: format!(
                            "source evidence `{source_evidence_key}` names undeclared extern `{crate_name}`"
                        ),
                    });
                }
                if !seen.insert(crate_name.as_str()) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest entrypoint externs",
                        message: format!(
                            "source evidence `{source_evidence_key}` names extern `{crate_name}` more than once"
                        ),
                    });
                }
            }
        }
        let mut declared_artifacts = BTreeMap::new();
        for artifact in self
            .externs
            .iter()
            .map(|artifact| (&artifact.relative_path, &artifact.digest))
            .chain(
                self.supporting_artifacts
                    .iter()
                    .map(|artifact| (&artifact.relative_path, &artifact.digest)),
            )
        {
            if declared_artifacts
                .insert(artifact.0.as_str(), artifact.1.as_str())
                .is_some()
            {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: format!("declares artifact `{}` more than once", artifact.0),
                });
            }
        }
        let mut package_versions = BTreeSet::new();
        for leaf in &self.registry_leaves {
            if leaf.package.trim().is_empty() || leaf.version.trim().is_empty() {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: "registry leaf package and version must not be empty".to_string(),
                });
            }
            validate_rust_identifier(&leaf.crate_name)?;
            if leaf.crate_name != leaf.artifact.crate_name {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: format!(
                        "registry leaf `{}` `{}` has inconsistent crate identity",
                        leaf.package, leaf.version
                    ),
                });
            }
            if !package_versions.insert((leaf.package.as_str(), leaf.version.as_str())) {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: format!(
                        "declares package `{}` version `{}` more than once",
                        leaf.package, leaf.version
                    ),
                });
            }
            let mut features = BTreeSet::new();
            for feature in &leaf.features {
                if feature.trim().is_empty() || !features.insert(feature.as_str()) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest registry catalog",
                        message: format!(
                            "registry leaf `{}` `{}` declares an empty or duplicate feature",
                            leaf.package, leaf.version
                        ),
                    });
                }
            }
            if Path::new(&leaf.artifact.relative_path)
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("rlib")
            {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: format!(
                        "registry leaf `{}` `{}` must reference an rlib",
                        leaf.package, leaf.version
                    ),
                });
            }
            match declared_artifacts.get(leaf.artifact.relative_path.as_str()) {
                Some(digest) if *digest == leaf.artifact.digest.as_str() => {}
                Some(_) => {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest registry catalog",
                        message: format!(
                            "registry leaf `{}` `{}` has a digest that disagrees with its sealed artifact",
                            leaf.package, leaf.version
                        ),
                    });
                }
                None => {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest registry catalog",
                        message: format!(
                            "registry leaf `{}` `{}` references undeclared artifact `{}`",
                            leaf.package, leaf.version, leaf.artifact.relative_path
                        ),
                    });
                }
            }
        }
        let mut auxiliary_targets = BTreeSet::new();
        for auxiliary in &self.vocab_auxiliary_targets {
            validate_rust_target(&auxiliary.target)?;
            if !auxiliary_targets.insert(auxiliary.target.as_str()) {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest vocabulary auxiliary targets",
                    message: format!("declares target `{}` more than once", auxiliary.target),
                });
            }
            let mut auxiliary_names = BTreeSet::new();
            for artifact in &auxiliary.externs {
                validate_rust_identifier(&artifact.crate_name)?;
                if !auxiliary_names.insert(artifact.crate_name.as_str()) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest vocabulary auxiliary targets",
                        message: format!(
                            "target `{}` declares duplicate extern crate `{}`",
                            auxiliary.target, artifact.crate_name
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// Expose only this plan's copied, digest-verified registry catalog to a direct-rustc consumer.
    ///
    /// A normal command must not aggregate leaves from a different compiler Loaf: Rust metadata can bind one direct
    /// crate to a particular feature-unified dependency graph even when the package/version names look identical.
    pub(crate) fn registry_leaf_authority(
        &self,
        artifact_root: &Path,
        plan: &OvenRustcArtifactPlan,
    ) -> Option<OvenRegistryLeafAuthority> {
        (!self.registry_leaves.is_empty()).then(|| {
            OvenRegistryLeafAuthority::new_with_trusted_dependency_search_paths(
                artifact_root.to_path_buf(),
                self.registry_leaves.clone(),
                plan.dependency_search_paths.clone(),
            )
        })
    }
}

/// Return whether one declared artifact is directly below a declared search directory.
fn artifact_is_below_search_path(relative_path: &str, search_path: &str) -> bool {
    Path::new(relative_path)
        .strip_prefix(Path::new(search_path))
        .is_ok_and(|suffix| !suffix.as_os_str().is_empty())
}

/// Validate a publisher's declared search-path shape before atomic copying.
///
/// Cargo target directories contain object and dep-info files that Oven intentionally does not retain. The eventual
/// store entry is still checked for exact directory completeness by `materialize`; this pre-copy check only confirms
/// that each declared directory is safe, unique, and owns at least one manifest-recorded artifact.
fn validate_publisher_search_paths(
    paths: &[String],
    kind: &'static str,
    expected: &BTreeMap<String, String>,
) -> Result<(), OvenRustcError> {
    let mut seen = BTreeSet::new();
    for relative in paths {
        let normalized = normalized_relative_path(relative, kind)?;
        if !seen.insert(normalized.clone()) {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!("declares duplicate {kind} path `{relative}`"),
            });
        }
        let prefix = format!("{normalized}/");
        if !expected.keys().any(|artifact| artifact.starts_with(&prefix)) {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!("declares {kind} path `{relative}` without a recorded artifact"),
            });
        }
    }
    Ok(())
}

/// Validate the small compile-time environment envelope that direct-rustc consumers may restore after ambient Cargo
/// state is cleared.
fn validated_compile_environment(
    environment: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, OvenRustcError> {
    for (name, value) in environment {
        let binary_name = name.strip_prefix("CARGO_BIN_EXE_");
        let allowed = name == "CARGO_MANIFEST_DIR"
            || name.starts_with("CARGO_PKG_")
            || binary_name.is_some_and(|name| !name.is_empty());
        if !allowed {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest compile environment",
                message: format!("does not permit `{name}`"),
            });
        }
        if value.is_empty() || value.contains('\0') {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest compile environment",
                message: format!("has an invalid value for `{name}`"),
            });
        }
        if binary_name.is_some() && !Path::new(value).is_absolute() {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest compile environment",
                message: format!("`{name}` must name an absolute caller-owned binary output"),
            });
        }
        if value == "@oven-source-root" || value.starts_with("@oven-source-ancestor:") {
            if name != "CARGO_MANIFEST_DIR" {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest compile environment",
                    message: "source-relative tokens are permitted only for CARGO_MANIFEST_DIR".to_string(),
                });
            }
            if value.starts_with("@oven-source-ancestor:") {
                let Some(distance) = value
                    .strip_prefix("@oven-source-ancestor:")
                    .and_then(|value| value.parse::<usize>().ok())
                else {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest compile environment",
                        message: "source-relative ancestor token must end in a positive integer".to_string(),
                    });
                };
                if !(1..=16).contains(&distance) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest compile environment",
                        message: "source-relative ancestor distance must be between 1 and 16".to_string(),
                    });
                }
            }
        }
    }
    Ok(environment.clone())
}

/// Resolve a portable caller-relative compile environment token after the source has been receipt-authorized.
pub(crate) fn resolve_compile_environment_value(
    name: &str,
    value: &str,
    source: &Path,
) -> Result<PathBuf, OvenRustcError> {
    let distance = if value == "@oven-source-root" {
        2
    } else if let Some(distance) = value
        .strip_prefix("@oven-source-ancestor:")
        .and_then(|value| value.parse::<usize>().ok())
    {
        distance
    } else {
        return Ok(PathBuf::from(value));
    };
    let mut ancestor = source.parent().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "source",
        message: format!("{} has no parent directory for {name}", source.display()),
    })?;
    for _ in 1..distance {
        ancestor = ancestor.parent().ok_or_else(|| OvenRustcError::InvalidInput {
            field: "source",
            message: format!(
                "{} cannot derive source ancestor {distance} for {name}",
                source.display()
            ),
        })?;
    }
    Ok(ancestor.to_path_buf())
}

/// Select a receipt-bound direct-rustc closure and retain its lease until the caller finishes execution.
pub fn bake_stored_direct_rustc_test(
    request: &OvenStoredDirectRustcTestRequest<'_>,
) -> Result<OvenDirectRustcTestBake, OvenRustcError> {
    bake_stored_direct_rustc_test_with_libraries(request, &[])
}

/// Select a receipt-bound direct-rustc closure and compile a native test while linking caller-owned direct Rust
/// libraries.
///
/// The supplemental libraries are generated only by the path-only Oven materializer. They are caller output rather
/// than store artifacts, but their bytes enter the direct output reuse identity through the same checked attachment
/// mechanism used for materialized Incan library dependencies.
pub(crate) fn bake_stored_direct_rustc_test_with_libraries(
    request: &OvenStoredDirectRustcTestRequest<'_>,
    caller_owned_libraries: &[OvenCallerOwnedRustcLibrary],
) -> Result<OvenDirectRustcTestBake, OvenRustcError> {
    let (artifacts, artifact_root, lease) =
        select_stored_direct_rustc_plan(request.store, &request.plan_identity, &request.receipt)?;
    let mut artifact_plan = artifacts.materialize_trusted_store(&artifact_root, &request.receipt.intent)?;
    attach_caller_owned_rustc_libraries(&mut artifact_plan, caller_owned_libraries)?;
    let mut bake = bake_direct_rustc(
        &request.receipt,
        &artifacts,
        &artifact_root,
        &request.rustc,
        &request.source,
        &request.output,
        &request.crate_name,
        &request.edition,
        &request.source_evidence_key,
        true,
        OvenDirectRustcOutputKind::Binary,
        true,
        Some(&artifact_plan),
        false,
        &request.receipt.intent.features,
    )?;
    bake.lease = Some(lease);
    Ok(bake)
}

/// Select a receipt-bound direct-rustc closure and retain its lease until the caller finishes running the binary.
pub fn bake_stored_direct_rustc_run(
    request: &OvenStoredDirectRustcRunRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_stored_direct_rustc_run_with_libraries(request, &[])
}

/// Select a receipt-bound direct-rustc closure and compile a binary while linking explicit caller-owned libraries.
///
/// The additional libraries are not stored artifacts and never participate in native-plan selection. They are the
/// caller-output bridge for already materialized Incan `pub::` dependencies; their exact bytes are recorded in the
/// consumer output sidecar before reuse is allowed.
pub(crate) fn bake_stored_direct_rustc_run_with_libraries(
    request: &OvenStoredDirectRustcRunRequest<'_>,
    caller_owned_libraries: &[OvenCallerOwnedRustcLibrary],
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    let (artifacts, artifact_root, lease) =
        select_stored_direct_rustc_plan(request.store, &request.plan_identity, &request.receipt)?;
    let mut artifact_plan = artifacts.materialize_trusted_store(&artifact_root, &request.receipt.intent)?;
    attach_caller_owned_rustc_libraries(&mut artifact_plan, caller_owned_libraries)?;
    let mut bake = bake_direct_rustc(
        &request.receipt,
        &artifacts,
        &artifact_root,
        &request.rustc,
        &request.source,
        &request.output,
        &request.crate_name,
        &request.edition,
        &request.source_evidence_key,
        false,
        OvenDirectRustcOutputKind::Binary,
        true,
        Some(&artifact_plan),
        false,
        &request.receipt.intent.features,
    )?;
    bake.lease = Some(lease);
    Ok(bake)
}

/// Select a receipt-bound direct-rustc closure and compile one regular caller-owned Rust library without a Cargo
/// consumer process.
pub fn bake_stored_direct_rustc_library(
    request: &OvenStoredDirectRustcLibraryRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_stored_direct_rustc_library_with_libraries(request, &[])
}

/// Select a receipt-bound direct-rustc closure and compile a library while linking explicit caller-owned libraries.
pub(crate) fn bake_stored_direct_rustc_library_with_libraries(
    request: &OvenStoredDirectRustcLibraryRequest<'_>,
    caller_owned_libraries: &[OvenCallerOwnedRustcLibrary],
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    let (artifacts, artifact_root, lease) =
        select_stored_direct_rustc_plan(request.store, &request.plan_identity, &request.receipt)?;
    let mut artifact_plan = artifacts.materialize_trusted_store(&artifact_root, &request.receipt.intent)?;
    attach_caller_owned_rustc_libraries(&mut artifact_plan, caller_owned_libraries)?;
    let mut bake = bake_direct_rustc(
        &request.receipt,
        &artifacts,
        &artifact_root,
        &request.rustc,
        &request.source,
        &request.output,
        &request.crate_name,
        &request.edition,
        &request.source_evidence_key,
        false,
        OvenDirectRustcOutputKind::Library,
        true,
        Some(&artifact_plan),
        false,
        &request.receipt.intent.features,
    )?;
    bake.lease = Some(lease);
    Ok(bake)
}

/// Select the unique stored direct-rustc plan authorized by a generated-project receipt and retain its lease.
///
/// Normal Oven commands select through the receipt's reusable build-unit identity rather than accepting a
/// caller-provided cache location or artifact identity. Generated source remains verified independently at execution,
/// so compatible clean worktrees can reuse one native closure without sharing source or final-output directories.
/// Distinct plans remain an explicit publisher error: silently choosing a "latest" plan would make normal command
/// execution non-deterministic. Byte-identical closures from compatible receipts are equivalent reusable entries;
/// those are collapsed by stable identity while future publication deduplicates them at admission. Matching,
/// integrity verification, and lease acquisition occur under one store-manager lock so policy pruning cannot reclaim
/// a compatible candidate after header selection but before execution begins.
pub fn select_direct_rustc_plan_for_execution(
    store: &OvenStore,
    receipt: &OvenReceipt,
) -> Result<Option<OvenStoreExecutionPayload>, OvenRustcError> {
    receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "receipt",
            message: error.to_string(),
        })?;
    let mut matches = Vec::new();
    for selected in store.select_payloads_matching_for_execution(|manifest| {
        manifest.kind == OvenArtifactKind::DirectRustcPlan
            && manifest.build_unit_identity == receipt.build_unit_identity
            && manifest.intent == receipt.intent
    })? {
        let Ok(plan) = serde_json::from_slice::<OvenRustcArtifactManifest>(&selected.payload) else {
            continue;
        };
        // A prior Alpha publisher may have retained a payload whose `--extern` identifiers no longer satisfy the
        // stricter direct-rustc contract. Ignore it for selection so a corrected explicit publication can coexist
        // until ordinary policy-driven pruning reclaims the inactive entry.
        if plan.validate_shape(&receipt.intent).is_ok() {
            matches.push(selected);
        }
    }
    match matches.len() {
        1 => Ok(matches.pop()),
        0 => Ok(None),
        _ if matches
            .iter()
            .skip(1)
            .all(|candidate| reusable_direct_rustc_entry(&matches[0], candidate)) =>
        {
            matches.sort_by(|left, right| left.manifest.identity.cmp(&right.manifest.identity));
            Ok(Some(matches.remove(0)))
        }
        _ => Err(OvenRustcError::PlanSelection {
            receipt_identity: receipt.identity.clone(),
            message: format!(
                "multiple compatible stored direct-rustc plans are available: {}",
                matches
                    .iter()
                    .map(|entry| entry.manifest.identity.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }),
    }
}

/// Return whether two receipt-compatible selections retain precisely the same reusable immutable closure.
fn reusable_direct_rustc_entry(left: &OvenStoreExecutionPayload, right: &OvenStoreExecutionPayload) -> bool {
    left.manifest.domain == right.manifest.domain
        && left.manifest.payload == right.manifest.payload
        && left.manifest.materialized_files == right.manifest.materialized_files
}

/// Select the unique stored direct-rustc plan identity authorized by a generated-project receipt.
///
/// This compatibility helper drops the returned execution lease with the identity. Normal command execution must
/// use [`select_direct_rustc_plan_for_execution`] so a concurrent bounded-policy publication cannot prune a chosen
/// plan before the caller starts using it.
pub fn select_direct_rustc_plan_identity(store: &OvenStore, receipt: &OvenReceipt) -> Result<String, OvenRustcError> {
    select_direct_rustc_plan_for_execution(store, receipt)?
        .map(|selected| selected.manifest.identity)
        .ok_or_else(|| OvenRustcError::PlanSelection {
            receipt_identity: receipt.identity.clone(),
            message: "no compatible stored direct-rustc plan is available".to_string(),
        })
}

/// Select a stored plan only when it matches the requested reusable build unit and return its closure with a live
/// lease.
fn select_stored_direct_rustc_plan(
    store: &OvenStore,
    plan_identity: &str,
    receipt: &OvenReceipt,
) -> Result<(OvenRustcArtifactManifest, PathBuf, OvenStoreLease), OvenRustcError> {
    receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "receipt",
            message: error.to_string(),
        })?;
    let (manifest, artifact_root, payload, lease) = store.select_payload_for_execution(plan_identity)?;
    if manifest.kind != OvenArtifactKind::DirectRustcPlan {
        return Err(OvenRustcError::InvalidStoredPlan {
            identity: manifest.identity,
            message: "selected artifact kind is not direct_rustc_plan".to_string(),
        });
    }
    if manifest.build_unit_identity != receipt.build_unit_identity || manifest.intent != receipt.intent {
        return Err(OvenRustcError::InvalidStoredPlan {
            identity: manifest.identity,
            message: "selected artifact is not authorized by the requested Oven build unit".to_string(),
        });
    }
    let artifacts = serde_json::from_slice::<OvenRustcArtifactManifest>(&payload).map_err(|error| {
        OvenRustcError::InvalidStoredPlan {
            identity: plan_identity.to_string(),
            message: format!("stored payload is not an Oven Rust artifact manifest: {error}"),
        }
    })?;
    Ok((artifacts, artifact_root, lease))
}

/// Run one compiler-suite doctest root directly with Rustdoc, without a Cargo consumer process.
///
/// Rustdoc creates and destroys the individual doctest executables internally. Oven therefore verifies the same
/// receipt-bound source and immutable closure as direct-rustc, pins Rustdoc to the selected Rustc sysroot, and keeps
/// all transient files in a caller-owned directory while the enclosing suite lease remains live.
pub(crate) fn run_trusted_rustdoc_test(
    request: &OvenTrustedRustdocTestRequest<'_>,
) -> Result<OvenRustdocTestReport, OvenRustcError> {
    request
        .receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "receipt",
            message: error.to_string(),
        })?;
    verify_rustc_identity(request.rustc, &request.receipt.intent.toolchain)?;
    validate_rust_identifier(request.crate_name)?;
    validate_edition(request.edition)?;
    let source = verified_regular_file(request.source, "source")?;
    let source_bytes = fs::read(&source).map_err(|source_error| OvenRustcError::Io {
        path: source.clone(),
        source: source_error,
    })?;
    let source_digest = digest_bytes(&source_bytes);
    let expected_source_digest = request
        .receipt
        .sources
        .supplemental_digests
        .get(request.source_evidence_key.trim())
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "source evidence",
            message: format!("receipt does not declare `{}`", request.source_evidence_key),
        })?;
    if expected_source_digest != &source_digest {
        return Err(OvenRustcError::SourceEvidenceMismatch {
            key: request.source_evidence_key.to_string(),
            expected: expected_source_digest.clone(),
            actual: source_digest,
        });
    }
    let artifacts = request.artifacts.for_source_evidence(request.source_evidence_key)?;
    // `Option::unwrap_or` evaluates its fallback eagerly.  Indexed compiler-suite doctests provide a composed
    // foundation plan while their thin shard deliberately has no local third-party closure, so touching that
    // fallback would fail before Rustdoc can use the supplied plan.  Keep legacy single-root materialization lazy.
    let plan = match request.artifact_plan {
        Some(plan) => plan.clone(),
        None => artifacts.materialize_trusted_store(request.artifact_root, &request.receipt.intent)?,
    };
    let temporary_directory = caller_temporary_directory(request.temporary_directory, request.artifact_root)?;
    let test_run_directory = match plan.compile_environment.get("CARGO_MANIFEST_DIR") {
        Some(value) => resolve_compile_environment_value("CARGO_MANIFEST_DIR", value, &source)?,
        None => source
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| OvenRustcError::InvalidInput {
                field: "source",
                message: format!("{} has no parent directory for Rustdoc", source.display()),
            })?,
    };
    let rustdoc = rustdoc_for_rustc(request.rustc)?;
    let mut command = Command::new(&rustdoc);
    command
        .arg("--test")
        .arg("--target")
        .arg(&request.receipt.intent.target)
        .arg(format!("--edition={}", request.edition))
        .arg("--crate-name")
        .arg(request.crate_name)
        .arg("--test-run-directory")
        .arg(&test_run_directory)
        .arg(&source);
    apply_oven_profile(&mut command, &request.receipt.intent.profile);
    if request.is_proc_macro {
        // Cargo's proc-macro Rustdoc invocation supplies both the crate type and the sysroot-provided
        // `proc_macro` extern. Without the latter, Rustdoc treats the source as an ordinary library and rejects
        // `use proc_macro::…` even though the root is receipt-classified as a proc macro.
        command
            .arg("--crate-type")
            .arg("proc-macro")
            .arg("--extern")
            .arg("proc_macro");
    }
    command.current_dir(&test_run_directory);
    clear_inherited_cargo_environment(&mut command);
    command.env("TMPDIR", &temporary_directory);
    for (name, value) in &plan.compile_environment {
        let value = resolve_compile_environment_value(name, value, &source)?;
        command.env(name, value);
    }
    if request.prefer_dynamic {
        // Rustdoc launches the generated doctest runner itself. That runner can link a caller-owned workspace dylib
        // such as the compiler library, so the selected toolchain libraries alone are not a complete runtime
        // closure. Carry only caller-owned dynamic-library directories: immutable store identities contain `sha256:`
        // and must remain direct `-L` compiler inputs rather than ambiguous path-list environment segments.
        let (name, value) = rustc_dynamic_library_environment_with_caller_owned_paths(request.rustc, &plan)?;
        command.args(["-C", "rpath"]);
        command.env(name, value);
    }
    for feature in request.features {
        command.arg("--cfg").arg(format!("feature={feature:?}"));
    }
    for path in &plan.dependency_search_paths {
        command.arg("-L").arg(format!("dependency={}", path.display()));
    }
    for path in &plan.native_search_paths {
        command.arg("-L").arg(format!("native={}", path.display()));
    }
    for (crate_name, path) in &plan.externs {
        command.arg("--extern").arg(format!("{crate_name}={}", path.display()));
    }
    let (output, timed_out) = run_supervised_rustdoc_command(command, &rustdoc, request.timeout)?;
    let mut transcript = combined_process_output(&output.stdout, &output.stderr);
    if timed_out {
        if !transcript.ends_with('\n') && !transcript.is_empty() {
            transcript.push('\n');
        }
        if let Some(timeout) = request.timeout {
            transcript.push_str(&format!(
                "Oven Rustdoc execution group timed out after {}ms (source: {})\n",
                timeout.as_millis(),
                source.display(),
            ));
        }
    }
    if timed_out || !output.status.success() {
        return Err(OvenRustcError::RustdocTestFailed { output: transcript });
    }
    Ok(OvenRustdocTestReport { output: transcript })
}

/// Run Rustdoc and its generated doctest children inside the same bounded process group as native suite roots.
fn run_supervised_rustdoc_command(
    mut command: Command,
    rustdoc: &Path,
    timeout: Option<Duration>,
) -> Result<(std::process::Output, bool), OvenRustcError> {
    let Some(timeout) = timeout else {
        let output = command.output().map_err(|source| OvenRustcError::Io {
            path: rustdoc.to_path_buf(),
            source,
        })?;
        return Ok((output, false));
    };

    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    isolate_process_group(&mut command);
    let mut child = command.spawn().map_err(|source| OvenRustcError::Io {
        path: rustdoc.to_path_buf(),
        source,
    })?;
    let mut stdout = child.stdout.take().ok_or_else(|| OvenRustcError::Io {
        path: rustdoc.to_path_buf(),
        source: io::Error::other("Rustdoc stdout was not piped"),
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| OvenRustcError::Io {
        path: rustdoc.to_path_buf(),
        source: io::Error::other("Rustdoc stderr was not piped"),
    })?;
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes)?;
        Ok::<_, io::Error>(bytes)
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes)?;
        Ok::<_, io::Error>(bytes)
    });
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait().map_err(|source| OvenRustcError::Io {
            path: rustdoc.to_path_buf(),
            source,
        })? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                timed_out = true;
                break terminate_process_group(&mut child).map_err(|source| OvenRustcError::Io {
                    path: rustdoc.to_path_buf(),
                    source,
                })?;
            }
            None => thread::sleep(Duration::from_millis(1)),
        }
    };
    let stdout = join_rustdoc_output_reader(stdout_reader, rustdoc, "stdout")?;
    let stderr = join_rustdoc_output_reader(stderr_reader, rustdoc, "stderr")?;
    Ok((std::process::Output { status, stdout, stderr }, timed_out))
}

/// Join one Rustdoc pipe reader and retain the executable path in any I/O diagnostic.
fn join_rustdoc_output_reader(
    reader: thread::JoinHandle<Result<Vec<u8>, io::Error>>,
    rustdoc: &Path,
    stream: &str,
) -> Result<Vec<u8>, OvenRustcError> {
    reader
        .join()
        .map_err(|_| OvenRustcError::Io {
            path: rustdoc.to_path_buf(),
            source: io::Error::other(format!("Rustdoc {stream} reader panicked")),
        })?
        .map_err(|source| OvenRustcError::Io {
            path: rustdoc.to_path_buf(),
            source,
        })
}

/// Compile one receipt-bound generated Rust test target without a Cargo consumer process.
pub fn bake_direct_rustc_test(request: &OvenDirectRustcTestRequest) -> Result<OvenDirectRustcTestBake, OvenRustcError> {
    bake_direct_rustc(
        &request.receipt,
        &request.artifacts,
        &request.artifact_root,
        &request.rustc,
        &request.source,
        &request.output,
        &request.crate_name,
        &request.edition,
        &request.source_evidence_key,
        true,
        OvenDirectRustcOutputKind::Binary,
        false,
        None,
        false,
        &request.receipt.intent.features,
    )
}

/// Compile one compiler-suite test root from a caller-held leased store artifact without a Cargo consumer process.
pub(crate) fn bake_trusted_direct_rustc_test(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
) -> Result<OvenDirectRustcTestBake, OvenRustcError> {
    bake_direct_rustc(
        request.receipt,
        request.artifacts,
        request.artifact_root,
        request.rustc,
        request.source,
        request.output,
        request.crate_name,
        request.edition,
        request.source_evidence_key,
        true,
        OvenDirectRustcOutputKind::Binary,
        true,
        request.artifact_plan,
        request.prefer_dynamic,
        request.features,
    )
}

/// Compile one regular Rust library from selected Oven foundations without a Cargo target directory.
///
/// The library is caller-owned ephemeral materialization. Its reusable sidecar still binds the exact receipt,
/// selected artifact manifest, source digest, resolved feature set, and library output kind before a later direct
/// Rustc target may link it.
pub(crate) fn bake_trusted_direct_rustc_library(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_direct_rustc(
        request.receipt,
        request.artifacts,
        request.artifact_root,
        request.rustc,
        request.source,
        request.output,
        request.crate_name,
        request.edition,
        request.source_evidence_key,
        false,
        OvenDirectRustcOutputKind::Library,
        true,
        request.artifact_plan,
        request.prefer_dynamic,
        request.features,
    )
}

/// Compile one regular dynamic Rust library from selected Oven foundations without a Cargo target directory.
///
/// The compiler suite uses this only for its shared top-level compiler library. Linking that one expensive crate
/// dynamically keeps each independently executed integration-test root from embedding another static copy.
pub(crate) fn bake_trusted_direct_rustc_dylib(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_direct_rustc(
        request.receipt,
        request.artifacts,
        request.artifact_root,
        request.rustc,
        request.source,
        request.output,
        request.crate_name,
        request.edition,
        request.source_evidence_key,
        false,
        OvenDirectRustcOutputKind::Dylib,
        true,
        request.artifact_plan,
        request.prefer_dynamic,
        request.features,
    )
}

/// Compile one procedural-macro library from selected Oven foundations without a Cargo target directory.
///
/// A workspace materialization DAG treats the resulting dynamic library as a caller-owned `--extern` input for
/// later direct-Rustc steps. Its receipt still binds the exact source, compiler, features, and immutable foundation.
pub(crate) fn bake_trusted_direct_rustc_proc_macro(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_direct_rustc(
        request.receipt,
        request.artifacts,
        request.artifact_root,
        request.rustc,
        request.source,
        request.output,
        request.crate_name,
        request.edition,
        request.source_evidence_key,
        false,
        OvenDirectRustcOutputKind::ProcMacro,
        true,
        request.artifact_plan,
        request.prefer_dynamic,
        request.features,
    )
}

/// Compile one compiler-suite binary root from a caller-held leased store artifact without a Cargo consumer process.
pub(crate) fn bake_trusted_direct_rustc_run(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_direct_rustc(
        request.receipt,
        request.artifacts,
        request.artifact_root,
        request.rustc,
        request.source,
        request.output,
        request.crate_name,
        request.edition,
        request.source_evidence_key,
        false,
        OvenDirectRustcOutputKind::Binary,
        true,
        request.artifact_plan,
        request.prefer_dynamic,
        request.features,
    )
}

/// Compile one receipt-bound generated Rust binary without a Cargo consumer process.
pub fn bake_direct_rustc_run(request: &OvenDirectRustcRunRequest) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_direct_rustc(
        &request.receipt,
        &request.artifacts,
        &request.artifact_root,
        &request.rustc,
        &request.source,
        &request.output,
        &request.crate_name,
        &request.edition,
        &request.source_evidence_key,
        false,
        OvenDirectRustcOutputKind::Binary,
        false,
        None,
        false,
        &request.receipt.intent.features,
    )
}

/// Project a pre-materialized trusted plan onto the receipt-authorized direct extern roots.
///
/// Compiler-suite callers retain an already checked plan to avoid a second traversal of large immutable
/// foundations. That plan can include caller-owned library outputs, so it cannot simply be re-materialized from the
/// filtered manifest. Remove only immutable manifest externs that the source-evidence projection excludes; the
/// caller-owned additions remain exact inputs and continue to participate in output reuse evidence.
fn trusted_artifact_plan_for_source(
    plan: &OvenRustcArtifactPlan,
    declared_artifacts: &OvenRustcArtifactManifest,
    selected_artifacts: &OvenRustcArtifactManifest,
) -> OvenRustcArtifactPlan {
    let declared_names = declared_artifacts
        .externs
        .iter()
        .map(|artifact| artifact.crate_name.as_str())
        .collect::<BTreeSet<_>>();
    let selected_names = selected_artifacts
        .externs
        .iter()
        .map(|artifact| artifact.crate_name.as_str())
        .collect::<BTreeSet<_>>();
    // An expose-extern caller library is identified by its crate-name reuse evidence. This matters when a caller
    // deliberately declares the same crate name as a compiler-private helper retained in the complete manifest:
    // source projection must remove the helper while preserving the caller's separately verified output.
    let caller_owned_extern_names = plan
        .caller_owned_library_digests
        .keys()
        .filter(|key| !key.starts_with("transitive:"))
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut selected_plan = plan.clone();
    let mut excluded_direct_root_parents = BTreeSet::new();
    let mut selected_direct_root_parents = BTreeSet::new();
    for (crate_name, path) in &plan.externs {
        if !declared_names.contains(crate_name.as_str()) {
            continue;
        }
        let Some(parent) = path.parent() else {
            continue;
        };
        if selected_names.contains(crate_name.as_str()) || caller_owned_extern_names.contains(crate_name.as_str()) {
            selected_direct_root_parents.insert(parent.to_path_buf());
        } else {
            excluded_direct_root_parents.insert(parent.to_path_buf());
        }
    }
    selected_plan.externs.retain(|(crate_name, _)| {
        !declared_names.contains(crate_name.as_str())
            || selected_names.contains(crate_name.as_str())
            || caller_owned_extern_names.contains(crate_name.as_str())
    });
    selected_plan.dependency_search_paths.retain(|search_path| {
        !excluded_direct_root_parents.contains(search_path) || selected_direct_root_parents.contains(search_path)
    });
    selected_plan
}

/// Project a verified, already-materialized direct-Rustc plan onto one receipt-authorized source root.
///
/// Callers that add a caller-owned registry leaf must make that decision against this projection, rather than the
/// complete Loaf plan. A complete Loaf deliberately retains compiler-private helpers such as the vocabulary
/// serializer; treating one of those helpers as a public caller extern can either hide a declared dependency or
/// expose a second incompatible Rust crate identity.
pub(crate) fn trusted_artifact_plan_for_source_evidence(
    plan: &OvenRustcArtifactPlan,
    artifacts: &OvenRustcArtifactManifest,
    source_evidence_key: &str,
) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
    let selected_artifacts = artifacts.for_source_evidence(source_evidence_key)?;
    Ok(trusted_artifact_plan_for_source(plan, artifacts, &selected_artifacts))
}

/// Return the direct extern names exposed to one receipt-authorized source root.
pub(crate) fn direct_rustc_source_extern_names(
    artifacts: &OvenRustcArtifactManifest,
    source_evidence_key: &str,
) -> Result<BTreeSet<String>, OvenRustcError> {
    Ok(artifacts
        .for_source_evidence(source_evidence_key)?
        .externs
        .into_iter()
        .map(|artifact| artifact.crate_name)
        .collect())
}

/// Compile one receipt-bound generated Rust source with either the libtest or binary direct-rustc mode.
#[allow(clippy::too_many_arguments)]
fn bake_direct_rustc(
    receipt: &OvenReceipt,
    artifacts: &OvenRustcArtifactManifest,
    artifact_root: &Path,
    rustc: &Path,
    source: &Path,
    output: &Path,
    crate_name: &str,
    edition: &str,
    source_evidence_key: &str,
    test_harness: bool,
    output_kind: OvenDirectRustcOutputKind,
    trusted_store: bool,
    trusted_artifact_plan: Option<&OvenRustcArtifactPlan>,
    prefer_dynamic: bool,
    features: &[String],
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "receipt",
            message: error.to_string(),
        })?;
    verify_rustc_identity(rustc, &receipt.intent.toolchain)?;
    validate_rust_identifier(crate_name)?;
    validate_edition(edition)?;
    let source = verified_regular_file(source, "source")?;
    let output = caller_output_path(output, artifact_root)?;
    let source_bytes = fs::read(&source).map_err(|source_error| OvenRustcError::Io {
        path: source.clone(),
        source: source_error,
    })?;
    let source_digest = digest_bytes(&source_bytes);
    let expected_source_digest = receipt
        .sources
        .supplemental_digests
        .get(source_evidence_key.trim())
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "source evidence",
            message: format!("receipt does not declare `{source_evidence_key}`"),
        })?;
    if expected_source_digest != &source_digest {
        return Err(OvenRustcError::SourceEvidenceMismatch {
            key: source_evidence_key.to_string(),
            expected: expected_source_digest.clone(),
            actual: source_digest,
        });
    }
    let selected_artifacts = artifacts.for_source_evidence(source_evidence_key)?;
    let parent = output.parent().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "output",
        message: "must have a parent directory".to_string(),
    })?;
    fs::create_dir_all(parent).map_err(|source_error| OvenRustcError::Io {
        path: parent.to_path_buf(),
        source: source_error,
    })?;
    let output_receipt = OvenDirectRustcOutputReceipt {
        schema_version: OVEN_DIRECT_RUSTC_OUTPUT_RECEIPT_SCHEMA_VERSION,
        receipt_identity: receipt.identity.clone(),
        artifact_manifest_digest: digest_bytes(&serde_json::to_vec(&selected_artifacts).map_err(|error| {
            OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!("cannot serialize verified manifest identity: {error}"),
            }
        })?),
        source_digest: source_digest.clone(),
        crate_name: crate_name.to_string(),
        edition: edition.to_string(),
        features: features.to_vec(),
        test_harness,
        prefer_dynamic,
        output_kind: output_kind.receipt_value().to_string(),
        caller_owned_library_digests: trusted_artifact_plan
            .map(|plan| plan.caller_owned_library_digests.clone())
            .unwrap_or_default(),
    };
    if caller_output_is_reusable(&output, &output_receipt) {
        let output_digest = digest_regular_file(&output, "output")?;
        return Ok(OvenDirectRustcBake {
            source_digest,
            output,
            output_digest,
            cargo_process_started: false,
            reused: true,
            lease: None,
        });
    }

    // Publication has already performed full content verification for a selected store entry. On a genuine caller
    // output miss, normal consumers prove file shape/containment under their active lease rather than rehash every
    // dependency; externally supplied plans retain the stronger byte-for-byte materialization path.
    let plan = if let Some(plan) = trusted_artifact_plan {
        trusted_artifact_plan_for_source(plan, artifacts, &selected_artifacts)
    } else if trusted_store {
        selected_artifacts.materialize_trusted_store(artifact_root, &receipt.intent)?
    } else {
        selected_artifacts.materialize(artifact_root, &receipt.intent)?
    };

    let mut command = Command::new(rustc);
    if test_harness {
        command.arg("--test");
    }
    match output_kind {
        OvenDirectRustcOutputKind::Binary => {}
        OvenDirectRustcOutputKind::Library => {
            command.args(["--crate-type", "lib"]);
        }
        OvenDirectRustcOutputKind::Dylib => {
            command.args(["--crate-type", "dylib"]);
        }
        OvenDirectRustcOutputKind::ProcMacro => {
            // `proc_macro` is supplied by the selected Rustc sysroot rather than by a Cargo-produced artifact.
            // Naming the crate explicitly is still required for edition-2018-and-later sources that import it with
            // `use proc_macro::…`; unlike an `extern crate proc_macro` declaration, that import does not cause
            // Rustc to infer the sysroot dependency.
            command.args(["--crate-type", "proc-macro", "--extern", "proc_macro"]);
        }
    }
    if prefer_dynamic {
        // Cargo emits both flags for a proc-macro libtest. `proc_macro` is provided by the receipt-selected Rust
        // toolchain sysroot rather than a Cargo target artifact, so it is intentionally not represented as a stored
        // third-party `--extern` file. `rpath` is required as well: compiler-suite children can pass a dynamically
        // linked caller-owned CLI through a shell script, and macOS strips `DYLD_*` values when it starts its system
        // shell. The selected Rustc and caller-owned `-L dependency` paths define the embedded loader closure.
        command.args(["-C", "prefer-dynamic", "-C", "rpath", "--extern", "proc_macro"]);
    }
    command
        .arg("--target")
        .arg(&receipt.intent.target)
        .arg(format!("--edition={edition}"))
        .arg("--crate-name")
        .arg(crate_name)
        .arg("--error-format=json")
        .arg(&source)
        .arg("-o")
        .arg(&output);
    apply_oven_profile(&mut command, &receipt.intent.profile);
    clear_inherited_cargo_environment(&mut command);
    for (name, value) in &plan.compile_environment {
        let value = resolve_compile_environment_value(name, value, &source)?;
        command.env(name, value);
    }
    for feature in features {
        command.arg("--cfg").arg(format!("feature={feature:?}"));
    }
    for path in &plan.dependency_search_paths {
        command.arg("-L").arg(format!("dependency={}", path.display()));
    }
    for path in &plan.native_search_paths {
        command.arg("-L").arg(format!("native={}", path.display()));
    }
    for (crate_name, path) in &plan.externs {
        command.arg("--extern").arg(format!("{crate_name}={}", path.display()));
    }
    let output_result = command.output().map_err(|source_error| OvenRustcError::Io {
        path: rustc.to_path_buf(),
        source: source_error,
    })?;
    if !output_result.status.success() {
        return Err(OvenRustcError::CompilationFailed {
            report: parse_rustc_diagnostics(&output_result.stdout, &output_result.stderr).with_invocation(&command),
        });
    }
    verified_regular_file(&output, "output")?;
    let output_digest = digest_regular_file(&output, "output")?;
    write_caller_output_receipt(&output, &output_receipt)?;
    Ok(OvenDirectRustcBake {
        source_digest,
        output,
        output_digest,
        cargo_process_started: false,
        reused: false,
        lease: None,
    })
}

/// Return the caller-owned sidecar path without accepting an output that lacks a safe file name.
fn caller_output_receipt_path(output: &Path) -> Result<PathBuf, OvenRustcError> {
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "output",
            message: "must end in a UTF-8 file name to retain Oven reuse evidence".to_string(),
        })?;
    Ok(output.with_file_name(format!("{name}.oven-output.json")))
}

/// Check only a fully matching regular output/sidecar pair; malformed or interrupted sidecars trigger a rebuild.
fn caller_output_is_reusable(output: &Path, expected: &OvenDirectRustcOutputReceipt) -> bool {
    if verified_regular_file(output, "output").is_err() {
        return false;
    }
    let Ok(receipt_path) = caller_output_receipt_path(output) else {
        return false;
    };
    let Ok(bytes) = fs::read(&receipt_path) else {
        return false;
    };
    serde_json::from_slice::<OvenDirectRustcOutputReceipt>(&bytes)
        .ok()
        .as_ref()
        == Some(expected)
}

/// Atomically publish reuse evidence only after rustc has produced and verified its regular caller-owned output.
fn write_caller_output_receipt(output: &Path, receipt: &OvenDirectRustcOutputReceipt) -> Result<(), OvenRustcError> {
    let path = caller_output_receipt_path(output)?;
    let parent = path.parent().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "output",
        message: "reuse evidence has no parent directory".to_string(),
    })?;
    let bytes = serde_json::to_vec(receipt).map_err(|error| OvenRustcError::InvalidInput {
        field: "output receipt",
        message: format!("cannot serialize reuse evidence: {error}"),
    })?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|name| name.to_str()).unwrap_or("oven-output"),
        std::process::id(),
    ));
    fs::write(&temporary, bytes).map_err(|source| OvenRustcError::Io {
        path: temporary.clone(),
        source,
    })?;
    fs::rename(&temporary, &path).map_err(|source| OvenRustcError::Io { path, source })
}

/// Clear ambient Cargo state before direct consumer execution; the explicit compiler path remains authoritative.
pub(crate) fn clear_inherited_cargo_environment(command: &mut Command) {
    for (name, _) in env::vars_os().filter(|(name, _)| name == "CARGO" || name.to_string_lossy().starts_with("CARGO_"))
    {
        command.env_remove(name);
    }
    for name in ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"] {
        command.env_remove(name);
    }
}

/// Apply the named Oven compiler-suite contract to a direct compiler invocation.
///
/// Cargo sees the matching manifest-declared profile only inside the explicit bootstrap publisher. Every normal
/// consumer uses this direct-rustc representation instead, so its flags are deliberate receipt semantics rather
/// than inherited Cargo environment state.
fn apply_oven_profile(command: &mut Command, profile: &str) {
    if profile == OVEN_COMPILER_TEST_PROFILE {
        command.args([
            "-C",
            "debuginfo=0",
            "-C",
            "strip=debuginfo",
            "-C",
            "debug-assertions=on",
            "-C",
            "overflow-checks=on",
        ]);
    }
}

/// Derive the selected compiler's stable version identity and require an exact receipt match before compilation.
fn verify_rustc_identity(rustc: &Path, expected: &str) -> Result<(), OvenRustcError> {
    let actual = rustc_identity(rustc)?;
    if actual == expected {
        return Ok(());
    }
    Err(OvenRustcError::ToolchainMismatch {
        expected: expected.to_string(),
        actual,
    })
}

/// Resolve the active Rust compiler without involving Cargo or a Cargo target directory.
///
/// An explicit `RUSTC` must be a regular executable file, not a shell fragment. When it is absent, the Rustup
/// toolchain resolver supplies the compiler path; that remains separate from the explicit `legacy_cargo` publisher.
pub fn resolve_active_rustc() -> Result<PathBuf, OvenRustcError> {
    if let Some(path) = env::var_os("RUSTC").filter(|path| !path.is_empty()) {
        return verified_regular_file(Path::new(&path), "RUSTC");
    }
    let mut command = Command::new("rustup");
    command.args(["which", "rustc"]);
    clear_inherited_cargo_environment(&mut command);
    let output = command.output().map_err(|source| OvenRustcError::Io {
        path: PathBuf::from("rustup"),
        source,
    })?;
    if !output.status.success() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: "rustup could not locate the active Rust compiler; set RUSTC to an explicit compiler path"
                .to_string(),
        });
    }
    let reported = String::from_utf8(output.stdout).map_err(|error| OvenRustcError::InvalidInput {
        field: "rustc",
        message: format!("rustup reported a non-UTF-8 compiler path: {error}"),
    })?;
    let reported = reported.trim();
    if reported.is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: "rustup reported an empty compiler path".to_string(),
        });
    }
    verified_regular_file(Path::new(reported), "rustc")
}

/// Read one regular Rust compiler's stable `--version` identity without invoking Cargo.
pub fn rustc_identity(rustc: &Path) -> Result<String, OvenRustcError> {
    let rustc = verified_regular_file(rustc, "rustc")?;
    let mut command = Command::new(&rustc);
    command.arg("--version");
    clear_inherited_cargo_environment(&mut command);
    let output = command.output().map_err(|source| OvenRustcError::Io {
        path: rustc.clone(),
        source,
    })?;
    if !output.status.success() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: "must report a successful `--version` identity".to_string(),
        });
    }
    let actual = String::from_utf8(output.stdout).map_err(|error| OvenRustcError::InvalidInput {
        field: "rustc",
        message: format!("reported non-UTF-8 `--version` output: {error}"),
    })?;
    let actual = actual.trim();
    if actual.is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: "reported an empty `--version` identity".to_string(),
        });
    }
    Ok(actual.to_string())
}

/// Read the active compiler's host target from `rustc -vV` without consulting Cargo metadata.
pub fn rustc_host_target(rustc: &Path) -> Result<String, OvenRustcError> {
    let rustc = verified_regular_file(rustc, "rustc")?;
    let mut command = Command::new(&rustc);
    command.arg("-vV");
    clear_inherited_cargo_environment(&mut command);
    let output = command.output().map_err(|source| OvenRustcError::Io {
        path: rustc.clone(),
        source,
    })?;
    if !output.status.success() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: "must report a successful `-vV` host target".to_string(),
        });
    }
    let output = String::from_utf8(output.stdout).map_err(|error| OvenRustcError::InvalidInput {
        field: "rustc",
        message: format!("reported non-UTF-8 `-vV` output: {error}"),
    })?;
    output
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "rustc",
            message: "did not report a host target in `-vV` output".to_string(),
        })
}

/// Resolve the selected compiler's sysroot without consulting Cargo.
fn rustc_sysroot(rustc: &Path) -> Result<PathBuf, OvenRustcError> {
    let rustc = verified_regular_file(rustc, "rustc")?;
    let mut command = Command::new(&rustc);
    command.args(["--print", "sysroot"]);
    clear_inherited_cargo_environment(&mut command);
    let output = command.output().map_err(|source| OvenRustcError::Io {
        path: rustc.clone(),
        source,
    })?;
    if !output.status.success() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: "must report a successful `--print sysroot`".to_string(),
        });
    }
    let sysroot = String::from_utf8(output.stdout).map_err(|error| OvenRustcError::InvalidInput {
        field: "rustc",
        message: format!("reported a non-UTF-8 sysroot: {error}"),
    })?;
    let sysroot = PathBuf::from(sysroot.trim());
    if !sysroot.is_dir() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: format!("reported a missing or non-directory sysroot {}", sysroot.display()),
        });
    }
    Ok(sysroot)
}

/// Resolve the Rustdoc executable from the same verified sysroot as a receipt-selected compiler.
pub(crate) fn rustdoc_for_rustc(rustc: &Path) -> Result<PathBuf, OvenRustcError> {
    let rustdoc = rustc_sysroot(rustc)?.join("bin/rustdoc");
    verified_regular_file(&rustdoc, "rustdoc")
}

/// Derive the selected compiler's dynamic-library search environment without consulting Cargo.
///
/// Direct `--test` compilation of a proc-macro crate uses Cargo's `-C prefer-dynamic` convention. Its caller-owned
/// test binary must therefore receive the matching toolchain standard-library directory for both inventory and test
/// execution; ambient Cargo-provided dynamic-library state is not trusted.
pub(crate) fn rustc_dynamic_library_environment(rustc: &Path) -> Result<(String, String), OvenRustcError> {
    let sysroot = rustc_sysroot(rustc)?;
    let host_target = rustc_host_target(rustc)?;
    let target_libraries = sysroot.join("lib/rustlib").join(host_target).join("lib");
    let toolchain_libraries = sysroot.join("lib");
    if !target_libraries.is_dir() || !toolchain_libraries.is_dir() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: format!(
                "sysroot {} does not contain direct-test dynamic library directories",
                sysroot.display()
            ),
        });
    }
    let value =
        env::join_paths([target_libraries, toolchain_libraries]).map_err(|error| OvenRustcError::InvalidInput {
            field: "rustc",
            message: format!("cannot construct direct-test dynamic library search path: {error}"),
        })?;
    let value = value.into_string().map_err(|_| OvenRustcError::InvalidInput {
        field: "rustc",
        message: "direct-test dynamic library search path is not valid UTF-8".to_string(),
    })?;
    let key = if cfg!(target_os = "macos") {
        "DYLD_FALLBACK_LIBRARY_PATH"
    } else if cfg!(target_os = "windows") {
        "PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    Ok((key.to_string(), value))
}

/// Extend the receipt-selected dynamic toolchain closure with caller-owned direct-Rustc dynamic-library directories.
///
/// Rustdoc owns its generated runner binary and launches it before Oven can attach a separate environment. The
/// caller-owned paths are already validated by the direct-Rustc plan, so they are safe to transport alongside the
/// selected toolchain directories rather than relying on Cargo's ambient loader setup. Store artifact directories
/// are deliberately excluded: their opaque `sha256:` identities are not valid Unix path-list segments and Rustdoc
/// already receives them as individual direct compiler search paths.
fn rustc_dynamic_library_environment_with_caller_owned_paths(
    rustc: &Path,
    plan: &OvenRustcArtifactPlan,
) -> Result<(String, String), OvenRustcError> {
    let (name, toolchain_value) = rustc_dynamic_library_environment(rustc)?;
    let mut paths = BTreeSet::new();
    for (crate_name, artifact) in &plan.externs {
        if !plan.caller_owned_library_digests.contains_key(crate_name) {
            continue;
        }
        let extension = artifact.extension().and_then(|extension| extension.to_str());
        if !matches!(extension, Some("dylib" | "so" | "dll")) {
            continue;
        }
        let parent = artifact.parent().ok_or_else(|| OvenRustcError::InvalidInput {
            field: "caller-owned dynamic library",
            message: format!("{} has no parent directory", artifact.display()),
        })?;
        paths.insert(parent.to_path_buf());
    }
    paths.extend(env::split_paths(&toolchain_value));
    let value = env::join_paths(paths)
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "rustc dynamic library environment",
            message: format!("cannot construct direct-Rustc dynamic library search path: {error}"),
        })?
        .into_string()
        .map_err(|_| OvenRustcError::InvalidInput {
            field: "rustc dynamic library environment",
            message: "direct-Rustc dynamic library search path is not valid UTF-8".to_string(),
        })?;
    Ok((name, value))
}

/// Collect every manifest-recorded artifact by safe relative path for directory completeness checks.
fn expected_artifacts(manifest: &OvenRustcArtifactManifest) -> Result<BTreeMap<String, String>, OvenRustcError> {
    let mut expected = BTreeMap::new();
    for artifact in manifest
        .externs
        .iter()
        .map(|artifact| (artifact.relative_path.as_str(), artifact.digest.as_str()))
        .chain(
            manifest
                .supporting_artifacts
                .iter()
                .map(|artifact| (artifact.relative_path.as_str(), artifact.digest.as_str())),
        )
        .chain(manifest.vocab_auxiliary_targets.iter().flat_map(|auxiliary| {
            auxiliary
                .externs
                .iter()
                .map(|artifact| (artifact.relative_path.as_str(), artifact.digest.as_str()))
        }))
    {
        let normalized = normalized_relative_path(artifact.0, "artifact")?;
        if expected.insert(normalized, artifact.1.to_string()).is_some() {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: "declares one relative artifact path more than once".to_string(),
            });
        }
    }
    Ok(expected)
}

/// Verify declared rustc search directories and refuse any regular file not named in the manifest.
fn materialize_search_paths(
    root: &Path,
    paths: &[String],
    kind: &'static str,
    expected: &BTreeMap<String, String>,
) -> Result<Vec<PathBuf>, OvenRustcError> {
    let mut materialized = Vec::new();
    let mut seen = BTreeSet::new();
    for relative in paths {
        let normalized = normalized_relative_path(relative, kind)?;
        if !seen.insert(normalized.clone()) {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!("declares duplicate {kind} path `{relative}`"),
            });
        }
        let path = safe_path(root, &normalized, kind)?;
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
            path: path.clone(),
            source,
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(OvenRustcError::InvalidArtifactPath {
                kind,
                path,
                message: "must be a non-symlink directory".to_string(),
            });
        }
        let mut files_in_directory = 0_u64;
        for child in fs::read_dir(&path).map_err(|source| OvenRustcError::Io {
            path: path.clone(),
            source,
        })? {
            let child = child.map_err(|source| OvenRustcError::Io {
                path: path.clone(),
                source,
            })?;
            let child_path = child.path();
            let child_metadata = fs::symlink_metadata(&child_path).map_err(|source| OvenRustcError::Io {
                path: child_path.clone(),
                source,
            })?;
            if !child_metadata.is_file() || child_metadata.file_type().is_symlink() {
                return Err(OvenRustcError::InvalidArtifactPath {
                    kind,
                    path: child_path,
                    message: "search directories may contain only regular files".to_string(),
                });
            }
            let relative_child = child_path
                .strip_prefix(root)
                .map_err(|_| OvenRustcError::InvalidArtifactPath {
                    kind,
                    path: child_path.clone(),
                    message: "resolved path escaped artifact root".to_string(),
                })?
                .to_string_lossy()
                .replace('\\', "/");
            let digest = expected
                .get(&relative_child)
                .ok_or_else(|| OvenRustcError::UnrecordedSearchArtifact {
                    path: child_path.clone(),
                })?;
            let actual = digest_bytes(&fs::read(&child_path).map_err(|source| OvenRustcError::Io {
                path: child_path.clone(),
                source,
            })?);
            if digest != &actual {
                return Err(OvenRustcError::ArtifactDigestMismatch {
                    path: child_path,
                    expected: digest.clone(),
                    actual,
                });
            }
            files_in_directory = files_in_directory.saturating_add(1);
        }
        if files_in_directory == 0 {
            return Err(OvenRustcError::InvalidArtifactPath {
                kind,
                path,
                message: "must contain at least one manifest-recorded regular file".to_string(),
            });
        }
        materialized.push(path);
    }
    Ok(materialized)
}

/// Verify the safe directory/file shape of a store-owned closure without repeating publisher-time SHA-256 work.
fn trusted_materialize_search_paths(
    root: &Path,
    paths: &[String],
    kind: &'static str,
    expected: &BTreeMap<String, String>,
) -> Result<Vec<PathBuf>, OvenRustcError> {
    let mut materialized = Vec::new();
    let mut seen = BTreeSet::new();
    for relative in paths {
        let normalized = normalized_relative_path(relative, kind)?;
        if !seen.insert(normalized.clone()) {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!("declares duplicate {kind} path `{relative}`"),
            });
        }
        let path = safe_path(root, &normalized, kind)?;
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
            path: path.clone(),
            source,
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(OvenRustcError::InvalidArtifactPath {
                kind,
                path,
                message: "must be a non-symlink directory".to_string(),
            });
        }
        let mut files_in_directory = 0_u64;
        for child in fs::read_dir(&path).map_err(|source| OvenRustcError::Io {
            path: path.clone(),
            source,
        })? {
            let child = child.map_err(|source| OvenRustcError::Io {
                path: path.clone(),
                source,
            })?;
            let child_path = child.path();
            let child_metadata = fs::symlink_metadata(&child_path).map_err(|source| OvenRustcError::Io {
                path: child_path.clone(),
                source,
            })?;
            if !child_metadata.is_file() || child_metadata.file_type().is_symlink() {
                return Err(OvenRustcError::InvalidArtifactPath {
                    kind,
                    path: child_path,
                    message: "search directories may contain only regular files".to_string(),
                });
            }
            let relative_child = child_path
                .strip_prefix(root)
                .map_err(|_| OvenRustcError::InvalidArtifactPath {
                    kind,
                    path: child_path.clone(),
                    message: "resolved path escaped artifact root".to_string(),
                })?
                .to_string_lossy()
                .replace('\\', "/");
            if !expected.contains_key(&relative_child) {
                return Err(OvenRustcError::UnrecordedSearchArtifact { path: child_path });
            }
            files_in_directory = files_in_directory.saturating_add(1);
        }
        if files_in_directory == 0 {
            return Err(OvenRustcError::InvalidArtifactPath {
                kind,
                path,
                message: "must contain at least one manifest-recorded regular file".to_string(),
            });
        }
        materialized.push(path);
    }
    Ok(materialized)
}

/// Verify one manifest artifact file and return its canonical artifact-root-contained path.
fn verified_file(
    root: &Path,
    relative: &str,
    expected_digest: &str,
    kind: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    let relative = normalized_relative_path(relative, kind)?;
    let path = safe_path(root, &relative, kind)?;
    let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
        path: path.clone(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path,
            message: "must be a non-symlink regular file".to_string(),
        });
    }
    let actual = digest_bytes(&fs::read(&path).map_err(|source| OvenRustcError::Io {
        path: path.clone(),
        source,
    })?);
    if expected_digest != actual {
        return Err(OvenRustcError::ArtifactDigestMismatch {
            path,
            expected: expected_digest.to_string(),
            actual,
        });
    }
    Ok(path)
}

/// Return a safe regular store artifact without repeating its publisher-verified content digest.
fn trusted_file(root: &Path, relative: &str, kind: &'static str) -> Result<PathBuf, OvenRustcError> {
    let relative = normalized_relative_path(relative, kind)?;
    let path = safe_path(root, &relative, kind)?;
    let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
        path: path.clone(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path,
            message: "must be a non-symlink regular file".to_string(),
        });
    }
    Ok(path)
}

/// Canonicalize an existing artifact root and reject an absent or non-directory root.
fn canonical_directory(path: &Path, kind: &'static str) -> Result<PathBuf, OvenRustcError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path: path.to_path_buf(),
            message: "must be a non-symlink directory".to_string(),
        });
    }
    path.canonicalize().map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Resolve a normalized relative path and prove that its canonical parent remains below the immutable root.
fn safe_path(root: &Path, relative: &str, kind: &'static str) -> Result<PathBuf, OvenRustcError> {
    let path = root.join(relative);
    let parent = path.parent().ok_or_else(|| OvenRustcError::InvalidArtifactPath {
        kind,
        path: path.clone(),
        message: "has no parent directory".to_string(),
    })?;
    let canonical_parent = parent.canonicalize().map_err(|source| OvenRustcError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    if !canonical_parent.starts_with(root) {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path,
            message: "escapes immutable artifact root".to_string(),
        });
    }
    Ok(path)
}

/// Normalize one relative artifact path and reject absolute, parent, or platform-prefix components.
fn normalized_relative_path(value: &str, kind: &'static str) -> Result<String, OvenRustcError> {
    let path = Path::new(value);
    if value.trim().is_empty()
        || path.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir | Component::CurDir
            )
        })
    {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path: PathBuf::from(value),
            message: "must be a non-empty normalized relative path".to_string(),
        });
    }
    Ok(path.to_string_lossy().replace('\\', "/"))
}

/// Validate that a direct-rustc source or output is a non-symlink regular file.
fn verified_regular_file(path: &Path, kind: &'static str) -> Result<PathBuf, OvenRustcError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path: path.to_path_buf(),
            message: "must be a non-symlink regular file".to_string(),
        });
    }
    Ok(path.to_path_buf())
}

/// Read a caller-owned regular file once and return its stable direct-Rustc reuse digest.
fn digest_regular_file(path: &Path, kind: &'static str) -> Result<String, OvenRustcError> {
    let path = verified_regular_file(path, kind)?;
    let bytes = fs::read(&path).map_err(|source| OvenRustcError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(digest_bytes(&bytes))
}

/// Ensure the caller-owned final output cannot become part of the immutable artifact root.
fn caller_output_path(output: &Path, artifact_root: &Path) -> Result<PathBuf, OvenRustcError> {
    if output.as_os_str().is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "output",
            message: "must not be empty".to_string(),
        });
    }
    let artifact_root = artifact_root.canonicalize().map_err(|source| OvenRustcError::Io {
        path: artifact_root.to_path_buf(),
        source,
    })?;
    let output_parent = output.parent().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "output",
        message: "must have a parent directory".to_string(),
    })?;
    fs::create_dir_all(output_parent).map_err(|source| OvenRustcError::Io {
        path: output_parent.to_path_buf(),
        source,
    })?;
    let output_parent = output_parent.canonicalize().map_err(|source| OvenRustcError::Io {
        path: output_parent.to_path_buf(),
        source,
    })?;
    let resolved = output_parent.join(output.file_name().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "output",
        message: "must name a file".to_string(),
    })?);
    if resolved.starts_with(&artifact_root) {
        return Err(OvenRustcError::InvalidInput {
            field: "output",
            message: "must remain outside the immutable artifact root".to_string(),
        });
    }
    Ok(resolved)
}

/// Create a caller-owned regular temporary directory without allowing Rustdoc to write into an immutable store entry.
fn caller_temporary_directory(directory: &Path, artifact_root: &Path) -> Result<PathBuf, OvenRustcError> {
    if directory.as_os_str().is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "Rustdoc temporary directory",
            message: "must not be empty".to_string(),
        });
    }
    let artifact_root = artifact_root.canonicalize().map_err(|source| OvenRustcError::Io {
        path: artifact_root.to_path_buf(),
        source,
    })?;
    fs::create_dir_all(directory).map_err(|source| OvenRustcError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    let metadata = fs::symlink_metadata(directory).map_err(|source| OvenRustcError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind: "Rustdoc temporary directory",
            path: directory.to_path_buf(),
            message: "must be a non-symlink directory".to_string(),
        });
    }
    let directory = directory.canonicalize().map_err(|source| OvenRustcError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    if directory.starts_with(&artifact_root) {
        return Err(OvenRustcError::InvalidInput {
            field: "Rustdoc temporary directory",
            message: "must remain outside the immutable artifact root".to_string(),
        });
    }
    Ok(directory)
}

/// Preserve both direct-tool streams for a single actionable Rustdoc failure report.
fn combined_process_output(stdout: &[u8], stderr: &[u8]) -> String {
    format!("{}{}", String::from_utf8_lossy(stdout), String::from_utf8_lossy(stderr))
}

/// Validate the narrow Rust identifier syntax accepted for a direct test crate name.
fn validate_rust_identifier(name: &str) -> Result<(), OvenRustcError> {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return Err(OvenRustcError::InvalidInput {
            field: "crate name",
            message: "must not be empty".to_string(),
        });
    };
    if !(first == '_' || first.is_ascii_alphabetic())
        || !characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err(OvenRustcError::InvalidInput {
            field: "crate name",
            message: "must use ASCII Rust identifier characters".to_string(),
        });
    }
    Ok(())
}

/// Validate an explicit Rust target triple without accepting command-line syntax or path components.
fn validate_rust_target(target: &str) -> Result<(), OvenRustcError> {
    if target.is_empty()
        || !target
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.'))
    {
        return Err(OvenRustcError::InvalidInput {
            field: "vocabulary auxiliary Rust target",
            message: "must use only ASCII target-triple characters".to_string(),
        });
    }
    Ok(())
}

/// Validate editions that the Alpha direct runner explicitly supports.
fn validate_edition(edition: &str) -> Result<(), OvenRustcError> {
    if matches!(edition, "2021" | "2024") {
        return Ok(());
    }
    Err(OvenRustcError::InvalidInput {
        field: "edition",
        message: "must be one of 2021 or 2024".to_string(),
    })
}

/// Decode rustc JSON diagnostics from both captured streams while retaining unstructured lines.
fn parse_rustc_diagnostics(stdout: &[u8], stderr: &[u8]) -> OvenRustcDiagnosticReport {
    let mut diagnostics = Vec::new();
    let mut unstructured = String::new();
    for line in [stdout, stderr].into_iter().flat_map(|stream| {
        String::from_utf8_lossy(stream)
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    }) {
        match serde_json::from_str::<RustcJsonDiagnostic>(&line) {
            Ok(record) if record.is_compiler_message() => diagnostics.push(OvenRustcDiagnostic {
                level: record.message.level,
                message: record.message.message,
                code: record.message.code.map(|code| code.code),
                spans: record
                    .message
                    .spans
                    .into_iter()
                    .map(|span| OvenRustcDiagnosticSpan {
                        file_name: span.file_name,
                        line_start: span.line_start,
                        column_start: span.column_start,
                        line_end: span.line_end,
                        column_end: span.column_end,
                        is_primary: span.is_primary,
                    })
                    .collect(),
                rendered: record.message.rendered,
            }),
            _ if line.trim().is_empty() => {}
            _ => {
                unstructured.push_str(&line);
                unstructured.push('\n');
            }
        }
    }
    OvenRustcDiagnosticReport {
        diagnostics,
        unstructured_output: unstructured,
        invocation: None,
    }
}

impl OvenRustcDiagnosticReport {
    /// Attach bounded process evidence to a report after Rustc has exited unsuccessfully.
    fn with_invocation(mut self, command: &Command) -> Self {
        const MAX_INVOCATION_CHARS: usize = 12_000;

        // Do not render `Command` itself: its debug format may include explicit compile-time environment values.
        // Rustc program and argument evidence is enough to replay the artifact closure without exposing that state.
        let mut rendered = format!("{:?}", command.get_program());
        for argument in command.get_args() {
            rendered.push(' ');
            rendered.push_str(&format!("{argument:?}"));
        }
        let mut invocation = rendered.chars().take(MAX_INVOCATION_CHARS).collect::<String>();
        if rendered.chars().count() > MAX_INVOCATION_CHARS {
            invocation.push_str(" … invocation truncated");
        }
        self.invocation = Some(invocation);
        self
    }
}

/// Minimal rustc JSON envelope for diagnostic preservation.
#[derive(Deserialize)]
struct RustcJsonDiagnostic {
    #[serde(default)]
    reason: String,
    #[serde(default, rename = "$message_type")]
    message_type: String,
    message: RustcJsonMessage,
}

impl RustcJsonDiagnostic {
    /// Cargo wraps rustc diagnostics with `reason`; direct rustc emits the `$message_type` envelope.
    fn is_compiler_message(&self) -> bool {
        self.reason == "compiler-message" || self.message_type == "diagnostic"
    }
}

/// Minimal structured rustc diagnostic message.
#[derive(Deserialize)]
struct RustcJsonMessage {
    level: String,
    message: String,
    code: Option<RustcJsonCode>,
    #[serde(default)]
    spans: Vec<RustcJsonSpan>,
    rendered: Option<String>,
}

/// Minimal rustc error-code representation.
#[derive(Deserialize)]
struct RustcJsonCode {
    code: String,
}

/// Minimal rustc span representation.
#[derive(Deserialize)]
struct RustcJsonSpan {
    file_name: String,
    line_start: u32,
    column_start: u32,
    line_end: u32,
    column_end: u32,
    is_primary: bool,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenCallerOwnedRustcLibrary, OvenDirectRustcTestRequest,
        OvenRegistryLeafAuthority, OvenRustcArtifactExtern, OvenRustcArtifactManifest, OvenRustcArtifactPlan,
        OvenRustcError, OvenRustcRegistryLeaf, OvenRustcSupportingArtifact, OvenSelectedPathRustcAuthority,
        OvenStoredDirectRustcRunRequest, OvenStoredDirectRustcTestRequest, OvenTrustedDirectRustcTargetRequest,
        OvenTrustedRustcArtifactRoot, OvenTrustedRustdocTestRequest, apply_oven_profile,
        attach_caller_owned_rustc_libraries, bake_direct_rustc_test, bake_stored_direct_rustc_run,
        bake_stored_direct_rustc_test, bake_trusted_direct_rustc_dylib, bake_trusted_direct_rustc_library,
        bake_trusted_direct_rustc_proc_macro, bake_trusted_direct_rustc_run, bake_trusted_direct_rustc_test,
        combined_process_output, materialize_declared_rust_libraries,
        materialize_declared_rust_libraries_with_selected_path_authority, resolve_sealed_registry_leaf,
        run_trusted_rustdoc_test, rustc_dynamic_library_environment, rustc_host_target,
        select_direct_rustc_plan_identity,
    };
    use crate::manifest::{DependencySource, DependencySpec};
    use crate::oven::native_test::run_native_test_batch_all;
    use crate::oven::store::{OvenArtifactKind, OvenArtifactPublishRequest, OvenStore, OvenStoreLimits};
    use crate::oven::{
        OVEN_COMPILER_TEST_PROFILE, OvenGeneratedProjectRequest, OvenImportRequest, digest_bytes,
        import_frozen_project, receipt_generated_project,
    };

    #[test]
    fn failed_direct_rustc_report_keeps_the_bounded_invocation() {
        let mut command = Command::new("rustc");
        command.args(["--crate-name", "closure_probe"]);
        let report = super::parse_rustc_diagnostics(b"", b"").with_invocation(&command);

        assert_eq!(
            report.invocation.as_deref(),
            Some("\"rustc\" \"--crate-name\" \"closure_probe\"")
        );
        assert_eq!(
            report.to_string(),
            "direct rustc invocation: \"rustc\" \"--crate-name\" \"closure_probe\""
        );
    }

    #[test]
    fn materializes_recursive_path_rust_libraries_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let helper = workspace.path().join("helper");
        let wrapper = workspace.path().join("wrapper");
        fs::create_dir_all(helper.join("src"))?;
        fs::create_dir_all(wrapper.join("src"))?;
        fs::write(
            helper.join("Cargo.toml"),
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(helper.join("src/lib.rs"), "pub fn value() -> i64 { 41 }\n")?;
        fs::write(
            wrapper.join("Cargo.toml"),
            "[package]\nname = \"wrapper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = { path = \"../helper\" }\n",
        )?;
        fs::write(
            wrapper.join("src/lib.rs"),
            "pub fn value() -> i64 { helper::value() + 1 }\n",
        )?;
        let rustc = rustc_path()?;
        let target = rustc_host_target(&rustc)?;
        let libraries = materialize_declared_rust_libraries(
            &workspace.path().join("oven-output"),
            &rustc,
            &target,
            "debug",
            &[DependencySpec {
                crate_name: "wrapper".to_string(),
                version: None,
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Path { path: wrapper.clone() },
                optional: false,
                package: None,
            }],
            None,
        )?;
        assert_eq!(libraries.len(), 2, "the direct closure should retain the nested helper");
        assert!(libraries.iter().all(|library| library.output.is_file()));
        let consumer_source = workspace.path().join("consumer.rs");
        let consumer_output = workspace.path().join("consumer");
        fs::write(&consumer_source, "fn main() { assert_eq!(wrapper::value(), 42); }\n")?;
        let mut command = Command::new(&rustc);
        command.arg("--edition=2021");
        for library in &libraries {
            command
                .arg("--extern")
                .arg(format!("{}={}", library.crate_name, library.output.display()));
            let parent = library.output.parent().ok_or("materialized library has no parent")?;
            command.arg("-L").arg(format!("dependency={}", parent.display()));
        }
        let status = command.arg(&consumer_source).arg("-o").arg(&consumer_output).status()?;
        assert!(
            status.success(),
            "direct-rustc consumer should link the materialized path crate"
        );
        assert!(Command::new(consumer_output).status()?.success());
        Ok(())
    }

    #[test]
    fn selected_path_authority_prefers_a_compilation_equivalent_sealed_registry_artifact()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let catalog = workspace.path().join("catalog");
        let selected = workspace.path().join("selected/deps");
        fs::create_dir_all(&catalog)?;
        fs::create_dir_all(&selected)?;
        let sealed = catalog.join("libserde-verified.rlib");
        let selected_copy = selected.join("libserde-verified.rlib");
        fs::write(&sealed, b"receipt-bound serde")?;
        fs::write(&selected_copy, b"receipt-bound serde")?;
        let plan = OvenRustcArtifactPlan {
            dependency_search_paths: vec![selected.clone()],
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        let authority = OvenSelectedPathRustcAuthority::new(&[], &plan);

        assert_eq!(
            authority.matching_sealed_registry_artifact(&sealed),
            Some(selected_copy)
        );
        fs::write(selected.join("libserde-verified.rlib"), b"different serde")?;
        assert_eq!(
            authority.matching_sealed_registry_artifact(&sealed),
            Some(selected.join("libserde-verified.rlib"))
        );
        Ok(())
    }

    #[test]
    fn materializes_a_path_library_with_a_selected_compiler_runtime_child_without_reparsing_features()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let runtime_root = workspace.path().join("leased-runtime");
        let runtime = runtime_root.join("incan_stdlib");
        let wrapper = workspace.path().join("caller-wrapper");
        let sealed_dependencies = workspace.path().join("selected-plan/deps");
        fs::create_dir_all(runtime.join("src"))?;
        fs::create_dir_all(wrapper.join("src"))?;
        fs::create_dir_all(&sealed_dependencies)?;
        fs::write(
            runtime.join("Cargo.toml"),
            "[package]\nname = \"incan_stdlib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nfull = []\n",
        )?;
        fs::write(runtime.join("src/lib.rs"), "pub fn value() -> i64 { 41 }\n")?;
        fs::write(
            wrapper.join("Cargo.toml"),
            "[package]\nname = \"caller_wrapper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nincan_stdlib = { path = \"../leased-runtime/incan_stdlib\" }\n",
        )?;
        fs::write(
            wrapper.join("src/lib.rs"),
            "pub fn value() -> i64 { incan_stdlib::value() + 1 }\n",
        )?;
        let rustc = rustc_path()?;
        let target = rustc_host_target(&rustc)?;
        let selected_runtime = sealed_dependencies.join("libincan_stdlib.rlib");
        let runtime_status = Command::new(&rustc)
            .args(["--crate-type", "lib", "--crate-name", "incan_stdlib"])
            .arg("--target")
            .arg(&target)
            .arg("--edition=2021")
            .arg(runtime.join("src/lib.rs"))
            .arg("-o")
            .arg(&selected_runtime)
            .status()?;
        assert!(runtime_status.success(), "selected runtime fixture should compile");
        let plan = OvenRustcArtifactPlan {
            dependency_search_paths: vec![sealed_dependencies.clone()],
            native_search_paths: Vec::new(),
            externs: vec![("incan_stdlib".to_string(), selected_runtime.clone())],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        let authority = OvenSelectedPathRustcAuthority::new(&[fs::canonicalize(&runtime_root)?], &plan);
        let libraries = materialize_declared_rust_libraries_with_selected_path_authority(
            &workspace.path().join("oven-output"),
            &rustc,
            &target,
            "debug",
            &[DependencySpec {
                crate_name: "caller_wrapper".to_string(),
                version: None,
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Path { path: wrapper },
                optional: false,
                package: None,
            }],
            None,
            Some(&authority),
        )?;
        assert_eq!(libraries.len(), 1, "the selected runtime remains plan-owned");
        assert_eq!(libraries[0].crate_name, "caller_wrapper");

        let consumer_source = workspace.path().join("consumer.rs");
        let consumer_output = workspace.path().join("consumer");
        fs::write(
            &consumer_source,
            "fn main() { assert_eq!(caller_wrapper::value(), 42); }\n",
        )?;
        let wrapper_output = &libraries[0].output;
        let wrapper_parent = wrapper_output.parent().ok_or("wrapper output parent")?;
        let status = Command::new(&rustc)
            .arg("--edition=2021")
            .arg("-L")
            .arg(format!("dependency={}", wrapper_parent.display()))
            .arg("-L")
            .arg(format!("dependency={}", sealed_dependencies.display()))
            .arg("--extern")
            .arg(format!("caller_wrapper={}", wrapper_output.display()))
            .arg("--extern")
            .arg(format!("incan_stdlib={}", selected_runtime.display()))
            .arg(&consumer_source)
            .arg("-o")
            .arg(&consumer_output)
            .status()?;
        assert!(
            status.success(),
            "direct-rustc consumer should link the selected runtime"
        );
        assert!(Command::new(consumer_output).status()?.success());
        Ok(())
    }

    #[test]
    fn materializes_a_path_library_that_disables_default_features_without_enabling_any_feature()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let component = workspace.path().join("component");
        let wrapper = workspace.path().join("wrapper");
        fs::create_dir_all(component.join("src"))?;
        fs::create_dir_all(wrapper.join("src"))?;
        fs::write(
            component.join("Cargo.toml"),
            "[package]\nname = \"feature_component\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\ndefault = [\"unavailable\"]\nunavailable = []\n",
        )?;
        fs::write(
            component.join("src/lib.rs"),
            "#[cfg(feature = \"unavailable\")] compile_error!(\"default Cargo feature must stay disabled\");\npub fn value() -> i64 { 41 }\n",
        )?;
        fs::write(
            wrapper.join("Cargo.toml"),
            "[package]\nname = \"feature_wrapper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nfeature_component = { path = \"../component\", default-features = false }\n",
        )?;
        fs::write(
            wrapper.join("src/lib.rs"),
            "pub fn value() -> i64 { feature_component::value() + 1 }\n",
        )?;
        let rustc = rustc_path()?;
        let target = rustc_host_target(&rustc)?;
        let libraries = materialize_declared_rust_libraries(
            &workspace.path().join("oven-output"),
            &rustc,
            &target,
            "debug",
            &[DependencySpec {
                crate_name: "feature_wrapper".to_string(),
                version: None,
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Path { path: wrapper.clone() },
                optional: false,
                package: None,
            }],
            None,
        )?;
        assert_eq!(
            libraries.len(),
            2,
            "the feature-disabled component remains a direct Rustc dependency"
        );
        let consumer_source = workspace.path().join("consumer.rs");
        let consumer_output = workspace.path().join("consumer");
        fs::write(
            &consumer_source,
            "fn main() { assert_eq!(feature_wrapper::value(), 42); }\n",
        )?;
        let mut command = Command::new(&rustc);
        command.arg("--edition=2021");
        for library in &libraries {
            command
                .arg("--extern")
                .arg(format!("{}={}", library.crate_name, library.output.display()));
            let parent = library.output.parent().ok_or("materialized library has no parent")?;
            command.arg("-L").arg(format!("dependency={}", parent.display()));
        }
        let status = command.arg(&consumer_source).arg("-o").arg(&consumer_output).status()?;
        assert!(
            status.success(),
            "direct-rustc consumer should link a default-feature-disabled component"
        );
        assert!(Command::new(consumer_output).status()?.success());

        let error = match materialize_declared_rust_libraries(
            &workspace.path().join("default-feature-output"),
            &rustc,
            &target,
            "debug",
            &[DependencySpec {
                crate_name: "feature_component".to_string(),
                version: None,
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Path { path: component },
                optional: false,
                package: None,
            }],
            None,
        ) {
            Ok(_) => return Err("a default Cargo feature activation remains unsupported".into()),
            Err(error) => error,
        };
        assert!(error.to_string().contains("activates default Cargo features"));
        Ok(())
    }

    #[test]
    fn materializes_a_path_proc_macro_with_a_sealed_registry_child_without_cargo()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let registry = workspace.path().join("registry");
        let macro_package = workspace.path().join("macro-package");
        let registry_deps = registry.join("target/aarch64-apple-darwin/debug/deps");
        fs::create_dir_all(&registry_deps)?;
        fs::create_dir_all(macro_package.join("src"))?;
        let rustc = rustc_path()?;
        let target = rustc_host_target(&rustc)?;
        let registry_base_source = registry.join("registry_base.rs");
        let registry_base_artifact = registry_deps.join("libregistry_base.rlib");
        fs::write(&registry_base_source, "pub fn marker() {}\n")?;
        let registry_base_status = Command::new(&rustc)
            .args(["--crate-type", "lib", "--crate-name", "registry_base"])
            .arg("--target")
            .arg(&target)
            .arg("--edition=2021")
            .arg(&registry_base_source)
            .arg("-o")
            .arg(&registry_base_artifact)
            .status()?;
        assert!(
            registry_base_status.success(),
            "fixture registry transitive leaf should compile"
        );
        let registry_source = registry.join("registry_helper.rs");
        let registry_artifact = registry_deps.join("libregistry_helper.rlib");
        fs::write(&registry_source, "pub fn marker() { registry_base::marker(); }\n")?;
        let registry_status = Command::new(&rustc)
            .args(["--crate-type", "lib", "--crate-name", "registry_helper"])
            .arg("--target")
            .arg(&target)
            .arg("--edition=2021")
            .arg("-L")
            .arg(format!("dependency={}", registry_deps.display()))
            .arg("--extern")
            .arg(format!("registry_base={}", registry_base_artifact.display()))
            .arg(&registry_source)
            .arg("-o")
            .arg(&registry_artifact)
            .status()?;
        assert!(registry_status.success(), "fixture registry leaf should compile");
        fs::write(
            macro_package.join("Cargo.toml"),
            "[package]\nname = \"proc_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nproc-macro = true\n\n[dependencies]\nregistry_helper = \"1\"\n",
        )?;
        fs::write(
            macro_package.join("src/lib.rs"),
            "use proc_macro::{Literal, TokenStream, TokenTree};\nuse registry_helper::marker;\n#[proc_macro]\npub fn answer(_input: TokenStream) -> TokenStream { marker(); TokenStream::from(TokenTree::Literal(Literal::u32_unsuffixed(42))) }\n",
        )?;
        let registry_relative = "target/aarch64-apple-darwin/debug/deps/libregistry_helper.rlib";
        let authority = OvenRegistryLeafAuthority::new_with_trusted_dependency_search_paths(
            registry.clone(),
            vec![OvenRustcRegistryLeaf {
                package: "registry_helper".to_string(),
                version: "1.0.0".to_string(),
                crate_name: "registry_helper".to_string(),
                features: Vec::new(),
                artifact: OvenRustcArtifactExtern {
                    crate_name: "registry_helper".to_string(),
                    relative_path: registry_relative.to_string(),
                    digest: digest_bytes(&fs::read(&registry_artifact)?),
                },
            }],
            vec![registry_deps],
        );
        let libraries = materialize_declared_rust_libraries(
            &workspace.path().join("oven-output"),
            &rustc,
            &target,
            "debug",
            &[DependencySpec {
                crate_name: "proc_fixture".to_string(),
                version: None,
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Path {
                    path: macro_package.clone(),
                },
                optional: false,
                package: None,
            }],
            Some(&authority),
        )?;
        let proc_macro = libraries
            .iter()
            .find(|library| library.crate_name == "proc_fixture")
            .ok_or("materialized proc macro")?;
        assert_eq!(
            proc_macro.output.extension().and_then(|extension| extension.to_str()),
            Some(std::env::consts::DLL_SUFFIX.trim_start_matches('.'))
        );
        let consumer_source = workspace.path().join("consumer.rs");
        let consumer_output = workspace.path().join("consumer");
        fs::write(
            &consumer_source,
            "use proc_fixture::answer;\nfn main() { assert_eq!(answer!(), 42); }\n",
        )?;
        let status = Command::new(&rustc)
            .arg("--edition=2021")
            .arg("--extern")
            .arg(format!("proc_fixture={}", proc_macro.output.display()))
            .arg("-L")
            .arg(format!(
                "dependency={}",
                proc_macro.output.parent().ok_or("proc macro parent")?.display()
            ))
            .arg(&consumer_source)
            .arg("-o")
            .arg(&consumer_output)
            .status()?;
        assert!(
            status.success(),
            "direct-rustc consumer should load the materialized proc macro"
        );
        assert!(Command::new(consumer_output).status()?.success());
        Ok(())
    }

    #[test]
    fn selects_the_highest_sealed_registry_leaf_matching_the_declared_requirement()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = tempfile::tempdir()?;
        let mut leaves = Vec::new();
        for version in ["1.0.8", "1.0.18", "2.0.0"] {
            let artifact = registry.path().join(format!("libitoa-{version}.rlib"));
            let bytes = format!("sealed itoa {version}").into_bytes();
            fs::write(&artifact, &bytes)?;
            leaves.push(OvenRustcRegistryLeaf {
                package: "itoa".to_string(),
                version: version.to_string(),
                crate_name: "itoa".to_string(),
                features: if version == "1.0.18" {
                    vec!["std".to_string()]
                } else {
                    Vec::new()
                },
                artifact: OvenRustcArtifactExtern {
                    crate_name: "itoa".to_string(),
                    relative_path: artifact
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or("registry artifact name")?
                        .to_string(),
                    digest: digest_bytes(&bytes),
                },
            });
        }
        let rustc = rustc_path()?;
        let target = rustc_host_target(&rustc)?;
        let authority = OvenRegistryLeafAuthority::new(registry.path().to_path_buf(), leaves);
        let libraries = materialize_declared_rust_libraries(
            &registry.path().join("output"),
            &rustc,
            &target,
            "debug",
            &[DependencySpec {
                crate_name: "itoa".to_string(),
                version: Some("1".to_string()),
                features: vec!["std".to_string()],
                default_features: true,
                source: DependencySource::Registry,
                optional: false,
                package: None,
            }],
            Some(&authority),
        )?;
        assert_eq!(libraries.len(), 1);
        assert_eq!(
            libraries[0].output.file_name().and_then(|name| name.to_str()),
            Some("libitoa-1.0.18.rlib")
        );
        let unavailable_feature = materialize_declared_rust_libraries(
            &registry.path().join("output"),
            &rustc,
            &target,
            "debug",
            &[DependencySpec {
                crate_name: "itoa".to_string(),
                version: Some("1".to_string()),
                features: vec!["alloc".to_string()],
                default_features: true,
                source: DependencySource::Registry,
                optional: false,
                package: None,
            }],
            Some(&authority),
        );
        assert!(matches!(unavailable_feature, Err(OvenRustcError::InvalidInput { .. })));
        Ok(())
    }

    #[test]
    fn aggregates_compatible_registry_catalogs_without_losing_the_leaf_root() -> Result<(), Box<dyn std::error::Error>>
    {
        let narrow = tempfile::tempdir()?;
        let broad = tempfile::tempdir()?;
        let narrow_artifact = narrow.path().join("libbitflags-v2.rlib");
        let narrow_bytes = b"sealed bitflags 2.13.1";
        fs::write(&narrow_artifact, narrow_bytes)?;
        let broad_artifact = broad.path().join("libbitflags-v1.rlib");
        let broad_bytes = b"sealed bitflags 1.3.2";
        fs::write(&broad_artifact, broad_bytes)?;
        let authority = OvenRegistryLeafAuthority::aggregate([
            OvenRegistryLeafAuthority::new(
                narrow.path().to_path_buf(),
                vec![OvenRustcRegistryLeaf {
                    package: "bitflags".to_string(),
                    version: "2.13.1".to_string(),
                    crate_name: "bitflags".to_string(),
                    features: Vec::new(),
                    artifact: OvenRustcArtifactExtern {
                        crate_name: "bitflags".to_string(),
                        relative_path: "libbitflags-v2.rlib".to_string(),
                        digest: digest_bytes(narrow_bytes),
                    },
                }],
            ),
            OvenRegistryLeafAuthority::new(
                broad.path().to_path_buf(),
                vec![OvenRustcRegistryLeaf {
                    package: "bitflags".to_string(),
                    version: "1.3.2".to_string(),
                    crate_name: "bitflags".to_string(),
                    features: Vec::new(),
                    artifact: OvenRustcArtifactExtern {
                        crate_name: "bitflags".to_string(),
                        relative_path: "libbitflags-v1.rlib".to_string(),
                        digest: digest_bytes(broad_bytes),
                    },
                }],
            ),
        ]);
        let dependency = DependencySpec {
            crate_name: "bitflags".to_string(),
            version: Some("=1.3.2".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };

        assert_eq!(
            resolve_sealed_registry_leaf(&dependency, Some(&authority), "debug")?,
            fs::canonicalize(broad_artifact)?
        );
        Ok(())
    }

    #[test]
    fn selects_one_profile_matched_copy_of_an_equivalent_sealed_registry_leaf() -> Result<(), Box<dyn std::error::Error>>
    {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        let relative_path = "target/aarch64-apple-darwin/debug/deps/libfixture_registry-1234.rlib";
        let first_artifact = first.path().join(relative_path);
        let second_artifact = second.path().join(relative_path);
        fs::create_dir_all(first_artifact.parent().ok_or("first registry parent")?)?;
        fs::create_dir_all(second_artifact.parent().ok_or("second registry parent")?)?;
        let first_bytes = b"first sealed copy";
        let second_bytes = b"second sealed copy";
        fs::write(&first_artifact, first_bytes)?;
        fs::write(&second_artifact, second_bytes)?;
        let leaf = |digest| OvenRustcRegistryLeaf {
            package: "fixture-registry".to_string(),
            version: "1.0.0".to_string(),
            crate_name: "fixture_registry".to_string(),
            features: vec!["derive".to_string()],
            artifact: OvenRustcArtifactExtern {
                crate_name: "fixture_registry".to_string(),
                relative_path: relative_path.to_string(),
                digest,
            },
        };
        let authority = OvenRegistryLeafAuthority::aggregate([
            OvenRegistryLeafAuthority::new(first.path().to_path_buf(), vec![leaf(digest_bytes(first_bytes))]),
            OvenRegistryLeafAuthority::new(second.path().to_path_buf(), vec![leaf(digest_bytes(second_bytes))]),
        ]);
        let dependency = DependencySpec {
            crate_name: "fixture_registry".to_string(),
            version: Some("1".to_string()),
            features: vec!["derive".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: Some("fixture-registry".to_string()),
        };

        let selected = resolve_sealed_registry_leaf(&dependency, Some(&authority), "debug")?;
        let expected = [fs::canonicalize(first_artifact)?, fs::canonicalize(second_artifact)?]
            .into_iter()
            .min()
            .ok_or("expected registry artifact")?;
        assert_eq!(selected, expected);
        Ok(())
    }

    #[test]
    fn compiler_test_profile_has_an_explicit_direct_rustc_contract() {
        let mut command = Command::new("rustc");
        apply_oven_profile(&mut command, OVEN_COMPILER_TEST_PROFILE);
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            vec![
                "-C",
                "debuginfo=0",
                "-C",
                "strip=debuginfo",
                "-C",
                "debug-assertions=on",
                "-C",
                "overflow-checks=on",
            ]
        );

        let mut developer_profile = Command::new("rustc");
        apply_oven_profile(&mut developer_profile, "debug");
        assert!(developer_profile.get_args().next().is_none());
    }

    #[test]
    fn artifact_manifest_rejects_an_escaping_path() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let manifest = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: intent(root.path())?.intent,
            dependency_search_paths: vec!["../escape".to_string()],
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let result = manifest.materialize(root.path(), &intent(root.path())?.intent);
        assert!(matches!(result, Err(OvenRustcError::InvalidArtifactPath { .. })));
        Ok(())
    }

    #[test]
    fn composed_trusted_plan_uses_each_foundation_root_without_a_composite_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        fs::create_dir_all(first.path().join("deps"))?;
        fs::create_dir_all(second.path().join("deps"))?;
        fs::write(first.path().join("deps/libfirst.rlib"), b"first")?;
        fs::write(second.path().join("deps/libsecond.rlib"), b"second")?;
        let receipt = intent(first.path())?;
        let manifest = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: vec!["deps".to_string()],
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: vec![
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libfirst.rlib".to_string(),
                    digest: "sha256:first".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libsecond.rlib".to_string(),
                    digest: "sha256:second".to_string(),
                },
            ],
        };
        let first_fragment = vec![OvenRustcSupportingArtifact {
            relative_path: "deps/libfirst.rlib".to_string(),
            digest: "sha256:first".to_string(),
        }];
        let second_fragment = vec![OvenRustcSupportingArtifact {
            relative_path: "deps/libsecond.rlib".to_string(),
            digest: "sha256:second".to_string(),
        }];
        let search_paths = vec!["deps".to_string()];
        let roots = [
            OvenTrustedRustcArtifactRoot {
                artifact_root: first.path(),
                dependency_search_paths: &search_paths,
                native_search_paths: &[],
                supporting_artifacts: &first_fragment,
            },
            OvenTrustedRustcArtifactRoot {
                artifact_root: second.path(),
                dependency_search_paths: &search_paths,
                native_search_paths: &[],
                supporting_artifacts: &second_fragment,
            },
        ];

        let plan = manifest.materialize_trusted_store_composed(&roots, &receipt.intent)?;
        let first_deps = fs::canonicalize(first.path().join("deps"))?;
        let second_deps = fs::canonicalize(second.path().join("deps"))?;
        assert_eq!(plan.dependency_search_paths.len(), 2);
        assert!(plan.dependency_search_paths.iter().any(|path| path == &first_deps));
        assert!(plan.dependency_search_paths.iter().any(|path| path == &second_deps));
        Ok(())
    }

    #[test]
    fn artifact_manifest_rejects_an_empty_search_directory() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let empty = root.path().join("empty");
        fs::create_dir(&empty)?;
        let receipt = intent(root.path())?;
        let manifest = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: vec!["empty".to_string()],
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };

        let result = manifest.materialize(root.path(), &receipt.intent);
        assert!(matches!(result, Err(OvenRustcError::InvalidArtifactPath { .. })));
        Ok(())
    }

    #[test]
    fn plan_selection_reuses_one_build_unit_across_distinct_generated_sources() -> Result<(), Box<dyn std::error::Error>>
    {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        for (root, source) in [
            (first.path(), "fn main() { println!(\"first\"); }\n"),
            (second.path(), "fn main() { println!(\"second\"); }\n"),
        ] {
            fs::create_dir_all(root.join("src"))?;
            fs::write(root.join("src/main.rs"), source)?;
        }
        let receipt_for = |root: &Path| {
            receipt_generated_project(
                &OvenGeneratedProjectRequest::new(
                    root,
                    "shared-oven-fixture",
                    "0.1.0",
                    "aarch64-apple-darwin",
                    "rustc 1.96.0",
                    "debug",
                    Vec::new(),
                )
                .with_generated_source("generated-root", root.join("src/main.rs"))
                .with_generated_source_tree("generated-tree", root.join("src"))
                .with_build_unit_input("runtime-lock", "sha256:shared"),
            )
        };
        let first_receipt = receipt_for(first.path())?;
        let second_receipt = receipt_for(second.path())?;
        assert_ne!(first_receipt.identity, second_receipt.identity);
        assert_eq!(first_receipt.build_unit_identity, second_receipt.build_unit_identity);

        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
        );
        let stored = store.publish(&OvenArtifactPublishRequest {
            receipt: first_receipt.clone(),
            domain: "shared-alpha".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&empty_manifest(&first_receipt))?,
            materialized_files: Vec::new(),
        })?;
        assert_eq!(
            select_direct_rustc_plan_identity(&store, &second_receipt)?,
            stored.identity
        );
        Ok(())
    }

    #[test]
    fn plan_selection_skips_a_legacy_manifest_schema_before_materializing_a_replacement()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        write_project(project.path())?;
        let receipt = intent(project.path())?;
        let mut legacy = empty_manifest(&receipt);
        legacy.schema_version = OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION - 1;
        let current = empty_manifest(&receipt);
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
        );
        store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "legacy-alpha".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&legacy)?,
            materialized_files: Vec::new(),
        })?;
        let replacement = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "current-alpha".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&current)?,
            materialized_files: Vec::new(),
        })?;

        assert_eq!(
            select_direct_rustc_plan_identity(&store, &receipt)?,
            replacement.identity
        );
        Ok(())
    }

    #[test]
    fn direct_rustc_test_runs_without_cargo_in_the_consumer_environment() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = output.path().join("consumer.rs");
        fs::write(
            &source,
            "#[test]\nfn cargo_is_not_visible_to_the_consumer() { assert!(option_env!(\"CARGO\").is_none()); assert!(option_env!(\"CARGO_PKG_NAME\").is_none()); }\n",
        )?;
        let source_digest = digest_bytes(&fs::read(&source)?);
        let rustc = rustc_path()?;
        let receipt = import_frozen_project(
            &OvenImportRequest::new(
                project.path(),
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_supplemental_source_digest("direct-rustc-source", source_digest),
        )?;
        let artifact_root = tempfile::tempdir()?;
        let request = OvenDirectRustcTestRequest {
            receipt: receipt.clone(),
            artifacts: OvenRustcArtifactManifest {
                schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: receipt.intent.clone(),
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_externs: BTreeMap::new(),
                registry_leaves: Vec::new(),
                compile_environment: BTreeMap::new(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
            artifact_root: artifact_root.path().to_path_buf(),
            rustc,
            source,
            output: output.path().join("consumer-test"),
            crate_name: "oven_consumer".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "direct-rustc-source".to_string(),
        };

        let bake = bake_direct_rustc_test(&request)?;
        assert!(!bake.cargo_process_started);
        assert!(!bake.reused);
        assert!(Command::new(&bake.output).status()?.success());
        let reused = bake_direct_rustc_test(&request)?;
        assert!(reused.reused);
        assert_eq!(reused.output, bake.output);
        Ok(())
    }

    #[test]
    fn trusted_direct_rustc_runs_a_proc_macro_libtest_with_its_selected_dynamic_toolchain()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = output.path().join("proc-macro-test.rs");
        fs::write(
            &source,
            "extern crate proc_macro;\nuse proc_macro::TokenStream;\n#[proc_macro]\npub fn passthrough(input: TokenStream) -> TokenStream { input }\n#[test]\nfn direct_proc_macro_test() {}\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = import_frozen_project(
            &OvenImportRequest::new(
                project.path(),
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_supplemental_source_digest("proc-macro-source", digest_bytes(&fs::read(&source)?)),
        )?;
        let bake = bake_trusted_direct_rustc_test(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_manifest(&receipt),
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &source,
            output: &output.path().join("proc-macro-test"),
            crate_name: "oven_proc_macro_test",
            edition: "2024",
            source_evidence_key: "proc-macro-source",
            features: &[],
            prefer_dynamic: true,
        })?;
        #[cfg(unix)]
        {
            let output = Command::new("/bin/sh")
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .args(["-c", "exec \"$1\" --list --format terse", "sh"])
                .arg(&bake.output)
                .output()?;
            assert!(
                output.status.success(),
                "a dynamic direct-Rustc test must survive a shell hop without ambient loader state: {}",
                combined_process_output(&output.stdout, &output.stderr)
            );
        }
        let (name, value) = rustc_dynamic_library_environment(&rustc)?;
        let report = run_native_test_batch_all(&bake.output, &BTreeMap::from([(name, value)]))?;
        assert!(report.success);
        assert_eq!(report.inventory.names, ["direct_proc_macro_test"]);
        Ok(())
    }

    #[test]
    fn trusted_direct_rustc_materializes_a_reusable_library_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let source = project.path().join("src/materialized.rs");
        fs::write(&source, "pub fn oven_materialized() -> u32 { 42 }\n")?;
        let rustc = rustc_path()?;
        let receipt = import_frozen_project(
            &OvenImportRequest::new(
                project.path(),
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_supplemental_source_digest("materialized-library", digest_bytes(&fs::read(&source)?)),
        )?;
        let request = OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_manifest(&receipt),
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &source,
            output: &output.path().join("liboven_materialized.rlib"),
            crate_name: "oven_materialized",
            edition: "2024",
            source_evidence_key: "materialized-library",
            features: &[],
            prefer_dynamic: false,
        };

        let bake = bake_trusted_direct_rustc_library(&request)?;
        assert!(!bake.cargo_process_started);
        assert!(!bake.reused);
        assert!(bake.output.is_file());
        let reused = bake_trusted_direct_rustc_library(&request)?;
        assert!(reused.reused);
        assert_eq!(reused.output, bake.output);
        Ok(())
    }

    #[test]
    fn transitive_caller_owned_library_keeps_its_search_path_and_reuse_evidence_private()
    -> Result<(), Box<dyn std::error::Error>> {
        let output = tempfile::tempdir()?;
        let transitive = output.path().join("libprovider_child.rlib");
        fs::write(&transitive, "receipt-bound child")?;
        let transitive_digest = digest_bytes(b"receipt-bound child");
        let mut plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };

        attach_caller_owned_rustc_libraries(
            &mut plan,
            &[OvenCallerOwnedRustcLibrary {
                crate_name: "provider_child".to_string(),
                output: transitive.clone(),
                digest: transitive_digest.clone(),
                expose_extern: false,
            }],
        )?;

        assert!(plan.externs.is_empty());
        assert_eq!(plan.dependency_search_paths, vec![output.path().to_path_buf()]);
        assert_eq!(plan.caller_owned_library_digests.len(), 1);
        assert_eq!(
            plan.caller_owned_library_digests
                .get(&format!("transitive:provider_child:{transitive_digest}")),
            Some(&transitive_digest),
        );
        assert!(
            plan.caller_owned_library_digests
                .keys()
                .all(|key| key.starts_with("transitive:provider_child:"))
        );
        Ok(())
    }

    #[test]
    fn trusted_direct_rustc_links_an_oven_materialized_library_without_cargo() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let first_library_source = project.path().join("src/materialized_one.rs");
        let second_library_source = project.path().join("src/materialized_two.rs");
        let binary_source = project.path().join("src/consumer.rs");
        fs::write(&first_library_source, "pub fn answer() -> u32 { 42 }\n")?;
        fs::write(&second_library_source, "pub fn answer() -> u32 { 43 }\n")?;
        fs::write(
            &binary_source,
            "fn main() { println!(\"{}\", oven_materialized::answer()); }\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = import_frozen_project(
            &OvenImportRequest::new(
                project.path(),
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_supplemental_source_digest(
                "materialized-library-one",
                digest_bytes(&fs::read(&first_library_source)?),
            )
            .with_supplemental_source_digest(
                "materialized-library-two",
                digest_bytes(&fs::read(&second_library_source)?),
            )
            .with_supplemental_source_digest("materialized-consumer", digest_bytes(&fs::read(&binary_source)?)),
        )?;
        let empty_artifacts = empty_manifest(&receipt);
        let library = bake_trusted_direct_rustc_library(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &first_library_source,
            output: &output.path().join("liboven_materialized_one.rlib"),
            crate_name: "oven_materialized",
            edition: "2024",
            source_evidence_key: "materialized-library-one",
            features: &[],
            prefer_dynamic: false,
        })?;
        let mut consumer_plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        attach_caller_owned_rustc_libraries(
            &mut consumer_plan,
            &[OvenCallerOwnedRustcLibrary {
                crate_name: "oven_materialized".to_string(),
                output: library.output.clone(),
                digest: library.output_digest.clone(),
                expose_extern: true,
            }],
        )?;
        assert_eq!(
            consumer_plan.caller_owned_library_digests.get("oven_materialized"),
            Some(&library.output_digest),
        );
        let consumer = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: Some(&consumer_plan),
            rustc: &rustc,
            source: &binary_source,
            output: &output.path().join("oven-consumer"),
            crate_name: "oven_consumer",
            edition: "2024",
            source_evidence_key: "materialized-consumer",
            features: &[],
            prefer_dynamic: false,
        })?;
        assert!(!library.cargo_process_started);
        assert!(!consumer.cargo_process_started);
        let first_run = Command::new(&consumer.output).output()?;
        assert!(first_run.status.success());
        assert_eq!(String::from_utf8(first_run.stdout)?.trim(), "42");

        let replacement_library = bake_trusted_direct_rustc_library(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &second_library_source,
            output: &output.path().join("liboven_materialized_two.rlib"),
            crate_name: "oven_materialized",
            edition: "2024",
            source_evidence_key: "materialized-library-two",
            features: &[],
            prefer_dynamic: false,
        })?;
        let mut replacement_plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        attach_caller_owned_rustc_libraries(
            &mut replacement_plan,
            &[OvenCallerOwnedRustcLibrary {
                crate_name: "oven_materialized".to_string(),
                output: replacement_library.output.clone(),
                digest: replacement_library.output_digest.clone(),
                expose_extern: true,
            }],
        )?;
        let rebuilt_consumer = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: Some(&replacement_plan),
            rustc: &rustc,
            source: &binary_source,
            output: &output.path().join("oven-consumer"),
            crate_name: "oven_consumer",
            edition: "2024",
            source_evidence_key: "materialized-consumer",
            features: &[],
            prefer_dynamic: false,
        })?;
        assert!(!rebuilt_consumer.reused);
        let second_run = Command::new(&rebuilt_consumer.output).output()?;
        assert!(second_run.status.success());
        assert_eq!(String::from_utf8(second_run.stdout)?.trim(), "43");
        Ok(())
    }

    #[test]
    fn trusted_direct_rustc_links_an_oven_materialized_dylib_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let library_source = project.path().join("src/materialized.rs");
        let binary_source = project.path().join("src/consumer.rs");
        fs::write(&library_source, "pub fn answer() -> u32 { 42 }\n")?;
        fs::write(
            &binary_source,
            "fn main() { println!(\"{}\", oven_materialized::answer()); }\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = import_frozen_project(
            &OvenImportRequest::new(
                project.path(),
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_supplemental_source_digest("materialized-dylib", digest_bytes(&fs::read(&library_source)?))
            .with_supplemental_source_digest("dylib-consumer", digest_bytes(&fs::read(&binary_source)?)),
        )?;
        let empty_artifacts = empty_manifest(&receipt);
        let dylib = bake_trusted_direct_rustc_dylib(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &library_source,
            output: &output
                .path()
                .join(format!("liboven_materialized{}", std::env::consts::DLL_SUFFIX)),
            crate_name: "oven_materialized",
            edition: "2024",
            source_evidence_key: "materialized-dylib",
            features: &[],
            prefer_dynamic: true,
        })?;
        let mut consumer_plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        attach_caller_owned_rustc_libraries(
            &mut consumer_plan,
            &[OvenCallerOwnedRustcLibrary {
                crate_name: "oven_materialized".to_string(),
                output: dylib.output.clone(),
                digest: dylib.output_digest.clone(),
                expose_extern: true,
            }],
        )?;
        let consumer = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: Some(&consumer_plan),
            rustc: &rustc,
            source: &binary_source,
            output: &output.path().join("oven-dylib-consumer"),
            crate_name: "oven_dylib_consumer",
            edition: "2024",
            source_evidence_key: "dylib-consumer",
            features: &[],
            prefer_dynamic: true,
        })?;
        let (name, toolchain_libraries) = rustc_dynamic_library_environment(&rustc)?;
        let dylib_directory = dylib.output.parent().ok_or("dylib parent missing")?;
        let search_path = std::env::join_paths(
            std::iter::once(dylib_directory.to_path_buf()).chain(std::env::split_paths(&toolchain_libraries)),
        )?;
        let result = Command::new(&consumer.output).env(name, search_path).output()?;
        assert!(result.status.success());
        assert_eq!(String::from_utf8(result.stdout)?.trim(), "42");
        Ok(())
    }

    #[test]
    fn trusted_direct_rustc_links_an_oven_materialized_proc_macro_without_cargo()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let macro_source = project.path().join("src/oven_macros.rs");
        let consumer_source = project.path().join("src/consumer.rs");
        fs::write(
            &macro_source,
            "use proc_macro::{Literal, TokenStream, TokenTree};\n#[proc_macro]\npub fn answer(_input: TokenStream) -> TokenStream { TokenStream::from(TokenTree::Literal(Literal::u32_unsuffixed(43))) }\n",
        )?;
        fs::write(
            &consumer_source,
            "use oven_macros::answer;\nfn main() { println!(\"{}\", answer!()); }\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = import_frozen_project(
            &OvenImportRequest::new(
                project.path(),
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_supplemental_source_digest("materialized-proc-macro", digest_bytes(&fs::read(&macro_source)?))
            .with_supplemental_source_digest("proc-macro-consumer", digest_bytes(&fs::read(&consumer_source)?)),
        )?;
        let empty_artifacts = empty_manifest(&receipt);
        let proc_macro = bake_trusted_direct_rustc_proc_macro(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &macro_source,
            output: &output
                .path()
                .join(format!("liboven_macros{}", std::env::consts::DLL_SUFFIX)),
            crate_name: "oven_macros",
            edition: "2024",
            source_evidence_key: "materialized-proc-macro",
            features: &[],
            prefer_dynamic: false,
        })?;
        let mut consumer_plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        attach_caller_owned_rustc_libraries(
            &mut consumer_plan,
            &[OvenCallerOwnedRustcLibrary {
                crate_name: "oven_macros".to_string(),
                output: proc_macro.output.clone(),
                digest: proc_macro.output_digest.clone(),
                expose_extern: true,
            }],
        )?;
        let consumer = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: Some(&consumer_plan),
            rustc: &rustc,
            source: &consumer_source,
            output: &output.path().join("oven-proc-macro-consumer"),
            crate_name: "oven_proc_macro_consumer",
            edition: "2024",
            source_evidence_key: "proc-macro-consumer",
            features: &[],
            prefer_dynamic: false,
        })?;
        assert!(!proc_macro.cargo_process_started);
        assert!(!consumer.cargo_process_started);
        let output = Command::new(&consumer.output).output()?;
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout)?.trim(), "43");
        Ok(())
    }

    #[test]
    fn trusted_rustdoc_runs_a_receipt_bound_doctest_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let source = project.path().join("src/doctest.rs");
        fs::write(
            &source,
            "//! ```\n//! assert!(std::env::var_os(\"CARGO\").is_none());\n//! ```\npub struct DoctestFixture;\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = import_frozen_project(
            &OvenImportRequest::new(
                project.path(),
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_supplemental_source_digest("doctest-source", digest_bytes(&fs::read(&source)?)),
        )?;
        let mut artifacts = empty_manifest(&receipt);
        artifacts
            .compile_environment
            .insert("CARGO_MANIFEST_DIR".to_string(), "@oven-source-ancestor:2".to_string());
        let report = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
            receipt: &receipt,
            artifacts: &artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &source,
            temporary_directory: &output.path().join("rustdoc-temporary"),
            crate_name: "oven_doctest_fixture",
            edition: "2024",
            source_evidence_key: "doctest-source",
            features: &[],
            is_proc_macro: false,
            prefer_dynamic: false,
            timeout: None,
        })?;
        assert!(report.output.contains("test result: ok"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn trusted_rustdoc_timeout_terminates_a_stalled_doctest_descendant() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let source = project.path().join("src/stalled_doctest.rs");
        fs::write(
            &source,
            "//! ```\n//! assert!(true);\n//! ```\npub struct StalledDoctest;\n",
        )?;

        let sysroot = output.path().join("sysroot");
        let rustc = output.path().join("rustc");
        let rustdoc = sysroot.join("bin/rustdoc");
        let descendant_started = output.path().join("descendant-started");
        fs::create_dir_all(rustdoc.parent().ok_or("Rustdoc parent missing")?)?;
        fs::write(
            &rustc,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf '%s\\n' 'rustc oven-timeout-fixture'\n  exit 0\nfi\nif [ \"$1\" = \"--print\" ] && [ \"$2\" = \"sysroot\" ]; then\n  printf '%s\\n' \"{}\"\n  exit 0\nfi\nexit 97\n",
                sysroot.display(),
            ),
        )?;
        fs::write(
            &rustdoc,
            format!(
                "#!/bin/sh\nsleep 30 &\nprintf '%s\\n' \"$!\" > \"{}\"\nwait\n",
                descendant_started.display(),
            ),
        )?;
        for executable in [&rustc, &rustdoc] {
            let mut permissions = fs::metadata(executable)?.permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(executable, permissions)?;
        }
        let receipt = import_frozen_project(
            &OvenImportRequest::new(
                project.path(),
                "fixture-target",
                "rustc oven-timeout-fixture",
                "release",
                Vec::new(),
            )
            .with_supplemental_source_digest("stalled-doctest", digest_bytes(&fs::read(&source)?)),
        )?;

        let started = Instant::now();
        let Err(error) = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
            receipt: &receipt,
            artifacts: &empty_manifest(&receipt),
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &source,
            temporary_directory: &output.path().join("rustdoc-temporary"),
            crate_name: "stalled_doctest",
            edition: "2024",
            source_evidence_key: "stalled-doctest",
            features: &[],
            is_proc_macro: false,
            prefer_dynamic: false,
            timeout: Some(Duration::from_secs(10)),
        }) else {
            return Err("stalled Rustdoc unexpectedly completed within its receipt-bound root deadline".into());
        };
        assert!(descendant_started.is_file(), "fake Rustdoc descendant was not started");
        assert!(matches!(error, OvenRustcError::RustdocTestFailed { .. }));
        assert!(error.to_string().contains("timed out after 10000ms"), "{error}");
        assert!(error.to_string().contains(&source.display().to_string()), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(25),
            "stalled doctest descendant outlived the Rustdoc deadline"
        );
        Ok(())
    }

    #[test]
    fn trusted_rustdoc_uses_a_composed_plan_without_materializing_a_thin_artifact_root()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let thin_artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let source = project.path().join("src/composed_doctest.rs");
        fs::write(
            &source,
            "//! ```\n//! assert_eq!(2 + 2, 4);\n//! ```\npub struct DoctestFixture;\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = import_frozen_project(
            &OvenImportRequest::new(
                project.path(),
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_supplemental_source_digest("composed-doctest-source", digest_bytes(&fs::read(&source)?)),
        )?;
        let mut thin_artifacts = empty_manifest(&receipt);
        thin_artifacts.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: "missing-foundation/libfixture.rlib".to_string(),
            digest: "sha256:fixture".to_string(),
        });
        let composed_plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };

        let report = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
            receipt: &receipt,
            artifacts: &thin_artifacts,
            artifact_root: thin_artifact_root.path(),
            artifact_plan: Some(&composed_plan),
            rustc: &rustc,
            source: &source,
            temporary_directory: &output.path().join("rustdoc-temporary"),
            crate_name: "oven_composed_doctest_fixture",
            edition: "2024",
            source_evidence_key: "composed-doctest-source",
            features: &[],
            is_proc_macro: false,
            prefer_dynamic: false,
            timeout: None,
        })?;
        assert!(report.output.contains("test result: ok"));
        Ok(())
    }

    #[test]
    fn trusted_rustdoc_runs_with_a_caller_owned_dynamic_library_without_cargo() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let library_source = project.path().join("src/doctest_dylib.rs");
        let source = project.path().join("src/dynamic_doctest.rs");
        fs::write(&library_source, "pub fn answer() -> u32 { 42 }\n")?;
        fs::write(
            &source,
            "//! ```\n//! assert_eq!(oven_doctest_dylib::answer(), 42);\n//! ```\npub struct DynamicDoctestFixture;\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = import_frozen_project(
            &OvenImportRequest::new(
                project.path(),
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_supplemental_source_digest("dynamic-doctest-library", digest_bytes(&fs::read(&library_source)?))
            .with_supplemental_source_digest("dynamic-doctest-source", digest_bytes(&fs::read(&source)?)),
        )?;
        let artifacts = empty_manifest(&receipt);
        let dylib = bake_trusted_direct_rustc_dylib(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &library_source,
            output: &output
                .path()
                .join(format!("liboven_doctest_dylib{}", std::env::consts::DLL_SUFFIX)),
            crate_name: "oven_doctest_dylib",
            edition: "2024",
            source_evidence_key: "dynamic-doctest-library",
            features: &[],
            prefer_dynamic: true,
        })?;
        let mut plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        attach_caller_owned_rustc_libraries(
            &mut plan,
            &[OvenCallerOwnedRustcLibrary {
                crate_name: "oven_doctest_dylib".to_string(),
                output: dylib.output,
                digest: dylib.output_digest,
                expose_extern: true,
            }],
        )?;
        let sealed_dependency_directory = output.path().join("sealed/sha256:immutable-doctest-dependency");
        fs::create_dir_all(&sealed_dependency_directory)?;
        plan.dependency_search_paths.push(sealed_dependency_directory);

        let report = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
            receipt: &receipt,
            artifacts: &artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: Some(&plan),
            rustc: &rustc,
            source: &source,
            temporary_directory: &output.path().join("rustdoc-temporary"),
            crate_name: "oven_dynamic_doctest",
            edition: "2024",
            source_evidence_key: "dynamic-doctest-source",
            features: &[],
            is_proc_macro: false,
            prefer_dynamic: true,
            timeout: None,
        })?;
        assert!(report.output.contains("test result: ok"));
        Ok(())
    }

    #[test]
    fn trusted_rustdoc_compiles_a_proc_macro_doctest_root() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let source = project.path().join("src/proc_macro_doctest.rs");
        fs::write(
            &source,
            "use proc_macro::TokenStream;\n\n#[proc_macro_derive(OvenFixture)]\npub fn oven_fixture(_input: TokenStream) -> TokenStream { TokenStream::new() }\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = import_frozen_project(
            &OvenImportRequest::new(
                project.path(),
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_supplemental_source_digest("proc-macro-doctest-source", digest_bytes(&fs::read(&source)?)),
        )?;
        let report = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
            receipt: &receipt,
            artifacts: &empty_manifest(&receipt),
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &source,
            temporary_directory: &output.path().join("rustdoc-temporary"),
            crate_name: "oven_proc_macro_doctest",
            edition: "2024",
            source_evidence_key: "proc-macro-doctest-source",
            features: &[],
            is_proc_macro: true,
            prefer_dynamic: true,
            timeout: None,
        })?;
        assert!(report.output.contains("test result: ok"));
        Ok(())
    }

    #[test]
    fn stored_receipt_bound_plan_runs_without_cargo_and_retains_its_lease() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = output.path().join("stored-consumer.rs");
        fs::write(
            &source,
            "#[test]\nfn oven_plan_is_the_consumer_input() { assert!(option_env!(\"CARGO\").is_none()); }\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = import_frozen_project(
            &OvenImportRequest::new(
                project.path(),
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_supplemental_source_digest("direct-rustc-source", digest_bytes(&fs::read(&source)?)),
        )?;
        let plan = empty_manifest(&receipt);
        let payload = serde_json::to_vec(&plan)?;
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
        );
        let stored = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "alpha-test".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload,
            materialized_files: Vec::new(),
        })?;
        assert_eq!(select_direct_rustc_plan_identity(&store, &receipt)?, stored.identity);

        let request = OvenStoredDirectRustcTestRequest {
            store: &store,
            plan_identity: stored.identity.clone(),
            receipt: receipt.clone(),
            rustc: rustc.clone(),
            source: source.clone(),
            output: output.path().join("stored-consumer-test"),
            crate_name: "oven_stored_consumer".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "direct-rustc-source".to_string(),
        };
        let bake = bake_stored_direct_rustc_test(&request)?;

        assert!(!bake.cargo_process_started);
        assert!(!bake.reused);
        let reused = bake_stored_direct_rustc_test(&request)?;
        assert!(reused.reused);
        assert_eq!(reused.output, bake.output);
        drop(reused);
        let first_physical = store.inspect()?.physical_bytes;
        let bounded = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(first_physical.saturating_add(1), 128 * 1024, 64 * 1024),
        );
        let replacement = OvenArtifactPublishRequest {
            receipt,
            domain: "alpha-replacement".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&plan)?,
            materialized_files: Vec::new(),
        };
        assert!(matches!(
            bounded.publish(&replacement),
            Err(crate::oven::store::OvenStoreError::CapacityBlocked { .. })
        ));
        assert!(Command::new(&bake.output).status()?.success());
        drop(bake);
        bounded.publish(&replacement)?;
        assert_eq!(bounded.inspect()?.entries.len(), 1);
        Ok(())
    }

    #[test]
    fn receipt_selection_rejects_ambiguous_stored_direct_rustc_plans() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        write_project(project.path())?;
        let receipt = intent(project.path())?;
        let plan = empty_manifest(&receipt);
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
        );
        for domain in ["alpha-primary", "alpha-duplicate"] {
            store.publish(&OvenArtifactPublishRequest {
                receipt: receipt.clone(),
                domain: domain.to_string(),
                kind: OvenArtifactKind::DirectRustcPlan,
                payload: serde_json::to_vec(&plan)?,
                materialized_files: Vec::new(),
            })?;
        }

        assert!(matches!(
            select_direct_rustc_plan_identity(&store, &receipt),
            Err(OvenRustcError::PlanSelection { .. })
        ));
        Ok(())
    }

    #[test]
    fn stored_receipt_bound_binary_runs_without_cargo_and_retains_its_lease() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = output.path().join("stored-binary.rs");
        fs::write(
            &source,
            "fn main() { assert!(option_env!(\"CARGO\").is_none()); assert!(option_env!(\"CARGO_PKG_NAME\").is_none()); }\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = import_frozen_project(
            &OvenImportRequest::new(
                project.path(),
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_supplemental_source_digest("direct-rustc-source", digest_bytes(&fs::read(&source)?)),
        )?;
        let plan = empty_manifest(&receipt);
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
        );
        let stored = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "alpha-run".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&plan)?,
            materialized_files: Vec::new(),
        })?;

        let bake = bake_stored_direct_rustc_run(&OvenStoredDirectRustcRunRequest {
            store: &store,
            plan_identity: stored.identity.clone(),
            receipt: receipt.clone(),
            rustc,
            source,
            output: output.path().join("stored-binary"),
            crate_name: "oven_stored_binary".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "direct-rustc-source".to_string(),
        })?;

        assert!(!bake.cargo_process_started);
        let first_physical = store.inspect()?.physical_bytes;
        let bounded = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(first_physical.saturating_add(1), 128 * 1024, 64 * 1024),
        );
        let replacement = OvenArtifactPublishRequest {
            receipt,
            domain: "alpha-run-replacement".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&plan)?,
            materialized_files: Vec::new(),
        };
        assert!(matches!(
            bounded.publish(&replacement),
            Err(crate::oven::store::OvenStoreError::CapacityBlocked { .. })
        ));
        assert!(Command::new(&bake.output).status()?.success());
        drop(bake);
        bounded.publish(&replacement)?;
        assert_eq!(bounded.inspect()?.entries.len(), 1);
        Ok(())
    }

    #[test]
    fn direct_rustc_refuses_source_changes_before_invoking_the_compiler() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = output.path().join("consumer.rs");
        fs::write(&source, "#[test]\nfn first() {}\n")?;
        let rustc = rustc_path()?;
        let receipt = import_frozen_project(
            &OvenImportRequest::new(
                project.path(),
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_supplemental_source_digest("direct-rustc-source", digest_bytes(&fs::read(&source)?)),
        )?;
        fs::write(&source, "#[test]\nfn changed() {}\n")?;
        let request = OvenDirectRustcTestRequest {
            artifacts: empty_manifest(&receipt),
            receipt,
            artifact_root: artifact_root.path().to_path_buf(),
            rustc,
            source,
            output: output.path().join("consumer-test"),
            crate_name: "oven_consumer".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "direct-rustc-source".to_string(),
        };
        let result = bake_direct_rustc_test(&request);
        assert!(matches!(result, Err(OvenRustcError::SourceEvidenceMismatch { .. })));
        Ok(())
    }

    #[test]
    fn direct_rustc_refuses_a_compiler_that_disagrees_with_the_receipt() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = output.path().join("consumer.rs");
        fs::write(&source, "#[test]\nfn identity_is_checked() {}\n")?;
        let rustc = rustc_path()?;
        let receipt = import_frozen_project(
            &OvenImportRequest::new(
                project.path(),
                rustc_host_target(&rustc)?,
                "rustc deliberately-not-the-selected-compiler",
                "release",
                Vec::new(),
            )
            .with_supplemental_source_digest("direct-rustc-source", digest_bytes(&fs::read(&source)?)),
        )?;
        let request = OvenDirectRustcTestRequest {
            artifacts: empty_manifest(&receipt),
            receipt,
            artifact_root: artifact_root.path().to_path_buf(),
            rustc,
            source,
            output: output.path().join("consumer-test"),
            crate_name: "oven_consumer".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "direct-rustc-source".to_string(),
        };

        assert!(matches!(
            bake_direct_rustc_test(&request),
            Err(OvenRustcError::ToolchainMismatch { .. })
        ));
        Ok(())
    }

    fn intent(project: &Path) -> Result<crate::oven::OvenReceipt, Box<dyn std::error::Error>> {
        write_project(project)?;
        Ok(import_frozen_project(&OvenImportRequest::new(
            project,
            "aarch64-apple-darwin",
            "rustc 1.96.0",
            "release",
            Vec::new(),
        ))?)
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
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        }
    }

    #[test]
    fn trusted_plan_respects_entrypoint_externs_without_dropping_caller_libraries()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let receipt = intent(project.path())?;
        let mut artifacts = empty_manifest(&receipt);
        artifacts.dependency_search_paths = vec!["runtime-deps".to_string(), "vocab-deps".to_string()];
        artifacts.externs = vec![
            OvenRustcArtifactExtern {
                crate_name: "runtime".to_string(),
                relative_path: "runtime-deps/libruntime.rlib".to_string(),
                digest: digest_bytes(b"runtime"),
            },
            OvenRustcArtifactExtern {
                crate_name: "vocab_helper".to_string(),
                relative_path: "vocab-deps/libvocab_helper.rlib".to_string(),
                digest: digest_bytes(b"vocab helper"),
            },
        ];
        artifacts
            .entrypoint_externs
            .insert("generated-root".to_string(), vec!["runtime".to_string()]);
        let selected_artifacts = artifacts.for_source_evidence("generated-root")?;
        assert_eq!(selected_artifacts.dependency_search_paths, vec!["runtime-deps"]);
        let plan = OvenRustcArtifactPlan {
            dependency_search_paths: vec![
                PathBuf::from("/immutable/runtime-deps"),
                PathBuf::from("/immutable/vocab-deps"),
                PathBuf::from("/caller"),
            ],
            native_search_paths: Vec::new(),
            externs: vec![
                (
                    "runtime".to_string(),
                    PathBuf::from("/immutable/runtime-deps/libruntime.rlib"),
                ),
                (
                    "vocab_helper".to_string(),
                    PathBuf::from("/immutable/vocab-deps/libvocab_helper.rlib"),
                ),
                (
                    "caller_library".to_string(),
                    PathBuf::from("/caller/libcaller_library.rlib"),
                ),
            ],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::from([("caller_library".to_string(), digest_bytes(b"caller"))]),
        };

        let projected = super::trusted_artifact_plan_for_source(&plan, &artifacts, &selected_artifacts);

        assert_eq!(
            projected
                .externs
                .iter()
                .map(|(crate_name, _)| crate_name.as_str())
                .collect::<Vec<_>>(),
            vec!["runtime", "caller_library"]
        );
        assert_eq!(
            projected.dependency_search_paths,
            vec![PathBuf::from("/immutable/runtime-deps"), PathBuf::from("/caller")]
        );
        assert_eq!(
            projected.caller_owned_library_digests,
            plan.caller_owned_library_digests
        );
        Ok(())
    }

    #[test]
    fn source_projection_allows_a_caller_registry_leaf_to_replace_a_private_compiler_helper()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let caller = tempfile::tempdir()?;
        let receipt = intent(project.path())?;
        let caller_serde_json = caller.path().join("libserde_json.rlib");
        fs::write(&caller_serde_json, "caller serde_json")?;

        let mut artifacts = empty_manifest(&receipt);
        artifacts.dependency_search_paths = vec!["runtime-deps".to_string(), "compiler-private".to_string()];
        artifacts.externs = vec![
            OvenRustcArtifactExtern {
                crate_name: "runtime".to_string(),
                relative_path: "runtime-deps/libruntime.rlib".to_string(),
                digest: digest_bytes(b"runtime"),
            },
            OvenRustcArtifactExtern {
                crate_name: "serde_json".to_string(),
                relative_path: "compiler-private/libserde_json.rlib".to_string(),
                digest: digest_bytes(b"compiler serde_json"),
            },
        ];
        artifacts
            .entrypoint_externs
            .insert("generated-root".to_string(), vec!["runtime".to_string()]);
        let plan = OvenRustcArtifactPlan {
            dependency_search_paths: vec![
                PathBuf::from("/immutable/runtime-deps"),
                PathBuf::from("/immutable/compiler-private"),
            ],
            native_search_paths: Vec::new(),
            externs: vec![
                (
                    "runtime".to_string(),
                    PathBuf::from("/immutable/runtime-deps/libruntime.rlib"),
                ),
                (
                    "serde_json".to_string(),
                    PathBuf::from("/immutable/compiler-private/libserde_json.rlib"),
                ),
            ],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };

        let mut projected = super::trusted_artifact_plan_for_source_evidence(&plan, &artifacts, "generated-root")?;
        attach_caller_owned_rustc_libraries(
            &mut projected,
            &[OvenCallerOwnedRustcLibrary {
                crate_name: "serde_json".to_string(),
                output: caller_serde_json.clone(),
                digest: digest_bytes(&fs::read(&caller_serde_json)?),
                expose_extern: true,
            }],
        )?;
        // The final bake projects a trusted plan once more. That second projection must distinguish the caller's
        // exact output from the compiler-private serde_json artifact it replaces.
        let projected = super::trusted_artifact_plan_for_source_evidence(&projected, &artifacts, "generated-root")?;

        assert_eq!(
            projected
                .externs
                .iter()
                .map(|(crate_name, path)| (crate_name.as_str(), path.clone()))
                .collect::<Vec<_>>(),
            vec![
                ("runtime", PathBuf::from("/immutable/runtime-deps/libruntime.rlib")),
                ("serde_json", caller_serde_json),
            ]
        );
        assert!(
            !projected
                .dependency_search_paths
                .contains(&PathBuf::from("/immutable/compiler-private"))
        );
        Ok(())
    }

    fn rustc_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
        let output = Command::new("rustup").args(["which", "rustc"]).output()?;
        if !output.status.success() {
            return Err("rustup could not locate rustc".into());
        }
        let path = String::from_utf8(output.stdout)?;
        let path = PathBuf::from(path.trim());
        if !path.is_file() {
            return Err(format!("rustup returned a non-file rustc path: {}", path.display()).into());
        }
        Ok(path)
    }

    fn rustc_identity(rustc: &Path) -> Result<String, Box<dyn std::error::Error>> {
        let output = Command::new(rustc).arg("--version").output()?;
        if !output.status.success() {
            return Err(format!("rustc could not report its version: {}", rustc.display()).into());
        }
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    fn write_project(root: &Path) -> Result<(), std::io::Error> {
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"rustc_fixture\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(root.join("Cargo.lock"), "version = 4\n")?;
        Ok(())
    }
}
