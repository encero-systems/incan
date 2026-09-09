//! Direct-rustc artifact plan, manifest and registry-source shapes.
//!
//! These describe *what* a direct-rustc invocation compiles and links: its externs, supporting and auxiliary
//! artifacts, the manifest recording them, and the registry leaves a sealed plan resolves against.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::super::OvenBuildIntent;

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

/// Search paths captured for a source role, with any earlier-schema projections kept as separate provenance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenRustcSourceSearchClosure {
    /// Paths selected by the current publisher before unrelated helper closures were added.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub publisher_paths: Vec<OvenRustcSourceSearchDirectory>,
    /// Existing schema-9 policy results, projected before composition rather than inferred from mixed paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub legacy_projections: Vec<OvenRustcLegacySearchProjection>,
}

/// Provenance of an already-admitted schema-9 contributor's original source projection.
///
/// The digest identifies the input manifest; it grants no artifact or filesystem authority. The enclosing plan's
/// publication, exact artifact declarations, and active store leases still authorize every physical path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenRustcLegacySearchProjection {
    /// Earlier input format whose unchanged projection produced these paths; currently exactly 9.
    pub source_schema_version: u32,
    /// SHA-256 of the original admitted manifest's canonical JSON encoding before composition or path relocation.
    pub source_manifest_digest: String,
    /// Original source role used by the earlier projection.
    pub source_evidence_key: String,
    /// Resulting paths, relocated only through the enclosing compositor's existing artifact mapping.
    pub dependency_search_paths: Vec<OvenRustcSourceSearchDirectory>,
}

/// One selected directory and its contributor's exact declared members, captured before composition.
///
/// These records bind physical owners; they do not discover files or grant another root with the same directory name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenRustcSourceSearchDirectory {
    /// Safe directory relative to the contributor's artifact root.
    pub relative_path: String,
    /// Existing selected artifacts, transported through exact substitutions and relocations.
    pub artifacts: Vec<OvenRustcSupportingArtifact>,
}

impl OvenRustcSourceSearchClosure {
    /// Capture selected membership from existing artifact declarations without discovering files or inferring roots.
    pub(crate) fn publisher_selected(paths: Vec<String>, declared: &BTreeMap<String, String>) -> Self {
        Self {
            publisher_paths: paths
                .into_iter()
                .map(|relative_path| {
                    let artifacts = declared
                        .iter()
                        .filter(|(path, _)| super::artifact_is_below_search_path(path, &relative_path))
                        .map(|(path, digest)| OvenRustcSupportingArtifact {
                            relative_path: path.clone(),
                            digest: digest.clone(),
                        })
                        .collect();
                    OvenRustcSourceSearchDirectory {
                        relative_path,
                        artifacts,
                    }
                })
                .collect(),
            legacy_projections: Vec::new(),
        }
    }

    /// Iterate member-bound directories without upgrading earlier-schema projection evidence.
    pub(crate) fn directories(&self) -> impl Iterator<Item = &OvenRustcSourceSearchDirectory> {
        self.publisher_paths.iter().chain(
            self.legacy_projections
                .iter()
                .flat_map(|projection| projection.dependency_search_paths.iter()),
        )
    }

    /// Iterate names for logical projection; physical projection separately requires the exact member bindings.
    pub(crate) fn paths(&self) -> impl Iterator<Item = &String> {
        self.directories().map(|directory| &directory.relative_path)
    }

    /// Combine already-derived role contributions while preserving each original legacy manifest and role.
    pub(crate) fn merge(&mut self, other: &Self) {
        merge_search_directories(&mut self.publisher_paths, &other.publisher_paths);
        for incoming in &other.legacy_projections {
            if let Some(existing) = self.legacy_projections.iter_mut().find(|projection| {
                projection.source_schema_version == incoming.source_schema_version
                    && projection.source_manifest_digest == incoming.source_manifest_digest
                    && projection.source_evidence_key == incoming.source_evidence_key
            }) {
                merge_search_directories(&mut existing.dependency_search_paths, &incoming.dependency_search_paths);
            } else {
                self.legacy_projections.push(incoming.clone());
            }
        }
        self.legacy_projections.sort_by(|left, right| {
            (&left.source_manifest_digest, &left.source_evidence_key)
                .cmp(&(&right.source_manifest_digest, &right.source_evidence_key))
        });
    }

    /// Transport members through an existing exact replacement decision, preserving original legacy provenance.
    pub(crate) fn replace_artifact(
        &mut self,
        original: &OvenRustcSupportingArtifact,
        replacement: Option<&OvenRustcSupportingArtifact>,
    ) {
        if replacement == Some(original) {
            return;
        }
        for directories in std::iter::once(&mut self.publisher_paths).chain(
            self.legacy_projections
                .iter_mut()
                .map(|projection| &mut projection.dependency_search_paths),
        ) {
            let mut moved = Vec::new();
            for directory in directories.iter_mut() {
                if !directory.artifacts.contains(original) {
                    continue;
                }
                directory.artifacts.retain(|artifact| artifact != original);
                if let Some(replacement) = replacement {
                    if super::artifact_is_below_search_path(&replacement.relative_path, &directory.relative_path) {
                        directory.artifacts.push(replacement.clone());
                    } else if let Some(parent) = std::path::Path::new(&replacement.relative_path).parent() {
                        moved.push(OvenRustcSourceSearchDirectory {
                            relative_path: parent.to_string_lossy().into_owned(),
                            artifacts: vec![replacement.clone()],
                        });
                    }
                }
            }
            directories.retain(|directory| !directory.artifacts.is_empty());
            merge_search_directories(directories, &moved);
        }
    }
}

