//! Shared CLI-session source-to-Body-IR glue. All Body-IR-to-plan decisions belong to the Incan lowering Loaf.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::dependency_report::RootOrigin;
use incan_driver::body_ir::{load_loaf_modules, required_loaf_identities};
use incan_driver::diagnostics::CliDiagnosticFailure;
use incan_driver::loaf_publication::{PublicationFacts, prepare_loaf_publication, publish_native_unions, write_loaf};
use incan_driver::modules::{collect_library_modules_detailed_with_session, collect_modules_detailed_with_session};
use incan_driver::session::{CompilationAnalysis, CompilationSession, RustFactsSetup};
use incan_driver::unit::{UnitDescription, UnitKind};
use incan_frontend::ast::{DecoratorArg, DecoratorArgValue, Expr, Literal};
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::loaf_index::{LoafIndex, LoafNativeClosure};
use incan_frontend::parsed_module::ParsedModule;
use incan_lang::lang::derives::{self, DeriveId};
use incan_loaf_names::native_crate_name;
use incan_mir_lowering::{
    RefusalPlanning, RuntimeRequirement, SourceInputs, UnitPlanning, layout_runtime_requirements, program_refusals,
    resume_unit_plan, unit_plan,
};
use incan_semantics_core::body_ir::{BodyIrModule, LoafCallable, NativeUnionDeclaration, NominalDeclaration};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolOrigin};

mod checked_program;
mod declaration_validation;

use checked_program::*;
use declaration_validation::*;

/// Environment variable naming the file where the census receives every refusal of one compilation, one per line.
///
/// Unset, compilation reports only its first refusal, as it always has. Set, the driver also lowers every body on its
/// own and writes each body's first refusal there before compiling as usual, so one refused body cannot hide another.
const REFUSAL_REPORT: &str = "INCAN_DIRECT_REFUSAL_REPORT";

/// An optional side channel for semantic CLI failures, distinct from unsupported lowering or malformed plans.
const SEMANTIC_REPORT: &str = "INCAN_DIRECT_SEMANTIC_REPORT";

/// The receipt `--unit --certify PATH` writes when unchanged inputs reproduce the compilation's refusal exactly.
const REFUSAL_CERTIFICATE: &str = "incan.compiler-refusal/1\n";

/// Where a compilation certifies a known deterministic refusal, which permits Oven to retain it; never an arbitrary
/// failure. Without `--certify PATH` it certifies nothing. The path travels with the request, so a short-lived process
/// and the resident worker certify the same outcomes.
#[derive(Clone)]
struct RefusalReceipt(Option<PathBuf>);

impl RefusalReceipt {
    /// Certify a refusal that unchanged inputs reproduce. A receipt that cannot be written only means the next
    /// unchanged compilation runs again.
    fn deterministic(&self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::write(path, REFUSAL_CERTIFICATE);
        }
    }

    /// Keep resource and source-reading failures retryable; only source diagnostics authorize replay.
    fn diagnostic(&self, failure: CliDiagnosticFailure) -> String {
        if failure.reproducible() {
            self.deterministic();
        }
        failure.render_human()
    }
}

/// A checked program failure that occurs before an executable can exist.
#[derive(Debug, thiserror::Error)]
enum SourceFailure {
    /// Feature projection and checked lowering completed, but the selected entry module declares no main.
    #[error("selected program has no main entrypoint")]
    MissingEntrypoint,
}

/// Bind a genuine missing-entrypoint outcome to the exact input, keeping both program streams free of report bytes.
fn report_missing_entrypoint(source: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = std::env::var_os(SEMANTIC_REPORT) else {
        return Ok(());
    };
    let source = std::fs::read_to_string(source)?;
    let report = serde_json::json!({
        "schema_version": 1,
        "kind": "missing_entrypoint",
        "source_digest": format!("sha256:{}", hex::encode(Sha256::digest(source.as_bytes()))),
    });
    std::fs::write(path, serde_json::to_vec(&report)?)?;
    Ok(())
}

/// Write refusals to the file [`REFUSAL_REPORT`] names, one per line; without the variable this does nothing.
fn write_refusal_report(refusals: &[String]) -> Result<(), std::io::Error> {
    let Some(path) = std::env::var_os(REFUSAL_REPORT) else {
        return Ok(());
    };
    let mut text = String::new();
    for refusal in refusals {
        text.push_str(&refusal.replace('\n', " "));
        text.push('\n');
    }
    std::fs::write(path, text)
}

