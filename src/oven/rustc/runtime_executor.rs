//! Direct-Rustc execution of the compiler-owned layer above a sealed runtime foundation.
//!
//! This module is the physical connector between the runtime-foundation authority and real `rustc`. It consumes only
//! [`OvenMaterializedRuntimeFoundation`], which has already verified every source tree and sealed artifact byte, and
//! turns its declared rebuild order into compiler launches. It never constructs an `OvenReceipt`, reads Cargo
//! metadata, resolves a package, scans a target directory, or infers a dependency edge from a filename: every
//! compiler input is either an admitted source fact, a verified prebuilt `--extern` path, or an output this executor
//! itself produced earlier in the same declared order.
//!
//! The boundary is deliberately narrow. The foundation decides *what* is compiled and in which order; this module
//! decides only *how* one already-selected unit is handed to a retained compiler.

#![allow(
    dead_code,
    reason = "runtime closure publication consumes these outputs in the next gate of the Cargo-free publisher"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{
    OvenCompiledRustUnitIdentity, OvenMaterializedRuntimeFoundation, OvenMaterializedRustFacetEnvironmentValue,
    OvenRuntimeFoundationUnitExecution, OvenRustcError, OvenSelectedRustFacetCrateKind, OvenSelectedRustFacetDomain,
    OvenSelectedRustFacetUnit, OvenSelectedRustFacetUnitRole, ValidatedOvenRuntimeFoundation, apply_oven_profile,
    clear_inherited_cargo_environment, digest_regular_file, parse_rustc_diagnostics, verified_regular_file,
};

/// The explicit retained compiler that owns every output this executor produces.
///
/// Both fields are supplied by the caller from a retained closure lease. This type deliberately has no discovery
/// constructor: an executor that could resolve `rustc` from `PATH` would silently break the identity contract, since
/// the foundation's compiled-unit identities are derived from one exact compiler closure digest.
#[derive(Debug, Clone)]
pub(crate) struct OvenRuntimeCompilerClosure {
    rustc: PathBuf,
    identity: String,
}

impl OvenRuntimeCompilerClosure {
    /// Bind one retained compiler binary to the closure identity its lease recorded.
    pub(crate) fn new(rustc: impl Into<PathBuf>, identity: impl Into<String>) -> Self {
        Self {
            rustc: rustc.into(),
            identity: identity.into(),
        }
    }

    /// Return the retained compiler binary this closure executes.
    pub(crate) fn rustc(&self) -> &Path {
        &self.rustc
    }

    /// Return the content identity that must match the foundation's declared compiler closure.
    pub(crate) fn identity(&self) -> &str {
        &self.identity
    }
}

/// How one direct dependency reached a rebuilt unit's compiler invocation.
///
/// The distinction is provenance, not mechanics: both kinds are passed as `--extern`, but a prebuilt edge is owned by
/// the sealed foundation while a rebuilt edge is owned by this executor's own earlier output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OvenRuntimeRebuildDependencyKind {
    /// A sealed third-party artifact verified during foundation materialization.
    Prebuilt,
    /// An output this executor produced earlier in the declared rebuild order.
    Rebuilt,
}

/// One direct compiler input recorded for a rebuilt unit's provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenRuntimeRebuildDependency {
    /// Rust-facing alias passed to `--extern`, taken from the selected graph rather than a filename.
    pub(crate) alias: String,
    /// Prebuilt artifact digest, or the compiled-unit identity of an earlier rebuild output.
    pub(crate) identity: String,
    /// Whether the foundation or this executor owns the bytes behind the alias.
    pub(crate) kind: OvenRuntimeRebuildDependencyKind,
}

/// One compiler-owned library this executor produced above the sealed foundation.
///
/// `compiled_identity` is the reuse key: it already excludes package coordinates and cache paths, so a published
/// closure keyed on it stays byte-stable across a coordinate-only change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenRuntimeRebuildOutput {
    /// Selected-source identity naming the unit in the foundation graph.
    pub(crate) selected_identity: String,
    /// Compiler-input identity used as the output's reuse key and published name.
    pub(crate) compiled_identity: OvenCompiledRustUnitIdentity,
    /// Rust crate name the unit was compiled under.
    pub(crate) crate_name: String,
    /// Explicit host or target domain the foundation declared for this unit.
    pub(crate) domain: OvenSelectedRustFacetDomain,
    /// Absolute path of the produced library below the caller-supplied output root.
    pub(crate) artifact: PathBuf,
    /// Digest of the exact produced bytes.
    pub(crate) digest: String,
    /// Sorted direct compiler inputs, both foundation-owned and executor-owned.
    pub(crate) dependencies: Vec<OvenRuntimeRebuildDependency>,
}

/// The complete result of rebuilding one foundation's compiler-owned layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenRuntimeFoundationBuild {
    outputs: Vec<OvenRuntimeRebuildOutput>,
    compiler_launches: usize,
}

impl OvenRuntimeFoundationBuild {
    /// Iterate produced outputs in the foundation's declared rebuild order.
    pub(crate) fn outputs(&self) -> &[OvenRuntimeRebuildOutput] {
        &self.outputs
    }

    /// Return how many times this executor actually started the retained compiler.
    ///
    /// The hot-path reuse proof reads this directly: a coordinate-only change that reuses every unit must report
    /// zero launches, and no other counter in the pipeline observes real process starts.
    pub(crate) fn compiler_launches(&self) -> usize {
        self.compiler_launches
    }

    /// Look up one produced output by its selected-source identity.
    pub(crate) fn output(&self, selected_identity: &str) -> Option<&OvenRuntimeRebuildOutput> {
        self.outputs
            .iter()
            .find(|output| output.selected_identity == selected_identity)
    }
}

