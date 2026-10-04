//! Candidate identity, Cargo evidence admission, and output-only transport for Linux bootstrap tools.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const TOOLS: [&str; 3] = ["incan", "generate_lang_reference", "generate_feature_inventory"];
/// Path of this executable as the CI workflow invokes it.
const RECORDER: &str = "target/debug/incan-ci-tool-outputs";
const ROOT_INPUTS: [&str; 4] = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "rust-toolchain"];
const RECIPE: [&str; 2] = [
    "CARGO_BUILD_JOBS=2 cargo build --locked --release -p incan-cli --bin incan --bin generate_feature_inventory --message-format=json-render-diagnostics",
    "CARGO_BUILD_JOBS=2 cargo build --locked --release -p incan_lang --bin generate_lang_reference --message-format=json-render-diagnostics",
];
const BUILD_ENVIRONMENT: [&str; 30] = [
    "AR",
    "CARGO_BUILD_JOBS",
    "CARGO_BUILD_RUSTFLAGS",
    "CARGO_BUILD_TARGET",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_HOME",
    "CARGO_INCREMENTAL",
    "CARGO_PROFILE_RELEASE_CODEGEN_UNITS",
    "CARGO_PROFILE_RELEASE_DEBUG",
    "CARGO_PROFILE_RELEASE_LTO",
    "CARGO_PROFILE_RELEASE_OPT_LEVEL",
    "CARGO_PROFILE_RELEASE_PANIC",
    "CARGO_PROFILE_RELEASE_STRIP",
    "CARGO_TARGET_DIR",
    "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER",
    "CC",
    "CFLAGS",
    "CPATH",
    "CXX",
    "CXXFLAGS",
    "LD_LIBRARY_PATH",
    "LDFLAGS",
    "LIBRARY_PATH",
    "PATH",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTC_WRAPPER",
    "RUSTDOCFLAGS",
    "RUSTFLAGS",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
];

#[derive(Debug, Parser)]
#[command(about = "Describe Linux bootstrap tool inputs and verify output-only transport")]
struct Cli {
    #[command(subcommand)]
    operation: Operation,
}

#[derive(Debug, Subcommand)]
enum Operation {
    /// Derive the candidate input identity and cache key.
    Identity(Common),
    /// Verify and restore a previously admitted output bundle.
    Restore(Common),
    /// Admit cold-build evidence and publish an output bundle.
    Admit(EvidenceArgs),
    /// Retain cold-build facts when candidate identity was ineligible.
    Collect(EvidenceArgs),
    /// Retain raw Cargo JSON while forwarding rendered compiler diagnostics.
    Record { destination: PathBuf },
}

#[derive(clap::Args, Debug)]
struct Common {
    #[arg(long, default_value = ".")]
    workspace: PathBuf,
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    bundle: PathBuf,
}

