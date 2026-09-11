//! Store ownership for Rust source and target facts consumed by semantic inspection.
//!
//! Direct compilation and semantic inspection deliberately retain different closures. The native compiler owner
//! contains only bytes that can affect a direct `rustc` invocation. This sibling owner contains `rust-src`, exact
//! target-spec output, and exact cfg output, bound back to that compiler without making inspection-only files part of
//! JEC equivalence.

#![allow(
    dead_code,
    reason = "selected inspection producer wiring follows this owner contract"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
#[cfg(feature = "rust_inspect")]
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use semver::Version;
use serde::{Deserialize, Serialize};

#[cfg(feature = "rust_inspect")]
use super::{
    OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION, OvenSelectedRustFacetCrateKind, OvenSelectedRustFacetDependency,
    OvenSelectedRustFacetDomain, OvenSelectedRustFacetEnvironmentValue, OvenSelectedRustFacetGraph,
    OvenSelectedRustFacetIntent, OvenSelectedRustFacetOwner, OvenSelectedRustFacetOwnerKind, OvenSelectedRustFacetPath,
    OvenSelectedRustFacetPurpose, OvenSelectedRustFacetSelection, OvenSelectedRustFacetSource,
    OvenSelectedRustFacetSourceKind, OvenSelectedRustFacetSourceMember, OvenSelectedRustFacetTargetSpec,
    OvenSelectedRustFacetUnit, OvenSelectedRustFacetUnitRole, ValidatedOvenSelectedRustFacetGraph,
    selected_graph_source_digest, selected_graph_unit_identity,
};
use super::{
    OvenDirectRustcCompilerEvidence, OvenOwnedDirectRustcCompiler, OvenRustcError, clear_inherited_cargo_environment,
    digest_bytes, digest_regular_file, rustc_host_target, rustc_identity, rustc_sysroot, verified_regular_file,
};
use crate::oven::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreExecutionPayload,
};
use crate::oven::{OvenBuildIntent, OvenReceipt};
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect::{
    InspectionSourceInput, RustMetadataCache, RustMetadataError, RustWorkspace, SelectedInspectionInputs,
    SelectedInspectionSysrootInput, ValidatedInspectionProject,
};

pub(crate) const OVEN_RUST_INSPECTION_TOOLCHAIN_SCHEMA_VERSION: u32 = 1;
const OVEN_RUST_INSPECTION_TOOLCHAIN_DOMAIN_PREFIX: &str = "rust-inspection-toolchain";
const OVEN_RUST_INSPECTION_TOOLCHAIN_DIGEST_DOMAIN: &str = "incan.oven.rust-inspection-toolchain/1";
const OVEN_RUST_INSPECTION_SOURCE_DIGEST_DOMAIN: &str = "incan.oven.rust-inspection-source/1";
const TARGET_SPEC_RELATIVE_PATH: &str = "target-spec.json";
const CFG_RELATIVE_PATH: &str = "rustc-cfg.txt";
const RUST_SRC_RELATIVE_ROOT: &str = "rust-src/library";
const MAX_RUST_SRC_MEMBERS: usize = 16_384;
const MAX_RUST_SRC_BYTES: u64 = 512 * 1024 * 1024;
static SCRATCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const SYSROOT_CRATE_PATHS: &[(&str, &str)] = &[
    ("alloc", "alloc/src/lib.rs"),
    ("backtrace", "backtrace/src/lib.rs"),
    ("core", "core/src/lib.rs"),
    ("panic_abort", "panic_abort/src/lib.rs"),
    ("panic_unwind", "panic_unwind/src/lib.rs"),
    ("proc_macro", "proc_macro/src/lib.rs"),
    ("profiler_builtins", "profiler_builtins/src/lib.rs"),
    ("std", "std/src/lib.rs"),
    ("test", "test/src/lib.rs"),
    ("unwind", "unwind/src/lib.rs"),
];
const STD_DETECT_CRATE_PATHS: &[&str] = &["stdarch/crates/std_detect/src/lib.rs", "std_detect/src/lib.rs"];
const STD_DEPENDENCIES: &[&str] = &[
    "alloc",
    "panic_unwind",
    "panic_abort",
    "core",
    "profiler_builtins",
    "unwind",
    "std_detect",
    "test",
];

/// Exact compiler identity to which one inspection closure belongs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRustInspectionCompilerBinding {
    pub(crate) binary_digest: String,
    pub(crate) closure_digest: String,
}

/// One small generated fact file stored beneath the immutable inspection owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRustInspectionToolchainFile {
    pub(crate) path: String,
    pub(crate) digest: String,
}

/// One exact regular file below the compiler's selected `rust-src` library root.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRustInspectionSourceMember {
    pub(crate) path: String,
    pub(crate) digest: String,
}

/// Portable source-tree evidence owned by one inspection toolchain closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRustInspectionSourceTree {
    pub(crate) root: String,
    pub(crate) digest: String,
    pub(crate) members: Vec<OvenRustInspectionSourceMember>,
}

/// One explicit sysroot unit in the Cargo-free v1 Rust source projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRustInspectionSysrootUnit {
    pub(crate) name: String,
    pub(crate) root_module: String,
    pub(crate) edition: String,
    pub(crate) dependencies: Vec<String>,
}

/// Immutable inspection-only facts for one exact compiler, host, and target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRustInspectionToolchainPayload {
    pub(crate) schema_version: u32,
    /// Location-independent identity of this inspection closure. Store coordinates and project receipts are absent.
    pub(crate) closure_digest: String,
    pub(crate) compiler: OvenRustInspectionCompilerBinding,
    pub(crate) host: String,
    pub(crate) target: String,
    pub(crate) toolchain: String,
    pub(crate) toolchain_version: String,
    pub(crate) target_spec: OvenRustInspectionToolchainFile,
    pub(crate) cfg_output: OvenRustInspectionToolchainFile,
    /// Sorted exact cfg facts parsed from `cfg_output` without consulting the ambient compiler again.
    pub(crate) cfg: Vec<String>,
    pub(crate) rust_src: OvenRustInspectionSourceTree,
    /// Sorted fixed-layout sysroot units and edges available to the selected-graph producer without Cargo.
    pub(crate) sysroot_units: Vec<OvenRustInspectionSysrootUnit>,
}

#[derive(Serialize)]
struct OvenRustInspectionToolchainDigestInput<'a> {
    domain: &'static str,
    compiler: &'a OvenRustInspectionCompilerBinding,
    host: &'a str,
    target: &'a str,
    toolchain: &'a str,
    toolchain_version: &'a str,
    target_spec: &'a OvenRustInspectionToolchainFile,
    cfg_output: &'a OvenRustInspectionToolchainFile,
    cfg: &'a [String],
    rust_src: &'a OvenRustInspectionSourceTree,
    sysroot_units: &'a [OvenRustInspectionSysrootUnit],
}

/// Lease-protected physical inspection closure selected from the bounded Store.
pub(crate) struct OvenOwnedRustInspectionToolchain {
    owner: OvenStoreExecutionPayload,
    payload: OvenRustInspectionToolchainPayload,
    target_spec: PathBuf,
    cfg_output: PathBuf,
    rust_src: PathBuf,
}

impl OvenOwnedRustInspectionToolchain {
    /// Return the location-independent content identity used by a portable selected graph.
    pub(crate) fn identity(&self) -> &str {
        &self.payload.closure_digest
    }

    /// Return the receipt-bearing Store coordinate that physically retains this closure.
    pub(crate) fn store_identity(&self) -> &str {
        &self.owner.manifest.identity
    }

    pub(crate) fn payload(&self) -> &OvenRustInspectionToolchainPayload {
        &self.payload
    }

    pub(crate) fn target_spec_path(&self) -> &Path {
        &self.target_spec
    }

    pub(crate) fn cfg_output_path(&self) -> &Path {
        &self.cfg_output
    }

