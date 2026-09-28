//! Hermetic execution of typed publisher-side tool units.
//!
//! This is the shared publisher-execution boundary for native-link and generator lanes. It resolves only admitted
//! owner-relative inputs, verifies every byte before spawn, clears the child environment, confines filesystem access,
//! validates the complete product tree, and emits a relocation-independent receipt. Consumer mode refuses before
//! resolving or spawning the executable; consumers import the receipt-bound products instead.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
#[cfg(target_os = "macos")]
use std::time::Duration;

use oven_model::manifest::{
    RustFactArgument, RustFactArtifact, RustFactArtifactKind, RustFactArtifactMember, RustFactEnvironment,
    RustFactExecutable, RustFactTool,
};
use serde::{Deserialize, Serialize};

use crate::digest_bytes;
#[cfg(target_os = "macos")]
use crate::process::{BoundedProcessLimits, BoundedProcessTermination, run_bounded_process};

const PUBLISHER_TOOL_RECEIPT_DOMAIN: &str = "incan.oven.publisher-tool-receipt/1";
#[cfg(target_os = "macos")]
const PUBLISHER_TOOL_STDOUT_LIMIT: usize = 1024 * 1024;
#[cfg(target_os = "macos")]
const PUBLISHER_TOOL_STDERR_LIMIT: usize = 1024 * 1024;
#[cfg(target_os = "macos")]
const PUBLISHER_TOOL_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether the caller owns a publisher bake or an ordinary consumer bake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OvenPublisherExecutionMode {
    /// The trusted publisher may execute a fully declared tool unit.
    Publisher,
    /// An ordinary consumer must import products and never execute the tool.
    Consumer,
}

/// One caller-held immutable root paired with the identity that admitted it.
#[derive(Debug, Clone)]
pub struct OvenPublisherToolOwner<'a> {
    /// Relocation-independent owner identity.
    pub identity: String,
    /// Physical root held immutable for the complete execution.
    pub root: &'a Path,
}

/// Complete authority required to execute one typed tool unit.
#[derive(Debug)]
pub struct OvenPublisherToolRequest<'a> {
    /// Explicit publisher/consumer role; consumer mode always refuses.
    pub mode: OvenPublisherExecutionMode,
    /// Validated typed generator declaration selected from one fact record.
    pub tool: &'a RustFactTool,
    /// Owner beneath which every declared tool input is resolved.
    pub fact_owner: OvenPublisherToolOwner<'a>,
    /// Owner named by the declared executable identity.
    pub executable_owner: OvenPublisherToolOwner<'a>,
    /// Exact build-host association.
    pub host: &'a str,
    /// Exact compilation-target association.
    pub target: &'a str,
    /// Sorted complete selected units that consume the generated products.
    pub consuming_units: &'a [&'a str],
    /// Empty publisher-owned root into which all declared products are written.
    pub product_root: &'a Path,
}

/// One generated product and its complete content identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenPublisherToolProduct {
    /// Stable invocation-local product name.
    pub name: String,
    /// File or complete tree product class.
    pub kind: RustFactArtifactKind,
    /// Portable path relative to the product root.
    pub path: String,
    /// Exact file digest or canonical complete-tree digest.
    pub digest: String,
    /// Sorted complete tree member catalog; empty for a file.
    pub members: Vec<RustFactArtifactMember>,
}

/// Receipt binding one publisher invocation to all executable, argument, environment, input, output, and selection
/// facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenPublisherToolReceipt {
    /// SHA-256 identity of every other receipt field under the publisher-tool domain.
    pub identity: String,
    /// Stable producer name from the admitted fact.
    pub producer: String,
    /// Fact-record owner that supplied the declared input closure.
    pub fact_owner: String,
    /// Exact executable identity verified immediately before spawn.
    pub executable: RustFactExecutable,
    /// Ordered portable argument declaration.
    pub arguments: Vec<RustFactArgument>,
    /// Sorted complete child environment after ambient values were cleared.
    pub environment: Vec<RustFactEnvironment>,
    /// Sorted complete admitted input closure.
    pub inputs: Vec<RustFactArtifact>,
    /// Sorted complete generated product closure.
    pub outputs: Vec<OvenPublisherToolProduct>,
    /// Exact build host on which the publisher ran.
    pub host: String,
    /// Exact compilation target for which products were generated.
    pub target: String,
    /// Sorted complete selected consumer identities.
    pub consuming_units: Vec<String>,
}

