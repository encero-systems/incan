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
use incan_frontend::loaf_index::{LoafIndex, LoafNativeClosure, NativeAnchors};
use incan_frontend::parsed_module::ParsedModule;
use incan_lang::lang::derives::{self, DeriveId};
use incan_loaf_names::native_crate_name;
use incan_lowering::{
    RefusalPlanning, RuntimeRequirement, SourceInputs, UnitPlanning, layout_runtime_requirements, program_refusals,
    resume_unit_plan, unit_plan,
};
use incan_semantics_core::body_ir::{BodyIrModule, LoafCallable, NativeUnionDeclaration, NominalDeclaration};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolOrigin};

mod callable_crumb;
mod callable_products;
mod checked_program;
mod declaration_validation;
pub(crate) mod decoded_pages;
mod graph_body_output;
mod namespace_crumb;
mod namespace_pages;

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
/// and the resident worker certify the same outcomes, and it crosses a rustc handoff to the process that runs rustc.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct RefusalReceipt(Option<PathBuf>);

impl RefusalReceipt {
    /// Certify a refusal that unchanged inputs reproduce. A receipt that cannot be written only means the next
    /// unchanged compilation runs again.
    pub(crate) fn deterministic(&self) {
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
    sources: Vec<String>,
    files: Vec<String>,
    entry: usize,
}

/// Checked owning modules and their source provenance, with one explicit native entry module.
struct CheckedProgram {
    /// Invocation-local producer evidence for self-contained scalar native function products.
    cacheable_bodies: Vec<(incan_semantics_core::CompilerNodeId, String)>,
    retained_namespaces: Vec<checked_program::RetainedNamespaceBody>,
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
    /// Compatibility graph bytes for a caller that has not opted into separate requested publication.
    graph: Option<Vec<u8>>,
    /// Session-collected test cases; lowering receives their entries and Oven receives their outcome policy.
    tests: Vec<crate::test_unit::CollectedTest>,
}

/// The changed checked-module slice passed across the Incan boundary for one lowering continuation.
struct LoweringRetryInputs {
    modules: Vec<BodyIrModule>,
    sources: Vec<String>,
    files: Vec<String>,
    entry: usize,
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
    /// Compatibility for a compiler generation whose Oven still requires graphs in native publications.
    collect_codegraph: bool,
    /// Describe a source interface for inspection: keep private registries, named by their source modules
    /// (`--inspection`). Ordinary publication and compilation leave it unset.
    pub(crate) inspection: bool,
    /// Arguments Oven passes to rustc unchanged (`--rustc-arg`): the pinned target's link policy (DD-0008).
    rustc_args: Vec<String>,
    /// A development executable may link exact native carriers from this immutable store.
    shared_native: Option<PathBuf>,
    /// Separate graph-selected checker values supplied by the engine; these bytes never enter the unit description.
    graph_body_records: incan_frontend::checker_records::GraphBodyRecords,
    /// Optional invocation output for freshly produced checker values; it is not a compilation input.
    graph_body_output: Option<PathBuf>,
    /// Request-local directory for actual plan-function producer evaluations and reuses.
    plan_record_report: Option<PathBuf>,
    /// Bakery-selected plan-function products; private target caches are never record authority.
    plan_record_products: Option<PathBuf>,
    /// The current home's toolchain and bakery roots, which anchor native-closure references (`--toolchain-root`,
    /// `--bakery-root`). A root Oven does not name leaves paths under it absolute.
    pub(crate) anchors: NativeAnchors,
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

