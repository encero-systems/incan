//! Content identities for direct-Rustc compilation units.
//!
//! A selected Rust facet graph records *source authority*: package coordinates, selected source identities and
//! owners must remain visible there so the source adapter cannot confuse two independently admitted packages. A
//! compiler output has a narrower identity. It depends on the effective compiler invocation and the byte identities
//! of its inputs, not on an unobserved package version or source coordinate. Keeping the two identities separate is
//! the core JEC rule: publishing `package@0.2.0` with exactly the same effective inputs as `package@0.1.0` may reuse
//! the already compiled unit.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{
    OvenRustcError, OvenSelectedRustFacetCrateKind, OvenSelectedRustFacetDependency, OvenSelectedRustFacetDomain,
    OvenSelectedRustFacetEnvironmentValue, OvenSelectedRustFacetGeneratedInput, OvenSelectedRustFacetGraph,
    OvenSelectedRustFacetPath, OvenSelectedRustFacetUnit, OvenSelectedRustFacetUnitRole,
    ValidatedOvenSelectedRustFacetGraph, digest_bytes,
};

/// Domain separator for a compiled unit identity.
///
/// Version 2 drops the selection-global root feature set from the identity. The two schemes name different things,
/// so the separator moves with them rather than letting a v1 identity be mistaken for a v2 one.
pub(crate) const OVEN_COMPILED_RUST_UNIT_IDENTITY_DOMAIN: &str = "incan.oven.compiled-rust-unit/2";

/// Content address of one direct-Rustc compilation unit.
///
/// This is intentionally separate from a selected graph unit identity. The latter names the source authority; this
/// value names an output that may be reused only when the compiler-visible inputs are the same.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct OvenCompiledRustUnitIdentity(String);

impl OvenCompiledRustUnitIdentity {
    /// Return the canonical SHA-256 identity.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Serialize)]
struct CompiledUnitIdentityInput<'a> {
    compiler_closure_digest: &'a str,
    toolchain: &'a str,
    toolchain_version: &'a str,
    profile: &'a str,
    purpose: super::OvenSelectedRustFacetPurpose,
    compilation_target: &'a str,
    target_spec_digest: Option<&'a str>,
    crate_name: &'a str,
    crate_kind: OvenSelectedRustFacetCrateKind,
    role: OvenSelectedRustFacetUnitRole,
    domain: OvenSelectedRustFacetDomain,
    edition: &'a str,
    source_digest: &'a str,
    source_members: &'a [super::OvenSelectedRustFacetSourceMember],
    root_module: &'a str,
    features: &'a [String],
    default_features: bool,
    cfg: &'a [String],
    environment: BTreeMap<&'a str, CompiledEnvironmentValue<'a>>,
    include_dirs: Vec<CompiledPath<'a>>,
    exclude_dirs: Vec<CompiledPath<'a>>,
    generated_inputs: Vec<CompiledGeneratedInput<'a>>,
    dependencies: Vec<CompiledDependency<'a>>,
}

/// A physical owner-relative path becomes source-root-relative when it names this unit's own complete source tree.
/// Other paths remain conservative owner bindings until the source publisher can supply their independent content
/// closure. That prevents a path-only include directory from being mistaken for content-addressed input.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CompiledPath<'a> {
    UnitSourceRoot,
    OwnerPath { owner: &'a str, path: &'a str },
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CompiledEnvironmentValue<'a> {
    Text { value: &'a str },
    Path { value: CompiledPath<'a> },
    SensitiveDigest { hmac_sha256: &'a str },
}

#[derive(Serialize)]
struct CompiledGeneratedInput<'a> {
    name: &'a str,
    source: CompiledPath<'a>,
    digest: &'a str,
}

#[derive(Serialize)]
struct CompiledDependency<'a> {
    alias: &'a str,
    unit: &'a str,
}

