//! Typed exchange with the Incan native-runtime identity engine.
//!
//! Removable transport for the minimized API gap tracked by #1698. Identity projection lives in
//! `workspaces/native-runtime-inputs`; this module binds the executable and exact request. Ordinary exchanges retain
//! authenticated native records and physical edges. The older canonical SDK-catalog exchange remains during migration.
//! Neither exchange establishes complete semantic or macro metadata authority.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::error::{CliError, CliResult};
use oven_model::manifest::{DependencySource, DependencySpec};
use oven_rustc::native_loaf::NativeLoafClosure;
use oven_store::OvenBuildIntent;
use serde::{Deserialize, Serialize};

/// Supplemental source role which binds the Incan engine to a source-built compiler.
pub const NATIVE_RUNTIME_ENGINE_SOURCE: &str = "native-runtime-engine";
/// Prepared executable installed beside its receipt-bound compiler by the Incan bootstrap.
pub const NATIVE_RUNTIME_ENGINE_FILE: &str = "native-runtime-inputs";

/// Digest an exact compiler input with replacement-sensitive local file observations.
///
/// This is only a content-digest accelerator: callers must still identify the actually loaded file and compare the
/// result with their build-bound evidence. Unix observations reject replacement and preserved-mtime edits; other
/// platforms read the bytes conservatively. A missing or malformed observation never grants an identity.
pub fn digest_native_compiler_input(path: &Path) -> std::io::Result<String> {
    super::file_freshness::digest_file(path)
}

/// Forward source-current completed-output selection without the optional borrowed-tuple argument that Incan cannot
/// emit.
///
/// Removable argument-shape adapter for the minimized #872 repro in
/// `workspaces/compiler-bootstrap/repros/optional-tuple-api-gap`. All selection inputs are caller-owned;
/// target/toolchain, exact compiler, dependency and lock acceptance remain in the Incan bootstrap. The returned owners
/// retain the existing immutable execution leases. Remove this adapter when the public optional tuple parameter can be
/// called directly.
pub fn matching_project_outputs_for_incan(
    store: &oven_store::store::OvenStore,
    project_root: &Path,
    entrypoint: &Path,
    target: super::OvenBakeProjectTarget,
    profile: &str,
    source_authority: &str,
) -> CliResult<Vec<super::OvenStoredProjectOutput>> {
    super::output_selection::matching_baked_project_outputs_with_source_authority(
        store,
        project_root,
        entrypoint,
        target,
        profile,
        source_authority,
        None,
    )
}

#[derive(Serialize)]
struct DependencyRecord<'a> {
    alias: &'a str,
    package: &'a Option<String>,
    version: &'a Option<String>,
    features: Vec<String>,
    default_features: bool,
    optional: bool,
    source: serde_json::Value,
}

#[derive(Serialize)]
struct Request<'a> {
    schema: &'static str,
    operation: &'a str,
    compiler_version: String,
    sdk_codegen_revision: String,
    native_catalog: String,
    provider_records: &'a [String],
    stdlib_facets: &'a [String],
    dependency_records: Vec<DependencyRecord<'a>>,
    runtime_units: &'a [String],
}

/// Runtime projection carries its selected Loaves; dependency projection requires no runtime roots.
enum Projection<'a> {
    Runtime(&'a [String]),
    DependencyRoots,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    schema: String,
    request_digest: String,
    inputs: BTreeMap<String, String>,
}

/// One declared alias and its original immutable native-record identity.
#[derive(Serialize)]
struct OrdinaryRoot<'a> {
    alias: &'a str,
    record_identity: &'a str,
}

/// Encode declared roots identically for the Incan exchange and the final native consumer receipt.
pub(crate) fn ordinary_roots_digest(roots: &BTreeMap<String, String>) -> CliResult<String> {
    let roots = roots
        .iter()
        .map(|(alias, record_identity)| OrdinaryRoot { alias, record_identity })
        .collect::<Vec<_>>();
    Ok(oven_store::digest_bytes(&serde_json::to_vec(&roots).map_err(failure)?))
}