    pub(crate) fn rust_src_root(&self) -> &Path {
        &self.rust_src
    }
}

/// Cargo-free `std` selection paired with its physical rust-analyzer projection and retained Store owner.
///
/// This is the fixed bootstrap graph for the installed Incan Engine. It is deliberately narrower than the general
/// RFC 119 producer: only the target-domain compiler units already recorded in the inspection owner are representable.
/// Keeping the owner in this value makes it impossible to retain the projection while dropping its Store lease.
#[cfg(feature = "rust_inspect")]
pub(crate) struct OvenBoundFixedStdInspection {
    owner: Arc<OvenOwnedRustInspectionToolchain>,
    graph: ValidatedOvenSelectedRustFacetGraph,
    projection: ValidatedInspectionProject,
}

/// A loaded fixed-`std` workspace paired with the Store owner that keeps every physical input leased.
#[cfg(feature = "rust_inspect")]
pub(crate) struct OvenLeasedFixedStdWorkspace {
    _inspection: Arc<OvenBoundFixedStdInspection>,
    workspace: RustWorkspace,
}

#[cfg(feature = "rust_inspect")]
impl OvenLeasedFixedStdWorkspace {
    pub(crate) fn workspace(&self) -> &RustWorkspace {
        &self.workspace
    }
}

#[cfg(feature = "rust_inspect")]
impl OvenBoundFixedStdInspection {
    pub(crate) fn graph(&self) -> &ValidatedOvenSelectedRustFacetGraph {
        &self.graph
    }

    /// Load a workspace while retaining this selection and its Store lease for the database lifetime.
    pub(crate) fn load(
        self: &Arc<Self>,
        temporary_root: &Path,
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<OvenLeasedFixedStdWorkspace, RustMetadataError> {
        let workspace = RustWorkspace::load_selected(&self.projection, temporary_root, progress)?;
        Ok(OvenLeasedFixedStdWorkspace {
            _inspection: Arc::clone(self),
            workspace,
        })
    }

    /// Bind this selection to a metadata cache that retains its Store lease until the context is replaced or dropped.
    pub(crate) fn bind_cache(
        self: &Arc<Self>,
        cache: &RustMetadataCache,
        context: &Path,
        temporary_root: &Path,
    ) -> Result<(), RustMetadataError> {
        cache.bind_selected_project_with_owner(context, self.projection.clone(), temporary_root, Arc::clone(self))
    }

    pub(crate) fn owner(&self) -> &Arc<OvenOwnedRustInspectionToolchain> {
        &self.owner
    }
}

/// Project the admitted toolchain's fixed `std` closure without Cargo, rustc, or filesystem graph discovery.
#[cfg(feature = "rust_inspect")]
pub(crate) fn bind_fixed_std_inspection(
    owner: Arc<OvenOwnedRustInspectionToolchain>,
    intent: &OvenBuildIntent,
) -> Result<Arc<OvenBoundFixedStdInspection>, OvenRustcError> {
    let graph = selected_fixed_std_graph(&owner, intent)?;
    let projection = bind_toolchain_only_graph(&graph, &owner)?;
    Ok(Arc::new(OvenBoundFixedStdInspection {
        owner,
        graph,
        projection,
    }))
}

#[cfg(feature = "rust_inspect")]
fn selected_fixed_std_graph(
    owner: &OvenOwnedRustInspectionToolchain,
    intent: &OvenBuildIntent,
) -> Result<ValidatedOvenSelectedRustFacetGraph, OvenRustcError> {
    let payload = owner.payload();
    if payload.target != intent.target {
        return Err(OvenRustcError::IntentMismatch);
    }
    if payload.toolchain != intent.toolchain {
        return Err(OvenRustcError::ToolchainMismatch {
            expected: intent.toolchain.clone(),
            actual: payload.toolchain.clone(),
        });
    }

    let selection = OvenSelectedRustFacetSelection {
        intent: OvenSelectedRustFacetIntent::from(intent),
        host: payload.host.clone(),
        purpose: OvenSelectedRustFacetPurpose::Normal,
        default_features: false,
        toolchain_version: payload.toolchain_version.clone(),
        target_spec: OvenSelectedRustFacetTargetSpec {
            source: OvenSelectedRustFacetPath {
                owner: owner.identity().to_string(),
                path: payload.target_spec.path.clone(),
            },
            digest: payload.target_spec.digest.clone(),
        },
    };
    let definitions = payload
        .sysroot_units
        .iter()
        .map(|unit| (unit.name.as_str(), unit))
        .collect::<BTreeMap<_, _>>();
    let mut reachable = BTreeSet::new();
    let mut pending = vec!["std"];
    while let Some(name) = pending.pop() {
        if !reachable.insert(name) {
            continue;
        }
        let definition = definitions
            .get(name)
            .copied()
            .ok_or_else(|| OvenRustcError::InvalidInput {
                field: "Rust inspection sysroot units",
                message: format!("fixed `std` graph references absent compiler unit `{name}`"),
            })?;
        pending.extend(definition.dependencies.iter().map(String::as_str));
    }

    let source_members = payload
        .rust_src
        .members
        .iter()
        .map(|member| OvenSelectedRustFacetSourceMember {
            path: member.path.clone(),
            digest: member.digest.clone(),
        })
        .collect::<Vec<_>>();
    let source_digest = selected_graph_source_digest(&source_members).map_err(selected_graph_error)?;
    let mut units = BTreeMap::<&str, OvenSelectedRustFacetUnit>::new();
    while units.len() < reachable.len() {
        let mut progressed = false;
        for name in &reachable {
            if units.contains_key(name) {
                continue;
            }
            let definition = definitions
                .get(name)
                .copied()
                .ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "Rust inspection sysroot units",
                    message: format!("fixed graph lost compiler unit `{name}`"),
                })?;
            if definition
                .dependencies
                .iter()
                .any(|dependency| !units.contains_key(dependency.as_str()))
            {
                continue;
            }
            let dependencies = definition
                .dependencies
                .iter()
                .map(|dependency| {
                    let unit = units
                        .get(dependency.as_str())
                        .ok_or_else(|| OvenRustcError::InvalidInput {
                            field: "Rust inspection sysroot units",
                            message: format!("compiler unit `{name}` lost dependency `{dependency}`"),
                        })?;
                    Ok(OvenSelectedRustFacetDependency {
                        alias: dependency.clone(),
                        unit: unit.identity.clone(),
                    })
                })
                .collect::<Result<Vec<_>, OvenRustcError>>()?;
            let mut unit = OvenSelectedRustFacetUnit {
                identity: String::new(),
                package: "rust-sysroot".to_string(),
                package_version: payload.toolchain_version.clone(),
                crate_name: definition.name.clone(),
                crate_kind: OvenSelectedRustFacetCrateKind::Rlib,
                role: OvenSelectedRustFacetUnitRole::CompilerSupport,
                domain: OvenSelectedRustFacetDomain::Target,
                edition: definition.edition.clone(),
                source: OvenSelectedRustFacetSource {
                    kind: OvenSelectedRustFacetSourceKind::Compiler,
                    identity: payload.rust_src.digest.clone(),
                    owner: owner.identity().to_string(),
                    root: payload.rust_src.root.clone(),
                    digest: source_digest.clone(),
                },
                root_module: definition.root_module.clone(),
                source_members: source_members.clone(),
                features: Vec::new(),
                default_features: false,
                cfg: payload.cfg.clone(),
                environment: BTreeMap::new(),
                include_dirs: vec![OvenSelectedRustFacetPath {
                    owner: owner.identity().to_string(),
                    path: payload.rust_src.root.clone(),
                }],
                exclude_dirs: Vec::new(),
                dependencies,
                generated_inputs: Vec::new(),
            };
            unit.identity = selected_graph_unit_identity(&selection, &unit).map_err(selected_graph_error)?;
            units.insert(name, unit);
            progressed = true;
        }
        if !progressed {
            return Err(OvenRustcError::InvalidInput {
                field: "Rust inspection sysroot units",
                message: "fixed `std` dependency closure is cyclic".to_string(),
            });
        }
    }
    let std_identity =
        units
            .get("std")
            .map(|unit| unit.identity.clone())
            .ok_or_else(|| OvenRustcError::InvalidInput {
                field: "Rust inspection sysroot units",
                message: "fixed graph has no `std` unit".to_string(),
            })?;
    OvenSelectedRustFacetGraph {
        schema_version: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
        selection,
        owners: vec![OvenSelectedRustFacetOwner {
            identity: owner.identity().to_string(),
            kind: OvenSelectedRustFacetOwnerKind::Toolchain,
        }],
        units: units.into_values().collect(),
        exposed_roots: BTreeMap::from([("std".to_string(), std_identity)]),
    }
    .validated()
    .map_err(selected_graph_error)
}

