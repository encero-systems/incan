//! Physical materialization of an already selected Rust facet graph.
//!
//! This module deliberately does not parse manifests, choose packages, or discover a dependency edge. It takes the
//! one validated facet graph produced by the selected source authority and binds each named owner to a caller-held
//! immutable root. Both direct Rustc publication and Rust inspection can use this adapter; neither gets a second
//! graphing path.

#![allow(
    dead_code,
    reason = "the source publisher wires this adapter after its selected runtime provider lands"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

use oven_model::oven_interop::{
    OVEN_INTEROP_EXECUTION_PROVENANCE_SCHEMA_VERSION, OVEN_INTEROP_EXECUTION_RECEIPT_SCHEMA_VERSION,
    OvenInteropExecutionProvenance, verify_interop_execution_receipt_identity,
};

use super::{
    OvenCompiledRustUnitIdentity, OvenRustcError, OvenSelectedRustFacetEnvironmentValue,
    OvenSelectedRustFacetGeneratedInput, OvenSelectedRustFacetGraph, OvenSelectedRustFacetLinkedLibrary,
    OvenSelectedRustFacetLinkedLibraryKind, OvenSelectedRustFacetOwnerKind, OvenSelectedRustFacetPath,
    OvenSelectedRustFacetSourceMember, OvenSelectedRustFacetUnit, ValidatedOvenSelectedRustFacetGraph,
    compiled_rust_unit_identities, digest_regular_file,
};

/// One physical root for an owner named by a validated selected facet graph.
///
/// The caller obtains these roots only from retained Store/Loaf/provider owners. This shape carries no authority to
/// search a checkout, registry cache, Cargo home, or neighbouring artifact directory.
#[derive(Debug, Clone)]
pub struct OvenSelectedRustFacetOwnerRoot {
    pub identity: String,
    pub root: PathBuf,
}

/// Supplemental package-local source members supplied by an already admitted execution record.
///
/// The selected unit's source root is the compiler-visible root. A provider/package root may be the same root or an
/// ancestor below the same retained owner, for example a package root `.` above a compiler root `src`. These members
/// never enter the selected unit's compiler-visible source catalogue or compiled identity. The higher-level record
/// must already bind their owner, root and bytes; this adapter only verifies that exact physical tree and refuses any
/// hidden file.
#[derive(Debug, Clone)]
pub struct OvenSelectedRustFacetSupplementalSourceMembers {
    /// Existing selected unit to which this package-local source evidence is attached.
    pub selected_identity: String,
    /// Existing owner-relative root of the complete supplemental tree.
    pub source_root: OvenSelectedRustFacetPath,
    /// Exact portable members admitted by the higher-level execution record, relative to `source_root`.
    pub members: Vec<OvenSelectedRustFacetSourceMember>,
}

/// Exact additional members grouped by one selected owner/root pair.
type SupplementalSourceTrees = BTreeMap<(String, String), BTreeMap<String, String>>;

/// A compiler-visible environment value rebound from a portable selected graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OvenMaterializedRustFacetEnvironmentValue {
    Text(String),
    Path(PathBuf),
}

/// One ordered linked-library input after its declared owner has been physically admitted.
///
/// Archives carry an exact verified file. Providers carry the target-specific capability, receipt and artifact
/// admitted from their complete owner tree, so the executor never needs to discover a same-named host library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OvenMaterializedRustFacetLinkedLibrary {
    /// Exact static or dynamic archive bytes retained beneath an admitted owner.
    Archive {
        /// Linker-visible declared name retained for evidence and diagnostics.
        name: String,
        /// Declared static or dynamic linkage class.
        kind: OvenSelectedRustFacetLinkedLibraryKind,
        /// Canonical path of the verified regular non-symlink file.
        artifact: PathBuf,
        /// Digest of the exact verified bytes.
        digest: String,
    },
    /// Logical framework or system capability beneath an admitted provider owner.
    Provider {
        /// Linker-visible declared capability name.
        name: String,
        /// Declared framework or system linkage class.
        kind: OvenSelectedRustFacetLinkedLibraryKind,
        /// Exact target verified against the held provider provenance.
        target: String,
        /// Provider-declared capability verified against the held provenance.
        capability: String,
        /// Exact provider receipt identity verified against the held provenance.
        receipt_identity: String,
        /// Canonical directory containing the physically admitted link input.
        search_root: PathBuf,
        /// Canonical path of the exact selected provider artifact.
        artifact: PathBuf,
        /// Digest of the exact selected provider artifact.
        digest: String,
    },
}

/// A physically verified selected source unit ready for the direct-Rustc publisher.
#[derive(Debug, Clone)]
pub struct OvenMaterializedRustFacetUnit {
    /// Source-selection identity used to preserve graph topology and source authority.
    pub selected_identity: String,
    /// Effective compiler-input identity used for immutable output reuse.
    pub compiled_identity: OvenCompiledRustUnitIdentity,
    pub source_root: PathBuf,
    pub root_module: PathBuf,
    pub include_dirs: Vec<PathBuf>,
    pub exclude_dirs: Vec<PathBuf>,
    pub environment: BTreeMap<String, OvenMaterializedRustFacetEnvironmentValue>,
    pub generated_inputs: Vec<(String, PathBuf, String)>,
    /// Compiler-owned bare externs admitted by the validated selected graph.
    pub sysroot_externs: Vec<String>,
    /// Ordered linked-library inputs. Order and repeated entries are compiler-visible and are preserved exactly.
    pub linked_libraries: Vec<OvenMaterializedRustFacetLinkedLibrary>,
}

/// A physically admitted selected graph and its source-unit projection.
#[derive(Debug, Clone)]
pub struct OvenMaterializedRustFacetGraph {
    target: OvenMaterializedRustTarget,
    units: BTreeMap<String, OvenMaterializedRustFacetUnit>,
    supplemental_source_roots: BTreeMap<(String, String), PathBuf>,
}

#[derive(Debug, Clone)]
enum OvenMaterializedRustTarget {
    BuiltIn(String),
    Custom(PathBuf),
}

impl OvenMaterializedRustFacetGraph {
    /// Return the exact custom target-spec JSON after digest verification.
    ///
    /// Built-in targets return `None`: the selected compiler owns those targets directly and no JSON file exists.
    pub fn custom_target_spec(&self) -> Option<&Path> {
        match &self.target {
            OvenMaterializedRustTarget::BuiltIn(_) => None,
            OvenMaterializedRustTarget::Custom(path) => Some(path),
        }
    }

    /// Return the exact `rustc --target` argument admitted by the selected target descriptor.
    pub fn compiler_target(&self) -> &OsStr {
        match &self.target {
            OvenMaterializedRustTarget::BuiltIn(target) => OsStr::new(target),
            OvenMaterializedRustTarget::Custom(path) => path.as_os_str(),
        }
    }

    /// Look up one selected source unit by its source-selection identity.
    pub fn unit(&self, selected_identity: &str) -> Option<&OvenMaterializedRustFacetUnit> {
        self.units.get(selected_identity)
    }

    /// Iterate deterministic source-selection identities and their admitted physical units.
    pub fn units(&self) -> impl Iterator<Item = (&str, &OvenMaterializedRustFacetUnit)> {
        self.units.iter().map(|(identity, unit)| (identity.as_str(), unit))
    }

    /// Return one already verified supplemental package root named by the higher-level admitted record.
    ///
    /// The graph deliberately exposes only roots the materializer already checked against an exact supplemental
    /// inventory. It does not expose an arbitrary owner-relative directory for later source discovery.
    pub fn supplemental_source_root(&self, root: &OvenSelectedRustFacetPath) -> Option<&Path> {
        self.supplemental_source_roots
            .get(&(root.owner.clone(), root.path.clone()))
            .map(PathBuf::as_path)
    }
}

/// Bind a validated selected graph to caller-held physical roots without discovery.
///
/// The source graph remains the authority for all package, dependency, feature, environment and source-member
/// facts. This adapter only verifies that the physical bytes retained by those owners still match those facts and
/// turns portable paths into safe absolute paths. It refuses an owner table mismatch, symlinks, an extra source file
/// or any source digest mismatch before a compiler can read the tree.
pub fn materialize_selected_rust_facet_graph(
    selected: &ValidatedOvenSelectedRustFacetGraph,
    compiler_closure_digest: &str,
    owner_roots: &[OvenSelectedRustFacetOwnerRoot],
) -> Result<OvenMaterializedRustFacetGraph, OvenRustcError> {
    materialize_selected_rust_facet_graph_with_supplemental_source_members(
        selected,
        compiler_closure_digest,
        owner_roots,
        &[],
    )
}