/// Recompute the relocation-independent identity of a publisher-tool receipt.
pub fn publisher_tool_receipt_identity(receipt: &OvenPublisherToolReceipt) -> Result<String, OvenPublisherToolError> {
    let input = OvenPublisherToolReceiptIdentity {
        producer: &receipt.producer,
        fact_owner: &receipt.fact_owner,
        executable: &receipt.executable,
        arguments: &receipt.arguments,
        environment: &receipt.environment,
        inputs: &receipt.inputs,
        outputs: &receipt.outputs,
        host: &receipt.host,
        target: &receipt.target,
        consuming_units: &receipt.consuming_units,
    };
    encode_receipt_identity(&receipt.producer, input)
}

/// Verify that a publisher-tool receipt still binds all of its public fields.
pub fn verify_publisher_tool_receipt(receipt: &OvenPublisherToolReceipt) -> Result<(), OvenPublisherToolError> {
    validate_receipt_shape(receipt)?;
    let actual = publisher_tool_receipt_identity(receipt)?;
    if actual != receipt.identity {
        return invalid(&receipt.producer, "receipt identity does not bind its public fields");
    }
    Ok(())
}

/// Refuse noncanonical or malformed receipt fields even when an attacker can recompute the outer digest.
fn validate_receipt_shape(receipt: &OvenPublisherToolReceipt) -> Result<(), OvenPublisherToolError> {
    if receipt.producer.is_empty()
        || receipt.host.is_empty()
        || receipt.target.is_empty()
        || !is_sha256_identity(&receipt.fact_owner)
        || !is_sha256_identity(&receipt.executable.owner)
        || !is_sha256_identity(&receipt.executable.digest)
    {
        return invalid(
            &receipt.producer,
            "receipt has malformed producer, owner, executable, host, or target facts",
        );
    }
    if receipt.consuming_units.is_empty()
        || receipt
            .consuming_units
            .iter()
            .any(|identity| !is_sha256_identity(identity))
        || receipt.consuming_units.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return invalid(
            &receipt.producer,
            "receipt consumer identities must be non-empty, sorted, unique SHA-256 identities",
        );
    }
    if receipt.outputs.is_empty() || receipt.outputs.windows(2).any(|pair| pair[0].name >= pair[1].name) {
        return invalid(
            &receipt.producer,
            "receipt outputs must be non-empty and sorted by unique logical name",
        );
    }
    for output in &receipt.outputs {
        if !is_sha256_identity(&output.digest)
            || join_portable(Path::new("."), &output.path, &receipt.producer, "receipt output").is_err()
            || output.members.windows(2).any(|pair| pair[0].path >= pair[1].path)
            || output.members.iter().any(|member| {
                !is_sha256_identity(&member.digest)
                    || join_portable(Path::new("."), &member.path, &receipt.producer, "receipt member").is_err()
            })
        {
            return invalid(&receipt.producer, "receipt output catalog is malformed or noncanonical");
        }
        match output.kind {
            RustFactArtifactKind::File if !output.members.is_empty() => {
                return invalid(&receipt.producer, "receipt file outputs cannot carry tree members");
            }
            RustFactArtifactKind::Tree if digest_tree_members(&output.members, &receipt.producer)? != output.digest => {
                return invalid(
                    &receipt.producer,
                    "receipt tree output digest does not match its member catalog",
                );
            }
            RustFactArtifactKind::File | RustFactArtifactKind::Tree => {}
        }
    }
    Ok(())
}

/// Failure to execute or receipt one publisher tool unit.
#[derive(Debug, thiserror::Error)]
pub enum OvenPublisherToolError {
    /// Consumer code attempted to run publisher-only work.
    #[error("consumer bakes never execute publisher tool `{producer}`; import its admitted products")]
    ConsumerExecution { producer: String },
    /// A selected declaration or request is malformed or inconsistent.
    #[error("invalid publisher tool `{producer}`: {message}")]
    Invalid { producer: String, message: String },
    /// A declared or generated filesystem entry could not be inspected.
    #[error("publisher tool `{producer}` could not access {path}: {source}")]
    Io {
        producer: String,
        path: PathBuf,
        source: std::io::Error,
    },
    /// The bounded, sandboxed child did not complete successfully.
    #[error("publisher tool `{producer}` failed: {message}")]
    Execution { producer: String, message: String },
    /// Canonical receipt encoding failed.
    #[error("publisher tool `{producer}` receipt could not be encoded: {message}")]
    Receipt { producer: String, message: String },
}