/// Derive content identities for every unit in an already validated selected graph.
///
/// `compiler_closure_digest` is the direct-Rustc compiler owner's content identity, not the selected graph's
/// inspection-toolchain owner. Semantic inspection and native execution deliberately have distinct closures.
/// Dependency links are rekeyed from source-selection identities to child compiled identities as the DAG is walked.
pub(crate) fn compiled_rust_unit_identities(
    selected: &ValidatedOvenSelectedRustFacetGraph,
    compiler_closure_digest: &str,
) -> Result<BTreeMap<String, OvenCompiledRustUnitIdentity>, OvenRustcError> {
    validate_compiler_closure_digest(compiler_closure_digest)?;
    let graph = selected.graph();
    let mut pending = graph
        .units
        .iter()
        .map(|unit| (unit.identity.as_str(), unit))
        .collect::<BTreeMap<_, _>>();
    let mut identities = BTreeMap::new();

    while !pending.is_empty() {
        let ready = pending
            .iter()
            .filter_map(|(selected_identity, unit)| {
                unit.dependencies
                    .iter()
                    .all(|dependency| identities.contains_key(dependency.unit.as_str()))
                    .then_some(*selected_identity)
            })
            .collect::<Vec<_>>();
        if ready.is_empty() {
            return Err(OvenRustcError::InvalidInput {
                field: "compiled Rust unit graph",
                message: "validated selected graph has no compilable dependency frontier".to_string(),
            });
        }
        for selected_identity in ready {
            let unit = pending
                .remove(selected_identity)
                .ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "compiled Rust unit graph",
                    message: "selected unit disappeared while deriving identities".to_string(),
                })?;
            let dependencies = unit
                .dependencies
                .iter()
                .map(|dependency| compiled_dependency(dependency, &identities))
                .collect::<Result<Vec<_>, _>>()?;
            let input = compiled_unit_identity_input(graph, unit, compiler_closure_digest, dependencies);
            let bytes = serde_json::to_vec(&(OVEN_COMPILED_RUST_UNIT_IDENTITY_DOMAIN, input)).map_err(|error| {
                OvenRustcError::InvalidInput {
                    field: "compiled Rust unit identity",
                    message: format!("cannot encode effective compiler inputs: {error}"),
                }
            })?;
            identities.insert(
                selected_identity.to_string(),
                OvenCompiledRustUnitIdentity(digest_bytes(&bytes)),
            );
        }
    }
    Ok(identities)
}

/// Require the compiler-closure digest folded into a compiled identity to be a real SHA-256 identity.
///
/// It arrives as a caller-supplied string, and a malformed one would still hash into a plausible-looking unit
/// identity that no other machine could reproduce.
fn validate_compiler_closure_digest(value: &str) -> Result<(), OvenRustcError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(OvenRustcError::InvalidInput {
            field: "compiled Rust unit compiler closure",
            message: "must be a SHA-256 identity".to_string(),
        });
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(OvenRustcError::InvalidInput {
            field: "compiled Rust unit compiler closure",
            message: "has malformed SHA-256 bytes".to_string(),
        });
    }
    Ok(())
}

/// Resolve one declared edge to the compiled identity of the unit it links against.
///
/// Folding the dependency's *compiled* identity rather than its selected one is what makes a unit identity a
/// Merkle root over its closure: a dependency that recompiles differently changes its dependents.
fn compiled_dependency<'a>(
    dependency: &'a OvenSelectedRustFacetDependency,
    identities: &'a BTreeMap<String, OvenCompiledRustUnitIdentity>,
) -> Result<CompiledDependency<'a>, OvenRustcError> {
    let identity = identities
        .get(&dependency.unit)
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "compiled Rust unit dependency",
            message: format!("selected child `{}` has no compiled identity", dependency.unit),
        })?;
    Ok(CompiledDependency {
        alias: &dependency.alias,
        unit: identity.as_str(),
    })
}

/// Assemble everything a compiled unit's identity is taken over, in one canonical shape.
///
/// Gathering the inputs here rather than hashing them at the call site is what keeps the identity auditable: a
/// new compiler input has to appear in this struct to reach the digest, so the set can be read in one place.
fn compiled_unit_identity_input<'a>(
    graph: &'a OvenSelectedRustFacetGraph,
    unit: &'a OvenSelectedRustFacetUnit,
    compiler_closure_digest: &'a str,
    dependencies: Vec<CompiledDependency<'a>>,
) -> CompiledUnitIdentityInput<'a> {
    let (compilation_target, target_spec_digest) = match unit.domain {
        OvenSelectedRustFacetDomain::Host => (graph.selection.host.as_str(), None),
        OvenSelectedRustFacetDomain::Target => (
            graph.selection.intent.target.as_str(),
            Some(graph.selection.target_spec.digest.as_str()),
        ),
    };
    CompiledUnitIdentityInput {
        compiler_closure_digest,
        toolchain: &graph.selection.intent.toolchain,
        toolchain_version: &graph.selection.toolchain_version,
        profile: &graph.selection.intent.profile,
        purpose: graph.selection.purpose,
        compilation_target,
        target_spec_digest,
        crate_name: &unit.crate_name,
        crate_kind: unit.crate_kind,
        role: unit.role,
        domain: unit.domain,
        edition: &unit.edition,
        source_digest: &unit.source.digest,
        source_members: &unit.source_members,
        root_module: &unit.root_module,
        features: &unit.features,
        default_features: unit.default_features,
        cfg: &unit.cfg,
        environment: unit
            .environment
            .iter()
            .map(|(name, value)| (name.as_str(), compiled_environment_value(value, unit)))
            .collect(),
        include_dirs: unit.include_dirs.iter().map(|path| compiled_path(path, unit)).collect(),
        exclude_dirs: unit.exclude_dirs.iter().map(|path| compiled_path(path, unit)).collect(),
        generated_inputs: unit
            .generated_inputs
            .iter()
            .map(|input| compiled_generated_input(input, unit))
            .collect(),
        dependencies,
    }
}