/// Invocation-local source context retained for strictly growing runtime dependency requests. No attempt publishes
/// its partial plan, and imported declaration spans never acquire the entry source's text.
struct RuntimeDependencyContext {
    loaves: std::sync::Arc<LoafIndex>,
    requested: BTreeSet<CanonicalSymbolId>,
    /// Why each requested root is requested: checked source, a selected default or a runtime operation.
    origins: BTreeMap<CanonicalSymbolId, RootOrigin>,
    checked_local: BTreeSet<CanonicalSymbolId>,
    /// Exact selected versions from the authenticated executable-resolution closure.
    package_versions: BTreeMap<String, String>,
    package: String,
    /// Declaration-only runtime modules that stabilize lowering layouts without becoming dependency roots.
    layout_modules: Vec<BodyIrModule>,
    /// Whether `layout_modules` already occupy the active program's prepared prefix.
    layout_modules_installed: bool,
    sources: Vec<String>,
    files: Vec<String>,
    entry: usize,
}

/// Checked owning modules and their source provenance, with one explicit native entry module.
struct CheckedProgram {
    dependencies: RuntimeDependencyContext,
    modules: Vec<BodyIrModule>,
    sources: Vec<String>,
    files: Vec<String>,
    entry: usize,
    /// The first of the program's own modules; the Loaf declarations it reaches come before it.
    own: usize,
    /// Number of reached dependency modules at the front; prepared layout modules between this and `own` survive
    /// retries.
    dependency_modules: usize,
    /// Standard native closures constrain linking only when this checked declaration closure reaches their package.
    /// Each reached package keeps the requester chains of the declarations that entered it.
    reached_loaves: BTreeMap<String, Vec<Vec<CanonicalSymbolId>>>,
    /// A library unit's manifest, which its executable representation completes once the unit compiles (DD-0007).
    manifest: Option<LibraryManifest>,
    /// The owning library's portable graph bytes, encoded while its checked source context is still owned here.
    graph: Vec<u8>,
    /// Session-collected tests; lowering receives their names and Oven receives their outcome policy.
    tests: Vec<crate::test_unit::CollectedTest>,
}

/// Exact caller-declared native libraries and their dependency search directories.
pub(crate) struct Dependencies {
    /// The native libraries the unit links, as `--extern CRATE=RLIB` pairs.
    pub(crate) externs: Vec<(String, PathBuf)>,
    /// The directories that resolve those libraries' own dependencies.
    pub(crate) directories: Vec<PathBuf>,
    /// The ABI caches Oven names in the unit description: the toolchain's standard crates and each compiled unit's.
    pub(crate) rust_abi: Vec<PathBuf>,
    /// Compatibility for bootstrap invocations that select a library with `--library`.
    library: bool,
    /// Arguments Oven passes to rustc unchanged (`--rustc-arg`): the pinned target's link policy (DD-0008).
    rustc_args: Vec<String>,
    /// A development executable may link exact native carriers from this immutable store.
    shared_native: Option<PathBuf>,
}

impl Dependencies {
    /// Read shipped ABI caches first, retaining bounded fallback results under the installed native generation.
    pub(crate) fn rust_facts(&self, unit: &UnitDescription, sysroot: &Path) -> RustFactsSetup {
        RustFactsSetup {
            caches: self.rust_abi.clone(),
            source: Some(std::sync::Arc::new(crate::rust_facts::RustcFacts {
                unit: unit.clone(),
                sysroot: sysroot.to_path_buf(),
                externs: self.externs.clone(),
                search: self.directories.clone(),
            })),
        }
    }
}