/// Compile every rebuildable unit of one materialized foundation, in its declared dependency-safe order.
///
/// The caller retains the foundation's source and artifact leases for the duration of this call and owns
/// `output_root`. Each unit is compiled exactly once; an output already present under its compiled identity is
/// reused without starting the compiler, which is what makes the reuse proof observable.
pub(crate) fn execute_runtime_foundation_rebuild(
    foundation: &ValidatedOvenRuntimeFoundation,
    materialized: &OvenMaterializedRuntimeFoundation,
    closure: &OvenRuntimeCompilerClosure,
    output_root: &Path,
) -> Result<OvenRuntimeFoundationBuild, OvenRustcError> {
    // ---- Admission: the executing compiler must be the one the identities were derived from ----
    if closure.identity() != foundation.compiler_closure_digest() {
        return Err(runtime_executor_invalid(
            "runtime executor compiler closure",
            format!(
                "retained closure {} does not own foundation closure {}",
                closure.identity(),
                foundation.compiler_closure_digest()
            ),
        ));
    }
    verified_regular_file(closure.rustc(), "retained direct compiler")?;

    let graph = foundation.selected_graph().graph();
    let selection = &graph.selection;
    // Cross-target rebuilds need an explicit target-spec hand-off and a distinct host/target artifact split. That is
    // a separate proof; refusing here keeps the first connector honest instead of silently emitting host artifacts.
    if selection.intent.target != selection.host {
        return Err(runtime_executor_invalid(
            "runtime executor compilation domain",
            format!(
                "cross-target rebuild from host {} to target {} is not supported by this connector",
                selection.host, selection.intent.target
            ),
        ));
    }

    let mut outputs: Vec<OvenRuntimeRebuildOutput> = Vec::new();
    let mut rebuilt: BTreeMap<String, (PathBuf, String)> = BTreeMap::new();
    let mut compiler_launches = 0usize;

    for selected_identity in materialized.rebuild_order() {
        // ---- Bind the unit's selected facts and confirm it is genuinely rebuildable ----
        let unit = graph
            .units
            .iter()
            .find(|unit| unit.identity == selected_identity)
            .ok_or_else(|| {
                runtime_executor_invalid(
                    "runtime executor rebuild unit",
                    format!("rebuild order names absent selected unit {selected_identity}"),
                )
            })?;
        let policy = foundation.unit_policy(selected_identity).ok_or_else(|| {
            runtime_executor_invalid(
                "runtime executor rebuild policy",
                format!("selected unit {selected_identity} has no execution policy"),
            )
        })?;
        if !matches!(policy.execution, OvenRuntimeFoundationUnitExecution::Rebuild) {
            return Err(runtime_executor_invalid(
                "runtime executor rebuild policy",
                format!("selected unit {selected_identity} is prebuilt and must not be recompiled"),
            ));
        }
        refuse_unsupported_rebuild_shape(unit)?;
        if policy.domain != unit.domain {
            return Err(runtime_executor_invalid(
                "runtime executor rebuild policy",
                format!("selected unit {selected_identity} declares a domain its policy does not share"),
            ));
        }

        let source = materialized.sources().unit(selected_identity).ok_or_else(|| {
            runtime_executor_invalid(
                "runtime executor rebuild source",
                format!("selected unit {selected_identity} was not materialized"),
            )
        })?;

        // ---- Resolve every direct compiler input from admitted edges only ----
        let prebuilt = materialized.prebuilt_dependencies(selected_identity).unwrap_or(&[]);
        let mut externs: Vec<(String, PathBuf)> = Vec::new();
        let mut dependencies: Vec<OvenRuntimeRebuildDependency> = Vec::new();
        for dependency in prebuilt {
            externs.push((dependency.alias.clone(), dependency.artifact.clone()));
            dependencies.push(OvenRuntimeRebuildDependency {
                alias: dependency.alias.clone(),
                identity: dependency.digest.clone(),
                kind: OvenRuntimeRebuildDependencyKind::Prebuilt,
            });
        }
        for edge in &unit.dependencies {
            let child = foundation.unit_policy(&edge.unit).ok_or_else(|| {
                runtime_executor_invalid(
                    "runtime executor rebuild dependency",
                    format!("selected child {} has no execution policy", edge.unit),
                )
            })?;
            if !matches!(child.execution, OvenRuntimeFoundationUnitExecution::Rebuild) {
                continue;
            }
            let (artifact, identity) = rebuilt.get(&edge.unit).ok_or_else(|| {
                runtime_executor_invalid(
                    "runtime executor rebuild dependency",
                    format!(
                        "unit {selected_identity} consumes rebuild child {} before its declared order produced it",
                        edge.unit
                    ),
                )
            })?;
            externs.push((edge.alias.clone(), artifact.clone()));
            dependencies.push(OvenRuntimeRebuildDependency {
                alias: edge.alias.clone(),
                identity: identity.clone(),
                kind: OvenRuntimeRebuildDependencyKind::Rebuilt,
            });
        }
        dependencies.sort_by(|left, right| left.alias.cmp(&right.alias));

        // ---- Compile into a directory this run owns ----
        //
        // The executor used to skip compilation whenever `lib<crate>.rlib` already existed here. That directory is
        // caller-owned scratch, so the check trusted whatever was in it -- a partially written file from an
        // interrupted run, a stale artifact from a compiler that has since changed, or bytes a caller placed
        // deliberately -- and published its digest as this identity's output. None of those is evidence that this
        // identity was compiled. Deciding that two compilations are interchangeable is the Store's decision, made
        // on identity; this executor's job is to produce the bytes. So the unit directory is emptied first, the
        // compiler always runs, and `compiler_launches` counts real launches rather than scratch misses.
        let compiled_identity = source.compiled_identity.clone();
        let unit_root = output_root.join(compiled_identity.as_str().replace(':', "-"));
        let artifact = unit_root.join(format!("lib{}.rlib", unit.crate_name));
        if unit_root.exists() {
            fs::remove_dir_all(&unit_root).map_err(|source_error| OvenRustcError::Io {
                path: unit_root.clone(),
                source: source_error,
            })?;
        }
        fs::create_dir_all(&unit_root).map_err(|source_error| OvenRustcError::Io {
            path: unit_root.clone(),
            source: source_error,
        })?;
        let search_paths = transitive_rebuild_search_paths(graph, foundation, &rebuilt, unit)?;
        compile_rebuild_unit(
            closure,
            unit,
            source,
            selection,
            materialized.artifact_plan(),
            &search_paths,
            &externs,
            output_root,
            &artifact,
        )?;
        compiler_launches += 1;
        let digest = digest_regular_file(&artifact, "runtime rebuild output")?;
        rebuilt.insert(
            selected_identity.to_string(),
            (artifact.clone(), compiled_identity.as_str().to_string()),
        );
        outputs.push(OvenRuntimeRebuildOutput {
            selected_identity: selected_identity.to_string(),
            compiled_identity,
            crate_name: unit.crate_name.clone(),
            domain: unit.domain,
            artifact,
            digest,
            dependencies,
        });
    }

    Ok(OvenRuntimeFoundationBuild {
        outputs,
        compiler_launches,
    })
}

