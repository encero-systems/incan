//! Receipt-bound, hermetic publisher execution for native-link and generator work.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;

use crate::process::BoundedProcessLimits;
#[cfg(target_os = "macos")]
use crate::process::{BoundedProcessTermination, run_bounded_process};

/// Launch one exact executable with a cleared environment under the host confinement primitive.
///
/// Callers must verify the executable and complete input closure before this boundary and verify the complete output
/// closure afterwards. The shared launcher admits reads only from those verified paths and writes only below the
/// private product root. Hosts without an equivalent process-tree filesystem sandbox fail closed.
#[cfg(target_os = "macos")]
fn run_hermetic_process(
    executable: &Path,
    arguments: &[OsString],
    environment: &BTreeMap<String, OsString>,
    inputs: &[PathBuf],
    product_root: &Path,
    limits: BoundedProcessLimits,
) -> Result<(), String> {
    // Seatbelt matches resolved paths, so a product root reached through a symlink (`/var` is `/private/var`) must be
    // spelled canonically or every write below it is refused.
    let product_root = std::fs::canonicalize(product_root)
        .map_err(|error| format!("could not resolve product root {}: {error}", product_root.display()))?;
    let profile = macos_sandbox_profile(executable, &product_root, inputs);
    let mut command = Command::new("/usr/bin/sandbox-exec");
    command
        .arg("-p")
        .arg(profile)
        .arg(executable)
        .args(arguments)
        .current_dir(&product_root)
        .env_clear()
        .envs(environment);
    let output = run_bounded_process(&mut command, limits, None)
        .map_err(|error| format!("could not launch {}: {error}", executable.display()))?;
    if output.termination != BoundedProcessTermination::Completed || !output.status.success() {
        return Err(format!(
            "termination {:?}, status {}, stderr {}",
            output.termination,
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// Refuse publisher execution when no process-tree filesystem confinement primitive is implemented.
#[cfg(not(target_os = "macos"))]
fn run_hermetic_process(
    _executable: &Path,
    _arguments: &[OsString],
    _environment: &BTreeMap<String, OsString>,
    _inputs: &[PathBuf],
    _product_root: &Path,
    _limits: BoundedProcessLimits,
) -> Result<(), String> {
    Err("publisher filesystem confinement is not available on this host".to_string())
}

/// Construct the deny-by-default macOS policy shared by link and tool publishers.
///
/// Besides the verified executable, inputs and product root, the policy admits only what process startup itself reads:
/// the root directory entry (the dynamic loader aborts the process without it), the system library trees, and `/bin/sh`
/// together with `/private/var/select/sh`, the host's selection of the shell `/bin/sh` delegates to.
#[cfg(target_os = "macos")]
fn macos_sandbox_profile(executable: &Path, product_root: &Path, inputs: &[PathBuf]) -> String {
    let mut reads = vec![
        format!("(literal \"{}\")", sandbox_path(executable)),
        "(literal \"/\")".to_string(),
        "(literal \"/bin/sh\")".to_string(),
        "(literal \"/private/var/select/sh\")".to_string(),
        "(subpath \"/usr/lib\")".to_string(),
        "(subpath \"/System/Library\")".to_string(),
        format!("(subpath \"{}\")", sandbox_path(product_root)),
    ];
    reads.extend(inputs.iter().map(|path| {
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

/// Escape one canonical path for a Seatbelt string literal.
#[cfg(target_os = "macos")]
fn sandbox_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"")
}

mod tool_execution {
    //! Hermetic execution of typed publisher-side tool units.
    //!
    //! This is the shared publisher-execution boundary for native-link and generator lanes. It resolves only admitted
    //! owner-relative inputs, verifies every byte before spawn, clears the child environment, confines filesystem
    //! access, validates the complete product tree, and emits a relocation-independent receipt. Consumer mode
    //! refuses before resolving or spawning the executable; consumers import the receipt-bound products instead.

    use std::collections::{BTreeMap, BTreeSet};
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Component, Path, PathBuf};
    use std::time::Duration;

    use oven_model::manifest::{
        RustFactArgument, RustFactArtifact, RustFactArtifactKind, RustFactArtifactMember, RustFactEnvironment,
        RustFactExecutable, RustFactTool,
    };
    use serde::{Deserialize, Serialize};

    use crate::digest_bytes;
    use crate::process::BoundedProcessLimits;

    const PUBLISHER_TOOL_RECEIPT_DOMAIN: &str = "incan.oven.publisher-tool-receipt/1";
    const PUBLISHER_TOOL_STDOUT_LIMIT: usize = 1024 * 1024;
    const PUBLISHER_TOOL_STDERR_LIMIT: usize = 1024 * 1024;
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
    pub fn publisher_tool_receipt_identity(
        receipt: &OvenPublisherToolReceipt,
    ) -> Result<String, OvenPublisherToolError> {
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
                RustFactArtifactKind::Tree
                    if digest_tree_members(&output.members, &receipt.producer)? != output.digest =>
                {
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
    /// product root is writable. On macOS, `sandbox-exec` enforces the read/write boundary; other hosts fail closed
    /// until they provide an equivalent process-tree filesystem confinement primitive.
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
                    let path =
                        resolve_directory(request.fact_owner.root, &input.path, &request.tool.name, "input tree")?;
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
    fn output_paths(
        request: &OvenPublisherToolRequest<'_>,
    ) -> Result<BTreeMap<String, PathBuf>, OvenPublisherToolError> {
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
            if output.kind == RustFactArtifactKind::Tree {
                fs::create_dir(&path).map_err(|source| OvenPublisherToolError::Io {
                    producer: request.tool.name.clone(),
                    path: path.clone(),
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
    fn run_tool(
        request: &OvenPublisherToolRequest<'_>,
        executable: &Path,
        arguments: &[String],
        environment: &BTreeMap<String, String>,
        inputs: &BTreeMap<String, PathBuf>,
    ) -> Result<(), OvenPublisherToolError> {
        let arguments = arguments.iter().map(OsString::from).collect::<Vec<_>>();
        let environment = environment
            .iter()
            .map(|(name, value)| (name.clone(), OsString::from(value)))
            .collect::<BTreeMap<_, _>>();
        let inputs = inputs.values().cloned().collect::<Vec<_>>();
        super::run_hermetic_process(
            executable,
            &arguments,
            &environment,
            &inputs,
            request.product_root,
            BoundedProcessLimits {
                stdout_bytes: PUBLISHER_TOOL_STDOUT_LIMIT,
                stderr_bytes: PUBLISHER_TOOL_STDERR_LIMIT,
                timeout: Some(PUBLISHER_TOOL_TIMEOUT),
            },
        )
        .map_err(|message| OvenPublisherToolError::Execution {
            producer: request.tool.name.clone(),
            message,
        })
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
    fn collect_tree_members(
        root: &Path,
        producer: &str,
    ) -> Result<Vec<RustFactArtifactMember>, OvenPublisherToolError> {
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
    fn digest_tree_members(
        members: &[RustFactArtifactMember],
        producer: &str,
    ) -> Result<String, OvenPublisherToolError> {
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
    fn join_portable(
        root: &Path,
        relative: &str,
        producer: &str,
        field: &str,
    ) -> Result<PathBuf, OvenPublisherToolError> {
        let path = Path::new(relative);
        if relative.is_empty()
            || path.is_absolute()
            || path.components().any(|part| !matches!(part, Component::Normal(_)))
        {
            return invalid(
                producer,
                &format!("{field} path `{relative}` is not a plain relative path"),
            );
        }
        Ok(root.join(path))
    }

    /// Verify a file's exact declared SHA-256 identity.
    fn verify_file_digest(
        path: &Path,
        expected: &str,
        producer: &str,
        field: &str,
    ) -> Result<(), OvenPublisherToolError> {
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

    #[cfg(all(test, target_os = "macos"))]
    mod tests {
        use std::error::Error;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        use oven_model::manifest::{
            RustFactArgument, RustFactArtifactKind, RustFactExecutable, RustFactOutput, RustFactTool,
        };
        use tempfile::tempdir;

        use super::{
            OvenPublisherExecutionMode, OvenPublisherToolError, OvenPublisherToolOwner, OvenPublisherToolRequest,
            execute_publisher_tool,
        };
        use crate::digest_bytes;

        /// Return whether the managed host refused a nested Seatbelt profile before the fixture could run.
        fn confinement_was_denied(error: &OvenPublisherToolError) -> bool {
            matches!(
                error,
                OvenPublisherToolError::Execution { message, .. }
                    if message.contains("sandbox_apply: Operation not permitted")
            )
        }

        #[test]
        /// A declared tree exists when the tool starts, so shell redirection can create a member inside it.
        fn publisher_tool_creates_declared_tree_output_before_execution() -> Result<(), Box<dyn Error>> {
            let executable_owner = tempdir()?;
            let executable_path = executable_owner.path().join("tree-writer");
            let executable_bytes = b"#!/bin/sh\nset -eu\nprintf '%s' generated > \"$1/member.rs\"\n";
            fs::write(&executable_path, executable_bytes)?;
            let mut permissions = fs::metadata(&executable_path)?.permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&executable_path, permissions)?;

            let fact_owner = tempdir()?;
            let product_root = tempdir()?;
            let executable_owner_identity =
                crate::publisher_owner::publisher_owner_identity(executable_owner.path(), ["tree-writer"])?;
            let tool = RustFactTool {
                name: "tree-writer".to_string(),
                target: "aarch64-apple-darwin".to_string(),
                executable: RustFactExecutable {
                    name: "tree-writer".to_string(),
                    owner: executable_owner_identity.clone(),
                    path: "tree-writer".to_string(),
                    digest: digest_bytes(executable_bytes),
                },
                arguments: vec![RustFactArgument::Output {
                    output: "generated-tree".to_string(),
                }],
                environment: Vec::new(),
                inputs: Vec::new(),
                outputs: vec![RustFactOutput {
                    name: "generated-tree".to_string(),
                    kind: RustFactArtifactKind::Tree,
                    path: "generated".to_string(),
                }],
            };
            let request = OvenPublisherToolRequest {
                mode: OvenPublisherExecutionMode::Publisher,
                tool: &tool,
                fact_owner: OvenPublisherToolOwner {
                    identity: digest_bytes(b"fixture fact owner"),
                    root: fact_owner.path(),
                },
                executable_owner: OvenPublisherToolOwner {
                    identity: executable_owner_identity,
                    root: executable_owner.path(),
                },
                host: "aarch64-apple-darwin",
                target: "aarch64-apple-darwin",
                consuming_units: &["sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"],
                product_root: product_root.path(),
            };

            match execute_publisher_tool(&request) {
                Ok(receipt) => assert_eq!(receipt.outputs[0].members[0].path, "member.rs"),
                Err(error) if confinement_was_denied(&error) => return Ok(()),
                Err(error) => return Err(error.into()),
            }
            assert_eq!(fs::read(product_root.path().join("generated/member.rs"))?, b"generated");
            Ok(())
        }
    }
}

pub use tool_execution::*;

mod link_execution {
    //! Receipt-bound execution shared by publisher-only native-link and generator work.

    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::io::Write;
    use std::path::{Component, Path, PathBuf};

    use serde::{Deserialize, Serialize};

    use crate::digest_bytes;
    use crate::process::BoundedProcessLimits;

    /// Current wire format for a publisher execution receipt.
    pub const PUBLISHER_EXECUTION_RECEIPT_SCHEMA_VERSION: u32 = 1;
    /// Asset-relative file carrying the producer receipt beside publisher products.
    pub const PUBLISHER_EXECUTION_RECEIPT_FILE: &str = "publisher-receipt.json";

    /// One portable argument in the exact order passed to publisher work.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PublisherExecutionArgument<'a> {
        /// Literal bytes from the declared work record.
        Literal(&'a str),
        /// Physical path of one declared input, selected by logical name.
        Input(&'a str),
        /// Physical path of one declared output, selected by logical name.
        Output(&'a str),
    }

    /// One exact regular-file input admitted for publisher execution.
    #[derive(Debug, Clone)]
    pub struct PublisherExecutionInput<'a> {
        /// Invocation-local logical name.
        pub name: &'a str,
        /// Already selected physical file.
        pub path: PathBuf,
        /// Expected `sha256:` byte identity.
        pub digest: String,
    }

    /// One exact regular-file output contract below the private product root.
    #[derive(Debug, Clone, Copy)]
    pub struct PublisherExecutionOutput<'a> {
        /// Invocation-local logical name.
        pub name: &'a str,
        /// Portable path below [`PublisherExecutionRequest::output_root`].
        pub relative_path: &'a str,
    }

    /// One declared environment value for publisher execution.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PublisherExecutionEnvironmentValue<'a> {
        /// Literal value from the work record.
        Literal(&'a str),
        /// Physical path of one declared input, selected by logical name.
        Input(&'a str),
    }

    /// Complete hermetic publisher invocation after manifest and asset selection.
    pub struct PublisherExecutionRequest<'a> {
        /// Stable work class, such as `link` or `tool`.
        pub role: &'a str,
        /// Stable producer name.
        pub name: &'a str,
        /// Source-selected consumer identity this product is being produced for.
        pub consuming_unit_identity: &'a str,
        /// Exact target association.
        pub target: &'a str,
        /// Exact selected toolchain identity.
        pub toolchain: &'a str,
        /// Already selected physical executable.
        pub executable: &'a Path,
        /// Immutable owner identity of the executable.
        pub executable_owner: &'a str,
        /// Expected executable byte identity.
        pub executable_digest: &'a str,
        /// Ordered declared arguments.
        pub arguments: Vec<PublisherExecutionArgument<'a>>,
        /// Complete declared environment. The child inherits no ambient values.
        pub environment: BTreeMap<&'a str, PublisherExecutionEnvironmentValue<'a>>,
        /// Complete declared regular-file inputs.
        pub inputs: Vec<PublisherExecutionInput<'a>>,
        /// Complete declared regular-file outputs.
        pub outputs: Vec<PublisherExecutionOutput<'a>>,
        /// Fresh private directory in which outputs must appear.
        pub output_root: &'a Path,
        /// Physical execution bounds.
        pub limits: BoundedProcessLimits,
    }

    /// Shared verified publisher product shape; link products are file-valued instances with an empty member list.
    pub type PublisherExecutionProduct = super::OvenPublisherToolProduct;

    /// Receipt sealing one publisher-only execution and its complete product set.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct PublisherExecutionReceipt {
        /// Receipt wire-format version.
        pub schema_version: u32,
        /// Content identity over every other field.
        pub identity: String,
        /// Work class (`link` or `tool`).
        pub role: String,
        /// Stable producer name.
        pub name: String,
        /// Source-selected consumer identity.
        pub consuming_unit_identity: String,
        /// Exact target association.
        pub target: String,
        /// Exact toolchain identity.
        pub toolchain: String,
        /// Immutable executable owner identity.
        pub executable_owner: String,
        /// Executable byte identity.
        pub executable_digest: String,
        /// Relocation-independent ordered argument projection.
        pub logical_argv: Vec<String>,
        /// Relocation-independent declared environment projection.
        pub logical_environment: BTreeMap<String, String>,
        /// Logical input names and exact byte identities.
        pub inputs: BTreeMap<String, String>,
        /// Complete verified product set.
        pub outputs: Vec<PublisherExecutionProduct>,
    }

    impl PublisherExecutionReceipt {
        /// Recompute and verify this receipt before its products are trusted by a consumer.
        pub fn verify_identity(&self) -> Result<(), PublisherExecutionError> {
            if self.schema_version != PUBLISHER_EXECUTION_RECEIPT_SCHEMA_VERSION {
                return Err(PublisherExecutionError::InvalidReceipt(
                    "unsupported schema version".to_string(),
                ));
            }
            let actual = publisher_execution_receipt_identity(self)?;
            if actual != self.identity {
                return Err(PublisherExecutionError::InvalidReceipt(format!(
                    "identity mismatch: expected {}, got {actual}",
                    self.identity
                )));
            }
            Ok(())
        }
    }

    /// Successful publisher execution paired with the receipt that admits its products.
    #[derive(Debug)]
    pub struct PublisherExecutionResult {
        /// Verified products in declared order.
        pub outputs: Vec<PublisherExecutionProduct>,
        /// Identity-bearing execution receipt.
        pub receipt: PublisherExecutionReceipt,
    }

    /// Why publisher work could not produce an admitted product.
    #[derive(Debug, thiserror::Error)]
    pub enum PublisherExecutionError {
        /// A request field, input, output, or receipt violated the closed contract.
        #[error("invalid publisher execution: {0}")]
        Invalid(String),
        /// The child process failed or exceeded a physical bound.
        #[error("publisher execution failed: {0}")]
        Execution(String),
        /// A persisted receipt was malformed or tampered.
        #[error("invalid publisher execution receipt: {0}")]
        InvalidReceipt(String),
        /// Filesystem I/O failed at a named path.
        #[error("publisher execution I/O failed at {path}: {source}")]
        Io {
            /// Path being accessed.
            path: PathBuf,
            /// Underlying I/O error.
            #[source]
            source: std::io::Error,
        },
    }

    /// Execute one exact publisher work record with no ambient environment or output discovery.
    pub fn execute_publisher_work(
        request: &PublisherExecutionRequest<'_>,
    ) -> Result<PublisherExecutionResult, PublisherExecutionError> {
        validate_name(request.role, "role")?;
        validate_name(request.name, "name")?;
        if request.consuming_unit_identity.is_empty() || request.target.is_empty() || request.toolchain.is_empty() {
            return Err(PublisherExecutionError::Invalid(
                "consumer identity, target and toolchain must be non-empty".to_string(),
            ));
        }
        let executable = verified_file(request.executable, request.executable_digest, "executable")?;
        let inputs = verified_inputs(&request.inputs)?;
        prepare_output_root(request.output_root)?;
        // Outputs are named to the child by the root's resolved spelling, which is the only one confinement admits.
        let output_root = fs::canonicalize(request.output_root).map_err(|source| PublisherExecutionError::Io {
            path: request.output_root.to_path_buf(),
            source,
        })?;
        let outputs = declared_outputs(&output_root, &request.outputs)?;

        let (arguments, logical_argv) = materialize_arguments(&request.arguments, &inputs, &outputs)?;
        let (environment, logical_environment) = materialize_environment(&request.environment, &inputs)?;
        let input_paths = inputs.values().cloned().collect::<Vec<_>>();
        super::run_hermetic_process(
            &executable,
            &arguments,
            &environment,
            &input_paths,
            &output_root,
            request.limits,
        )
        .map_err(PublisherExecutionError::Execution)?;

        let products = verify_complete_outputs(&output_root, &request.outputs)?;
        let mut receipt = PublisherExecutionReceipt {
            schema_version: PUBLISHER_EXECUTION_RECEIPT_SCHEMA_VERSION,
            identity: String::new(),
            role: request.role.to_string(),
            name: request.name.to_string(),
            consuming_unit_identity: request.consuming_unit_identity.to_string(),
            target: request.target.to_string(),
            toolchain: request.toolchain.to_string(),
            executable_owner: request.executable_owner.to_string(),
            executable_digest: request.executable_digest.to_string(),
            logical_argv,
            logical_environment,
            inputs: request
                .inputs
                .iter()
                .map(|input| (input.name.to_string(), input.digest.clone()))
                .collect(),
            outputs: products.clone(),
        };
        receipt.identity = publisher_execution_receipt_identity(&receipt)?;
        Ok(PublisherExecutionResult {
            outputs: products,
            receipt,
        })
    }

    /// Compute the portable identity of a publisher receipt, excluding its identity field.
    pub fn publisher_execution_receipt_identity(
        receipt: &PublisherExecutionReceipt,
    ) -> Result<String, PublisherExecutionError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema_version: u32,
            role: &'a str,
            name: &'a str,
            consuming_unit_identity: &'a str,
            target: &'a str,
            toolchain: &'a str,
            executable_owner: &'a str,
            executable_digest: &'a str,
            logical_argv: &'a [String],
            logical_environment: &'a BTreeMap<String, String>,
            inputs: &'a BTreeMap<String, String>,
            outputs: &'a [PublisherExecutionProduct],
        }
        let bytes = serde_json::to_vec(&Identity {
            schema_version: receipt.schema_version,
            role: &receipt.role,
            name: &receipt.name,
            consuming_unit_identity: &receipt.consuming_unit_identity,
            target: &receipt.target,
            toolchain: &receipt.toolchain,
            executable_owner: &receipt.executable_owner,
            executable_digest: &receipt.executable_digest,
            logical_argv: &receipt.logical_argv,
            logical_environment: &receipt.logical_environment,
            inputs: &receipt.inputs,
            outputs: &receipt.outputs,
        })
        .map_err(|error| PublisherExecutionError::InvalidReceipt(error.to_string()))?;
        Ok(digest_bytes(&bytes))
    }

    /// Persist a verified producer receipt beside its asset products through same-directory atomic replacement.
    pub fn write_publisher_execution_receipt(
        receipt: &PublisherExecutionReceipt,
        product_root: &Path,
    ) -> Result<PathBuf, PublisherExecutionError> {
        receipt.verify_identity()?;
        let path = product_root.join(PUBLISHER_EXECUTION_RECEIPT_FILE);
        let staged = product_root.join(format!(".{PUBLISHER_EXECUTION_RECEIPT_FILE}.staged"));
        let bytes = serde_json::to_vec_pretty(receipt)
            .map_err(|error| PublisherExecutionError::InvalidReceipt(error.to_string()))?;
        if path.exists() {
            let existing = fs::read(&path).map_err(|source| PublisherExecutionError::Io {
                path: path.clone(),
                source,
            })?;
            if existing == bytes {
                return Ok(path);
            }
            return Err(PublisherExecutionError::Invalid(
                "publisher receipt path collides with different bytes".to_string(),
            ));
        }
        let mut staged_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .map_err(|source| PublisherExecutionError::Io {
                path: staged.clone(),
                source,
            })?;
        staged_file
            .write_all(&bytes)
            .and_then(|()| staged_file.sync_all())
            .map_err(|source| PublisherExecutionError::Io {
                path: staged.clone(),
                source,
            })?;
        fs::rename(&staged, &path).map_err(|source| PublisherExecutionError::Io {
            path: path.clone(),
            source,
        })?;
        fs::File::open(product_root)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| PublisherExecutionError::Io {
                path: product_root.to_path_buf(),
                source,
            })?;
        Ok(path)
    }

    /// Admit a conservative portable logical name.
    fn validate_name(value: &str, field: &str) -> Result<(), PublisherExecutionError> {
        if value.is_empty()
            || !value
                .bytes()
                .all(|byte| byte == b'-' || byte == b'_' || byte.is_ascii_alphanumeric())
        {
            return Err(PublisherExecutionError::Invalid(format!(
                "{field} `{value}` is not a portable name"
            )));
        }
        Ok(())
    }

    /// Verify one non-symlink regular file against its exact bytes.
    fn verified_file(path: &Path, expected: &str, field: &str) -> Result<PathBuf, PublisherExecutionError> {
        let metadata = fs::symlink_metadata(path).map_err(|source| PublisherExecutionError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(PublisherExecutionError::Invalid(format!(
                "{field} {} must be a regular non-symlink file",
                path.display()
            )));
        }
        let bytes = fs::read(path).map_err(|source| PublisherExecutionError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let actual = digest_bytes(&bytes);
        if actual != expected {
            return Err(PublisherExecutionError::Invalid(format!(
                "{field} {} digest mismatch: expected {expected}, got {actual}",
                path.display()
            )));
        }
        fs::canonicalize(path).map_err(|source| PublisherExecutionError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Verify the complete uniquely named regular-file input map.
    fn verified_inputs(
        declared: &[PublisherExecutionInput<'_>],
    ) -> Result<BTreeMap<String, PathBuf>, PublisherExecutionError> {
        let mut inputs = BTreeMap::new();
        for input in declared {
            validate_name(input.name, "input name")?;
            let path = verified_file(&input.path, &input.digest, "input")?;
            if inputs.insert(input.name.to_string(), path).is_some() {
                return Err(PublisherExecutionError::Invalid(format!(
                    "input `{}` is declared twice",
                    input.name
                )));
            }
        }
        Ok(inputs)
    }

    /// Resolve and validate uniquely named output paths without creating them.
    fn declared_outputs(
        root: &Path,
        declared: &[PublisherExecutionOutput<'_>],
    ) -> Result<BTreeMap<String, PathBuf>, PublisherExecutionError> {
        let mut outputs = BTreeMap::new();
        let mut paths = BTreeSet::new();
        for output in declared {
            validate_name(output.name, "output name")?;
            let relative = Path::new(output.relative_path);
            if relative.as_os_str().is_empty()
                || relative.is_absolute()
                || relative
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
            {
                return Err(PublisherExecutionError::Invalid(format!(
                    "output `{}` has non-portable path `{}`",
                    output.name, output.relative_path
                )));
            }
            if !paths.insert(output.relative_path) {
                return Err(PublisherExecutionError::Invalid(format!(
                    "output path `{}` is declared twice",
                    output.relative_path
                )));
            }
            if outputs.insert(output.name.to_string(), root.join(relative)).is_some() {
                return Err(PublisherExecutionError::Invalid(format!(
                    "output `{}` is declared twice",
                    output.name
                )));
            }
        }
        Ok(outputs)
    }

    /// Create a fresh private output root and refuse any collision.
    fn prepare_output_root(root: &Path) -> Result<(), PublisherExecutionError> {
        if root.exists() {
            let metadata = fs::symlink_metadata(root).map_err(|source| PublisherExecutionError::Io {
                path: root.to_path_buf(),
                source,
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(PublisherExecutionError::Invalid(format!(
                    "publisher output root {} must be a regular non-symlink directory",
                    root.display()
                )));
            }
            let mut entries = fs::read_dir(root).map_err(|source| PublisherExecutionError::Io {
                path: root.to_path_buf(),
                source,
            })?;
            if entries
                .next()
                .transpose()
                .map_err(|source| PublisherExecutionError::Io {
                    path: root.to_path_buf(),
                    source,
                })?
                .is_some()
            {
                return Err(PublisherExecutionError::Invalid(format!(
                    "publisher output root {} collides with existing content",
                    root.display()
                )));
            }
        } else {
            fs::create_dir_all(root).map_err(|source| PublisherExecutionError::Io {
                path: root.to_path_buf(),
                source,
            })?;
        }
        Ok(())
    }

    /// Materialize portable arguments while retaining their exact logical projection.
    fn materialize_arguments(
        arguments: &[PublisherExecutionArgument<'_>],
        inputs: &BTreeMap<String, PathBuf>,
        outputs: &BTreeMap<String, PathBuf>,
    ) -> Result<(Vec<std::ffi::OsString>, Vec<String>), PublisherExecutionError> {
        let mut physical = Vec::with_capacity(arguments.len());
        let mut logical = Vec::with_capacity(arguments.len());
        for argument in arguments {
            match argument {
                PublisherExecutionArgument::Literal(value) => {
                    physical.push((*value).into());
                    logical.push(format!("literal:{value}"));
                }
                PublisherExecutionArgument::Input(name) => {
                    let path = inputs.get(*name).ok_or_else(|| {
                        PublisherExecutionError::Invalid(format!("argument names undeclared input `{name}`"))
                    })?;
                    physical.push(path.as_os_str().to_owned());
                    logical.push(format!("input:{name}"));
                }
                PublisherExecutionArgument::Output(name) => {
                    let path = outputs.get(*name).ok_or_else(|| {
                        PublisherExecutionError::Invalid(format!("argument names undeclared output `{name}`"))
                    })?;
                    physical.push(path.as_os_str().to_owned());
                    logical.push(format!("output:{name}"));
                }
            }
        }
        Ok((physical, logical))
    }

    /// Materialize the complete environment from literals and admitted input paths.
    fn materialize_environment(
        environment: &BTreeMap<&str, PublisherExecutionEnvironmentValue<'_>>,
        inputs: &BTreeMap<String, PathBuf>,
    ) -> Result<(BTreeMap<String, std::ffi::OsString>, BTreeMap<String, String>), PublisherExecutionError> {
        let mut physical = BTreeMap::new();
        let mut logical = BTreeMap::new();
        for (name, value) in environment {
            if name.is_empty() || name.contains('=') || name.contains('\0') {
                return Err(PublisherExecutionError::Invalid(format!(
                    "environment name `{name}` is invalid"
                )));
            }
            match value {
                PublisherExecutionEnvironmentValue::Literal(value) => {
                    physical.insert((*name).to_string(), (*value).into());
                    logical.insert((*name).to_string(), format!("literal:{value}"));
                }
                PublisherExecutionEnvironmentValue::Input(input) => {
                    let path = inputs.get(*input).ok_or_else(|| {
                        PublisherExecutionError::Invalid(format!(
                            "environment `{name}` names undeclared input `{input}`"
                        ))
                    })?;
                    physical.insert((*name).to_string(), path.as_os_str().to_owned());
                    logical.insert((*name).to_string(), format!("input:{input}"));
                }
            }
        }
        Ok((physical, logical))
    }

    /// Verify that the product root contains exactly the declared regular files.
    fn verify_complete_outputs(
        root: &Path,
        declared: &[PublisherExecutionOutput<'_>],
    ) -> Result<Vec<PublisherExecutionProduct>, PublisherExecutionError> {
        let expected = declared
            .iter()
            .map(|output| output.relative_path.to_string())
            .collect::<BTreeSet<_>>();
        let mut actual = BTreeSet::new();
        collect_output_files(root, root, &mut actual)?;
        if let Some(missing) = expected.difference(&actual).next() {
            return Err(PublisherExecutionError::Invalid(format!(
                "declared output `{missing}` is missing"
            )));
        }
        if let Some(extra) = actual.difference(&expected).next() {
            return Err(PublisherExecutionError::Invalid(format!(
                "undeclared output `{extra}` was produced"
            )));
        }
        declared
            .iter()
            .map(|output| {
                let path = root.join(output.relative_path);
                let bytes = fs::read(&path).map_err(|source| PublisherExecutionError::Io {
                    path: path.clone(),
                    source,
                })?;
                Ok(PublisherExecutionProduct {
                    name: output.name.to_string(),
                    kind: oven_model::manifest::RustFactArtifactKind::File,
                    path: output.relative_path.to_string(),
                    digest: digest_bytes(&bytes),
                    members: Vec::new(),
                })
            })
            .collect()
    }

    /// Walk publisher products without following symlinks or accepting empty undeclared directories.
    fn collect_output_files(
        root: &Path,
        current: &Path,
        files: &mut BTreeSet<String>,
    ) -> Result<(), PublisherExecutionError> {
        for entry in fs::read_dir(current).map_err(|source| PublisherExecutionError::Io {
            path: current.to_path_buf(),
            source,
        })? {
            let entry = entry.map_err(|source| PublisherExecutionError::Io {
                path: current.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| PublisherExecutionError::Io {
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() {
                return Err(PublisherExecutionError::Invalid(format!(
                    "publisher output {} is symlink-substituted",
                    path.display()
                )));
            }
            if metadata.is_dir() {
                collect_output_files(root, &path, files)?;
            } else if metadata.is_file() {
                let relative = path.strip_prefix(root).map_err(|_| {
                    PublisherExecutionError::Invalid("publisher output escaped its product root".to_string())
                })?;
                files.insert(relative.to_string_lossy().replace('\\', "/"));
            } else {
                return Err(PublisherExecutionError::Invalid(format!(
                    "publisher output {} is not a regular file or directory",
                    path.display()
                )));
            }
        }
        Ok(())
    }

    #[cfg(all(test, unix))]
    mod tests {
        use std::collections::BTreeMap;
        use std::error::Error;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::time::Duration;

        use tempfile::tempdir;

        use super::{
            PublisherExecutionArgument, PublisherExecutionError, PublisherExecutionInput, PublisherExecutionOutput,
            PublisherExecutionRequest, execute_publisher_work,
        };
        use crate::digest_bytes;
        use crate::process::BoundedProcessLimits;

        /// Copy the first argument's single-line contents to the second using shell builtins only, because the
        /// confinement admits no undeclared executable such as `/bin/cp`.
        const COPY_INPUT_TO_OUTPUT: &str = "IFS= read -r content < \"$1\" || true\nprintf '%s' \"$content\" > \"$2\"";

        /// Write one executable fake publisher tool and return its exact byte identity.
        fn executable(path: &std::path::Path, body: &str) -> Result<String, Box<dyn Error>> {
            fs::write(path, format!("#!/bin/sh\nset -eu\n{body}\n"))?;
            let mut permissions = fs::metadata(path)?.permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions)?;
            Ok(digest_bytes(&fs::read(path)?))
        }

        /// Return whether the managed host refused a nested Seatbelt profile before the fixture could run.
        fn confinement_was_denied(error: &PublisherExecutionError) -> bool {
            matches!(error, PublisherExecutionError::Execution(message) if message.contains("sandbox_apply: Operation not permitted"))
        }

        /// Build the common one-input, one-output fake compiler request.
        fn request<'a>(
            executable: &'a std::path::Path,
            executable_digest: &'a str,
            source: &'a std::path::Path,
            output_root: &'a std::path::Path,
        ) -> PublisherExecutionRequest<'a> {
            PublisherExecutionRequest {
                role: "link",
                name: "fixture-native",
                consuming_unit_identity: "sha256:consumer",
                target: "aarch64-apple-darwin",
                toolchain: "rustc fixture",
                executable,
                executable_owner: "sha256:compiler",
                executable_digest,
                arguments: vec![
                    PublisherExecutionArgument::Input("source"),
                    PublisherExecutionArgument::Literal("libfixture.a"),
                ],
                environment: BTreeMap::new(),
                inputs: vec![PublisherExecutionInput {
                    name: "source",
                    path: source.to_path_buf(),
                    digest: digest_bytes(b"source"),
                }],
                outputs: vec![PublisherExecutionOutput {
                    name: "archive",
                    relative_path: "libfixture.a",
                }],
                output_root,
                limits: BoundedProcessLimits {
                    stdout_bytes: 1024,
                    stderr_bytes: 1024,
                    timeout: Some(Duration::from_secs(2)),
                },
            }
        }

        #[test]
        /// A successful publisher run retains exact argv and complete product evidence.
        fn publisher_execution_receipts_exact_argv_and_output() -> Result<(), Box<dyn Error>> {
            let root = tempdir()?;
            let compiler = root.path().join("fake-compiler");
            let source = root.path().join("source.c");
            let products = root.path().join("products");
            fs::write(&source, b"source")?;
            let compiler_digest = executable(&compiler, COPY_INPUT_TO_OUTPUT)?;

            let product = match execute_publisher_work(&request(&compiler, &compiler_digest, &source, &products)) {
                Ok(product) => product,
                Err(error) if confinement_was_denied(&error) => return Ok(()),
                Err(error) => return Err(error.into()),
            };

            assert_eq!(product.outputs[0].digest, digest_bytes(b"source"));
            assert_eq!(product.receipt.logical_argv, ["input:source", "literal:libfixture.a"]);
            product.receipt.verify_identity()?;
            Ok(())
        }

        #[test]
        /// Missing and undeclared products both fail the closed output contract.
        fn publisher_execution_refuses_missing_and_extra_outputs() -> Result<(), Box<dyn Error>> {
            let root = tempdir()?;
            let source = root.path().join("source.c");
            fs::write(&source, b"source")?;

            let missing = root.path().join("missing-compiler");
            let missing_digest = executable(&missing, ":")?;
            let error = execute_publisher_work(&request(
                &missing,
                &missing_digest,
                &source,
                &root.path().join("missing-products"),
            ))
            .err()
            .ok_or("missing output was accepted")?;
            if confinement_was_denied(&error) {
                return Ok(());
            }
            assert!(error.to_string().contains("missing"));

            let extra = root.path().join("extra-compiler");
            let extra_digest = executable(
                &extra,
                &format!("{COPY_INPUT_TO_OUTPUT}\nprintf '%s' \"$content\" > extra.a"),
            )?;
            let error = execute_publisher_work(&request(
                &extra,
                &extra_digest,
                &source,
                &root.path().join("extra-products"),
            ))
            .err()
            .ok_or("extra output was accepted")?;
            assert!(error.to_string().contains("undeclared"));
            Ok(())
        }

        #[test]
        /// Toolchain identity participates in receipts, whose immutable fields are reverified before use.
        fn publisher_receipt_observes_toolchain_identity_and_tampering() -> Result<(), Box<dyn Error>> {
            let root = tempdir()?;
            let compiler = root.path().join("fake-compiler");
            let source = root.path().join("source.c");
            fs::write(&source, b"source")?;
            let compiler_digest = executable(&compiler, COPY_INPUT_TO_OUTPUT)?;
            let first = match execute_publisher_work(&request(
                &compiler,
                &compiler_digest,
                &source,
                &root.path().join("first"),
            )) {
                Ok(product) => product,
                Err(error) if confinement_was_denied(&error) => return Ok(()),
                Err(error) => return Err(error.into()),
            };
            let second_products = root.path().join("second");
            let mut changed_request = request(&compiler, &compiler_digest, &source, &second_products);
            changed_request.toolchain = "rustc changed";
            let changed = execute_publisher_work(&changed_request)?;
            assert_ne!(first.receipt.identity, changed.receipt.identity);

            let mut tampered = first.receipt;
            tampered.target = "substituted-target".to_string();
            assert!(tampered.verify_identity().is_err());
            Ok(())
        }
    }
}

pub use link_execution::*;