#[cfg(feature = "rust_inspect")]
fn bind_toolchain_only_graph(
    selected: &ValidatedOvenSelectedRustFacetGraph,
    owner: &OvenOwnedRustInspectionToolchain,
) -> Result<ValidatedInspectionProject, OvenRustcError> {
    let graph = selected.graph();
    let payload = owner.payload();
    if graph.owners.as_slice()
        != [OvenSelectedRustFacetOwner {
            identity: owner.identity().to_string(),
            kind: OvenSelectedRustFacetOwnerKind::Toolchain,
        }]
        || graph.selection.purpose != OvenSelectedRustFacetPurpose::Normal
        || graph.selection.intent.target != payload.target
        || graph.selection.intent.toolchain != payload.toolchain
        || graph.selection.host != payload.host
        || graph.selection.toolchain_version != payload.toolchain_version
        || graph.selection.target_spec.source.owner != owner.identity()
        || graph.selection.target_spec.source.path != payload.target_spec.path
        || graph.selection.target_spec.digest != payload.target_spec.digest
        || graph.exposed_roots.len() != 1
        || !graph.exposed_roots.contains_key("std")
    {
        return Err(OvenRustcError::InvalidInput {
            field: "fixed Rust inspection graph",
            message: "does not match its retained toolchain owner and `std` bootstrap envelope".to_string(),
        });
    }

    // Rebuild the only graph this owner can truthfully supply. Structural equality covers every unit fact and the
    // exact exposed `std` identity, so a separately valid graph cannot smuggle in a different root, cfg, feature,
    // environment, edition, dependency edge, or compiler-source coordinate.
    let expected = selected_fixed_std_graph(
        owner,
        &OvenBuildIntent {
            target: graph.selection.intent.target.clone(),
            toolchain: graph.selection.intent.toolchain.clone(),
            profile: graph.selection.intent.profile.clone(),
            features: graph.selection.intent.features.clone(),
        },
    )?;
    if graph != expected.graph() {
        return Err(OvenRustcError::InvalidInput {
            field: "fixed Rust inspection graph",
            message: "does not exactly reproduce the reachable sysroot catalog retained by its toolchain owner"
                .to_string(),
        });
    }

    let source_members = payload
        .rust_src
        .members
        .iter()
        .map(|member| OvenSelectedRustFacetSourceMember {
            path: member.path.clone(),
            digest: member.digest.clone(),
        })
        .collect::<Vec<_>>();
    let source_digest = selected_graph_source_digest(&source_members).map_err(selected_graph_error)?;
    let rust_src_root = owner.rust_src_root().to_path_buf();
    let unit_indexes = graph
        .units
        .iter()
        .enumerate()
        .map(|(index, unit)| (unit.identity.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut sysroot_crates = Vec::with_capacity(graph.units.len());
    let mut roots = BTreeMap::new();
    for unit in &graph.units {
        if unit.role != OvenSelectedRustFacetUnitRole::CompilerSupport
            || unit.domain != OvenSelectedRustFacetDomain::Target
            || unit.crate_kind != OvenSelectedRustFacetCrateKind::Rlib
            || unit.source.kind != OvenSelectedRustFacetSourceKind::Compiler
            || unit.source.owner != owner.identity()
            || unit.source.root != payload.rust_src.root
            || unit.source.digest != source_digest
            || unit.source_members != source_members
            || unit.include_dirs
                != [OvenSelectedRustFacetPath {
                    owner: owner.identity().to_string(),
                    path: payload.rust_src.root.clone(),
                }]
            || !unit.exclude_dirs.is_empty()
            || !unit.generated_inputs.is_empty()
        {
            return Err(OvenRustcError::InvalidInput {
                field: "fixed Rust inspection graph",
                message: format!(
                    "unit `{}` escapes the toolchain-only bootstrap envelope",
                    unit.crate_name
                ),
            });
        }
        let environment = unit
            .environment
            .iter()
            .map(|(name, value)| match value {
                OvenSelectedRustFacetEnvironmentValue::Text { value } => Ok((name.clone(), value.clone())),
                OvenSelectedRustFacetEnvironmentValue::Path { value }
                    if value.owner == owner.identity() && value.path == payload.rust_src.root =>
                {
                    Ok((name.clone(), rust_src_root.to_string_lossy().into_owned()))
                }
                OvenSelectedRustFacetEnvironmentValue::Path { .. }
                | OvenSelectedRustFacetEnvironmentValue::SensitiveDigest { .. } => Err(OvenRustcError::InvalidInput {
                    field: "fixed Rust inspection environment",
                    message: format!("unit `{}` contains an unsupported environment binding", unit.crate_name),
                }),
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let dependencies = unit
            .dependencies
            .iter()
            .map(|dependency| {
                let index = unit_indexes.get(dependency.unit.as_str()).copied().ok_or_else(|| {
                    OvenRustcError::InvalidInput {
                        field: "fixed Rust inspection dependencies",
                        message: format!("unit `{}` references an absent dependency", unit.crate_name),
                    }
                })?;
                Ok(serde_json::json!({"crate": index, "name": dependency.alias}))
            })
            .collect::<Result<Vec<_>, OvenRustcError>>()?;
        let root_module = rust_src_root.join(&unit.root_module);
        roots.insert(unit.identity.as_str(), root_module.clone());
        sysroot_crates.push(serde_json::json!({
            "display_name": unit.crate_name,
            "root_module": root_module,
            "edition": unit.edition,
            "version": unit.package_version,
            "deps": dependencies,
            "cfg": unit.cfg,
            "env": environment,
            "is_workspace_member": false,
            "source": {"include_dirs": [rust_src_root.clone()], "exclude_dirs": []},
            "is_proc_macro": false,
        }));
    }
    let query_roots = graph
        .exposed_roots
        .iter()
        .map(|(alias, identity)| {
            roots
                .get(identity.as_str())
                .cloned()
                .map(|root| (alias.clone(), root))
                .ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "fixed Rust inspection roots",
                    message: format!("query alias `{alias}` references an absent unit"),
                })
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let project_json =
        serde_json::to_vec(&serde_json::json!({"crates": []})).map_err(|error| OvenRustcError::InvalidInput {
            field: "fixed Rust inspection projection",
            message: error.to_string(),
        })?;
    let sysroot_project_json = serde_json::to_vec(&serde_json::json!({"crates": sysroot_crates})).map_err(|error| {
        OvenRustcError::InvalidInput {
            field: "fixed Rust inspection sysroot projection",
            message: error.to_string(),
        }
    })?;
    let target_spec_json = fs::read(owner.target_spec_path()).map_err(|source| OvenRustcError::Io {
        path: owner.target_spec_path().to_path_buf(),
        source,
    })?;
    ValidatedInspectionProject::validate(SelectedInspectionInputs {
        project_digest: digest_bytes(&project_json),
        project_json,
        sysroot: Some(SelectedInspectionSysrootInput {
            project_digest: digest_bytes(&sysroot_project_json),
            project_json: sysroot_project_json,
            source_root: rust_src_root.clone(),
        }),
        sources: vec![InspectionSourceInput {
            root: rust_src_root,
            digest: source_digest,
        }],
        target_spec_digest: graph.selection.target_spec.digest.clone(),
        target_spec_json,
        toolchain_version: graph.selection.toolchain_version.clone(),
        query_roots,
    })
    .map_err(|error| OvenRustcError::InvalidInput {
        field: "fixed Rust inspection projection",
        message: error.to_string(),
    })
}

#[cfg(feature = "rust_inspect")]
fn selected_graph_error(error: impl std::fmt::Display) -> OvenRustcError {
    OvenRustcError::InvalidInput {
        field: "selected Rust facet graph",
        message: error.to_string(),
    }
}

struct InspectionToolchainObservation {
    payload: OvenRustInspectionToolchainPayload,
    materialized_files: Vec<OvenArtifactMaterializedFile>,
    _scratch: InspectionScratch,
}

type CollectedRustSource = (Vec<OvenRustInspectionSourceMember>, Vec<(String, PathBuf)>);

struct InspectionScratch {
    root: PathBuf,
}

impl Drop for InspectionScratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Select or publish the immutable inspection facts associated with an already admitted compiler owner.
///
/// A unique compatible Store owner is authoritative after complete materialized-file verification. The ambient
/// compiler and `rust-src` tree are read only for the cold or ambiguous path. `scratch_root` is caller-owned output
/// space used only while turning compiler stdout into immutable Store files; it is removed before return.
pub(crate) fn retain_rust_inspection_toolchain(
    store: &OvenStore,
    receipt: &OvenReceipt,
    compiler: &OvenOwnedDirectRustcCompiler,
    ambient_rustc: &Path,
    scratch_root: &Path,
) -> Result<OvenOwnedRustInspectionToolchain, OvenRustcError> {
    validate_request(receipt, compiler)?;
    let expected = compiler.evidence();
    let mut selected = store.select_payloads_matching_for_execution(|manifest| {
        manifest.kind == OvenArtifactKind::RustInspectionToolchain
            && manifest.intent.target == receipt.intent.target
            && manifest.intent.toolchain == receipt.intent.toolchain
    })?;
    selected.sort_by(|left, right| left.manifest.identity.cmp(&right.manifest.identity));

    let compatible = selected
        .iter()
        .enumerate()
        .filter_map(|(index, owner)| {
            matching_payload(owner, receipt, expected).map(|payload| (index, payload.closure_digest))
        })
        .collect::<Vec<_>>();
    let closure_digests = compatible
        .iter()
        .map(|(_, digest)| digest.as_str())
        .collect::<BTreeSet<_>>();
    if closure_digests.len() == 1
        && let Some((index, _)) = compatible.first()
    {
        let owner = selected.remove(*index);
        if let Some(owner) = admit_owner(owner, receipt, expected, None)? {
            return Ok(owner);
        }
    }

    let observation = observe_toolchain(compiler, ambient_rustc, &receipt.intent.target, scratch_root)?;
    if let Some(index) = selected
        .iter()
        .position(|owner| matching_payload(owner, receipt, expected).as_ref() == Some(&observation.payload))
    {
        let owner = selected.remove(index);
        if let Some(owner) = admit_owner(owner, receipt, expected, Some(&observation.payload))? {
            return Ok(owner);
        }
    }

    let payload = serde_json::to_vec(&observation.payload).map_err(|error| OvenRustcError::InvalidInput {
        field: "Rust inspection toolchain",
        message: format!("cannot encode inspection closure metadata: {error}"),
    })?;
    let manifest = store.publish(&OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: inspection_toolchain_domain(expected.closure_digest())?,
        kind: OvenArtifactKind::RustInspectionToolchain,
        payload,
        materialized_files: observation.materialized_files,
    })?;
    let mut selected = store.select_payloads_for_execution(std::slice::from_ref(&manifest.identity))?;
    let owner = selected.pop().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "Rust inspection toolchain",
        message: "published closure has no retained execution owner".to_string(),
    })?;
    admit_owner(owner, receipt, expected, Some(&observation.payload))?.ok_or_else(|| OvenRustcError::InvalidInput {
        field: "Rust inspection toolchain",
        message: "published closure failed final admission".to_string(),
    })
}

fn validate_request(receipt: &OvenReceipt, compiler: &OvenOwnedDirectRustcCompiler) -> Result<(), OvenRustcError> {
    receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "Rust inspection toolchain receipt",
            message: error.to_string(),
        })?;
    if receipt.intent.target != compiler.evidence().target() {
        return Err(OvenRustcError::IntentMismatch);
    }
    if receipt.intent.toolchain != compiler.toolchain() {
        return Err(OvenRustcError::ToolchainMismatch {
            expected: receipt.intent.toolchain.clone(),
            actual: compiler.toolchain().to_string(),
        });
    }
    Ok(())
}