/// Physical identity exchange; no semantic, macro or namespace authority is conveyed by this payload.
#[derive(Serialize)]
struct OrdinaryRequest<'a> {
    schema: &'static str,
    compiler_receipt: &'a str,
    compiler_output: &'a str,
    engine_digest: &'a str,
    rustc_output: &'a str,
    rustc_commit: &'a str,
    target: &'a str,
    host: &'a str,
    profile: &'a str,
    toolchain: &'a str,
    consumer_features: Vec<String>,
    roots: Vec<OrdinaryRoot<'a>>,
    selected_records: Vec<&'a str>,
    provider_records: Vec<String>,
    stdlib_facets: Vec<String>,
    dependency_records: Vec<DependencyRecord<'a>>,
}

/// Project physical build inputs from current ordinary roots, retaining their original owners through exchange.
///
/// The caller must select roots from current declaration/lock authority and supply the actual compiler. Existing graph
/// selection revalidates each original record/native owner and complete physical edge binding without Store admission.
/// Host units must match that compiler's real host; target units match the consumer. This does not establish complete
/// semantic or macro metadata authority. The returned inputs supplement, rather than replace, that separate contract.
pub(crate) fn ordinary_runtime_inputs(
    closure: &NativeLoafClosure,
    intent: &OvenBuildIntent,
    rustc: &Path,
    providers: &[String],
    facets: &[String],
    dependencies: &[DependencySpec],
) -> CliResult<BTreeMap<String, String>> {
    // ---- Current compiler and original physical closure ----
    let bound = compiler_engine_binding()?;
    let toolchain = oven_rustc::rustc::rustc_identity(rustc).map_err(failure)?;
    let host = oven_rustc::rustc::rustc_host_target(rustc).map_err(failure)?;
    if intent.toolchain != toolchain || bound.toolchain != toolchain {
        return Err(CliError::failure(
            "ordinary native consumer differs from the bound compiler toolchain",
        ));
    }
    let rustc = rustc.canonicalize().map_err(failure)?;
    let rustc_output = oven_store::store::digest_regular_file(&rustc).map_err(failure)?.1;
    let rustc_commit = oven_rustc::rustc::rustc_commit_hash(&rustc)
        .ok_or_else(|| CliError::failure("ordinary native compiler has no current commit identity"))?;
    let selected = validate_ordinary_closure(closure, intent, &host, &rustc_output, &rustc_commit)?;
    // ---- Deterministic transport from retained capabilities ----
    let mut declarations = dependency_records(dependencies);
    declarations.sort_by(|left, right| left.alias.cmp(right.alias));
    let request = OrdinaryRequest {
        schema: "incan.ordinary-native-inputs.request/1",
        compiler_receipt: &bound.compiler_receipt,
        compiler_output: &bound.compiler_output,
        engine_digest: &bound.engine_digest,
        rustc_output: &rustc_output,
        rustc_commit: &rustc_commit,
        target: &intent.target,
        host: &host,
        profile: &intent.profile,
        toolchain: &intent.toolchain,
        consumer_features: sorted_strings(&intent.features),
        roots: selected
            .roots()
            .iter()
            .map(|(alias, record_identity)| OrdinaryRoot { alias, record_identity })
            .collect(),
        selected_records: selected.graph().units().keys().map(String::as_str).collect(),
        provider_records: sorted_strings(providers),
        stdlib_facets: sorted_strings(facets),
        dependency_records: declarations,
    };
    let bytes = serde_json::to_vec(&request).map_err(failure)?;
    let actual_engine = super::file_freshness::digest_file(&bound.engine).map_err(failure)?;
    if actual_engine != bound.engine_digest {
        return Err(CliError::failure(
            "ordinary native engine differs from the bound compiler receipt",
        ));
    }
    // ---- Bound executable response and final original-owner observation ----
    let response = execute_exchange(&bound.engine, &bytes)?;
    let roots_digest = ordinary_roots_digest(selected.roots())?;
    let records_digest = oven_store::digest_bytes(&serde_json::to_vec(&request.selected_records).map_err(failure)?);
    let features_digest = oven_store::digest_bytes(&serde_json::to_vec(&request.consumer_features).map_err(failure)?);
    let providers_digest = oven_store::digest_bytes(&serde_json::to_vec(&request.provider_records).map_err(failure)?);
    let facets_digest = oven_store::digest_bytes(&serde_json::to_vec(&request.stdlib_facets).map_err(failure)?);
    if response.schema != "incan.ordinary-native-inputs.response/1"
        || response.request_digest != oven_store::digest_bytes(&bytes)
        || response.inputs.get("ordinary-native-roots") != Some(&roots_digest)
        || response.inputs.get("ordinary-native-records") != Some(&records_digest)
        || response.inputs.get("compiler-receipt") != Some(&bound.compiler_receipt)
        || response.inputs.get("compiler-output") != Some(&bound.compiler_output)
        || response.inputs.get("native-runtime-engine") != Some(&bound.engine_digest)
        || response.inputs.get("rustc-executable") != Some(&rustc_output)
        || response.inputs.get("rustc-commit") != Some(&rustc_commit)
        || response.inputs.get("runtime-identity-domain").map(String::as_str) != Some("ordinary-native-records/1")
        || response.inputs.get("consumer-target") != Some(&intent.target)
        || response.inputs.get("consumer-host") != Some(&host)
        || response.inputs.get("consumer-profile") != Some(&intent.profile)
        || response.inputs.get("consumer-toolchain") != Some(&intent.toolchain)
        || response.inputs.get("consumer-features") != Some(&features_digest)
        || response.inputs.get("provider-plan") != Some(&providers_digest)
        || response.inputs.get("stdlib-facets") != Some(&facets_digest)
        || !response.inputs.contains_key("rust-dependencies")
        || response.inputs.len() != 16 + usize::from(!request.provider_records.is_empty())
        || (!request.provider_records.is_empty()
            && response.inputs.get("providers") != Some(&request.provider_records.join("\n")))
        || response.inputs.keys().any(|key| {
            !matches!(
                key.as_str(),
                "runtime-identity-domain"
                    | "ordinary-native-roots"
                    | "ordinary-native-records"
                    | "compiler-receipt"
                    | "compiler-output"
                    | "native-runtime-engine"
                    | "rustc-executable"
                    | "rustc-commit"
                    | "consumer-target"
                    | "consumer-host"
                    | "consumer-profile"
                    | "consumer-toolchain"
                    | "consumer-features"
                    | "provider-plan"
                    | "stdlib-facets"
                    | "rust-dependencies"
                    | "providers"
            )
        })
    {
        return Err(CliError::failure(
            "ordinary native response differs from the exact request and original records",
        ));
    }
    // Holding the original capabilities prevents pruning; revalidate bytes before handing off the response as well.
    for unit in selected.graph().units().values() {
        unit.verify().map_err(failure)?;
    }
    Ok(response.inputs)
}