/// Bind a selected graph while retaining a higher-level, already admitted source closure in the exact physical tree.
///
/// Provider-only supplemental members do not enter [`compiled_rust_unit_identities`]; an overlapping member already
/// enters through the selected unit's ordinary source catalogue. Every supplemental member is checked solely to make
/// sure a physical source root contains neither hidden files nor undeclared provider inputs. Callers must provide
/// them from an authority record that has already selected their owner, source root and byte identities.
pub fn materialize_selected_rust_facet_graph_with_supplemental_source_members(
    selected: &ValidatedOvenSelectedRustFacetGraph,
    compiler_closure_digest: &str,
    owner_roots: &[OvenSelectedRustFacetOwnerRoot],
    supplemental_source_members: &[OvenSelectedRustFacetSupplementalSourceMembers],
) -> Result<OvenMaterializedRustFacetGraph, OvenRustcError> {
    let graph = selected.graph();
    let owners = admitted_owner_roots(graph, owner_roots)?;
    let supplemental_source_trees = supplemental_source_members_by_tree(graph, supplemental_source_members)?;
    validate_supplemental_source_roots(graph, &owners, supplemental_source_members)?;
    let supplemental_source_roots = supplemental_source_trees
        .keys()
        .map(|(owner, root)| {
            Ok((
                (owner.clone(), root.clone()),
                resolve_directory(&owners, owner, root, "selected Rust supplemental source root")?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, OvenRustcError>>()?;
    let target = match &graph.selection.target_spec {
        super::OvenSelectedRustFacetTargetSpec::BuiltIn { target, .. } => {
            OvenMaterializedRustTarget::BuiltIn(target.clone())
        }
        super::OvenSelectedRustFacetTargetSpec::Custom { source, digest } => {
            let path = resolve_file(&owners, source, digest, "selected Rust target spec")?;
            validate_custom_target_spec(&path)?;
            OvenMaterializedRustTarget::Custom(path)
        }
    };
    let compiled_identities = compiled_rust_unit_identities(selected, compiler_closure_digest)?;
    let mut verified_source_trees = BTreeSet::new();
    let compiler_source_roots = graph
        .units
        .iter()
        .map(|unit| (unit.source.owner.clone(), unit.source.root.clone()))
        .collect::<BTreeSet<_>>();
    let mut units = BTreeMap::new();
    for unit in &graph.units {
        let source_root = resolve_directory(
            &owners,
            &unit.source.owner,
            &unit.source.root,
            "selected Rust source root",
        )?;
        let tree_key = (
            unit.source.owner.as_str(),
            unit.source.root.as_str(),
            unit.source.digest.as_str(),
        );
        if verified_source_trees.insert(tree_key) {
            let source_tree_key = (unit.source.owner.clone(), unit.source.root.clone());
            let supplemental = supplemental_source_trees.get(&source_tree_key);
            verify_source_tree(&source_root, &unit.source_members, &unit.source.digest, supplemental)?;
        }
        let root_module = resolve_file_relative(
            &source_root,
            &unit.root_module,
            unit.source_members
                .iter()
                .find(|member| member.path == unit.root_module)
                .map(|member| member.digest.as_str())
                .ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "selected Rust source unit",
                    message: format!("`{}` has no root-module member digest", unit.crate_name),
                })?,
            "selected Rust root module",
        )?;
        let include_dirs = unit
            .include_dirs
            .iter()
            .map(|path| resolve_directory_path(&owners, path, "selected Rust include directory"))
            .collect::<Result<Vec<_>, _>>()?;
        let exclude_dirs = unit
            .exclude_dirs
            .iter()
            .map(|path| resolve_directory_path(&owners, path, "selected Rust exclude directory"))
            .collect::<Result<Vec<_>, _>>()?;
        let environment = materialize_environment(&owners, unit)?;
        let generated_inputs = unit
            .generated_inputs
            .iter()
            .map(|input| materialize_generated_input(&owners, input))
            .collect::<Result<Vec<_>, _>>()?;
        let linked_libraries = materialize_linked_libraries(graph, &owners, unit)?;
        let compiled_identity =
            compiled_identities
                .get(&unit.identity)
                .cloned()
                .ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "selected Rust source unit",
                    message: format!("`{}` has no derived compiled identity", unit.crate_name),
                })?;
        if units
            .insert(
                unit.identity.clone(),
                OvenMaterializedRustFacetUnit {
                    selected_identity: unit.identity.clone(),
                    compiled_identity,
                    source_root,
                    root_module,
                    include_dirs,
                    exclude_dirs,
                    environment,
                    generated_inputs,
                    sysroot_externs: unit.sysroot_externs.clone(),
                    linked_libraries,
                },
            )
            .is_some()
        {
            return Err(OvenRustcError::InvalidInput {
                field: "selected Rust source unit",
                message: "validated graph repeated a selected identity".to_string(),
            });
        }
    }
    for ((owner, root), members) in &supplemental_source_trees {
        if compiler_source_roots.contains(&(owner.clone(), root.clone())) {
            continue;
        }
        let source_root = supplemental_source_roots
            .get(&(owner.clone(), root.clone()))
            .ok_or_else(|| OvenRustcError::InvalidInput {
                field: "selected Rust supplemental source root",
                message: "materializer lost a verified supplemental source root".to_string(),
            })?;
        verify_supplemental_source_tree(source_root, members)?;
    }
    Ok(OvenMaterializedRustFacetGraph {
        target,
        units,
        supplemental_source_roots,
    })
}

/// Require a custom target spec to be an actual JSON object before it can reach rustc.
fn validate_custom_target_spec(path: &Path) -> Result<(), OvenRustcError> {
    let bytes = fs::read(path).map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|error| OvenRustcError::InvalidInput {
        field: "selected Rust target spec",
        message: format!("is not valid JSON: {error}"),
    })?;
    if !value.is_object() {
        return Err(OvenRustcError::InvalidInput {
            field: "selected Rust target spec",
            message: "must be a JSON object".to_string(),
        });
    }
    Ok(())
}

/// Validate and group admitted supplemental source members by their existing physical source root.
fn supplemental_source_members_by_tree(
    graph: &OvenSelectedRustFacetGraph,
    supplemental_source_members: &[OvenSelectedRustFacetSupplementalSourceMembers],
) -> Result<SupplementalSourceTrees, OvenRustcError> {
    let units = graph
        .units
        .iter()
        .map(|unit| (unit.identity.as_str(), unit))
        .collect::<BTreeMap<_, _>>();
    let mut seen_units = BTreeSet::new();
    let mut by_tree = BTreeMap::new();
    for supplemental in supplemental_source_members {
        if !seen_units.insert(supplemental.selected_identity.as_str()) {
            return Err(OvenRustcError::InvalidInput {
                field: "selected Rust supplemental source",
                message: format!("repeats selected unit `{}`", supplemental.selected_identity),
            });
        }
        let unit = units
            .get(supplemental.selected_identity.as_str())
            .ok_or_else(|| OvenRustcError::InvalidInput {
                field: "selected Rust supplemental source",
                message: format!("names unknown selected unit `{}`", supplemental.selected_identity),
            })?;
        if supplemental.source_root.owner != unit.source.owner {
            return Err(OvenRustcError::InvalidInput {
                field: "selected Rust supplemental source",
                message: format!(
                    "selected unit `{}` names supplemental root under a different owner",
                    supplemental.selected_identity
                ),
            });
        }
        let _ = super::selected_graph_source_digest(&supplemental.members).map_err(|error| {
            OvenRustcError::InvalidInput {
                field: "selected Rust supplemental source",
                message: error.to_string(),
            }
        })?;
        let members = by_tree
            .entry((
                supplemental.source_root.owner.clone(),
                supplemental.source_root.path.clone(),
            ))
            .or_insert_with(BTreeMap::new);
        for member in &supplemental.members {
            match members.insert(member.path.clone(), member.digest.clone()) {
                Some(existing) if existing != member.digest => {
                    return Err(OvenRustcError::InvalidInput {
                        field: "selected Rust supplemental source",
                        message: format!(
                            "selected source root has conflicting digest declarations for `{}`",
                            member.path
                        ),
                    });
                }
                Some(_) | None => {}
            }
        }
    }
    Ok(by_tree)
}

