//! Project-inspection authority payloads.
//!
//! The sealed description of what a project's inspection authority covers -- its sources, constituents, root and
//! test dependencies -- as recorded for one schema version.

use std::collections::BTreeMap;
use std::path::Path;

use super::artifact::OvenRustcRegistrySourcePackage;
use oven_store::OvenReceipt;
use oven_store::store::{OvenArtifactKind, OvenStoreExecutionPayload, OvenStoreLease};
use serde::{Deserialize, Serialize};

/// Wire schema for one project-level Rust inspection authority.
pub const OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION: u32 = 2;

mod selected_rust_facet_graph;

pub use selected_rust_facet_graph::*;

/// Exact immutable source owner named by a project inspection authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "owner", rename_all = "snake_case")]
pub enum OvenProjectInspectionSourceOwner {
    /// The small project authority entry materializes this source fragment itself.
    Authority,
    /// One exact constituent supplies the source tree at the catalog's relative root.
    Constituent { index: usize },
}

/// One exact immutable constituent of a project inspection authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OvenProjectInspectionConstituent {
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
pub struct OvenProjectInspectionSource {
    pub package: OvenRustcRegistrySourcePackage,
    pub owner: OvenProjectInspectionSourceOwner,
}

/// One exact normal or dev registry root selected for project Rust inspection.
///
/// The locked package identity proves which source tree Cargo selected at the explicit bake boundary. The requested
/// feature contract remains separate because two source-identical root edges can expose different Rust APIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenProjectInspectionRootDependency {
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
pub struct OvenProjectInspectionTestDependencyEnvelope {
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
pub enum OvenProjectInspectionTestDependencyRoot {
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
pub struct OvenProjectInspectionAuthorityPayload {
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
    /// Sealed build-script output directories the bake's Cargo targets wrote.
    ///
    /// Retained on the dev line while the legacy-Cargo bake still produces them. The direct-rustc route has no
    /// build-script phase of its own yet, so dropping the field here would lose the only record a normal command
    /// has of generated code the authority already sealed.
    #[serde(default)]
    pub generated_out_dirs: Vec<OvenProjectInspectionGeneratedOutDir>,
}

/// Exact authority entry named by a source-current completed project output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenProjectInspectionAuthorityRef {
    pub identity: String,
    pub receipt_identity: String,
    pub build_unit_identity: String,
}

/// Source-current singular project authority with every bounded-store constituent leased in one batch.
pub struct OvenLoadedProjectInspectionAuthority {
    source_owner: OvenStoreExecutionPayload,
    pub payload: OvenProjectInspectionAuthorityPayload,
    pub stored_constituents: Vec<OvenStoreExecutionPayload>,
    pub(crate) lineage_leases: Vec<OvenStoreLease>,
}

impl OvenLoadedProjectInspectionAuthority {
    /// Retain one validated authority owner together with every store-owned constituent it names.
    pub(crate) fn new(
        source_owner: OvenStoreExecutionPayload,
        payload: OvenProjectInspectionAuthorityPayload,
        stored_constituents: Vec<OvenStoreExecutionPayload>,
    ) -> Self {
        Self {
            source_owner,
            payload,
            stored_constituents,
            lineage_leases: Vec::new(),
        }
    }

    /// Return the immutable Store identity that owns the authority payload and materialized files.
    pub fn identity(&self) -> &str {
        &self.source_owner.manifest.identity
    }

    /// Return the materialized root derived from the retained authority owner.
    pub fn artifact_root(&self) -> &Path {
        &self.source_owner.artifact_root
    }

    /// Retain completed-output leases for the complete inspection command.
    pub fn retain_lineage_leases(&mut self, leases: Vec<OvenStoreLease>) {
        self.lineage_leases = leases;
    }
}

#[cfg(test)]
mod selected_rust_facet_graph_tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn dependency_owner_identity() -> String {
        selected_graph_sha256(b"dependency owner")
    }

    fn project_owner_identity(root_bytes: &[u8]) -> String {
        selected_graph_sha256(root_bytes)
    }

    fn toolchain_owner_identity() -> String {
        selected_graph_sha256(b"toolchain inspection closure")
    }

    fn member(path: &str, bytes: &[u8]) -> OvenSelectedRustFacetSourceMember {
        OvenSelectedRustFacetSourceMember {
            path: path.to_string(),
            digest: selected_graph_sha256(bytes),
        }
    }

    fn source(
        kind: OvenSelectedRustFacetSourceKind,
        identity: &str,
        owner: &str,
        members: &[OvenSelectedRustFacetSourceMember],
    ) -> Result<OvenSelectedRustFacetSource, OvenSelectedRustFacetGraphError> {
        Ok(OvenSelectedRustFacetSource {
            kind,
            identity: identity.to_string(),
            owner: owner.to_string(),
            root: ".".to_string(),
            digest: selected_graph_source_digest(members)?,
        })
    }

    fn cfg_snapshot(architecture: &str, operating_system: &str) -> OvenSelectedRustFacetCfgSnapshot {
        OvenSelectedRustFacetCfgSnapshot {
            flags: vec!["unix".to_string()],
            values: BTreeMap::from([
                ("target_arch".to_string(), vec![architecture.to_string()]),
                ("target_os".to_string(), vec![operating_system.to_string()]),
            ]),
        }
    }

    fn selection() -> OvenSelectedRustFacetSelection {
        OvenSelectedRustFacetSelection {
            intent: OvenSelectedRustFacetIntent {
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: "rustc 1.85.0 (fixture)".to_string(),
                profile: "dev".to_string(),
                features: vec!["root-feature".to_string()],
            },
            host: "aarch64-apple-darwin".to_string(),
            host_cfg: cfg_snapshot("aarch64", "macos"),
            target_cfg: cfg_snapshot("x86_64", "linux"),
            purpose: OvenSelectedRustFacetPurpose::Normal,
            default_features: true,
            toolchain_version: "1.85.0".to_string(),
            target_spec: OvenSelectedRustFacetTargetSpec {
                source: OvenSelectedRustFacetPath {
                    owner: toolchain_owner_identity(),
                    path: "target-specs/x86_64-unknown-linux-gnu.json".to_string(),
                },
                digest: selected_graph_sha256(b"fixture target spec"),
            },
        }
    }