fn matching_payload(
    owner: &OvenStoreExecutionPayload,
    receipt: &OvenReceipt,
    compiler: &OvenDirectRustcCompilerEvidence,
) -> Option<OvenRustInspectionToolchainPayload> {
    if owner.manifest.kind != OvenArtifactKind::RustInspectionToolchain
        || owner.manifest.intent.target != receipt.intent.target
        || owner.manifest.intent.toolchain != receipt.intent.toolchain
        || owner.manifest.domain != inspection_toolchain_domain(compiler.closure_digest()).ok()?
    {
        return None;
    }
    let payload = serde_json::from_slice::<OvenRustInspectionToolchainPayload>(&owner.payload).ok()?;
    if payload.schema_version != OVEN_RUST_INSPECTION_TOOLCHAIN_SCHEMA_VERSION
        || payload.compiler.binary_digest != compiler.binary_digest()
        || payload.compiler.closure_digest != compiler.closure_digest()
        || payload.host != compiler.host()
        || payload.target != compiler.target()
        || payload.toolchain != receipt.intent.toolchain
        || inspection_toolchain_payload_digest(&payload).ok().as_deref() != Some(payload.closure_digest.as_str())
    {
        return None;
    }
    Some(payload)
}

fn admit_owner(
    owner: OvenStoreExecutionPayload,
    receipt: &OvenReceipt,
    compiler: &OvenDirectRustcCompilerEvidence,
    expected: Option<&OvenRustInspectionToolchainPayload>,
) -> Result<Option<OvenOwnedRustInspectionToolchain>, OvenRustcError> {
    let Some(payload) = matching_payload(&owner, receipt, compiler) else {
        return Ok(None);
    };
    if expected.is_some_and(|expected| expected != &payload) {
        return Ok(None);
    }
    validate_payload_shape(&payload)?;
    owner.verify_materialized_files()?;

    let manifest_files = owner
        .manifest
        .materialized_files
        .iter()
        .map(|file| (file.relative_path.clone(), file.digest.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut expected_files = BTreeMap::new();
    expected_files.insert(payload.target_spec.path.clone(), payload.target_spec.digest.clone());
    expected_files.insert(payload.cfg_output.path.clone(), payload.cfg_output.digest.clone());
    for member in &payload.rust_src.members {
        let path = format!("{}/{}", payload.rust_src.root, member.path);
        if expected_files.insert(path, member.digest.clone()).is_some() {
            return Ok(None);
        }
    }
    if manifest_files.len() != expected_files.len()
        || expected_files
            .iter()
            .any(|(path, digest)| manifest_files.get(path) != Some(digest))
    {
        return Ok(None);
    }

    let target_spec = owner.artifact_root.join(&payload.target_spec.path);
    let target_spec_bytes = fs::read(&target_spec).map_err(|source| OvenRustcError::Io {
        path: target_spec.clone(),
        source,
    })?;
    validate_target_spec(&target_spec_bytes)?;
    let cfg_output = owner.artifact_root.join(&payload.cfg_output.path);
    let cfg_bytes = fs::read(&cfg_output).map_err(|source| OvenRustcError::Io {
        path: cfg_output.clone(),
        source,
    })?;
    if parse_cfg(&cfg_bytes)? != payload.cfg {
        return Ok(None);
    }
    let rust_src = owner.artifact_root.join(&payload.rust_src.root);
    if !rust_src.is_dir() {
        return Ok(None);
    }
    Ok(Some(OvenOwnedRustInspectionToolchain {
        owner,
        payload,
        target_spec,
        cfg_output,
        rust_src,
    }))
}

fn observe_toolchain(
    compiler: &OvenOwnedDirectRustcCompiler,
    ambient_rustc: &Path,
    target: &str,
    scratch_root: &Path,
) -> Result<InspectionToolchainObservation, OvenRustcError> {
    let ambient_rustc =
        fs::canonicalize(verified_regular_file(ambient_rustc, "Rust inspection compiler")?).map_err(|source| {
            OvenRustcError::Io {
                path: ambient_rustc.to_path_buf(),
                source,
            }
        })?;
    let ambient_identity = rustc_identity(&ambient_rustc)?;
    if ambient_identity != compiler.toolchain() {
        return Err(OvenRustcError::ToolchainMismatch {
            expected: compiler.toolchain().to_string(),
            actual: ambient_identity,
        });
    }
    if rustc_host_target(&ambient_rustc)? != compiler.evidence().host() {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection compiler",
            message: "ambient compiler host differs from the admitted compiler owner".to_string(),
        });
    }
    if digest_regular_file(&ambient_rustc, "Rust inspection compiler")? != compiler.evidence().binary_digest() {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection compiler",
            message: "ambient compiler bytes differ from the admitted compiler owner".to_string(),
        });
    }

    let sysroot = fs::canonicalize(rustc_sysroot(&ambient_rustc)?).map_err(|source| OvenRustcError::Io {
        path: ambient_rustc.clone(),
        source,
    })?;
    let sysroot_rustc_path = sysroot.join(rustc_relative_path());
    let sysroot_rustc =
        fs::canonicalize(verified_regular_file(&sysroot_rustc_path, "sysroot rustc")?).map_err(|source| {
            OvenRustcError::Io {
                path: sysroot_rustc_path.clone(),
                source,
            }
        })?;
    if sysroot_rustc != ambient_rustc {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection compiler",
            message: format!(
                "ambient compiler is not its reported sysroot's own {}",
                rustc_relative_path().display()
            ),
        });
    }

    let rust_src =
        fs::canonicalize(sysroot.join("lib/rustlib/src/rust/library")).map_err(|source| OvenRustcError::Io {
            path: sysroot.join("lib/rustlib/src/rust/library"),
            source,
        })?;
    let (source_members, source_files) = collect_rust_src(&rust_src)?;
    let source_digest = rust_src_digest(&source_members)?;
    // The ambient compiler is consulted only to locate an eligible rust-src tree. Once its binary and host have been
    // matched to the retained compiler closure, all executable facts come from that immutable Store-owned compiler.
    let target_spec_bytes = compiler_output(
        compiler.rustc(),
        &[
            "-Z",
            "unstable-options",
            "--print",
            "target-spec-json",
            "--target",
            target,
        ],
        true,
        "target-spec JSON",
    )?;
    validate_target_spec(&target_spec_bytes)?;
    let cfg_bytes = compiler_output(compiler.rustc(), &["--print", "cfg", "--target", target], false, "cfg")?;
    let cfg = parse_cfg(&cfg_bytes)?;
    let scratch = create_scratch(scratch_root)?;
    let target_spec_source = scratch.root.join(TARGET_SPEC_RELATIVE_PATH);
    let cfg_source = scratch.root.join(CFG_RELATIVE_PATH);
    fs::write(&target_spec_source, &target_spec_bytes).map_err(|source| OvenRustcError::Io {
        path: target_spec_source.clone(),
        source,
    })?;
    fs::write(&cfg_source, &cfg_bytes).map_err(|source| OvenRustcError::Io {
        path: cfg_source.clone(),
        source,
    })?;

    let compiler_binding = OvenRustInspectionCompilerBinding {
        binary_digest: compiler.evidence().binary_digest().to_string(),
        closure_digest: compiler.evidence().closure_digest().to_string(),
    };
    let target_spec = OvenRustInspectionToolchainFile {
        path: TARGET_SPEC_RELATIVE_PATH.to_string(),
        digest: digest_bytes(&target_spec_bytes),
    };
    let cfg_output = OvenRustInspectionToolchainFile {
        path: CFG_RELATIVE_PATH.to_string(),
        digest: digest_bytes(&cfg_bytes),
    };
    let rust_src = OvenRustInspectionSourceTree {
        root: RUST_SRC_RELATIVE_ROOT.to_string(),
        digest: source_digest,
        members: source_members,
    };
    let sysroot_units = selected_sysroot_units(&rust_src.members)?;
    let toolchain_version = rustc_semver(compiler.toolchain())?;
    let mut payload = OvenRustInspectionToolchainPayload {
        schema_version: OVEN_RUST_INSPECTION_TOOLCHAIN_SCHEMA_VERSION,
        closure_digest: String::new(),
        compiler: compiler_binding,
        host: compiler.evidence().host().to_string(),
        target: target.to_string(),
        toolchain: compiler.toolchain().to_string(),
        toolchain_version,
        target_spec,
        cfg_output,
        cfg,
        rust_src,
        sysroot_units,
    };
    payload.closure_digest = inspection_toolchain_payload_digest(&payload)?;

    let mut materialized_files = source_files
        .into_iter()
        .map(|(path, source_path)| OvenArtifactMaterializedFile {
            source_path,
            relative_path: format!("{RUST_SRC_RELATIVE_ROOT}/{path}"),
        })
        .collect::<Vec<_>>();
    materialized_files.push(OvenArtifactMaterializedFile {
        source_path: target_spec_source,
        relative_path: TARGET_SPEC_RELATIVE_PATH.to_string(),
    });
    materialized_files.push(OvenArtifactMaterializedFile {
        source_path: cfg_source,
        relative_path: CFG_RELATIVE_PATH.to_string(),
    });
    Ok(InspectionToolchainObservation {
        payload,
        materialized_files,
        _scratch: scratch,
    })
}