/// Refuse every selected shape this first connector cannot compile, naming the exact unsupported fact.
///
/// Silence here would be the dangerous outcome: a benchmark or integration-test role compiled as a plain library
/// produces an artifact that links but does not mean what its role claims. Proc-macro and binary roles are deferred
/// to their own proofs rather than approximated.
fn refuse_unsupported_rebuild_shape(unit: &OvenSelectedRustFacetUnit) -> Result<(), OvenRustcError> {
    if unit.domain != OvenSelectedRustFacetDomain::Target {
        return Err(runtime_executor_invalid(
            "runtime executor rebuild domain",
            format!(
                "unit {} is a host-domain rebuild; host units are a separate proc-macro proof",
                unit.crate_name
            ),
        ));
    }
    if unit.crate_kind != OvenSelectedRustFacetCrateKind::Rlib {
        return Err(runtime_executor_invalid(
            "runtime executor rebuild crate kind",
            format!(
                "unit {} selects crate kind {:?}; this connector rebuilds only rlib libraries",
                unit.crate_name, unit.crate_kind
            ),
        ));
    }
    if unit.role != OvenSelectedRustFacetUnitRole::Library {
        return Err(runtime_executor_invalid(
            "runtime executor rebuild role",
            format!(
                "unit {} selects role {:?}; this connector rebuilds only ordinary libraries",
                unit.crate_name, unit.role
            ),
        ));
    }
    Ok(())
}

/// Collect every directory a rebuilt unit needs on its `-L dependency` path, beyond the sealed foundation's own.
///
/// Rustc resolves a dependency's *own* dependencies while loading its metadata, so a direct `--extern` is not enough:
/// the whole reachable closure must be findable. The walk follows selected graph edges only. Prebuilt transitives are
/// deliberately absent because the sealed artifact plan already names the one directory that holds them.
fn transitive_rebuild_search_paths(
    graph: &super::OvenSelectedRustFacetGraph,
    foundation: &ValidatedOvenRuntimeFoundation,
    rebuilt: &BTreeMap<String, (PathBuf, String)>,
    root: &OvenSelectedRustFacetUnit,
) -> Result<BTreeSet<PathBuf>, OvenRustcError> {
    let mut search_paths = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut pending = root
        .dependencies
        .iter()
        .map(|edge| edge.unit.clone())
        .collect::<Vec<_>>();
    while let Some(identity) = pending.pop() {
        if !visited.insert(identity.clone()) {
            continue;
        }
        let policy = foundation.unit_policy(&identity).ok_or_else(|| {
            runtime_executor_invalid(
                "runtime executor rebuild dependency",
                format!("selected child {identity} has no execution policy"),
            )
        })?;
        if matches!(policy.execution, OvenRuntimeFoundationUnitExecution::Rebuild) {
            let (artifact, _) = rebuilt.get(&identity).ok_or_else(|| {
                runtime_executor_invalid(
                    "runtime executor rebuild dependency",
                    format!(
                        "unit {} reaches rebuild child {identity} before its declared order produced it",
                        root.identity
                    ),
                )
            })?;
            if let Some(parent) = artifact.parent() {
                search_paths.insert(parent.to_path_buf());
            }
        }
        let child = graph
            .units
            .iter()
            .find(|unit| unit.identity == identity)
            .ok_or_else(|| {
                runtime_executor_invalid(
                    "runtime executor rebuild dependency",
                    format!("selected child {identity} is absent from the foundation graph"),
                )
            })?;
        pending.extend(child.dependencies.iter().map(|edge| edge.unit.clone()));
    }
    Ok(search_paths)
}

