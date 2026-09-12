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
use std::fs;
use std::path::{Component, Path, PathBuf};

use super::{
    OvenCompiledRustUnitIdentity, OvenRustcError, OvenSelectedRustFacetEnvironmentValue,
    OvenSelectedRustFacetGeneratedInput, OvenSelectedRustFacetGraph, OvenSelectedRustFacetPath,
    OvenSelectedRustFacetSourceMember, OvenSelectedRustFacetUnit, ValidatedOvenSelectedRustFacetGraph,
    compiled_rust_unit_identities, digest_regular_file,
};

/// One physical root for an owner named by a validated selected facet graph.
///
/// The caller obtains these roots only from retained Store/Loaf/provider owners. This shape carries no authority to
/// search a checkout, registry cache, Cargo home, or neighbouring artifact directory.
#[derive(Debug, Clone)]
pub(crate) struct OvenSelectedRustFacetOwnerRoot {
    pub(crate) identity: String,
    pub(crate) root: PathBuf,
}

/// Supplemental package-local source members supplied by an already admitted execution record.
///
/// The selected unit's source root is the compiler-visible root. A provider/package root may be the same root or an
/// ancestor below the same retained owner, for example a package root `.` above a compiler root `src`. These members
/// never enter the selected unit's compiler-visible source catalogue or compiled identity. The higher-level record
/// must already bind their owner, root and bytes; this adapter only verifies that exact physical tree and refuses any
/// hidden file.
#[derive(Debug, Clone)]
pub(crate) struct OvenSelectedRustFacetSupplementalSourceMembers {
    /// Existing selected unit to which this package-local source evidence is attached.
    pub(crate) selected_identity: String,
    /// Existing owner-relative root of the complete supplemental tree.
    pub(crate) source_root: OvenSelectedRustFacetPath,
    /// Exact portable members admitted by the higher-level execution record, relative to `source_root`.
    pub(crate) members: Vec<OvenSelectedRustFacetSourceMember>,
}

/// Exact additional members grouped by one selected owner/root pair.
type SupplementalSourceTrees = BTreeMap<(String, String), BTreeMap<String, String>>;

/// A compiler-visible environment value rebound from a portable selected graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OvenMaterializedRustFacetEnvironmentValue {
    Text(String),
    Path(PathBuf),
}

/// A physically verified selected source unit ready for the direct-Rustc publisher.
#[derive(Debug, Clone)]
pub(crate) struct OvenMaterializedRustFacetUnit {
    /// Source-selection identity used to preserve graph topology and source authority.
    pub(crate) selected_identity: String,
    /// Effective compiler-input identity used for immutable output reuse.
    pub(crate) compiled_identity: OvenCompiledRustUnitIdentity,
    pub(crate) source_root: PathBuf,
    pub(crate) root_module: PathBuf,
    pub(crate) include_dirs: Vec<PathBuf>,
    pub(crate) exclude_dirs: Vec<PathBuf>,
    pub(crate) environment: BTreeMap<String, OvenMaterializedRustFacetEnvironmentValue>,
    pub(crate) generated_inputs: Vec<(String, PathBuf, String)>,
}

/// A physically admitted selected graph and its source-unit projection.
#[derive(Debug, Clone)]
pub(crate) struct OvenMaterializedRustFacetGraph {
    target_spec: PathBuf,
    units: BTreeMap<String, OvenMaterializedRustFacetUnit>,
    supplemental_source_roots: BTreeMap<(String, String), PathBuf>,
}

impl OvenMaterializedRustFacetGraph {
    /// Return the compiler-selected target-spec JSON after digest verification.
    pub(crate) fn target_spec(&self) -> &Path {
        &self.target_spec
    }

    /// Look up one selected source unit by its source-selection identity.
    pub(crate) fn unit(&self, selected_identity: &str) -> Option<&OvenMaterializedRustFacetUnit> {
        self.units.get(selected_identity)
    }

    /// Iterate deterministic source-selection identities and their admitted physical units.
    pub(crate) fn units(&self) -> impl Iterator<Item = (&str, &OvenMaterializedRustFacetUnit)> {
        self.units.iter().map(|(identity, unit)| (identity.as_str(), unit))
    }