#[derive(clap::Args, Debug)]
struct EvidenceArgs {
    #[command(flatten)]
    common: Common,
    #[arg(long = "cargo-log", required = true)]
    cargo_logs: Vec<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Candidate {
    identity: String,
    inputs: CandidateInputs,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct CandidateInputs {
    schema_version: u8,
    workspace: String,
    coordinates: Value,
    files: Vec<InputRecord>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct InputRecord {
    path: String,
    mode: u32,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct OutputManifest {
    schema_version: u8,
    input_identity: String,
    outputs: BTreeMap<String, String>,
}

/// How one operation ended once its CI outputs were published.
#[derive(Debug)]
pub enum Completion {
    /// The operation produced its evidence.
    Done,
    /// The operation could not vouch for the tool outputs. Its fail-closed outputs are published, so CI continues on
    /// the cold compiler build: these outputs are an accelerator, never a gate.
    Unavailable(io::Error),
}

/// Parse the process arguments and run one operation, publishing fail-closed CI outputs when it is unavailable.
///
/// An error means the fail-closed outputs themselves could not be published.
pub fn run_from_environment() -> io::Result<Completion> {
    let cli = Cli::parse();
    let unavailable = unavailable_context(&cli.operation);
    let github_output = env::var_os("GITHUB_OUTPUT").map(PathBuf::from);
    settle(run(cli.operation), unavailable, github_output.as_deref())
}

/// Turn an operation result into its completion, publishing fail-closed outputs and evidence for a refusal.
fn settle(
    result: io::Result<()>,
    unavailable: Option<(PathBuf, &'static str)>,
    github_output: Option<&Path>,
) -> io::Result<Completion> {
    let Err(error) = result else {
        return Ok(Completion::Done);
    };
    if let Some(github_output) = github_output {
        append_github_outputs(
            github_output,
            &BTreeMap::from([("eligible", "false"), ("hit", "false"), ("admitted", "false")]),
        )?;
    }
    if let Some((state, operation)) = unavailable {
        fs::create_dir_all(&state)?;
        write_json(
            &state.join(format!("{operation}-unavailable.json")),
            &json!({"status": "unavailable", "detail": error.to_string()}),
        )?;
    }
    Ok(Completion::Unavailable(error))
}

/// Return the state path and operation name used for unavailable evidence.
fn unavailable_context(operation: &Operation) -> Option<(PathBuf, &'static str)> {
    match operation {
        Operation::Identity(args) => Some((args.state.clone(), "identity")),
        Operation::Restore(args) => Some((args.state.clone(), "restore")),
        Operation::Admit(args) => Some((args.common.state.clone(), "admit")),
        Operation::Collect(args) => Some((args.common.state.clone(), "collect")),
        Operation::Record { .. } => None,
    }
}

/// Execute the selected operation.
fn run(operation: Operation) -> io::Result<()> {
    if let Operation::Record { destination } = operation {
        return record_cargo_messages(&destination, io::stdin().lock(), io::stdout().lock());
    }
    let started = Instant::now();
    match operation {
        Operation::Identity(args) => identity(args, started),
        Operation::Restore(args) => restore(args, started),
        Operation::Admit(args) => evidence_operation(args, true, started),
        Operation::Collect(args) => evidence_operation(args, false, started),
        Operation::Record { .. } => unreachable!("record returned above"),
    }
}

/// Derive and publish the candidate identity and cache key.
fn identity(args: Common, started: Instant) -> io::Result<()> {
    let workspace = fs::canonicalize(&args.workspace)?;
    fs::create_dir_all(&args.state)?;
    verify_workflow_recipe(&workspace)?;
    refuse_ambient_cargo_config(&workspace)?;
    let runner_os = env::var("ImageOS").map_err(io::Error::other)?;
    let runner_version = env::var("ImageVersion").map_err(io::Error::other)?;
    let coordinates = json!({
        "recipe": RECIPE,
        "rustc": command_text(&workspace, "rustc", &["-vV"] )?,
        "cargo": command_text(&workspace, "cargo", &["-vV"] )?,
        "linker": format!("{}\n{}", command_text(&workspace, "cc", &["--version"] )?, command_text(&workspace, "ld", &["--version"] )?),
        "runner_image": [runner_os, runner_version],
        "environment": build_environment(env::vars())?,
    });
    let candidate = input_manifest(&workspace, coordinates)?;
    write_json(&args.state.join("inputs.json"), &candidate)?;
    publish_github_outputs(&BTreeMap::from([
        ("key", format!("incan-linux-tools-v1-{}", candidate.identity)),
        ("eligible", "true".to_owned()),
    ]))?;
    finish_report(&args.state, "identity", "candidate", started, 0)
}

/// Verify a warm bundle before copying its tools to the release delivery paths.
fn restore(args: Common, started: Instant) -> io::Result<()> {
    let workspace = fs::canonicalize(&args.workspace)?;
    fs::create_dir_all(&args.state)?;
    let candidate: Candidate = read_json(&args.state.join("inputs.json"))?;
    verify_candidate_current(&workspace, &candidate)?;
    let result = verify_bundle(&args.bundle, &candidate).and_then(|()| {
        let release = workspace.join("target/release");
        fs::create_dir_all(&release)?;
        for tool in TOOLS {
            copy_regular_file(&args.bundle.join(tool), &release.join(tool))?;
        }
        Ok(())
    });
    match result {
        Ok(()) => {
            publish_github_outputs(&BTreeMap::from([("hit", "true")]))?;
            finish_report(&args.state, "restore", "hit", started, 0)
        }
        Err(error) => {
            if args.bundle.exists() || fs::symlink_metadata(&args.bundle).is_ok() {
                let rejected = args.bundle.with_file_name(format!(
                    "{}-rejected",
                    args.bundle
                        .file_name()
                        .unwrap_or_else(|| OsStr::new("bundle"))
                        .to_string_lossy()
                ));
                if rejected.exists() || fs::symlink_metadata(&rejected).is_ok() {
                    return Err(io::Error::other("previous rejected tool bundle already exists"));
                }
                fs::rename(&args.bundle, rejected)?;
            }
            publish_github_outputs(&BTreeMap::from([("hit", "false")]))?;
            write_json(
                &args.state.join("restore.json"),
                &json!({"operation": "restore", "status": "miss", "detail": error.to_string(), "elapsed_seconds": started.elapsed().as_secs_f64()}),
            )?;
            println!("{}", json!({"operation": "restore", "status": "miss"}));
            Ok(())
        }
    }
}

/// Collect cold evidence, optionally admitting and publishing it against an eligible candidate.
fn evidence_operation(args: EvidenceArgs, admit: bool, started: Instant) -> io::Result<()> {
    if args.cargo_logs.len() != 2 {
        return Err(io::Error::other("two completed Cargo JSON logs are required"));
    }
    let workspace = fs::canonicalize(&args.common.workspace)?;
    fs::create_dir_all(&args.common.state)?;
    let candidate = if admit {
        let candidate: Candidate = read_json(&args.common.state.join("inputs.json"))?;
        verify_candidate_current(&workspace, &candidate)?;
        Some(candidate)
    } else {
        None
    };
    let evidence = collect_evidence(&workspace, candidate.as_ref(), &args.cargo_logs)?;
    write_json(&args.common.state.join("coverage.json"), &evidence)?;
    let admitted = admit && evidence["admitted"] == true;
    publish_github_outputs(&BTreeMap::from([("admitted", if admitted { "true" } else { "false" })]))?;
    if admitted {
        let candidate = candidate
            .as_ref()
            .ok_or_else(|| io::Error::other("admission lost its candidate"))?;
        if args.common.bundle.exists() {
            return Err(io::Error::other(
                "refusing to overwrite a restored bundle; use a separate publication directory",
            ));
        }
        fs::create_dir_all(&args.common.bundle)?;
        for tool in TOOLS {
            copy_regular_file(
                &workspace.join("target/release").join(tool),
                &args.common.bundle.join(tool),
            )?;
        }
        let tools = output_manifest(&args.common.bundle, &candidate.identity)?;
        write_json(
            &args.common.bundle.join("manifest.json"),
            &json!({"inputs": candidate, "evidence": evidence, "tools": tools}),
        )?;
    }
    finish_report(
        &args.common.state,
        if admit { "admit" } else { "collect" },
        if admitted {
            "admitted"
        } else if admit {
            "not-admitted"
        } else {
            "evidence-only"
        },
        started,
        evidence["refusals"].as_array().map_or(0, Vec::len),
    )
}

/// Bind tracked compiler inputs and explicit external build coordinates.
fn input_manifest(workspace: &Path, coordinates: Value) -> io::Result<Candidate> {
    validate_coordinates(&coordinates)?;
    let output = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(workspace)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("git ls-files failed"));
    }
    let tracked = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8(name.to_vec()).map_err(io::Error::other))
        .collect::<io::Result<BTreeSet<_>>>()?;
    for name in [".cargo/config", ".cargo/config.toml"] {
        if workspace.join(name).exists() && !tracked.contains(name) {
            return Err(io::Error::other(format!("untracked Cargo configuration: {name}")));
        }
    }
    let mut files = Vec::new();
    for name in tracked.into_iter().filter(|name| covered_path(name)) {
        let path = workspace.join(&name);
        require_unlinked_regular_file(&path)?;
        files.push(InputRecord {
            path: name,
            mode: file_mode(&path)?,
            sha256: file_digest(&path)?,
        });
    }
    if !ROOT_INPUTS[..2]
        .iter()
        .all(|required| files.iter().any(|record| record.path == *required))
    {
        return Err(io::Error::other("tracked Cargo manifest and lock are required"));
    }
    let inputs = CandidateInputs {
        schema_version: 1,
        workspace: workspace.display().to_string(),
        coordinates,
        files,
    };
    let identity = canonical_digest(&serde_json::to_value(&inputs).map_err(io::Error::other)?)?;
    Ok(Candidate { identity, inputs })
}

/// Validate the exact coordinate fields and value classes.
fn validate_coordinates(coordinates: &Value) -> io::Result<()> {
    let object = coordinates
        .as_object()
        .ok_or_else(|| io::Error::other("build coordinates must be an object"))?;
    let required = BTreeSet::from(["recipe", "rustc", "cargo", "linker", "runner_image", "environment"]);
    if object.keys().map(String::as_str).collect::<BTreeSet<_>>() != required
        || ["recipe", "rustc", "cargo", "linker", "runner_image"]
            .iter()
            .any(|key| object.get(*key).is_none_or(Value::is_null))
        || !object.get("environment").is_some_and(Value::is_object)
    {
        return Err(io::Error::other("incomplete or unexpected build coordinates"));
    }
    Ok(())
}

/// Select conservative compiler source and configuration, excluding test/workflow administration.
fn covered_path(name: &str) -> bool {
    ROOT_INPUTS.contains(&name)
        || ["src/", "crates/", "assets/", ".cargo/", "loaves/"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

/// Capture reviewed compilation coordinates while redacting acquisition credentials and refusing unknown settings.
fn build_environment(
    environment: impl IntoIterator<Item = (String, String)>,
) -> io::Result<BTreeMap<String, Option<String>>> {
    let environment = environment.into_iter().collect::<BTreeMap<_, _>>();
    let unsupported = environment
        .keys()
        .filter(|name| name.starts_with("RUST") || name.starts_with("CARGO"))
        .filter(|name| !BUILD_ENVIRONMENT.contains(&name.as_str()) && !acquisition_environment(name))
        .cloned()
        .collect::<Vec<_>>();
    if !unsupported.is_empty() {
        return Err(io::Error::other(format!(
            "unsupported build configuration names: {}",
            unsupported.join(", ")
        )));
    }
    Ok(BUILD_ENVIRONMENT
        .iter()
        .map(|name| ((*name).to_owned(), environment.get(*name).cloned()))
        .collect())
}

/// Identify acquisition-only variables without inspecting or retaining their values.
fn acquisition_environment(name: &str) -> bool {
    matches!(
        name,
        "CARGO_TERM_COLOR"
            | "CARGO_TERM_VERBOSE"
            | "CARGO_TERM_QUIET"
            | "CARGO_NET_OFFLINE"
            | "CARGO_NET_RETRY"
            | "CARGO_NET_GIT_FETCH_WITH_CLI"
            | "RUSTUP_DIST_SERVER"
            | "RUSTUP_UPDATE_ROOT"
            | "RUSTUP_MAX_RETRIES"
            | "RUST_BACKTRACE"
            | "RUST_LIB_BACKTRACE"
            | "RUST_LOG"
            | "RUSTUP_IO_THREADS"
            | "CARGO_REGISTRY_TOKEN"
    ) || name.starts_with("CARGO_HTTP_")
        || name.starts_with("CARGO_REGISTRIES_") && name.ends_with("_TOKEN")
}

/// Collect Cargo artifacts, depfile inputs, extension refusals, and output hashes.
fn collect_evidence(workspace: &Path, candidate: Option<&Candidate>, logs: &[PathBuf]) -> io::Result<Value> {
    let mut paths = BTreeSet::new();
    let mut artifacts = Vec::new();
    let mut refusals = if candidate.is_some() {
        Vec::new()
    } else {
        vec![json!({"kind": "identity-ineligible-evidence-only"})]
    };
    let mut finished = 0;
    let mut tools = BTreeSet::new();
    let mut environment_facts = BTreeMap::new();
    for log in logs {
        for line in BufReader::new(File::open(log)?).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let message: Value = serde_json::from_str(&line).map_err(io::Error::other)?;
            match message["reason"].as_str() {
                Some("build-finished") => {
                    if message["success"] != true {
                        return Err(io::Error::other("Cargo reported an unsuccessful build"));
                    }
                    finished += 1;
                }
                Some("build-script-executed") => refusals.push(json!({
                    "kind": "unreviewed-build-script",
                    "package_id": message.get("package_id"),
                    "out_dir": message.get("out_dir"),
                    "env_names": message["env"].as_array().map(|values| values.iter().filter_map(|pair| pair.as_array().and_then(|pair| pair.first()).and_then(Value::as_str)).collect::<Vec<_>>()).unwrap_or_default(),
                    "linked_path_count": message["linked_paths"].as_array().map_or(0, Vec::len),
                })),
                Some("compiler-artifact") => {
                    let target = &message["target"];
                    if target["kind"].as_array().is_some_and(|kinds| kinds.iter().any(|kind| kind == "proc-macro")) {
                        refusals.push(json!({"kind": "unreviewed-proc-macro", "package_id": message.get("package_id"), "source": target.get("src_path")}));
                    }
                    if let Some(name) = target["name"].as_str().filter(|name| TOOLS.contains(name)) {
                        let executable = message["executable"].as_str().ok_or_else(|| io::Error::other("tool artifact has no executable"))?;
                        if fs::canonicalize(executable)? != fs::canonicalize(workspace.join("target/release").join(name))? {
                            return Err(io::Error::other("Cargo tool output does not match the release delivery path"));
                        }
                        tools.insert(name.to_owned());
                    }
                    artifacts.push(message.clone());
                    match artifact_depfiles(&message) {
                        Ok(depfiles) => {
                            for depfile in depfiles {
                                match depfile_facts(&depfile) {
                                    Ok((inputs, observed)) => {
                                        paths.extend(inputs);
                                        for (name, value) in observed {
                                            if name.starts_with("CARGO_PKG_") || name == "CARGO_MANIFEST_DIR" {
                                                if let Err(error) = verify_cargo_environment(&message, &name, value.as_deref(), candidate) {
                                                    refusals.push(json!({"kind": "cargo-environment-projection-needs-audit", "name": name, "detail": error.to_string()}));
                                                }
                                            } else if !BUILD_ENVIRONMENT.contains(&name.as_str())
                                                || env::var(&name).ok().as_deref() != value.as_deref()
                                            {
                                                refusals.push(json!({"kind": "unaccounted-environment", "name": name}));
                                            } else {
                                                environment_facts.insert(name, value);
                                            }
                                        }
                                    }
                                    Err(error) => refusals.push(json!({"kind": "depfile-unavailable", "detail": error.to_string()})),
                                }
                            }
                        }
                        Err(error) => refusals.push(json!({"kind": "depfile-unavailable", "detail": error.to_string()})),
                    }
                }
                _ => {}
            }
        }
    }
    if finished != logs.len() || tools != TOOLS.into_iter().map(str::to_owned).collect() {
        return Err(io::Error::other(
            "Cargo evidence must finish both recipes and name all three tool outputs",
        ));
    }
    let mut records = Vec::new();
    for value in paths {
        let path = if Path::new(&value).is_absolute() {
            PathBuf::from(&value)
        } else {
            workspace.join(&value)
        };
        match candidate.and_then(|candidate| record_input(candidate, &path).ok()) {
            Some(record) => records.push(record),
            None if candidate.is_none()
                && path.starts_with(workspace)
                && require_unlinked_regular_file(&path).is_ok() =>
            {
                records.push(json!({"path": path, "sha256": file_digest(&path)?, "source": "unadmitted-observation"}));
            }
            _ => refusals.push(json!({"kind": "input-coverage-unavailable", "path": path})),
        }
    }
    if records.is_empty() {
        refusals.push(json!({"kind": "no-covered-local-inputs"}));
    }
    let outputs = TOOLS
        .into_iter()
        .map(|name| Ok((name, file_digest(&workspace.join("target/release").join(name))?)))
        .collect::<io::Result<BTreeMap<_, _>>>()?;
    Ok(
        json!({"admitted": refusals.is_empty(), "refusals": refusals, "files": records, "environment": environment_facts, "artifacts": artifacts, "observed_tool_outputs": outputs}),
    )
}

/// A depfile's sorted inputs and its `env-dep` variables with their recorded values, if any.
type DepfileFacts = (Vec<String>, BTreeMap<String, Option<String>>);

/// Parse Make-style rustc depfile inputs and env-dep facts without expanding variables.
fn depfile_facts(path: &Path) -> io::Result<DepfileFacts> {
    let text = fs::read_to_string(path)?.replace("\\\n", "");
    let mut inputs = BTreeSet::new();
    let mut environment = BTreeMap::new();
    for line in text.lines() {
        if let Some(fact) = line.strip_prefix("# env-dep:") {
            let (name, value) = fact
                .split_once('=')
                .map_or((fact, None), |(name, value)| (name, Some(value.to_owned())));
            if name.is_empty() || environment.insert(name.to_owned(), value).is_some() {
                return Err(io::Error::other(format!("ambiguous env-dep in {}", path.display())));
            }
        } else if !line.is_empty() && !line.starts_with('#') {
            if line.contains('$') || !line.contains(": ") {
                if line.ends_with(':') && !line.contains('$') {
                    continue;
                }
                return Err(io::Error::other(format!(
                    "unsupported depfile syntax in {}",
                    path.display()
                )));
            }
            let values = line.split_once(": ").map(|(_, values)| values).unwrap_or_default();
            inputs.extend(split_make_words(values)?);
        }
    }
    if inputs.is_empty() {
        return Err(io::Error::other(format!(
            "depfile contains no inputs: {}",
            path.display()
        )));
    }
    Ok((inputs.into_iter().collect(), environment))
}

/// Split the bounded depfile word syntax, supporting backslash-escaped spaces.
fn split_make_words(value: &str) -> io::Result<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if escaped {
        return Err(io::Error::other("depfile ends in an escape"));
    }
    if !current.is_empty() {
        words.push(current);
    }
    Ok(words)
}

/// Locate depfiles only beside Cargo-reported output filenames.
fn artifact_depfiles(message: &Value) -> io::Result<Vec<PathBuf>> {
    let mut result = BTreeSet::new();
    for name in message["filenames"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let output = PathBuf::from(name);
        if message["target"]["kind"]
            .as_array()
            .is_some_and(|kinds| kinds.len() == 1 && kinds[0] == "custom-build")
        {
            let package_id = message["package_id"].as_str().unwrap_or_default();
            let package = package_id
                .rsplit('#')
                .next()
                .unwrap_or_default()
                .split('@')
                .next()
                .unwrap_or_default();
            let parent = output
                .parent()
                .ok_or_else(|| io::Error::other("custom-build output has no owner"))?;
            let parent_name = parent.file_name().and_then(OsStr::to_str).unwrap_or_default();
            let fingerprint = parent_name
                .strip_prefix(&format!("{package}-"))
                .filter(|value| value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit()));
            if output.file_name() != Some(OsStr::new("build-script-build")) || fingerprint.is_none() {
                return Err(io::Error::other(
                    "custom-build output has no exact package/fingerprint owner",
                ));
            }
            let fingerprint = fingerprint.ok_or_else(|| io::Error::other("custom-build fingerprint is absent"))?;
            let candidate = normalize_owned_path(
                &output.with_file_name(format!("build_script_build-{fingerprint}.d")),
                parent,
            )?;
            let (sources, _) = depfile_facts(&candidate)?;
            let expected_source = message["target"]["src_path"].as_str().unwrap_or_default();
            let mut targets = BTreeSet::new();
            for line in fs::read_to_string(&candidate)?.replace("\\\n", "").lines() {
                if !line.starts_with('#')
                    && let Some((values, _)) = line.split_once(": ")
                {
                    targets.extend(split_make_words(values)?);
                }
            }
            if !sources.iter().any(|source| source == expected_source)
                || !targets.contains(&candidate.with_extension("").display().to_string())
            {
                return Err(io::Error::other(
                    "custom-build depfile does not bind its reported source and output",
                ));
            }
            result.insert(candidate);
            continue;
        }
        let mut stem = output
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_owned();
        if stem.starts_with("lib")
            && matches!(
                output.extension().and_then(OsStr::to_str),
                Some("rlib" | "rmeta" | "so" | "dylib" | "a")
            )
        {
            stem = stem[3..].to_owned();
        }
        let candidate = output.with_file_name(format!("{stem}.d"));
        if candidate.is_file() {
            result.insert(candidate);
        }
    }
    if let Some(executable) = message["executable"].as_str() {
        let candidate = Path::new(executable).with_extension("d");
        if candidate.is_file() {
            result.insert(candidate);
        }
    }
    if result.is_empty() {
        Err(io::Error::other(format!(
            "no concrete depfile for Cargo artifact {}",
            message["package_id"]
        )))
    } else {
        Ok(result.into_iter().collect())
    }
}

/// Verify a local consumed input against the candidate's exact bytes and mode.
fn local_record(candidate: &Candidate, path: &Path) -> io::Result<Value> {
    let workspace = Path::new(&candidate.inputs.workspace);
    let normalized = normalize_owned_path(path, workspace)?;
    let name = normalized
        .strip_prefix(workspace)
        .map_err(io::Error::other)?
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let expected = candidate
        .inputs
        .files
        .iter()
        .find(|record| record.path == name)
        .ok_or_else(|| io::Error::other(format!("uncovered consumed input: {}", path.display())))?;
    let sha256 = file_digest(&normalized)?;
    if expected.sha256 != sha256 || expected.mode != file_mode(&normalized)? {
        return Err(io::Error::other(format!(
            "consumed input bytes or mode changed: {}",
            path.display()
        )));
    }
    Ok(json!({"path": normalized, "sha256": sha256, "source": "workspace"}))
}

/// Verify common Cargo-injected constants against the reported package manifest.
fn verify_cargo_environment(
    message: &Value,
    name: &str,
    value: Option<&str>,
    candidate: Option<&Candidate>,
) -> io::Result<()> {
    let candidate =
        candidate.ok_or_else(|| io::Error::other("package environment is unadmitted without an eligible identity"))?;
    let manifest = Path::new(
        message["manifest_path"]
            .as_str()
            .ok_or_else(|| io::Error::other("Cargo artifact has no manifest"))?,
    );
    let parsed: toml::Value = toml::from_str(&fs::read_to_string(manifest)?).map_err(io::Error::other)?;
    let expected = match name {
        "CARGO_MANIFEST_DIR" => manifest
            .parent()
            .ok_or_else(|| io::Error::other("package manifest has no parent"))?
            .display()
            .to_string(),
        "CARGO_PKG_NAME" | "CARGO_PKG_VERSION" => {
            let field = if name.ends_with("NAME") { "name" } else { "version" };
            match &parsed["package"][field] {
                toml::Value::String(value) => value.clone(),
                toml::Value::Table(table) if table.get("workspace") == Some(&toml::Value::Boolean(true)) => {
                    let workspace: toml::Value = toml::from_str(&fs::read_to_string(
                        Path::new(&candidate.inputs.workspace).join("Cargo.toml"),
                    )?)
                    .map_err(io::Error::other)?;
                    workspace["workspace"]["package"][field]
                        .as_str()
                        .ok_or_else(|| io::Error::other("workspace package field is absent"))?
                        .to_owned()
                }
                _ => return Err(io::Error::other("package field is absent")),
            }
        }
        _ => {
            return Err(io::Error::other(format!(
                "Cargo constant needs explicit projection audit: {name}"
            )));
        }
    };
    if value == Some(expected.as_str()) {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "Cargo constant differs from reported package: {name}"
        )))
    }
}

