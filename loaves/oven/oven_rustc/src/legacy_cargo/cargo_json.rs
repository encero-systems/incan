//! Cargo's own JSON shapes, as this publisher reads them.
//!
//! These mirror what `cargo` emits -- its build-artifact messages, unit graph, metadata resolve, and the
//! registry checksum lock. They are deserialization targets and carry no publisher behavior, so they are
//! grouped here to keep the publisher's own logic legible beside them.

use std::path::PathBuf;

use serde::Deserialize;

/// Minimal Cargo JSON message shape used to map publisher-built dependency artifacts back to unit-graph edges.
#[derive(Clone, Deserialize)]
pub(crate) struct CargoCompilerArtifact {
    pub(crate) reason: String,
    pub(crate) package_id: String,
    pub(crate) target: CargoCompilerArtifactTarget,
    #[serde(default)]
    pub(crate) features: Vec<String>,
    #[serde(default)]
    pub(crate) filenames: Vec<PathBuf>,
    #[serde(default)]
    pub(crate) profile: CargoCompilerArtifactProfile,
}

#[derive(Clone, Default, Deserialize)]
pub(crate) struct CargoCompilerArtifactProfile {
    #[serde(default)]
    pub(crate) test: bool,
}

/// Target identity emitted by Cargo's stable JSON message stream.
#[derive(Clone, Deserialize)]
pub(crate) struct CargoCompilerArtifactTarget {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) kind: Vec<String>,
    #[serde(default)]
    pub(crate) src_path: PathBuf,
}

/// Cargo's unstable-but-structured unit graph, read only at the named publisher boundary.
///
/// The graph is not retained as an execution dependency. Oven converts its workspace test roots, resolved features,
/// and direct dependency edges into a receipt-bound target plan before the transient Cargo target is reclaimed.
#[derive(Deserialize)]
pub struct CargoUnitGraph {
    pub(crate) version: u32,
    pub(crate) units: Vec<CargoUnitGraphUnit>,
    pub(crate) roots: Vec<usize>,
}

#[derive(Clone, Deserialize)]
pub struct CargoUnitGraphUnit {
    pub(crate) pkg_id: String,
    pub(crate) target: CargoUnitGraphTarget,
    pub(crate) mode: String,
    #[serde(default)]
    pub(crate) platform: Option<String>,
    #[serde(default)]
    pub(crate) features: Vec<String>,
    #[serde(default)]
    pub(crate) dependencies: Vec<CargoUnitGraphDependency>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct CargoUnitGraphTarget {
    pub(crate) kind: Vec<String>,
    #[serde(default)]
    pub(crate) crate_types: Vec<String>,
    pub(crate) name: String,
    pub(crate) src_path: PathBuf,
    pub(crate) edition: String,
}

#[derive(Clone, Deserialize)]
pub(crate) struct CargoUnitGraphDependency {
    pub(crate) index: usize,
    pub(crate) extern_crate_name: Option<String>,
}

/// Minimal publisher-only Cargo metadata needed to name a sealed third-party foundation manifest.
///
/// The unit graph is authoritative for the resolved feature set and dependency edges; Cargo metadata supplies the
/// stable package name/version for a synthetic legacy publisher manifest. Neither record reaches an Oven consumer.
#[derive(Clone, Deserialize)]
pub struct CargoMetadata {
    pub(crate) packages: Vec<CargoMetadataPackage>,
    #[serde(default)]
    pub(crate) resolve: Option<CargoMetadataResolve>,
}

/// Feature selections resolved by the explicit publisher's locked Cargo metadata call.
#[derive(Clone, Deserialize)]
pub(crate) struct CargoMetadataResolve {
    #[serde(default)]
    pub(crate) root: Option<String>,
    #[serde(default)]
    pub(crate) nodes: Vec<CargoMetadataResolveNode>,
}

/// One exact package ID and its unified features in publisher metadata.
#[derive(Clone, Deserialize)]
pub(crate) struct CargoMetadataResolveNode {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) features: Vec<String>,
    #[serde(default)]
    pub(crate) dependencies: Vec<String>,
    /// Direct dependency edges with the Cargo name used by the root package and the exact resolved package ID.
    ///
    /// `dependencies` is sufficient for source-closure walking, but it discards the alias-to-package relationship
    /// needed to select a direct Rustc artifact when the lock contains multiple versions of the same crate.
    #[serde(default)]
    pub(crate) deps: Vec<CargoMetadataResolveDependency>,
}

/// One resolved Cargo dependency edge retained only while the explicit baker is publishing a Loaf.
#[derive(Clone, Deserialize)]
pub(crate) struct CargoMetadataResolveDependency {
    pub(crate) name: String,
    pub(crate) pkg: String,
}

/// One declared `rustc --extern` name bound to the exact Cargo package instance that owns its artifact.
///
/// Package names are not sufficient: a valid lock can contain two versions of one crate name. The resolved Cargo
/// package ID is therefore consumed at the explicit baker boundary and never guessed by a normal Oven command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDirectDependency {
    pub(crate) package: String,
    pub(crate) package_id: String,
}

/// One Cargo package identity used while creating the explicit third-party foundation publisher input.
#[derive(Clone, Deserialize)]
pub(crate) struct CargoMetadataPackage {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) manifest_path: PathBuf,
    #[serde(default)]
    pub(crate) source: Option<String>,
}

/// Exact registry checksum records decoded from the publisher's already-resolved Cargo lock.
#[derive(Deserialize)]
pub(crate) struct CargoChecksumLock {
    #[serde(default)]
    pub(crate) package: Vec<CargoChecksumLockPackage>,
}

/// One package identity whose checksum must agree with the source retained in a Loaf.
#[derive(Deserialize)]
pub(crate) struct CargoChecksumLockPackage {
    pub(crate) name: String,
    pub(crate) version: String,
    #[serde(default)]
    pub(crate) source: Option<String>,
    #[serde(default)]
    pub(crate) checksum: Option<String>,
}