    /// The engine's graph-selected checker values, which a check installs exactly as a build does.
    pub(crate) fn graph_body_records(&self) -> incan_frontend::checker_records::GraphBodyRecords {
        self.graph_body_records.clone()
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
    let mut collect_codegraph = true;
    let mut inspection = false;
    let mut rustc_args = Vec::new();
    let mut shared_native = None;
    let mut graph_body_directory = None;
    let mut graph_body_output = None;
    let mut plan_record_report = None;
    let mut plan_record_products = None;
    let mut anchors = NativeAnchors::default();
    let mut remaining = arguments.iter();
    while let Some(option) = remaining.next() {
        if option == "--library" {
            library = true;
            continue;
        }
        if option == "--inspection" {
            inspection = true;
            continue;
        }
        if option == "--no-codegraph" {
            collect_codegraph = false;
            continue;
        }
        let value = remaining.next().ok_or_else(|| format!("missing value for {option}"))?;
        match option.as_str() {
            "--rustc-arg" => rustc_args.push(value.clone()),
            "--shared-native" => shared_native = Some(PathBuf::from(value)),
            "--graph-body-records" => graph_body_directory = Some(Path::new(value)),
            "--graph-body-output" => graph_body_output = Some(PathBuf::from(value)),
            "--plan-record-report" => plan_record_report = Some(PathBuf::from(value)),
            "--plan-record-products" => plan_record_products = Some(PathBuf::from(value)),
            "--toolchain-root" => anchors.toolchain = Some(PathBuf::from(value)),
            "--bakery-root" => anchors.bakery = Some(PathBuf::from(value)),
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
    let graph_body_records = match graph_body_directory {
        Some(path) => read_graph_body_records(
            path,
            anchors.bakery.clone(),
            &unit.project_root,
            graph_body_output.is_some() && plan_record_products.is_some() && unit.kind == UnitKind::Program,
        )?,
        None => incan_frontend::checker_records::GraphBodyRecords::default(),
    };
    Ok(Dependencies {
        externs,
        directories: unit.search.clone(),
        rust_abi: unit.rust_abi.clone(),
        library,
        collect_codegraph,
        inspection,
        rustc_args,
        shared_native,
        graph_body_records,
        graph_body_output,
        plan_record_report,
        plan_record_products,
        anchors,
    })
}

/// Filesystem-backed source of independently bounded graph body pages.
#[derive(Debug)]
struct GraphBodyRecordPages {
    root: PathBuf,
    bakery: Option<PathBuf>,
    namespaces: namespace_pages::NamespacePages,
    reopened: std::sync::OnceLock<Result<tempfile::TempDir, String>>,
}

impl GraphBodyRecordPages {
    fn owner_page(&self, identity: &str) -> PathBuf {
        let key = hex::encode(Sha256::digest(identity.as_bytes()));
        if let Some(Ok(directory)) = self.reopened.get() {
            let supplied = directory.path().join(&key);
            if supplied.is_file() {
                return supplied;
            }
        }
        self.root.join(key)
    }
}

impl incan_frontend::checker_records::GraphBodyRecordSource for GraphBodyRecordPages {
    fn has_owner(&self, identity: &str) -> bool {
        self.owner_page(identity).try_exists().unwrap_or(true)
    }

    /// Read only the page addressed by this stable owner; absence keeps that owner on the ordinary checking path.
    fn read(
        &self,
        identity: &str,
    ) -> Result<
        Option<incan_frontend::checker_records::GraphBodyRecord>,
        incan_frontend::checker_records::RecordAdapterFault,
    > {
        const MAX_PAGE_BYTES: u64 = 64 * 1024 * 1024;
        let path = self.owner_page(identity);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(incan_frontend::checker_records::RecordAdapterFault::InvalidField(
                    format!("cannot inspect graph body page {}: {error}", path.display()),
                ));
            }
        };
        if !metadata.is_file() || metadata.len() > MAX_PAGE_BYTES {
            return Err(incan_frontend::checker_records::RecordAdapterFault::InvalidField(
                "graph body page is not a bounded regular file".into(),
            ));
        }
        let generation = crate::native_search::FileGeneration::observe(&metadata);
        if let Some(generation) = generation
            && let Some(record) = decoded_pages::observed(generation, identity)
        {
            tracing::trace!(target: "incan::invocation", phase = "checker_page_retained", owner = identity);
            return Ok(Some(record));
        }
        let _input = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::read_checker_page",
            owner = identity, bytes = metadata.len())
        .entered();
        let (encoded, generation) = read_checker_page(&path, generation, MAX_PAGE_BYTES)?;
        let digest: [u8; 32] = Sha256::digest(&encoded).into();
        if let Some(record) = decoded_pages::decoded(digest, identity, generation) {
            return Ok(Some(record));
        }
        // Oven writes each page as one owner's Crumb row (`encode_graph_body_values`); the shared codec decodes and
        // authenticates it, both checker records included.
        let mut records = internal_models::decode_graph_checking_inputs(&encoded).map_err(|error| {
            incan_frontend::checker_records::RecordAdapterFault::InvalidField(format!(
                "graph body page refused: {error:?}"
            ))
        })?;
        let record = match (records.pop(), records.is_empty()) {
            (Some(record), true) if record.identity == identity => record,
            _ => {
                return Err(incan_frontend::checker_records::RecordAdapterFault::InvalidField(
                    "graph body page disagrees with its requested identity".into(),
                ));
            }
        };
        let frame = if record.body_ir.is_empty() {
            None
        } else {
            let frame =
                incan_semantics_core::executable_representation::decode_owner(&record.body_ir).map_err(|error| {
                    incan_frontend::checker_records::RecordAdapterFault::InvalidField(format!(
                        "graph Body IR owner frame refused: {error}"
                    ))
                })?;
            if frame.source.is_empty() || frame.identity != record.identity {
                return Err(incan_frontend::checker_records::RecordAdapterFault::InvalidField(
                    "graph Body IR owner frame disagrees with its selected identity".into(),
                ));
            }
            Some(frame)
        };
        let record = incan_frontend::checker_records::GraphBodyRecord {
            identity: record.identity,
            local: record.local.into(),
            final_value: record.final_fields.into(),
            body_ir: frame,
        };
        decoded_pages::remember(digest, generation, &record);
        Ok(Some(record))
    }