/// Verify a consumed workspace or registry input against its authoritative owner.
fn record_input(candidate: &Candidate, path: &Path) -> io::Result<Value> {
    let workspace = Path::new(&candidate.inputs.workspace);
    if path.starts_with(workspace) {
        return local_record(candidate, path);
    }
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".cargo"));
    let registry = cargo_home.join("registry/src");
    let relative = path.strip_prefix(&registry).map_err(|_| {
        io::Error::other("input is neither covered workspace source nor a regular locked registry file")
    })?;
    let parts = relative.components().collect::<Vec<_>>();
    if parts.len() < 3 {
        return Err(io::Error::other("registry source path has no package owner"));
    }
    let Component::Normal(index) = parts[0] else {
        return Err(io::Error::other("registry source path has no package owner"));
    };
    let Component::Normal(package_directory) = parts[1] else {
        return Err(io::Error::other("registry source path has no package owner"));
    };
    let root = registry.join(index).join(package_directory);
    let normalized = normalize_owned_path(path, &root)?;
    let manifest: toml::Value =
        toml::from_str(&fs::read_to_string(root.join("Cargo.toml"))?).map_err(io::Error::other)?;
    let name = manifest["package"]["name"]
        .as_str()
        .ok_or_else(|| io::Error::other("registry package has no name"))?;
    let version = manifest["package"]["version"]
        .as_str()
        .ok_or_else(|| io::Error::other("registry package has no version"))?;
    if package_directory.to_string_lossy() != format!("{name}-{version}") {
        return Err(io::Error::other(
            "extracted registry directory differs from its package identity",
        ));
    }
    let lock: toml::Value =
        toml::from_str(&fs::read_to_string(workspace.join("Cargo.lock"))?).map_err(io::Error::other)?;
    let matches = lock["package"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| {
            entry["name"].as_str() == Some(name)
                && entry["version"].as_str() == Some(version)
                && entry["source"]
                    .as_str()
                    .is_some_and(|source| source.starts_with("registry+"))
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(io::Error::other("registry input lacks one exact locked checksum owner"));
    }
    let checksum = matches[0]["checksum"]
        .as_str()
        .ok_or_else(|| io::Error::other("registry input lacks one exact locked checksum owner"))?;
    let source = matches[0]["source"].as_str().unwrap_or_default();
    let archive = cargo_home
        .join("registry/cache")
        .join(index)
        .join(format!("{}.crate", package_directory.to_string_lossy()));
    let files = authenticated_archive_files(&archive, checksum, &package_directory.to_string_lossy())?;
    let manifest_digest = file_digest(&root.join("Cargo.toml"))?;
    if files.get("Cargo.toml").map(String::as_str) != Some(manifest_digest.as_str()) {
        return Err(io::Error::other(
            "extracted package manifest differs from the locked archive",
        ));
    }
    let relative = normalized
        .strip_prefix(&root)
        .map_err(io::Error::other)?
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let sha256 = file_digest(&normalized)?;
    if files.get(&relative).map(String::as_str) != Some(sha256.as_str()) {
        return Err(io::Error::other("registry file differs from the locked archive"));
    }
    Ok(json!({"path": normalized, "sha256": sha256, "source": source, "package_checksum": checksum}))
}

