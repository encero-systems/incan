//! Typed exchange with the Incan native-runtime identity engine.
//!
//! Removable transport for the minimized API gap tracked by #1698. Identity projection lives in
//! `workspaces/native-runtime-inputs`; this module binds the executable, request and canonical catalog.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::error::{CliError, CliResult};
use oven_model::manifest::{DependencySource, DependencySpec};
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
    runtime_units: Vec<&'static str>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    schema: String,
    request_digest: String,
    inputs: BTreeMap<String, String>,
}

/// Project inputs using the engine already bound to the active source compiler's receipt.
pub(crate) fn runtime_inputs(
    catalog: &[u8],
    providers: &[String],
    facets: &[String],
    dependencies: &[DependencySpec],
) -> CliResult<BTreeMap<String, String>> {
    let (engine, digest) = bound_compiler_engine()?;
    exchange(&engine, &digest, catalog, providers, facets, dependencies, "runtime")
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
    exchange(engine, digest, catalog, &[], &[], dependencies, "dependency-roots")
}

/// Resolve and verify the source-built compiler and its engine once during this compiler process.
pub(crate) fn bound_compiler_engine() -> CliResult<(PathBuf, String)> {
    static BOUND: OnceLock<Result<(PathBuf, String), String>> = OnceLock::new();
    BOUND
        .get_or_init(|| resolve_compiler_engine().map_err(|error| error.to_string()))
        .clone()
        .map_err(CliError::failure)
}

/// Verify the selected compiler's retained output and source receipt before accepting its adjacent engine.
fn resolve_compiler_engine() -> CliResult<(PathBuf, String)> {
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
    Ok((engine, digest))
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
    operation: &str,
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
    let mut dependency_records = Vec::new();
    for dependency in dependencies {
        let source = match &dependency.source {
            DependencySource::Registry => serde_json::json!({"kind": "registry"}),
            DependencySource::Path { .. } => serde_json::json!({"kind": "path"}),
            DependencySource::Git { url, reference } => serde_json::json!({
                "kind": "git", "url": url, "reference": format!("{reference:?}")
            }),
        };
        dependency_records.push(DependencyRecord {
            alias: &dependency.crate_name,
            package: &dependency.package,
            version: &dependency.version,
            features: dependency.clone().normalized().features,
            default_features: dependency.default_features,
            optional: dependency.optional,
            source,
        });
    }
    let request = Request {
        schema: "incan.native-runtime-inputs.request/1",
        operation,
        compiler_version: compiler.version,
        sdk_codegen_revision: compiler.sdk_provider_codegen_revision.to_string(),
        native_catalog,
        provider_records: providers,
        stdlib_facets: facets,
        dependency_records,
        runtime_units: if operation == "runtime" {
            oven_model::toolchain_layout::SDK_RUNTIME_CRATES.to_vec()
        } else {
            Vec::new()
        },
    };
    let bytes = serde_json::to_vec(&request).map_err(|error| CliError::failure(error.to_string()))?;
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
    let response: Response = read_json(&response_path)?;
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