#[derive(Serialize)]
struct OvenPublisherToolReceiptIdentity<'a> {
    producer: &'a str,
    fact_owner: &'a str,
    executable: &'a RustFactExecutable,
    arguments: &'a [RustFactArgument],
    environment: &'a [RustFactEnvironment],
    inputs: &'a [RustFactArtifact],
    outputs: &'a [OvenPublisherToolProduct],
    host: &'a str,
    target: &'a str,
    consuming_units: &'a [String],
}

/// Execute one exact declared generator in a publisher-only filesystem sandbox and receipt its products.
///
/// Inputs remain under their immutable admitted owner and are mounted read-only by the sandbox. Only the declared
/// product root is writable. On macOS, `sandbox-exec` enforces the read/write boundary; other hosts fail closed until
/// they provide an equivalent process-tree filesystem confinement primitive.
pub fn execute_publisher_tool(
    request: &OvenPublisherToolRequest<'_>,
) -> Result<OvenPublisherToolReceipt, OvenPublisherToolError> {
    let producer = request.tool.name.clone();
    if request.mode != OvenPublisherExecutionMode::Publisher {
        return Err(OvenPublisherToolError::ConsumerExecution { producer });
    }
    validate_request(request)?;

    let executable = resolve_regular_file(
        request.executable_owner.root,
        &request.tool.executable.path,
        &producer,
        "executable",
    )?;
    verify_file_digest(&executable, &request.tool.executable.digest, &producer, "executable")?;
    let inputs = materialize_inputs(request)?;
    prepare_product_root(request)?;
    // Products are named to the child by the root's resolved spelling, which is the only one confinement admits.
    let product_root = canonical_directory(request.product_root, &producer, "product")?;
    let request = &OvenPublisherToolRequest {
        fact_owner: request.fact_owner.clone(),
        executable_owner: request.executable_owner.clone(),
        product_root: &product_root,
        ..*request
    };
    let outputs = output_paths(request)?;
    let arguments = materialize_arguments(request, &inputs, &outputs)?;
    let environment = materialize_environment(request, &inputs)?;
    run_tool(request, &executable, &arguments, &environment, &inputs)?;
    verify_file_digest(
        &executable,
        &request.tool.executable.digest,
        &producer,
        "executable after execution",
    )?;
    let _verified_inputs_after_execution = materialize_inputs(request)?;
    let products = collect_products(request)?;
    let consuming_units = request
        .consuming_units
        .iter()
        .map(|unit| (*unit).to_string())
        .collect::<Vec<_>>();
    let identity_input = OvenPublisherToolReceiptIdentity {
        producer: &request.tool.name,
        fact_owner: &request.fact_owner.identity,
        executable: &request.tool.executable,
        arguments: &request.tool.arguments,
        environment: &request.tool.environment,
        inputs: &request.tool.inputs,
        outputs: &products,
        host: request.host,
        target: request.target,
        consuming_units: &consuming_units,
    };
    let identity = encode_receipt_identity(&producer, identity_input)?;
    Ok(OvenPublisherToolReceipt {
        identity,
        producer,
        fact_owner: request.fact_owner.identity.clone(),
        executable: request.tool.executable.clone(),
        arguments: request.tool.arguments.clone(),
        environment: request.tool.environment.clone(),
        inputs: request.tool.inputs.clone(),
        outputs: products,
        host: request.host.to_string(),
        target: request.target.to_string(),
        consuming_units,
    })
}

/// Encode the single canonical identity projection used by execution and later verification.
fn encode_receipt_identity(
    producer: &str,
    input: OvenPublisherToolReceiptIdentity<'_>,
) -> Result<String, OvenPublisherToolError> {
    let encoded = serde_json::to_vec(&(PUBLISHER_TOOL_RECEIPT_DOMAIN, input)).map_err(|error| {
        OvenPublisherToolError::Receipt {
            producer: producer.to_string(),
            message: error.to_string(),
        }
    })?;
    Ok(digest_bytes(&encoded))
}