/// Authenticate and inventory regular members of one locked registry archive without extracting them.
fn authenticated_archive_files(
    archive: &Path,
    checksum: &str,
    package_directory: &str,
) -> io::Result<BTreeMap<String, String>> {
    require_unlinked_regular_file(archive)?;
    if file_digest(archive)? != checksum {
        return Err(io::Error::other("registry archive checksum differs from Cargo.lock"));
    }
    let listing = Command::new("tar").args(["-tzf"]).arg(archive).output()?;
    let verbose = Command::new("tar").args(["-tvzf"]).arg(archive).output()?;
    if !listing.status.success() || !verbose.status.success() {
        return Err(io::Error::other("locked registry archive cannot be decoded"));
    }
    let names = String::from_utf8(listing.stdout)
        .map_err(io::Error::other)?
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let kinds = String::from_utf8(verbose.stdout)
        .map_err(io::Error::other)?
        .lines()
        .filter_map(|line| line.bytes().next())
        .collect::<Vec<_>>();
    if names.len() != kinds.len() {
        return Err(io::Error::other("locked registry archive cannot be decoded"));
    }
    let mut seen = BTreeSet::new();
    let mut files = BTreeMap::new();
    for (member, kind) in names.into_iter().zip(kinds) {
        let relative = validate_archive_member(&member, package_directory)?;
        if !seen.insert(relative.clone()) {
            return Err(io::Error::other("registry archive contains duplicate paths"));
        }
        match kind {
            b'd' => {}
            b'-' => {
                if relative.is_empty() {
                    return Err(io::Error::other(
                        "registry archive contains a linked or unsupported entry",
                    ));
                }
                let output = Command::new("tar").args(["-xOzf"]).arg(archive).arg(&member).output()?;
                if !output.status.success() {
                    return Err(io::Error::other("locked registry archive cannot be decoded"));
                }
                files.insert(relative, bytes_digest(&output.stdout));
            }
            _ => {
                return Err(io::Error::other(
                    "registry archive contains a linked or unsupported entry",
                ));
            }
        }
    }
    if !files.contains_key("Cargo.toml") {
        return Err(io::Error::other("registry archive omits its package manifest"));
    }
    Ok(files)
}