    fn leaf_unit(
        selection: &OvenSelectedRustFacetSelection,
    ) -> Result<OvenSelectedRustFacetUnit, OvenSelectedRustFacetGraphError> {
        let source_members = vec![member("src/lib.rs", b"pub struct Dependency;\n")];
        let mut unit = OvenSelectedRustFacetUnit {
            identity: String::new(),
            package: "dependency-package".to_string(),
            package_version: "1.2.3".to_string(),
            crate_name: "dependency_crate".to_string(),
            crate_kind: OvenSelectedRustFacetCrateKind::Rlib,
            role: OvenSelectedRustFacetUnitRole::Library,
            domain: OvenSelectedRustFacetDomain::Target,
            edition: "2021".to_string(),
            source: source(
                OvenSelectedRustFacetSourceKind::Registry,
                "registry:dependency-package@1.2.3",
                &dependency_owner_identity(),
                &source_members,
            )?,
            root_module: "src/lib.rs".to_string(),
            source_members,
            features: Vec::new(),
            default_features: false,
            cfg: Vec::new(),
            environment: BTreeMap::new(),
            include_dirs: vec![OvenSelectedRustFacetPath {
                owner: dependency_owner_identity(),
                path: ".".to_string(),
            }],
            exclude_dirs: Vec::new(),
            dependencies: Vec::new(),
            generated_inputs: Vec::new(),
        };
        unit.identity = selected_graph_unit_identity(selection, &unit)?;
        Ok(unit)
    }

    fn root_unit(
        selection: &OvenSelectedRustFacetSelection,
        dependency_identity: Option<&str>,
        bytes: &[u8],
    ) -> Result<OvenSelectedRustFacetUnit, OvenSelectedRustFacetGraphError> {
        let source_members = vec![member("src/lib.rs", bytes)];
        let dependencies = dependency_identity.map_or_else(Vec::new, |identity| {
            vec![OvenSelectedRustFacetDependency {
                alias: "renamed_dep".to_string(),
                unit: identity.to_string(),
            }]
        });
        let owner = project_owner_identity(bytes);
        let mut unit = OvenSelectedRustFacetUnit {
            identity: String::new(),
            package: "fixture".to_string(),
            package_version: "0.1.0".to_string(),
            crate_name: "fixture".to_string(),
            crate_kind: OvenSelectedRustFacetCrateKind::Rlib,
            role: OvenSelectedRustFacetUnitRole::Library,
            domain: OvenSelectedRustFacetDomain::Target,
            edition: "2024".to_string(),
            source: source(
                OvenSelectedRustFacetSourceKind::Generated,
                "generated:fixture",
                &owner,
                &source_members,
            )?,
            root_module: "src/lib.rs".to_string(),
            source_members,
            features: vec!["root-feature".to_string()],
            default_features: true,
            cfg: vec!["feature=\"root-feature\"".to_string()],
            environment: BTreeMap::new(),
            include_dirs: vec![OvenSelectedRustFacetPath {
                owner,
                path: ".".to_string(),
            }],
            exclude_dirs: Vec::new(),
            dependencies,
            generated_inputs: Vec::new(),
        };
        unit.identity = selected_graph_unit_identity(selection, &unit)?;
        Ok(unit)
    }

    fn graph(root_bytes: &[u8]) -> Result<OvenSelectedRustFacetGraph, OvenSelectedRustFacetGraphError> {
        let selection = selection();
        let leaf = leaf_unit(&selection)?;
        let root = root_unit(&selection, Some(&leaf.identity), root_bytes)?;
        let exposed_root = root.identity.clone();
        Ok(OvenSelectedRustFacetGraph {
            schema_version: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
            selection,
            owners: vec![
                OvenSelectedRustFacetOwner {
                    identity: project_owner_identity(root_bytes),
                    kind: OvenSelectedRustFacetOwnerKind::ProjectAuthority,
                },
                OvenSelectedRustFacetOwner {
                    identity: toolchain_owner_identity(),
                    kind: OvenSelectedRustFacetOwnerKind::Toolchain,
                },
                OvenSelectedRustFacetOwner {
                    identity: dependency_owner_identity(),
                    kind: OvenSelectedRustFacetOwnerKind::Constituent,
                },
            ],
            units: vec![root, leaf],
            exposed_roots: BTreeMap::from([("fixture".to_string(), exposed_root)]),
        })
    }

    fn single_unit_graph(
        role: OvenSelectedRustFacetUnitRole,
        domain: OvenSelectedRustFacetDomain,
        crate_kind: OvenSelectedRustFacetCrateKind,
    ) -> Result<OvenSelectedRustFacetGraph, OvenSelectedRustFacetGraphError> {
        let selection = selection();
        let bytes = b"pub fn fixture() {}\n";
        let mut unit = root_unit(&selection, None, bytes)?;
        unit.role = role;
        unit.domain = domain;
        unit.crate_kind = crate_kind;
        unit.identity = selected_graph_unit_identity(&selection, &unit)?;
        let identity = unit.identity.clone();
        Ok(OvenSelectedRustFacetGraph {
            schema_version: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
            selection,
            owners: vec![
                OvenSelectedRustFacetOwner {
                    identity: project_owner_identity(bytes),
                    kind: OvenSelectedRustFacetOwnerKind::ProjectAuthority,
                },
                OvenSelectedRustFacetOwner {
                    identity: toolchain_owner_identity(),
                    kind: OvenSelectedRustFacetOwnerKind::Toolchain,
                },
            ],
            units: vec![unit],
            exposed_roots: BTreeMap::from([("fixture".to_string(), identity)]),
        })
    }

