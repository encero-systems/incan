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
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

mod active_plan;
mod callbacks;
mod completed_codegen;
mod library_verification;
mod native_object_products;
mod rustc_queries;
pub(crate) mod stable_link;

pub(crate) use active_plan::PlanReader;
use library_verification::*;
pub(crate) use native_object_products::PREDECESSOR_LOCATOR;
pub(crate) use rustc_queries::*;

/// Serialize native compiles and the typed failure and extern-binding state they share.
static COMPILATION: Mutex<()> = Mutex::new(());
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
    /// The link's uniquely issued file and any earlier group admitted under the same private holder.
    completed_objects: Option<(std::fs::File, BTreeMap<String, stable_link::PreviousObject>)>,
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
    compact_definitions: Vec<incan_mir_plan_native::AdmittedDefinition>,
    previous_link_locator: Option<&Path>,
) -> Result<(), DriverError> {
    ensure_no_active_plan()?;
    let mut timing = crate::native_timing::Timing::requested();
    force_plan_failure_once()?;
    let features = declared_features(access)?;

    // ---- Validated source plan ----
    write_validation_report(&plan, kind)?;
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
    let reader = Arc::new(
        PlanReader::new(Arc::clone(&plan), plan_record_report, compact_definitions).map_err(DriverError::Invocation)?,
    );
    reader.write_fingerprint_report()?;
    *FAILURE.lock().map_err(|_| DriverError::State)? = None;
    let all = externs.iter().chain(&shared).cloned().collect::<Vec<_>>();
    let mut args = compilation_arguments(crate_name, output, identity, kind, overflow_checks, &all);
    let search = crate::native_search::dependencies(&all, dependency_search_paths);
    let links_sysroot = access.links_sysroot() && kind != UnitKind::Library;
    append_native_linkage(&mut args, &shared, identity, shared_store, links_sysroot)?;
    args.extend_from_slice(rustc_args);
    let private = match plan_record_report {
        Some(report)
            if NATIVE_INCREMENTAL_QUALIFIED
                && kind == UnitKind::Program
                && overflow_checks
                && reader.has_record_inputs() =>
        {
            native_incremental(
                &mut args,
                crate_name,
                &identity.toolchain,
                &identity.target,
                report,
                &all,
            )?
        }
        _ => None,
    };
    let stores_objects = shared_store.is_some() && identity.target.ends_with("-apple-darwin");
    let mut completed = native_object_products::LinkCapture::issue(
        private.as_ref().zip(shared_store).filter(|_| stores_objects),
        &mut args,
        previous_link_locator,
    );
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
        completed_objects: completed.as_mut().and_then(|capture| capture.backend_input()),
        partition: kind == UnitKind::Program,
        reader,
    };
    run_plan_compiler(&args, &mut callbacks)?;

    // ---- Completed loader witnesses ----
    if let Some(held) = &private {
        // A development link on Darwin has already stored every unit object by its content.
        held.publish(output, stores_objects)?;
    }
    if shared_store.is_some() {
        crate::native_shared::write_witness(output, &callbacks.loaded)?;
    }
    if let Some(capture) = completed {
        capture.store_result(output);
    }
    callbacks.timing.mark("loader_witness");
    Ok(())
}

/// Keep the optional first-refusal report ahead of the normal validation result.
fn write_validation_report(plan: &Plan, kind: UnitKind) -> Result<(), DriverError> {
    if let Some(path) = std::env::var_os("INCAN_DIRECT_VALIDATION_REPORT") {
        let mut report = String::new();
        for refusal in validation::refusals(plan, kind != UnitKind::Program) {
            report.push_str(&refusal.to_string().replace('\n', " "));
            report.push('\n');
        }
        std::fs::write(path, report)
            .map_err(|error| DriverError::Invocation(format!("cannot write native validation report: {error}")))?;
    }
    Ok(())
}