/// Validate one archive member is an unambiguous child of its expected package directory.
fn validate_archive_member(member: &str, package_directory: &str) -> io::Result<String> {
    if member.contains('\\') || member.starts_with('/') {
        return Err(io::Error::other(
            "registry archive contains an escaping or ambiguous path",
        ));
    }
    let parts = member.trim_end_matches('/').split('/').collect::<Vec<_>>();
    if parts.first().copied() != Some(package_directory) || parts.iter().any(|part| matches!(*part, "" | "." | "..")) {
        return Err(io::Error::other(
            "registry archive contains an escaping or ambiguous path",
        ));
    }
    Ok(parts[1..].join("/"))
}

/// Normalize a same-owner path while rejecting links and escape attempts.
fn normalize_owned_path(path: &Path, owner: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() || !owner.is_absolute() {
        return Err(io::Error::other("consumed path has no original source owner"));
    }
    let mut current = owner.to_path_buf();
    let relative = path
        .strip_prefix(owner)
        .map_err(|_| io::Error::other("consumed path has no original source owner"))?;
    for component in relative.components() {
        match component {
            Component::ParentDir => {
                if current == owner || !current.is_dir() {
                    return Err(io::Error::other("consumed path escapes its source owner"));
                }
                current.pop();
            }
            Component::CurDir => {}
            Component::Normal(part) => {
                current.push(part);
                if fs::symlink_metadata(&current)?.file_type().is_symlink() {
                    return Err(io::Error::other("consumed input contains a symlink"));
                }
            }
            _ => return Err(io::Error::other("consumed path has no original source owner")),
        }
    }
    require_unlinked_regular_file(&current)?;
    Ok(current)
}