    fn reopen_namespace_peers(&self, source: &str, changed: &std::collections::BTreeSet<String>) -> Result<(), String> {
        let Some(bakery) = &self.bakery else { return Ok(()) };
        if !self
            .root
            .join("reopen-snapshot.crumb")
            .try_exists()
            .map_err(|e| e.to_string())?
        {
            return Ok(());
        }
        let Some(oven) = std::env::var_os("INCAN_GRAPH_PEER_READER") else {
            return Ok(());
        };
        let temporary = self
            .reopened
            .get_or_init(|| {
                tempfile::Builder::new()
                    .prefix("incan-reopened-peers-")
                    .tempdir()
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(Clone::clone)?;
        let _trace = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::reopen_namespace_peers", module = source).entered();
        let status = std::process::Command::new(oven)
            .arg("--internal-reopened-peers")
            .arg(bakery)
            .arg(&self.root)
            .arg(temporary.path())
            .arg(source)
            .args(changed)
            .status()
            .map_err(|error| format!("cannot start the selected graph reader: {error}"))?;
        if !status.success() {
            return Err(format!("graph reader refused reopened namespace {source} ({status})"));
        }
        Ok(())
    }

    /// Read the request's exact namespace selection on demand.
    fn read_namespace(
        &self,
        source: &str,
    ) -> Result<
        Option<incan_frontend::checker_records::GraphNamespaceRecord>,
        incan_frontend::checker_records::RecordAdapterFault,
    > {
        self.namespaces.read(&self.root, source)
    }

    fn publish_namespace(
        &self,
        header: std::sync::Arc<incan_frontend::checker_records::GraphNamespaceHeader>,
        value: String,
        body_ir: Vec<u8>,
    ) {
        self.namespaces.publish(header, value, body_ir);
    }

    fn retains_namespace(&self, source: &str) -> bool {
        self.namespaces.retains(source)
    }

    fn retain_namespace(&self, source: &str, body: &mut BodyIrModule) {
        self.namespaces.retain(source, body);
    }

    fn settle_namespace_retention(&self) {
        self.namespaces.settle();
    }
}

/// Read an exact bounded file through one descriptor. A changing read is refused before the codec sees its bytes.
fn read_checker_page(
    path: &Path,
    generation: Option<crate::native_search::FileGeneration>,
    limit: u64,
) -> Result<(Vec<u8>, Option<crate::native_search::FileGeneration>), incan_frontend::checker_records::RecordAdapterFault>
{
    use crate::native_search::FileGeneration;
    use incan_frontend::checker_records::RecordAdapterFault;
    use std::io::Read;
    let read_failed =
        |error| RecordAdapterFault::InvalidField(format!("cannot read graph body page {}: {error}", path.display()));
    let changed = || RecordAdapterFault::InvalidField("graph body page changed during its read".into());
    let mut file = std::fs::File::open(path).map_err(read_failed)?;
    let metadata = file.metadata().map_err(read_failed)?;
    if !metadata.is_file() || metadata.len() > limit || FileGeneration::observe(&metadata) != generation {
        return Err(changed());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(read_failed)?;
    if bytes.len() as u64 != metadata.len()
        || FileGeneration::observe(&file.metadata().map_err(read_failed)?) != generation
    {
        return Err(changed());
    }
    Ok((bytes, generation))
}

/// Bind the engine's keyed graph-record page directory without reading an aggregate inventory.
fn read_graph_body_records(
    path: &Path,
    bakery: Option<PathBuf>,
    project_root: &Path,
    retain_namespaces: bool,
) -> Result<incan_frontend::checker_records::GraphBodyRecords, String> {
    if !path.is_dir() {
        return Err(format!(
            "graph body record pages are not a directory: {}",
            path.display()
        ));
    }
    let records =
        incan_frontend::checker_records::GraphBodyRecords::from_source(std::sync::Arc::new(GraphBodyRecordPages {
            root: path.to_path_buf(),
            namespaces: namespace_pages::NamespacePages::open(
                path,
                bakery.as_deref(),
                project_root,
                retain_namespaces,
            )?,
            bakery,
            reopened: std::sync::OnceLock::new(),
        }));
    if path
        .join("ranked-source-edit.crumb")
        .try_exists()
        .map_err(|e| e.to_string())?
    {
        return bind_contextual_rank(path, records);
    }
    Ok(records)
}

/// Bind a body-only edit and the exact namespace order the engine selected for it.
fn bind_contextual_rank(
    directory: &Path,
    records: incan_frontend::checker_records::GraphBodyRecords,
) -> Result<incan_frontend::checker_records::GraphBodyRecords, String> {
    let bounded = |name: &str| -> Result<Vec<u8>, String> {
        use std::io::Read;
        const LIMIT: u64 = 65_536;
        let mut bytes = Vec::new();
        std::fs::File::open(directory.join(name))
            .map_err(|error| error.to_string())?
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > LIMIT {
            return Err(format!("contextual ranked page {name} exceeds its bound"));
        }
        Ok(bytes)
    };
    let demand = internal_models::decode_demand_page(&bounded("ranked-source-edit.crumb")?)
        .map_err(|error| error.to_string())?;
    let [owner, source]: [_; 2] = demand
        .keys
        .try_into()
        .map_err(|_| "contextual ranked edit requires one owner and its changed source")?;
    let name = owner
        .identity
        .split_once('|')
        .and_then(|(identity, _)| identity.strip_prefix("function:"))
        .and_then(|identity| identity.rsplit_once("::"))
        .map(|(_, name)| name);
    if owner.family != "ranked-source-edit"
        || owner.identity_profile != "incan.stable-declaration/1"
        || source.family != "owner-source"
        || source.identity_profile != "incan.source-projection/1"
        || source.origin != owner.origin
        || source.identity != owner.facet
        || name != Some(source.facet.as_str())
        || !owner.facet.ends_with(".incn")
        || Path::new(&owner.facet)
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err("contextual ranked edit does not bind a project source owner".into());
    }
    let order = internal_models::decode_namespace_order(&bounded("ranked-source-order.crumb")?, owner.origin)
        .map_err(|error| error.to_string())?;
    if order.evidence != owner.context || order.members.as_slice() != [owner.facet.as_str()] {
        return Err("contextual ranked edit does not match its namespace certificate".into());
    }
    Ok(records.for_contextual_ranked_owner(owner.identity, owner.facet, order.rank, order.evidence))
}

/// Run one ordinary signature reader in the real compiler environment and publish only that owner's two Crumb values.
pub(crate) fn check_ranked_reader(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [unit, sysroot, demand_path, output, options @ ..] = arguments else {
        return Err("usage: --check-ranked-reader UNIT SYSROOT DEMAND DIRECTORY [compiler options]".into());
    };
    crate::identity::verify(Path::new(sysroot))?;
    let path = Path::new(demand_path);
    if !path.is_file() || path.metadata()?.len() > 65536 {
        return Err("ranked reader requires a bounded work page".into());
    }
    let demand = internal_models::decode_demand_page(&std::fs::read(path)?).map_err(|error| error.to_string())?;
    let [owner, triggers @ ..] = demand.keys.as_slice() else {
        return Err("empty ranked reader work".into());
    };
    if owner.family != "ranked-reader" || triggers.is_empty() || triggers.iter().any(|key| key.origin != owner.origin) {
        return Err("ranked reader demand has incomplete authority".into());
    }
    if Path::new(&owner.facet)
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err("ranked reader module is not project-relative".into());
    }
    let unit = UnitDescription::read(Path::new(unit)).map_err(|error| error.message)?;
    let deps = dependencies(&unit, options)?;
    let rust = deps.rust_facts(&unit, Path::new(sysroot));
    let records = deps.graph_body_records();
    let module = owner.facet.clone();
    let identity = owner.identity.clone();
    let Some(fields) = incan_frontend::compiler_stack::run_on_compiler_stack(move || {
        incan_driver::check::ranked_reader(&unit, &rust, records, &module, &identity)
    })
    .map_err(|error| error.message)?
    else {
        std::fs::write(Path::new(output).join("unavailable"), b"missing-settled-peer")?;
        return Ok(());
    };
    if graph_body_output::uses_method_dispatch(&fields) {
        // The standalone protocol currently carries only checker products. The ordinary compile also returns
        // the resolver's candidate set and can authenticate the caller and its membership together.
        std::fs::write(Path::new(output).join("unavailable"), b"missing-settled-peer")?;
        return Ok(());
    }
    let records = graph_body_output::encoded_body_value(&owner.facet, &fields).map_err(|refusal| refusal.reason)?;
    let output = Path::new(output);
    std::fs::write(output.join("declaration.crumb"), &records.declaration)?;
    std::fs::write(output.join("local.crumb"), &records.local)?;
    std::fs::write(output.join("final.crumb"), &records.final_value)?;
    tracing::debug!(target: "incan::invocation", owner = %owner.identity, family = "reader",
        declaration = %hex::encode(Sha256::digest(&records.declaration)),
        local = %hex::encode(Sha256::digest(&records.local)), root = %hex::encode(Sha256::digest(&records.final_value)),
        "completed ranked checking producer");
    Ok(())
}

/// Run source, frontend, canonical Body IR, Incan lowering, plan validation, adapter, and pinned native compilation.
/// Adapter errors cross the executable boundary as their display text so named unsupported gaps retain the CLI's
/// refusal classification instead of appearing as debug-form plan errors.
/// An exact-body census selection reports only that diagnostic lowering pass and returns without compiling an artifact.
pub fn compile(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    compile_unit(arguments, false, None).map(|_| ())
}

/// Where a compiled unit's native compilation ran.
pub(crate) enum UnitCompiled {
    /// rustc ran in this process, or the unit needed no native compilation.
    InProcess,
    /// This worker wrote the rustc handoff at this path for its caller to run in a fresh process.
    HandedOff(PathBuf),
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
    let mut program = check_compilation_source(
        unit,
        rust,
        false,
        receipt.clone(),
        dependencies.graph_body_records,
        dependencies.graph_body_output,
        dependencies.collect_codegraph,
    )?;
    lower_compilation_plan(
        &mut program,
        kind,
        overflow_checks,
        &source,
        &receipt,
        &mut Vec::new(),
        None,
        None,
    )?;
    // The dependency inspection's roots come from checked source and final planning, independently of the
    // storage reads of the measured build. Preserve canonical identities (including overloads) for replay.
    if tracing::enabled!(target: "incan::invocation", tracing::Level::TRACE) {
        let mut roots = 0usize;
        for identity in program
            .dependencies
            .requested
            .difference(&program.dependencies.checked_local)
        {
            let SymbolOrigin::Package { library, .. } = &identity.origin else {
                continue;
            };
            let runtime = match program.dependencies.origins.get(identity) {
                Some(RootOrigin::Runtime { path, .. }) => path.as_str(),
                _ => "",
            };
            tracing::trace!(
                target: "incan::invocation", phase = "dependency_root", source = %source.display(), loaf = %library,
                identity = %incan_semantics_core::encode_incan_symbol_identity(identity), runtime,
            );
            roots += 1;
        }
        tracing::trace!(target: "incan::invocation", phase = "dependency_roots_complete", source = %source.display(), roots);
    }
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
    crate::native_timing::observe_frontend(false);
    let [unit_path, sysroot, directory, options @ ..] = arguments else {
        return Err(
            "usage: --source-interface UNIT SYSROOT DIRECTORY [--toolchain-root DIR] [--bakery-root DIR]".into(),
        );
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
    let dependencies = dependencies(&unit, options)?;
    let rust = dependencies.rust_facts(&unit, Path::new(sysroot));
    let loaf_links = unit.loaves.iter().map(|loaf| native_crate_name(&loaf.name)).collect();
    let mut closure = LoafNativeClosure {
        externs: dependencies.externs,
        search: unit.own_search.clone(),
        macro_externs: Vec::new(),
    };

    // ---- Checked source and governed publication ----
    let program = incan_frontend::compiler_stack::run_on_compiler_stack(move || {
        checked_modules(
            &unit,
            &rust,
            false,
            &RefusalReceipt(None),
            dependencies.graph_body_records,
            None,
            true,
            dependencies.inspection,
        )
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
        &dependencies.anchors,
        program.graph.as_deref(),
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

/// Compile through the same native pipeline with a full canonical session, borrowing only generation-guarded
/// immutable readings. A resident worker has no use for a persisted unchanged-source lookup footprint. After lowering,
/// `hands_off` decides whether this worker writes the rustc handoff for its caller instead of running rustc.
pub(crate) fn compile_resident(
    arguments: &[String],
    hands_off: &dyn Fn() -> bool,
) -> Result<UnitCompiled, Box<dyn std::error::Error>> {
    compile_unit(arguments, true, Some(hands_off))
}

/// Compile one unit with the invocation's immutable-input retention policy; source checking and native execution
/// remain fresh for both short-lived and resident callers. A leading `--certify PATH` option names where a refusal
/// that unchanged inputs reproduce writes its receipt. Checking, lowering and native compilation retain their
/// separate refusal boundaries; only successful compilation publishes library artifacts. Native compilation runs in
/// this process unless `hands_off` answers yes after lowering, when the unit's handoff goes to the caller instead
/// (`crate::rustc_process`).
fn compile_unit(
    arguments: &[String],
    resident: bool,
    hands_off: Option<&dyn Fn() -> bool>,
) -> Result<UnitCompiled, Box<dyn std::error::Error>> {
    if arguments.len() < 4 {
        return Err(
            "usage: --unit UNIT CRATE OUTPUT SYSROOT [--certify PATH] [--rustc-arg ARG] [--library] [--shared-native DIR] [--toolchain-root DIR] [--bakery-root DIR] [--graph-body-records PATH] [--graph-body-output PATH]"
                .into(),
        );
    }

    // ---- Checked source and Incan plan ----
    let mut timing = crate::native_timing::Timing::requested();
    let mut unit = UnitDescription::read(Path::new(&arguments[0])).map_err(|error| error.message)?;
    timing.mark("unit_descriptor");
    let source_path = unit.source.clone();
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::compile_unit", input = %source_path.display()).entered();
    crate::native_timing::observe_frontend(true);
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
    let cloves = unit.cloves.clone();
    let rust = dependencies.rust_facts(&unit, Path::new(&arguments[3]));
    let native_products = dependencies
        .plan_record_products
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .or_else(|| callable_products::native_directory(&unit));
    let mut program = check_compilation_source(
        unit,
        rust,
        resident,
        receipt.clone(),
        dependencies.graph_body_records.clone(),
        dependencies.graph_body_output.clone(),
        dependencies.collect_codegraph,
    )?;
    dependencies.graph_body_records.report_contextual_ranked_owner();
    if census_complete(&mut program, kind, overflow_checks)? {
        return Ok(UnitCompiled::InProcess);
    }
    // Checking is done, and the Rust items it decoded are not lowering inputs. Lowering keeps the index for runtime
    // resolution, so release the items rather than carry them through lowering and rustc.
    program.dependencies.loaves.release_resolved_rust_items();
    // Checking freed its analysis and the session; return those pages before lowering allocates its contexts.
    incan_platform::release_free_heap();
    timing.mark("checked_heap_released");
    let mut published_unions = Vec::new();
    let plan = lower_compilation_plan(
        &mut program,
        kind,
        overflow_checks,
        &source_path,
        &receipt,
        &mut published_unions,
        native_products,
        dependencies
            .plan_record_report
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
    )?;
    timing.mark("plan_lowered");
    let tests = crate::test_unit::report(&program.tests, &plan.tests)?;
    if let (Some(products), Some(report)) = (&dependencies.plan_record_products, &dependencies.plan_record_report) {
        incan_lowering::publish_plan_records(
            &plan,
            products.to_string_lossy().into_owned(),
            report.to_string_lossy().into_owned(),
        )?;
    }

    // ---- Exact native dependencies, including checked Loaf implementation layouts ----
    let native_externs = crate::native_crates::link_externs(
        &plan,
        &dependencies.externs,
        &loaves,
        &reached_names(&program),
        &dependencies.anchors,
    )?;
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
            &dependencies.anchors,
            program.graph.as_deref(),
        )
        .map_err(|error| error.to_string())?;
        Some((directory, prepared))
    } else {
        None
    };
    timing.mark("native_inputs_prepared");
    checked_program::return_retained_namespaces(&mut program, &dependencies.graph_body_records);
    dependencies.graph_body_records.settle_namespace_retention();
    drop(program);
    drop(dependencies.graph_body_records);
    // The checked program, its Body IR and the lowering's temporaries are gone; return their pages before rustc runs
    // here or the footprint that decides a handoff is read, so neither counts the frontend's freed heap.
    incan_platform::release_free_heap();
    timing.mark("frontend_heap_released");
    // Clove archives a checked binding links are found in the directories Oven compiled them into (RFC 159).
    let mut rustc_args = dependencies.rustc_args;
    rustc_args.extend(crate::foreign::clove_link_arguments(&plan, &cloves).map_err(|error| {
        receipt.deterministic();
        error.to_string()
    })?);
    let handoff = crate::rustc_process::RustcHandoff {
        plan,
        crate_name: arguments[1].clone(),
        output: output.to_path_buf(),
        sysroot: PathBuf::from(&arguments[3]),
        externs: native_externs,
        dependency_search_paths: dependencies.directories,
        rustc_args,
        access,
        shared_store: dependencies.shared_native,
        kind,
        overflow_checks,
        library_facades,
        plan_record_report: dependencies.plan_record_report,
        tests: (kind == UnitKind::Test)
            .then(|| serde_json::to_vec(&tests))
            .transpose()?,
        publication,
        checked_sources: Vec::new(),
    };
    // A worker over its footprint ceiling answers with the handoff and retires; its caller runs rustc fresh.
    if hands_off.is_some_and(|hands_off| hands_off()) {
        return Ok(UnitCompiled::HandedOff(handoff.hand_off(&mut timing)?));
    }
    handoff.compile(&identity)?;
    Ok(UnitCompiled::InProcess)
}

/// Check owned invocation inputs on the compiler stack and preserve the source refusal report on failure. The
/// returned program owns checked facts; no invocation descriptor is copied to bridge the stack boundary.
fn check_compilation_source(
    unit: UnitDescription,
    rust: RustFactsSetup,
    resident: bool,
    receipt: RefusalReceipt,
    graph_body_records: incan_frontend::checker_records::GraphBodyRecords,
    graph_body_output: Option<PathBuf>,
    collect_codegraph: bool,
) -> Result<CheckedProgram, Box<dyn std::error::Error>> {
    let parent = tracing::Span::current();
    let checked = incan_frontend::compiler_stack::run_on_compiler_stack(move || {
        let _parent = parent.enter();
        checked_modules(
            &unit,
            &rust,
            resident,
            &receipt,
            graph_body_records,
            graph_body_output,
            collect_codegraph,
            false,
        )
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
    native_products: Option<String>,
    plan_record_report: Option<String>,
) -> Result<crate::plan::Plan, Box<dyn std::error::Error>> {
    tracing::debug!(target: "incan::invocation", modules = program.modules.len(),
        module_body_entries = program.modules.iter().map(|module| module.bodies.len()).sum::<usize>(),
        "checked Body IR inventory");
    let kind_name = kind.as_str().to_owned();
    let file = program
        .files
        .get(program.entry)
        .ok_or("selected Body IR entry source is missing")?;
    // Install the reached dependencies before the first attempt, with the binding layouts only a decorated unit needs.
    let layout_requirements = layout_runtime_requirements(&program.modules, file.clone());
    prepare_runtime_layouts(program, layout_requirements)?;
    let mut timing = crate::native_timing::Timing::requested();
    let mut continuation = None;
    let mut retry_indices: Option<Vec<usize>> = None;
    loop {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::lower_program", modules = program.modules.len()).entered();
        tracing::debug!(target: "incan::invocation", held = %held_modules(&program.modules), "lowering Body IR modules");
        let mut attempt_unions = Vec::new();
        let planning = match continuation.take() {
            Some(preserved) => {
                let selected = retry_indices
                    .take()
                    .ok_or("lowering continuation has no resolved module delta")?;
                let retry = lowering_retry_inputs(program, &selected)?;
                let inputs = SourceInputs {
                    scalar_products: None,
                    scalar_bodies: Vec::new(),
                    sources: retry.sources,
                    files: retry.files,
                    tests: program.tests.iter().map(|test| test.entry.clone()).collect(),
                    package_versions: program.dependencies.package_versions.clone(),
                    overflow_checks,
                    record_report: plan_record_report.clone(),
                };
                resume_unit_plan(
                    preserved,
                    &retry.modules,
                    // #3022: remove this borrow when an owned model field can transfer through the Incan wrapper.
                    &inputs,
                    retry.entry.try_into()?,
                    &kind_name,
                    &mut attempt_unions,
                )
            }
            None => {
                let inputs = SourceInputs {
                    scalar_products: native_products.clone(),
                    scalar_bodies: std::mem::take(&mut program.cacheable_bodies),
                    sources: program.sources.clone(),
                    files: program.files.clone(),
                    tests: program.tests.iter().map(|test| test.entry.clone()).collect(),
                    package_versions: program.dependencies.package_versions.clone(),
                    overflow_checks,
                    record_report: plan_record_report.clone(),
                };
                unit_plan(
                    &program.modules,
                    &inputs,
                    program.entry.try_into()?,
                    &kind_name,
                    &mut attempt_unions,
                )
            }
        }
        .inspect_err(|_| receipt.deterministic())?;
        match planning {
            UnitPlanning::NeedsRuntime(preserved, requirements) => {
                // The attempt's worker contexts and temporaries are freed; return their pages before runtime
                // resolution and the resumed attempt allocate.
                incan_platform::release_free_heap();
                timing.mark("lowering_attempt_heap_released");
                retry_indices = Some(satisfy_runtime_requirements(program, requirements)?);
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

/// Name each Body IR module one lowering attempt holds, with its body count, for an invocation trace.
fn held_modules(modules: &[BodyIrModule]) -> String {
    modules
        .iter()
        .map(|module| format!("{}:{}", module.module_id.path(), module.bodies.len()))
        .collect::<Vec<_>>()
        .join(",")
}

/// Copy only the changed runtime modules and aligned provenance into one continuation boundary value.
fn lowering_retry_inputs(program: &CheckedProgram, selected: &[usize]) -> Result<LoweringRetryInputs, String> {
    let mut modules = Vec::with_capacity(selected.len());
    let mut sources = Vec::with_capacity(selected.len());
    let mut files = Vec::with_capacity(selected.len());
    let mut entry = None;
    for (retry_index, &program_index) in selected.iter().enumerate() {
        modules.push(
            program
                .modules
                .get(program_index)
                .ok_or("lowering continuation selected a missing Body IR module")?
                .clone(),
        );
        sources.push(
            program
                .sources
                .get(program_index)
                .ok_or("lowering continuation selected missing source provenance")?
                .clone(),
        );
        files.push(
            program
                .files
                .get(program_index)
                .ok_or("lowering continuation selected missing file provenance")?
                .clone(),
        );
        if program_index == program.entry {
            entry = Some(retry_index);
        }
    }
    Ok(LoweringRetryInputs {
        modules,
        sources,
        files,
        entry: entry.ok_or("lowering continuation omitted the entry module")?,
    })
}

/// Lower authored modules from their checked facts and the canonical provider catalog. Dependency bodies already
/// present in `bodies` supply context for admitted namespaces and freshly checked sources.
fn lower_authored_modules(
    modules: &[ParsedModule],
    analysis: &mut CompilationAnalysis,
    session: &CompilationSession,
    bodies: &mut Vec<BodyIrModule>,
    files: &mut Vec<String>,
) -> Result<BTreeSet<PathBuf>, String> {
    let _invocation =
        tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::build_body_ir", modules = modules.len())
            .entered();
    let own = bodies.len();
    let rebound = incan_driver::body_ir::lower_compilation_modules(modules, analysis, session, bodies)?;
    for (module, body_ir) in modules.iter().zip(&bodies[own..]) {
        validate_retained_storage(&module.ast, &body_ir)
            .map_err(|error| format!("{}: {error}", module.file_path.display()))?;
        files.push(module.file_path.to_string_lossy().into_owned());
    }
    Ok(rebound)
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
            &SourceInputs {
                scalar_products: None,
                scalar_bodies: Vec::new(),
                sources: program.sources.clone(),
                files: program.files.clone(),
                tests: Vec::new(),
                package_versions: program.dependencies.package_versions.clone(),
                overflow_checks,
                record_report: None,
            },
            program.entry.try_into()?,
            false,
            &selection,
            &kind_name,
        );
        match outcome {
            RefusalPlanning::NeedsRuntime(requirements) => {
                let _ = satisfy_runtime_requirements(program, requirements)?;
            }
            RefusalPlanning::Complete(refusals) => break refusals,
        }
    };
    // Progress belongs to the settled attempt; dependency discovery never publishes a partial census report.
    if std::env::var_os("INCAN_DIRECT_CENSUS_PROGRESS").is_some() {
        match program_refusals(
            &program.modules,
            &SourceInputs {
                scalar_products: None,
                scalar_bodies: Vec::new(),
                sources: program.sources.clone(),
                files: program.files.clone(),
                tests: Vec::new(),
                package_versions: program.dependencies.package_versions.clone(),
                overflow_checks,
                record_report: None,
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

#[cfg(test)]
mod graph_body_page_tests {
    use super::GraphBodyRecordPages;
    use incan_frontend::checker_records::GraphBodyRecordSource;
    use sha2::{Digest, Sha256};

    #[test]
    fn keyed_body_pages_do_not_scan_unrequested_owners() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("incan-graph-body-pages-{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root)?;
        }
        std::fs::create_dir_all(&root)?;
        let unrelated = root.join(hex::encode(Sha256::digest(b"unrelated-owner")));
        std::fs::write(&unrelated, b"malformed unrelated page")?;
        let source = GraphBodyRecordPages {
            root: root.clone(),
            bakery: None,
            namespaces: Default::default(),
            reopened: Default::default(),
        };

        assert!(source.read("requested-owner")?.is_none());
        assert!(source.read("unrelated-owner").is_err());

        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