fn rustc_relative_path() -> PathBuf {
    rustc_relative_path_for_suffix(std::env::consts::EXE_SUFFIX)
}

fn rustc_relative_path_for_suffix(executable_suffix: &str) -> PathBuf {
    Path::new("bin").join(format!("rustc{executable_suffix}"))
}

fn compiler_output(
    rustc: &Path,
    arguments: &[&str],
    bootstrap: bool,
    description: &'static str,
) -> Result<Vec<u8>, OvenRustcError> {
    let mut command = Command::new(rustc);
    command.args(arguments);
    clear_inherited_cargo_environment(&mut command);
    if bootstrap {
        command.env("RUSTC_BOOTSTRAP", "1");
    }
    let output = command.output().map_err(|source| OvenRustcError::Io {
        path: rustc.to_path_buf(),
        source,
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let excerpt = stderr.chars().take(4_096).collect::<String>();
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection compiler",
            message: format!("failed to report {description}: {excerpt}"),
        });
    }
    if output.stdout.is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection compiler",
            message: format!("reported empty {description}"),
        });
    }
    Ok(output.stdout)
}

fn validate_target_spec(bytes: &[u8]) -> Result<(), OvenRustcError> {
    let value = serde_json::from_slice::<serde_json::Value>(bytes).map_err(|error| OvenRustcError::InvalidInput {
        field: "Rust inspection target spec",
        message: error.to_string(),
    })?;
    if !value.is_object() {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection target spec",
            message: "must be a JSON object".to_string(),
        });
    }
    Ok(())
}