/// Require canonical request facts before any filesystem access or process spawn.
fn validate_request(request: &OvenPublisherToolRequest<'_>) -> Result<(), OvenPublisherToolError> {
    let producer = request.tool.name.as_str();
    if request.tool.target != request.target {
        return invalid(producer, "tool target does not equal the selected compilation target");
    }
    if request.executable_owner.identity != request.tool.executable.owner {
        return invalid(producer, "executable owner does not match its declared identity");
    }
    if !is_sha256_identity(&request.fact_owner.identity)
        || !is_sha256_identity(&request.executable_owner.identity)
        || !is_sha256_identity(&request.tool.executable.digest)
    {
        return invalid(
            producer,
            "fact, executable owner, and executable byte identities must be SHA-256 identities",
        );
    }
    if request.host.trim().is_empty() || request.target.trim().is_empty() {
        return invalid(producer, "host and target associations must be non-empty");
    }
    if request
        .consuming_units
        .windows(2)
        .any(|pair| pair[0].is_empty() || pair[0] >= pair[1])
        || request.consuming_units.iter().any(|unit| !is_sha256_identity(unit))
    {
        return invalid(producer, "consuming units must be sorted, unique, and non-empty");
    }
    if request.consuming_units.is_empty() {
        return invalid(producer, "at least one consuming unit is required");
    }
    if request.tool.outputs.is_empty() {
        return invalid(producer, "the complete output contract must be non-empty");
    }
    if request.tool.inputs.windows(2).any(|pair| pair[0].name >= pair[1].name)
        || request.tool.outputs.windows(2).any(|pair| pair[0].name >= pair[1].name)
        || request
            .tool
            .environment
            .windows(2)
            .any(|pair| pair[0].name >= pair[1].name)
    {
        return invalid(
            producer,
            "inputs, outputs, and environment must be sorted by unique logical name",
        );
    }
    Ok(())
}

/// Verify every declared input and return its resolved physical path by logical name.
fn materialize_inputs(
    request: &OvenPublisherToolRequest<'_>,
) -> Result<BTreeMap<String, PathBuf>, OvenPublisherToolError> {
    let mut resolved = BTreeMap::new();
    for input in &request.tool.inputs {
        let path = match input.kind {
            RustFactArtifactKind::File => {
                let path = resolve_regular_file(request.fact_owner.root, &input.path, &request.tool.name, "input")?;
                verify_file_digest(&path, &input.digest, &request.tool.name, "input")?;
                if !input.members.is_empty() {
                    return invalid(&request.tool.name, "a file input cannot carry tree members");
                }
                path
            }
            RustFactArtifactKind::Tree => {
                let path = resolve_directory(request.fact_owner.root, &input.path, &request.tool.name, "input tree")?;
                let members = collect_tree_members(&path, &request.tool.name)?;
                if members != input.members {
                    return invalid(
                        &request.tool.name,
                        &format!(
                            "input tree `{}` does not match its complete declared member catalog",
                            input.name
                        ),
                    );
                }
                if digest_tree_members(&members, &request.tool.name)? != input.digest {
                    return invalid(
                        &request.tool.name,
                        &format!("input tree `{}` digest changed", input.name),
                    );
                }
                path
            }
        };
        if resolved.insert(input.name.clone(), path).is_some() {
            return invalid(&request.tool.name, "input names must be unique");
        }
    }
    Ok(resolved)
}