/// Read exact artifacts from Oven's unit description and parse compilation options. Native inputs cannot be
/// overridden by command-line flags, so checking, facts extraction, and compilation share one source.
pub(crate) fn dependencies(unit: &UnitDescription, arguments: &[String]) -> Result<Dependencies, String> {
    let mut names = std::collections::BTreeSet::new();
    let mut externs = Vec::new();
    for native in &unit.externs {
        if !names.insert(&native.name) {
            return Err(format!("unit names more than one exact `{}` artifact", native.name));
        }
        if !native.path.is_file() {
            return Err(format!(
                "unit's `{}` artifact does not exist: {}",
                native.name,
                native.path.display()
            ));
        }
        externs.push((native.name.clone(), native.path.clone()));
    }
    let mut library = false;
    let mut rustc_args = Vec::new();
    let mut shared_native = None;
    let mut remaining = arguments.iter();
    while let Some(option) = remaining.next() {
        if option == "--library" {
            library = true;
            continue;
        }
        let value = remaining.next().ok_or_else(|| format!("missing value for {option}"))?;
        match option.as_str() {
            "--rustc-arg" => rustc_args.push(value.clone()),
            "--shared-native" => shared_native = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown native source option {option}")),
        }
    }
    if shared_native.is_some() && (unit.profile != "debug" || unit.kind == UnitKind::Library || library) {
        return Err("shared native linking requires a debug executable".into());
    }
    // Toolchain libraries carry the standard library they were built with; shared carriers would link another copy.
    if shared_native.is_some() && unit.toolchain_access.links_sysroot() {
        return Err("shared native linking cannot link the toolchain's own libraries".into());
    }
    Ok(Dependencies {
        externs,
        directories: unit.search.clone(),
        rust_abi: unit.rust_abi.clone(),
        library,
        rustc_args,
        shared_native,
    })
}

/// Run source, frontend, canonical Body IR, Incan lowering, plan validation, adapter, and pinned native compilation.
/// Adapter errors cross the executable boundary as their display text so named unsupported gaps retain the CLI's
/// refusal classification instead of appearing as debug-form plan errors.
/// An exact-body census selection reports only that diagnostic lowering pass and returns without compiling an artifact.
pub fn compile(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    compile_unit(arguments, false)
}

/// Print the unit's settled dependency closure. Checking and planning run exactly as for compilation, so the report
/// names the closure a build of the same description links; nothing is generated or published.
pub fn dependency_report(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [unit_path, sysroot, options @ ..] = arguments else {
        return Err("usage: --dependencies UNIT SYSROOT [--library]".into());
    };
    crate::identity::verify(Path::new(sysroot))?;
    let mut unit = UnitDescription::read(Path::new(unit_path)).map_err(|error| error.message)?;
    let dependencies = dependencies(&unit, options)?;
    if dependencies.library {
        unit.kind = UnitKind::Library;
    }
    let (kind, source, project_root) = (unit.kind, unit.source.clone(), unit.project_root.clone());
    let overflow_checks = unit.profile != "release";
    let rust = dependencies.rust_facts(&unit, Path::new(sysroot));
    let receipt = RefusalReceipt(None);
    let mut program = check_compilation_source(unit, rust, false, receipt.clone())?;
    lower_compilation_plan(&mut program, kind, overflow_checks, &source, &receipt, &mut Vec::new())?;
    print!(
        "{}",
        crate::dependency_report::render(
            &program.dependencies.package,
            kind,
            &program.reached_loaves,
            &program.dependencies.origins,
            &project_root,
        )
    );
    Ok(())
}

/// The names of the packages the checked declaration closure reaches.
fn reached_names(program: &CheckedProgram) -> BTreeSet<String> {
    program.reached_loaves.keys().cloned().collect()
}