/// Reuse the canonical rooted edge validator, retaining the same original capabilities without reacquiring owners.
fn validate_ordinary_closure(
    closure: &NativeLoafClosure,
    intent: &OvenBuildIntent,
    host: &str,
    rustc_output: &str,
    rustc_commit: &str,
) -> CliResult<NativeLoafClosure> {
    if closure.roots().is_empty() || intent.target.is_empty() || intent.profile.is_empty() || host.is_empty() {
        return Err(CliError::failure(
            "ordinary native runtime requires explicit roots and consumer coordinates",
        ));
    }
    // ---- Exact rooted graph validation using the existing producer authority ----
    let mut roots = Vec::new();
    for (alias, identity) in closure.roots() {
        let unit = closure
            .graph()
            .units()
            .get(identity)
            .ok_or_else(|| CliError::failure("ordinary native root has no original selected owner"))?;
        roots.push(unit.declared_root(alias).map_err(failure)?);
    }
    let selected = closure.graph().select(&roots).map_err(failure)?;
    if selected.graph().units().keys().ne(closure.graph().units().keys()) {
        return Err(CliError::failure("ordinary native closure contains unrooted records"));
    }
    // ---- Current consumer and host/compiler coordinates ----
    for unit in selected.graph().units().values() {
        let record = unit.record();
        let expected_target = match record.source.domain.as_str() {
            "host" => host,
            "target" => intent.target.as_str(),
            _ => {
                return Err(CliError::failure(
                    "ordinary native record has an unknown compilation domain",
                ));
            }
        };
        if record.recipe.intent.target != expected_target
            || record.recipe.intent.profile != intent.profile
            || record.recipe.intent.toolchain != intent.toolchain
            || record
                .recipe
                .sources
                .build_unit_inputs
                .get("native-compiler-executable")
                .map(String::as_str)
                != Some(rustc_output)
            || record
                .recipe
                .sources
                .build_unit_inputs
                .get("compiler-host")
                .map(String::as_str)
                != Some(host)
            || record
                .recipe
                .sources
                .build_unit_inputs
                .get("compiler-commit")
                .map(String::as_str)
                != Some(rustc_commit)
        {
            return Err(CliError::failure(
                "ordinary native record differs from the current consumer intent",
            ));
        }
    }
    Ok(selected)
}