/// Require an existing empty, non-symlink product root before the child receives it.
fn prepare_product_root(request: &OvenPublisherToolRequest<'_>) -> Result<(), OvenPublisherToolError> {
    let metadata = fs::symlink_metadata(request.product_root).map_err(|source| OvenPublisherToolError::Io {
        producer: request.tool.name.clone(),
        path: request.product_root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return invalid(&request.tool.name, "product root must be a real directory");
    }
    let mut entries = fs::read_dir(request.product_root).map_err(|source| OvenPublisherToolError::Io {
        producer: request.tool.name.clone(),
        path: request.product_root.to_path_buf(),
        source,
    })?;
    if entries
        .next()
        .transpose()
        .map_err(|source| OvenPublisherToolError::Io {
            producer: request.tool.name.clone(),
            path: request.product_root.to_path_buf(),
            source,
        })?
        .is_some()
    {
        return invalid(&request.tool.name, "product root must be empty before execution");
    }
    Ok(())
}

/// Resolve declared product paths, creating only their declared parent directories.
fn output_paths(request: &OvenPublisherToolRequest<'_>) -> Result<BTreeMap<String, PathBuf>, OvenPublisherToolError> {
    let mut outputs = BTreeMap::new();
    for output in &request.tool.outputs {
        let path = join_portable(request.product_root, &output.path, &request.tool.name, "output")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| OvenPublisherToolError::Io {
                producer: request.tool.name.clone(),
                path: parent.to_path_buf(),
                source,
            })?;
        }
        if outputs.insert(output.name.clone(), path).is_some() {
            return invalid(&request.tool.name, "output names must be unique");
        }
    }
    Ok(outputs)
}

/// Convert typed argument references into exact verified physical paths without command discovery.
fn materialize_arguments(
    request: &OvenPublisherToolRequest<'_>,
    inputs: &BTreeMap<String, PathBuf>,
    outputs: &BTreeMap<String, PathBuf>,
) -> Result<Vec<String>, OvenPublisherToolError> {
    request
        .tool
        .arguments
        .iter()
        .map(|argument| match argument {
            RustFactArgument::Literal { literal } => Ok(literal.clone()),
            RustFactArgument::Input { input } => inputs
                .get(input)
                .map(|path| path.to_string_lossy().into_owned())
                .ok_or_else(|| {
                    invalid_error(
                        &request.tool.name,
                        &format!("argument names undeclared input `{input}`"),
                    )
                }),
            RustFactArgument::Output { output } => outputs
                .get(output)
                .map(|path| path.to_string_lossy().into_owned())
                .ok_or_else(|| {
                    invalid_error(
                        &request.tool.name,
                        &format!("argument names undeclared output `{output}`"),
                    )
                }),
        })
        .collect()
}

/// Build the complete child environment after ambient environment removal.
fn materialize_environment(
    request: &OvenPublisherToolRequest<'_>,
    inputs: &BTreeMap<String, PathBuf>,
) -> Result<BTreeMap<String, String>, OvenPublisherToolError> {
    let mut environment = BTreeMap::new();
    for entry in &request.tool.environment {
        let value = match (&entry.literal, &entry.input) {
            (Some(value), None) => value.clone(),
            (None, Some(input)) => inputs
                .get(input)
                .map(|path| path.to_string_lossy().into_owned())
                .ok_or_else(|| {
                    invalid_error(
                        &request.tool.name,
                        &format!("environment names undeclared input `{input}`"),
                    )
                })?,
            _ => return invalid(&request.tool.name, "environment entries require exactly one value form"),
        };
        if environment.insert(entry.name.clone(), value).is_some() {
            return invalid(&request.tool.name, "environment names must be unique");
        }
    }
    Ok(environment)
}