/// Run the ordinary compiler and collect its typed failure while the caller holds the compilation lease.
fn run_plan_compiler(args: &[String], callbacks: &mut Callbacks) -> Result<(), DriverError> {
    let capture = crate::native_timing::Capture::begin();
    crate::native_crates::release_extern_crates();
    let result = rustc_driver::catch_fatal_errors(|| rustc_driver::run_compiler(args, callbacks));
    crate::native_crates::release_extern_crates();
    capture.finish(callbacks.codegen_started.map(|started| started.elapsed()));
    callbacks.timing.mark("rustc_codegen_link_teardown");
    if let Some(error) = FAILURE.lock().map_err(|_| DriverError::State)?.take() {
        return Err(DriverError::Plan(error));
    }
    result.map_err(|_| DriverError::Compilation)
}

/// Append the selected development and toolchain link flags in their ordinary rustc argument order.
fn append_native_linkage(
    args: &mut Vec<String>,
    shared: &[(String, PathBuf)],
    identity: &DriverIdentity,
    shared_store: Option<&Path>,
    links_sysroot: bool,
) -> Result<(), DriverError> {
    if let Some(store) = shared_store {
        // Rustc's relative `-Crpath` entries would name the store from the staged output, not from the viewed copy.
        args.push("-Cprefer-dynamic".into());
        // Rustc already constructs the native Darwin invocation; bypass the C driver's extra process for edits.
        // The development linker gives every object a content-derived store path before forwarding the otherwise
        // unchanged argument vector to ld. Darwin records those paths in the debug map.
        let darwin = identity.target.ends_with("-apple-darwin");
        if darwin {
            let linker = development_linker().map_err(|error| {
                DriverError::Invocation(format!("cannot locate stable development linker: {error}"))
            })?;
            args.push(format!("-Clinker={}", linker.display()));
            args.push("-Clinker-flavor=ld".into());
            args.push(format!("-Clink-arg={}{}", stable_link::STORE_ARGUMENT, store.display()));
        }
        // The executable is linked in the bakery and run from a copy elsewhere, so its carriers are found through
        // absolute search paths into the bakery's shared native store, which every home on that bakery reads.
        args.extend(absolute_rpaths(shared, identity, darwin));
    }
    if links_sysroot {
        args.extend(sysroot_linkage(identity));
    }
    Ok(())
}

/// Use a stable crate identity for a program whose every function has a record input. Only the holder of its
/// private output uses the associated incremental session. A contender still compiles to its requested output.
/// The session is named by the crates the program links (`linked`) as well, see [`incremental_session`].
fn native_incremental(
    args: &mut Vec<String>,
    crate_name: &str,
    toolchain: &str,
    target: &str,
    report: &Path,
    linked: &[(String, PathBuf)],
) -> Result<Option<PrivateOutput>, DriverError> {
    args.retain(|argument| !argument.starts_with("-Cmetadata=incan-unit-"));
    let parent = report
        .parent()
        .ok_or_else(|| DriverError::Invocation("native plan report has no private unit directory".into()))?;
    // Older writers must not populate or retire the receipt-compatible session or its private link output.
    let compiler = incan_semantics_core::semantic_digest::semantic_digest(&("incan.native-objects/1", toolchain))
        .map_err(|error| DriverError::Invocation(format!("cannot fingerprint compiler identity: {error}")))?;
    let compiler = compiler.trim_start_matches("sha256:");
    let mut private = PrivateOutput::acquire(parent, crate_name, compiler)?;
    let metadata = incan_semantics_core::semantic_digest::semantic_digest(&vec![
        crate_name.to_owned(),
        toolchain.to_owned(),
        target.to_owned(),
    ])
    .map_err(|error| DriverError::Invocation(format!("cannot fingerprint crate identity: {error}")))?;
    args.push("-C".into());
    args.push(format!("metadata={}", metadata.trim_start_matches("sha256:")));
    if let Some(held) = &mut private {
        let incremental = incremental_session(parent, crate_name, compiler, linked)?;
        args.push("-C".into());
        args.push(format!("incremental={}", incremental.display()));
        held.session = Some(incremental);
        held.redirect(args);
    }
    Ok(private)
}

