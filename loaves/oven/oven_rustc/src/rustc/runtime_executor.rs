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

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{
    OvenCompiledRustUnitIdentity, OvenMaterializedRuntimeFoundation,
    OvenMaterializedRuntimeFoundationPrebuiltDependency, OvenMaterializedRustFacetEnvironmentValue,
    OvenMaterializedRustFacetLinkedLibrary, OvenRuntimeFoundationUnitExecution, OvenRustcError,
    OvenSelectedRustFacetCompilerArgument, OvenSelectedRustFacetCrateKind, OvenSelectedRustFacetDomain,
    OvenSelectedRustFacetUnit, OvenSelectedRustFacetUnitRole, ValidatedOvenRuntimeFoundation, apply_oven_profile,
    digest_regular_file, parse_rustc_diagnostics, verified_regular_file,
};

/// The explicit retained compiler that owns every output this executor produces.
///
/// Both fields are supplied by the caller from a retained closure lease. This type deliberately has no discovery
/// constructor: an executor that could resolve `rustc` from `PATH` would silently break the identity contract, since
/// the foundation's compiled-unit identities are derived from one exact compiler closure digest.
#[derive(Debug, Clone)]
pub struct OvenRuntimeCompilerClosure {
    rustc: PathBuf,
    identity: String,
}

impl OvenRuntimeCompilerClosure {
    /// Bind one retained compiler binary to the closure identity its lease recorded.
    pub fn new(rustc: impl Into<PathBuf>, identity: impl Into<String>) -> Self {
        Self {
            rustc: rustc.into(),
            identity: identity.into(),
        }
    }

    /// Return the retained compiler binary this closure executes.
    pub fn rustc(&self) -> &Path {
        &self.rustc
    }

    /// Return the content identity that must match the foundation's declared compiler closure.
    pub fn identity(&self) -> &str {
        &self.identity
    }
}

/// How one direct dependency reached a rebuilt unit's compiler invocation.
///
/// The distinction is provenance, not mechanics: both kinds are passed as `--extern`, but a prebuilt edge is owned by
/// the sealed foundation while a rebuilt edge is owned by this executor's own earlier output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OvenRuntimeRebuildDependencyKind {
    /// A sealed third-party artifact verified during foundation materialization.
    Prebuilt,
    /// An output this executor produced earlier in the declared rebuild order.
    Rebuilt,
}

/// One direct compiler input recorded for a rebuilt unit's provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenRuntimeRebuildDependency {
    /// Rust-facing alias passed to `--extern`, taken from the selected graph rather than a filename.
    pub alias: String,
    /// Prebuilt artifact digest, or the compiled-unit identity of an earlier rebuild output.
    pub identity: String,
    /// Whether the foundation or this executor owns the bytes behind the alias.
    pub kind: OvenRuntimeRebuildDependencyKind,
}

/// One compiler-owned library this executor produced above the sealed foundation.
///
/// `compiled_identity` is the reuse key: it already excludes package coordinates and cache paths, so a published
/// closure keyed on it stays byte-stable across a coordinate-only change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenRuntimeRebuildOutput {
    /// Selected-source identity naming the unit in the foundation graph.
    pub selected_identity: String,
    /// Compiler-input identity used as the output's reuse key and published name.
    pub compiled_identity: OvenCompiledRustUnitIdentity,
    /// Rust crate name the unit was compiled under.
    pub crate_name: String,
    /// Explicit host or target domain the foundation declared for this unit.
    pub domain: OvenSelectedRustFacetDomain,
    /// Absolute path of the produced library below the caller-supplied output root.
    pub artifact: PathBuf,
    /// Digest of the exact produced bytes.
    pub digest: String,
    /// Sorted direct compiler inputs, both foundation-owned and executor-owned.
    pub dependencies: Vec<OvenRuntimeRebuildDependency>,
}

/// The complete result of rebuilding one foundation's compiler-owned layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenRuntimeFoundationBuild {
    outputs: Vec<OvenRuntimeRebuildOutput>,
    compiler_launches: usize,
}

impl OvenRuntimeFoundationBuild {
    /// A build that produced nothing, for tests of what a closure may claim about one.
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            outputs: Vec::new(),
            compiler_launches: 0,
        }
    }

    /// Iterate produced outputs in the foundation's declared rebuild order.
    pub fn outputs(&self) -> &[OvenRuntimeRebuildOutput] {
        &self.outputs
    }

    /// Return how many times this executor actually started the retained compiler.
    ///
    /// This counts real process starts and nothing else: every unit handed to the executor is compiled, so the
    /// number equals the units in the foundation. Reuse of an already-published output is the Store's decision on
    /// identity and never shows up here as a skipped launch.
    pub fn compiler_launches(&self) -> usize {
        self.compiler_launches
    }

    /// Look up one produced output by its selected-source identity.
    pub fn output(&self, selected_identity: &str) -> Option<&OvenRuntimeRebuildOutput> {
        self.outputs
            .iter()
            .find(|output| output.selected_identity == selected_identity)
    }
}