fn parse_cfg(bytes: &[u8]) -> Result<Vec<String>, OvenRustcError> {
    let output = std::str::from_utf8(bytes).map_err(|error| OvenRustcError::InvalidInput {
        field: "Rust inspection cfg",
        message: format!("is not UTF-8: {error}"),
    })?;
    let mut cfg = Vec::new();
    for line in output.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.trim() != line {
            return Err(OvenRustcError::InvalidInput {
                field: "Rust inspection cfg",
                message: "contains an empty or whitespace-padded fact".to_string(),
            });
        }
        cfg.push(line.to_string());
    }
    cfg.sort();
    if cfg.is_empty() || cfg.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection cfg",
            message: "must contain unique compiler facts".to_string(),
        });
    }
    Ok(cfg)
}

fn rustc_semver(identity: &str) -> Result<String, OvenRustcError> {
    let mut words = identity.split_whitespace();
    if words.next() != Some("rustc") {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection toolchain version",
            message: "compiler identity does not begin with `rustc`".to_string(),
        });
    }
    let version = words.next().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "Rust inspection toolchain version",
        message: "compiler identity has no semantic version".to_string(),
    })?;
    Version::parse(version).map_err(|error| OvenRustcError::InvalidInput {
        field: "Rust inspection toolchain version",
        message: error.to_string(),
    })?;
    Ok(version.to_string())
}

fn create_scratch(root: &Path) -> Result<InspectionScratch, OvenRustcError> {
    fs::create_dir_all(root).map_err(|source| OvenRustcError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let metadata = fs::symlink_metadata(root).map_err(|source| OvenRustcError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection scratch root",
            message: "must be a non-symlink directory".to_string(),
        });
    }
    let sequence = SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let scratch = root.join(format!("inspection-toolchain-{}-{sequence}", std::process::id()));
    fs::create_dir(&scratch).map_err(|source| OvenRustcError::Io {
        path: scratch.clone(),
        source,
    })?;
    Ok(InspectionScratch { root: scratch })
}

fn collect_rust_src(root: &Path) -> Result<CollectedRustSource, OvenRustcError> {
    let metadata = fs::symlink_metadata(root).map_err(|source| OvenRustcError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection rust-src",
            message: "library root must be a non-symlink directory".to_string(),
        });
    }
    let mut files = BTreeMap::new();
    let mut total_bytes = 0_u64;
    collect_rust_src_directory(root, root, &mut files, &mut total_bytes)?;
    if files.is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection rust-src",
            message: "library root contains no regular files".to_string(),
        });
    }
    let members = files
        .iter()
        .map(|(path, (digest, _))| OvenRustInspectionSourceMember {
            path: path.clone(),
            digest: digest.clone(),
        })
        .collect();
    let sources = files
        .into_iter()
        .map(|(path, (_, source_path))| (path, source_path))
        .collect();
    Ok((members, sources))
}

fn collect_rust_src_directory(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, (String, PathBuf)>,
    total_bytes: &mut u64,
) -> Result<(), OvenRustcError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|source| OvenRustcError::Io {
            path: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenRustcError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(OvenRustcError::InvalidInput {
                field: "Rust inspection rust-src",
                message: format!("contains a symlink at {}", path.display()),
            });
        }
        if metadata.is_dir() {
            collect_rust_src_directory(root, &path, files, total_bytes)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(OvenRustcError::InvalidInput {
                field: "Rust inspection rust-src",
                message: format!("contains a non-regular member at {}", path.display()),
            });
        }
        *total_bytes = total_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| OvenRustcError::InvalidInput {
                field: "Rust inspection rust-src",
                message: "source byte count overflowed".to_string(),
            })?;
        if *total_bytes > MAX_RUST_SRC_BYTES || files.len() >= MAX_RUST_SRC_MEMBERS {
            return Err(OvenRustcError::InvalidInput {
                field: "Rust inspection rust-src",
                message: format!(
                    "exceeds the bounded closure of {MAX_RUST_SRC_MEMBERS} files or {MAX_RUST_SRC_BYTES} bytes"
                ),
            });
        }
        let relative = portable_relative_path(root, &path)?;
        let digest = digest_regular_file(&path, "Rust inspection rust-src member")?;
        if files.insert(relative, (digest, path)).is_some() {
            return Err(OvenRustcError::InvalidInput {
                field: "Rust inspection rust-src",
                message: "contains a duplicate portable path".to_string(),
            });
        }
    }
    Ok(())
}

fn portable_relative_path(root: &Path, path: &Path) -> Result<String, OvenRustcError> {
    let relative = path.strip_prefix(root).map_err(|_| OvenRustcError::InvalidInput {
        field: "Rust inspection rust-src",
        message: format!("member {} escapes the library root", path.display()),
    })?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(OvenRustcError::InvalidInput {
                field: "Rust inspection rust-src",
                message: format!("member {} has a non-portable path", path.display()),
            });
        };
        let part = part.to_str().ok_or_else(|| OvenRustcError::InvalidInput {
            field: "Rust inspection rust-src",
            message: format!("member {} has a non-UTF-8 path", path.display()),
        })?;
        parts.push(part);
    }
    if parts.is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection rust-src",
            message: "member path is empty".to_string(),
        });
    }
    Ok(parts.join("/"))
}

fn rust_src_digest(members: &[OvenRustInspectionSourceMember]) -> Result<String, OvenRustcError> {
    let bytes = serde_json::to_vec(&(OVEN_RUST_INSPECTION_SOURCE_DIGEST_DOMAIN, members)).map_err(|error| {
        OvenRustcError::InvalidInput {
            field: "Rust inspection rust-src",
            message: format!("cannot encode source member identities: {error}"),
        }
    })?;
    Ok(digest_bytes(&bytes))
}

fn inspection_toolchain_payload_digest(payload: &OvenRustInspectionToolchainPayload) -> Result<String, OvenRustcError> {
    let input = OvenRustInspectionToolchainDigestInput {
        domain: OVEN_RUST_INSPECTION_TOOLCHAIN_DIGEST_DOMAIN,
        compiler: &payload.compiler,
        host: &payload.host,
        target: &payload.target,
        toolchain: &payload.toolchain,
        toolchain_version: &payload.toolchain_version,
        target_spec: &payload.target_spec,
        cfg_output: &payload.cfg_output,
        cfg: &payload.cfg,
        rust_src: &payload.rust_src,
        sysroot_units: &payload.sysroot_units,
    };
    let bytes = serde_json::to_vec(&input).map_err(|error| OvenRustcError::InvalidInput {
        field: "Rust inspection toolchain",
        message: format!("cannot encode inspection closure identity: {error}"),
    })?;
    Ok(digest_bytes(&bytes))
}

fn inspection_toolchain_domain(compiler_closure_digest: &str) -> Result<String, OvenRustcError> {
    let Some(hex) = compiler_closure_digest.strip_prefix("sha256:") else {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection compiler binding",
            message: "has no SHA-256 identity prefix".to_string(),
        });
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection compiler binding",
            message: "has a malformed SHA-256 identity".to_string(),
        });
    }
    Ok(format!("{OVEN_RUST_INSPECTION_TOOLCHAIN_DOMAIN_PREFIX}.{hex}"))
}