/// The incremental session directory of a program: named by its crate, its compiler and the crates it links, each by
/// its name, physical path and file generation.
///
/// rustc cannot load a session that names a crate whose identity changed since: a Rust facet built under another
/// `-C metadata` (as when `INCAN_RUST_INCREMENTAL` changes) or an edited Incan library has another `StableCrateId`, and
/// marking the earlier session's nodes green stops rustc ("uninterned StableCrateId"). A changed linked crate
/// therefore starts a new session. A compilation that holds the program's private output removes the sessions named
/// for other crates, except one a compilation is still working in.
fn incremental_session(
    parent: &Path,
    crate_name: &str,
    compiler: &str,
    linked: &[(String, PathBuf)],
) -> Result<PathBuf, DriverError> {
    let mut crates = linked
        .iter()
        .map(|(name, path)| {
            let physical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            let generation = crate::native_search::file_identity(&physical).unwrap_or_default();
            format!("{name}\t{}\t{generation}", physical.display())
        })
        .collect::<Vec<_>>();
    crates.sort();
    let digest = incan_semantics_core::semantic_digest::semantic_digest(&crates)
        .map_err(|error| DriverError::Invocation(format!("cannot fingerprint linked crates: {error}")))?;
    let prefix = format!(".{crate_name}.{compiler}.");
    let name = format!(
        "{prefix}{}.rustc-incremental",
        &digest.trim_start_matches("sha256:")[..16]
    );
    for entry in std::fs::read_dir(parent).into_iter().flatten().flatten() {
        let other = entry.file_name().to_string_lossy().into_owned();
        let earlier = other != name && other.starts_with(&prefix) && other.ends_with(".rustc-incremental");
        if earlier && !working(&entry.path()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    let session = parent.join(name);
    std::fs::create_dir_all(&session)
        .map_err(|error| DriverError::Invocation(format!("cannot create private incremental directory: {error}")))?;
    Ok(session)
}

/// Whether a compilation is working in an incremental session directory: rustc keeps each crate's sessions in a
/// directory of their own and names the one it works in `s-...-working` until it finishes.
fn working(session: &Path) -> bool {
    std::fs::read_dir(session)
        .into_iter()
        .flatten()
        .flatten()
        .any(|crate_sessions| {
            std::fs::read_dir(crate_sessions.path())
                .into_iter()
                .flatten()
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().ends_with("-working"))
        })
}

/// The private output of a program compiled with native incremental reuse, named by its crate and compiler alone.
/// rustc's output filenames are an input of every codegen unit, so compiling to the build's own staging path would
/// make every unit's code stale on every edit; the executable is published to the requested output afterwards. The
/// lock keeps a concurrent compilation of the same crate on its own requested output.
struct PrivateOutput {
    path: PathBuf,
    _lock: std::fs::File,
    session: Option<PathBuf>,
}

impl PrivateOutput {
    /// Hold the private output beside the unit's incremental directory, or none while another compilation holds it.
    fn acquire(parent: &Path, crate_name: &str, compiler: &str) -> Result<Option<Self>, DriverError> {
        let failed =
            |error: std::io::Error| DriverError::Invocation(format!("cannot hold private native output: {error}"));
        let directory = parent.join(format!(".{crate_name}.{compiler}.rustc-output"));
        std::fs::create_dir_all(&directory).map_err(failed)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(directory.join(".lock"))
            .map_err(failed)?;
        match lock.try_lock() {
            Ok(()) => Ok(Some(Self {
                path: directory.join(crate_name),
                _lock: lock,
                session: None,
            })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(failed(error)),
        }
    }

    /// Point rustc's output argument at the private output.
    fn redirect(&self, args: &mut [String]) {
        if let Some(position) = args.iter().position(|argument| argument == "-o")
            && let Some(slot) = args.get_mut(position + 1)
        {
            *slot = self.path.to_string_lossy().into_owned();
        }
    }

    /// Move the executable to the requested output, copying it across volumes. When the development link has stored
    /// every unit object by its content, the objects are removed: each is a hard link to a work product the
    /// incremental session keeps, which the next build's codegen would otherwise write over in place.
    fn publish(&self, output: &Path, objects_stored: bool) -> Result<(), DriverError> {
        let failed =
            |error: std::io::Error| DriverError::Invocation(format!("cannot publish private native output: {error}"));
        match std::fs::rename(&self.path, output) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                std::fs::copy(&self.path, output).map_err(failed)?;
                std::fs::remove_file(&self.path).map_err(failed)?;
            }
            Err(error) => return Err(failed(error)),
        }
        let Some(directory) = self.path.parent().filter(|_| objects_stored) else {
            return Ok(());
        };
        for entry in std::fs::read_dir(directory).map_err(failed)? {
            let path = entry.map_err(failed)?.path();
            if path.extension().is_some_and(|extension| extension == "o") {
                std::fs::remove_file(&path).map_err(failed)?;
            }
        }
        Ok(())
    }
}

