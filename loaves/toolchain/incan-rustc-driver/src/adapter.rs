//! The only rustc-private boundary: Incan declarations enter as AST items and bodies enter as MIR.

use crate::error::PlanError;
use crate::identity::DriverIdentity;
use crate::native_crates::native_roots;
use crate::plan::{CalleeKind, Carrier, Plan, PlanType, TerminatorKind};
use crate::spans::Sources;
use crate::{bodies, callees, declarations, types, union_debug, validation, value_enum_serde};
use incan_driver::unit::{ToolchainAccess, UnitKind};
use incan_lang::lang::derives::{self, DeriveId};
use rustc_ast as ast;
use rustc_data_structures::steal::Steal;
use rustc_hir::def_id::LocalDefId;
use rustc_interface::interface;
use rustc_middle::mir::Body;
use rustc_middle::ty::{Ty, TyCtxt};
use rustc_session::config::Input;
use rustc_span::FileName;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

mod active_plan;
mod callbacks;
mod library_verification;
mod rustc_queries;
pub(crate) mod stable_link;

pub(crate) use active_plan::PlanReader;
use library_verification::*;
pub(crate) use rustc_queries::*;

/// Compilation is serialized: `PLAN` and `FAILURE` are process-global slots, also in the resident worker.
static COMPILATION: Mutex<()> = Mutex::new(());
/// Query providers are function pointers, so the invocation's source-owned plan has an explicit scoped slot, with its
/// name index.
static PLAN: Mutex<Option<Arc<PlanReader>>> = Mutex::new(None);
/// Typed errors raised while loading metadata or source locations survive rustc's fatal-error boundary.
static FAILURE: Mutex<Option<PlanError>> = Mutex::new(None);
/// Qualified development reuse requires tracked plan records and content-addressed Darwin link inputs.
const NATIVE_INCREMENTAL_QUALIFIED: bool = true;

/// Stable name of rustc's empty synthetic input; authored source spans are loaded separately from the native plan.
const SYNTHETIC_SOURCE_NAME: &str = "source";

/// Typed refusals and native compiler failures remain distinct at the executable boundary.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    /// The scalar plan violates its preflight contract.
    #[error(transparent)]
    Plan(#[from] PlanError),
    /// rustc refused the compilation after valid preflight input.
    #[error("native compilation failed")]
    Compilation,
    /// A previous failed invocation left synchronization state unusable.
    #[error("driver state is unavailable")]
    State,
    /// Crate or dependency arguments violate the native invocation contract.
    #[error("invalid native invocation: {0}")]
    Invocation(String),
}

/// Inject the prevalidated declarations after parsing an empty Incan crate root.
struct Callbacks {
    /// The unstable features the unit's description declares; this session enables them and no others.
    features: Vec<String>,
    plan: Arc<Plan>,
    library_facades: Vec<crate::library_exports::LibraryFacade>,
    search: Vec<rustc_session::search_paths::SearchPath>,
    shared: Vec<String>,
    development: bool,
    loaded: Vec<(PathBuf, String)>,
    timing: crate::native_timing::Timing,
    rustc_started: std::time::Instant,
    codegen_started: Option<std::time::Instant>,
    /// A program has no consumers of its item paths, so its callables may live in per-file modules. A library's paths
    /// are published, and a test harness selects each test by its module path.
    partition: bool,
    /// Record-backed native definitions and their complete per-definition tracked inputs.
    reader: Arc<PlanReader>,
}