/// Run the exact executable through the host confinement primitive with no inherited environment or PATH lookup.
#[cfg(target_os = "macos")]
fn run_tool(
    request: &OvenPublisherToolRequest<'_>,
    executable: &Path,
    arguments: &[String],
    environment: &BTreeMap<String, String>,
    inputs: &BTreeMap<String, PathBuf>,
) -> Result<(), OvenPublisherToolError> {
    let profile = macos_sandbox_profile(executable, request.product_root, inputs.values());
    let mut command = Command::new("/usr/bin/sandbox-exec");
    command
        .arg("-p")
        .arg(profile)
        .arg(executable)
        .args(arguments)
        .current_dir(request.product_root)
        .env_clear()
        .envs(environment);
    let output = run_bounded_process(
        &mut command,
        BoundedProcessLimits {
            stdout_bytes: PUBLISHER_TOOL_STDOUT_LIMIT,
            stderr_bytes: PUBLISHER_TOOL_STDERR_LIMIT,
            timeout: Some(PUBLISHER_TOOL_TIMEOUT),
        },
        None,
    )
    .map_err(|source| OvenPublisherToolError::Io {
        producer: request.tool.name.clone(),
        path: executable.to_path_buf(),
        source,
    })?;
    if output.termination != BoundedProcessTermination::Completed || !output.status.success() {
        return Err(OvenPublisherToolError::Execution {
            producer: request.tool.name.clone(),
            message: format!(
                "termination {:?}, status {}, stderr {}",
                output.termination,
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }
    Ok(())
}

/// Fail closed on hosts without an installed process-tree filesystem confinement implementation.
#[cfg(not(target_os = "macos"))]
fn run_tool(
    request: &OvenPublisherToolRequest<'_>,
    _executable: &Path,
    _arguments: &[String],
    _environment: &BTreeMap<String, String>,
    _inputs: &BTreeMap<String, PathBuf>,
) -> Result<(), OvenPublisherToolError> {
    Err(OvenPublisherToolError::Execution {
        producer: request.tool.name.clone(),
        message: "publisher tool filesystem confinement is not available on this host".to_string(),
    })
}

/// Construct a deny-by-default macOS filesystem policy for exactly the executable, declared inputs, and product root.
///
/// Besides those, the policy admits only what process startup itself reads: the root directory entry (the dynamic
/// loader aborts the process without it), the system library trees, and `/bin/sh` together with
/// `/private/var/select/sh`, the host's selection of the shell `/bin/sh` delegates to.
#[cfg(target_os = "macos")]
fn macos_sandbox_profile<'a>(
    executable: &Path,
    product_root: &Path,
    inputs: impl Iterator<Item = &'a PathBuf>,
) -> String {
    let mut reads = vec![
        format!("(literal \"{}\")", sandbox_path(executable)),
        "(literal \"/\")".to_string(),
        "(literal \"/bin/sh\")".to_string(),
        "(literal \"/private/var/select/sh\")".to_string(),
        "(subpath \"/usr/lib\")".to_string(),
        "(subpath \"/System/Library\")".to_string(),
        format!("(subpath \"{}\")", sandbox_path(product_root)),
    ];
    reads.extend(inputs.map(|path| {
        if path.is_dir() {
            format!("(subpath \"{}\")", sandbox_path(path))
        } else {
            format!("(literal \"{}\")", sandbox_path(path))
        }
    }));
    format!(
        "(version 1)\n(deny default)\n(allow process*)\n(allow sysctl-read)\n(allow mach-lookup)\n(allow file-read* {})\n(allow file-write* (subpath \"{}\"))\n",
        reads.join(" "),
        sandbox_path(product_root),
    )
}

/// Escape one canonical path for a sandbox profile string literal.
#[cfg(target_os = "macos")]
fn sandbox_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"")
}

/// Validate the declared output set against every generated filesystem entry and compute exact product identities.
fn collect_products(
    request: &OvenPublisherToolRequest<'_>,
) -> Result<Vec<OvenPublisherToolProduct>, OvenPublisherToolError> {
    let mut allowed_files = BTreeSet::new();
    let mut allowed_directories = BTreeSet::new();
    let mut products = Vec::with_capacity(request.tool.outputs.len());
    for output in &request.tool.outputs {
        let path = join_portable(request.product_root, &output.path, &request.tool.name, "output")?;
        let (digest, members) = match output.kind {
            RustFactArtifactKind::File => {
                let path = require_regular_file(&path, &request.tool.name, "output")?;
                allowed_files.insert(path.clone());
                (digest_file(&path, &request.tool.name)?, Vec::new())
            }
            RustFactArtifactKind::Tree => {
                let path = require_directory(&path, &request.tool.name, "output tree")?;
                allowed_directories.insert(path.clone());
                let members = collect_tree_members(&path, &request.tool.name)?;
                for member in &members {
                    allowed_files.insert(path.join(&member.path));
                }
                (digest_tree_members(&members, &request.tool.name)?, members)
            }
        };
        products.push(OvenPublisherToolProduct {
            name: output.name.clone(),
            kind: output.kind,
            path: output.path.clone(),
            digest,
            members,
        });
    }
    refuse_extra_products(
        request.product_root,
        request.product_root,
        &allowed_files,
        &allowed_directories,
        &request.tool.name,
    )?;
    Ok(products)
}