fn validate_payload_shape(payload: &OvenRustInspectionToolchainPayload) -> Result<(), OvenRustcError> {
    if payload.schema_version != OVEN_RUST_INSPECTION_TOOLCHAIN_SCHEMA_VERSION {
        return Err(OvenRustcError::UnsupportedRustInspectionToolchainSchema {
            found: payload.schema_version,
            expected: OVEN_RUST_INSPECTION_TOOLCHAIN_SCHEMA_VERSION,
        });
    }
    if payload.target_spec.path != TARGET_SPEC_RELATIVE_PATH
        || payload.cfg_output.path != CFG_RELATIVE_PATH
        || payload.rust_src.root != RUST_SRC_RELATIVE_ROOT
    {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection toolchain",
            message: "uses unsupported v1 owner-relative paths".to_string(),
        });
    }
    if rustc_semver(&payload.toolchain)? != payload.toolchain_version {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection toolchain version",
            message: "does not match the exact compiler identity".to_string(),
        });
    }
    if payload.cfg.is_empty() || payload.cfg.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection cfg",
            message: "must be sorted, unique, and non-empty".to_string(),
        });
    }
    if payload.rust_src.members.is_empty()
        || payload
            .rust_src
            .members
            .windows(2)
            .any(|pair| pair[0].path >= pair[1].path)
        || rust_src_digest(&payload.rust_src.members)? != payload.rust_src.digest
        || inspection_toolchain_payload_digest(payload)? != payload.closure_digest
    {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection toolchain",
            message: "contains noncanonical or mismatched closure evidence".to_string(),
        });
    }
    if selected_sysroot_units(&payload.rust_src.members)? != payload.sysroot_units {
        return Err(OvenRustcError::InvalidInput {
            field: "Rust inspection sysroot units",
            message: "do not match the supported Cargo-free v1 source layout".to_string(),
        });
    }
    Ok(())
}