/// Force one malformed-plan failure for the end-to-end refusal-retention contract, consuming the named sentinel.
/// An absent sentinel leaves ordinary compilation unchanged; a sentinel that cannot be consumed is a host failure.
fn force_plan_failure_once() -> Result<(), DriverError> {
    let Some(path) = std::env::var_os("INCAN_TEST_PLAN_FAILURE_ONCE") else {
        return Ok(());
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Err(DriverError::Plan(PlanError::Invalid {
            function: "test seam".into(),
            reason: "forced toolchain plan failure".into(),
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DriverError::Invocation(format!(
            "cannot consume one-shot plan failure {}: {error}",
            Path::new(&path).display()
        ))),
    }
}

/// The program a development link runs as its linker: the dedicated `incan-link` installed beside this driver under
/// the same suffix, or this driver itself where none is installed. The dedicated program is small and never relinked,
/// so a link pays neither the driver's start nor a first-run check.
pub(crate) fn development_linker() -> std::io::Result<PathBuf> {
    let driver = std::env::current_exe()?;
    if let Some(suffix) = driver
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("incan-rustc-driver"))
    {
        let linker = driver.with_file_name(format!("incan-link{suffix}"));
        if linker.is_file() {
            return Ok(linker);
        }
    }
    Ok(driver)
}

/// Preserve the ordinary native argument sequence; optional development linkage is appended only by its caller.
/// Overflow checks follow the unit profile: on in debug, off in release.
fn compilation_arguments(
    crate_name: &str,
    output: &Path,
    identity: &DriverIdentity,
    kind: UnitKind,
    overflow_checks: bool,
    externs: &[(String, PathBuf)],
) -> Vec<String> {
    let overflow = if overflow_checks { "yes" } else { "no" };
    let optimization = optimization_level(overflow_checks);
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
        format!("opt-level={optimization}"),
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
    for (name, path) in externs {
        args.push("--extern".into());
        args.push(format!("{name}={}", path.display()));
    }
    args
}

/// Select unoptimized native code for the debug profile and optimized native code for the release profile.
fn optimization_level(overflow_checks: bool) -> &'static str {
    if overflow_checks { "0" } else { "3" }
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

/// Absolute run-time search paths for a shared-linkage executable: the directory of each shared carrier, in the
/// bakery's shared native store, and the toolchain's standard library directory. A viewed copy of the executable loads
/// them wherever it is placed, and no path names the home that linked it. Proc-macro and other compile-time
/// libraries are never loaded at run time, so their directories are not searched.
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
    use super::{PrivateOutput, UnitKind, debuginfo, incremental_session, native_incremental, optimization_level};
    use std::path::{Path, PathBuf};

    const CALLABLE_LENDING_SOURCE: &str = r#"def twice(callback: (int) -> bool) -> bool:
    if callback(3):
        return callback(5)
    return false

pub def forward(callback: (int) -> bool) -> bool:
    return twice(callback)

pub def read(value: Borrowed[int]) -> bool:
    show: (int) -> bool = (item) => value < item
    if show(1):
        return true
    return twice(show)

class Cart:
    """Own the list the closure updates."""

    pub items: list[str]

pub def add_twice(mut cart: Cart) -> None:
    """Call the same writable closure twice."""
    mut items = cart.items
    add: (str) -> None = (item) => items.append(item)
    add("plum")
    add("fig")
    println(cart.items)

def call_twice(callback: (str) -> None) -> None:
    """Invoke the callback twice without retaining it."""
    callback("plum")
    callback("fig")

pub def forward_twice(mut cart: Cart) -> None:
    """Observe the list after the forwarded writable callable's final use."""
    mut items = cart.items
    add: (str) -> None = (item) => items.append(item)
    call_twice(add)
    call_twice(add)
    println(cart.items)

pub def forward_in_one_arm(mut cart: Cart, flag: bool) -> None:
    """Read the owner after forwarding on one continuing arm."""
    mut items = cart.items
    add: (str) -> None = (item) => items.append(item)
    if flag:
        call_twice(add)
        println(cart.items)
    println(cart.items)

pub def owned(limit: int) -> bool:
    """Call an erased closure that captures an owned scalar."""
    permits: (int) -> bool = (item) => limit < item
    return permits(5)
"#;

    /// Compile direct writable, lent shared, and cloneable erased callable environments.
    #[test]
    fn callable_lending_preserves_native_capture_access() -> Result<(), Box<dyn std::error::Error>> {
        use incan_frontend::body_ir::{BodyIrContext, ContextModules, ProviderOperationCatalog, build_body_ir_module};
        use incan_frontend::{lexer, parser, typechecker::TypeChecker};
        use incan_lowering::{PlanAdmissions, SourceInputs, UnitPlanning, unit_plan};

        let source = CALLABLE_LENDING_SOURCE;
        let directory = tempfile::tempdir()?;
        let file = directory.path().join("lib.incn");
        std::fs::write(&file, source)?;
        let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let providers = ProviderOperationCatalog::new();
        let module = build_body_ir_module(
            &program,
            &[],
            checker.type_info(),
            &BodyIrContext {
                modules: ContextModules::default(),
                provider_operations: &providers,
                registry_package: None,
                library_package: None,
                include_inline_tests: false,
            },
        );
        let inputs = SourceInputs {
            sources: vec![source.to_owned()],
            files: vec![file.to_string_lossy().into_owned()],
            ..Default::default()
        };
        let UnitPlanning::Complete(Some(plan)) = unit_plan(
            &[module],
            &inputs,
            0,
            "library",
            &mut Vec::new(),
            &mut PlanAdmissions::default(),
        )?
        else {
            return Err("missing native callable plan".into());
        };
        assert_callable_environments(&plan)?;
        assert!(
            plan.externals
                .iter()
                .any(|external| external.path == "incan_native_runtime::append_list"),
            "missing the native list append"
        );
        let unit = describe_callable_fixture(&file)?;
        let dependencies = crate::frontend::dependencies(&unit, &[])?;
        let output = directory.path().join("libcallable_lending.rlib");
        super::compile(
            plan,
            "callable_lending",
            &output,
            &crate::identity::verify_loaded_test_sysroot()?,
            &dependencies.externs,
            &dependencies.directories,
            &[],
            &unit.toolchain_access,
            None,
            UnitKind::Library,
            false,
            Vec::new(),
            None,
            Vec::new(),
            None,
        )?;
        assert!(output.is_file());
        Ok(())
    }

    /// Use the ordinary unit description to admit derives and runtime libraries for the fixture.
    fn describe_callable_fixture(
        source: &Path,
    ) -> Result<incan_driver::unit::UnitDescription, Box<dyn std::error::Error>> {
        let selected_path = |variable| {
            std::env::var_os(variable)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        };
        let oven = if let Some(engine) = selected_path("OVEN_ENGINE_BINARY") {
            engine.with_file_name("oven")
        } else {
            let home = selected_path("INCAN_HOME")
                .or_else(|| {
                    ["HOME", "USERPROFILE"]
                        .into_iter()
                        .find_map(selected_path)
                        .map(|home| home.join(".incan"))
                })
                .ok_or("no Incan home for the callable fixture")?;
            home.join("bin").join("oven")
        };
        let output = std::process::Command::new(&oven)
            .arg("describe-unit")
            .arg(source)
            .args(["--unattended", "--profile", "release"])
            .output()
            .map_err(|error| format!("cannot run {} for the callable fixture: {error}", oven.display()))?;
        if !output.status.success() {
            return Err(format!(
                "could not describe callable fixture: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(incan_driver::unit::UnitDescription::parse(&output.stdout, source)?)
    }

    /// Check the actual native carriers and captures before compiling the fixture.
    fn assert_callable_environments(plan: &crate::plan::Plan) -> Result<(), Box<dyn std::error::Error>> {
        use crate::plan::PlanType;

        let helper = plan
            .functions
            .iter()
            .find(|function| function.name == "twice")
            .ok_or("missing call-only helper")?;
        assert!(
            matches!(helper.parameters.as_slice(), [parameter] if matches!(parameter.ty, PlanType::ExclusiveClosure(_)))
        );
        for owner in ["read", "forward"] {
            let constructor_prefix = format!("__incan_borrowed_callable_{owner}_");
            let envelope = plan
                .functions
                .iter()
                .find(|function| function.name.starts_with(&constructor_prefix) && function.name.ends_with("_new"))
                .ok_or_else(|| {
                    let names: Vec<_> = plan
                        .functions
                        .iter()
                        .filter_map(|function| {
                            function
                                .name
                                .starts_with("__incan_borrowed_callable_")
                                .then_some(&function.name)
                        })
                        .collect();
                    format!("missing {owner} lending envelope among {names:?}")
                })?;
            assert!(
                matches!(envelope.return_type, PlanType::ExclusiveClosure(_)),
                "{} has lending carrier {:?}",
                envelope.name,
                envelope.return_type
            );
            let [capture] = envelope.parameters.as_slice() else {
                return Err(format!("{} has {} captures", envelope.name, envelope.parameters.len()).into());
            };
            assert!(
                matches!(capture.ty, PlanType::ClosureRef(_) | PlanType::BorrowingClosureRef(_)),
                "{} capture is {:?}",
                envelope.name,
                capture.ty
            );
        }
        let write = plan
            .functions
            .iter()
            .find(|function| function.name.starts_with("__incan_closure_add_twice_") && function.name.ends_with("_new"))
            .ok_or("missing direct writable constructor")?;
        assert!(matches!(write.return_type, PlanType::ExclusiveClosure(_)));
        assert!(
            matches!(write.parameters.as_slice(), [capture] if crate::plan::exclusive_capture(&capture.ty)),
            "direct writable constructor must retain an exclusive capture"
        );
        assert!(plan.functions.iter().any(|function| {
            function.name.starts_with("__incan_closure_owned_")
                && function.name.ends_with("_new")
                && matches!(function.return_type, PlanType::Closure(_))
                && matches!(function.parameters.as_slice(), [capture] if capture.ty == PlanType::Int)
        }));
        Ok(())
    }

    /// Two builds of one program compile to the same private output whatever output each was asked for, so rustc's
    /// output filenames, an input of every codegen unit, are the same between edits; one compilation holds it at a
    /// time.
    #[test]
    fn a_program_with_native_reuse_compiles_to_one_private_output() -> Result<(), Box<dyn std::error::Error>> {
        let units = tempfile::tempdir()?;
        let arguments = |requested: &str| {
            vec![
                "incan-rustc-driver".to_owned(),
                "-o".to_owned(),
                requested.to_owned(),
                "lib.incn".to_owned(),
            ]
        };
        let report = units.path().join("native-report");
        let prepare = |args: &mut Vec<String>| native_incremental(args, "oven", "pinned-rustc", "target", &report, &[]);
        let (mut first, mut contender, mut second) = (
            arguments("/staging/one/oven"),
            arguments("/staging/contender/oven"),
            arguments("/staging/two/oven"),
        );
        let held = prepare(&mut first)?.ok_or("the private output is free")?;
        let incremental_arguments = |args: &[String]| {
            args.iter()
                .filter(|argument| argument.starts_with("incremental="))
                .count()
        };
        assert_eq!(incremental_arguments(&first), 1);
        assert!(prepare(&mut contender)?.is_none());
        assert_eq!(contender[2], "/staging/contender/oven");
        assert_eq!(
            incremental_arguments(&contender),
            0,
            "a compilation without the native output lease must not write its predecessor's incremental root"
        );
        drop(held);
        let _again = prepare(&mut second)?.ok_or("the private output is free again")?;
        assert_eq!(first, second);
        assert!(Path::new(&first[2]).starts_with(units.path()));
        Ok(())
    }

    /// The executable reaches the requested output, and the unit objects the development link already stored by
    /// content do not stay behind to be written again in place by the next build.
    #[test]
    fn a_private_output_publishes_its_executable_and_keeps_no_stored_objects() -> Result<(), Box<dyn std::error::Error>>
    {
        let (units, staging) = (tempfile::tempdir()?, tempfile::tempdir()?);
        let held = PrivateOutput::acquire(units.path(), "oven", "abc")?.ok_or("the private output is free")?;
        let mut arguments = vec!["-o".to_owned(), String::new()];
        held.redirect(&mut arguments);
        let executable = Path::new(&arguments[1]).to_path_buf();
        std::fs::write(&executable, b"executable")?;
        let object = executable.with_file_name("oven.unit.rcgu.o");
        std::fs::write(&object, b"object")?;
        let requested = staging.path().join("oven");
        held.publish(&requested, true)?;
        assert_eq!(std::fs::read(&requested)?, b"executable");
        assert!(!executable.exists());
        assert!(!object.exists());
        Ok(())
    }

    /// A program's incremental session is named by the crates it links as well as by its crate and compiler: the same
    /// linked crates name the same session; a linked crate that changed, such as a Rust facet built under another
    /// `-C metadata`, names a new one, since rustc stops on a session naming a crate whose identity changed; and the
    /// session named for the earlier crates is removed.
    #[test]
    fn a_program_s_incremental_session_is_named_by_the_crates_it_links() -> Result<(), Box<dyn std::error::Error>> {
        let (units, crates) = (tempfile::tempdir()?, tempfile::tempdir()?);
        let library = crates.path().join("libfacet.rlib");
        std::fs::write(&library, b"facet built under one metadata")?;
        let linked = vec![("facet".to_owned(), library.clone())];
        let first = incremental_session(units.path(), "oven", "abc", &linked)?;
        assert!(first.is_dir());
        assert_eq!(incremental_session(units.path(), "oven", "abc", &linked)?, first);
        std::fs::write(&library, b"facet built under another metadata")?;
        let second = incremental_session(units.path(), "oven", "abc", &linked)?;
        assert_ne!(second, first, "a changed linked crate kept the earlier session");
        assert!(second.is_dir());
        assert!(!first.exists(), "the session named for the earlier crates was kept");
        Ok(())
    }

    /// Release executables omit debug metadata while development executables and libraries retain it.
    #[test]
    fn debug_metadata_follows_unit_contract() {
        assert_eq!(debuginfo(UnitKind::Program, false), "0");
        assert_eq!(debuginfo(UnitKind::Test, false), "0");
        assert_eq!(debuginfo(UnitKind::Program, true), "2");
        assert_eq!(debuginfo(UnitKind::Library, false), "2");
    }

    /// Debug compilation remains fast while release compilation retains optimized native code.
    #[test]
    fn optimization_follows_unit_profile() {
        assert_eq!(optimization_level(true), "0");
        assert_eq!(optimization_level(false), "3");
    }
}