/// Walk the product root without following links and reject anything outside the declared output closure.
fn refuse_extra_products(
    root: &Path,
    directory: &Path,
    allowed_files: &BTreeSet<PathBuf>,
    allowed_directories: &BTreeSet<PathBuf>,
    producer: &str,
) -> Result<(), OvenPublisherToolError> {
    let entries = fs::read_dir(directory).map_err(|source| OvenPublisherToolError::Io {
        producer: producer.to_string(),
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| OvenPublisherToolError::Io {
            producer: producer.to_string(),
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| OvenPublisherToolError::Io {
            producer: producer.to_string(),
            path: path.clone(),
            source,
        })?;
        if file_type.is_symlink() {
            return invalid(producer, "generated products cannot contain symlinks");
        }
        if file_type.is_file() {
            if !allowed_files.contains(&path) {
                return invalid(
                    producer,
                    &format!(
                        "undeclared product `{}` was generated",
                        portable_from(root, &path, producer)?
                    ),
                );
            }
        } else if file_type.is_dir() {
            let contains_declared = allowed_files.iter().any(|file| file.starts_with(&path))
                || allowed_directories
                    .iter()
                    .any(|declared| declared == &path || declared.starts_with(&path));
            if !contains_declared {
                return invalid(
                    producer,
                    &format!(
                        "undeclared product directory `{}` was generated",
                        portable_from(root, &path, producer)?
                    ),
                );
            }
            refuse_extra_products(root, &path, allowed_files, allowed_directories, producer)?;
        } else {
            return invalid(producer, "generated products must be regular files or directories");
        }
    }
    Ok(())
}

/// Collect a sorted, complete regular-file catalog without following symlinks or accepting special files.
fn collect_tree_members(root: &Path, producer: &str) -> Result<Vec<RustFactArtifactMember>, OvenPublisherToolError> {
    let mut members = Vec::new();
    collect_tree_members_from(root, root, producer, &mut members)?;
    members.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(members)
}

/// Recursive implementation of complete tree catalog collection.
fn collect_tree_members_from(
    root: &Path,
    directory: &Path,
    producer: &str,
    members: &mut Vec<RustFactArtifactMember>,
) -> Result<(), OvenPublisherToolError> {
    let entries = fs::read_dir(directory).map_err(|source| OvenPublisherToolError::Io {
        producer: producer.to_string(),
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| OvenPublisherToolError::Io {
            producer: producer.to_string(),
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| OvenPublisherToolError::Io {
            producer: producer.to_string(),
            path: path.clone(),
            source,
        })?;
        if file_type.is_symlink() {
            return invalid(producer, "artifact trees cannot contain symlinks");
        }
        if file_type.is_dir() {
            collect_tree_members_from(root, &path, producer, members)?;
        } else if file_type.is_file() {
            members.push(RustFactArtifactMember {
                path: portable_from(root, &path, producer)?,
                digest: digest_file(&path, producer)?,
            });
        } else {
            return invalid(
                producer,
                "artifact trees may contain only regular files and directories",
            );
        }
    }
    Ok(())
}

/// Hash a tree's complete portable member catalog under an explicit stable domain.
fn digest_tree_members(members: &[RustFactArtifactMember], producer: &str) -> Result<String, OvenPublisherToolError> {
    let bytes = serde_json::to_vec(&("incan.oven.publisher-artifact-tree/1", members)).map_err(|error| {
        OvenPublisherToolError::Receipt {
            producer: producer.to_string(),
            message: error.to_string(),
        }
    })?;
    Ok(digest_bytes(&bytes))
}

/// Resolve one owner-relative path and require a regular file with no symlink escape.
fn resolve_regular_file(
    root: &Path,
    relative: &str,
    producer: &str,
    field: &str,
) -> Result<PathBuf, OvenPublisherToolError> {
    let path = join_portable(root, relative, producer, field)?;
    let canonical_root = canonical_directory(root, producer, field)?;
    let canonical = fs::canonicalize(&path).map_err(|source| OvenPublisherToolError::Io {
        producer: producer.to_string(),
        path: path.clone(),
        source,
    })?;
    if !canonical.starts_with(&canonical_root) {
        return invalid(producer, &format!("{field} escapes its admitted owner"));
    }
    require_regular_file(&canonical, producer, field)
}

/// Resolve one owner-relative path and require a real directory with no symlink escape.
fn resolve_directory(
    root: &Path,
    relative: &str,
    producer: &str,
    field: &str,
) -> Result<PathBuf, OvenPublisherToolError> {
    let path = join_portable(root, relative, producer, field)?;
    let canonical_root = canonical_directory(root, producer, field)?;
    let canonical = fs::canonicalize(&path).map_err(|source| OvenPublisherToolError::Io {
        producer: producer.to_string(),
        path: path.clone(),
        source,
    })?;
    if !canonical.starts_with(&canonical_root) {
        return invalid(producer, &format!("{field} escapes its admitted owner"));
    }
    require_directory(&canonical, producer, field)
}

/// Canonicalize a real directory root used as physical authority.
fn canonical_directory(root: &Path, producer: &str, field: &str) -> Result<PathBuf, OvenPublisherToolError> {
    let metadata = fs::symlink_metadata(root).map_err(|source| OvenPublisherToolError::Io {
        producer: producer.to_string(),
        path: root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return invalid(producer, &format!("{field} owner root must be a real directory"));
    }
    fs::canonicalize(root).map_err(|source| OvenPublisherToolError::Io {
        producer: producer.to_string(),
        path: root.to_path_buf(),
        source,
    })
}

/// Require one existing path to be a non-symlink regular file.
fn require_regular_file(path: &Path, producer: &str, field: &str) -> Result<PathBuf, OvenPublisherToolError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenPublisherToolError::Io {
        producer: producer.to_string(),
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return invalid(producer, &format!("{field} must be a regular non-symlink file"));
    }
    Ok(path.to_path_buf())
}

/// Require one existing path to be a non-symlink directory.
fn require_directory(path: &Path, producer: &str, field: &str) -> Result<PathBuf, OvenPublisherToolError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenPublisherToolError::Io {
        producer: producer.to_string(),
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return invalid(producer, &format!("{field} must be a real directory"));
    }
    Ok(path.to_path_buf())
}

/// Join a portable path only when every component is a normal relative component.
fn join_portable(root: &Path, relative: &str, producer: &str, field: &str) -> Result<PathBuf, OvenPublisherToolError> {
    let path = Path::new(relative);
    if relative.is_empty() || path.is_absolute() || path.components().any(|part| !matches!(part, Component::Normal(_)))
    {
        return invalid(
            producer,
            &format!("{field} path `{relative}` is not a plain relative path"),
        );
    }
    Ok(root.join(path))
}

/// Verify a file's exact declared SHA-256 identity.
fn verify_file_digest(path: &Path, expected: &str, producer: &str, field: &str) -> Result<(), OvenPublisherToolError> {
    let actual = digest_file(path, producer)?;
    if actual != expected {
        return invalid(
            producer,
            &format!("{field} digest changed: expected {expected}, found {actual}"),
        );
    }
    Ok(())
}

/// Hash one regular file without interpreting its contents.
fn digest_file(path: &Path, producer: &str) -> Result<String, OvenPublisherToolError> {
    let bytes = fs::read(path).map_err(|source| OvenPublisherToolError::Io {
        producer: producer.to_string(),
        path: path.to_path_buf(),
        source,
    })?;
    Ok(digest_bytes(&bytes))
}

/// Render one portable path from a verified root/path pair.
fn portable_from(root: &Path, path: &Path, producer: &str) -> Result<String, OvenPublisherToolError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| invalid_error(producer, "artifact path escaped its declared root"))?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

/// Return whether one identity uses Oven's canonical lowercase SHA-256 rendering.
fn is_sha256_identity(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Construct a typed invalid-request error for iterator adapters.
fn invalid_error(producer: &str, message: &str) -> OvenPublisherToolError {
    OvenPublisherToolError::Invalid {
        producer: producer.to_string(),
        message: message.to_string(),
    }
}

/// Return a typed invalid-request result without repeating producer conversion.
fn invalid<T>(producer: &str, message: &str) -> Result<T, OvenPublisherToolError> {
    Err(invalid_error(producer, message))
}