/// Describe exactly the three executable tool outputs for one candidate identity.
fn output_manifest(directory: &Path, identity: &str) -> io::Result<OutputManifest> {
    if fs::symlink_metadata(directory).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(io::Error::other("tool directory cannot be a symlink"));
    }
    let mut outputs = BTreeMap::new();
    for tool in TOOLS {
        let path = directory.join(tool);
        require_unlinked_regular_file(&path)?;
        if file_mode(&path)? & 0o111 == 0 {
            return Err(io::Error::other(format!("tool is not executable: {tool}")));
        }
        outputs.insert(tool.to_owned(), file_digest(&path)?);
    }
    Ok(OutputManifest {
        schema_version: 1,
        input_identity: identity.to_owned(),
        outputs,
    })
}

/// Verify a bundle's candidate, admitted evidence, consumed files, and tool output bytes.
fn verify_bundle(bundle: &Path, candidate: &Candidate) -> io::Result<()> {
    if fs::symlink_metadata(bundle).is_ok_and(|metadata| metadata.file_type().is_symlink())
        || fs::symlink_metadata(bundle.join("manifest.json")).is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(io::Error::other("bundle or manifest cannot be a symlink"));
    }
    let manifest: Value = read_json(&bundle.join("manifest.json"))?;
    if manifest["inputs"] != serde_json::to_value(candidate).map_err(io::Error::other)?
        || manifest["evidence"]["admitted"] != true
    {
        return Err(io::Error::other("bundle input identity or admission evidence mismatch"));
    }
    let files = manifest["evidence"]["files"]
        .as_array()
        .ok_or_else(|| io::Error::other("bundle has incomplete consumed-input evidence"))?;
    if files.is_empty()
        || manifest["evidence"]["refusals"]
            .as_array()
            .is_none_or(|values| !values.is_empty())
    {
        return Err(io::Error::other("bundle has incomplete consumed-input evidence"));
    }
    for record in files {
        let path = PathBuf::from(
            record["path"]
                .as_str()
                .ok_or_else(|| io::Error::other("consumed file has no path"))?,
        );
        if record_input(candidate, &path)? != *record {
            return Err(io::Error::other("consumed input bytes changed"));
        }
    }
    let environment = manifest["evidence"]["environment"]
        .as_object()
        .ok_or_else(|| io::Error::other("bundle has invalid consumed environment evidence"))?;
    for (name, value) in environment {
        let expected = value.as_str();
        if env::var(name).ok().as_deref() != expected {
            return Err(io::Error::other(format!("consumed environment changed: {name}")));
        }
    }
    let expected: OutputManifest = serde_json::from_value(manifest["tools"].clone()).map_err(io::Error::other)?;
    if output_manifest(bundle, &candidate.identity)? != expected {
        return Err(io::Error::other(
            "tool bundle identity, membership or output digest mismatch",
        ));
    }
    Ok(())
}

/// Recompute candidate bytes and environment before restore or admission.
fn verify_candidate_current(workspace: &Path, candidate: &Candidate) -> io::Result<()> {
    let environment = candidate.inputs.coordinates["environment"]
        .as_object()
        .ok_or_else(|| io::Error::other("candidate environment is not an object"))?;
    for (name, value) in environment {
        let current = env::var(name).ok().map(Value::String).unwrap_or(Value::Null);
        if &current != value {
            return Err(io::Error::other(format!(
                "build environment changed after identity: {name}"
            )));
        }
    }
    if input_manifest(workspace, candidate.inputs.coordinates.clone())? != *candidate {
        return Err(io::Error::other("source inputs changed after identity selection"));
    }
    Ok(())
}

/// Retain raw Cargo messages privately while forwarding only rendered compiler diagnostics.
fn record_cargo_messages(destination: &Path, stream: impl BufRead, mut diagnostics: impl Write) -> io::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = File::create(destination)?;
    for line in stream.lines() {
        let line = line?;
        writeln!(output, "{line}")?;
        let message: Value = serde_json::from_str(&line).map_err(io::Error::other)?;
        if message["reason"] == "compiler-message"
            && let Some(rendered) = message["message"]["rendered"].as_str()
        {
            diagnostics.write_all(rendered.as_bytes())?;
        }
    }
    Ok(())
}

