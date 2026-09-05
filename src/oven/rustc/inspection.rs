//! Project-inspection authority payloads.
//!
//! The sealed description of what a project's inspection authority covers -- its sources, constituents, root and
//! test dependencies, and generated output directory -- as recorded for one schema version.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::super::OvenReceipt;
use super::super::store::{OvenArtifactKind, OvenStoreExecutionPayload, OvenStoreLease};
use super::artifact::OvenRustcRegistrySourcePackage;
use serde::{Deserialize, Serialize};

/// Wire schema for one project-level Rust inspection authority.
pub(crate) const OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION: u32 = 1;

/// Exact immutable source owner named by a project inspection authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "owner", rename_all = "snake_case")]
pub(crate) enum OvenProjectInspectionSourceOwner {
    /// The small project authority entry materializes this source fragment itself.
    Authority,
    /// One exact constituent supplies the source tree at the catalog's relative root.
    Constituent { index: usize },
}

/// One exact immutable constituent of a project inspection authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum OvenProjectInspectionConstituent {
    /// A compiler-shipped release Loaf retained by its immutable toolchain generation.
    ReleaseLoaf {
        loaf_identity: String,
        build_unit_identity: String,
        receipt: OvenReceipt,
    },
    /// A receipt-bound entry in the bounded project store.
    Stored {
        identity: String,
        artifact_kind: OvenArtifactKind,
        receipt: OvenReceipt,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_loaf_identity: Option<String>,
    },
}

/// One canonical registry source and the exact immutable root that owns its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectInspectionSource {
    pub package: OvenRustcRegistrySourcePackage,
    pub owner: OvenProjectInspectionSourceOwner,
}

/// One exact normal or dev registry root selected for project Rust inspection.
///
/// The locked package identity proves which source tree Cargo selected at the explicit bake boundary. The requested
/// feature contract remains separate because two source-identical root edges can expose different Rust APIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectInspectionRootDependency {
    pub alias: String,
    pub package: String,
    pub version: String,
    pub registry: String,
    pub checksum: String,
    pub requested_features: Vec<String>,
    pub default_features: bool,
}

/// Exact project-owned dependency envelope used only by generated native tests.
///
/// The envelope promotes the canonical normal and dev dependency surface into one checked debug executable closure at
/// the explicit project-bake boundary. Its dependency digest is deliberately independent of authored test bytes, so
/// unchanged dependency declarations reuse the same Loaf while every generated harness remains caller-owned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectInspectionTestDependencyEnvelope {
    pub constituent_index: usize,
    pub dependency_surface_digest: String,
    pub dependency_roots: BTreeMap<String, OvenProjectInspectionTestDependencyRoot>,
}

/// Exact per-root evidence admitted by the generated native-test dependency envelope.
///
/// Registry roots retain Cargo's locked package/source identity as well as the declared edge digest. Path and Git
/// roots carry their complete portable declaration/source digest; for paths that digest includes the source tree,
/// never its machine-local spelling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub(crate) enum OvenProjectInspectionTestDependencyRoot {
    Registry {
        dependency_digest: String,
        locked: OvenProjectInspectionRootDependency,
    },
    Path {
        dependency_digest: String,
    },
    Git {
        dependency_digest: String,
    },
}

/// Singular source authority published once by an explicit project bake.
///
/// The authority owns one canonical publisher lock and exact normal/dev root-edge records. Its source catalog is a
/// composition of named immutable constituents plus only those bounded source fragments absent from every
/// constituent; normal commands never union independent locks or search the store by dependency compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectInspectionAuthorityPayload {
    pub schema_version: u32,
    pub project_identity: String,
    pub source_authority_digest: String,
    pub compiler_version: String,
    pub registry_lock_digest: String,
    #[serde(default)]
    pub registry_source_dependencies: Vec<OvenProjectInspectionRootDependency>,
    #[serde(default)]
    pub dev_registry_source_dependencies: Vec<OvenProjectInspectionRootDependency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_dependency_envelope: Option<OvenProjectInspectionTestDependencyEnvelope>,
    #[serde(default)]
    pub constituents: Vec<OvenProjectInspectionConstituent>,
    #[serde(default)]
    pub registry_sources: Vec<OvenProjectInspectionSource>,
    /// Build-script output directories the explicit bake sealed below this authority's artifact root.
    ///
    /// Generated Rust such as prost's `include!`d modules exists as files only where the bake's Cargo bootstrap
    /// wrote its `OUT_DIR`s. A normal command never runs Cargo, so the authority carries those files itself and a
    /// direct inspection workspace reads them the way it would read a Cargo `OUT_DIR`.
    #[serde(default)]
    pub generated_out_dirs: Vec<OvenProjectInspectionGeneratedOutDir>,
}

/// One sealed build-script output directory, laid out as `generated-out-dirs/build/<crate>-<hash>/out` so the
/// generated-code route recognizes it like a Cargo target directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectInspectionGeneratedOutDir {
    /// Cargo package whose build script produced the directory.
    pub crate_name: String,
    /// Directory below the authority's artifact root holding the sealed `*.rs` output.
    pub relative_root: String,
    /// Exact package version whose build script wrote the directory, when the bake knew it. A closure can hold
    /// several build units of one package, and a consumer reads only the unit built from the version it inspects.
    #[serde(default)]
    pub version: Option<String>,
}

/// Exact authority entry named by a source-current completed project output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectInspectionAuthorityRef {
    pub identity: String,
    pub receipt_identity: String,
    pub build_unit_identity: String,
}

/// Source-current singular project authority with every bounded-store constituent leased in one batch.
pub(crate) struct OvenLoadedProjectInspectionAuthority {
    pub(crate) identity: String,
    pub(crate) artifact_root: PathBuf,
    pub(crate) payload: OvenProjectInspectionAuthorityPayload,
    pub(crate) stored_constituents: Vec<OvenStoreExecutionPayload>,
    pub(super) _authority_lease: OvenStoreLease,
    pub(super) lineage_leases: Vec<OvenStoreLease>,
}

impl OvenLoadedProjectInspectionAuthority {
    /// Retain completed-output leases for the complete inspection command.
    pub(crate) fn retain_lineage_leases(&mut self, leases: Vec<OvenStoreLease>) {
        self.lineage_leases = leases;
    }
}