    /// Return one already verified supplemental package root named by the higher-level admitted record.
    ///
    /// The graph deliberately exposes only roots the materializer already checked against an exact supplemental
    /// inventory. It does not expose an arbitrary owner-relative directory for later source discovery.
    pub(crate) fn supplemental_source_root(&self, root: &OvenSelectedRustFacetPath) -> Option<&Path> {
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
pub(crate) fn materialize_selected_rust_facet_graph(
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
pub(crate) fn materialize_selected_rust_facet_graph_with_supplemental_source_members(
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
    let target_spec = resolve_file(
        &owners,
        &graph.selection.target_spec.source,
        &graph.selection.target_spec.digest,
        "selected Rust target spec",
    )?;
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
        target_spec,
        units,
        supplemental_source_roots,
    })
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
        super::selected_graph_source_digest(&members).map_err(|error| OvenRustcError::InvalidInput {
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

fn resolve_directory(
    owners: &BTreeMap<String, PathBuf>,
    owner: &str,
    relative: &str,
    field: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    let path = resolve_relative_owner_path(owners, owner, relative, field)?;
    canonical_directory(&path, field)
}

fn resolve_directory_path(
    owners: &BTreeMap<String, PathBuf>,
    path: &OvenSelectedRustFacetPath,
    field: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    let path = resolve_path(owners, path, field)?;
    canonical_directory(&path, field)
}

fn resolve_file(
    owners: &BTreeMap<String, PathBuf>,
    path: &OvenSelectedRustFacetPath,
    expected_digest: &str,
    field: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    let path = resolve_path(owners, path, field)?;
    verify_file(&path, expected_digest, field)
}

fn resolve_file_relative(
    root: &Path,
    relative: &str,
    expected_digest: &str,
    field: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    let path = resolve_relative_path(root, relative, field)?;
    verify_file(&path, expected_digest, field)
}

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

fn resolve_path(
    owners: &BTreeMap<String, PathBuf>,
    path: &OvenSelectedRustFacetPath,
    field: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    resolve_relative_owner_path(owners, &path.owner, &path.path, field)
}

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
    use crate::oven::rustc::{
        OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION, OvenSelectedRustFacetCrateKind, OvenSelectedRustFacetDomain,
        OvenSelectedRustFacetGraph, OvenSelectedRustFacetOwner, OvenSelectedRustFacetOwnerKind,
        OvenSelectedRustFacetPurpose, OvenSelectedRustFacetSelection, OvenSelectedRustFacetSource,
        OvenSelectedRustFacetSourceKind, OvenSelectedRustFacetTargetSpec, OvenSelectedRustFacetUnitRole,
        compiled_rust_unit_identities, selected_graph_sha256, selected_graph_source_digest,
        selected_graph_unit_identity,
    };

    const COMPILER_CLOSURE: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn source_owner() -> String {
        selected_graph_sha256(b"selected-unit source owner")
    }

    fn toolchain_owner() -> String {
        selected_graph_sha256(b"selected-unit toolchain owner")
    }

    fn selection() -> OvenSelectedRustFacetSelection {
        OvenSelectedRustFacetSelection {
            intent: super::super::OvenSelectedRustFacetIntent {
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: "rustc 1.85.0 (fixture)".to_string(),
                profile: "debug".to_string(),
                features: Vec::new(),
            },
            host: "x86_64-unknown-linux-gnu".to_string(),
            purpose: OvenSelectedRustFacetPurpose::Normal,
            default_features: false,
            toolchain_version: "1.85.0".to_string(),
            target_spec: OvenSelectedRustFacetTargetSpec {
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
            default_features: false,
            cfg: Vec::new(),
            environment: BTreeMap::new(),
            include_dirs: vec![OvenSelectedRustFacetPath {
                owner: owner.clone(),
                path: ".".to_string(),
            }],
            exclude_dirs: Vec::new(),
            dependencies: Vec::new(),
            generated_inputs: Vec::new(),
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
            exposed_roots: BTreeMap::from([("fixture".to_string(), identity)]),
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
        graph.exposed_roots = BTreeMap::from([("fixture".to_string(), unit.identity.clone())]);
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
        assert!(materialized.target_spec().ends_with("target-spec.json"));
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