/// Require a supplemental package root to contain its selected unit's compiler-visible root under the same owner.
///
/// This check happens after owner-relative path resolution, so platform-specific spelling cannot turn a sibling or
/// escaped location into an admitted package tree.
fn validate_supplemental_source_roots(
    graph: &OvenSelectedRustFacetGraph,
    owners: &BTreeMap<String, PathBuf>,
    supplemental_source_members: &[OvenSelectedRustFacetSupplementalSourceMembers],
) -> Result<(), OvenRustcError> {
    let units = graph
        .units
        .iter()
        .map(|unit| (unit.identity.as_str(), unit))
        .collect::<BTreeMap<_, _>>();
    for supplemental in supplemental_source_members {
        let unit = units
            .get(supplemental.selected_identity.as_str())
            .ok_or_else(|| OvenRustcError::InvalidInput {
                field: "selected Rust supplemental source",
                message: format!("names unknown selected unit `{}`", supplemental.selected_identity),
            })?;
        let supplemental_root = resolve_directory_path(
            owners,
            &supplemental.source_root,
            "selected Rust supplemental source root",
        )?;
        let compiler_root = resolve_directory(
            owners,
            &unit.source.owner,
            &unit.source.root,
            "selected Rust source root",
        )?;
        if !compiler_root.starts_with(&supplemental_root) {
            return Err(OvenRustcError::InvalidInput {
                field: "selected Rust supplemental source",
                message: format!(
                    "selected unit `{}` has a supplemental root that does not contain its compiler root",
                    supplemental.selected_identity
                ),
            });
        }
    }
    Ok(())
}

/// Bind each owner the selected graph names to exactly one supplied physical root.
///
/// The supplied set has to match the declared one exactly, both ways: an owner without a root cannot be
/// materialized, and a supplied root for an owner the graph never names is authority nobody asked for.
fn admitted_owner_roots(
    graph: &OvenSelectedRustFacetGraph,
    supplied: &[OvenSelectedRustFacetOwnerRoot],
) -> Result<BTreeMap<String, PathBuf>, OvenRustcError> {
    let expected = graph
        .owners
        .iter()
        .map(|owner| owner.identity.as_str())
        .collect::<BTreeSet<_>>();
    let mut roots = BTreeMap::new();
    for owner in supplied {
        let root = canonical_directory(&owner.root, "selected Rust owner root")?;
        if roots.insert(owner.identity.clone(), root).is_some() {
            return Err(OvenRustcError::InvalidInput {
                field: "selected Rust owner roots",
                message: format!("repeat owner `{}`", owner.identity),
            });
        }
    }
    let actual = roots.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(OvenRustcError::InvalidInput {
            field: "selected Rust owner roots",
            message: "do not exactly match the selected graph owner table".to_string(),
        });
    }
    Ok(roots)
}

/// Resolve one unit's declared compiler environment, turning its path-valued entries into admitted paths.
///
/// Text entries pass through unchanged; a path entry is resolved against the admitted owner roots like any other
/// declared path, so an environment variable cannot name a tree the selection never admitted.
fn materialize_environment(
    owners: &BTreeMap<String, PathBuf>,
    unit: &OvenSelectedRustFacetUnit,
) -> Result<BTreeMap<String, OvenMaterializedRustFacetEnvironmentValue>, OvenRustcError> {
    unit.environment
        .iter()
        .map(|(name, value)| {
            let value = match value {
                OvenSelectedRustFacetEnvironmentValue::Text { value } => {
                    OvenMaterializedRustFacetEnvironmentValue::Text(value.clone())
                }
                OvenSelectedRustFacetEnvironmentValue::Path { value } => {
                    OvenMaterializedRustFacetEnvironmentValue::Path(resolve_path(
                        owners,
                        value,
                        "selected Rust environment path",
                    )?)
                }
                OvenSelectedRustFacetEnvironmentValue::SensitiveDigest { .. } => {
                    return Err(OvenRustcError::InvalidInput {
                        field: "selected Rust environment",
                        message: format!(
                            "unit `{}` has redacted `{name}`; direct compilation requires a separately admitted value provider",
                            unit.crate_name
                        ),
                    });
                }
            };
            Ok((name.clone(), value))
        })
        .collect()
}

/// Admit one build-script output as a compiler input, returning its name, physical path and digest.
///
/// Generated inputs are the one class of source this materialization does not find in a declared source tree, so
/// each is admitted individually against the digest its producing unit's receipt recorded.
fn materialize_generated_input(
    owners: &BTreeMap<String, PathBuf>,
    input: &OvenSelectedRustFacetGeneratedInput,
) -> Result<(String, PathBuf, String), OvenRustcError> {
    let path = resolve_path(owners, &input.source, "selected Rust generated input")?;
    let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
        path: path.clone(),
        source,
    })?;
    if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
        return Err(OvenRustcError::InvalidInput {
            field: "selected Rust generated input",
            message: format!("`{}` is not a regular non-symlink file or directory", input.name),
        });
    }
    let actual = if metadata.is_file() {
        digest_regular_file(&path, "selected Rust generated input")?
    } else {
        let mut members = BTreeMap::new();
        collect_source_tree(&path, &path, &mut members)?;
        let members = members
            .into_iter()
            .map(|(path, digest)| OvenSelectedRustFacetSourceMember { path, digest })
            .collect::<Vec<_>>();
        super::selected_graph_generated_input_digest(&members).map_err(|error| OvenRustcError::InvalidInput {
            field: "selected Rust generated input",
            message: error.to_string(),
        })?
    };
    if actual != input.digest {
        return Err(OvenRustcError::ArtifactDigestMismatch {
            path,
            expected: input.digest.clone(),
            actual,
        });
    }
    Ok((input.name.clone(), path, input.digest.clone()))
}