    fn unit_index(graph: &OvenSelectedRustFacetGraph, package: &str) -> Result<usize, &'static str> {
        graph
            .units
            .iter()
            .position(|unit| unit.package == package)
            .ok_or("fixture graph lost a unit")
    }

    fn reidentify_unit(
        graph: &mut OvenSelectedRustFacetGraph,
        index: usize,
    ) -> Result<(), OvenSelectedRustFacetGraphError> {
        let old = graph.units[index].identity.clone();
        let new = selected_graph_unit_identity(&graph.selection, &graph.units[index])?;
        graph.units[index].identity = new.clone();
        for unit in &mut graph.units {
            for dependency in &mut unit.dependencies {
                if dependency.unit == old {
                    dependency.unit = new.clone();
                }
            }
        }
        for identity in graph.exposed_roots.values_mut() {
            if *identity == old {
                *identity = new.clone();
            }
        }
        Ok(())
    }

    fn environment_text(value: &str) -> OvenSelectedRustFacetEnvironmentValue {
        OvenSelectedRustFacetEnvironmentValue::Text {
            value: value.to_string(),
        }
    }

    /// Stand in for the producer's keyed redaction; a fixture only needs well-formed, distinguishable identities.
    fn environment_sensitive_digest(material: &[u8]) -> OvenSelectedRustFacetEnvironmentValue {
        OvenSelectedRustFacetEnvironmentValue::SensitiveDigest {
            hmac_sha256: selected_graph_sha256(material).replace("sha256:", "hmac-sha256:"),
        }
    }

    fn graph_with_root_environment(
        environment: &[(&str, OvenSelectedRustFacetEnvironmentValue)],
    ) -> Result<OvenSelectedRustFacetGraph, Box<dyn std::error::Error>> {
        let mut graph = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&graph, "fixture")?;
        for (name, value) in environment {
            graph.units[root].environment.insert((*name).to_string(), value.clone());
        }
        reidentify_unit(&mut graph, root)?;
        Ok(graph)
    }

    /// Build a one-unit graph whose source carries the supplied kind and logical identity.
    ///
    /// The owner identity is derived from the source bytes, never from where those bytes were read, which is what
    /// lets the same logical source relocate between physical roots.
    fn kind_source_graph(
        kind: OvenSelectedRustFacetSourceKind,
        source_identity: &str,
        bytes: &[u8],
    ) -> Result<OvenSelectedRustFacetGraph, OvenSelectedRustFacetGraphError> {
        let selection = selection();
        let mut unit = root_unit(&selection, None, bytes)?;
        unit.source.kind = kind;
        unit.source.identity = source_identity.to_string();
        // A delivery location the physical adapter rebinds; relocation must not reach the portable graph through it.
        unit.environment.insert(
            "OUT_DIR".to_string(),
            OvenSelectedRustFacetEnvironmentValue::Path {
                value: OvenSelectedRustFacetPath {
                    owner: project_owner_identity(bytes),
                    path: "generated/out".to_string(),
                },
            },
        );
        unit.identity = selected_graph_unit_identity(&selection, &unit)?;
        let identity = unit.identity.clone();
        Ok(OvenSelectedRustFacetGraph {
            schema_version: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
            selection,
            owners: vec![
                OvenSelectedRustFacetOwner {
                    identity: project_owner_identity(bytes),
                    kind: OvenSelectedRustFacetOwnerKind::ProjectAuthority,
                },
                OvenSelectedRustFacetOwner {
                    identity: toolchain_owner_identity(),
                    kind: OvenSelectedRustFacetOwnerKind::Toolchain,
                },
            ],
            units: vec![unit],
            exposed_roots: BTreeMap::from([("fixture".to_string(), identity)]),
        })
    }

    fn path_source_graph(
        source_identity: &str,
        bytes: &[u8],
    ) -> Result<OvenSelectedRustFacetGraph, OvenSelectedRustFacetGraphError> {
        kind_source_graph(OvenSelectedRustFacetSourceKind::Path, source_identity, bytes)
    }

    /// Return the refusal field for a graph that must not validate, whichever refusal variant it produces.
    fn refusal_field(graph: OvenSelectedRustFacetGraph, subject: &str) -> Result<String, Box<dyn std::error::Error>> {
        match graph.validated() {
            Ok(_) => Err(format!("{subject} was admitted into a portable graph").into()),
            Err(
                OvenSelectedRustFacetGraphError::Invalid { field, .. }
                | OvenSelectedRustFacetGraphError::Missing { field },
            ) => Ok(field),
            Err(other) => Err(format!("{subject} was refused for an unrelated reason: {other}").into()),
        }
    }

    #[test]
    fn selected_graph_preserves_renamed_dependency_alias() -> TestResult {
        let selected = graph(b"pub fn use_dependency() {}\n")?.validated()?;
        let root = selected
            .graph()
            .units
            .iter()
            .find(|unit| unit.package == "fixture")
            .ok_or("validated graph lost its root unit")?;
        let dependency = selected
            .graph()
            .units
            .iter()
            .find(|unit| unit.package == "dependency-package")
            .ok_or("validated graph lost its dependency unit")?;
        assert_eq!(
            root.dependencies,
            vec![OvenSelectedRustFacetDependency {
                alias: "renamed_dep".to_string(),
                unit: dependency.identity.clone(),
            }]
        );
        let encoded = selected.to_json_bytes()?;
        let decoded = OvenSelectedRustFacetGraph::decode_validated(&encoded)?;
        assert_eq!(decoded.graph(), selected.graph());
        assert_eq!(decoded.digest(), selected.digest());
        Ok(())
    }

    #[test]
    fn selected_graph_retains_explicit_empty_leaves_and_rejects_missing_leaf() -> TestResult {
        let selected = graph(b"pub fn use_dependency() {}\n")?.validated()?;
        let encoded = selected.to_json_bytes()?;
        let mut document = serde_json::from_slice::<serde_json::Value>(&encoded)?;
        let leaf = document["units"]
            .as_array()
            .ok_or("serialized graph has no unit array")?
            .iter()
            .find(|unit| unit["package"].as_str() == Some("dependency-package"))
            .ok_or("serialized graph lost its leaf unit")?;
        for field in ["dependencies", "features", "cfg", "exclude_dirs", "generated_inputs"] {
            assert_eq!(leaf[field].as_array().map(Vec::len), Some(0));
        }
        assert_eq!(leaf["environment"].as_object().map(serde_json::Map::len), Some(0));

        let missing_leaf = document["units"]
            .as_array_mut()
            .ok_or("serialized graph has no mutable unit array")?
            .iter_mut()
            .find(|unit| unit["package"].as_str() == Some("dependency-package"))
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("serialized graph lost its mutable leaf unit")?;
        assert!(missing_leaf.remove("dependencies").is_some());
        let missing_bytes = serde_json::to_vec(&document)?;
        assert!(matches!(
            OvenSelectedRustFacetGraph::decode_validated(&missing_bytes),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));
        Ok(())
    }

    #[test]
    fn selected_graph_accepts_host_and_target_units_sharing_one_source_root() -> TestResult {
        let mut graph = graph(b"pub fn shared() {}\n")?;
        let root_index = unit_index(&graph, "fixture")?;
        let mut host = graph.units[root_index].clone();
        host.domain = OvenSelectedRustFacetDomain::Host;
        host.dependencies.clear();
        host.identity = selected_graph_unit_identity(&graph.selection, &host)?;
        graph
            .exposed_roots
            .insert("fixture_host".to_string(), host.identity.clone());
        graph.units.push(host);

        let selected = graph.validated()?;
        let shared = selected
            .graph()
            .units
            .iter()
            .filter(|unit| unit.package == "fixture")
            .collect::<Vec<_>>();
        assert_eq!(shared.len(), 2);
        let first = shared.first().ok_or("validated graph lost first shared-root unit")?;
        let second = shared.get(1).ok_or("validated graph lost second shared-root unit")?;
        assert_eq!(first.source.owner, second.source.owner);
        assert_eq!(first.source.root, second.source.root);
        assert_eq!(first.root_module, second.root_module);
        assert_ne!(first.domain, second.domain);
        Ok(())
    }

    #[test]
    fn selected_graph_enforces_every_inspection_role_combination() -> TestResult {
        let roles = [
            OvenSelectedRustFacetUnitRole::Library,
            OvenSelectedRustFacetUnitRole::Binary,
            OvenSelectedRustFacetUnitRole::UnitTest,
            OvenSelectedRustFacetUnitRole::IntegrationTest,
            OvenSelectedRustFacetUnitRole::Example,
            OvenSelectedRustFacetUnitRole::Doctest,
            OvenSelectedRustFacetUnitRole::Benchmark,
            OvenSelectedRustFacetUnitRole::ProcMacro,
            OvenSelectedRustFacetUnitRole::CompilerSupport,
            OvenSelectedRustFacetUnitRole::CallerProjection,
        ];
        let domains = [OvenSelectedRustFacetDomain::Host, OvenSelectedRustFacetDomain::Target];
        let crate_kinds = [
            OvenSelectedRustFacetCrateKind::Rlib,
            OvenSelectedRustFacetCrateKind::Binary,
            OvenSelectedRustFacetCrateKind::ProcMacro,
        ];
        for role in roles {
            for domain in domains {
                for crate_kind in crate_kinds {
                    let expected = matches!(
                        (role, domain, crate_kind),
                        (
                            OvenSelectedRustFacetUnitRole::Library | OvenSelectedRustFacetUnitRole::CompilerSupport,
                            OvenSelectedRustFacetDomain::Host | OvenSelectedRustFacetDomain::Target,
                            OvenSelectedRustFacetCrateKind::Rlib
                        ) | (
                            OvenSelectedRustFacetUnitRole::Binary
                                | OvenSelectedRustFacetUnitRole::UnitTest
                                | OvenSelectedRustFacetUnitRole::IntegrationTest
                                | OvenSelectedRustFacetUnitRole::Example
                                | OvenSelectedRustFacetUnitRole::Doctest
                                | OvenSelectedRustFacetUnitRole::Benchmark,
                            OvenSelectedRustFacetDomain::Target,
                            OvenSelectedRustFacetCrateKind::Binary
                        ) | (
                            OvenSelectedRustFacetUnitRole::ProcMacro,
                            OvenSelectedRustFacetDomain::Host,
                            OvenSelectedRustFacetCrateKind::ProcMacro
                        ) | (
                            OvenSelectedRustFacetUnitRole::CallerProjection,
                            OvenSelectedRustFacetDomain::Target,
                            OvenSelectedRustFacetCrateKind::Rlib
                        )
                    );
                    let actual = single_unit_graph(role, domain, crate_kind)?.validated().is_ok();
                    assert_eq!(
                        actual, expected,
                        "role={role:?} domain={domain:?} crate_kind={crate_kind:?}"
                    );
                }
            }
        }

        let selected = single_unit_graph(
            OvenSelectedRustFacetUnitRole::Library,
            OvenSelectedRustFacetDomain::Target,
            OvenSelectedRustFacetCrateKind::Rlib,
        )?
        .validated()?;
        for unsupported in ["build_script", "generated_source", "carrier"] {
            let mut document = serde_json::from_slice::<serde_json::Value>(&selected.to_json_bytes()?)?;
            let unit = document["units"]
                .as_array_mut()
                .and_then(|units| units.first_mut())
                .ok_or("serialized role graph lost its unit")?;
            unit["role"] = serde_json::json!(unsupported);
            let bytes = serde_json::to_vec(&document)?;
            assert!(matches!(
                OvenSelectedRustFacetGraph::decode_validated(&bytes),
                Err(OvenSelectedRustFacetGraphError::Invalid { .. })
            ));
        }
        Ok(())
    }

    #[test]
    fn selected_graph_binds_target_spec_to_a_toolchain_owner() -> TestResult {
        let selected = graph(b"pub fn use_dependency() {}\n")?.validated()?;
        let target_owner = &selected.graph().selection.target_spec.source.owner;
        let owner = selected
            .graph()
            .owners
            .iter()
            .find(|owner| &owner.identity == target_owner)
            .ok_or("validated graph lost target-spec owner")?;
        assert_eq!(owner.kind, OvenSelectedRustFacetOwnerKind::Toolchain);

        let mut wrong_target_spec_owner = graph(b"pub fn use_dependency() {}\n")?;
        wrong_target_spec_owner.selection.target_spec.source.owner = dependency_owner_identity();
        assert!(matches!(
            wrong_target_spec_owner.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { field, .. })
                if field == "selection.target_spec.source.owner"
        ));

        let mut missing_toolchain_kind = graph(b"pub fn use_dependency() {}\n")?;
        let toolchain = toolchain_owner_identity();
        let owner = missing_toolchain_kind
            .owners
            .iter_mut()
            .find(|owner| owner.identity == toolchain)
            .ok_or("fixture graph lost toolchain owner")?;
        owner.kind = OvenSelectedRustFacetOwnerKind::Constituent;
        assert!(matches!(
            missing_toolchain_kind.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut duplicate_toolchain_kind = graph(b"pub fn use_dependency() {}\n")?;
        duplicate_toolchain_kind.owners.push(OvenSelectedRustFacetOwner {
            identity: selected_graph_sha256(b"second toolchain closure"),
            kind: OvenSelectedRustFacetOwnerKind::Toolchain,
        });
        assert!(matches!(
            duplicate_toolchain_kind.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut missing_owner = graph(b"pub fn use_dependency() {}\n")?;
        missing_owner.selection.target_spec.source.owner = selected_graph_sha256(b"absent toolchain owner");
        assert!(matches!(
            missing_owner.validated(),
            Err(OvenSelectedRustFacetGraphError::Missing { .. })
        ));

        let mut compiler_from_constituent = graph(b"pub fn use_dependency() {}\n")?;
        let dependency = unit_index(&compiler_from_constituent, "dependency-package")?;
        compiler_from_constituent
            .units
            .get_mut(dependency)
            .ok_or("fixture graph lost dependency unit")?
            .source
            .kind = OvenSelectedRustFacetSourceKind::Compiler;
        assert!(matches!(
            compiler_from_constituent.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { field, .. }) if field.ends_with(".source.owner")
        ));

        let mut registry_from_toolchain = graph(b"pub fn use_dependency() {}\n")?;
        let dependency = unit_index(&registry_from_toolchain, "dependency-package")?;
        registry_from_toolchain
            .units
            .get_mut(dependency)
            .ok_or("fixture graph lost dependency unit")?
            .source
            .owner = toolchain_owner_identity();
        assert!(matches!(
            registry_from_toolchain.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { field, .. }) if field.ends_with(".source.owner")
        ));
        Ok(())
    }

    #[test]
    fn selected_graph_rejects_detached_units_and_owners() -> TestResult {
        let mut detached_unit = graph(b"pub fn use_dependency() {}\n")?;
        let selection = detached_unit.selection.clone();
        let mut unit = leaf_unit(&selection)?;
        unit.domain = OvenSelectedRustFacetDomain::Host;
        unit.identity = selected_graph_unit_identity(&selection, &unit)?;
        detached_unit.units.push(unit);
        assert!(matches!(
            detached_unit.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut detached_owner = graph(b"pub fn use_dependency() {}\n")?;
        detached_owner.owners.push(OvenSelectedRustFacetOwner {
            identity: selected_graph_sha256(b"detached owner"),
            kind: OvenSelectedRustFacetOwnerKind::GeneratedOutput,
        });
        assert!(matches!(
            detached_owner.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));
        Ok(())
    }

    #[test]
    fn selected_graph_checks_schema_before_strict_nested_wire_fields() -> TestResult {
        let selected = graph(b"pub fn use_dependency() {}\n")?.validated()?;
        let mut current = serde_json::from_slice::<serde_json::Value>(&selected.to_json_bytes()?)?;
        current["selection"]["intent"]["future_field"] = serde_json::json!(true);
        let current_bytes = serde_json::to_vec(&current)?;
        assert!(matches!(
            OvenSelectedRustFacetGraph::decode_validated(&current_bytes),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let future = serde_json::to_vec(&serde_json::json!({
            "schema_version": OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION + 1,
            "selection": { "intent": { "future_field": true } },
            "future_top_level": true
        }))?;
        assert!(matches!(
            OvenSelectedRustFacetGraph::decode_validated(&future),
            Err(OvenSelectedRustFacetGraphError::UnsupportedSchema { .. })
        ));
        Ok(())
    }

    #[test]
    fn selected_graph_requires_complete_canonical_cfg_snapshots() -> TestResult {
        let selected = graph(b"pub fn use_dependency() {}\n")?.validated()?;
        let mut missing = serde_json::from_slice::<serde_json::Value>(&selected.to_json_bytes()?)?;
        let selection = missing["selection"]
            .as_object_mut()
            .ok_or("serialized graph lost selection")?;
        selection.remove("host_cfg");
        assert!(matches!(
            OvenSelectedRustFacetGraph::decode_validated(&serde_json::to_vec(&missing)?),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut empty = graph(b"pub fn use_dependency() {}\n")?;
        empty.selection.host_cfg = OvenSelectedRustFacetCfgSnapshot {
            flags: Vec::new(),
            values: BTreeMap::new(),
        };
        assert!(matches!(
            empty.validated(),
            Err(OvenSelectedRustFacetGraphError::Missing { field }) if field == "selection.host_cfg"
        ));

        let mut unordered_flags = graph(b"pub fn use_dependency() {}\n")?;
        unordered_flags
            .selection
            .host_cfg
            .flags
            .push("debug_assertions".to_string());
        assert!(matches!(
            unordered_flags.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { field, .. }) if field == "selection.host_cfg.flags"
        ));

        let mut unordered_values = graph(b"pub fn use_dependency() {}\n")?;
        let target_arch = unordered_values
            .selection
            .target_cfg
            .values
            .get_mut("target_arch")
            .ok_or("fixture snapshot lost target architecture")?;
        target_arch.push("aarch64".to_string());
        assert!(matches!(
            unordered_values.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { field, .. }) if field == "selection.target_cfg.values.target_arch"
        ));
        Ok(())
    }

    #[test]
    fn selected_graph_digest_binds_host_and_target_cfg_snapshots() -> TestResult {
        let first = graph(b"pub fn use_dependency() {}\n")?.validated()?;
        let mut changed = graph(b"pub fn use_dependency() {}\n")?;
        let target_os = changed
            .selection
            .target_cfg
            .values
            .get_mut("target_os")
            .ok_or("fixture snapshot lost target operating system")?;
        target_os.clear();
        target_os.push("macos".to_string());
        let changed = changed.validated()?;
        assert_ne!(first.digest(), changed.digest());
        Ok(())
    }

    #[test]
    fn selected_graph_eliminates_alpha_labels_and_normalizes_insertion_order() -> TestResult {
        let first = graph(b"pub fn use_dependency() {}\n")?.validated()?;
        let mut reordered = graph(b"pub fn use_dependency() {}\n")?;
        reordered.owners.reverse();
        reordered.units.reverse();
        reordered.selection.intent.features.reverse();
        for unit in &mut reordered.units {
            unit.features.reverse();
            unit.cfg.reverse();
            unit.include_dirs.reverse();
            unit.exclude_dirs.reverse();
            unit.dependencies.reverse();
            unit.generated_inputs.reverse();
            unit.source_members.reverse();
        }
        let reordered = reordered.validated()?;
        assert_eq!(first.digest(), reordered.digest());
        assert_eq!(first.to_json_bytes()?, reordered.to_json_bytes()?);
        let encoded = String::from_utf8(first.to_json_bytes()?)?;
        assert!(!encoded.contains("unit-root"));
        assert!(!encoded.contains("owner-project"));
        assert!(!encoded.contains("\"id\""));
        Ok(())
    }

    #[test]
    fn selected_graph_rejects_cycles_and_duplicates() -> TestResult {
        let mut cycle = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&cycle, "fixture")?;
        let leaf = unit_index(&cycle, "dependency-package")?;
        let root_identity = cycle
            .units
            .get(root)
            .ok_or("fixture graph lost root unit")?
            .identity
            .clone();
        cycle
            .units
            .get_mut(leaf)
            .ok_or("fixture graph lost leaf unit")?
            .dependencies
            .push(OvenSelectedRustFacetDependency {
                alias: "fixture".to_string(),
                unit: root_identity,
            });
        assert!(matches!(
            cycle.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut duplicate_feature = graph(b"pub fn use_dependency() {}\n")?;
        duplicate_feature
            .selection
            .intent
            .features
            .push("root-feature".to_string());
        assert!(matches!(
            duplicate_feature.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut duplicate_owner = graph(b"pub fn use_dependency() {}\n")?;
        let owner = duplicate_owner
            .owners
            .first()
            .ok_or("fixture graph lost first owner")?
            .clone();
        duplicate_owner.owners.push(owner);
        assert!(matches!(
            duplicate_owner.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut duplicate_alias = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&duplicate_alias, "fixture")?;
        let dependency = duplicate_alias
            .units
            .get(root)
            .and_then(|unit| unit.dependencies.first())
            .ok_or("fixture graph lost root dependency")?
            .clone();
        duplicate_alias
            .units
            .get_mut(root)
            .ok_or("fixture graph lost root unit")?
            .dependencies
            .push(dependency);
        assert!(matches!(
            duplicate_alias.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));
        Ok(())
    }

    #[test]
    fn selected_graph_rejects_invalid_portable_paths() -> TestResult {
        let mut absolute_source = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&absolute_source, "fixture")?;
        absolute_source.units[root].source.root = "/tmp/fixture".to_string();
        assert!(matches!(
            absolute_source.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut owner_root_target_spec = graph(b"pub fn use_dependency() {}\n")?;
        owner_root_target_spec.selection.target_spec.source.path = ".".to_string();
        assert!(matches!(
            owner_root_target_spec.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut escaping_include = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&escaping_include, "fixture")?;
        escaping_include
            .units
            .get_mut(root)
            .and_then(|unit| unit.include_dirs.first_mut())
            .ok_or("fixture graph lost root include directory")?
            .path = "../src".to_string();
        assert!(matches!(
            escaping_include.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));
        Ok(())
    }

    #[test]
    fn selected_graph_enforces_portable_and_sensitive_environment_vocabulary() -> TestResult {
        let mut out_dir_text = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&out_dir_text, "fixture")?;
        out_dir_text.units[root].environment.insert(
            "OUT_DIR".to_string(),
            OvenSelectedRustFacetEnvironmentValue::Text {
                value: "/tmp/out".to_string(),
            },
        );
        assert!(matches!(
            out_dir_text.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut absolute_text = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&absolute_text, "fixture")?;
        absolute_text.units[root].environment.insert(
            "FIXTURE_VALUE".to_string(),
            OvenSelectedRustFacetEnvironmentValue::Text {
                value: "/tmp/private".to_string(),
            },
        );
        assert!(matches!(
            absolute_text.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut sensitive_text = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&sensitive_text, "fixture")?;
        sensitive_text.units[root].environment.insert(
            "API_TOKEN".to_string(),
            OvenSelectedRustFacetEnvironmentValue::Text {
                value: "do-not-serialize".to_string(),
            },
        );
        assert!(matches!(
            sensitive_text.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut admitted = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&admitted, "fixture")?;
        let project_owner = admitted.units[root].source.owner.clone();
        admitted.units[root].environment.insert(
            "OUT_DIR".to_string(),
            OvenSelectedRustFacetEnvironmentValue::Path {
                value: OvenSelectedRustFacetPath {
                    owner: project_owner,
                    path: "generated/out".to_string(),
                },
            },
        );
        admitted.units[root].environment.insert(
            "API_TOKEN".to_string(),
            OvenSelectedRustFacetEnvironmentValue::SensitiveDigest {
                hmac_sha256: format!("hmac-sha256:{}", "a".repeat(64)),
            },
        );
        reidentify_unit(&mut admitted, root)?;
        let encoded = String::from_utf8(admitted.validated()?.to_json_bytes()?)?;
        assert!(!encoded.contains("do-not-serialize"));
        assert!(encoded.contains("hmac-sha256:"));
        Ok(())
    }

    #[test]
    fn selected_graph_digest_changes_after_semantic_exposure_mutation() -> TestResult {
        let first = graph(b"pub fn use_dependency() {}\n")?.validated()?;
        let mut changed = graph(b"pub fn use_dependency() {}\n")?;
        let root = changed
            .exposed_roots
            .get("fixture")
            .cloned()
            .ok_or("fixture graph lost exposed root")?;
        changed.exposed_roots.insert("fixture_alias".to_string(), root);
        let changed = changed.validated()?;
        assert_ne!(first.digest(), changed.digest());
        Ok(())
    }

    #[test]
    fn selected_graph_refuses_missing_units_owners_digests_and_root_references() -> TestResult {
        let baseline = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&baseline, "fixture")?;

        let mut missing_unit = baseline.clone();
        missing_unit
            .units
            .get_mut(root)
            .and_then(|unit| unit.dependencies.first_mut())
            .ok_or("fixture graph lost root dependency")?
            .unit = selected_graph_sha256(b"absent unit");
        assert!(matches!(
            missing_unit.validated(),
            Err(OvenSelectedRustFacetGraphError::Missing { .. })
        ));

        let mut missing_owner = baseline.clone();
        missing_owner.units[root].source.owner = selected_graph_sha256(b"absent owner");
        assert!(matches!(
            missing_owner.validated(),
            Err(OvenSelectedRustFacetGraphError::Missing { .. })
        ));

        let mut missing_digest = baseline.clone();
        missing_digest.units[root].source.digest.clear();
        assert!(matches!(
            missing_digest.validated(),
            Err(OvenSelectedRustFacetGraphError::Missing { .. })
        ));

        let mut missing_root = baseline;
        missing_root
            .exposed_roots
            .insert("fixture".to_string(), selected_graph_sha256(b"absent root"));
        assert!(matches!(
            missing_root.validated(),
            Err(OvenSelectedRustFacetGraphError::Missing { .. })
        ));
        Ok(())
    }

    fn graph_from_relocated_source(root: &Path) -> Result<OvenSelectedRustFacetGraph, Box<dyn std::error::Error>> {
        let bytes = fs::read(root.join("src/lib.rs"))?;
        Ok(graph(&bytes)?)
    }

    #[test]
    fn selected_graph_digest_is_stable_across_physical_relocation() -> TestResult {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        for root in [first.path(), second.path()] {
            fs::create_dir(root.join("src"))?;
            fs::write(root.join("src/lib.rs"), b"pub fn use_dependency() {}\n")?;
        }

        let first_selected = graph_from_relocated_source(first.path())?.validated()?;
        let second_selected = graph_from_relocated_source(second.path())?.validated()?;
        assert_eq!(first_selected.digest(), second_selected.digest());
        assert_eq!(first_selected.to_json_bytes()?, second_selected.to_json_bytes()?);
        let encoded = String::from_utf8(first_selected.to_json_bytes()?)?;
        assert!(!encoded.contains(&first.path().to_string_lossy().to_string()));
        assert!(!encoded.contains(&second.path().to_string_lossy().to_string()));
        Ok(())
    }

    #[test]
    fn selected_graph_retains_registered_public_compiler_facts() -> TestResult {
        let facts = [
            ("CARGO_CFG_TARGET_ARCH", "x86_64"),
            ("CARGO_CFG_TARGET_FEATURE", "cmpxchg16b,fxsr,sse,sse2,sse3,sse4.1"),
            ("CARGO_CFG_UNIX", "1"),
            ("CARGO_CRATE_NAME", "fixture"),
            ("CARGO_FEATURE_ROOT_FEATURE", "1"),
            ("CARGO_PKG_NAME", "fixture"),
            ("CARGO_PKG_VERSION", "0.1.0"),
            // A package with no prerelease selects this fact as empty; that is checked evidence, not a missing value.
            ("CARGO_PKG_VERSION_PRE", ""),
            ("DEBUG", "true"),
            ("HOST", "aarch64-apple-darwin"),
            ("NUM_JOBS", "8"),
            ("OPT_LEVEL", "0"),
            ("PROFILE", "dev"),
            ("TARGET", "x86_64-unknown-linux-gnu"),
        ];
        let environment = facts
            .iter()
            .map(|(name, value)| (*name, environment_text(value)))
            .collect::<Vec<_>>();
        let selected = graph_with_root_environment(&environment)?.validated()?;

        let encoded = selected.to_json_bytes()?;
        let rendered = String::from_utf8(encoded.clone())?;
        for (name, value) in facts {
            assert!(
                rendered.contains(name),
                "public fact `{name}` was dropped from the payload"
            );
            assert!(rendered.contains(value), "public fact `{name}` lost its retained value");
        }
        assert_eq!(
            OvenSelectedRustFacetGraph::decode_validated(&encoded)?.digest(),
            selected.digest()
        );
        Ok(())
    }

    #[test]
    fn selected_graph_environment_text_fails_closed_outside_the_public_registry() -> TestResult {
        let credential_uri = "postgres://user:password@host/db";
        let refused = [
            // Secrecy is a property of the value, and no name-shaped denylist predicts either of these.
            ("DATABASE_URL", environment_text(credential_uri)),
            ("PRIVATE_MATERIAL", environment_text("abc123")),
            // Machine-local locations, under both unregistered and registered names.
            ("FIXTURE_LOCATION", environment_text("/Users/alice/project")),
            ("PROFILE", environment_text("/Users/alice/project")),
            ("TARGET", environment_text("C:\\Users\\alice\\project")),
            ("HOST", environment_text("~/project")),
            // A feature flag carries exactly one meaning; anything else is not that fact.
            ("CARGO_FEATURE_ROOT_FEATURE", environment_text("0")),
            // Nothing outside the registry may claim a retaining representation, path included.
            (
                "DATABASE_URL",
                OvenSelectedRustFacetEnvironmentValue::Path {
                    value: OvenSelectedRustFacetPath {
                        owner: project_owner_identity(b"pub fn use_dependency() {}\n"),
                        path: "config".to_string(),
                    },
                },
            ),
            // A registered public fact is not a place to hide a keyed blob either.
            ("PROFILE", environment_sensitive_digest(b"dev")),
        ];
        for (name, value) in refused {
            let graph = graph_with_root_environment(&[(name, value)])?;
            let field = refusal_field(graph, &format!("environment `{name}`"))?;
            assert!(
                field.ends_with(&format!("environment.{name}")),
                "environment `{name}` was refused at the unrelated field `{field}`"
            );
        }

        // A refusal reaches logs, so it must never carry the material it refused.
        let leaked = graph_with_root_environment(&[("DATABASE_URL", environment_text(credential_uri))])?;
        let rendered = match leaked.validated() {
            Ok(_) => return Err("a credential-bearing URI was admitted as public text".into()),
            Err(error) => error.to_string(),
        };
        assert!(
            !rendered.contains("password"),
            "a refusal echoed the credential it refused"
        );
        assert!(!rendered.contains(credential_uri));
        Ok(())
    }

    #[test]
    fn selected_graph_binds_sensitive_environment_to_identity_without_retaining_it() -> TestResult {
        let first = graph_with_root_environment(&[("PRIVATE_MATERIAL", environment_sensitive_digest(b"abc123"))])?
            .validated()?;
        let second = graph_with_root_environment(&[("PRIVATE_MATERIAL", environment_sensitive_digest(b"xyz789"))])?
            .validated()?;
        let repeated = graph_with_root_environment(&[("PRIVATE_MATERIAL", environment_sensitive_digest(b"abc123"))])?
            .validated()?;

        // The value affects compilation identity, so the graph must not collapse two different values into one.
        assert_ne!(first.digest(), second.digest());
        assert_eq!(first.digest(), repeated.digest());

        let rendered = String::from_utf8(first.to_json_bytes()?)?;
        assert!(rendered.contains("PRIVATE_MATERIAL"));
        assert!(rendered.contains("hmac-sha256:"));
        assert!(
            !rendered.contains("abc123"),
            "sensitive material reached the graph payload"
        );
        Ok(())
    }

    #[test]
    fn selected_graph_rejects_host_specific_source_identity() -> TestResult {
        // Every spelling below must be refused on every validating host: a Windows drive or UNC identity looks
        // like an ordinary relative name to this macOS/Linux test runner, so the rule cannot lean on `std::path`.
        let refused = [
            "/Users/alice/project",
            "C:\\Users\\alice\\project",
            "D:/work/foo",
            "\\\\server\\share\\foo",
            "~/project",
            "path:/Users/alice/project",
            "path:C:\\Users\\alice\\project",
            "path:D:/work/foo",
            "path:\\\\server\\share\\foo",
            "path:~/project",
            "path:../escape",
            "path:./foo",
            "path:.",
            "path:foo/../bar",
            "path:",
            // A `file:` URI is an absolute machine-local path wearing URI clothing; its coordinate opens with a
            // scheme letter, so every separator-shaped check above walks straight past it.
            "path:file:///tmp/project",
            "path:file:///Users/alice/project",
            "path:file:/tmp/project",
            "path:file://localhost/tmp/project",
            "path:file:///C:/Users/alice/project",
            "path:FILE:///tmp/project",
            "path:jar:file:/tmp/project",
            "path:git+file:///tmp/project",
            "file:///tmp/project",
            // Percent-encoding decodes to the same absolute path; the leading position must not be one escape away.
            "path:%2Ftmp/project",
            "path:%2FUsers%2Falice%2Fproject",
            "path:%5CUsers%5Calice",
            "path:c%3A/Users/alice",
            // A portable identity still has to say which vocabulary it is written in.
            "registry:fixture@0.1.0",
            "foo",
        ];
        for identity in refused {
            let graph = path_source_graph(identity, b"pub fn fixture() {}\n")?;
            let field = refusal_field(graph, &format!("source identity `{identity}`"))?;
            assert!(
                field.ends_with(".source.identity"),
                "source identity `{identity}` was refused at the unrelated field `{field}`"
            );
        }

        for identity in ["path:fixture", "path:workspaces/fixture"] {
            path_source_graph(identity, b"pub fn fixture() {}\n")?.validated()?;
        }

        // A Git coordinate is URL-shaped by nature, so it needs the same rule proved separately: `file:` is a local
        // path however it is dressed, while a remote scheme resolves the same way on every machine.
        for identity in [
            "git:file:///tmp/project",
            "git:file:/home/alice/repo",
            "git:FILE:///tmp/project",
            "git:git+file:///tmp/project",
        ] {
            let graph = kind_source_graph(OvenSelectedRustFacetSourceKind::Git, identity, b"pub fn fixture() {}\n")?;
            let field = refusal_field(graph, &format!("source identity `{identity}`"))?;
            assert!(
                field.ends_with(".source.identity"),
                "source identity `{identity}` was refused at the unrelated field `{field}`"
            );
        }
        for identity in [
            "git:https://github.com/encero-systems/incan#0a8395835",
            "git:ssh://git@github.com/encero-systems/incan#0a8395835",
            // Encoding deeper in a remote coordinate is ordinary URL syntax, not a local location.
            "git:https://gitlab.example.com/group%2Fsubgroup/repo#0a8395835",
        ] {
            kind_source_graph(OvenSelectedRustFacetSourceKind::Git, identity, b"pub fn fixture() {}\n")?.validated()?;
        }
        Ok(())
    }

    /// Retarget a one-unit graph at a Windows host and target, re-deriving the identities that depend on them.
    fn windows_selection_graph(
        environment: &[(&str, OvenSelectedRustFacetEnvironmentValue)],
    ) -> Result<OvenSelectedRustFacetGraph, Box<dyn std::error::Error>> {
        let mut graph = path_source_graph("path:fixture", b"pub fn fixture() {}\n")?;
        graph.selection.host = "x86_64-pc-windows-msvc".to_string();
        graph.selection.intent.target = "x86_64-pc-windows-msvc".to_string();
        graph.selection.target_spec.source.path = "target-specs/x86_64-pc-windows-msvc.json".to_string();
        for (name, value) in environment {
            graph.units[0].environment.insert((*name).to_string(), value.clone());
        }
        reidentify_unit(&mut graph, 0)?;
        Ok(graph)
    }

    #[test]
    fn selected_graph_admits_a_windows_hosted_and_windows_targeted_selection() -> TestResult {
        // Windows *facts* are first-class compiler inputs; only Windows *locations* are refused. A developer on
        // Windows must be able to describe their build, or the portability rule would just be an exclusion.
        let facts = [
            ("CARGO_CFG_TARGET_ENV", "msvc"),
            ("CARGO_CFG_TARGET_FAMILY", "windows"),
            ("CARGO_CFG_TARGET_OS", "windows"),
            ("CARGO_CFG_WINDOWS", "1"),
            ("HOST", "x86_64-pc-windows-msvc"),
            ("TARGET", "x86_64-pc-windows-msvc"),
        ];
        let mut environment = facts
            .iter()
            .map(|(name, value)| (*name, environment_text(value)))
            .collect::<Vec<_>>();
        // The one genuinely machine-local fact a Windows build has still travels as an owner-relative path.
        environment.push((
            "OUT_DIR",
            OvenSelectedRustFacetEnvironmentValue::Path {
                value: OvenSelectedRustFacetPath {
                    owner: project_owner_identity(b"pub fn fixture() {}\n"),
                    path: "generated/out".to_string(),
                },
            },
        ));

        let selected = windows_selection_graph(&environment)?.validated()?;
        let rendered = String::from_utf8(selected.to_json_bytes()?)?;
        for (name, value) in facts {
            assert!(rendered.contains(name));
            assert!(
                rendered.contains(value),
                "Windows fact `{name}` lost its retained value"
            );
        }

        // No Windows separator reaches the payload: a literal backslash encodes as `\\` in JSON, whereas the
        // escaped quotes in the fixture's cfg encode as `\"` and must not be mistaken for one.
        assert!(
            !rendered.contains("\\\\"),
            "a Windows path separator reached the portable graph"
        );

        // The host is a real compiler input, so the Windows graph must not collapse into the POSIX one.
        let posix = path_source_graph("path:fixture", b"pub fn fixture() {}\n")?.validated()?;
        assert_ne!(selected.digest(), posix.digest());
        Ok(())
    }

    fn path_source_graph_from_root(root: &Path) -> Result<OvenSelectedRustFacetGraph, Box<dyn std::error::Error>> {
        let bytes = fs::read(root.join("src/lib.rs"))?;
        Ok(path_source_graph("path:fixture", &bytes)?)
    }

    #[test]
    fn selected_graph_path_source_relocates_without_changing_identity_or_digest() -> TestResult {
        // Alice's and Bob's checkouts of the same source differ only in where they physically live.
        let alice = tempfile::tempdir()?;
        let bob = tempfile::tempdir()?;
        assert_ne!(alice.path(), bob.path());
        for root in [alice.path(), bob.path()] {
            fs::create_dir(root.join("src"))?;
            fs::write(root.join("src/lib.rs"), b"pub fn relocatable() {}\n")?;
        }

        let from_alice = path_source_graph_from_root(alice.path())?.validated()?;
        let from_bob = path_source_graph_from_root(bob.path())?.validated()?;

        assert_eq!(from_alice.digest(), from_bob.digest());
        assert_eq!(from_alice.to_json_bytes()?, from_bob.to_json_bytes()?);

        let rendered = String::from_utf8(from_alice.to_json_bytes()?)?;
        for root in [alice.path(), bob.path()] {
            assert!(
                !rendered.contains(root.to_string_lossy().as_ref()),
                "a physical checkout root reached the portable graph"
            );
        }
        assert!(rendered.contains("path:fixture"));

        // The digest must still answer to the logical identity, or equality above would be worth nothing.
        let renamed = path_source_graph("path:renamed", b"pub fn relocatable() {}\n")?.validated()?;
        assert_ne!(from_alice.digest(), renamed.digest());
        Ok(())
    }
}
/// One sealed build-script output directory, laid out as `generated-out-dirs/build/<crate>-<hash>/out` so the
/// generated-code route recognizes it like a Cargo target directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenProjectInspectionGeneratedOutDir {
    /// Cargo package whose build script produced the directory.
    pub crate_name: String,
    /// Directory below the authority's artifact root holding the sealed `*.rs` output.
    pub relative_root: String,
    /// Exact package version whose build script wrote the directory, when the bake knew it. A closure can hold
    /// several build units of one package, and a consumer reads only the unit built from the version it inspects.
    #[serde(default)]
    pub version: Option<String>,
}