/// Merge equal directory records while retaining the exact members that select their physical owners.
fn merge_search_directories(
    target: &mut Vec<OvenRustcSourceSearchDirectory>,
    incoming: &[OvenRustcSourceSearchDirectory],
) {
    for directory in incoming {
        if let Some(existing) = target
            .iter_mut()
            .find(|existing| existing.relative_path == directory.relative_path)
        {
            existing.artifacts.extend(directory.artifacts.iter().cloned());
        } else {
            target.push(directory.clone());
        }
    }
    for directory in target.iter_mut() {
        directory
            .artifacts
            .sort_by(|left, right| (&left.relative_path, &left.digest).cmp(&(&right.relative_path, &right.digest)));
        directory.artifacts.dedup();
    }
    target.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
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
    /// Source-role search paths, separating publisher-captured closure from retained earlier-schema projections.
    ///
    /// Direct extern visibility does not determine which host or target artifacts a selected rlib's metadata needs.
    /// Schema 9 omits this evidence; schema 10 records every declared source role, including an empty closure.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entrypoint_dependency_search_paths: BTreeMap<String, OvenRustcSourceSearchClosure>,
    /// Exact registry package artifacts whose metadata closure was emitted with this immutable plan.
    ///
    /// The Loaf repeats this catalog for human inspection, while this copy travels with every bounded
    /// store entry so a selected plan resolves caller `rust::` imports only from its own compatibility domain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub registry_leaves: Vec<OvenRustcRegistryLeaf>,
    /// Complete locked registry source closure authorized for build-system-neutral Rust inspection.
    ///
    /// This is distinct from `registry_leaves`: a transitive proc macro may be required by the source graph without
    /// exposing an `.rlib` that a caller can select as a direct Rust dependency.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub registry_sources: Vec<OvenRustcRegistrySourcePackage>,
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
    /// Exact physical bindings of recorded source-role paths, retained across caller-owned additions.
    pub(crate) source_path_projection: Option<OvenRustcSourcePathProjection>,
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

impl OvenRustcArtifactPlan {
    /// Retain one caller dependency path whose artifact ownership was already established by the calling boundary.
    ///
    /// A caller-owned output or admitted provider fragment may share a directory with the immutable plan. Record that
    /// separate grant so repeated source projection cannot remove it. This method neither discovers nor admits files.
    pub(crate) fn retain_caller_dependency_search_path(&mut self, path: PathBuf) {
        if let Some(projection) = &mut self.source_path_projection {
            projection.declared.remove(&path);
        }
        if !self.dependency_search_paths.contains(&path) {
            self.dependency_search_paths.push(path);
        }
    }
}

/// Validated path bindings from one complete manifest, including independently leased fragment roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenRustcSourcePathProjection {
    /// Immutable paths still subject to role filtering; explicit caller additions remove their own paths here.
    pub(crate) declared: BTreeSet<PathBuf>,
    /// Exact source contracts paired with physical paths; a same-named differing role cannot reuse the binding.
    pub(crate) roles: BTreeMap<String, (OvenRustcSourceSearchClosure, BTreeSet<PathBuf>)>,
}

/// A verified partition of one complete direct-Rustc closure across a selected base Loaf and a project extension.
///
/// The partition is derived from exact relative paths and digests, never crate names.  This makes an extension a
/// genuine delta: a same-named artifact whose bytes differ is not silently borrowed from a base compiled with a
/// different feature graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenRustcArtifactPartition {
    /// Artifacts supplied by the selected immutable base Loaf.
    pub(crate) base_paths: BTreeSet<String>,
    /// Artifacts that must be retained in the receipt-bound extension Loaf.
    pub(crate) extension_paths: BTreeSet<String>,
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
    /// Exact registry source closure retained for build-system-neutral Rust inspection.
    pub source: OvenRustcRegistrySource,
    /// Digest-verified compiler artifact retained below the Loaf root.
    pub artifact: OvenRustcArtifactExtern,
}

/// Publisher-sealed registry source corresponding to one compiled registry leaf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenRustcRegistrySource {
    /// Cargo registry identity recorded by the exact publisher lock.
    pub registry: String,
    /// Registry archive checksum recorded by the exact publisher lock.
    pub checksum: String,
    /// Safe directory path relative to the immutable Loaf root.
    pub relative_root: String,
    /// Content digest of every regular source file and its portable relative path.
    pub digest: String,
}

/// One exact locked registry package in the sealed Rust-inspection source closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenRustcRegistrySourcePackage {
    /// Cargo package name recorded by the explicit publisher.
    pub package: String,
    /// Exact package version recorded by the explicit publisher lock.
    pub version: String,
    /// Unified feature set selected by the publisher's resolved graph.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Immutable source identity and artifact-root-relative location.
    pub source: OvenRustcRegistrySource,
}