/// Resolve one unit's ordered linked-library inputs without introducing linker discovery.
///
/// Archive bytes use the same contained owner-relative file admission as every other selected file. A provider is
/// retained only when its identity names an admitted `LinkedLibraryProvider`; its root is evidence of that held
/// owner, not permission to search the root or the host by name.
fn materialize_linked_libraries(
    graph: &OvenSelectedRustFacetGraph,
    owners: &BTreeMap<String, PathBuf>,
    unit: &OvenSelectedRustFacetUnit,
) -> Result<Vec<OvenMaterializedRustFacetLinkedLibrary>, OvenRustcError> {
    unit.linked_libraries
        .iter()
        .map(|library| match library {
            OvenSelectedRustFacetLinkedLibrary::Archive {
                name,
                kind,
                artifact,
                digest,
            } => Ok(OvenMaterializedRustFacetLinkedLibrary::Archive {
                name: name.clone(),
                kind: *kind,
                artifact: resolve_file(owners, artifact, digest, "selected Rust linked archive")?,
                digest: digest.clone(),
            }),
            OvenSelectedRustFacetLinkedLibrary::Provider { details } => {
                let crate::rustc::OvenSelectedRustFacetLinkedLibraryProvider {
                    name,
                    kind,
                    provider,
                    target,
                    capability,
                    receipt_identity,
                    provenance,
                    provenance_digest,
                    search_root,
                    artifact,
                    digest,
                    members,
                } = details.as_ref();
                let declared = graph
                    .owners
                    .iter()
                    .find(|owner| owner.identity == *provider)
                    .ok_or_else(|| OvenRustcError::InvalidInput {
                        field: "selected Rust linked provider",
                        message: format!("names absent owner `{provider}`"),
                    })?;
                if declared.kind != OvenSelectedRustFacetOwnerKind::LinkedLibraryProvider {
                    return Err(OvenRustcError::InvalidInput {
                        field: "selected Rust linked provider",
                        message: format!("owner `{provider}` is not a linked-library provider"),
                    });
                }
                let expected_target = match unit.domain {
                    super::OvenSelectedRustFacetDomain::Host => graph.selection.host.as_str(),
                    super::OvenSelectedRustFacetDomain::Target => graph.selection.intent.target.as_str(),
                };
                if target != expected_target {
                    return Err(OvenRustcError::InvalidInput {
                        field: "selected Rust linked provider",
                        message: format!("target `{target}` does not match unit target `{expected_target}`"),
                    });
                }
                validate_provider_target(*kind, target)?;
                let provenance_path = resolve_file(
                    owners,
                    provenance,
                    provenance_digest,
                    "selected Rust linked provider provenance",
                )?;
                let provenance_bytes = fs::read(&provenance_path).map_err(|source| OvenRustcError::Io {
                    path: provenance_path.clone(),
                    source,
                })?;
                let held: OvenInteropExecutionProvenance =
                    serde_json::from_slice(&provenance_bytes).map_err(|error| OvenRustcError::InvalidInput {
                        field: "selected Rust linked provider provenance",
                        message: error.to_string(),
                    })?;
                verify_interop_execution_receipt_identity(&held.receipt).map_err(|error| {
                    OvenRustcError::InvalidInput {
                        field: "selected Rust linked provider provenance",
                        message: error,
                    }
                })?;
                if held.schema_version != OVEN_INTEROP_EXECUTION_PROVENANCE_SCHEMA_VERSION
                    || held.receipt.schema_version != OVEN_INTEROP_EXECUTION_RECEIPT_SCHEMA_VERSION
                    || held.receipt.target != target.as_str()
                    || held.receipt.identity != receipt_identity.as_str()
                    || !held.system_capabilities.iter().any(|held| held == capability)
                {
                    return Err(OvenRustcError::InvalidInput {
                        field: "selected Rust linked provider provenance",
                        message: "does not bind the selected schema, target, receipt and capability".to_string(),
                    });
                }
                let search_root = resolve_directory_path(owners, search_root, "selected Rust linked provider root")?;
                let member_digest =
                    super::selected_graph_source_digest(members).map_err(|error| OvenRustcError::InvalidInput {
                        field: "selected Rust linked provider members",
                        message: error.to_string(),
                    })?;
                verify_source_tree(&search_root, members, &member_digest, None)?;
                let artifact = resolve_file(owners, artifact, digest, "selected Rust linked provider artifact")?;
                if !artifact.starts_with(&search_root) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "selected Rust linked provider artifact",
                        message: "is outside its exact provider search root".to_string(),
                    });
                }
                if *kind == OvenSelectedRustFacetLinkedLibraryKind::Framework {
                    validate_framework_artifact(&search_root, &artifact, name)?;
                }
                Ok(OvenMaterializedRustFacetLinkedLibrary::Provider {
                    name: name.clone(),
                    kind: *kind,
                    target: target.clone(),
                    capability: capability.clone(),
                    receipt_identity: receipt_identity.clone(),
                    search_root,
                    artifact,
                    digest: digest.clone(),
                })
            }
        })
        .collect()
}

/// Require an admitted framework artifact to be the named binary inside the exact named framework directory.
fn validate_framework_artifact(search_root: &Path, artifact: &Path, name: &str) -> Result<(), OvenRustcError> {
    let expected = search_root.join(format!("{name}.framework")).join(name);
    if artifact != expected {
        return Err(OvenRustcError::InvalidInput {
            field: "selected Rust linked framework",
            message: format!(
                "{} is not the exact `{name}.framework/{name}` artifact",
                artifact.display()
            ),
        });
    }
    Ok(())
}

/// Refuse framework linkage outside an Apple target instead of passing a meaningless framework flag to its linker.
fn validate_provider_target(kind: OvenSelectedRustFacetLinkedLibraryKind, target: &str) -> Result<(), OvenRustcError> {
    if kind == OvenSelectedRustFacetLinkedLibraryKind::Framework && target.split('-').nth(1) != Some("apple") {
        return Err(OvenRustcError::InvalidInput {
            field: "selected Rust linked framework",
            message: format!("framework linkage is unsupported for non-Apple target `{target}`"),
        });
    }
    Ok(())
}

/// Require the physical tree under `root` to be exactly the member set the selection declared.
///
/// Both directions matter and are checked: a declared member that is absent, and a file present on disk that was
/// never declared. Admitting only the first would let an undeclared file reach the compiler, which is how a
/// source closure stops being the thing its digest describes. `supplemental` carries the members a package root
/// contributes beyond the unit's own, so they are admitted rather than reported as strays.
fn verify_source_tree(
    root: &Path,
    members: &[OvenSelectedRustFacetSourceMember],
    expected_digest: &str,
    supplemental: Option<&BTreeMap<String, String>>,
) -> Result<(), OvenRustcError> {
    let mut expected = members
        .iter()
        .map(|member| (member.path.as_str(), member.digest.as_str()))
        .collect::<BTreeMap<_, _>>();
    if let Some(supplemental) = supplemental {
        for (path, digest) in supplemental {
            match expected.get(path.as_str()) {
                Some(existing) if *existing != digest.as_str() => {
                    return Err(OvenRustcError::InvalidInput {
                        field: "selected Rust supplemental source",
                        message: format!("conflicts with compiler-visible selected member `{path}`"),
                    });
                }
                Some(_) => {}
                None => {
                    expected.insert(path.as_str(), digest.as_str());
                }
            }
        }
    }
    let mut actual = BTreeMap::new();
    collect_source_tree(root, root, &mut actual)?;
    if actual.len() != expected.len() {
        return Err(OvenRustcError::InvalidInput {
            field: "selected Rust source tree",
            message: "physical members do not match the complete selected source catalog".to_string(),
        });
    }
    for (path, digest) in &expected {
        let actual_digest = actual.get(*path).ok_or_else(|| OvenRustcError::InvalidInput {
            field: "selected Rust source tree",
            message: format!("selected member `{path}` is absent"),
        })?;
        if actual_digest != digest {
            return Err(OvenRustcError::ArtifactDigestMismatch {
                path: root.join(path),
                expected: (*digest).to_string(),
                actual: actual_digest.clone(),
            });
        }
    }
    let source_digest = super::selected_graph_source_digest(members).map_err(|error| OvenRustcError::InvalidInput {
        field: "selected Rust source tree",
        message: error.to_string(),
    })?;
    if source_digest != expected_digest {
        return Err(OvenRustcError::InvalidInput {
            field: "selected Rust source tree",
            message: "selected source members disagree with their source digest".to_string(),
        });
    }
    Ok(())
}

/// Verify a complete non-compiler package tree retained by an admitted higher-level record.
///
/// Unlike [`verify_source_tree`], this tree has no selected compiler-source digest because it is intentionally not a
/// compiler input. Its digest is derived only to validate the canonical member inventory before exact physical
/// coverage is checked.
fn verify_supplemental_source_tree(root: &Path, members: &BTreeMap<String, String>) -> Result<(), OvenRustcError> {
    let members = members
        .iter()
        .map(|(path, digest)| OvenSelectedRustFacetSourceMember {
            path: path.clone(),
            digest: digest.clone(),
        })
        .collect::<Vec<_>>();
    let digest = super::selected_graph_source_digest(&members).map_err(|error| OvenRustcError::InvalidInput {
        field: "selected Rust supplemental source",
        message: error.to_string(),
    })?;
    verify_source_tree(root, &members, &digest, None)
}

/// Walk one admitted directory into `root`-relative paths and digests, refusing anything not a plain file.
fn collect_source_tree(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, String>,
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
                field: "selected Rust source tree",
                message: format!("contains symlink {}", path.display()),
            });
        }
        if metadata.is_dir() {
            collect_source_tree(root, &path, files)?;
        } else if metadata.is_file() {
            let relative = portable_relative_path(root, &path, "selected Rust source tree")?;
            let digest = digest_regular_file(&path, "selected Rust source member")?;
            if files.insert(relative, digest).is_some() {
                return Err(OvenRustcError::InvalidInput {
                    field: "selected Rust source tree",
                    message: "contains duplicate portable path".to_string(),
                });
            }
        } else {
            return Err(OvenRustcError::InvalidInput {
                field: "selected Rust source tree",
                message: format!("contains non-regular member {}", path.display()),
            });
        }
    }
    Ok(())
}