/// Launch the retained compiler once for one admitted rebuild unit.
///
/// Every argument comes from an already-admitted fact: the sealed artifact plan supplies the foundation's own search
/// paths and compiler environment, the selected graph supplies features and cfgs, and `search_paths`/`externs` were
/// derived from graph edges. Nothing here lists a directory to discover an input.
///
/// Each argument is a distinct admitted authority, so `clippy::too_many_arguments` is allowed here: bundling them
/// into one struct would hide which authority supplied a given flag, which is the thing this signature records.
#[allow(clippy::too_many_arguments)]
fn compile_rebuild_unit(
    closure: &OvenRuntimeCompilerClosure,
    unit: &OvenSelectedRustFacetUnit,
    source: &super::OvenMaterializedRustFacetUnit,
    selection: &super::OvenSelectedRustFacetSelection,
    plan: &super::OvenRustcArtifactPlan,
    search_paths: &BTreeSet<PathBuf>,
    externs: &[(String, PathBuf)],
    output_root: &Path,
    artifact: &Path,
) -> Result<(), OvenRustcError> {
    let mut command = Command::new(closure.rustc());
    command
        .args(["--crate-type", "lib"])
        .arg("--target")
        .arg(&selection.intent.target)
        .arg(format!("--edition={}", unit.edition))
        .arg("--crate-name")
        .arg(&unit.crate_name)
        .arg("--error-format=json")
        .arg(&source.root_module)
        .arg("-o")
        .arg(artifact);
    apply_oven_profile(&mut command, &selection.intent.profile);
    // ---- Deterministic path remapping for reproducible unit bytes ----
    //
    // `rustc` folds absolute paths into what it emits, so the same unit compiled from two scratch locations
    // produces different bytes and therefore a different closure identity. That is not hypothetical here: with the
    // executor no longer reusing whatever sat in its output directory, the coordinate-only reuse proof only holds
    // because these remaps make a cold root reproduce the original bytes. The legacy-Cargo publisher already
    // applies the same discipline for the same reason; this is the direct route's half of it, and the two must not
    // drift, which is why the rustc source remap calls the one helper rather than restating the form.
    command.arg(format!(
        "--remap-path-prefix={}=/incan/source",
        source.source_root.display()
    ));
    command.arg(format!("--remap-path-prefix={}=/incan/target", output_root.display()));
    // Standard-library spans leak through inlined core and alloc generics. A toolchain carrying `rust-src`
    // resolves them to its real checkout while one without emits the virtual `/rustc/<commit>` form, so the same
    // unit compiles differently depending on which components are installed. On a toolchain without the component
    // the prefix never matches and the flag is inert.
    if let Some(toolchain_root) = closure.rustc().parent().and_then(Path::parent)
        && let Some(commit) = crate::oven::legacy_cargo::rustc_commit_hash(closure.rustc())
    {
        command.arg(format!(
            "--remap-path-prefix={}=/rustc/{commit}",
            toolchain_root.join("lib/rustlib/src/rust").display()
        ));
    }
    clear_inherited_cargo_environment(&mut command);
    for (name, value) in &plan.compile_environment {
        command.env(name, value);
    }
    for (name, value) in &source.environment {
        match value {
            OvenMaterializedRustFacetEnvironmentValue::Text(text) => command.env(name, text),
            OvenMaterializedRustFacetEnvironmentValue::Path(path) => command.env(name, path),
        };
    }
    for feature in &unit.features {
        command.arg("--cfg").arg(format!("feature={feature:?}"));
    }
    for cfg in &unit.cfg {
        command.arg("--cfg").arg(cfg);
    }
    for path in &plan.dependency_search_paths {
        command.arg("-L").arg(format!("dependency={}", path.display()));
    }
    for path in search_paths {
        command.arg("-L").arg(format!("dependency={}", path.display()));
    }
    for path in &plan.native_search_paths {
        command.arg("-L").arg(format!("native={}", path.display()));
    }
    for (alias, path) in externs {
        command.arg("--extern").arg(format!("{alias}={}", path.display()));
    }
    let result = command.output().map_err(|source_error| OvenRustcError::Io {
        path: closure.rustc().to_path_buf(),
        source: source_error,
    })?;
    if !result.status.success() {
        return Err(OvenRustcError::CompilationFailed {
            report: parse_rustc_diagnostics(&result.stdout, &result.stderr).with_invocation(&command),
        });
    }
    verified_regular_file(artifact, "runtime rebuild output")?;
    Ok(())
}

