//! Cargo's own JSON shapes, as this publisher reads them.
//!
//! These mirror what `cargo` emits -- its build-artifact messages, unit graph, metadata resolve, and the
//! registry checksum lock. They are deserialization targets and carry no publisher behavior, so they are
//! grouped here to keep the publisher's own logic legible beside them.

use std::path::PathBuf;

use serde::Deserialize;

/// Minimal Cargo JSON message shape used to map publisher-built dependency artifacts back to unit-graph edges.
#[derive(Clone, Deserialize)]
pub(super) struct CargoCompilerArtifact {
    pub(super) reason: String,
    pub(super) package_id: String,
    pub(super) target: CargoCompilerArtifactTarget,
    #[serde(default)]
    pub(super) features: Vec<String>,
    #[serde(default)]
    pub(super) filenames: Vec<PathBuf>,
    #[serde(default)]
    pub(super) profile: CargoCompilerArtifactProfile,
}

#[derive(Clone, Default, Deserialize)]
pub(super) struct CargoCompilerArtifactProfile {
    #[serde(default)]
    pub(super) test: bool,
}

/// Target identity emitted by Cargo's stable JSON message stream.
#[derive(Clone, Deserialize)]
pub(super) struct CargoCompilerArtifactTarget {
    pub(super) name: String,
    #[serde(default)]
    pub(super) src_path: PathBuf,
}

/// Cargo's unstable-but-structured unit graph, read only at the named publisher boundary.
///
/// The graph is not retained as an execution dependency. Oven converts its workspace test roots, resolved features,
/// and direct dependency edges into a receipt-bound target plan before the transient Cargo target is reclaimed.
#[derive(Deserialize)]
pub(super) struct CargoUnitGraph {
    pub(super) version: u32,
    pub(super) units: Vec<CargoUnitGraphUnit>,
    pub(super) roots: Vec<usize>,
}

#[derive(Clone, Deserialize)]
pub(super) struct CargoUnitGraphUnit {
    pub(super) pkg_id: String,
    pub(super) target: CargoUnitGraphTarget,
    pub(super) mode: String,
    #[serde(default)]
    pub(super) platform: Option<String>,
    #[serde(default)]
    pub(super) features: Vec<String>,
    #[serde(default)]
    pub(super) dependencies: Vec<CargoUnitGraphDependency>,
}

#[derive(Clone, Deserialize)]
pub(super) struct CargoUnitGraphTarget {
    pub(super) kind: Vec<String>,
    #[serde(default)]
    pub(super) crate_types: Vec<String>,
    pub(super) name: String,
    pub(super) src_path: PathBuf,
    pub(super) edition: String,
}

#[derive(Clone, Deserialize)]
pub(super) struct CargoUnitGraphDependency {
    pub(super) index: usize,
    pub(super) extern_crate_name: Option<String>,
}

/// Minimal publisher-only Cargo metadata needed to name a sealed third-party foundation manifest.
///
/// The unit graph is authoritative for the resolved feature set and dependency edges; Cargo metadata supplies the
/// stable package name/version for a synthetic legacy publisher manifest. Neither record reaches an Oven consumer.
#[derive(Clone, Deserialize)]
pub(super) struct CargoMetadata {
    pub(super) packages: Vec<CargoMetadataPackage>,
    #[serde(default)]
    pub(super) resolve: Option<CargoMetadataResolve>,
}

/// Feature selections resolved by the explicit publisher's locked Cargo metadata call.
#[derive(Clone, Deserialize)]
pub(super) struct CargoMetadataResolve {
    #[serde(default)]
    pub(super) root: Option<String>,
    #[serde(default)]
    pub(super) nodes: Vec<CargoMetadataResolveNode>,
}

/// One exact package ID and its unified features in publisher metadata.
#[derive(Clone, Deserialize)]
pub(super) struct CargoMetadataResolveNode {
    pub(super) id: String,
    #[serde(default)]
    pub(super) features: Vec<String>,
    #[serde(default)]
    pub(super) dependencies: Vec<String>,
    /// Direct dependency edges with the Cargo name used by the root package and the exact resolved package ID.
    ///
    /// `dependencies` is sufficient for source-closure walking, but it discards the alias-to-package relationship
    /// needed to select a direct Rustc artifact when the lock contains multiple versions of the same crate.
    #[serde(default)]
    pub(super) deps: Vec<CargoMetadataResolveDependency>,
}

/// One resolved Cargo dependency edge retained only while the explicit baker is publishing a Loaf.
#[derive(Clone, Deserialize)]
pub(super) struct CargoMetadataResolveDependency {
    pub(super) name: String,
    pub(super) pkg: String,
}

/// One declared `rustc --extern` name bound to the exact Cargo package instance that owns its artifact.
///
/// Package names are not sufficient: a valid lock can contain two versions of one crate name. The resolved Cargo
/// package ID is therefore consumed at the explicit baker boundary and never guessed by a normal Oven command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedDirectDependency {
    pub(super) package: String,
    pub(super) package_id: String,
}

/// One Cargo package identity used while creating the explicit third-party foundation publisher input.
#[derive(Clone, Deserialize)]
pub(super) struct CargoMetadataPackage {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) version: String,
    pub(super) manifest_path: PathBuf,
    #[serde(default)]
    pub(super) source: Option<String>,
}

/// Exact registry checksum records decoded from the publisher's already-resolved Cargo lock.
#[derive(Deserialize)]
pub(super) struct CargoChecksumLock {
    #[serde(default)]
    pub(super) package: Vec<CargoChecksumLockPackage>,
}

/// One package identity whose checksum must agree with the source retained in a Loaf.
#[derive(Deserialize)]
pub(super) struct CargoChecksumLockPackage {
    pub(super) name: String,
    pub(super) version: String,
    #[serde(default)]
    pub(super) source: Option<String>,
    #[serde(default)]
    pub(super) checksum: Option<String>,
}