/// Verify that CI invokes the exact fingerprinted recipe through this recorder.
///
/// The recorder is the built executable, not `cargo run`: the build coordinates are read from this process's
/// environment, and a Cargo or Rustup launcher adds its own per-process variables to it.
fn verify_workflow_recipe(workspace: &Path) -> io::Result<()> {
    let workflow = fs::read_to_string(workspace.join(".github/workflows/ci.yml"))?;
    for command in RECIPE {
        let expected = format!("{command} | {RECORDER} record ");
        if workflow.matches(&expected).count() != 1 {
            return Err(io::Error::other(
                "workflow build command differs from the fingerprinted recipe",
            ));
        }
    }
    Ok(())
}

/// Refuse ambient Cargo configurations outside the tracked checkout contract.
fn refuse_ambient_cargo_config(workspace: &Path) -> io::Result<()> {
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".cargo"));
    for root in std::iter::once(cargo_home).chain(workspace.ancestors().skip(1).map(|root| root.join(".cargo"))) {
        for name in ["config", "config.toml"] {
            let path = root.join(name);
            if path.exists() {
                return Err(io::Error::other(format!(
                    "unadmitted ambient Cargo configuration: {}",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

/// Run a bounded identity command and return trimmed stdout.
fn command_text(workspace: &Path, command: &str, arguments: &[&str]) -> io::Result<String> {
    let output = Command::new(command)
        .args(arguments)
        .current_dir(workspace)
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() || output.stdout.len() > 1024 * 1024 || output.stderr.len() > 1024 * 1024 {
        return Err(io::Error::other(format!("{command} identity command failed")));
    }
    Ok(String::from_utf8(output.stdout)
        .map_err(io::Error::other)?
        .trim()
        .to_owned())
}

/// Complete one public operation report.
fn finish_report(
    state: &Path,
    operation: &str,
    status: &str,
    started: Instant,
    refusal_count: usize,
) -> io::Result<()> {
    let report = json!({"operation": operation, "status": status, "refusal_count": refusal_count, "elapsed_seconds": started.elapsed().as_secs_f64()});
    write_json(&state.join(format!("{operation}.json")), &report)?;
    println!("{report}");
    Ok(())
}

/// Append named values to the GitHub step-output file when configured.
fn publish_github_outputs<V: AsRef<str>>(values: &BTreeMap<&str, V>) -> io::Result<()> {
    let Some(path) = env::var_os("GITHUB_OUTPUT") else {
        return Ok(());
    };
    append_github_outputs(Path::new(&path), values)
}

/// Append step outputs to one GitHub output file.
fn append_github_outputs<V: AsRef<str>>(path: &Path, values: &BTreeMap<&str, V>) -> io::Result<()> {
    let mut output = OpenOptions::new().create(true).append(true).open(path)?;
    for (name, value) in values {
        writeln!(output, "{name}={}", value.as_ref())?;
    }
    Ok(())
}

/// Read one JSON artifact.
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<T> {
    serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)
}

/// Publish one pretty JSON artifact with a trailing newline.
fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    fs::write(path, bytes)
}

/// Hash canonical compact JSON with object keys in sorted order.
fn canonical_digest(value: &Value) -> io::Result<String> {
    let canonical = canonical_value(value);
    Ok(bytes_digest(&serde_json::to_vec(&canonical).map_err(io::Error::other)?))
}

/// Recursively sort JSON object keys while retaining array order.
fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .map(|(key, value)| (key.clone(), canonical_value(value)))
                .collect::<Map<_, _>>(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(canonical_value).collect()),
        _ => value.clone(),
    }
}

/// Hash one regular, non-symlink file without loading it into memory.
fn file_digest(path: &Path) -> io::Result<String> {
    require_unlinked_regular_file(path)?;
    let mut input = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// Hash an in-memory byte slice.
fn bytes_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Require one regular file without following a final-component symlink.
fn require_unlinked_regular_file(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::other(format!("expected a regular file: {}", path.display())));
    }
    Ok(())
}

/// Return portable Unix permission bits used by the Linux candidate identity.
fn file_mode(path: &Path) -> io::Result<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(fs::metadata(path)?.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(io::Error::other("tool-output evidence requires Unix permission bits"))
    }
}