/// Compile a plan supplied directly by an Incan caller function into a native binary.
///
/// The caller supplies the exact toolchain/library identity already verified before checking. External crates are
/// explicit rlib paths; their metadata resolves canonical callees. No generated Rust source or ambient unstable grant
/// is used: the unstable features and toolchain libraries a unit uses are the ones its description declares (`access`).
/// `INCAN_DIRECT_VALIDATION_REPORT` optionally records each function's first preflight refusal before normal
/// validation. A library supplies its already checked public facades as compact aliases of canonical native items,
/// without retaining its frontend manifest. Development executables retain debug information for edit-time
/// diagnostics; release executables omit it, while libraries retain metadata for their downstream consumers.
/// A development store derives shared carriers from those same native identities and records loader witnesses;
/// ordinary builds and release compilation do not select that policy.
/// `overflow_checks` is the unit profile's integer overflow policy, the one the lowering already applied to the plan;
/// rustc applies the same policy to the arithmetic it compiles from Rust crates, such as inlined standard library code.
pub fn compile(
    plan: Plan,
    crate_name: &str,
    output: &Path,
    identity: &DriverIdentity,
    externs: &[(String, PathBuf)],
    dependency_search_paths: &[PathBuf],
    rustc_args: &[String],
    access: &ToolchainAccess,
    shared_store: Option<&Path>,
    kind: UnitKind,
    overflow_checks: bool,
    library_facades: Vec<crate::library_exports::LibraryFacade>,
    plan_record_report: Option<&Path>,
) -> Result<(), DriverError> {
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::native_compile", input = %output.display(), functions = plan.functions.len()).entered();
    let mut timing = crate::native_timing::Timing::requested();
    let features = declared_features(access)?;

    // ---- Validated source plan ----
    if let Some(path) = std::env::var_os("INCAN_DIRECT_VALIDATION_REPORT") {
        let mut report = String::new();
        for refusal in validation::refusals(&plan, kind != UnitKind::Program) {
            report.push_str(&refusal.to_string().replace('\n', " "));
            report.push('\n');
        }
        std::fs::write(path, report)
            .map_err(|error| DriverError::Invocation(format!("cannot write native validation report: {error}")))?;
    }
    validation::validate(&plan, kind != UnitKind::Program)?;
    if crate_name.is_empty() || !crate_name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        return Err(DriverError::Invocation("crate name must be a Rust identifier".into()));
    }
    timing.mark("validated_plan");

    // ---- Optional shared native dependencies ----
    let _compilation = COMPILATION.lock().map_err(|_| DriverError::State)?;
    if shared_store.is_some() && kind == UnitKind::Library {
        return Err(DriverError::Invocation(
            "shared native linking requires a development executable".into(),
        ));
    }
    let shared = match shared_store {
        Some(store) => {
            let mut roots = native_roots(&plan);
            if kind == UnitKind::Test {
                roots.insert("test");
            }
            crate::native_shared::libraries(roots, externs, dependency_search_paths, identity, store)?
        }
        None => Vec::new(),
    };
    timing.mark("shared_dependencies");

    // ---- Fresh rustc session and planned MIR ----
    let plan = Arc::new(plan);
    let reader = Arc::new(PlanReader::new(Arc::clone(&plan), plan_record_report).map_err(DriverError::Invocation)?);
    reader.write_fingerprint_report()?;
    *PLAN.lock().map_err(|_| DriverError::State)? = Some(Arc::clone(&reader));
    *FAILURE.lock().map_err(|_| DriverError::State)? = None;
    let mut args = compilation_arguments(crate_name, output, identity, kind, overflow_checks);
    let all = externs.iter().chain(&shared).cloned().collect::<Vec<_>>();
    for (name, path) in &all {
        args.push("--extern".into());
        args.push(format!("{name}={}", path.display()));
    }
    let search = crate::native_search::dependencies(&all, dependency_search_paths);
    if let Some(store) = shared_store {
        args.extend(["-Cprefer-dynamic".into(), "-Crpath".into()]);
        // Rustc already constructs the native Darwin invocation; bypass the C driver's extra process for edits.
        // The adapter interposes its own executable only to give every object a content-derived store path before
        // forwarding the otherwise unchanged argument vector to ld. Darwin records those paths in the debug map.
        let darwin = identity.target.ends_with("-apple-darwin");
        if darwin {
            let linker = std::env::current_exe().map_err(|error| {
                DriverError::Invocation(format!("cannot locate stable development linker: {error}"))
            })?;
            args.push(format!("-Clinker={}", linker.display()));
            args.push("-Clinker-flavor=ld".into());
            args.push(format!("-Clink-arg={}{}", stable_link::STORE_ARGUMENT, store.display()));
        }
        // The executable is linked in the store and run from a copy elsewhere, so its libraries are also found
        // through absolute search paths, which do not depend on where the copy lives.
        args.extend(absolute_rpaths(&all, identity, darwin));
    }
    if access.links_sysroot() && kind != UnitKind::Library {
        args.extend(sysroot_linkage(identity));
    }
    args.extend_from_slice(rustc_args);
    if NATIVE_INCREMENTAL_QUALIFIED && kind == UnitKind::Program && overflow_checks && reader.has_record_inputs() {
        if let Some(report) = plan_record_report {
            args.retain(|argument| !argument.starts_with("-Cmetadata=incan-unit-"));
            let parent = report
                .parent()
                .ok_or_else(|| DriverError::Invocation("native plan report has no private unit directory".into()))?;
            let compiler = incan_semantics_core::semantic_digest::semantic_digest(&identity.toolchain)
                .map_err(|error| DriverError::Invocation(format!("cannot fingerprint compiler identity: {error}")))?;
            let incremental = parent.join(format!(
                ".{crate_name}.{}.rustc-incremental",
                compiler.trim_start_matches("sha256:")
            ));
            std::fs::create_dir_all(&incremental).map_err(|error| {
                DriverError::Invocation(format!("cannot create private incremental directory: {error}"))
            })?;
            let metadata = incan_semantics_core::semantic_digest::semantic_digest(&vec![
                crate_name.to_owned(),
                identity.toolchain.clone(),
                identity.target.clone(),
            ])
            .map_err(|error| DriverError::Invocation(format!("cannot fingerprint crate identity: {error}")))?;
            args.push("-C".into());
            args.push(format!("metadata={}", metadata.trim_start_matches("sha256:")));
            args.push("-C".into());
            args.push(format!("incremental={}", incremental.display()));
        }
    }
    timing.mark("native_arguments");
    let mut callbacks = Callbacks {
        features,
        plan,
        library_facades,
        search,
        shared: shared.iter().map(|(name, _)| name.clone()).collect(),
        development: shared_store.is_some(),
        loaded: Vec::new(),
        timing,
        rustc_started: std::time::Instant::now(),
        codegen_started: None,
        partition: kind == UnitKind::Program,
        reader,
    };
    let capture = crate::native_timing::Capture::begin();
    let result = rustc_driver::catch_fatal_errors(|| rustc_driver::run_compiler(&args, &mut callbacks));
    capture.finish(callbacks.codegen_started.map(|started| started.elapsed()));
    callbacks.timing.mark("rustc_codegen_link_teardown");
    *PLAN.lock().map_err(|_| DriverError::State)? = None;
    if let Some(error) = FAILURE.lock().map_err(|_| DriverError::State)?.take() {
        return Err(DriverError::Plan(error));
    }

    // ---- Completed loader witnesses ----
    result.map_err(|_| DriverError::Compilation)?;
    if shared_store.is_some() {
        crate::native_shared::write_witness(output, &callbacks.loaded)?;
    }
    callbacks.timing.mark("loader_witness");
    Ok(())
}