/// Normalize unordered declarations before transport without changing retained record identities.
fn sorted_strings(values: &[String]) -> Vec<String> {
    let mut values = values.to_vec();
    values.sort();
    values.dedup();
    values
}

/// Preserve the existing portable dependency declaration contract for both exchange versions.
fn dependency_records(dependencies: &[DependencySpec]) -> Vec<DependencyRecord<'_>> {
    dependencies
        .iter()
        .map(|dependency| {
            let source = match &dependency.source {
                DependencySource::Registry => serde_json::json!({"kind": "registry"}),
                DependencySource::Path { .. } => serde_json::json!({"kind": "path"}),
                DependencySource::Git { url, reference } => serde_json::json!({
                    "kind": "git", "url": url, "reference": format!("{reference:?}")
                }),
            };
            DependencyRecord {
                alias: &dependency.crate_name,
                package: &dependency.package,
                version: &dependency.version,
                features: dependency.clone().normalized().features,
                default_features: dependency.default_features,
                optional: dependency.optional,
                source,
            }
        })
        .collect()
}

/// Preserve context when converting authority or exchange failures to the driver error contract.
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

/// Project inputs using the engine already bound to the active source compiler's receipt.
pub(crate) fn runtime_inputs(
    selection: &incan_provider::sdk_native::SdkNativeSelection,
    catalog: &[u8],
    providers: &[String],
    facets: &[String],
    dependencies: &[DependencySpec],
) -> CliResult<BTreeMap<String, String>> {
    let (engine, digest) = bound_compiler_engine()?;
    selection.verify()?;
    let receipts: BTreeMap<String, String> =
        serde_json::from_slice(catalog).map_err(|error| CliError::failure(error.to_string()))?;
    if receipts.len() != selection.units.len() {
        return Err(CliError::failure(
            "native runtime catalog differs from its selected units",
        ));
    }
    for unit in &selection.units {
        let key = serde_json::to_string(&unit.binding.identity_binding())
            .map_err(|error| CliError::failure(error.to_string()))?;
        if receipts.get(&key) != Some(&unit.receipt_identity) {
            return Err(CliError::failure(
                "native runtime catalog differs from its selected receipt",
            ));
        }
    }
    let required = incan_provider::sdk_native::sdk_native_runtime_loaves(
        selection,
        &oven_model::toolchain_layout::SDK_RUNTIME_CRATES,
    )?;
    exchange(
        &engine,
        &digest,
        catalog,
        providers,
        facets,
        dependencies,
        Projection::Runtime(&required),
    )
}