/// Resolve one owner-relative directory and require it to be a real directory.
fn resolve_directory(
    owners: &BTreeMap<String, PathBuf>,
    owner: &str,
    relative: &str,
    field: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    let path = resolve_relative_owner_path(owners, owner, relative, field)?;
    canonical_directory(&path, field)
}

/// Resolve one declared owner-relative path and require it to be a real directory.
fn resolve_directory_path(
    owners: &BTreeMap<String, PathBuf>,
    path: &OvenSelectedRustFacetPath,
    field: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    let path = resolve_path(owners, path, field)?;
    canonical_directory(&path, field)
}

/// Resolve one declared owner-relative file and admit it only at its expected digest.
fn resolve_file(
    owners: &BTreeMap<String, PathBuf>,
    path: &OvenSelectedRustFacetPath,
    expected_digest: &str,
    field: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    let path = resolve_path(owners, path, field)?;
    verify_file(&path, expected_digest, field)
}

/// Resolve one file below an already-admitted root and admit it only at its expected digest.
fn resolve_file_relative(
    root: &Path,
    relative: &str,
    expected_digest: &str,
    field: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    let path = resolve_relative_path(root, relative, field)?;
    verify_file(&path, expected_digest, field)
}

/// Admit one physical file only when it is a regular non-symlink file whose bytes match what was declared.
///
/// Shape is checked on the link rather than its target, before canonicalization, so a symlink cannot smuggle a
/// file from outside the selected closure past a digest that happens to match. The digest is read from the
/// canonical path so the bytes verified are the bytes a later step opens.
fn verify_file(path: &Path, expected_digest: &str, field: &'static str) -> Result<PathBuf, OvenRustcError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(OvenRustcError::InvalidInput {
            field,
            message: format!("{} is not a regular non-symlink file", path.display()),
        });
    }
    let canonical = fs::canonicalize(path).map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let actual = digest_regular_file(&canonical, field)?;
    if actual != expected_digest {
        return Err(OvenRustcError::ArtifactDigestMismatch {
            path: canonical,
            expected: expected_digest.to_string(),
            actual,
        });
    }
    fs::canonicalize(path).map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Resolve one declared owner-relative path against the roots this materialization admitted.
///
/// The declared form carries an owner name and a relative path rather than a location, which is what lets one
/// selected graph be materialized at different roots. This is the single place that turns the pair back into a
/// physical path, so no caller invents its own join.
fn resolve_path(
    owners: &BTreeMap<String, PathBuf>,
    path: &OvenSelectedRustFacetPath,
    field: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    resolve_relative_owner_path(owners, &path.owner, &path.path, field)
}

/// Look the owner up among the admitted roots before resolving anything under it.
///
/// An unbound owner is a refusal rather than a fallback to some default root: a path whose owner was never
/// admitted names a tree this materialization has no authority over, and silently resolving it elsewhere would
/// admit source the selection never declared.
fn resolve_relative_owner_path(
    owners: &BTreeMap<String, PathBuf>,
    owner: &str,
    relative: &str,
    field: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    let root = owners.get(owner).ok_or_else(|| OvenRustcError::InvalidInput {
        field,
        message: format!("references unbound owner `{owner}`"),
    })?;
    resolve_relative_path(root, relative, field)
}

/// Walk one relative path under an admitted root, refusing any component that could leave it.
///
/// Every step is checked as it is built rather than the result being checked at the end, because a symlink part
/// way along escapes the root while the final resolved path can still look like it sits inside. Absolute paths,
/// `..`, a root, and a drive prefix are each refused for the same reason.
fn resolve_relative_path(root: &Path, relative: &str, field: &'static str) -> Result<PathBuf, OvenRustcError> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute() {
        return Err(OvenRustcError::InvalidInput {
            field,
            message: format!("uses absolute path `{relative}`"),
        });
    }
    let mut resolved = root.to_path_buf();
    for component in relative_path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(component) => {
                resolved.push(component);
                let metadata = fs::symlink_metadata(&resolved).map_err(|source| OvenRustcError::Io {
                    path: resolved.clone(),
                    source,
                })?;
                if metadata.file_type().is_symlink() {
                    return Err(OvenRustcError::InvalidInput {
                        field,
                        message: format!("uses a symlinked owner-relative path `{relative}`"),
                    });
                }
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(OvenRustcError::InvalidInput {
                    field,
                    message: format!("uses non-portable owner-relative path `{relative}`"),
                });
            }
        }
    }
    Ok(resolved)
}

/// Resolve one admitted directory, refusing a symlink before it is followed.
///
/// The symlink check runs on the link itself rather than on its target, and it runs *before* canonicalization,
/// because canonicalizing first would silently accept a link pointing outside the selected source closure and
/// report the resolved path as if it had been admitted. `field` names the caller's input so a refusal says which
/// declared root was wrong rather than only which path it resolved to.
fn canonical_directory(path: &Path, field: &'static str) -> Result<PathBuf, OvenRustcError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(OvenRustcError::InvalidInput {
            field,
            message: format!("{} is not a non-symlink directory", path.display()),
        });
    }
    fs::canonicalize(path).map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Express one admitted path relative to its selected root, in a form another machine reads the same way.