/// Preserve the ordinary native argument sequence; optional development linkage is appended only by its caller.
/// Overflow checks follow the unit profile: on in debug, off in release.
fn compilation_arguments(
    crate_name: &str,
    output: &Path,
    identity: &DriverIdentity,
    kind: UnitKind,
    overflow_checks: bool,
) -> Vec<String> {
    let overflow = if overflow_checks { "yes" } else { "no" };
    let debuginfo = debuginfo(kind, overflow_checks);
    let mut args = vec![
        "incan-rustc-driver".into(),
        "--sysroot".into(),
        identity.sysroot.to_string_lossy().into_owned(),
        "--crate-name".into(),
        crate_name.into(),
        "--edition".into(),
        "2024".into(),
        "--cap-lints".into(),
        "allow".into(),
        "-C".into(),
        format!("overflow-checks={overflow}"),
        "-C".into(),
        "opt-level=3".into(),
        "-C".into(),
        format!("debuginfo={debuginfo}"),
        "-o".into(),
        output.to_string_lossy().into_owned(),
        // A named input is never read: the callback installs the source-free root without consuming stdin. Its name
        // is stable because authored spans are loaded from the plan and remapped independently.
        SYNTHETIC_SOURCE_NAME.into(),
    ];
    if kind == UnitKind::Library {
        args.extend(["--crate-type".into(), "lib".into()]);
    }
    if kind == UnitKind::Test {
        args.push("--test".into());
    }
    if kind != UnitKind::Library && debuginfo != "0" {
        args.push("-Csplit-debuginfo=unpacked".into());
    }
    args
}