/// Copy one regular tool without accepting a symlink source.
fn copy_regular_file(source: &Path, destination: &Path) -> io::Result<()> {
    require_unlinked_regular_file(source)?;
    fs::copy(source, destination)?;
    fs::set_permissions(destination, fs::metadata(source)?.permissions())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a tracked fixture checkout and complete external coordinates.
    fn checkout() -> io::Result<(tempfile::TempDir, Value)> {
        let root = tempfile::tempdir()?;
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .status()?;
        if !status.success() {
            return Err(io::Error::other("git init failed"));
        }
        for (name, contents) in [
            ("Cargo.toml", "[workspace]\n"),
            ("Cargo.lock", "version = 4\n"),
            ("loaves/compiler/main.rs", "fn main() {}\n"),
            ("assets/logo.txt", "logo\n"),
            ("tests/admin.rs", "admin\n"),
        ] {
            let path = root.path().join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, contents)?;
        }
        let status = Command::new("git")
            .args(["add", "."])
            .current_dir(root.path())
            .status()?;
        if !status.success() {
            return Err(io::Error::other("git add failed"));
        }
        Ok((
            root,
            json!({
                "recipe": ["cargo", "build", "--release"],
                "rustc": "rustc identity",
                "cargo": "cargo identity",
                "linker": "linker identity",
                "runner_image": "image revision",
                "environment": {},
            }),
        ))
    }

    /// Create executable tool outputs for manifest tests.
    fn tools(root: &Path) -> io::Result<()> {
        fs::create_dir_all(root)?;
        for tool in TOOLS {
            fs::write(root.join(tool), tool)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(root.join(tool), fs::Permissions::from_mode(0o755))?;
            }
        }
        Ok(())
    }

    #[test]
    fn output_manifest_requires_exact_executable_tools_and_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        tools(root.path())?;
        let manifest = output_manifest(root.path(), "expected")?;
        assert_eq!(manifest.outputs.len(), 3);
        fs::write(root.path().join("incan"), b"tampered")?;
        assert_ne!(output_manifest(root.path(), "expected")?, manifest);
        Ok(())
    }

    #[test]
    fn depfiles_capture_spaces_and_environment_without_expansion() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let depfile = root.path().join("sample.d");
        fs::write(
            &depfile,
            "out: src/a\\ b.rs src/main.rs\n# env-dep:FLAG=value\n# env-dep:ABSENT\n",
        )?;
        let (files, environment) = depfile_facts(&depfile)?;
        assert_eq!(files, vec!["src/a b.rs", "src/main.rs"]);
        assert_eq!(environment.get("FLAG"), Some(&Some("value".to_owned())));
        assert_eq!(environment.get("ABSENT"), Some(&None));
        fs::write(&depfile, "out: $(UNSUPPORTED)/input\n")?;
        assert!(depfile_facts(&depfile).is_err());
        Ok(())
    }

    #[test]
    fn acquisition_credentials_never_enter_build_environment() -> Result<(), Box<dyn std::error::Error>> {
        let clean = build_environment(Vec::<(String, String)>::new())?;
        let token = build_environment(vec![("CARGO_REGISTRY_TOKEN".to_owned(), "secret".to_owned())])?;
        assert_eq!(clean, token);
        let error = build_environment(vec![("CARGO_REGISTRIES_PRIVATE_INDEX".to_owned(), "secret".to_owned())])
            .err()
            .map(|error| error.to_string());
        assert!(
            error
                .as_deref()
                .is_some_and(|error| error.contains("CARGO_REGISTRIES_PRIVATE_INDEX"))
        );
        assert!(error.as_deref().is_some_and(|error| !error.contains("secret")));
        Ok(())
    }

    #[test]
    fn cargo_launcher_variables_are_unsupported_build_configuration() {
        for name in ["CARGO_MANIFEST_DIR", "CARGO_PKG_NAME", "RUST_RECURSION_COUNT"] {
            let error = build_environment(vec![(name.to_owned(), "launcher".to_owned())])
                .err()
                .map(|error| error.to_string());
            assert!(error.as_deref().is_some_and(|error| error.contains(name)), "{name}");
        }
    }

    #[test]
    fn repository_workflow_runs_the_recorder_as_a_plain_executable() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        verify_workflow_recipe(&workspace)?;
        let workflow = fs::read_to_string(workspace.join(".github/workflows/ci.yml"))?;
        assert!(
            !workflow.contains("--bin incan-ci-tool-outputs --"),
            "the recorder reads its own environment as build coordinates and must not be launched through `cargo run`"
        );
        assert!(workflow.contains(&format!("{RECORDER} identity ")));
        Ok(())
    }

    #[test]
    fn unavailable_operation_publishes_fail_closed_outputs_and_completes() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let state = root.path().join("state");
        let github_output = root.path().join("github-output");
        let completion = settle(
            Err(io::Error::other("refused")),
            Some((state.clone(), "identity")),
            Some(&github_output),
        )?;
        assert!(matches!(completion, Completion::Unavailable(error) if error.to_string() == "refused"));
        assert_eq!(
            fs::read_to_string(&github_output)?,
            "admitted=false\neligible=false\nhit=false\n"
        );
        let evidence: Value = read_json(&state.join("identity-unavailable.json"))?;
        assert_eq!(evidence, json!({"status": "unavailable", "detail": "refused"}));

        let untouched = root.path().join("untouched-output");
        assert!(matches!(settle(Ok(()), None, Some(&untouched))?, Completion::Done));
        assert!(!untouched.exists());

        let unpublishable = root.path().join("missing-directory/github-output");
        assert!(settle(Err(io::Error::other("refused")), None, Some(&unpublishable)).is_err());
        Ok(())
    }

    #[test]
    fn cargo_recorder_keeps_raw_facts_private_and_forwards_rendered_diagnostics()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let private = root.path().join("private/messages.jsonl");
        let input = b"{\"reason\":\"compiler-message\",\"message\":{\"rendered\":\"visible diagnostic\\n\"},\"secret\":\"private\"}\n";
        let mut diagnostics = Vec::new();
        record_cargo_messages(&private, &input[..], &mut diagnostics)?;
        assert_eq!(diagnostics, b"visible diagnostic\n");
        assert!(fs::read_to_string(private)?.contains("private"));
        Ok(())
    }

    #[test]
    fn candidate_tracks_sources_config_and_checkout_but_not_administration() -> Result<(), Box<dyn std::error::Error>> {
        let (root, coordinates) = checkout()?;
        let original = input_manifest(root.path(), coordinates.clone())?;
        fs::write(root.path().join("tests/admin.rs"), b"changed administration")?;
        assert_eq!(input_manifest(root.path(), coordinates.clone())?, original);
        fs::write(root.path().join("loaves/compiler/main.rs"), b"changed source")?;
        assert_ne!(input_manifest(root.path(), coordinates)?.identity, original.identity);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn candidate_and_local_records_refuse_symlinks_and_mode_changes() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let (root, coordinates) = checkout()?;
        let candidate = input_manifest(root.path(), coordinates.clone())?;
        let source = root.path().join("loaves/compiler/main.rs");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755))?;
        assert!(local_record(&candidate, &source).is_err());
        fs::remove_file(&source)?;
        symlink(root.path().join("Cargo.toml"), &source)?;
        assert!(input_manifest(root.path(), coordinates).is_err());
        Ok(())
    }

    #[test]
    fn custom_build_depfile_binds_exact_owner_source_and_output() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("target/release/build/example-0123456789abcdef");
        fs::create_dir_all(&directory)?;
        let output = directory.join("build-script-build");
        let source = root.path().join("src/main.rs");
        fs::create_dir_all(source.parent().ok_or("source needs parent")?)?;
        fs::write(&source, b"source")?;
        let depfile = directory.join("build_script_build-0123456789abcdef.d");
        let native = depfile.with_extension("");
        fs::write(&depfile, format!("{}: {}\n", native.display(), source.display()))?;
        let message = json!({
            "package_id": "registry+https://example.invalid#index#example@1.0.0",
            "target": {"kind": ["custom-build"], "src_path": source},
            "filenames": [output],
        });
        assert_eq!(artifact_depfiles(&message)?, vec![depfile.clone()]);
        fs::write(&depfile, format!("{}: other.rs\n", native.display()))?;
        assert!(artifact_depfiles(&message).is_err());
        Ok(())
    }

    #[test]
    fn archive_member_validation_refuses_escape_duplicates_and_links() -> Result<(), Box<dyn std::error::Error>> {
        for member in ["example-1.0.0/../escape", "/absolute", "example-1.0.0/a\\b"] {
            assert!(validate_archive_member(member, "example-1.0.0").is_err());
        }
        assert_eq!(
            validate_archive_member("example-1.0.0/src/lib.rs", "example-1.0.0")?,
            "src/lib.rs"
        );
        Ok(())
    }

    #[test]
    fn failed_and_incomplete_cargo_evidence_refuses_admission() -> Result<(), Box<dyn std::error::Error>> {
        let (root, coordinates) = checkout()?;
        let candidate = input_manifest(root.path(), coordinates)?;
        let log = root.path().join("cargo.jsonl");
        fs::write(&log, "{\"reason\":\"build-finished\",\"success\":false}\n")?;
        assert!(collect_evidence(root.path(), Some(&candidate), &[log.clone(), log]).is_err());
        Ok(())
    }
}