/// Project requested native dependency roots through an explicitly source-bound bootstrap engine.
///
/// Native plan selection subsequently checks each declaration against the catalog's admitted source, feature,
/// version and domain authority. The exchange carries no machine-local path as reusable identity.
pub(crate) fn dependency_inputs(
    engine: &Path,
    source_receipt: &oven_store::OvenReceipt,
    catalog: &[u8],
    dependencies: &[DependencySpec],
) -> CliResult<BTreeMap<String, String>> {
    let digest = source_receipt
        .sources
        .supplemental_digests
        .get(NATIVE_RUNTIME_ENGINE_SOURCE)
        .or_else(|| {
            source_receipt
                .sources
                .build_unit_inputs
                .get(NATIVE_RUNTIME_ENGINE_SOURCE)
        })
        .ok_or_else(|| CliError::failure("compiler bootstrap receipt has no native-runtime engine binding"))?;
    exchange(
        engine,
        digest,
        catalog,
        &[],
        &[],
        dependencies,
        Projection::DependencyRoots,
    )
}

/// Exact verified compiler output and adjacent engine binding, retained for one command.
#[derive(Clone)]
struct BoundCompilerEngine {
    engine: PathBuf,
    engine_digest: String,
    compiler_receipt: String,
    compiler_output: String,
    toolchain: String,
}

/// Resolve and verify the source-built compiler and its engine once during this compiler process.
pub(crate) fn bound_compiler_engine() -> CliResult<(PathBuf, String)> {
    let bound = compiler_engine_binding()?;
    Ok((bound.engine, bound.engine_digest))
}

/// Keep the legacy pair API while sharing the exact verified compiler binding with ordinary requests.
fn compiler_engine_binding() -> CliResult<BoundCompilerEngine> {
    static BOUND: OnceLock<Result<BoundCompilerEngine, String>> = OnceLock::new();
    BOUND
        .get_or_init(|| resolve_compiler_engine().map_err(|error| error.to_string()))
        .clone()
        .map_err(CliError::failure)
}

/// Verify the selected compiler's retained output and source receipt before accepting its adjacent engine.
fn resolve_compiler_engine() -> CliResult<BoundCompilerEngine> {
    let executable = std::env::current_exe().map_err(|error| CliError::failure(error.to_string()))?;
    // Native libtest hosts carry the exact compiler selected by the existing runner.
    let compiler = if executable.file_name().is_some_and(|name| name == "incan") {
        executable
    } else {
        std::env::var_os("CARGO_BIN_EXE_incan")
            .map(PathBuf::from)
            .ok_or_else(|| CliError::failure("native-runtime identity requires its receipt-bound compiler"))?
    };
    let receipt: oven_store::OvenReceipt = read_json(&PathBuf::from(format!("{}.receipt.json", compiler.display())))?;
    let identity = oven_store::receipt_identity(
        &receipt.project,
        &receipt.sources,
        &receipt.intent,
        &receipt.compatibility,
    )
    .map_err(|error| CliError::failure(error.to_string()))?;
    let unit = oven_store::build_unit_identity(
        &receipt.intent,
        &receipt.compatibility,
        &receipt.sources.build_unit_inputs,
    )
    .map_err(|error| CliError::failure(error.to_string()))?;
    if receipt.schema_version != oven_store::OVEN_RECEIPT_SCHEMA_VERSION
        || identity != receipt.identity
        || unit != receipt.build_unit_identity
    {
        return Err(CliError::failure("native-runtime compiler receipt is invalid"));
    }
    let output: serde_json::Value = read_json(&PathBuf::from(format!("{}.oven-output.json", compiler.display())))?;
    let digest = super::file_freshness::digest_file(&compiler).map_err(|error| CliError::failure(error.to_string()))?;
    if output.get("receipt_identity").and_then(serde_json::Value::as_str) != Some(receipt.identity.as_str())
        || output.get("output_digest").and_then(serde_json::Value::as_str) != Some(digest.as_str())
    {
        return Err(CliError::failure(
            "native-runtime compiler output is not bound to its source receipt",
        ));
    }
    let compiler_output = digest;
    let digest = receipt
        .sources
        .supplemental_digests
        .get(NATIVE_RUNTIME_ENGINE_SOURCE)
        .ok_or_else(|| CliError::failure("source compiler has no prepared native-runtime identity engine"))?
        .clone();
    let engine = compiler
        .parent()
        .ok_or_else(|| CliError::failure("source compiler has no parent"))?
        .join(NATIVE_RUNTIME_ENGINE_FILE);
    Ok(BoundCompilerEngine {
        engine,
        engine_digest: digest,
        compiler_receipt: receipt.identity,
        compiler_output,
        toolchain: receipt.intent.toolchain,
    })
}