///
/// Everything but ordinary named components is refused rather than normalized: `..`, a root prefix, or a Windows
/// drive letter would each let a recorded path mean something different where it is later materialized, and a
/// non-UTF-8 component cannot survive the wire contract at all. Refusing here keeps a relocation failure at the
/// boundary that admitted the path instead of at whichever consumer first resolves it.
fn portable_relative_path(root: &Path, path: &Path, field: &'static str) -> Result<String, OvenRustcError> {
    let relative = path.strip_prefix(root).map_err(|_| OvenRustcError::InvalidInput {
        field,
        message: format!("{} escapes its selected source root", path.display()),
    })?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(OvenRustcError::InvalidInput {
                field,
                message: format!("{} has non-portable path components", path.display()),
            });
        };
        let part = part.to_str().ok_or_else(|| OvenRustcError::InvalidInput {
            field,
            message: format!("{} has non-UTF-8 path components", path.display()),
        })?;
        parts.push(part);
    }
    if parts.is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field,
            message: "source member has empty relative path".to_string(),
        });
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use super::*;
    use crate::rustc::{
        OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION, OvenSelectedRustFacetCfgSnapshot,
        OvenSelectedRustFacetCrateKind, OvenSelectedRustFacetDomain, OvenSelectedRustFacetGraph,
        OvenSelectedRustFacetOwner, OvenSelectedRustFacetOwnerKind, OvenSelectedRustFacetPurpose,
        OvenSelectedRustFacetSelection, OvenSelectedRustFacetSource, OvenSelectedRustFacetSourceKind,
        OvenSelectedRustFacetTargetSpec, OvenSelectedRustFacetUnitRole, compiled_rust_unit_identities,
        selected_graph_sha256, selected_graph_source_digest, selected_graph_unit_identity,
    };

    const COMPILER_CLOSURE: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn source_owner() -> String {
        selected_graph_sha256(b"selected-unit source owner")
    }

    fn toolchain_owner() -> String {
        selected_graph_sha256(b"selected-unit toolchain owner")
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
            intent: super::super::OvenSelectedRustFacetIntent {
                target: "aarch64-apple-darwin".to_string(),
                toolchain: "rustc 1.85.0 (fixture)".to_string(),
                profile: "debug".to_string(),
            },
            host: "x86_64-unknown-linux-gnu".to_string(),
            host_cfg: cfg_snapshot("x86_64", "linux"),
            target_cfg: cfg_snapshot("x86_64", "linux"),
            purpose: OvenSelectedRustFacetPurpose::Normal,
            toolchain_version: "1.85.0".to_string(),
            target_spec: OvenSelectedRustFacetTargetSpec::Custom {
                source: OvenSelectedRustFacetPath {
                    owner: toolchain_owner(),
                    path: "target-spec.json".to_string(),
                },
                digest: selected_graph_sha256(b"target spec"),
            },
        }
    }

    fn selected_graph() -> Result<ValidatedOvenSelectedRustFacetGraph, Box<dyn std::error::Error>> {
        let selection = selection();
        let members = vec![OvenSelectedRustFacetSourceMember {
            path: "src/lib.rs".to_string(),
            digest: selected_graph_sha256(b"pub fn marker() -> u8 { 7 }\n"),
        }];
        let owner = source_owner();
        let mut unit = OvenSelectedRustFacetUnit {
            sysroot_externs: Vec::new(),
            identity: String::new(),
            package: "fixture".to_string(),
            package_version: "1.0.0".to_string(),
            crate_name: "fixture".to_string(),
            crate_kind: OvenSelectedRustFacetCrateKind::Rlib,
            role: OvenSelectedRustFacetUnitRole::Library,
            domain: OvenSelectedRustFacetDomain::Target,
            edition: "2024".to_string(),
            source: OvenSelectedRustFacetSource {
                kind: OvenSelectedRustFacetSourceKind::Generated,
                identity: "generated:fixture".to_string(),
                owner: owner.clone(),
                root: ".".to_string(),
                digest: selected_graph_source_digest(&members)?,
            },
            root_module: "src/lib.rs".to_string(),
            source_members: members,
            features: Vec::new(),
            cfg: Vec::new(),
            environment: BTreeMap::new(),
            include_dirs: vec![OvenSelectedRustFacetPath {
                owner: owner.clone(),
                path: ".".to_string(),
            }],
            exclude_dirs: Vec::new(),
            dependencies: Vec::new(),
            generated_inputs: Vec::new(),
            linked_libraries: Vec::new(),
        };
        unit.identity = selected_graph_unit_identity(&selection, &unit)?;
        let identity = unit.identity.clone();
        Ok(OvenSelectedRustFacetGraph {
            schema_version: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
            selection,
            owners: vec![
                OvenSelectedRustFacetOwner {
                    identity: owner,
                    kind: OvenSelectedRustFacetOwnerKind::ProjectAuthority,
                },
                OvenSelectedRustFacetOwner {
                    identity: toolchain_owner(),
                    kind: OvenSelectedRustFacetOwnerKind::Toolchain,
                },
            ],
            units: vec![unit],
            exposed_roots: BTreeMap::from([(
                "fixture".to_string(),
                crate::rustc::OvenSelectedRustFacetRoot {
                    unit: identity,
                    requested_features: Vec::new(),
                    default_features: true,
                    intent_owner: toolchain_owner(),
                },
            )]),
        }
        .validated()?)
    }

    /// Re-root the compiler-visible fixture below an ordinary package root without changing its source bytes.
    fn selected_graph_with_compiler_root() -> Result<ValidatedOvenSelectedRustFacetGraph, Box<dyn std::error::Error>> {
        let mut graph = selected_graph()?.graph().clone();
        let selection = graph.selection.clone();
        let unit = graph.units.first_mut().ok_or("fixture has no selected unit")?;
        let members = vec![OvenSelectedRustFacetSourceMember {
            path: "lib.rs".to_string(),
            digest: selected_graph_sha256(b"pub fn marker() -> u8 { 7 }\n"),
        }];
        unit.source.root = "src".to_string();
        unit.source.digest = selected_graph_source_digest(&members)?;
        unit.root_module = "lib.rs".to_string();
        unit.source_members = members;
        unit.include_dirs = vec![OvenSelectedRustFacetPath {
            owner: source_owner(),
            path: "src".to_string(),
        }];
        unit.identity = selected_graph_unit_identity(&selection, unit)?;
        graph.exposed_roots = BTreeMap::from([(
            "fixture".to_string(),
            crate::rustc::OvenSelectedRustFacetRoot {
                unit: unit.identity.clone(),
                requested_features: Vec::new(),
                default_features: true,
                intent_owner: toolchain_owner(),
            },
        )]);
        Ok(graph.validated()?)
    }

    fn roots(root: &Path) -> Result<Vec<OvenSelectedRustFacetOwnerRoot>, Box<dyn std::error::Error>> {
        let source = root.join("source");
        fs::create_dir_all(source.join("src"))?;
        fs::write(source.join("src/lib.rs"), b"pub fn marker() -> u8 { 7 }\n")?;
        let toolchain = root.join("toolchain");
        fs::create_dir_all(&toolchain)?;
        fs::write(toolchain.join("target-spec.json"), b"target spec")?;
        Ok(vec![
            OvenSelectedRustFacetOwnerRoot {
                identity: source_owner(),
                root: source,
            },
            OvenSelectedRustFacetOwnerRoot {
                identity: toolchain_owner(),
                root: toolchain,
            },
        ])
    }

    #[test]
    fn materializes_exact_selected_source_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let selected = selected_graph()?;
        let materialized = materialize_selected_rust_facet_graph(&selected, COMPILER_CLOSURE, &roots(root.path())?)?;
        assert!(
            materialized
                .custom_target_spec()
                .is_some_and(|path| path.ends_with("target-spec.json"))
        );
        let unit = materialized
            .units()
            .next()
            .map(|(_, unit)| unit)
            .ok_or("selected graph materialized no units")?;
        assert!(unit.root_module.ends_with("src/lib.rs"));
        assert_eq!(unit.include_dirs, vec![unit.source_root.clone()]);
        assert!(unit.generated_inputs.is_empty());
        Ok(())
    }

    #[test]
    fn custom_target_spec_refuses_invalid_json_before_compiler_execution() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("custom-target.json");
        fs::write(&path, b"not json")?;
        assert!(matches!(
            validate_custom_target_spec(&path),
            Err(OvenRustcError::InvalidInput {
                field: "selected Rust target spec",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn materializes_linked_archives_in_declared_order_with_repetition() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let archive_owner = selected_graph_sha256(b"linked archive owner");
        let archive_root = root.path().join("linked");
        fs::create_dir_all(&archive_root)?;
        fs::write(archive_root.join("libfixture.a"), b"linked archive")?;
        let digest = selected_graph_sha256(b"linked archive");
        let mut graph = selected_graph()?.graph().clone();
        graph.owners.push(OvenSelectedRustFacetOwner {
            identity: archive_owner.clone(),
            kind: OvenSelectedRustFacetOwnerKind::Constituent,
        });
        let unit = graph.units.first_mut().ok_or("fixture has no selected unit")?;
        let archive = crate::rustc::OvenSelectedRustFacetLinkedLibrary::Archive {
            name: "fixture".to_string(),
            kind: crate::rustc::OvenSelectedRustFacetLinkedLibraryKind::Static,
            artifact: OvenSelectedRustFacetPath {
                owner: archive_owner.clone(),
                path: "libfixture.a".to_string(),
            },
            digest: digest.clone(),
        };
        unit.linked_libraries = vec![archive.clone(), archive];
        let unit = unit.clone();
        let mut owner_roots = roots(root.path())?
            .into_iter()
            .map(|owner| (owner.identity, owner.root))
            .collect::<BTreeMap<_, _>>();
        owner_roots.insert(archive_owner, fs::canonicalize(&archive_root)?);

        let linked = materialize_linked_libraries(&graph, &owner_roots, &unit)?;
        assert_eq!(linked.len(), 2);
        assert_eq!(linked[0], linked[1]);
        assert!(matches!(
            &linked[0],
            OvenMaterializedRustFacetLinkedLibrary::Archive { artifact, digest: actual, .. }
                if artifact.ends_with("libfixture.a") && actual == &digest
        ));
        Ok(())
    }

    #[test]
    fn linked_archive_materialization_refuses_tampered_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let archive_owner = selected_graph_sha256(b"linked archive owner");
        let archive_root = root.path().join("linked");
        fs::create_dir_all(&archive_root)?;
        fs::write(archive_root.join("libfixture.a"), b"tampered")?;
        let mut graph = selected_graph()?.graph().clone();
        graph.owners.push(OvenSelectedRustFacetOwner {
            identity: archive_owner.clone(),
            kind: OvenSelectedRustFacetOwnerKind::Constituent,
        });
        let unit = graph.units.first_mut().ok_or("fixture has no selected unit")?;
        unit.linked_libraries = vec![crate::rustc::OvenSelectedRustFacetLinkedLibrary::Archive {
            name: "fixture".to_string(),
            kind: crate::rustc::OvenSelectedRustFacetLinkedLibraryKind::Static,
            artifact: OvenSelectedRustFacetPath {
                owner: archive_owner.clone(),
                path: "libfixture.a".to_string(),
            },
            digest: selected_graph_sha256(b"expected"),
        }];
        let unit = unit.clone();
        let mut owner_roots = roots(root.path())?
            .into_iter()
            .map(|owner| (owner.identity, owner.root))
            .collect::<BTreeMap<_, _>>();
        owner_roots.insert(archive_owner, fs::canonicalize(&archive_root)?);

        assert!(materialize_linked_libraries(&graph, &owner_roots, &unit).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn linked_archive_materialization_refuses_symlinks() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let archive_owner = selected_graph_sha256(b"linked archive owner");
        let archive_root = root.path().join("linked");
        fs::create_dir_all(&archive_root)?;
        fs::write(archive_root.join("actual.a"), b"linked archive")?;
        symlink("actual.a", archive_root.join("libfixture.a"))?;
        let mut graph = selected_graph()?.graph().clone();
        graph.owners.push(OvenSelectedRustFacetOwner {
            identity: archive_owner.clone(),
            kind: OvenSelectedRustFacetOwnerKind::Constituent,
        });
        let unit = graph.units.first_mut().ok_or("fixture has no selected unit")?;
        unit.linked_libraries = vec![crate::rustc::OvenSelectedRustFacetLinkedLibrary::Archive {
            name: "fixture".to_string(),
            kind: crate::rustc::OvenSelectedRustFacetLinkedLibraryKind::Static,
            artifact: OvenSelectedRustFacetPath {
                owner: archive_owner.clone(),
                path: "libfixture.a".to_string(),
            },
            digest: selected_graph_sha256(b"linked archive"),
        }];
        let unit = unit.clone();
        let mut owner_roots = roots(root.path())?
            .into_iter()
            .map(|owner| (owner.identity, owner.root))
            .collect::<BTreeMap<_, _>>();
        owner_roots.insert(archive_owner, fs::canonicalize(&archive_root)?);

        assert!(materialize_linked_libraries(&graph, &owner_roots, &unit).is_err());
        Ok(())
    }

    #[test]
    fn materializes_provider_only_from_declared_provider_owner() -> Result<(), Box<dyn std::error::Error>> {
        use oven_model::oven_interop::{
            OvenInteropExecutionProvenance, OvenInteropExecutionReceipt, interop_execution_receipt_identity,
        };

        let root = tempfile::tempdir()?;
        let provider = selected_graph_sha256(b"linked provider");
        let provider_root = root.path().join("provider");
        let framework = provider_root.join("Security.framework/Security");
        let provenance = provider_root.join("provenance/interop-execution.json");
        fs::create_dir_all(framework.parent().ok_or("framework has no parent")?)?;
        fs::create_dir_all(provenance.parent().ok_or("provenance has no parent")?)?;
        fs::write(&framework, b"framework")?;
        let mut receipt = OvenInteropExecutionReceipt {
            schema_version: OVEN_INTEROP_EXECUTION_RECEIPT_SCHEMA_VERSION,
            locked_target_identity: selected_graph_sha256(b"locked target"),
            target: "aarch64-apple-darwin".to_string(),
            toolchain: None,
            sdk: None,
            identity: String::new(),
        };
        receipt.identity = interop_execution_receipt_identity(&receipt)?;
        let receipt_identity = receipt.identity.clone();
        let held = OvenInteropExecutionProvenance {
            schema_version: OVEN_INTEROP_EXECUTION_PROVENANCE_SCHEMA_VERSION,
            receipt,
            archives: Vec::new(),
            bundles: Vec::new(),
            system_capabilities: vec!["apple.framework.Security".to_string()],
        };
        let provenance_bytes = serde_json::to_vec_pretty(&held)?;
        fs::write(&provenance, &provenance_bytes)?;
        let members = vec![
            OvenSelectedRustFacetSourceMember {
                path: "Security.framework/Security".to_string(),
                digest: selected_graph_sha256(b"framework"),
            },
            OvenSelectedRustFacetSourceMember {
                path: "provenance/interop-execution.json".to_string(),
                digest: selected_graph_sha256(&provenance_bytes),
            },
        ];
        let mut graph = selected_graph()?.graph().clone();
        graph.selection.intent.target = "aarch64-apple-darwin".to_string();
        graph.owners.push(OvenSelectedRustFacetOwner {
            identity: provider.clone(),
            kind: OvenSelectedRustFacetOwnerKind::LinkedLibraryProvider,
        });
        let unit = graph.units.first_mut().ok_or("fixture has no selected unit")?;
        unit.linked_libraries = vec![crate::rustc::OvenSelectedRustFacetLinkedLibrary::Provider {
            details: Box::new(crate::rustc::OvenSelectedRustFacetLinkedLibraryProvider {
                name: "Security".to_string(),
                kind: crate::rustc::OvenSelectedRustFacetLinkedLibraryKind::Framework,
                provider: provider.clone(),
                target: "aarch64-apple-darwin".to_string(),
                capability: "apple.framework.Security".to_string(),
                receipt_identity: receipt_identity.clone(),
                provenance: OvenSelectedRustFacetPath {
                    owner: provider.clone(),
                    path: "provenance/interop-execution.json".to_string(),
                },
                provenance_digest: selected_graph_sha256(&provenance_bytes),
                search_root: OvenSelectedRustFacetPath {
                    owner: provider.clone(),
                    path: ".".to_string(),
                },
                artifact: OvenSelectedRustFacetPath {
                    owner: provider.clone(),
                    path: "Security.framework/Security".to_string(),
                },
                digest: selected_graph_sha256(b"framework"),
                members,
            }),
        }];
        let unit = unit.clone();
        let mut owner_roots = roots(root.path())?
            .into_iter()
            .map(|owner| (owner.identity, owner.root))
            .collect::<BTreeMap<_, _>>();
        owner_roots.insert(provider, fs::canonicalize(&provider_root)?);

        let linked = materialize_linked_libraries(&graph, &owner_roots, &unit)?;
        assert!(matches!(
            &linked[0],
            OvenMaterializedRustFacetLinkedLibrary::Provider {
                receipt_identity: actual_receipt,
                search_root: actual_root,
                artifact: actual_artifact,
                ..
            } if actual_receipt == &receipt_identity
                && actual_root == &fs::canonicalize(&provider_root)?
                && actual_artifact == &fs::canonicalize(&framework)?
        ));
        let mut unheld = unit.clone();
        if let crate::rustc::OvenSelectedRustFacetLinkedLibrary::Provider { details } = &mut unheld.linked_libraries[0]
        {
            details.capability = "apple.framework.Unheld".to_string();
        }
        assert!(materialize_linked_libraries(&graph, &owner_roots, &unheld).is_err());

        let mut tampered = held;
        tampered.receipt.locked_target_identity = selected_graph_sha256(b"substituted locked target");
        let tampered_bytes = serde_json::to_vec_pretty(&tampered)?;
        fs::write(&provenance, &tampered_bytes)?;
        let mut tampered_unit = unit;
        if let crate::rustc::OvenSelectedRustFacetLinkedLibrary::Provider { details } =
            &mut tampered_unit.linked_libraries[0]
        {
            details.provenance_digest = selected_graph_sha256(&tampered_bytes);
            let provenance_member = details
                .members
                .iter_mut()
                .find(|member| member.path == "provenance/interop-execution.json")
                .ok_or("provider fixture has no provenance member")?;
            provenance_member.digest = selected_graph_sha256(&tampered_bytes);
        }
        let Err(error) = materialize_linked_libraries(&graph, &owner_roots, &tampered_unit) else {
            return Err("provider with a self-asserted receipt identity materialized".into());
        };
        assert!(error.to_string().contains("does not match canonical identity"));
        assert!(
            validate_provider_target(
                crate::rustc::OvenSelectedRustFacetLinkedLibraryKind::Framework,
                "x86_64-unknown-linux-gnu",
            )
            .is_err()
        );
        Ok(())
    }

    /// An admitted provider-only file remains part of the exact physical source tree without becoming a compiled
    /// Rust unit input. This is the generic lower-level JEC contract used by runtime-foundation build scripts.
    #[test]
    fn materializes_admitted_supplemental_source_without_rekeying_compiled_unit()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let selected = selected_graph()?;
        let roots = roots(root.path())?;
        let selected_identity = selected
            .graph()
            .units
            .first()
            .map(|unit| unit.identity.clone())
            .ok_or("fixture has no selected unit")?;
        let baseline = compiled_rust_unit_identities(&selected, COMPILER_CLOSURE)?;
        fs::write(root.path().join("source/build.rs"), b"fn main() {}\n")?;
        let supplemental = [OvenSelectedRustFacetSupplementalSourceMembers {
            selected_identity: selected_identity.clone(),
            source_root: OvenSelectedRustFacetPath {
                owner: source_owner(),
                path: ".".to_string(),
            },
            members: vec![OvenSelectedRustFacetSourceMember {
                path: "build.rs".to_string(),
                digest: selected_graph_sha256(b"fn main() {}\n"),
            }],
        }];

        let materialized = materialize_selected_rust_facet_graph_with_supplemental_source_members(
            &selected,
            COMPILER_CLOSURE,
            &roots,
            &supplemental,
        )?;
        let unit = materialized
            .unit(&selected_identity)
            .ok_or("supplemented selected unit did not materialize")?;
        let expected = baseline
            .get(&selected_identity)
            .ok_or("fixture has no compiled identity")?;
        assert_eq!(unit.compiled_identity, expected.clone());
        Ok(())
    }

    /// A provider may explicitly retain a compiler-visible file as part of its own read closure when both authorities
    /// bind the same bytes. Such overlap is not a second input or a conflicting source identity.
    #[test]
    fn materializes_byte_identical_supplemental_compiler_visible_member() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let selected = selected_graph()?;
        let roots = roots(root.path())?;
        let selected_identity = selected
            .graph()
            .units
            .first()
            .map(|unit| unit.identity.clone())
            .ok_or("fixture has no selected unit")?;
        let supplemental = [OvenSelectedRustFacetSupplementalSourceMembers {
            selected_identity: selected_identity.clone(),
            source_root: OvenSelectedRustFacetPath {
                owner: source_owner(),
                path: ".".to_string(),
            },
            members: vec![OvenSelectedRustFacetSourceMember {
                path: "src/lib.rs".to_string(),
                digest: selected_graph_sha256(b"pub fn marker() -> u8 { 7 }\n"),
            }],
        }];

        assert!(
            materialize_selected_rust_facet_graph_with_supplemental_source_members(
                &selected,
                COMPILER_CLOSURE,
                &roots,
                &supplemental,
            )?
            .unit(&selected_identity)
            .is_some()
        );
        Ok(())
    }

    /// A provider/package root can sit above `src` without rekeying the compiler-visible Rust unit.
    #[test]
    fn materializes_package_root_supplement_without_rekeying_compiled_unit() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let selected = selected_graph_with_compiler_root()?;
        let roots = roots(root.path())?;
        let package_root = root.path().join("source");
        fs::write(
            package_root.join("Cargo.toml"),
            b"[package]\nname = \"fixture\"\nversion = \"1.0.0\"\nbuild = \"build.rs\"\n",
        )?;
        fs::write(package_root.join("build.rs"), b"fn main() {}\n")?;
        let selected_identity = selected
            .graph()
            .units
            .first()
            .map(|unit| unit.identity.clone())
            .ok_or("fixture has no selected unit")?;
        let baseline = compiled_rust_unit_identities(&selected, COMPILER_CLOSURE)?;
        let supplemental = [OvenSelectedRustFacetSupplementalSourceMembers {
            selected_identity: selected_identity.clone(),
            source_root: OvenSelectedRustFacetPath {
                owner: source_owner(),
                path: ".".to_string(),
            },
            members: vec![
                OvenSelectedRustFacetSourceMember {
                    path: "Cargo.toml".to_string(),
                    digest: selected_graph_sha256(
                        b"[package]\nname = \"fixture\"\nversion = \"1.0.0\"\nbuild = \"build.rs\"\n",
                    ),
                },
                OvenSelectedRustFacetSourceMember {
                    path: "build.rs".to_string(),
                    digest: selected_graph_sha256(b"fn main() {}\n"),
                },
                OvenSelectedRustFacetSourceMember {
                    path: "src/lib.rs".to_string(),
                    digest: selected_graph_sha256(b"pub fn marker() -> u8 { 7 }\n"),
                },
            ],
        }];

        let materialized = materialize_selected_rust_facet_graph_with_supplemental_source_members(
            &selected,
            COMPILER_CLOSURE,
            &roots,
            &supplemental,
        )?;
        // Canonicalized on both sides: materialization resolves the root through the filesystem while the fixture
        // holds the path it created, so on a host whose temporary directory sits behind a symlink -- `/var` to
        // `/private/var` on macOS -- the two spell one directory differently.
        assert_eq!(
            materialized
                .supplemental_source_root(&supplemental[0].source_root)
                .ok_or("package-root fixture lost its admitted supplemental root")?
                .canonicalize()?,
            package_root.as_path().canonicalize()?
        );
        assert_eq!(
            materialized
                .unit(&selected_identity)
                .ok_or("package-root fixture did not materialize its compiler unit")?
                .compiled_identity,
            baseline
                .get(&selected_identity)
                .ok_or("fixture has no compiled identity")?
                .clone()
        );

        fs::write(package_root.join("Cargo.toml"), b"[package]\nname = \"changed\"\n")?;
        assert!(matches!(
            materialize_selected_rust_facet_graph_with_supplemental_source_members(
                &selected,
                COMPILER_CLOSURE,
                &roots,
                &supplemental,
            ),
            Err(OvenRustcError::ArtifactDigestMismatch { .. })
        ));
        Ok(())
    }

    /// A supplemental root may contain a compiler root but may not point to a sibling package tree.
    #[test]
    fn refuses_supplemental_root_outside_compiler_root() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let selected = selected_graph_with_compiler_root()?;
        let roots = roots(root.path())?;
        let selected_identity = selected
            .graph()
            .units
            .first()
            .map(|unit| unit.identity.clone())
            .ok_or("fixture has no selected unit")?;
        let sibling = root.path().join("source/sibling");
        fs::create_dir_all(&sibling)?;
        fs::write(sibling.join("build.rs"), b"fn main() {}\n")?;
        let supplemental = [OvenSelectedRustFacetSupplementalSourceMembers {
            selected_identity,
            source_root: OvenSelectedRustFacetPath {
                owner: source_owner(),
                path: "sibling".to_string(),
            },
            members: vec![OvenSelectedRustFacetSourceMember {
                path: "build.rs".to_string(),
                digest: selected_graph_sha256(b"fn main() {}\n"),
            }],
        }];
        assert!(matches!(
            materialize_selected_rust_facet_graph_with_supplemental_source_members(
                &selected,
                COMPILER_CLOSURE,
                &roots,
                &supplemental,
            ),
            Err(OvenRustcError::InvalidInput {
                field: "selected Rust supplemental source",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn refuses_an_extra_unselected_source_file() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let selected = selected_graph()?;
        let roots = roots(root.path())?;
        fs::write(root.path().join("source/src/unselected.rs"), b"pub fn hidden() {}\n")?;
        assert!(matches!(
            materialize_selected_rust_facet_graph(&selected, COMPILER_CLOSURE, &roots),
            Err(OvenRustcError::InvalidInput {
                field: "selected Rust source tree",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn refuses_owner_table_widening() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let selected = selected_graph()?;
        let mut roots = roots(root.path())?;
        roots.push(OvenSelectedRustFacetOwnerRoot {
            identity: selected_graph_sha256(b"unselected owner"),
            root: root.path().to_path_buf(),
        });
        assert!(matches!(
            materialize_selected_rust_facet_graph(&selected, COMPILER_CLOSURE, &roots),
            Err(OvenRustcError::InvalidInput {
                field: "selected Rust owner roots",
                ..
            })
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_selected_source_member() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let selected = selected_graph()?;
        let roots = roots(root.path())?;
        let source = root.path().join("source/src/lib.rs");
        let replacement = root.path().join("replacement.rs");
        fs::write(&replacement, b"pub fn marker() -> u8 { 7 }\n")?;
        fs::remove_file(&source)?;
        symlink(&replacement, &source)?;
        assert!(matches!(
            materialize_selected_rust_facet_graph(&selected, COMPILER_CLOSURE, &roots),
            Err(OvenRustcError::InvalidInput {
                field: "selected Rust source tree",
                ..
            })
        ));
        Ok(())
    }
}