/// Build one refusal that names the executor field responsible.
fn runtime_executor_invalid(field: &'static str, message: impl Into<String>) -> OvenRustcError {
    OvenRustcError::InvalidInput {
        field,
        message: message.into(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::oven::OvenBuildIntent;
    use crate::oven::rustc::{
        OVEN_RUNTIME_FOUNDATION_SCHEMA_VERSION, OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION, OvenRuntimeFoundation, OvenRuntimeFoundationUnit,
        OvenRustcArtifactExtern, OvenRustcArtifactManifest, OvenRustcRegistryLeaf, OvenRustcRegistrySource,
        OvenRustcRegistrySourcePackage, OvenRustcSupportingArtifact, OvenSelectedRustFacetDependency,
        OvenSelectedRustFacetGraph, OvenSelectedRustFacetIntent, OvenSelectedRustFacetOwner,
        OvenSelectedRustFacetOwnerKind, OvenSelectedRustFacetOwnerRoot, OvenSelectedRustFacetPath,
        OvenSelectedRustFacetPurpose, OvenSelectedRustFacetSelection, OvenSelectedRustFacetSource,
        OvenSelectedRustFacetSourceKind, OvenSelectedRustFacetSourceMember, OvenSelectedRustFacetTargetSpec,
        resolve_active_rustc, rustc_host_target, rustc_identity, selected_graph_sha256, selected_graph_source_digest,
        selected_graph_unit_identity,
    };

    /// Compiler closure identity the fixture binds to the retained host compiler.
    pub(crate) const FIXTURE_CLOSURE: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    /// Owner-relative root of the sealed third-party source tree.
    const DEP_SOURCE_ROOT: &str = "registry-sources/fixture_dep-1.0.0";
    /// Owner-relative path of the sealed third-party artifact.
    const DEP_ARTIFACT: &str = "deps/libfixture_dep.rlib";

    /// Owner identity of the sealed third-party foundation root.
    fn foundation_owner() -> String {
        selected_graph_sha256(b"runtime executor foundation owner")
    }

    /// Owner identity of the compiler-owned source and target-fact root.
    fn toolchain_owner() -> String {
        selected_graph_sha256(b"runtime executor toolchain owner")
    }

    /// Return the sealed third-party crate's manifest bytes.
    fn dep_cargo_toml() -> String {
        "[package]\nname = \"fixture_dep\"\nversion = \"1.0.0\"\n".to_string()
    }

    /// Return the real Rust source compiled for one fixture crate.
    ///
    /// The call chain is the proof. `incan_core` calls into the sealed prebuilt crate and `incan_stdlib` calls into
    /// `incan_core`, so neither compiles unless the executor passed the right `--extern` for both an SDK-owned
    /// artifact and an output it produced itself earlier in the declared order.
    fn fixture_source(crate_name: &str) -> String {
        match crate_name {
            "fixture_dep" => "pub fn dep_marker() -> u8 {\n    3\n}\n".to_string(),
            "incan_core" => "pub fn core_marker() -> u8 {\n    fixture_dep::dep_marker() + 4\n}\n".to_string(),
            _ => "pub fn stdlib_marker() -> u8 {\n    incan_core::core_marker()\n}\n".to_string(),
        }
    }

    /// Write one fixture file below a retained owner root.
    fn write_fixture_file(root: &Path, relative: &str, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        let path = root.join(relative);
        let parent = path.parent().ok_or("fixture path has no parent")?;
        fs::create_dir_all(parent)?;
        fs::write(path, bytes)?;
        Ok(())
    }

    /// Build one selected library unit bound to its exact owner-relative source root.
    ///
    /// `clippy::too_many_arguments` is allowed because the fixture mirrors the selected unit's own distinct facts
    /// one for one; bundling them would stop the fixture reading like the thing it stands in for.
    #[allow(clippy::too_many_arguments)]
    fn library_unit(
        selection: &OvenSelectedRustFacetSelection,
        crate_name: &str,
        package_version: &str,
        source_kind: OvenSelectedRustFacetSourceKind,
        owner: &str,
        source_root: &str,
        dependencies: Vec<OvenSelectedRustFacetDependency>,
    ) -> Result<OvenSelectedRustFacetUnit, Box<dyn std::error::Error>> {
        let mut members = Vec::new();
        if source_kind == OvenSelectedRustFacetSourceKind::Registry {
            members.push(OvenSelectedRustFacetSourceMember {
                path: "Cargo.toml".to_string(),
                digest: selected_graph_sha256(dep_cargo_toml().as_bytes()),
            });
        }
        members.push(OvenSelectedRustFacetSourceMember {
            path: "src/lib.rs".to_string(),
            digest: selected_graph_sha256(fixture_source(crate_name).as_bytes()),
        });
        let source_identity = match source_kind {
            OvenSelectedRustFacetSourceKind::Registry => format!("registry:{crate_name}@{package_version}"),
            _ => selected_graph_sha256(crate_name.as_bytes()),
        };
        let mut unit = OvenSelectedRustFacetUnit {
            identity: String::new(),
            package: crate_name.to_string(),
            package_version: package_version.to_string(),
            crate_name: crate_name.to_string(),
            crate_kind: OvenSelectedRustFacetCrateKind::Rlib,
            role: OvenSelectedRustFacetUnitRole::Library,
            domain: OvenSelectedRustFacetDomain::Target,
            edition: "2024".to_string(),
            source: OvenSelectedRustFacetSource {
                kind: source_kind,
                identity: source_identity,
                owner: owner.to_string(),
                root: source_root.to_string(),
                digest: selected_graph_source_digest(&members)?,
            },
            root_module: "src/lib.rs".to_string(),
            source_members: members,
            features: Vec::new(),
            default_features: false,
            cfg: Vec::new(),
            environment: BTreeMap::new(),
            include_dirs: vec![OvenSelectedRustFacetPath {
                owner: owner.to_string(),
                path: source_root.to_string(),
            }],
            exclude_dirs: Vec::new(),
            dependencies,
            generated_inputs: Vec::new(),
        };
        unit.identity = selected_graph_unit_identity(selection, &unit)?;
        Ok(unit)
    }

    /// Populate both retained roots and compile the sealed third-party artifact with the same retained compiler.
    ///
    /// The prebuilt edge has to be a real rlib produced by this exact compiler; a placeholder file would prove
    /// nothing, because rustc would reject it before the dependency wiring was ever exercised.
    fn write_fixture_roots(
        rustc: &Path,
        foundation_root: &Path,
        toolchain_root: &Path,
    ) -> Result<String, Box<dyn std::error::Error>> {
        write_fixture_file(toolchain_root, "target-spec.json", b"{}")?;
        for crate_name in ["incan_core", "incan_stdlib"] {
            write_fixture_file(
                toolchain_root,
                &format!("compiler/{crate_name}/src/lib.rs"),
                fixture_source(crate_name).as_bytes(),
            )?;
        }
        write_fixture_file(
            foundation_root,
            &format!("{DEP_SOURCE_ROOT}/Cargo.toml"),
            dep_cargo_toml().as_bytes(),
        )?;
        write_fixture_file(
            foundation_root,
            &format!("{DEP_SOURCE_ROOT}/src/lib.rs"),
            fixture_source("fixture_dep").as_bytes(),
        )?;
        let artifact = foundation_root.join(DEP_ARTIFACT);
        let parent = artifact.parent().ok_or("artifact path has no parent")?;
        fs::create_dir_all(parent)?;
        // Remap the source prefix so the sealed artifact's bytes do not depend on this fixture's temporary root.
        // Without it two runs of the same source produce different rlibs, and a reuse assertion would be measuring
        // the temporary directory rather than the compiler inputs.
        let status = Command::new(rustc)
            .args(["--crate-type", "lib", "--crate-name", "fixture_dep", "--edition=2024"])
            .arg("-C")
            .arg("opt-level=0")
            .arg(format!(
                "--remap-path-prefix={}=/incan-runtime-fixture",
                foundation_root.display()
            ))
            .arg(foundation_root.join(format!("{DEP_SOURCE_ROOT}/src/lib.rs")))
            .arg("-o")
            .arg(&artifact)
            .status()?;
        if !status.success() {
            return Err("could not compile the sealed fixture dependency".into());
        }
        Ok(selected_graph_sha256(&fs::read(&artifact)?))
    }

    /// Assemble the host-native foundation this connector's first proof compiles.
    fn host_native_foundation(
        host: &str,
        toolchain: &str,
        dep_artifact_digest: &str,
        package_version: &str,
    ) -> Result<OvenRuntimeFoundation, Box<dyn std::error::Error>> {
        let selection = OvenSelectedRustFacetSelection {
            intent: OvenSelectedRustFacetIntent {
                target: host.to_string(),
                toolchain: toolchain.to_string(),
                profile: "debug".to_string(),
                features: Vec::new(),
            },
            host: host.to_string(),
            purpose: OvenSelectedRustFacetPurpose::Normal,
            default_features: true,
            toolchain_version: "1.85.0".to_string(),
            target_spec: OvenSelectedRustFacetTargetSpec {
                source: OvenSelectedRustFacetPath {
                    owner: toolchain_owner(),
                    path: "target-spec.json".to_string(),
                },
                digest: selected_graph_sha256(b"{}"),
            },
        };
        let dep = library_unit(
            &selection,
            "fixture_dep",
            package_version,
            OvenSelectedRustFacetSourceKind::Registry,
            &foundation_owner(),
            DEP_SOURCE_ROOT,
            Vec::new(),
        )?;
        let core = library_unit(
            &selection,
            "incan_core",
            package_version,
            OvenSelectedRustFacetSourceKind::Compiler,
            &toolchain_owner(),
            "compiler/incan_core",
            vec![OvenSelectedRustFacetDependency {
                alias: "fixture_dep".to_string(),
                unit: dep.identity.clone(),
            }],
        )?;
        let stdlib = library_unit(
            &selection,
            "incan_stdlib",
            package_version,
            OvenSelectedRustFacetSourceKind::Compiler,
            &toolchain_owner(),
            "compiler/incan_stdlib",
            vec![OvenSelectedRustFacetDependency {
                alias: "incan_core".to_string(),
                unit: core.identity.clone(),
            }],
        )?;
        let dep_source = OvenRustcRegistrySource {
            registry: "registry+https://example.invalid/index".to_string(),
            checksum: "fixture-dep-checksum".to_string(),
            relative_root: DEP_SOURCE_ROOT.to_string(),
            digest: dep.source.digest.clone(),
        };
        let dep_extern = OvenRustcArtifactExtern {
            crate_name: "fixture_dep".to_string(),
            relative_path: DEP_ARTIFACT.to_string(),
            digest: dep_artifact_digest.to_string(),
        };
        let artifacts = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: OvenBuildIntent {
                target: selection.intent.target.clone(),
                toolchain: selection.intent.toolchain.clone(),
                profile: selection.intent.profile.clone(),
                features: selection.intent.features.clone(),
            },
            dependency_search_paths: vec!["deps".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![dep_extern.clone()],
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: vec![OvenRustcRegistryLeaf {
                package: "fixture_dep".to_string(),
                version: package_version.to_string(),
                crate_name: "fixture_dep".to_string(),
                features: Vec::new(),
                source: dep_source.clone(),
                artifact: dep_extern.clone(),
            }],
            registry_sources: vec![OvenRustcRegistrySourcePackage {
                package: "fixture_dep".to_string(),
                version: package_version.to_string(),
                features: Vec::new(),
                source: dep_source,
            }],
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: vec![OvenRustcSupportingArtifact {
                relative_path: format!("{DEP_SOURCE_ROOT}/Cargo.toml"),
                digest: selected_graph_sha256(dep_cargo_toml().as_bytes()),
            }],
            entrypoint_dependency_search_paths: Default::default(),
        };
        let units = vec![
            OvenRuntimeFoundationUnit {
                selected_identity: dep.identity.clone(),
                domain: dep.domain,
                execution: OvenRuntimeFoundationUnitExecution::Prebuilt { artifact: dep_extern },
            },
            OvenRuntimeFoundationUnit {
                selected_identity: core.identity.clone(),
                domain: core.domain,
                execution: OvenRuntimeFoundationUnitExecution::Rebuild,
            },
            OvenRuntimeFoundationUnit {
                selected_identity: stdlib.identity.clone(),
                domain: stdlib.domain,
                execution: OvenRuntimeFoundationUnitExecution::Rebuild,
            },
        ];
        Ok(OvenRuntimeFoundation {
            schema_version: OVEN_RUNTIME_FOUNDATION_SCHEMA_VERSION,
            compiler_closure_digest: FIXTURE_CLOSURE.to_string(),
            artifact_owner: foundation_owner(),
            artifacts,
            selected_graph: OvenSelectedRustFacetGraph {
                schema_version: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
                selection,
                owners: vec![
                    OvenSelectedRustFacetOwner {
                        identity: foundation_owner(),
                        kind: OvenSelectedRustFacetOwnerKind::Constituent,
                    },
                    OvenSelectedRustFacetOwner {
                        identity: toolchain_owner(),
                        kind: OvenSelectedRustFacetOwnerKind::Toolchain,
                    },
                ],
                units: vec![dep, core, stdlib.clone()],
                exposed_roots: BTreeMap::from([("incan_stdlib".to_string(), stdlib.identity)]),
            },
            units,
        })
    }

    /// One prepared fixture: retained roots, a validated foundation and its publication materialization.
    pub(crate) struct Fixture {
        pub(crate) foundation: ValidatedOvenRuntimeFoundation,
        pub(crate) materialized: OvenMaterializedRuntimeFoundation,
        pub(crate) rustc: PathBuf,
        _foundation_root: tempfile::TempDir,
        _toolchain_root: tempfile::TempDir,
    }

    /// Prepare the complete host-native fixture through the real materialization boundary.
    pub(crate) fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
        fixture_at_version("1.0.0")
    }

    /// Prepare the same fixture at one explicit package coordinate.
    ///
    /// Only the package version differs between calls. Every compiler input -- source bytes, features, cfgs, edition,
    /// target and toolchain -- is held constant, which is what makes a coordinate-only reuse claim meaningful.
    pub(crate) fn fixture_at_version(package_version: &str) -> Result<Fixture, Box<dyn std::error::Error>> {
        let rustc = resolve_active_rustc()?;
        let host = rustc_host_target(&rustc)?;
        let toolchain = rustc_identity(&rustc)?;
        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        let dep_artifact_digest = write_fixture_roots(&rustc, foundation_root.path(), toolchain_root.path())?;
        let owner_roots = vec![
            OvenSelectedRustFacetOwnerRoot {
                identity: foundation_owner(),
                root: foundation_root.path().to_path_buf(),
            },
            OvenSelectedRustFacetOwnerRoot {
                identity: toolchain_owner(),
                root: toolchain_root.path().to_path_buf(),
            },
        ];
        let foundation =
            host_native_foundation(&host, &toolchain, &dep_artifact_digest, package_version)?.validated()?;
        let materialized = foundation.materialize_for_publication(&owner_roots)?;
        Ok(Fixture {
            foundation,
            materialized,
            rustc,
            _foundation_root: foundation_root,
            _toolchain_root: toolchain_root,
        })
    }

    /// Real `rustc` compiles the compiler-owned layer in declared order above a sealed prebuilt edge, and a second
    /// execution reproduces it byte for byte.
    #[test]
    fn host_native_rebuild_compiles_incan_core_then_incan_stdlib() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture()?;
        let output_root = tempfile::tempdir()?;
        let closure = OvenRuntimeCompilerClosure::new(&fixture.rustc, FIXTURE_CLOSURE);

        let build = execute_runtime_foundation_rebuild(
            &fixture.foundation,
            &fixture.materialized,
            &closure,
            output_root.path(),
        )?;

        assert_eq!(
            build.compiler_launches(),
            2,
            "each rebuild unit starts the compiler once"
        );
        let produced = build
            .outputs()
            .iter()
            .map(|output| output.crate_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(produced, vec!["incan_core", "incan_stdlib"]);
        for output in build.outputs() {
            assert!(output.artifact.is_file(), "{} produced no rlib", output.crate_name);
            assert!(output.digest.starts_with("sha256:"));
            assert!(
                output.artifact.starts_with(output_root.path()),
                "output escaped the caller-owned root"
            );
        }

        let core = build
            .outputs()
            .iter()
            .find(|output| output.crate_name == "incan_core")
            .ok_or("build lost incan_core")?;
        let stdlib = build
            .outputs()
            .iter()
            .find(|output| output.crate_name == "incan_stdlib")
            .ok_or("build lost incan_stdlib")?;
        assert_eq!(
            core.dependencies
                .iter()
                .map(|dependency| (dependency.alias.as_str(), dependency.kind))
                .collect::<Vec<_>>(),
            vec![("fixture_dep", OvenRuntimeRebuildDependencyKind::Prebuilt)],
            "the compiler-owned root links the sealed SDK artifact"
        );
        assert_eq!(
            stdlib.dependencies,
            vec![OvenRuntimeRebuildDependency {
                alias: "incan_core".to_string(),
                identity: core.compiled_identity.as_str().to_string(),
                kind: OvenRuntimeRebuildDependencyKind::Rebuilt,
            }],
            "the linked edge is the earlier rebuild output, recorded by compiled identity"
        );

        // A second execution against the same inputs compiles again and lands byte-identical outputs. That pair is
        // the claim worth making. The earlier version asserted zero launches, which only showed that the scratch
        // directory had survived between runs -- a statement about the filesystem, not about identity, and true
        // even when the file there had never been compiled by this compiler. Determinism is what lets the Store
        // decide two compilations are interchangeable; producing the bytes is this executor's job either way.
        let rebuilt = execute_runtime_foundation_rebuild(
            &fixture.foundation,
            &fixture.materialized,
            &closure,
            output_root.path(),
        )?;
        assert_eq!(
            rebuilt.compiler_launches(),
            2,
            "a cold scratch executor compiles every unit it is asked for, every time"
        );
        assert_eq!(
            rebuilt.outputs(),
            build.outputs(),
            "the same inputs must produce the same identities and the same bytes"
        );
        Ok(())
    }

    /// A file already sitting at a unit's scratch output path is never mistaken for that unit's output.
    ///
    /// The executor used to skip compilation whenever `lib<crate>.rlib` existed under the unit's identity
    /// directory. That directory is caller-owned scratch, so the check trusted whatever was there: a partially
    /// written file from an interrupted run, a stale artifact from a compiler that has since changed, or bytes a
    /// caller placed deliberately. None of those is evidence that this identity was compiled, and treating them as
    /// evidence is what made the old two-to-zero assertion a statement about the filesystem rather than about
    /// identity. Reuse is the Store's decision; this executor's job is to compile.
    #[test]
    fn a_planted_scratch_artifact_is_never_reused() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture()?;
        let output_root = tempfile::tempdir()?;
        let closure = OvenRuntimeCompilerClosure::new(&fixture.rustc, FIXTURE_CLOSURE);

        let build = execute_runtime_foundation_rebuild(
            &fixture.foundation,
            &fixture.materialized,
            &closure,
            output_root.path(),
        )?;
        let core = build
            .outputs()
            .iter()
            .find(|output| output.crate_name == "incan_core")
            .ok_or("build lost incan_core")?
            .clone();

        // Bytes `rustc` did not produce, at the exact path the executor writes.
        fs::write(&core.artifact, b"not an rlib")?;
        let planted = digest_regular_file(&core.artifact, "planted scratch artifact")?;
        assert_ne!(planted, core.digest, "the fixture must actually have been overwritten");

        let rebuilt = execute_runtime_foundation_rebuild(
            &fixture.foundation,
            &fixture.materialized,
            &closure,
            output_root.path(),
        )?;
        let rebuilt_core = rebuilt
            .outputs()
            .iter()
            .find(|output| output.crate_name == "incan_core")
            .ok_or("rebuild lost incan_core")?;
        assert_eq!(
            rebuilt_core.digest, core.digest,
            "the planted bytes must be replaced by a real compilation of this identity"
        );
        assert_eq!(
            rebuilt.compiler_launches(),
            2,
            "a cold scratch executor compiles every unit it is asked for"
        );
        Ok(())
    }

    /// The same sources at two unrelated absolute roots compile to byte-identical outputs.
    ///
    /// This is the relocation claim the Store depends on. Reuse is decided by identity, and identity folds output
    /// digests, so a unit that compiles differently depending on where its checkout happens to sit can never be
    /// shared between two worktrees, two projects, or two machines — each would publish the same identity with
    /// different bytes. Nothing here compares paths; it compares what the compiler produced from them.
    ///
    /// `coordinate_only_change_selects_the_published_closure_without_recompiling` exercises the same property
    /// through the Store. This one states it directly, so a regression in the remapping names relocation rather
    /// than surfacing as a confusing selection miss two layers up.
    #[test]
    fn the_same_sources_at_different_roots_compile_identically() -> Result<(), Box<dyn std::error::Error>> {
        let first = fixture()?;
        let second = fixture()?;
        let first_root = tempfile::tempdir()?;
        let second_root = tempfile::tempdir()?;
        let first_build = execute_runtime_foundation_rebuild(
            &first.foundation,
            &first.materialized,
            &OvenRuntimeCompilerClosure::new(&first.rustc, FIXTURE_CLOSURE),
            first_root.path(),
        )?;
        let second_build = execute_runtime_foundation_rebuild(
            &second.foundation,
            &second.materialized,
            &OvenRuntimeCompilerClosure::new(&second.rustc, FIXTURE_CLOSURE),
            second_root.path(),
        )?;

        assert_eq!(first_build.compiler_launches(), 2);
        assert_eq!(second_build.compiler_launches(), 2, "the second root is genuinely cold");
        let digests = |build: &OvenRuntimeFoundationBuild| {
            build
                .outputs()
                .iter()
                .map(|output| (output.crate_name.clone(), output.digest.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            digests(&first_build),
            digests(&second_build),
            "identical sources at unrelated roots must produce identical bytes"
        );
        Ok(())
    }

    /// A retained closure that does not own the foundation's compiled identities is refused before any compile.
    #[test]
    fn foreign_compiler_closure_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture()?;
        let output_root = tempfile::tempdir()?;
        let foreign = OvenRuntimeCompilerClosure::new(
            &fixture.rustc,
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        );

        let refusal = execute_runtime_foundation_rebuild(
            &fixture.foundation,
            &fixture.materialized,
            &foreign,
            output_root.path(),
        );

        let Err(OvenRustcError::InvalidInput { field, .. }) = refusal else {
            return Err("a foreign compiler closure was accepted".into());
        };
        assert_eq!(field, "runtime executor compiler closure");
        assert_eq!(
            fs::read_dir(output_root.path())?.count(),
            0,
            "a refused closure must not leave outputs"
        );
        Ok(())
    }

    /// Unsupported selected roles, crate kinds and domains are named explicitly instead of approximated.
    #[test]
    fn unsupported_rebuild_shapes_are_refused_by_name() -> Result<(), Box<dyn std::error::Error>> {
        let rustc = resolve_active_rustc()?;
        let host = rustc_host_target(&rustc)?;
        let toolchain = rustc_identity(&rustc)?;
        let foundation = host_native_foundation(&host, &toolchain, &selected_graph_sha256(b"placeholder"), "1.0.0")?;
        let core = foundation
            .selected_graph
            .units
            .iter()
            .find(|unit| unit.crate_name == "incan_core")
            .ok_or("fixture lost incan_core")?;

        let mut host_unit = core.clone();
        host_unit.domain = OvenSelectedRustFacetDomain::Host;
        let Err(OvenRustcError::InvalidInput { field, .. }) = refuse_unsupported_rebuild_shape(&host_unit) else {
            return Err("a host-domain rebuild was accepted".into());
        };
        assert_eq!(field, "runtime executor rebuild domain");

        let mut macro_unit = core.clone();
        macro_unit.crate_kind = OvenSelectedRustFacetCrateKind::ProcMacro;
        let Err(OvenRustcError::InvalidInput { field, .. }) = refuse_unsupported_rebuild_shape(&macro_unit) else {
            return Err("a proc-macro crate kind was accepted".into());
        };
        assert_eq!(field, "runtime executor rebuild crate kind");

        let mut benchmark_unit = core.clone();
        benchmark_unit.role = OvenSelectedRustFacetUnitRole::Benchmark;
        let Err(OvenRustcError::InvalidInput { field, .. }) = refuse_unsupported_rebuild_shape(&benchmark_unit) else {
            return Err("a benchmark role was accepted".into());
        };
        assert_eq!(field, "runtime executor rebuild role");
        Ok(())
    }
}