/// Decode a caller-owned exchange or receipt file without suppressing IO or shape failures.
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> CliResult<T> {
    let bytes = std::fs::read(path).map_err(|error| CliError::failure(format!("{}: {error}", path.display())))?;
    serde_json::from_slice(&bytes).map_err(|error| CliError::failure(error.to_string()))
}

/// Execute the source-bound Incan projection and verify its response belongs to this exact request and catalog.
fn exchange(
    engine: &Path,
    expected_engine: &str,
    catalog: &[u8],
    providers: &[String],
    facets: &[String],
    dependencies: &[DependencySpec],
    projection: Projection<'_>,
) -> CliResult<BTreeMap<String, String>> {
    let engine_digest = super::file_freshness::digest_file(engine)
        .map_err(|error| CliError::failure(format!("{}: {error}", engine.display())))?;
    if engine_digest != expected_engine {
        return Err(CliError::failure(
            "native-runtime identity engine differs from the compiler source receipt",
        ));
    }
    let receipts: BTreeMap<String, String> =
        serde_json::from_slice(catalog).map_err(|error| CliError::failure(error.to_string()))?;
    let native_catalog = serde_json::to_string(&receipts).map_err(|error| CliError::failure(error.to_string()))?;
    let catalog_digest = oven_store::digest_bytes(native_catalog.as_bytes());
    let compiler = incan_oven_facet::compiler_identity();
    let dependency_records = dependency_records(dependencies);
    let (operation, runtime_units) = match projection {
        Projection::Runtime(units) => ("runtime", units),
        Projection::DependencyRoots => ("dependency-roots", &[][..]),
    };
    let request = Request {
        schema: "incan.native-runtime-inputs.request/1",
        operation,
        compiler_version: compiler.version,
        sdk_codegen_revision: compiler.sdk_provider_codegen_revision.to_string(),
        native_catalog,
        provider_records: providers,
        stdlib_facets: facets,
        dependency_records,
        runtime_units,
    };
    let bytes = serde_json::to_vec(&request).map_err(|error| CliError::failure(error.to_string()))?;
    let response = execute_exchange(engine, &bytes)?;
    if response.schema != "incan.native-runtime-inputs.response/1"
        || response.request_digest != oven_store::digest_bytes(&bytes)
        || response.inputs.get("sdk-native-closure") != Some(&catalog_digest)
    {
        return Err(CliError::failure(
            "native-runtime response is not bound to the current request and catalog",
        ));
    }
    Ok(response.inputs)
}

/// Execute only the caller-owned request/response files through the already verified engine.
fn execute_exchange(engine: &Path, bytes: &[u8]) -> CliResult<Response> {
    let directory = tempfile::tempdir().map_err(|error| CliError::failure(error.to_string()))?;
    let request_path = directory.path().join("request.json");
    let response_path = directory.path().join("response.json");
    std::fs::write(&request_path, &bytes).map_err(|error| CliError::failure(error.to_string()))?;
    let output = Command::new(engine)
        .arg(&request_path)
        .arg(&response_path)
        .output()
        .map_err(|error| CliError::failure(error.to_string()))?;
    if !output.status.success() {
        return Err(CliError::failure(format!(
            "Incan native-runtime identity refused: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    read_json(&response_path)
}

#[cfg(test)]
#[path = "native_runtime_inputs/tests.rs"]
mod tests;