/// Express one declared path relative to the unit's own source root wherever it names that root.
///
/// A path inside the unit becomes a root-relative form and an outside one keeps its owner, so the identity does
/// not change when the same unit is materialized under a different owner root.
fn compiled_path<'a>(path: &'a OvenSelectedRustFacetPath, unit: &'a OvenSelectedRustFacetUnit) -> CompiledPath<'a> {
    if path.owner == unit.source.owner && path.path == unit.source.root {
        CompiledPath::UnitSourceRoot
    } else {
        CompiledPath::OwnerPath {
            owner: &path.owner,
            path: &path.path,
        }
    }
}

/// Project one declared environment value into its identity form, relocating any path it carries.
fn compiled_environment_value<'a>(
    value: &'a OvenSelectedRustFacetEnvironmentValue,
    unit: &'a OvenSelectedRustFacetUnit,
) -> CompiledEnvironmentValue<'a> {
    match value {
        OvenSelectedRustFacetEnvironmentValue::Text { value } => CompiledEnvironmentValue::Text { value },
        OvenSelectedRustFacetEnvironmentValue::Path { value } => CompiledEnvironmentValue::Path {
            value: compiled_path(value, unit),
        },
        OvenSelectedRustFacetEnvironmentValue::SensitiveDigest { hmac_sha256 } => {
            CompiledEnvironmentValue::SensitiveDigest { hmac_sha256 }
        }
    }
}