/// Select full debug metadata for libraries and development executables, but omit metadata stripped from release
/// executables.
fn debuginfo(kind: UnitKind, overflow_checks: bool) -> &'static str {
    if kind == UnitKind::Library || overflow_checks {
        "2"
    } else {
        "0"
    }
}

/// The unstable features a unit's description declares, each a Rust identifier the crate root enables.
fn declared_features(access: &ToolchainAccess) -> Result<Vec<String>, DriverError> {
    let identifier = |name: &str| {
        name.chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    };
    access
        .unstable_features
        .iter()
        .map(|name| {
            if identifier(name) {
                Ok(name.clone())
            } else {
                Err(DriverError::Invocation(format!(
                    "unit declares unstable feature `{name}`, which is not a Rust identifier"
                )))
            }
        })
        .collect()
}

/// An executable that links declared toolchain libraries loads them from the verified sysroot at run time, with the
/// standard library they were built with. Rustc links every crate it has as a shared library dynamically, and the
/// runtime search path names the sysroot's `lib` before its target libraries, so a toolchain library loaded from `lib`
/// finds the libraries it loads itself.
fn sysroot_linkage(identity: &DriverIdentity) -> Vec<String> {
    let libraries = identity.sysroot.join("lib");
    let target = identity.sysroot.join("lib/rustlib").join(&identity.target).join("lib");
    let mut args = vec![
        "-Cprefer-dynamic".to_owned(),
        format!("-Lnative={}", libraries.display()),
    ];
    for directory in [&libraries, &target] {
        args.push(format!("-Clink-arg=-Wl,-rpath,{}", directory.display()));
    }
    args
}

/// Absolute run-time search paths for a shared-linkage executable: each shared library's directory and the
/// toolchain's own library directory, so a copy of the executable loads them wherever it is placed.
fn absolute_rpaths(linked: &[(String, PathBuf)], identity: &DriverIdentity, darwin: bool) -> Vec<String> {
    let mut directories: Vec<PathBuf> = vec![identity.sysroot.join("lib/rustlib").join(&identity.target).join("lib")];
    for (_, path) in linked {
        let shared = matches!(path.extension().and_then(|e| e.to_str()), Some("dylib" | "so"));
        if let (true, Some(parent)) = (shared, path.parent()) {
            if !directories.iter().any(|known| known == parent) {
                directories.push(parent.to_path_buf());
            }
        }
    }
    let mut args = Vec::new();
    for directory in directories {
        if darwin {
            args.push("-Clink-arg=-rpath".into());
            args.push(format!("-Clink-arg={}", directory.display()));
        } else {
            args.push(format!("-Clink-arg=-Wl,-rpath,{}", directory.display()));
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::{UnitKind, debuginfo};

    /// Release executables omit debug metadata while development executables and libraries retain it.
    #[test]
    fn debug_metadata_follows_unit_contract() {
        assert_eq!(debuginfo(UnitKind::Program, false), "0");
        assert_eq!(debuginfo(UnitKind::Test, false), "0");
        assert_eq!(debuginfo(UnitKind::Program, true), "2");
        assert_eq!(debuginfo(UnitKind::Library, false), "2");
    }
}