/// Compile every rebuildable unit of one materialized foundation, in its declared dependency-safe order.
///
/// The caller retains the foundation's source and artifact leases for the duration of this call and owns
/// `output_root`. Each unit is compiled exactly once and unconditionally: the executor empties the unit directory
/// and runs the compiler rather than trusting whatever scratch already holds, so `compiler_launches` reports real
/// process starts. Whether two compilations are interchangeable is decided by the Store on identity, not here.
pub fn execute_runtime_foundation_rebuild(
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
        let mut externs: Vec<(String, PathBuf, String)> = Vec::new();
        let mut dependencies: Vec<OvenRuntimeRebuildDependency> = Vec::new();
        for dependency in prebuilt {
            let child = graph
                .units
                .iter()
                .find(|unit| unit.identity == dependency.selected_identity)
                .ok_or_else(|| {
                    runtime_executor_invalid(
                        "runtime executor prebuilt dependency",
                        format!(
                            "selected child {} is absent from the graph",
                            dependency.selected_identity
                        ),
                    )
                })?;
            externs.push((
                dependency.alias.clone(),
                runtime_prebuilt_extern_artifact(unit, dependency)?,
                child.compiler_paths.output_directory.clone(),
            ));
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
            let child_unit = graph
                .units
                .iter()
                .find(|candidate| candidate.identity == edge.unit)
                .ok_or_else(|| {
                    runtime_executor_invalid(
                        "runtime executor rebuild dependency",
                        format!("selected child {} is absent from the graph", edge.unit),
                    )
                })?;
            externs.push((
                edge.alias.clone(),
                runtime_rebuilt_extern_artifact(unit, &edge.alias, artifact)?,
                child_unit.compiler_paths.output_directory.clone(),
            ));
            dependencies.push(OvenRuntimeRebuildDependency {
                alias: edge.alias.clone(),
                identity: identity.clone(),
                kind: OvenRuntimeRebuildDependencyKind::Rebuilt,
            });
        }
        let externs = order_runtime_externs(unit, externs)?;
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
        let artifact = unit_root.join(runtime_rebuild_artifact_name(unit, &selection.host)?);
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
        let dependency_artifacts = transitive_rebuild_artifacts(graph, foundation, &rebuilt, unit)?;
        let search_paths = if unit.crate_kind == OvenSelectedRustFacetCrateKind::ProcMacro {
            stage_proc_macro_dependency_artifacts(&unit_root, &dependency_artifacts)?;
            BTreeSet::from([unit_root.clone()])
        } else {
            dependency_artifacts
                .iter()
                .filter_map(|artifact| artifact.parent().map(Path::to_path_buf))
                .collect()
        };
        let compiler_target = match unit.domain {
            OvenSelectedRustFacetDomain::Host => std::ffi::OsStr::new(&selection.host),
            OvenSelectedRustFacetDomain::Target => materialized.sources().compiler_target(),
        };
        compile_rebuild_unit(
            closure,
            unit,
            source,
            selection,
            compiler_target,
            materialized.artifact_plan(),
            &search_paths,
            &externs,
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
    let supported = matches!(
        (unit.role, unit.crate_kind, unit.domain),
        (
            OvenSelectedRustFacetUnitRole::Library,
            OvenSelectedRustFacetCrateKind::Rlib,
            OvenSelectedRustFacetDomain::Host | OvenSelectedRustFacetDomain::Target
        ) | (
            OvenSelectedRustFacetUnitRole::ProcMacro,
            OvenSelectedRustFacetCrateKind::ProcMacro,
            OvenSelectedRustFacetDomain::Host
        )
    );
    if !supported {
        return Err(runtime_executor_invalid(
            "runtime executor rebuild shape",
            format!(
                "unit {} selects unsupported domain {:?}, role {:?}, crate kind {:?}",
                unit.crate_name, unit.domain, unit.role, unit.crate_kind
            ),
        ));
    }
    Ok(())
}

/// Name one direct-rustc publisher output using the platform form rustc emits for its crate kind.
fn runtime_rebuild_artifact_name(unit: &OvenSelectedRustFacetUnit, host: &str) -> Result<String, OvenRustcError> {
    let extra_filename = unit
        .compiler_arguments
        .iter()
        .find_map(|argument| match argument {
            OvenSelectedRustFacetCompilerArgument::Codegen { name, value } if name == "extra-filename" => {
                Some(value.as_str())
            }
            _ => None,
        })
        .ok_or_else(|| runtime_executor_invalid("runtime executor output", "unit has no captured extra filename"))?;
    let name = match unit.crate_kind {
        OvenSelectedRustFacetCrateKind::Rlib => format!("lib{}{extra_filename}.rlib", unit.crate_name),
        OvenSelectedRustFacetCrateKind::ProcMacro if host.contains("windows") => {
            format!("{}{extra_filename}.dll", unit.crate_name)
        }
        OvenSelectedRustFacetCrateKind::ProcMacro if host.contains("apple") => {
            format!("lib{}{extra_filename}.dylib", unit.crate_name)
        }
        OvenSelectedRustFacetCrateKind::ProcMacro => format!("lib{}{extra_filename}.so", unit.crate_name),
        _ => {
            return Err(runtime_executor_invalid(
                "runtime executor output",
                format!(
                    "unit {} has unsupported artifact kind {:?}",
                    unit.crate_name, unit.crate_kind
                ),
            ));
        }
    };
    Ok(name)
}

/// Collect every rebuilt artifact a unit needs through its transitive `-L dependency` closure.
///
/// Rustc resolves a dependency's *own* dependencies while loading its metadata, so a direct `--extern` is not enough:
/// the whole reachable closure must be findable. The walk follows selected graph edges only. Prebuilt transitives are
/// deliberately absent because the sealed artifact plan already names the one directory that holds them.
fn transitive_rebuild_artifacts(
    graph: &super::OvenSelectedRustFacetGraph,
    foundation: &ValidatedOvenRuntimeFoundation,
    rebuilt: &BTreeMap<String, (PathBuf, String)>,
    root: &OvenSelectedRustFacetUnit,
) -> Result<BTreeSet<PathBuf>, OvenRustcError> {
    let mut artifacts = BTreeSet::new();
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
            artifacts.insert(artifact.clone());
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
    Ok(artifacts)
}

/// Co-locate one proc macro's verified dependency closure with its output, matching Cargo's host layout.
///
/// Cargo places every host dependency in one profile `deps` directory and supplies that directory once. Oven keeps
/// normal rebuild outputs in identity-specific directories, so replaying those directories separately changes
/// rustc's metadata session hash. Hard links preserve the verified bytes while presenting the same one-directory
/// compiler fact; duplicate filenames are refused instead of selecting one artifact by discovery.
fn stage_proc_macro_dependency_artifacts(
    output_directory: &Path,
    artifacts: &BTreeSet<PathBuf>,
) -> Result<(), OvenRustcError> {
    for artifact in artifacts {
        let filename = artifact.file_name().ok_or_else(|| {
            runtime_executor_invalid(
                "runtime executor proc-macro dependency",
                format!("artifact {} has no filename", artifact.display()),
            )
        })?;
        let destination = output_directory.join(filename);
        if destination.exists() {
            return Err(runtime_executor_invalid(
                "runtime executor proc-macro dependency",
                format!("artifact filename {} occurs more than once", filename.to_string_lossy()),
            ));
        }
        fs::hard_link(artifact, &destination).map_err(|source_error| OvenRustcError::Io {
            path: destination,
            source: source_error,
        })?;
    }
    Ok(())
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
    compiler_target: &std::ffi::OsStr,
    plan: &super::OvenRustcArtifactPlan,
    search_paths: &BTreeSet<PathBuf>,
    externs: &[(String, PathBuf, String)],
    artifact: &Path,
) -> Result<(), OvenRustcError> {
    let private_out_dir = stage_private_out_dir(source, artifact)?;
    let mut command = rebuild_unit_command(
        closure,
        unit,
        source,
        selection,
        compiler_target,
        plan,
        search_paths,
        externs,
        artifact,
        private_out_dir.as_deref(),
    )?;
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

/// Copy the captured generated tree into a fresh directory owned only by this unit compilation.
fn stage_private_out_dir(
    source: &super::OvenMaterializedRustFacetUnit,
    artifact: &Path,
) -> Result<Option<PathBuf>, OvenRustcError> {
    let outputs = source
        .generated_inputs
        .iter()
        .filter(|(name, _, _)| name == "out_dir")
        .collect::<Vec<_>>();
    let ([] | [_]) = outputs.as_slice() else {
        return Err(runtime_executor_invalid(
            "runtime executor OUT_DIR",
            "unit declares more than one generated out_dir input",
        ));
    };
    let Some((_, captured, _)) = outputs.first() else {
        return Ok(None);
    };
    let unit_root = artifact.parent().ok_or_else(|| {
        runtime_executor_invalid("runtime executor OUT_DIR", "artifact has no private unit directory")
    })?;
    let destination = unit_root.join("out");
    fs::create_dir_all(&destination).map_err(|source_error| OvenRustcError::Io {
        path: destination.clone(),
        source: source_error,
    })?;
    copy_generated_tree(captured, captured, &destination)?;
    Ok(Some(destination))
}

/// Copy one verified generated tree without following links or admitting undeclared file kinds.
fn copy_generated_tree(root: &Path, directory: &Path, destination: &Path) -> Result<(), OvenRustcError> {
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
        let source_path = entry.path();
        let relative = source_path.strip_prefix(root).map_err(|_| {
            runtime_executor_invalid("runtime executor OUT_DIR", "generated member escapes its captured root")
        })?;
        let destination_path = destination.join(relative);
        let metadata = fs::symlink_metadata(&source_path).map_err(|source| OvenRustcError::Io {
            path: source_path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(runtime_executor_invalid(
                "runtime executor OUT_DIR",
                format!("generated member {} is a symlink", source_path.display()),
            ));
        }
        if metadata.is_dir() {
            fs::create_dir_all(&destination_path).map_err(|source| OvenRustcError::Io {
                path: destination_path.clone(),
                source,
            })?;
            copy_generated_tree(root, &source_path, destination)?;
        } else if metadata.is_file() {
            fs::copy(&source_path, &destination_path).map_err(|source| OvenRustcError::Io {
                path: destination_path,
                source,
            })?;
        } else {
            return Err(runtime_executor_invalid(
                "runtime executor OUT_DIR",
                format!(
                    "generated member {} is not a regular file or directory",
                    source_path.display()
                ),
            ));
        }
    }
    Ok(())
}

/// Build the one retained-compiler command that compiles an admitted rebuild unit, without launching it.
///
/// The command starts from [`super::rustc_probe_command`], the launcher every compiler probe uses, rather than a bare
/// `Command`. A store-retained compiler derives its sysroot from wherever the dynamic loader found `librustc_driver`,
/// and on Linux an inherited `LD_LIBRARY_PATH` is consulted before the retained binary's own runpath, so a rebuild
/// that inherited the caller's loader path could run another toolchain's driver while its outputs are attributed to
/// the retained closure (#1785, the rebuild half of #1755). The launcher clears the inherited Cargo state and the
/// loader search paths; the only environment the compiler then sees beyond the process's own is what the sealed
/// artifact plan and the materialized unit admit, applied below.
///
/// The arguments are the admitted authorities [`compile_rebuild_unit`] documents; building the command separately
/// lets a test pin the environment a rebuild launches with.
#[allow(clippy::too_many_arguments)]
fn rebuild_unit_command(
    closure: &OvenRuntimeCompilerClosure,
    unit: &OvenSelectedRustFacetUnit,
    source: &super::OvenMaterializedRustFacetUnit,
    selection: &super::OvenSelectedRustFacetSelection,
    compiler_target: &std::ffi::OsStr,
    plan: &super::OvenRustcArtifactPlan,
    search_paths: &BTreeSet<PathBuf>,
    externs: &[(String, PathBuf, String)],
    artifact: &Path,
    private_out_dir: Option<&Path>,
) -> Result<Command, OvenRustcError> {
    let mut command = super::rustc_probe_command(closure.rustc());
    command.args(["--crate-type", unit.compiler_crate_type.as_str()]);
    if unit.domain == OvenSelectedRustFacetDomain::Target {
        append_compiler_target(&mut command, compiler_target);
    }
    command
        .arg(format!("--edition={}", unit.edition))
        .arg("--crate-name")
        .arg(&unit.crate_name)
        .arg("--error-format=json")
        .arg("--json=diagnostic-rendered-ansi,artifacts,future-incompat")
        // Rustc folds Cargo's relative-versus-absolute crate-root spelling into metadata even when both spellings
        // remap to the same virtual source path, so replay that captured distinction exactly.
        .arg(if unit.compiler_paths.root_module_is_relative {
            Path::new(&unit.root_module)
        } else {
            &source.root_module
        })
        .arg("--out-dir")
        .arg(
            artifact.parent().ok_or_else(|| {
                runtime_executor_invalid("runtime executor output", "artifact has no output directory")
            })?,
        );
    if unit.compiler_arguments.is_empty() {
        apply_oven_profile(&mut command, &selection.intent.profile);
    }
    append_rebuild_unit_environment(&mut command, unit, source, plan, private_out_dir)?;
    append_rebuild_unit_inputs(
        &mut command,
        closure,
        unit,
        source,
        plan,
        search_paths,
        externs,
        artifact,
        private_out_dir,
    )?;
    let working_relative = Path::new(&unit.compiler_paths.working_directory)
        .strip_prefix(&unit.compiler_paths.source_root)
        .map_err(|_| {
            runtime_executor_invalid(
                "runtime executor working directory",
                "captured working directory is outside the selected source root",
            )
        })?;
    command.current_dir(source.source_root.join(working_relative));
    Ok(command)
}

/// Apply the captured compile environment and fallback cfgs before replaying ordered compiler arguments.
fn append_rebuild_unit_environment(
    command: &mut Command,
    unit: &OvenSelectedRustFacetUnit,
    source: &super::OvenMaterializedRustFacetUnit,
    plan: &super::OvenRustcArtifactPlan,
    private_out_dir: Option<&Path>,
) -> Result<(), OvenRustcError> {
    for (name, value) in &plan.compile_environment {
        command.env(name, value);
    }
    for (name, value) in &source.environment {
        match value {
            OvenMaterializedRustFacetEnvironmentValue::Text(text) => command.env(name, text),
            OvenMaterializedRustFacetEnvironmentValue::Path(path) => command.env(name, path),
            OvenMaterializedRustFacetEnvironmentValue::OutDir(relative) => {
                let out_dir = private_out_dir.ok_or_else(|| {
                    runtime_executor_invalid(
                        "runtime executor OUT_DIR",
                        format!("environment `{name}` requires an absent generated out_dir"),
                    )
                })?;
                command.env(
                    name,
                    if relative == "." {
                        out_dir.to_path_buf()
                    } else {
                        out_dir.join(relative)
                    },
                )
            }
        };
    }
    if !unit
        .compiler_arguments
        .iter()
        .any(|argument| matches!(argument, OvenSelectedRustFacetCompilerArgument::Cfg { .. }))
    {
        for feature in &unit.features {
            command.arg("--cfg").arg(format!("feature={feature:?}"));
        }
        for cfg in &unit.cfg {
            command.arg("--cfg").arg(cfg);
        }
    }
    Ok(())
}

/// Replay captured search, extern, remap, linker, sysroot, and native-link inputs in Cargo's order.
#[allow(clippy::too_many_arguments)]
fn append_rebuild_unit_inputs(
    command: &mut Command,
    closure: &OvenRuntimeCompilerClosure,
    unit: &OvenSelectedRustFacetUnit,
    source: &super::OvenMaterializedRustFacetUnit,
    plan: &super::OvenRustcArtifactPlan,
    search_paths: &BTreeSet<PathBuf>,
    externs: &[(String, PathBuf, String)],
    artifact: &Path,
    private_out_dir: Option<&Path>,
) -> Result<(), OvenRustcError> {
    let proc_macro_search_paths;
    let proc_macro_externs;
    let replay_search_paths = if unit.crate_kind == OvenSelectedRustFacetCrateKind::ProcMacro {
        proc_macro_search_paths = BTreeSet::from([artifact
            .parent()
            .ok_or_else(|| runtime_executor_invalid("runtime executor output", "artifact has no output directory"))?
            .to_path_buf()]);
        &proc_macro_search_paths
    } else {
        search_paths
    };
    let replay_externs = if unit.crate_kind == OvenSelectedRustFacetCrateKind::ProcMacro {
        let output_directory = artifact
            .parent()
            .ok_or_else(|| runtime_executor_invalid("runtime executor output", "artifact has no output directory"))?;
        proc_macro_externs = externs
            .iter()
            .map(|(alias, path, virtual_directory)| {
                let filename = path.file_name().ok_or_else(|| {
                    runtime_executor_invalid(
                        "runtime executor proc-macro dependency",
                        format!("extern artifact {} has no filename", path.display()),
                    )
                })?;
                Ok((
                    alias.clone(),
                    output_directory.join(filename),
                    virtual_directory.clone(),
                ))
            })
            .collect::<Result<Vec<_>, OvenRustcError>>()?;
        proc_macro_externs.as_slice()
    } else {
        externs
    };
    if unit.compiler_arguments.is_empty() {
        append_runtime_search_paths(
            command,
            plan,
            replay_search_paths,
            unit.crate_kind != OvenSelectedRustFacetCrateKind::ProcMacro,
        );
        for (alias, path, _) in replay_externs {
            command.arg("--extern").arg(format!("{alias}={}", path.display()));
        }
    } else {
        append_captured_compiler_arguments(
            command,
            &unit.compiler_arguments,
            plan,
            replay_search_paths,
            replay_externs,
            unit.crate_kind != OvenSelectedRustFacetCrateKind::ProcMacro,
        )?;
    }
    append_runtime_path_remaps(
        command,
        closure,
        unit,
        source,
        replay_externs,
        artifact,
        private_out_dir,
    )?;
    append_captured_linker_arguments(command, &unit.compiler_arguments);
    append_materialized_sysroot_extern_arguments(command, &source.sysroot_externs);
    append_materialized_link_arguments(command, &source.linked_libraries)?;
    if unit.crate_kind == OvenSelectedRustFacetCrateKind::ProcMacro {
        // The Cargo capture boundary clears this undeclared host input before linking the corresponding dylib.
        // Replaying an inherited SDK path would perturb Apple ld's UUID and the derived ad-hoc signature.
        command.env_remove("SDKROOT");
    }
    Ok(())
}

/// Append the publisher's injected path remaps after Cargo's own compiler arguments.
fn append_runtime_path_remaps(
    command: &mut Command,
    closure: &OvenRuntimeCompilerClosure,
    unit: &OvenSelectedRustFacetUnit,
    source: &super::OvenMaterializedRustFacetUnit,
    externs: &[(String, PathBuf, String)],
    artifact: &Path,
    private_out_dir: Option<&Path>,
) -> Result<(), OvenRustcError> {
    command.arg(format!(
        "--remap-path-prefix={}={}",
        source.source_root.display(),
        unit.compiler_paths.source_root
    ));
    let artifact_directory = artifact
        .parent()
        .ok_or_else(|| runtime_executor_invalid("runtime executor output", "artifact has no output directory"))?;
    command.arg(format!(
        "--remap-path-prefix={}={}",
        artifact_directory.display(),
        unit.compiler_paths.output_directory
    ));
    // The private OUT_DIR is nested below the artifact directory. rustc applies the last matching remap, so the
    // narrower captured OUT_DIR coordinate must follow the enclosing output-directory coordinate.
    if let (Some(private_out_dir), Some(compiler_out_dir)) = (private_out_dir, &unit.compiler_paths.out_dir) {
        command.arg(format!(
            "--remap-path-prefix={}={compiler_out_dir}",
            private_out_dir.display()
        ));
    }
    for (_, dependency, compiler_output_directory) in externs {
        if let Some(parent) = dependency.parent() {
            command.arg(format!(
                "--remap-path-prefix={}={compiler_output_directory}",
                parent.display()
            ));
        }
    }
    if let Some(toolchain_root) = closure.rustc().parent().and_then(Path::parent)
        && let Some(commit) = crate::rustc::rustc_commit_hash(closure.rustc())
    {
        command.arg(format!(
            "--remap-path-prefix={}=/rustc/{commit}",
            toolchain_root.join("lib/rustlib/src/rust").display()
        ));
    }
    Ok(())
}

/// Append Cargo's ordered compiler facts, substituting admitted search paths and extern artifacts at their markers.
fn append_captured_compiler_arguments(
    command: &mut Command,
    arguments: &[OvenSelectedRustFacetCompilerArgument],
    plan: &super::OvenRustcArtifactPlan,
    search_paths: &BTreeSet<PathBuf>,
    externs: &[(String, PathBuf, String)],
    include_plan_dependency_paths: bool,
) -> Result<(), OvenRustcError> {
    let mut paths_appended = false;
    let mut extern_index = 0usize;
    for argument in arguments {
        let search_boundary = match argument {
            OvenSelectedRustFacetCompilerArgument::Extern { .. }
            | OvenSelectedRustFacetCompilerArgument::CapLints { .. } => true,
            OvenSelectedRustFacetCompilerArgument::Codegen { name, .. } => name == "link-arg",
            _ => false,
        };
        if !paths_appended && search_boundary {
            append_runtime_search_paths(command, plan, search_paths, include_plan_dependency_paths);
            paths_appended = true;
        }
        match argument {
            OvenSelectedRustFacetCompilerArgument::Emit { value } => {
                command.arg("--emit").arg(value);
            }
            OvenSelectedRustFacetCompilerArgument::Codegen { name, value } => {
                if name != "link-arg" {
                    command.arg("-C").arg(format!("{name}={value}"));
                }
            }
            OvenSelectedRustFacetCompilerArgument::CheckCfg { value } => {
                command.arg("--check-cfg").arg(value);
            }
            OvenSelectedRustFacetCompilerArgument::Cfg { value } => {
                command.arg("--cfg").arg(value);
            }
            OvenSelectedRustFacetCompilerArgument::CapLints { value } => {
                command.arg("--cap-lints").arg(value);
            }
            OvenSelectedRustFacetCompilerArgument::Lint { level, name } => {
                command.arg(format!("--{level}={name}"));
            }
            OvenSelectedRustFacetCompilerArgument::Extern { alias, .. } => {
                let (resolved_alias, path, _) = externs.get(extern_index).ok_or_else(|| {
                    runtime_executor_invalid(
                        "runtime executor extern order",
                        format!("captured extern alias `{alias}` has no admitted dependency edge"),
                    )
                })?;
                if resolved_alias != alias {
                    return Err(runtime_executor_invalid(
                        "runtime executor extern order",
                        format!("captured extern alias `{alias}` resolved as `{resolved_alias}`"),
                    ));
                }
                command.arg("--extern").arg(format!("{alias}={}", path.display()));
                extern_index += 1;
            }
        }
    }
    if !paths_appended {
        append_runtime_search_paths(command, plan, search_paths, include_plan_dependency_paths);
    }
    if extern_index != externs.len() {
        return Err(runtime_executor_invalid(
            "runtime executor extern order",
            "admitted dependency edges remain after captured compiler arguments",
        ));
    }
    Ok(())
}

/// Append all admitted dependency and native search paths at Cargo's captured search-path boundary.
fn append_runtime_search_paths(
    command: &mut Command,
    plan: &super::OvenRustcArtifactPlan,
    search_paths: &BTreeSet<PathBuf>,
    include_plan_dependency_paths: bool,
) {
    if include_plan_dependency_paths {
        for path in &plan.dependency_search_paths {
            command.arg("-L").arg(format!("dependency={}", path.display()));
        }
    }
    for path in search_paths {
        command.arg("-L").arg(format!("dependency={}", path.display()));
    }
    for path in &plan.native_search_paths {
        command.arg("-L").arg(format!("native={}", path.display()));
    }
}

/// Append capture-bound linker arguments at Cargo's terminal injection point after all path remaps.
fn append_captured_linker_arguments(command: &mut Command, arguments: &[OvenSelectedRustFacetCompilerArgument]) {
    for argument in arguments {
        if let OvenSelectedRustFacetCompilerArgument::Codegen { name, value } = argument
            && name == "link-arg"
        {
            command.arg("-C").arg(format!("link-arg={value}"));
        }
    }
}

/// Restore Cargo's exact path-backed extern order after resolving every alias through admitted graph edges.
fn order_runtime_externs(
    unit: &OvenSelectedRustFacetUnit,
    externs: Vec<(String, PathBuf, String)>,
) -> Result<Vec<(String, PathBuf, String)>, OvenRustcError> {
    let mut by_alias = externs
        .into_iter()
        .map(|external| (external.0.clone(), external))
        .collect::<BTreeMap<_, _>>();
    let mut ordered = Vec::with_capacity(by_alias.len());
    for argument in &unit.compiler_arguments {
        let OvenSelectedRustFacetCompilerArgument::Extern { alias, .. } = argument else {
            continue;
        };
        let external = by_alias.remove(alias).ok_or_else(|| {
            runtime_executor_invalid(
                "runtime executor extern order",
                format!("captured extern alias `{alias}` has no admitted dependency edge"),
            )
        })?;
        ordered.push(external);
    }
    if !by_alias.is_empty() {
        return Err(runtime_executor_invalid(
            "runtime executor extern order",
            format!(
                "admitted dependency aliases are absent from captured extern order: {}",
                by_alias.keys().cloned().collect::<Vec<_>>().join(", ")
            ),
        ));
    }
    Ok(ordered)
}

/// Select the exact admitted prebuilt artifact form Cargo supplied for one extern edge.
fn runtime_prebuilt_extern_artifact(
    unit: &OvenSelectedRustFacetUnit,
    dependency: &OvenMaterializedRuntimeFoundationPrebuiltDependency,
) -> Result<PathBuf, OvenRustcError> {
    if runtime_extern_uses_metadata(unit, &dependency.alias) {
        return dependency.metadata_artifact.clone().ok_or_else(|| {
            runtime_executor_invalid(
                "runtime executor metadata extern",
                format!(
                    "captured extern alias `{}` has no admitted metadata artifact",
                    dependency.alias
                ),
            )
        });
    }
    Ok(dependency.artifact.clone())
}

/// Select the metadata or link output just produced for one rebuilt extern edge.
fn runtime_rebuilt_extern_artifact(
    unit: &OvenSelectedRustFacetUnit,
    alias: &str,
    artifact: &Path,
) -> Result<PathBuf, OvenRustcError> {
    if !runtime_extern_uses_metadata(unit, alias) {
        return Ok(artifact.to_path_buf());
    }
    let mut metadata = artifact.to_path_buf();
    metadata.set_extension("rmeta");
    verified_regular_file(&metadata, "runtime rebuild metadata output")
}

/// Report whether Cargo's captured compiler arguments used a pipelined metadata artifact for one alias.
fn runtime_extern_uses_metadata(unit: &OvenSelectedRustFacetUnit, alias: &str) -> bool {
    unit.compiler_arguments.iter().any(|argument| {
        matches!(
            argument,
            OvenSelectedRustFacetCompilerArgument::Extern {
                alias: candidate,
                metadata: true,
            } if candidate == alias
        )
    })
}

/// Append compiler-owned bare externs already admitted by the selected graph's verified toolchain contract.
fn append_materialized_sysroot_extern_arguments(command: &mut Command, sysroot_externs: &[String]) {
    for sysroot_extern in sysroot_externs {
        command.arg("--extern").arg(sysroot_extern);
    }
}

/// Append the exact already-materialized built-in triple or verified custom JSON path.
fn append_compiler_target(command: &mut Command, compiler_target: &std::ffi::OsStr) {
    command.arg("--target").arg(compiler_target);
}

/// Append ordered physically admitted linked-library inputs to one rustc invocation.
///
/// Exact archives use rustc's native search and library arguments so rlib metadata retains Cargo's link directive.
/// Repeated archives remain repeated and their order is unchanged. Provider inputs have already been checked against
/// held target-specific provenance and exact member bytes; frameworks and system libraries use only their admitted
/// search root and Cargo-equivalent rustc metadata arguments.
fn append_materialized_link_arguments(
    command: &mut Command,
    libraries: &[OvenMaterializedRustFacetLinkedLibrary],
) -> Result<(), OvenRustcError> {
    for library in libraries {
        match library {
            OvenMaterializedRustFacetLinkedLibrary::Archive {
                name, kind, artifact, ..
            } => {
                let parent = artifact.parent().ok_or_else(|| {
                    runtime_executor_invalid(
                        "runtime executor linked archive",
                        format!("archive {} has no admitted search directory", artifact.display()),
                    )
                })?;
                let rustc_kind = match kind {
                    crate::rustc::OvenSelectedRustFacetLinkedLibraryKind::Static => "static",
                    crate::rustc::OvenSelectedRustFacetLinkedLibraryKind::Dynamic => "dylib",
                    crate::rustc::OvenSelectedRustFacetLinkedLibraryKind::Framework
                    | crate::rustc::OvenSelectedRustFacetLinkedLibraryKind::System => {
                        return Err(runtime_executor_invalid(
                            "runtime executor linked archive",
                            format!("archive `{name}` has provider linkage kind {kind:?}"),
                        ));
                    }
                };
                command.arg("-L").arg(format!("native={}", parent.display()));
                command.arg("-l").arg(format!("{rustc_kind}={name}"));
            }
            OvenMaterializedRustFacetLinkedLibrary::Provider {
                name,
                kind,
                search_root,
                ..
            } => match kind {
                crate::rustc::OvenSelectedRustFacetLinkedLibraryKind::Framework => {
                    command.arg("-L").arg(format!("framework={}", search_root.display()));
                    command.arg("-l").arg(format!("framework={name}"));
                }
                crate::rustc::OvenSelectedRustFacetLinkedLibraryKind::System => {
                    command.arg("-L").arg(format!("native={}", search_root.display()));
                    command.arg("-l").arg(name);
                }
                crate::rustc::OvenSelectedRustFacetLinkedLibraryKind::Static
                | crate::rustc::OvenSelectedRustFacetLinkedLibraryKind::Dynamic => {
                    return Err(runtime_executor_invalid(
                        "runtime executor linked provider",
                        format!("provider `{name}` has archive linkage kind {kind:?}"),
                    ));
                }
            },
        }
    }
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
    use crate::rustc::{
        OVEN_RUNTIME_FOUNDATION_SCHEMA_VERSION, OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION, OvenMaterializedRustFacetUnit, OvenRuntimeFoundation,
        OvenRuntimeFoundationUnit, OvenRustcArtifactExtern, OvenRustcArtifactManifest, OvenRustcRegistryLeaf,
        OvenRustcRegistrySource, OvenRustcRegistrySourcePackage, OvenRustcSupportingArtifact,
        OvenSelectedRustFacetCfgSnapshot, OvenSelectedRustFacetCompilerPaths, OvenSelectedRustFacetDependency,
        OvenSelectedRustFacetGraph, OvenSelectedRustFacetIntent, OvenSelectedRustFacetOwner,
        OvenSelectedRustFacetOwnerKind, OvenSelectedRustFacetOwnerRoot, OvenSelectedRustFacetPath,
        OvenSelectedRustFacetPurpose, OvenSelectedRustFacetSelection, OvenSelectedRustFacetSource,
        OvenSelectedRustFacetSourceKind, OvenSelectedRustFacetSourceMember, OvenSelectedRustFacetTargetSpec,
        resolve_active_rustc, rustc_host_target, rustc_identity, selected_graph_sha256, selected_graph_source_digest,
        selected_graph_unit_identity,
    };

    /// Exact archive paths, including repeats, reach the linker in declared order.
    #[test]
    fn linked_archive_arguments_preserve_exact_order_and_multiplicity() -> Result<(), Box<dyn std::error::Error>> {
        let first = PathBuf::from("admitted/libfirst.a");
        let second = PathBuf::from("admitted/libsecond.a");
        let archive = |name: &str, artifact: &Path| OvenMaterializedRustFacetLinkedLibrary::Archive {
            name: name.to_string(),
            kind: crate::rustc::OvenSelectedRustFacetLinkedLibraryKind::Static,
            artifact: artifact.to_path_buf(),
            digest: selected_graph_sha256(name.as_bytes()),
        };
        let libraries = vec![
            archive("first", &first),
            archive("second", &second),
            archive("first", &first),
        ];
        let mut command = Command::new("rustc");
        append_materialized_link_arguments(&mut command, &libraries)?;
        assert_eq!(
            command
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            [
                "-L",
                "native=admitted",
                "-l",
                "static=first",
                "-L",
                "native=admitted",
                "-l",
                "static=second",
                "-L",
                "native=admitted",
                "-l",
                "static=first",
            ]
        );
        Ok(())
    }

    /// A custom target reaches rustc through its admitted JSON path rather than a substituted target triple.
    #[test]
    fn custom_target_uses_the_exact_admitted_json_path() -> Result<(), Box<dyn std::error::Error>> {
        let target = PathBuf::from("/sealed/toolchain/targets/custom.json");
        let mut command = Command::new("rustc");
        append_compiler_target(&mut command, target.as_os_str());
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [std::ffi::OsStr::new("--target"), target.as_os_str()]
        );
        Ok(())
    }

    /// Framework and system arguments retain admitted paths and declared ordering.
    #[test]
    fn linked_provider_arguments_use_only_verified_paths_and_preserve_order() -> Result<(), Box<dyn std::error::Error>>
    {
        let provider = |name: &str, kind: crate::rustc::OvenSelectedRustFacetLinkedLibraryKind, artifact: &str| {
            OvenMaterializedRustFacetLinkedLibrary::Provider {
                name: name.to_string(),
                kind,
                target: "aarch64-apple-darwin".to_string(),
                capability: format!("fixture.{name}"),
                receipt_identity: selected_graph_sha256(name.as_bytes()),
                search_root: PathBuf::from("admitted/provider"),
                artifact: PathBuf::from(artifact),
                digest: selected_graph_sha256(artifact.as_bytes()),
            }
        };
        let libraries = [
            provider(
                "Security",
                crate::rustc::OvenSelectedRustFacetLinkedLibraryKind::Framework,
                "admitted/provider/Security.framework/Security",
            ),
            provider(
                "sqlite3",
                crate::rustc::OvenSelectedRustFacetLinkedLibraryKind::System,
                "admitted/provider/libsqlite3.tbd",
            ),
        ];
        let mut command = Command::new("rustc");
        append_materialized_link_arguments(&mut command, &libraries)?;
        assert_eq!(
            command
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            [
                "-L",
                "framework=admitted/provider",
                "-l",
                "framework=Security",
                "-L",
                "native=admitted/provider",
                "-l",
                "sqlite3",
            ]
        );
        Ok(())
    }

    /// A verified sysroot input stays pathless so rustc resolves it from its own toolchain.
    #[test]
    fn sysroot_extern_arguments_preserve_exact_bare_form() {
        let mut command = Command::new("rustc");
        append_materialized_sysroot_extern_arguments(&mut command, &["proc_macro".to_string()]);
        assert_eq!(
            command
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            ["--extern", "proc_macro"]
        );
    }

    /// Rebuilds replay Cargo's host/target shape, output shape, lint cap, and ordered byte-affecting arguments exactly.
    #[test]
    fn rebuild_arguments_follow_selected_unit_domain_and_source_kind() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture()?;
        let output_root = tempfile::tempdir()?;
        let closure = OvenRuntimeCompilerClosure::new(&fixture.rustc, FIXTURE_CLOSURE);
        let graph = fixture.foundation.selected_graph().graph();
        let selected_identity = fixture
            .materialized
            .rebuild_order()
            .next()
            .ok_or("the fixture declares no rebuild unit")?;
        let source = fixture
            .materialized
            .sources()
            .unit(selected_identity)
            .ok_or("the first rebuild unit was not materialized")?;
        let base_unit = graph
            .units
            .iter()
            .find(|unit| unit.identity == selected_identity)
            .ok_or("the first rebuild unit is absent from the selected graph")?;
        let captured = vec![
            OvenSelectedRustFacetCompilerArgument::Emit {
                value: "dep-info,metadata,link".to_string(),
            },
            OvenSelectedRustFacetCompilerArgument::Codegen {
                name: "embed-bitcode".to_string(),
                value: "no".to_string(),
            },
            OvenSelectedRustFacetCompilerArgument::Codegen {
                name: "prefer-dynamic".to_string(),
                value: "yes".to_string(),
            },
            OvenSelectedRustFacetCompilerArgument::CheckCfg {
                value: "cfg(docsrs,test)".to_string(),
            },
            OvenSelectedRustFacetCompilerArgument::CapLints {
                value: "warn".to_string(),
            },
            OvenSelectedRustFacetCompilerArgument::Codegen {
                name: "link-arg".to_string(),
                value: "-Wl,-reproducible".to_string(),
            },
            OvenSelectedRustFacetCompilerArgument::Codegen {
                name: "link-arg".to_string(),
                value: "-install_name".to_string(),
            },
            OvenSelectedRustFacetCompilerArgument::Codegen {
                name: "link-arg".to_string(),
                value: "@rpath/libfixture.dylib".to_string(),
            },
        ];
        let arguments_for = |kind, domain| -> Result<Vec<String>, OvenRustcError> {
            let mut unit = base_unit.clone();
            unit.source.kind = kind;
            unit.domain = domain;
            unit.compiler_arguments = captured.clone();
            let command = rebuild_unit_command(
                &closure,
                &unit,
                source,
                &graph.selection,
                fixture.materialized.sources().compiler_target(),
                fixture.materialized.artifact_plan(),
                &BTreeSet::new(),
                &[],
                &output_root.path().join(format!("lib{}.rlib", unit.crate_name)),
                None,
            )?;
            Ok(command
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect())
        };

        let registry = arguments_for(
            OvenSelectedRustFacetSourceKind::Registry,
            OvenSelectedRustFacetDomain::Target,
        )?;
        assert!(registry.iter().any(|argument| argument == "--target"));
        assert!(registry.windows(2).any(|pair| pair == ["--cap-lints", "warn"]));
        assert!(registry.windows(2).any(|pair| pair == ["--crate-type", "lib"]));
        assert!(
            registry
                .windows(2)
                .any(|pair| pair == ["--emit", "dep-info,metadata,link"])
        );
        assert!(registry.iter().any(|argument| argument == "--out-dir"));
        assert!(!registry.iter().any(|argument| argument == "-o"));
        assert!(registry.windows(2).any(|pair| pair == ["-C", "embed-bitcode=no"]));
        assert!(registry.windows(2).any(|pair| pair == ["-C", "prefer-dynamic=yes"]));
        assert!(
            registry
                .windows(2)
                .any(|pair| pair == ["--check-cfg", "cfg(docsrs,test)"])
        );
        let final_remap = registry
            .iter()
            .rposition(|argument| argument.starts_with("--remap-path-prefix="))
            .ok_or("rebuild command has no path remap")?;
        let install_name = registry
            .iter()
            .position(|argument| argument == "link-arg=-install_name")
            .ok_or("rebuild command has no captured install-name argument")?;
        assert!(registry.iter().any(|argument| argument == "link-arg=-Wl,-reproducible"));
        assert!(install_name > final_remap);

        let path = arguments_for(OvenSelectedRustFacetSourceKind::Path, OvenSelectedRustFacetDomain::Host)?;
        assert!(!path.iter().any(|argument| argument == "--target"));

        let mut proc_macro = base_unit.clone();
        proc_macro.crate_kind = OvenSelectedRustFacetCrateKind::ProcMacro;
        proc_macro.compiler_crate_type = "proc-macro".to_string();
        proc_macro.compiler_arguments = captured;
        let proc_macro_command = rebuild_unit_command(
            &closure,
            &proc_macro,
            source,
            &graph.selection,
            fixture.materialized.sources().compiler_target(),
            fixture.materialized.artifact_plan(),
            &BTreeSet::new(),
            &[],
            &output_root.path().join("libfixture.dylib"),
            None,
        )?;
        assert!(
            proc_macro_command
                .get_envs()
                .any(|(name, value)| name == "SDKROOT" && value.is_none())
        );
        let proc_macro_arguments = proc_macro_command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            proc_macro_arguments
                .windows(2)
                .filter(|pair| pair[0] == "-L" && pair[1].starts_with("dependency="))
                .count(),
            1
        );
        assert!(path.windows(2).any(|pair| pair == ["--cap-lints", "warn"]));
        assert!(path.windows(2).any(|pair| pair == ["-C", "embed-bitcode=no"]));
        Ok(())
    }

    /// Return the ordered compiler facts shared by the Cargo and direct-rustc byte-equivalence fixture.
    fn equivalence_compiler_arguments(
        metadata: &str,
        extra_filename: &str,
    ) -> Vec<OvenSelectedRustFacetCompilerArgument> {
        vec![
            OvenSelectedRustFacetCompilerArgument::Codegen {
                name: "opt-level".to_string(),
                value: "3".to_string(),
            },
            OvenSelectedRustFacetCompilerArgument::Codegen {
                name: "embed-bitcode".to_string(),
                value: "no".to_string(),
            },
            OvenSelectedRustFacetCompilerArgument::Lint {
                level: "warn".to_string(),
                name: "missing_docs".to_string(),
            },
            OvenSelectedRustFacetCompilerArgument::CheckCfg {
                value: "cfg(docsrs,test)".to_string(),
            },
            OvenSelectedRustFacetCompilerArgument::CheckCfg {
                value: "cfg(feature, values())".to_string(),
            },
            OvenSelectedRustFacetCompilerArgument::Codegen {
                name: "metadata".to_string(),
                value: metadata.to_string(),
            },
            OvenSelectedRustFacetCompilerArgument::Codegen {
                name: "extra-filename".to_string(),
                value: extra_filename.to_string(),
            },
            OvenSelectedRustFacetCompilerArgument::Codegen {
                name: "strip".to_string(),
                value: "debuginfo".to_string(),
            },
            OvenSelectedRustFacetCompilerArgument::Emit {
                value: "dep-info,metadata,link".to_string(),
            },
            OvenSelectedRustFacetCompilerArgument::CapLints {
                value: "warn".to_string(),
            },
        ]
    }

    /// Collect Cargo's matching fixture rlibs without assuming whether repository Cargo config redirects build-dir.
    fn collect_equivalence_rlibs(directory: &Path, artifacts: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                collect_equivalence_rlibs(&path, artifacts)?;
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("libequivalence_fixture-") && name.ends_with(".rlib"))
            {
                artifacts.push(path);
            }
        }
        Ok(())
    }

    /// Real Cargo output and the captured facts needed to replay its synthetic library invocation.
    struct CargoEquivalenceArtifact {
        artifact: PathBuf,
        generated_out_dir: PathBuf,
        metadata: String,
        extra_filename: String,
        dependency_search_path: PathBuf,
        trace: String,
    }

    /// Compile the synthetic library with Cargo while forcing the captured metadata and portable path coordinates.
    fn compile_cargo_equivalence_fixture(
        cargo: &str,
        rustc: &Path,
        package: &Path,
        target: &Path,
        host: &str,
    ) -> Result<CargoEquivalenceArtifact, Box<dyn std::error::Error>> {
        fs::create_dir_all(package.join("src"))?;
        let cargo_home = package
            .parent()
            .ok_or("Cargo package has no parent")?
            .join("cargo-home");
        fs::create_dir(&cargo_home)?;
        fs::write(
            package.join("Cargo.toml"),
            "[package]\nname = \"equivalence-fixture\"\nversion = \"1.0.0\"\nedition = \"2024\"\nbuild = \"build.rs\"\n\n[lib]\npath = \"src/lib.rs\"\n\n[lints.rust]\nmissing_docs = \"warn\"\n\n[profile.release]\nstrip = \"debuginfo\"\n",
        )?;
        fs::write(
            package.join("build.rs"),
            "fn main() -> Result<(), Box<dyn std::error::Error>> {\n    let out = std::env::var_os(\"OUT_DIR\").ok_or(\"OUT_DIR missing\")?;\n    std::fs::write(std::path::Path::new(&out).join(\"generated.rs\"), \"const GENERATED: u32 = 1561;\\n\")?;\n    Ok(())\n}\n",
        )?;
        fs::write(
            package.join("src/lib.rs"),
            "//! Byte-equivalence fixture.\n\ninclude!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\nmod value;\n\n/// Return the issue number pinned by this fixture.\npub use value::answer;\n",
        )?;
        fs::write(
            package.join("src/value.rs"),
            "/// Return the issue number pinned by this fixture.\npub fn answer() -> u32 { crate::GENERATED }\n",
        )?;
        let output = Command::new(cargo)
            .args([
                "rustc",
                "-vv",
                "--offline",
                "--release",
                "--target",
                host,
                "--lib",
                "--",
            ])
            .args(["--cap-lints", "warn"])
            .current_dir(package)
            .env("RUSTC", rustc)
            .env("CARGO_HOME", cargo_home)
            .env("CARGO_TARGET_DIR", target)
            .env(
                "RUSTFLAGS",
                format!(
                    "--remap-path-prefix={}=/incan/source --remap-path-prefix={}=/incan/target",
                    package.display(),
                    target.display()
                ),
            )
            .output()?;
        if !output.status.success() {
            return Err(format!("Cargo fixture failed: {}", String::from_utf8_lossy(&output.stderr)).into());
        }
        let mut artifacts = Vec::new();
        collect_equivalence_rlibs(target, &mut artifacts)?;
        let [artifact] = artifacts.as_slice() else {
            return Err(format!("Cargo fixture produced {} rlibs", artifacts.len()).into());
        };
        let filename = artifact
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("Cargo rlib name is not UTF-8")?;
        let extra_filename = filename
            .strip_prefix("libequivalence_fixture")
            .and_then(|name| name.strip_suffix(".rlib"))
            .ok_or("Cargo rlib name has an unexpected shape")?;
        let trace = String::from_utf8_lossy(&output.stderr).into_owned();
        let metadata = trace
            .split_whitespace()
            .filter_map(|word| word.strip_prefix("metadata="))
            .next_back()
            .ok_or("Cargo trace has no metadata argument")?;
        let dependency_search_path = trace
            .split_whitespace()
            .filter_map(|word| word.strip_prefix("dependency="))
            .next_back()
            .map(PathBuf::from)
            .ok_or("Cargo trace has no dependency search path")?;
        fs::remove_dir_all(target)?;
        let replay = Command::new(cargo)
            .args(["rustc", "--offline", "--release", "--target", host, "--lib", "--"])
            .args(["--cap-lints", "warn"])
            .current_dir(package)
            .env("RUSTC", rustc)
            .env(
                "CARGO_HOME",
                package
                    .parent()
                    .ok_or("Cargo package has no parent")?
                    .join("cargo-home"),
            )
            .env("CARGO_TARGET_DIR", target)
            .env(
                "RUSTFLAGS",
                format!(
                    "--remap-path-prefix={}=/incan/source --remap-path-prefix={}=/incan/target",
                    package.display(),
                    target.display()
                ),
            )
            .output()?;
        if !replay.status.success() {
            return Err(format!(
                "Cargo fixture replay failed: {}",
                String::from_utf8_lossy(&replay.stderr)
            )
            .into());
        }
        let mut replay_artifacts = Vec::new();
        collect_equivalence_rlibs(target, &mut replay_artifacts)?;
        let [replay_artifact] = replay_artifacts.as_slice() else {
            return Err(format!("Cargo fixture replay produced {} rlibs", replay_artifacts.len()).into());
        };
        let mut generated_outputs = Vec::new();
        collect_equivalence_generated_outputs(target, &mut generated_outputs)?;
        let [generated_out_dir] = generated_outputs.as_slice() else {
            return Err(format!(
                "Cargo fixture produced {} generated OUT_DIR trees",
                generated_outputs.len()
            )
            .into());
        };
        Ok(CargoEquivalenceArtifact {
            artifact: replay_artifact.clone(),
            generated_out_dir: generated_out_dir.clone(),
            metadata: metadata.to_string(),
            extra_filename: extra_filename.to_string(),
            dependency_search_path,
            trace,
        })
    }

    /// Collect the synthetic build script's generated OUT_DIR without assuming Cargo's target layout.
    fn collect_equivalence_generated_outputs(directory: &Path, outputs: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                collect_equivalence_generated_outputs(&path, outputs)?;
            } else if entry.file_name() == "generated.rs" {
                let parent = path.parent().ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "generated output has no parent")
                })?;
                outputs.push(parent.to_path_buf());
            }
        }
        Ok(())
    }

    /// Give the direct compiler the same Cargo package environment present during the reference build.
    fn set_cargo_equivalence_environment(source: &mut OvenMaterializedRustFacetUnit, package: &Path) {
        source.environment.clear();
        source.environment.insert(
            "CARGO_MANIFEST_DIR".to_string(),
            OvenMaterializedRustFacetEnvironmentValue::Path(package.to_path_buf()),
        );
        source.environment.insert(
            "CARGO_MANIFEST_PATH".to_string(),
            OvenMaterializedRustFacetEnvironmentValue::Path(package.join("Cargo.toml")),
        );
        for (name, value) in [
            ("CARGO_CRATE_NAME", "equivalence_fixture"),
            ("CARGO_PKG_AUTHORS", ""),
            ("CARGO_PKG_DESCRIPTION", ""),
            ("CARGO_PKG_HOMEPAGE", ""),
            ("CARGO_PKG_LICENSE", ""),
            ("CARGO_PKG_LICENSE_FILE", ""),
            ("CARGO_PKG_NAME", "equivalence-fixture"),
            ("CARGO_PKG_README", ""),
            ("CARGO_PKG_REPOSITORY", ""),
            ("CARGO_PKG_RUST_VERSION", ""),
            ("CARGO_PKG_VERSION", "1.0.0"),
            ("CARGO_PKG_VERSION_MAJOR", "1"),
            ("CARGO_PKG_VERSION_MINOR", "0"),
            ("CARGO_PKG_VERSION_PATCH", "0"),
            ("CARGO_PKG_VERSION_PRE", ""),
            ("CARGO_PRIMARY_PACKAGE", "1"),
        ] {
            source.environment.insert(
                name.to_string(),
                OvenMaterializedRustFacetEnvironmentValue::Text(value.to_string()),
            );
        }
    }

    /// Report member-level evidence when a real Cargo rlib and its direct rebuild differ.
    fn require_equal_equivalence_rlibs(
        cargo_witness: &Path,
        oven_artifact: &Path,
        cargo_bytes: &[u8],
        cargo_trace: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let oven_bytes = fs::read(oven_artifact)
            .map_err(|error| format!("cannot read Oven artifact {}: {error}", oven_artifact.display()))?;
        if cargo_bytes == oven_bytes {
            return Ok(());
        }
        let cargo_members = Command::new("ar").arg("t").arg(cargo_witness).output()?;
        let oven_members = Command::new("ar").arg("t").arg(oven_artifact).output()?;
        let cargo_rmeta = Command::new("ar")
            .args(["p", cargo_witness.to_string_lossy().as_ref(), "lib.rmeta"])
            .output()?;
        let oven_rmeta = Command::new("ar")
            .args(["p", oven_artifact.to_string_lossy().as_ref(), "lib.rmeta"])
            .output()?;
        let object = String::from_utf8_lossy(&cargo_members.stdout)
            .lines()
            .find(|member| member.ends_with(".o"))
            .ok_or("Cargo rlib has no object member")?
            .to_string();
        let cargo_object = Command::new("ar")
            .args(["p", cargo_witness.to_string_lossy().as_ref(), &object])
            .output()?;
        let oven_object = Command::new("ar")
            .args(["p", oven_artifact.to_string_lossy().as_ref(), &object])
            .output()?;
        Err(format!(
            "Cargo and Oven rlibs differ; Cargo rmeta {}; Oven rmeta {}; Cargo object {}; Oven object {}; Cargo members: {}; Oven members: {}; Cargo trace: {cargo_trace}",
            selected_graph_sha256(&cargo_rmeta.stdout),
            selected_graph_sha256(&oven_rmeta.stdout),
            selected_graph_sha256(&cargo_object.stdout),
            selected_graph_sha256(&oven_object.stdout),
            String::from_utf8_lossy(&cargo_members.stdout),
            String::from_utf8_lossy(&oven_members.stdout)
        )
        .into())
    }

    /// A Cargo-selected synthetic library and the direct executor produce the same raw rlib bytes.
    ///
    /// The compiler suite runs without Cargo, so this comparison runs only when a publisher names a Cargo executable
    /// in `INCAN_TEST_EQUIVALENCE_CARGO`; the release-family artifact-equivalence gate covers the same contract over
    /// every governed unit.
    #[test]
    fn publisher_rebuild_matches_real_cargo_rlib_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let Some(cargo) = std::env::var_os("INCAN_TEST_EQUIVALENCE_CARGO") else {
            return Ok(());
        };
        let cargo = cargo.to_string_lossy().into_owned();
        let cargo = cargo.as_str();
        let fixture = fixture()?;
        let closure = OvenRuntimeCompilerClosure::new(&fixture.rustc, FIXTURE_CLOSURE);
        let graph = fixture.foundation.selected_graph().graph();
        let selected_identity = fixture
            .materialized
            .rebuild_order()
            .next()
            .ok_or("the fixture declares no rebuild unit")?;
        let scratch = tempfile::tempdir()?;
        let package = scratch.path().join("package");
        let target = scratch.path().join("target");
        let host = rustc_host_target(&fixture.rustc)?;
        let cargo_output = compile_cargo_equivalence_fixture(cargo, &fixture.rustc, &package, &target, &host)?;
        let cargo_artifact = cargo_output.artifact.clone();

        let mut unit = graph
            .units
            .iter()
            .find(|unit| unit.identity == selected_identity)
            .ok_or("the first rebuild unit is absent from the selected graph")?
            .clone();
        unit.package = "equivalence-fixture".to_string();
        unit.package_version = "1.0.0".to_string();
        unit.crate_name = "equivalence_fixture".to_string();
        unit.edition = "2024".to_string();
        unit.compiler_crate_type = "lib".to_string();
        unit.compiler_arguments = equivalence_compiler_arguments(&cargo_output.metadata, &cargo_output.extra_filename);
        let cargo_output_directory = cargo_artifact
            .parent()
            .ok_or("Cargo artifact has no output directory")?
            .strip_prefix(&target)?;
        unit.compiler_paths = OvenSelectedRustFacetCompilerPaths {
            root_module_is_relative: true,
            source_root: "/incan/source".to_string(),
            working_directory: "/incan/source".to_string(),
            output_directory: Path::new("/incan/target")
                .join(cargo_output_directory)
                .to_string_lossy()
                .into_owned(),
            out_dir: Some(
                Path::new("/incan/target")
                    .join(cargo_output.generated_out_dir.strip_prefix(&target)?)
                    .to_string_lossy()
                    .into_owned(),
            ),
        };
        let mut source = fixture
            .materialized
            .sources()
            .unit(selected_identity)
            .ok_or("the first rebuild unit was not materialized")?
            .clone();
        source.source_root = package.clone();
        source.root_module = package.join("src/lib.rs");
        set_cargo_equivalence_environment(&mut source, &package);
        source.environment.insert(
            "OUT_DIR".to_string(),
            OvenMaterializedRustFacetEnvironmentValue::OutDir(".".to_string()),
        );
        source.generated_inputs = vec![(
            "out_dir".to_string(),
            cargo_output.generated_out_dir.clone(),
            selected_graph_sha256(b"synthetic generated output"),
        )];
        source.linked_libraries.clear();
        source.sysroot_externs.clear();
        let cargo_bytes = fs::read(&cargo_artifact)
            .map_err(|error| format!("cannot read Cargo artifact {}: {error}", cargo_artifact.display()))?;
        let cargo_witness = scratch.path().join("cargo-witness.rlib");
        fs::write(&cargo_witness, &cargo_bytes)?;
        let oven_artifact = cargo_artifact.clone();
        assert_eq!(
            oven_artifact.file_name().and_then(|name| name.to_str()),
            Some(runtime_rebuild_artifact_name(&unit, &host)?.as_str())
        );
        fs::remove_file(&oven_artifact)?;
        let mut plan = fixture.materialized.artifact_plan().clone();
        plan.dependency_search_paths = vec![cargo_output.dependency_search_path];
        plan.native_search_paths.clear();
        plan.compile_environment.clear();
        compile_rebuild_unit(
            &closure,
            &unit,
            &source,
            &graph.selection,
            std::ffi::OsStr::new(&host),
            &plan,
            &BTreeSet::new(),
            &[],
            &oven_artifact,
        )?;
        require_equal_equivalence_rlibs(&cargo_witness, &oven_artifact, &cargo_bytes, &cargo_output.trace)
    }

    /// A rebuilt registry unit can include source from its captured generated output through a private OUT_DIR.
    #[test]
    fn publisher_rebuild_stages_a_private_out_dir() -> Result<(), Box<dyn std::error::Error>> {
        compile_synthetic_registry_unit(
            "include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\npub fn value() -> u8 { GENERATED }\n",
            &[("generated.rs", "const GENERATED: u8 = 7;\n")],
            BTreeMap::from([(
                "OUT_DIR".to_string(),
                OvenMaterializedRustFacetEnvironmentValue::OutDir(".".to_string()),
            )]),
            &[],
        )
    }

    /// Cargo package metadata captured for the unit remains available to compile-time `env!` expansion.
    #[test]
    fn publisher_rebuild_replays_cargo_package_environment() -> Result<(), Box<dyn std::error::Error>> {
        compile_synthetic_registry_unit(
            "pub const VERSION: &str = env!(\"CARGO_PKG_VERSION\");\n",
            &[],
            BTreeMap::from([(
                "CARGO_PKG_VERSION".to_string(),
                OvenMaterializedRustFacetEnvironmentValue::Text("9.8.7-made-up".to_string()),
            )]),
            &[],
        )
    }

    /// Build-script cfg and rustc-env outputs reach the same rebuilt unit together.
    #[test]
    fn publisher_rebuild_replays_build_script_cfg_and_environment() -> Result<(), Box<dyn std::error::Error>> {
        compile_synthetic_registry_unit(
            "#[cfg(synthetic_switch)]\npub const MARKER: &str = env!(\"SYNTHETIC_MARKER\");\n",
            &[],
            BTreeMap::from([(
                "SYNTHETIC_MARKER".to_string(),
                OvenMaterializedRustFacetEnvironmentValue::Text("apricot".to_string()),
            )]),
            &["synthetic_switch"],
        )
    }

    /// Compile one made-up registry package through the actual retained-rustc rebuild boundary.
    fn compile_synthetic_registry_unit(
        source_text: &str,
        generated: &[(&str, &str)],
        environment: BTreeMap<String, OvenMaterializedRustFacetEnvironmentValue>,
        cfg: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture()?;
        let closure = OvenRuntimeCompilerClosure::new(&fixture.rustc, FIXTURE_CLOSURE);
        let graph = fixture.foundation.selected_graph().graph();
        let selected_identity = fixture
            .materialized
            .rebuild_order()
            .next()
            .ok_or("the fixture declares no rebuild unit")?;
        let mut unit = graph
            .units
            .iter()
            .find(|unit| unit.identity == selected_identity)
            .ok_or("the first rebuild unit is absent from the selected graph")?
            .clone();
        unit.package = "velvet-fixture".to_string();
        unit.package_version = "9.8.7".to_string();
        unit.crate_name = "velvet_fixture".to_string();
        unit.source.kind = OvenSelectedRustFacetSourceKind::Registry;
        unit.cfg = cfg.iter().map(|value| (*value).to_string()).collect();
        unit.compiler_arguments
            .retain(|argument| !matches!(argument, OvenSelectedRustFacetCompilerArgument::Extern { .. }));

        let scratch = tempfile::tempdir()?;
        let source_root = scratch.path().join("source");
        let captured_out = scratch.path().join("captured-out");
        let output_root = scratch.path().join("build");
        fs::create_dir_all(&source_root)?;
        fs::create_dir_all(&output_root)?;
        let root_module = if unit.compiler_paths.root_module_is_relative {
            source_root.join(&unit.root_module)
        } else {
            source_root.join("lib.rs")
        };
        fs::create_dir_all(root_module.parent().ok_or("synthetic root module has no parent")?)?;
        fs::write(&root_module, source_text)?;
        let mut generated_inputs = Vec::new();
        if !generated.is_empty() {
            fs::create_dir_all(&captured_out)?;
            for (name, contents) in generated {
                fs::write(captured_out.join(name), contents)?;
            }
            generated_inputs.push((
                "out_dir".to_string(),
                captured_out,
                selected_graph_sha256(b"synthetic generated output"),
            ));
        }
        let mut source = fixture
            .materialized
            .sources()
            .unit(selected_identity)
            .ok_or("the first rebuild unit was not materialized")?
            .clone();
        source.source_root = source_root;
        source.root_module = root_module;
        source.environment = environment;
        source.generated_inputs = generated_inputs;
        source.linked_libraries.clear();
        source.sysroot_externs.clear();
        let artifact = output_root.join(runtime_rebuild_artifact_name(&unit, &graph.selection.host)?);
        compile_rebuild_unit(
            &closure,
            &unit,
            &source,
            &graph.selection,
            fixture.materialized.sources().compiler_target(),
            fixture.materialized.artifact_plan(),
            &BTreeSet::new(),
            &[],
            &artifact,
        )?;
        assert!(artifact.is_file());
        Ok(())
    }

    use oven_store::OvenBuildIntent;

    /// Compiler closure identity the fixture binds to the retained host compiler.
    pub const FIXTURE_CLOSURE: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
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
    /// The call chain is the proof. `incan_lang` calls into the sealed prebuilt crate and `incan_std_core` calls into
    /// `incan_lang`, so neither compiles unless the executor passed the right `--extern` for both an SDK-owned
    /// artifact and an output it produced itself earlier in the declared order.
    fn fixture_source(crate_name: &str) -> String {
        match crate_name {
            "fixture_dep" => "pub fn dep_marker() -> u8 {\n    3\n}\n".to_string(),
            "incan_lang" => "pub fn core_marker() -> u8 {\n    fixture_dep::dep_marker() + 4\n}\n".to_string(),
            _ => "pub fn stdlib_marker() -> u8 {\n    incan_lang::core_marker()\n}\n".to_string(),
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
        let compiler_arguments = dependencies
            .iter()
            .map(|dependency| OvenSelectedRustFacetCompilerArgument::Extern {
                alias: dependency.alias.clone(),
                metadata: false,
            })
            .chain([
                OvenSelectedRustFacetCompilerArgument::Emit {
                    value: "dep-info,metadata,link".to_string(),
                },
                OvenSelectedRustFacetCompilerArgument::Codegen {
                    name: "metadata".to_string(),
                    value: "runtime-fixture".to_string(),
                },
                OvenSelectedRustFacetCompilerArgument::Codegen {
                    name: "extra-filename".to_string(),
                    value: "-runtime-fixture".to_string(),
                },
                OvenSelectedRustFacetCompilerArgument::CapLints {
                    value: "warn".to_string(),
                },
            ])
            .collect();
        let mut unit = OvenSelectedRustFacetUnit {
            sysroot_externs: Vec::new(),
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
            cfg: Vec::new(),
            compiler_crate_type: "lib".to_string(),
            compiler_paths: crate::rustc::fixture_compiler_paths(),
            compiler_arguments,
            environment: BTreeMap::new(),
            include_dirs: vec![OvenSelectedRustFacetPath {
                owner: owner.to_string(),
                path: source_root.to_string(),
            }],
            exclude_dirs: Vec::new(),
            dependencies,
            generated_inputs: Vec::new(),
            linked_libraries: Vec::new(),
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
        for crate_name in ["incan_lang", "incan_std_core"] {
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

    /// Declare the minimal Unix cfg evidence used by the sealed executor fixture.
    fn cfg_snapshot(architecture: &str, operating_system: &str) -> OvenSelectedRustFacetCfgSnapshot {
        OvenSelectedRustFacetCfgSnapshot {
            flags: vec!["unix".to_string()],
            values: BTreeMap::from([
                ("target_arch".to_string(), vec![architecture.to_string()]),
                ("target_os".to_string(), vec![operating_system.to_string()]),
            ]),
        }
    }

    /// Assemble the host-native foundation this connector's first proof compiles.
    fn host_native_foundation(
        host: &str,
        toolchain: &str,
        dep_artifact_digest: &str,
        package_version: &str,
    ) -> Result<OvenRuntimeFoundation, Box<dyn std::error::Error>> {
        let target_cfg = cfg_snapshot("fixture-target", "fixture");
        let selection = OvenSelectedRustFacetSelection {
            intent: OvenSelectedRustFacetIntent {
                target: host.to_string(),
                toolchain: toolchain.to_string(),
                profile: "debug".to_string(),
            },
            host: host.to_string(),
            host_cfg: cfg_snapshot("fixture-host", "fixture"),
            target_cfg: target_cfg.clone(),
            purpose: OvenSelectedRustFacetPurpose::Normal,
            toolchain_version: "1.85.0".to_string(),
            target_spec: OvenSelectedRustFacetTargetSpec::BuiltIn {
                toolchain_owner: toolchain_owner(),
                target: host.to_string(),
                rustc_identity: toolchain.to_string(),
                target_cfg_digest: selected_graph_sha256(&serde_json::to_vec(&target_cfg)?),
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
        let mut core = library_unit(
            &selection,
            "incan_lang",
            package_version,
            OvenSelectedRustFacetSourceKind::Compiler,
            &toolchain_owner(),
            "compiler/incan_lang",
            vec![OvenSelectedRustFacetDependency {
                alias: "fixture_dep".to_string(),
                unit: dep.identity.clone(),
            }],
        )?;
        core.sysroot_externs = vec!["proc_macro".to_string()];
        core.identity = selected_graph_unit_identity(&selection, &core)?;
        let stdlib = library_unit(
            &selection,
            "incan_std_core",
            package_version,
            OvenSelectedRustFacetSourceKind::Compiler,
            &toolchain_owner(),
            "compiler/incan_std_core",
            vec![OvenSelectedRustFacetDependency {
                alias: "incan_lang".to_string(),
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
                features: Vec::new(),
            },
            dependency_search_paths: vec!["deps".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![dep_extern.clone()],
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: vec![OvenRustcRegistryLeaf {
                domain: Default::default(),
                crate_kind: Default::default(),
                selected_unit_identity: None,
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
                exposed_roots: BTreeMap::from([(
                    "incan_std_core".to_string(),
                    crate::rustc::OvenSelectedRustFacetRoot {
                        unit: stdlib.identity,
                        requested_features: Vec::new(),
                        default_features: true,
                        intent_owner: toolchain_owner(),
                    },
                )]),
            },
            units,
        })
    }

    /// One prepared fixture: retained roots, a validated foundation and its publication materialization.
    pub struct Fixture {
        pub foundation: ValidatedOvenRuntimeFoundation,
        pub materialized: OvenMaterializedRuntimeFoundation,
        pub rustc: PathBuf,
        _foundation_root: tempfile::TempDir,
        _toolchain_root: tempfile::TempDir,
    }

    /// Prepare the complete host-native fixture through the real materialization boundary.
    pub fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
        fixture_at_version("1.0.0")
    }

    /// Prepare the same fixture at one explicit package coordinate.
    ///
    /// Only the package version differs between calls. Every compiler input -- source bytes, features, cfgs, edition,
    /// target and toolchain -- is held constant, which is what makes a coordinate-only reuse claim meaningful.
    pub fn fixture_at_version(package_version: &str) -> Result<Fixture, Box<dyn std::error::Error>> {
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
    fn host_native_rebuild_compiles_incan_lang_then_incan_std_core() -> Result<(), Box<dyn std::error::Error>> {
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
        assert_eq!(produced, vec!["incan_lang", "incan_std_core"]);
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
            .find(|output| output.crate_name == "incan_lang")
            .ok_or("build lost incan_lang")?;
        let stdlib = build
            .outputs()
            .iter()
            .find(|output| output.crate_name == "incan_std_core")
            .ok_or("build lost incan_std_core")?;
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
                alias: "incan_lang".to_string(),
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
            .find(|output| output.crate_name == "incan_lang")
            .ok_or("build lost incan_lang")?
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
            .find(|output| output.crate_name == "incan_lang")
            .ok_or("rebuild lost incan_lang")?;
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

    /// A rebuild launches the retained compiler without the caller's dynamic-loader search paths.
    ///
    /// The retained `rustc` derives its sysroot from wherever the loader found `librustc_driver`. On Linux an
    /// inherited `LD_LIBRARY_PATH` wins over the binary's own runpath, and the compiler-suite runner exports one to
    /// every libtest child, so a rebuild that kept it ran the ambient toolchain's driver while its outputs were
    /// attributed to the retained closure (#1785). The probes were fixed the same way by #1755.
    #[test]
    fn rebuild_launch_clears_the_inherited_loader_search_paths() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture()?;
        let output_root = tempfile::tempdir()?;
        let closure = OvenRuntimeCompilerClosure::new(&fixture.rustc, FIXTURE_CLOSURE);
        let graph = fixture.foundation.selected_graph().graph();
        let selected_identity = fixture
            .materialized
            .rebuild_order()
            .next()
            .ok_or("the fixture declares no rebuild unit")?;
        let mut unit = graph
            .units
            .iter()
            .find(|unit| unit.identity == selected_identity)
            .ok_or("the first rebuild unit is absent from the selected graph")?
            .clone();
        unit.compiler_arguments
            .retain(|argument| !matches!(argument, OvenSelectedRustFacetCompilerArgument::Extern { .. }));
        let source = fixture
            .materialized
            .sources()
            .unit(selected_identity)
            .ok_or("the first rebuild unit was not materialized")?;

        let command = rebuild_unit_command(
            &closure,
            &unit,
            source,
            &graph.selection,
            fixture.materialized.sources().compiler_target(),
            fixture.materialized.artifact_plan(),
            &BTreeSet::new(),
            &[],
            &output_root.path().join(format!("lib{}.rlib", unit.crate_name)),
            None,
        )?;

        assert_eq!(command.get_program(), fixture.rustc.as_os_str());
        let cleared = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect::<BTreeSet<_>>();
        for name in crate::rustc::toolchain::INHERITED_LOADER_SEARCH_PATH_VARIABLES {
            assert!(
                cleared.contains(name),
                "`{name}` would steer the retained compiler to another toolchain's driver and sysroot"
            );
        }
        assert!(
            !cleared.contains("PATH"),
            "`PATH` locates the compiler on Windows and must survive the rebuild launch"
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
            .find(|unit| unit.crate_name == "incan_lang")
            .ok_or("fixture lost incan_lang")?;

        let mut host_unit = core.clone();
        host_unit.domain = OvenSelectedRustFacetDomain::Host;
        refuse_unsupported_rebuild_shape(&host_unit)?;

        let mut macro_unit = core.clone();
        macro_unit.crate_kind = OvenSelectedRustFacetCrateKind::ProcMacro;
        let Err(OvenRustcError::InvalidInput { field, .. }) = refuse_unsupported_rebuild_shape(&macro_unit) else {
            return Err("a proc-macro crate kind was accepted".into());
        };
        assert_eq!(field, "runtime executor rebuild shape");

        macro_unit.domain = OvenSelectedRustFacetDomain::Host;
        macro_unit.role = OvenSelectedRustFacetUnitRole::ProcMacro;
        refuse_unsupported_rebuild_shape(&macro_unit)?;

        let mut benchmark_unit = core.clone();
        benchmark_unit.role = OvenSelectedRustFacetUnitRole::Benchmark;
        let Err(OvenRustcError::InvalidInput { field, .. }) = refuse_unsupported_rebuild_shape(&benchmark_unit) else {
            return Err("a benchmark role was accepted".into());
        };
        assert_eq!(field, "runtime executor rebuild shape");
        Ok(())
    }
}