/// Project one declared build-script output into its identity form, relocating the path it names.
fn compiled_generated_input<'a>(
    input: &'a OvenSelectedRustFacetGeneratedInput,
    unit: &'a OvenSelectedRustFacetUnit,
) -> CompiledGeneratedInput<'a> {
    CompiledGeneratedInput {
        name: &input.name,
        source: compiled_path(&input.source, unit),
        digest: &input.digest,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::oven::rustc::{
        OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION, OvenSelectedRustFacetGraph, OvenSelectedRustFacetIntent,
        OvenSelectedRustFacetOwner, OvenSelectedRustFacetOwnerKind, OvenSelectedRustFacetPurpose,
        OvenSelectedRustFacetSelection, OvenSelectedRustFacetSource, OvenSelectedRustFacetSourceKind,
        OvenSelectedRustFacetSourceMember, OvenSelectedRustFacetTargetSpec, selected_graph_sha256,
        selected_graph_source_digest, selected_graph_unit_identity,
    };

    const COMPILER_CLOSURE: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn source_owner() -> String {
        selected_graph_sha256(b"fixture source owner")
    }

    fn toolchain_owner() -> String {
        selected_graph_sha256(b"fixture inspection toolchain")
    }

    fn source_member(path: &str, bytes: &[u8]) -> OvenSelectedRustFacetSourceMember {
        OvenSelectedRustFacetSourceMember {
            path: path.to_string(),
            digest: selected_graph_sha256(bytes),
        }
    }

    fn selection() -> OvenSelectedRustFacetSelection {
        OvenSelectedRustFacetSelection {
            intent: OvenSelectedRustFacetIntent {
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: "rustc 1.85.0 (fixture)".to_string(),
                profile: "debug".to_string(),
                features: vec!["root-feature".to_string()],
            },
            host: "aarch64-apple-darwin".to_string(),
            purpose: OvenSelectedRustFacetPurpose::Normal,
            default_features: true,
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

    fn graph(
        package_version: &str,
        include_package_version: bool,
    ) -> Result<ValidatedOvenSelectedRustFacetGraph, Box<dyn std::error::Error>> {
        graph_with_owner(package_version, include_package_version, source_owner())
    }

    fn graph_with_owner(
        package_version: &str,
        include_package_version: bool,
        owner: String,
    ) -> Result<ValidatedOvenSelectedRustFacetGraph, Box<dyn std::error::Error>> {
        let selection = selection();
        let members = vec![source_member("src/lib.rs", b"pub fn marker() -> u8 { 7 }\n")];
        let mut environment = BTreeMap::new();
        if include_package_version {
            environment.insert(
                "CARGO_PKG_VERSION".to_string(),
                OvenSelectedRustFacetEnvironmentValue::Text {
                    value: package_version.to_string(),
                },
            );
        }
        let mut unit = OvenSelectedRustFacetUnit {
            identity: String::new(),
            package: "fixture".to_string(),
            package_version: package_version.to_string(),
            crate_name: "fixture".to_string(),
            crate_kind: OvenSelectedRustFacetCrateKind::Rlib,
            role: OvenSelectedRustFacetUnitRole::Library,
            domain: OvenSelectedRustFacetDomain::Target,
            edition: "2024".to_string(),
            source: OvenSelectedRustFacetSource {
                kind: OvenSelectedRustFacetSourceKind::Registry,
                identity: format!("registry:fixture@{package_version}"),
                owner: owner.clone(),
                root: ".".to_string(),
                digest: selected_graph_source_digest(&members)?,
            },
            root_module: "src/lib.rs".to_string(),
            source_members: members,
            features: vec!["feature_a".to_string()],
            default_features: true,
            cfg: vec!["feature=\"feature_a\"".to_string()],
            environment,
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
                    kind: OvenSelectedRustFacetOwnerKind::Constituent,
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

    /// Build the fixture graph with the selection mutated, leaving every unit fact this unit compiles with intact.
    ///
    /// The unit keeps its own features, default-feature choice, cfg, source, environment and dependencies; only the
    /// selection-global root feature set changes. Nothing in the `rustc` command for this unit depends on it.
    fn graph_with_root_selection(
        root_features: &[&str],
        root_default_features: bool,
    ) -> Result<ValidatedOvenSelectedRustFacetGraph, Box<dyn std::error::Error>> {
        let mut selection = selection();
        selection.intent.features = root_features.iter().map(|feature| (*feature).to_string()).collect();
        selection.default_features = root_default_features;
        let members = vec![source_member("src/lib.rs", b"pub fn marker() -> u8 { 7 }\n")];
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
                kind: OvenSelectedRustFacetSourceKind::Registry,
                identity: "registry:fixture@1.0.0".to_string(),
                owner: owner.clone(),
                root: ".".to_string(),
                digest: selected_graph_source_digest(&members)?,
            },
            root_module: "src/lib.rs".to_string(),
            source_members: members,
            features: vec!["feature_a".to_string()],
            default_features: true,
            cfg: vec!["feature=\"feature_a\"".to_string()],
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
                    kind: OvenSelectedRustFacetOwnerKind::Constituent,
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

    /// A fact the compilation never observes must not rekey its output. RFC 124 states that plainly, and the
    /// selection-global root feature set is the case Gate 6-7 review blocker 8 names: a workspace root toggling one
    /// of its own features leaves every dependency's `rustc` command byte-identical, yet moved every compiled
    /// identity in the graph and therefore reproduced every output.
    #[test]
    fn a_root_feature_the_unit_never_compiles_with_does_not_rekey_it() -> Result<(), Box<dyn std::error::Error>> {
        let baseline = graph_with_root_selection(&["root-feature"], true)?;
        let other_features = graph_with_root_selection(&["root-feature", "unrelated-root-feature"], true)?;
        let other_defaults = graph_with_root_selection(&["root-feature"], false)?;

        let identity_of = |graph: &ValidatedOvenSelectedRustFacetGraph| {
            compiled_rust_unit_identities(graph, COMPILER_CLOSURE)
                .map(|identities| identities.into_values().map(|identity| identity.0).collect::<Vec<_>>())
        };

        let expected = identity_of(&baseline)?;
        assert_eq!(expected.len(), 1);
        assert_eq!(
            identity_of(&other_features)?,
            expected,
            "adding a root feature this unit does not compile with must not rekey it"
        );
        assert_eq!(
            identity_of(&other_defaults)?,
            expected,
            "changing the root's default-feature choice must not rekey a unit that keeps its own"
        );
        Ok(())
    }

    /// The other two fields blocker 8 names are kept, and this is the mutation that justifies keeping them.
    ///
    /// `include_dirs` and `exclude_dirs` are per-unit, not selection-global, and they decide which directories are
    /// materialized into the compilation sandbox. A different directory is a different set of files `rustc` can
    /// reach through `include!`, `include_str!` or a generated module, so a compilation does observe them and an
    /// output compiled against one must not be reused for the other.
    #[test]
    fn a_units_own_include_directories_do_rekey_it() -> Result<(), Box<dyn std::error::Error>> {
        let baseline = graph_with_root_selection(&["root-feature"], true)?;
        let mut widened = graph_with_root_selection(&["root-feature"], true)?.graph().clone();
        let owner = source_owner();
        for unit in &mut widened.units {
            unit.include_dirs.push(OvenSelectedRustFacetPath {
                owner: owner.clone(),
                path: "vendored".to_string(),
            });
            unit.identity = selected_graph_unit_identity(&widened.selection, unit)?;
        }
        let exposed = widened
            .units
            .first()
            .map(|unit| unit.identity.clone())
            .ok_or("fixture graph has no unit")?;
        widened.exposed_roots = BTreeMap::from([("fixture".to_string(), exposed)]);
        let widened = widened.validated()?;

        let identity_of = |graph: &ValidatedOvenSelectedRustFacetGraph| {
            compiled_rust_unit_identities(graph, COMPILER_CLOSURE)
                .map(|identities| identities.into_values().map(|identity| identity.0).collect::<Vec<_>>())
        };

        assert_ne!(
            identity_of(&widened)?,
            identity_of(&baseline)?,
            "a directory the compilation can reach is an input, so adding one must rekey the unit"
        );
        Ok(())
    }

    fn graph_with_dependency(
        dependency_version: &str,
    ) -> Result<ValidatedOvenSelectedRustFacetGraph, Box<dyn std::error::Error>> {
        let selection = selection();
        let dependency_members = vec![source_member("src/lib.rs", b"pub fn dependency() -> u8 { 7 }\n")];
        let dependency_owner = selected_graph_sha256(b"fixture dependency source owner");
        let mut dependency = OvenSelectedRustFacetUnit {
            identity: String::new(),
            package: "fixture_dependency".to_string(),
            package_version: dependency_version.to_string(),
            crate_name: "fixture_dependency".to_string(),
            crate_kind: OvenSelectedRustFacetCrateKind::Rlib,
            role: OvenSelectedRustFacetUnitRole::Library,
            domain: OvenSelectedRustFacetDomain::Target,
            edition: "2024".to_string(),
            source: OvenSelectedRustFacetSource {
                kind: OvenSelectedRustFacetSourceKind::Registry,
                identity: format!("registry:fixture_dependency@{dependency_version}"),
                owner: dependency_owner.clone(),
                root: ".".to_string(),
                digest: selected_graph_source_digest(&dependency_members)?,
            },
            root_module: "src/lib.rs".to_string(),
            source_members: dependency_members,
            features: Vec::new(),
            default_features: false,
            cfg: Vec::new(),
            environment: BTreeMap::new(),
            include_dirs: vec![OvenSelectedRustFacetPath {
                owner: dependency_owner.clone(),
                path: ".".to_string(),
            }],
            exclude_dirs: Vec::new(),
            dependencies: Vec::new(),
            generated_inputs: Vec::new(),
        };
        dependency.identity = selected_graph_unit_identity(&selection, &dependency)?;

        let root_members = vec![source_member(
            "src/lib.rs",
            b"pub use fixture_dependency::dependency;\n",
        )];
        let root_owner = selected_graph_sha256(b"fixture parent source owner");
        let mut root = OvenSelectedRustFacetUnit {
            identity: String::new(),
            package: "fixture_parent".to_string(),
            package_version: "1.0.0".to_string(),
            crate_name: "fixture_parent".to_string(),
            crate_kind: OvenSelectedRustFacetCrateKind::Rlib,
            role: OvenSelectedRustFacetUnitRole::Library,
            domain: OvenSelectedRustFacetDomain::Target,
            edition: "2024".to_string(),
            source: OvenSelectedRustFacetSource {
                kind: OvenSelectedRustFacetSourceKind::Generated,
                identity: "generated:fixture-parent".to_string(),
                owner: root_owner.clone(),
                root: ".".to_string(),
                digest: selected_graph_source_digest(&root_members)?,
            },
            root_module: "src/lib.rs".to_string(),
            source_members: root_members,
            features: Vec::new(),
            default_features: false,
            cfg: Vec::new(),
            environment: BTreeMap::new(),
            include_dirs: vec![OvenSelectedRustFacetPath {
                owner: root_owner.clone(),
                path: ".".to_string(),
            }],
            exclude_dirs: Vec::new(),
            dependencies: vec![OvenSelectedRustFacetDependency {
                alias: "fixture_dependency".to_string(),
                unit: dependency.identity.clone(),
            }],
            generated_inputs: Vec::new(),
        };
        root.identity = selected_graph_unit_identity(&selection, &root)?;
        let root_identity = root.identity.clone();
        Ok(OvenSelectedRustFacetGraph {
            schema_version: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
            selection,
            owners: vec![
                OvenSelectedRustFacetOwner {
                    identity: root_owner,
                    kind: OvenSelectedRustFacetOwnerKind::ProjectAuthority,
                },
                OvenSelectedRustFacetOwner {
                    identity: dependency_owner,
                    kind: OvenSelectedRustFacetOwnerKind::Constituent,
                },
                OvenSelectedRustFacetOwner {
                    identity: toolchain_owner(),
                    kind: OvenSelectedRustFacetOwnerKind::Toolchain,
                },
            ],
            units: vec![root, dependency],
            exposed_roots: BTreeMap::from([("fixture_parent".to_string(), root_identity)]),
        }
        .validated()?)
    }

    fn only_identity(
        graph: &ValidatedOvenSelectedRustFacetGraph,
    ) -> Result<OvenCompiledRustUnitIdentity, Box<dyn std::error::Error>> {
        let identities = compiled_rust_unit_identities(graph, COMPILER_CLOSURE)?;
        identities
            .into_values()
            .next()
            .ok_or_else(|| "fixture graph produced no compiled unit identity".into())
    }

    #[test]
    fn package_coordinate_changes_do_not_recompile_byte_identical_units() -> Result<(), Box<dyn std::error::Error>> {
        let first = graph("0.1.0", false)?;
        let second = graph("0.2.0", false)?;
        assert_ne!(first.graph().units[0].identity, second.graph().units[0].identity);
        assert_eq!(only_identity(&first)?, only_identity(&second)?);
        Ok(())
    }

    #[test]
    fn observed_package_version_remains_a_compilation_input() -> Result<(), Box<dyn std::error::Error>> {
        let first = graph("0.1.0", true)?;
        let second = graph("0.2.0", true)?;
        assert_ne!(only_identity(&first)?, only_identity(&second)?);
        Ok(())
    }

    #[test]
    fn parent_reuses_when_only_a_child_package_coordinate_changes() -> Result<(), Box<dyn std::error::Error>> {
        let first = graph_with_dependency("0.1.0")?;
        let second = graph_with_dependency("0.2.0")?;
        let first_parent = first
            .graph()
            .units
            .iter()
            .find(|unit| unit.crate_name == "fixture_parent")
            .ok_or("first fixture lost parent")?;
        let second_parent = second
            .graph()
            .units
            .iter()
            .find(|unit| unit.crate_name == "fixture_parent")
            .ok_or("second fixture lost parent")?;
        assert_ne!(first_parent.identity, second_parent.identity);
        let first_compiled = compiled_rust_unit_identities(&first, COMPILER_CLOSURE)?;
        let second_compiled = compiled_rust_unit_identities(&second, COMPILER_CLOSURE)?;
        assert_eq!(
            first_compiled.get(&first_parent.identity),
            second_compiled.get(&second_parent.identity)
        );
        Ok(())
    }

    #[test]
    fn source_owner_relocation_does_not_change_compilation_identity() -> Result<(), Box<dyn std::error::Error>> {
        let first = graph_with_owner("0.1.0", false, selected_graph_sha256(b"alice checkout"))?;
        let second = graph_with_owner("0.1.0", false, selected_graph_sha256(b"bob checkout"))?;
        assert_ne!(first.graph().units[0].identity, second.graph().units[0].identity);
        assert_eq!(only_identity(&first)?, only_identity(&second)?);
        Ok(())
    }

    #[test]
    fn malformed_compiler_closure_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let selected = graph("0.1.0", false)?;
        assert!(matches!(
            compiled_rust_unit_identities(&selected, "not-a-digest"),
            Err(OvenRustcError::InvalidInput {
                field: "compiled Rust unit compiler closure",
                ..
            })
        ));
        Ok(())
    }
}