fn selected_sysroot_units(
    members: &[OvenRustInspectionSourceMember],
) -> Result<Vec<OvenRustInspectionSysrootUnit>, OvenRustcError> {
    let member_paths = members
        .iter()
        .map(|member| member.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut available = SYSROOT_CRATE_PATHS
        .iter()
        .filter(|(_, root)| member_paths.contains(*root))
        .map(|(name, root)| (*name, *root))
        .collect::<BTreeMap<_, _>>();
    let std_detect_roots = STD_DETECT_CRATE_PATHS
        .iter()
        .copied()
        .filter(|root| member_paths.contains(*root))
        .collect::<Vec<_>>();
    match std_detect_roots.as_slice() {
        [] => {}
        [root] => {
            available.insert("std_detect", root);
        }
        roots => {
            return Err(OvenRustcError::InvalidInput {
                field: "Rust inspection sysroot units",
                message: format!("rust-src contains ambiguous `std_detect` roots: {}", roots.join(", ")),
            });
        }
    }
    for required in ["core", "alloc", "std"] {
        if !available.contains_key(required) {
            return Err(OvenRustcError::InvalidInput {
                field: "Rust inspection sysroot units",
                message: format!("required `{required}` root is absent from rust-src"),
            });
        }
    }
    let available_names = available.keys().copied().collect::<BTreeSet<_>>();
    let mut units = Vec::with_capacity(available.len());
    for (name, root_module) in available {
        let declared = match name {
            "alloc" => &["core"][..],
            "std" => STD_DEPENDENCIES,
            "proc_macro" => &["std", "core"][..],
            _ => &[][..],
        };
        let mut dependencies = declared
            .iter()
            .filter(|dependency| available_names.contains(**dependency))
            .map(|dependency| (*dependency).to_string())
            .collect::<Vec<_>>();
        dependencies.sort();
        units.push(OvenRustInspectionSysrootUnit {
            name: name.to_string(),
            root_module: root_module.to_string(),
            edition: "2024".to_string(),
            dependencies,
        });
    }
    Ok(units)
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::oven::store::OvenStoreLimits;
    use crate::oven::{OvenGeneratedProjectRequest, receipt_generated_project};

    fn write_fake_toolchain(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let sysroot = root.join("toolchain");
        let rustc = sysroot.join("bin/rustc");
        fs::create_dir_all(sysroot.join("bin"))?;
        fs::create_dir_all(sysroot.join("lib/rustlib/test-target/lib"))?;
        fs::create_dir_all(sysroot.join("lib/rustlib/src/rust/library/std/src"))?;
        fs::create_dir_all(sysroot.join("lib/rustlib/src/rust/library/core/src"))?;
        fs::create_dir_all(sysroot.join("lib/rustlib/src/rust/library/alloc/src"))?;
        fs::write(
            &rustc,
            r#"#!/bin/sh
if [ "$1" = "--print" ] && [ "$2" = "sysroot" ]; then
  cd "$(dirname "$0")/.." || exit 1
  pwd -P
elif [ "$1" = "--version" ]; then
  printf '%s\n' 'rustc 1.99.0-test'
elif [ "$1" = "-vV" ]; then
  printf '%s\n' 'rustc 1.99.0-test' 'host: test-target'
elif [ "$1" = "--print" ] && [ "$2" = "cfg" ]; then
  printf '%s\n' 'target_arch="fixture"' 'unix'
elif [ "$1" = "-Z" ] && [ "$2" = "unstable-options" ]; then
  printf '%s\n' '{"arch":"fixture","data-layout":"e-p:64:64"}'
else
  exit 2
fi
"#,
        )?;
        fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755))?;
        fs::write(sysroot.join("lib/libdriver.dylib"), b"driver bytes")?;
        fs::write(
            sysroot.join("lib/rustlib/test-target/lib/libstd-test.rlib"),
            b"standard-library bytes",
        )?;
        fs::write(
            sysroot.join("lib/rustlib/src/rust/library/std/src/lib.rs"),
            "#![no_std]\npub mod env { pub fn args() {} }\n",
        )?;
        fs::write(
            sysroot.join("lib/rustlib/src/rust/library/core/src/lib.rs"),
            "#![no_std]\n",
        )?;
        fs::write(
            sysroot.join("lib/rustlib/src/rust/library/alloc/src/lib.rs"),
            "#![no_std]\n",
        )?;
        Ok(rustc)
    }

    fn receipt(root: &Path, source: &Path, version: &str) -> Result<OvenReceipt, Box<dyn std::error::Error>> {
        Ok(receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                root,
                "inspection-owner-fixture",
                version,
                "test-target",
                "rustc 1.99.0-test",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", source),
        )?)
    }

    #[cfg(feature = "rust_inspect")]
    fn mutate_std_unit(
        selected: &ValidatedOvenSelectedRustFacetGraph,
        mutate: impl FnOnce(&mut OvenSelectedRustFacetUnit),
    ) -> Result<ValidatedOvenSelectedRustFacetGraph, Box<dyn std::error::Error>> {
        let mut graph = selected.clone().into_graph();
        let std_index = graph
            .units
            .iter()
            .position(|unit| unit.crate_name == "std")
            .ok_or("fixed graph lost std")?;
        mutate(&mut graph.units[std_index]);
        graph.units[std_index].features.sort();
        graph.units[std_index].cfg.sort();
        graph.units[std_index]
            .dependencies
            .sort_by(|left, right| left.alias.cmp(&right.alias));
        let identity = selected_graph_unit_identity(&graph.selection, &graph.units[std_index])?;
        graph.units[std_index].identity = identity.clone();
        graph.exposed_roots.insert("std".to_string(), identity);
        Ok(graph.validated()?)
    }

    fn source_member(path: &str) -> OvenRustInspectionSourceMember {
        OvenRustInspectionSourceMember {
            path: path.to_string(),
            digest: format!("sha256:{}", "a".repeat(64)),
        }
    }

    fn minimal_sysroot_members(std_detect_root: &str) -> Vec<OvenRustInspectionSourceMember> {
        let mut members = ["alloc/src/lib.rs", "core/src/lib.rs", "std/src/lib.rs", std_detect_root]
            .into_iter()
            .map(source_member)
            .collect::<Vec<_>>();
        members.sort();
        members
    }

    #[test]
    fn inspection_toolchain_reuses_verified_store_bytes_across_receipts() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let rustc = write_fake_toolchain(root.path())?;
        let source = root.path().join("generated.rs");
        fs::write(&source, "pub fn generated() {}\n")?;
        let store = OvenStore::new(
            root.path().join("store"),
            OvenStoreLimits::new(32 * 1024 * 1024, 32 * 1024 * 1024, 32 * 1024 * 1024),
        );
        let first_receipt = receipt(root.path(), &source, "0.1.0")?;
        let super::super::OvenDirectRustcCompilerRetention::Retained(first_compiler) =
            super::super::retain_direct_rustc_compiler(&store, &first_receipt, &rustc, "test-target")?
        else {
            return Err("compiler owner was not retained".into());
        };
        let first = retain_rust_inspection_toolchain(
            &store,
            &first_receipt,
            &first_compiler,
            &rustc,
            &root.path().join("scratch"),
        )?;
        assert_ne!(first.identity(), first.store_identity());
        assert_eq!(
            fs::read_to_string(first.rust_src_root().join("std/src/lib.rs"))?,
            "#![no_std]\npub mod env { pub fn args() {} }\n"
        );
        assert_eq!(
            fs::read_to_string(first.cfg_output_path())?,
            "target_arch=\"fixture\"\nunix\n"
        );
        assert!(first.target_spec_path().starts_with(store.root().canonicalize()?));
        let first_content_identity = first.identity().to_string();
        let first_store_identity = first.store_identity().to_string();

        // Once admitted, the unique immutable Store closure is the authority. A mutable ambient rust-src tree that
        // still belongs to the same compiler generation does not trigger another source walk or replace those bytes.
        fs::write(
            root.path()
                .join("toolchain/lib/rustlib/src/rust/library/std/src/lib.rs"),
            "#![no_std]\npub mod changed {}\n",
        )?;
        let second_receipt = receipt(root.path(), &source, "0.2.0")?;
        let super::super::OvenDirectRustcCompilerRetention::Retained(second_compiler) =
            super::super::retain_direct_rustc_compiler(&store, &second_receipt, &rustc, "test-target")?
        else {
            return Err("warm compiler owner was not retained".into());
        };
        let second = retain_rust_inspection_toolchain(
            &store,
            &second_receipt,
            &second_compiler,
            &rustc,
            &root.path().join("scratch"),
        )?;
        assert_eq!(second.identity(), first_content_identity);
        assert_eq!(second.store_identity(), first_store_identity);
        assert_eq!(
            fs::read_to_string(second.rust_src_root().join("std/src/lib.rs"))?,
            "#![no_std]\npub mod env { pub fn args() {} }\n"
        );
        assert_eq!(store.inspect()?.entries.len(), 2);
        assert!(root.path().join("scratch").read_dir()?.next().is_none());

        #[cfg(feature = "rust_inspect")]
        {
            let owner = Arc::new(second);
            let selected = bind_fixed_std_inspection(Arc::clone(&owner), &second_receipt.intent)?;
            assert_eq!(selected.owner().identity(), first_content_identity);
            assert_eq!(selected.graph().graph().units.len(), 3);
            assert_eq!(
                selected.graph().graph().exposed_roots.keys().collect::<Vec<_>>(),
                vec!["std"]
            );
            let output = tempfile::tempdir()?;
            let selected_weak = Arc::downgrade(&selected);
            let workspace = selected.load(output.path(), &|_| {})?;
            drop(selected);
            assert!(selected_weak.upgrade().is_some());
            crate::rust_inspect::extract_rust_item(workspace.workspace(), "std::env::args")?;
            assert!(output.path().read_dir()?.next().is_none());
            drop(workspace);
            assert!(selected_weak.upgrade().is_none());

            let selected = bind_fixed_std_inspection(Arc::clone(&owner), &second_receipt.intent)?;
            let selected_weak = Arc::downgrade(&selected);
            let cache_context = tempfile::tempdir()?;
            let cache_output = tempfile::tempdir()?;
            let cache = RustMetadataCache::new();
            selected.bind_cache(&cache, cache_context.path(), cache_output.path())?;
            drop(selected);
            assert!(selected_weak.upgrade().is_some());
            cache.invalidate_manifest_dir(cache_context.path())?;
            assert!(selected_weak.upgrade().is_none());

            let canonical = selected_fixed_std_graph(&owner, &second_receipt.intent)?;
            let mut adversarial = Vec::new();
            adversarial.push(mutate_std_unit(&canonical, |unit| {
                unit.cfg.push("zz_invented_cfg".to_string());
            })?);
            adversarial.push(mutate_std_unit(&canonical, |unit| {
                unit.root_module = "core/src/lib.rs".to_string();
            })?);
            adversarial.push(mutate_std_unit(&canonical, |unit| {
                unit.edition = "2021".to_string();
            })?);
            adversarial.push(mutate_std_unit(&canonical, |unit| {
                unit.dependencies
                    .iter_mut()
                    .find(|dependency| dependency.alias == "alloc")
                    .expect("fixed std should depend on alloc")
                    .alias = "renamed_alloc".to_string();
            })?);
            adversarial.push(mutate_std_unit(&canonical, |unit| {
                unit.features.push("invented_feature".to_string());
            })?);
            adversarial.push(mutate_std_unit(&canonical, |unit| {
                unit.environment.insert(
                    "PROFILE".to_string(),
                    OvenSelectedRustFacetEnvironmentValue::Text {
                        value: "invented".to_string(),
                    },
                );
            })?);
            adversarial.push(mutate_std_unit(&canonical, |unit| {
                unit.source.identity = format!("sha256:{}", "b".repeat(64));
            })?);
            adversarial.push(mutate_std_unit(&canonical, |unit| {
                unit.default_features = true;
            })?);
            let mut wrong_exposure = canonical.clone().into_graph();
            let std_identity = wrong_exposure
                .exposed_roots
                .remove("std")
                .ok_or("fixed graph lost std exposure")?;
            wrong_exposure
                .exposed_roots
                .insert("invented_std".to_string(), std_identity);
            adversarial.push(wrong_exposure.validated()?);
            for graph in adversarial {
                assert!(matches!(
                    bind_toolchain_only_graph(&graph, &owner),
                    Err(OvenRustcError::InvalidInput {
                        field: "fixed Rust inspection graph",
                        ..
                    })
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn sysroot_catalog_accepts_each_known_std_detect_layout_and_rejects_ambiguity() {
        for root in STD_DETECT_CRATE_PATHS {
            let units =
                selected_sysroot_units(&minimal_sysroot_members(root)).expect("known layout should be accepted");
            let std_detect = units
                .iter()
                .find(|unit| unit.name == "std_detect")
                .expect("known layout should select std_detect");
            assert_eq!(std_detect.root_module, *root);
            assert!(
                units
                    .iter()
                    .find(|unit| unit.name == "std")
                    .expect("minimal catalog should select std")
                    .dependencies
                    .iter()
                    .any(|dependency| dependency == "std_detect")
            );
        }

        let mut ambiguous = minimal_sysroot_members(STD_DETECT_CRATE_PATHS[0]);
        ambiguous.push(source_member(STD_DETECT_CRATE_PATHS[1]));
        ambiguous.sort();
        assert!(matches!(
            selected_sysroot_units(&ambiguous),
            Err(OvenRustcError::InvalidInput {
                field: "Rust inspection sysroot units",
                ..
            })
        ));
    }

    #[test]
    fn rustc_relative_path_adds_the_selected_platform_suffix() {
        assert_eq!(rustc_relative_path_for_suffix(""), PathBuf::from("bin/rustc"));
        assert_eq!(rustc_relative_path_for_suffix(".exe"), PathBuf::from("bin/rustc.exe"));
        assert_eq!(
            rustc_relative_path(),
            rustc_relative_path_for_suffix(std::env::consts::EXE_SUFFIX)
        );
    }
}