/// Publish checked declarations and governed bodies before a library's native artifact exists. This uses the same
/// source-to-Body-IR pass and publication as compilation, but provides no compiled callable or native layout. It
/// breaks source dependency cycles such as the runtime's standard trait bounds without importing a prior runtime.
/// Consumers that require the provider's compiled crate still refuse until ordinary compilation supplies it.
pub fn source_interface(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [unit_path, sysroot, directory] = arguments else {
        return Err("usage: --source-interface UNIT SYSROOT DIRECTORY".into());
    };

    // ---- Checking inputs without an existing native publication ----
    crate::identity::verify(Path::new(sysroot))?;
    let unit = UnitDescription::read(Path::new(unit_path)).map_err(|error| error.message)?;
    if unit.kind != UnitKind::Library {
        return Err("a source interface requires a library unit".into());
    }
    let directory = Path::new(directory);
    let library = directory.join(format!("lib{}.rlib", native_crate_name(&unit.package)));
    if library.exists() {
        return Err("a source interface cannot replace a compiled library's publication".into());
    }
    let dependencies = dependencies(&unit, &[])?;
    let rust = dependencies.rust_facts(&unit, Path::new(sysroot));
    let loaf_links = unit.loaves.iter().map(|loaf| native_crate_name(&loaf.name)).collect();
    let mut closure = LoafNativeClosure {
        externs: dependencies.externs,
        search: unit.own_search.clone(),
        macro_externs: Vec::new(),
    };

    // ---- Checked source and governed publication ----
    let program = incan_frontend::compiler_stack::run_on_compiler_stack(move || {
        checked_modules(&unit, &rust, false, &RefusalReceipt(None))
    })?;
    let manifest = program
        .manifest
        .ok_or("a checked source interface requires a library manifest")?;
    closure.macro_externs = macro_externs(&manifest, &closure.externs, &loaf_links);
    std::fs::create_dir_all(directory)?;
    write_loaf(
        directory,
        manifest,
        &program.modules[program.own..],
        &closure,
        &program.graph,
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

/// Compile through the same native pipeline with a full canonical session, borrowing only generation-guarded
/// immutable readings. A resident worker has no use for a persisted unchanged-source lookup footprint.
pub(crate) fn compile_resident(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    compile_unit(arguments, true)
}

/// Compile one unit with the invocation's immutable-input retention policy; source checking and native execution
/// remain fresh for both short-lived and resident callers. A leading `--certify PATH` option names where a refusal
/// that unchanged inputs reproduce writes its receipt. Checking, lowering and native compilation retain their
/// separate refusal boundaries; only successful compilation publishes library artifacts.
fn compile_unit(arguments: &[String], resident: bool) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.len() < 4 {
        return Err(
            "usage: --unit UNIT CRATE OUTPUT SYSROOT [--certify PATH] [--rustc-arg ARG] [--library] [--shared-native DIR]"
                .into(),
        );
    }

    // ---- Checked source and Incan plan ----
    let mut timing = crate::native_timing::Timing::requested();
    let mut unit = UnitDescription::read(Path::new(&arguments[0])).map_err(|error| error.message)?;
    timing.mark("unit_descriptor");
    let source_path = unit.source.clone();
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::compile_unit", input = %source_path.display()).entered();
    let identity = crate::identity::verify(Path::new(&arguments[3]))?;
    let output = Path::new(&arguments[2]);
    let (certificate, options) = crate::certified_options(&arguments[4..]);
    let receipt = RefusalReceipt(certificate.map(Path::to_path_buf));
    let dependencies = dependencies(&unit, options)?;
    if dependencies.library {
        if unit.kind == UnitKind::Test {
            return Err("a test unit cannot also be a library".into());
        }
        unit.kind = UnitKind::Library;
    }
    let kind = unit.kind;
    let overflow_checks = unit.profile != "release";
    let loaves = unit.loaves.clone();
    // A library's closure publishes only the directories beyond the toolchain generation's search index.
    let published_search = unit.own_search.clone();
    let loaf_links: std::collections::BTreeSet<String> =
        unit.loaves.iter().map(|loaf| native_crate_name(&loaf.name)).collect();
    // The grant comes only from the description, read after the identity check and the bootstrap refusal.
    let access = unit.toolchain_access.clone();
    let rust = dependencies.rust_facts(&unit, Path::new(&arguments[3]));
    let mut program = check_compilation_source(unit, rust, resident, receipt.clone())?;
    if census_complete(&mut program, kind, overflow_checks)? {
        return Ok(());
    }
    let mut published_unions = Vec::new();
    let plan = lower_compilation_plan(
        &mut program,
        kind,
        overflow_checks,
        &source_path,
        &receipt,
        &mut published_unions,
    )?;
    let tests = crate::test_unit::report(&program.tests, &plan.tests)?;

    // ---- Exact native dependencies, including checked Loaf implementation layouts ----
    let native_externs =
        crate::native_crates::link_externs(&plan, &dependencies.externs, &loaves, &reached_names(&program))?;
    let library_facades = crate::library_exports::resolve_facades(&plan, program.manifest.as_ref())?;
    let publication = if let Some(mut manifest) = program.manifest.take() {
        publish_native_unions(&mut manifest, published_unions).map_err(|error| error.to_string())?;
        let directory = output.parent().ok_or("library output has no directory")?.to_path_buf();
        let closure = LoafNativeClosure {
            externs: native_externs.clone(),
            search: published_search,
            macro_externs: macro_externs(&manifest, &dependencies.externs, &loaf_links),
        };
        let prepared = prepare_loaf_publication(
            &directory,
            manifest,
            &program.modules[program.own..],
            &closure,
            &program.graph,
        )
        .map_err(|error| error.to_string())?;
        Some((directory, prepared))
    } else {
        None
    };
    drop(program);
    crate::adapter::compile(
        plan,
        &arguments[1],
        Path::new(&arguments[2]),
        &identity,
        &native_externs,
        &dependencies.directories,
        &dependencies.rustc_args,
        &access,
        dependencies.shared_native.as_deref(),
        kind,
        overflow_checks,
        library_facades,
    )
    .map_err(|error| {
        if matches!(error, crate::adapter::DriverError::Plan(_)) {
            receipt.deterministic();
        }
        error.to_string()
    })?;
    if kind == UnitKind::Test {
        std::fs::write(output.with_extension("tests.json"), serde_json::to_vec(&tests)?)?;
    }

    // ---- The Loaf beside a library unit's rlib (DD-0007) ----
    if let Some((directory, publication)) = publication {
        publication.write(&directory).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Check owned invocation inputs on the compiler stack and preserve the source refusal report on failure. The
/// returned program owns checked facts; no invocation descriptor is copied to bridge the stack boundary.
fn check_compilation_source(
    unit: UnitDescription,
    rust: RustFactsSetup,
    resident: bool,
    receipt: RefusalReceipt,
) -> Result<CheckedProgram, Box<dyn std::error::Error>> {
    let parent = tracing::Span::current();
    let checked = incan_frontend::compiler_stack::run_on_compiler_stack(move || {
        let _parent = parent.enter();
        checked_modules(&unit, &rust, resident, &receipt)
    });
    match checked {
        Ok(program) => Ok(program),
        Err(error) => {
            write_refusal_report(std::slice::from_ref(&error))?;
            Err(error.into())
        }
    }
}

/// Preserve ordinary-frame lowering while resolving only the authenticated runtime declarations it selects. A
/// lowering refusal or missing program entry receives the same deterministic certificate as ordinary compilation.
fn lower_compilation_plan(
    program: &mut CheckedProgram,
    kind: UnitKind,
    overflow_checks: bool,
    source: &Path,
    receipt: &RefusalReceipt,
    published_unions: &mut Vec<NativeUnionDeclaration>,
) -> Result<crate::plan::Plan, Box<dyn std::error::Error>> {
    tracing::debug!(target: "incan::invocation", modules = ?program.modules, "checked direct-route Body IR");
    let kind_name = kind.as_str().to_owned();
    let file = program
        .files
        .get(program.entry)
        .ok_or("selected Body IR entry source is missing")?;
    let layout_requirements = layout_runtime_requirements(&program.modules, file.clone());
    if !layout_requirements.is_empty() {
        prepare_runtime_layouts(program, layout_requirements)?;
    }
    let mut continuation = None;
    loop {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::lower_program", modules = program.modules.len()).entered();
        let inputs = SourceInputs {
            sources: program.sources.clone(),
            files: program.files.clone(),
            tests: program.tests.iter().map(|test| test.name.clone()).collect(),
            package_versions: program.dependencies.package_versions.clone(),
            overflow_checks,
        };
        let mut attempt_unions = Vec::new();
        let planning = match continuation.take() {
            Some(preserved) => resume_unit_plan(
                preserved,
                &program.modules,
                inputs,
                program.entry.try_into()?,
                &kind_name,
                &mut attempt_unions,
            ),
            None => unit_plan(
                &program.modules,
                inputs,
                program.entry.try_into()?,
                &kind_name,
                &mut attempt_unions,
            ),
        }
        .inspect_err(|_| receipt.deterministic())?;
        match planning {
            UnitPlanning::NeedsRuntime(preserved, requirements) => {
                satisfy_runtime_requirements(program, requirements)?;
                continuation = Some(preserved);
            }
            UnitPlanning::Complete(Some(plan)) => {
                published_unions.extend(attempt_unions);
                return Ok(plan);
            }
            UnitPlanning::Complete(None) => break,
        }
    }
    report_missing_entrypoint(source)?;
    receipt.deterministic();
    Err(SourceFailure::MissingEntrypoint.into())
}

/// Lower authored modules from their checked facts and the canonical provider catalog. Dependency bodies already
/// present in `bodies` supply context; every source body is built anew and remains outside immutable worker caches.
fn lower_authored_modules(
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
    session: &CompilationSession,
    bodies: &mut Vec<BodyIrModule>,
    files: &mut Vec<String>,
) -> Result<(), String> {
    let _invocation =
        tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::build_body_ir", modules = modules.len())
            .entered();
    let own = bodies.len();
    incan_driver::body_ir::lower_authored_modules(modules, analysis, session, bodies)?;
    for (module, body_ir) in modules.iter().zip(&bodies[own..]) {
        validate_retained_storage(&module.ast, &body_ir)
            .map_err(|error| format!("{}: {error}", module.file_path.display()))?;
        files.push(module.file_path.to_string_lossy().into_owned());
    }
    Ok(())
}

/// Report an optional exact-body census and decide whether it completed the invocation without native compilation.
/// Ordinary invocations retain their current checked program and proceed through the single lowering path.
fn census_complete(
    program: &mut CheckedProgram,
    kind: UnitKind,
    overflow_checks: bool,
) -> Result<bool, Box<dyn std::error::Error>> {
    if std::env::var_os(REFUSAL_REPORT).is_none() {
        return Ok(false);
    }
    let selection = std::env::var("INCAN_DIRECT_CENSUS_BODY").unwrap_or_default();
    let kind_name = kind.as_str().to_owned();
    let selected = !selection.is_empty();
    let refusals = loop {
        let outcome = program_refusals(
            &program.modules,
            SourceInputs {
                sources: program.sources.clone(),
                files: program.files.clone(),
                tests: Vec::new(),
                package_versions: program.dependencies.package_versions.clone(),
                overflow_checks,
            },
            program.entry.try_into()?,
            false,
            &selection,
            &kind_name,
        );
        match outcome {
            RefusalPlanning::NeedsRuntime(requirements) => satisfy_runtime_requirements(program, requirements)?,
            RefusalPlanning::Complete(refusals) => break refusals,
        }
    };
    // Progress belongs to the settled attempt; dependency discovery never publishes a partial census report.
    if std::env::var_os("INCAN_DIRECT_CENSUS_PROGRESS").is_some() {
        match program_refusals(
            &program.modules,
            SourceInputs {
                sources: program.sources.clone(),
                files: program.files.clone(),
                tests: Vec::new(),
                package_versions: program.dependencies.package_versions.clone(),
                overflow_checks,
            },
            program.entry.try_into()?,
            true,
            &selection,
            &kind_name,
        ) {
            RefusalPlanning::Complete(reported) if reported == refusals => {}
            _ => return Err("settled census changed during progress reporting".into()),
        }
    }
    write_refusal_report(&refusals)?;
    if selected && let Some(refusal) = refusals.first() {
        return Err(refusal.clone().into());
    }
    Ok(selected)
}

/// The crates a consumer must name directly when this library publicly reexports an attribute macro, else none.
///
/// The checked identity graph names the exact `rust::` target and the ABI payload proves it is an attribute macro.
/// rustc expands that macro in the consuming crate, so the producer's non-Loaf links must remain available there.
fn macro_externs(
    manifest: &LibraryManifest,
    externs: &[(String, PathBuf)],
    loaf_links: &BTreeSet<String>,
) -> Vec<(String, PathBuf)> {
    let exports_attribute_macro = manifest.contract_metadata.identity_graph.exports.iter().any(|export| {
        let Some([namespace, _crate_name, rest @ ..]) = export.target_path() else {
            return false;
        };
        if namespace != "rust" || rest.is_empty() {
            return false;
        }
        let path = export.target_path().map(|segments| segments[1..].join("::"));
        let Some(metadata) = path
            .as_deref()
            .and_then(|path| manifest.rust_abi.as_ref().and_then(|abi| abi.get(path)))
        else {
            return false;
        };
        matches!(&metadata.kind, incan_lang::interop::RustItemKind::Macro(info) if info.is_attribute())
    });
    if !exports_attribute_macro {
        return Vec::new();
    }
    externs
        .iter()
        .filter(|(name, _)| !loaf_links.contains(name))
        .cloned()
        .collect()
}
