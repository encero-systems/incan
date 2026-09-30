//! Stable physical rustc-invocation capture for the explicit Cargo compatibility publisher.
//!
//! TODO(#1561): transitional with the publisher's Cargo capture (see the crate header); the trace exists to observe
//! what Cargo asked rustc to do, and has no role once a bake settles from registry records.

use std::collections::BTreeMap;
use std::env;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use serde::{Deserialize, Serialize};

use super::OvenLegacyCargoError;

/// Trace file selected by the explicit compatibility publisher for its wrapper subprocesses.
pub const OVEN_RUSTC_TRACE_PATH_ENV: &str = "INCAN_OVEN_RUSTC_TRACE_PATH";
/// Marker that makes the `incan` or `oven` executable dispatch the rustc wrapper protocol before CLI parsing.
pub const OVEN_RUSTC_TRACE_WRAPPER_ENV: &str = "INCAN_OVEN_RUSTC_TRACE_WRAPPER";
/// Ordered publisher arguments injected into every Cargo rustc invocation, including host-side units.
pub const OVEN_RUSTC_TRACE_EXTRA_ARGUMENTS_ENV: &str = "INCAN_OVEN_RUSTC_TRACE_EXTRA_ARGUMENTS";
const MAX_RUSTC_TRACE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RUSTC_TRACE_RECORD_BYTES: usize = 1024 * 1024;
const MAX_RUSTC_TRACE_RECORDS: usize = 100_000;
const PROC_MACRO_UUID_SALT_ENV: &str = "RC_UUID_SALT";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyRustcInvocation {
    /// Stable discriminator used when the record is mixed into Cargo's JSON message stream.
    pub reason: String,
    /// Exact compiler executable invoked by Cargo, later checked against the publisher request.
    pub rustc: String,
    /// Canonical working directory used to resolve rustc's relative source arguments.
    #[serde(default)]
    pub working_directory: String,
    /// Ordered rustc arguments, excluding the compiler executable itself.
    pub arguments: Vec<String>,
    /// Allowlisted Cargo compilation context needed to bind the invocation to a package and physical variant.
    pub environment: BTreeMap<String, String>,
    /// Digest of bounded source bytes forwarded unchanged when rustc reads its input from stdin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_digest: Option<String>,
    /// Metadata output verified before the invoking build script can remove or overwrite its probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_probe_output: Option<OvenLegacyStdinProbeOutput>,
}

/// Physical evidence captured immediately after a successful stdin probe that emitted only metadata or LLVM IR.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyStdinProbeOutput {
    /// Output path relative to the verified build-script OUT_DIR.
    pub relative_path: String,
    /// Digest of the regular output file before control returns to the build script.
    pub digest: String,
}

/// Run the current executable as Cargo's stable `RUSTC_WRAPPER`, returning `None` for every ordinary invocation.
pub fn run_legacy_rustc_trace_wrapper() -> Option<i32> {
    if env::var_os(OVEN_RUSTC_TRACE_WRAPPER_ENV).as_deref() != Some(std::ffi::OsStr::new("1")) {
        return None;
    }
    Some(run_marked_rustc_trace_wrapper().unwrap_or(1))
}

/// Execute one already-marked wrapper request and fail closed on malformed protocol or trace persistence.
fn run_marked_rustc_trace_wrapper() -> Result<i32, ()> {
    let trace = env::var_os(OVEN_RUSTC_TRACE_PATH_ENV).map(PathBuf::from).ok_or(())?;
    let mut arguments = env::args_os().skip(1);
    let rustc = arguments.next().ok_or(())?;
    let rustc = rustc.into_string().map_err(|_| ())?;
    let mut arguments = arguments
        .map(|argument| argument.into_string())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ())?;
    if let Some(encoded) = env::var_os(OVEN_RUSTC_TRACE_EXTRA_ARGUMENTS_ENV) {
        let encoded = encoded.into_string().map_err(|_| ())?;
        let extra = serde_json::from_str::<Vec<String>>(&encoded).map_err(|_| ())?;
        arguments.extend(extra);
    }
    let proc_macro = compiler_argument_value(&arguments, "--crate-type") == Some("proc-macro");
    let uuid_salt =
        append_stable_proc_macro_install_name(&mut arguments, cfg!(all(target_vendor = "apple", target_os = "macos")))?;
    // Cargo names registry crate roots absolutely even though their manifest directory is already authoritative.
    // Rustc folds that physical spelling into multi-file metadata before path remapping, so run and record the
    // equivalent package-relative spelling from the manifest directory. This is part of the observed Cargo build,
    // not a later artifact normalization.
    let compiler_working_directory = if proc_macro {
        None
    } else {
        normalize_cargo_crate_root(&mut arguments)?
    };
    let mut environment: BTreeMap<String, String> = env::vars()
        .filter(|(name, _)| rustc_compile_environment_name(name))
        .collect();
    if let Some(uuid_salt) = &uuid_salt {
        environment.insert(PROC_MACRO_UUID_SALT_ENV.to_string(), uuid_salt.clone());
    }
    let working_directory = compiler_working_directory
        .clone()
        .map(Ok)
        .unwrap_or_else(env::current_dir)
        .and_then(std::fs::canonicalize)
        .map_err(|_| ())?
        .into_os_string()
        .into_string()
        .map_err(|_| ())?;
    let stdin_source = if rustc_positional_source(&arguments) == Some("-") {
        Some(read_bounded_rustc_stdin(std::io::stdin())?)
    } else {
        None
    };
    let stdin_digest = stdin_source.as_deref().map(super::digest_bytes);
    let status = run_traced_rustc_in(
        &rustc,
        &arguments,
        stdin_source,
        compiler_working_directory.as_deref(),
        uuid_salt.as_deref(),
    )?;
    let exit_code = status.code().unwrap_or(1);
    if !status.success()
        || arguments
            .iter()
            .any(|argument| argument == "-vV" || argument == "--version")
        || arguments
            .iter()
            .any(|argument| argument == "--print" || argument.starts_with("--print="))
    {
        return Ok(exit_code);
    }
    let mut record = OvenLegacyRustcInvocation {
        reason: "incan-rustc-invocation".to_string(),
        rustc: rustc.clone(),
        working_directory,
        arguments: arguments.clone(),
        environment,
        stdin_digest,
        stdin_probe_output: None,
    };
    record.stdin_probe_output = super::selected_unit_capture::capture_stdin_probe_output(&record);
    let encoded = serde_json::to_vec(&record).map_err(|_| ())?;
    if encoded.len() > MAX_RUSTC_TRACE_RECORD_BYTES {
        return Err(());
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(trace)
        .map_err(|_| ())?;
    File::lock(&file).map_err(|_| ())?;
    let existing = file.metadata().map_err(|_| ())?.len();
    if existing.saturating_add(encoded.len() as u64).saturating_add(1) > MAX_RUSTC_TRACE_BYTES {
        let _ = File::unlock(&file);
        return Err(());
    }
    let written = file.write_all(&encoded).and_then(|_| file.write_all(b"\n"));
    let _ = File::unlock(&file);
    if written.is_err() {
        return Err(());
    }
    Ok(exit_code)
}

/// Select Cargo's documented compile-time crate environment without inheriting Cargo's own ambient configuration.
fn rustc_compile_environment_name(name: &str) -> bool {
    matches!(
        name,
        "CARGO_BIN_NAME"
            | "CARGO_CRATE_NAME"
            | "CARGO_MANIFEST_DIR"
            | "CARGO_MANIFEST_PATH"
            | "CARGO_PRIMARY_PACKAGE"
            | "DEBUG"
            | "HOST"
            | "OPT_LEVEL"
            | "OUT_DIR"
            | PROC_MACRO_UUID_SALT_ENV
            | "PROFILE"
            | "TARGET"
    ) || name.starts_with("CARGO_FEATURE_")
        || name.starts_with("CARGO_PKG_")
}

/// Read one compiler stdin source without allocating beyond the per-record capture limit.
fn read_bounded_rustc_stdin(mut source: impl Read) -> Result<Vec<u8>, ()> {
    let mut bytes = Vec::new();
    source
        .by_ref()
        .take(MAX_RUSTC_TRACE_RECORD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    (bytes.len() <= MAX_RUSTC_TRACE_RECORD_BYTES).then_some(bytes).ok_or(())
}

/// Run rustc while delivering captured stdin concurrently so an early exit always closes the pipe writer.
#[cfg(test)]
fn run_traced_rustc(
    rustc: &str,
    arguments: &[String],
    stdin_source: Option<Vec<u8>>,
) -> Result<std::process::ExitStatus, ()> {
    run_traced_rustc_in(rustc, arguments, stdin_source, None, None)
}

/// Run one traced compiler from an explicitly captured Cargo working directory when normalization requires it.
fn run_traced_rustc_in(
    rustc: &str,
    arguments: &[String],
    stdin_source: Option<Vec<u8>>,
    working_directory: Option<&Path>,
    uuid_salt: Option<&str>,
) -> Result<std::process::ExitStatus, ()> {
    let mut command = Command::new(rustc);
    command.args(arguments);
    if let Some(uuid_salt) = uuid_salt {
        command.env(PROC_MACRO_UUID_SALT_ENV, uuid_salt);
        // Cargo's bounded native-tool capture sets a CommandLineTools SDKROOT while the Cargo-free executor would
        // otherwise inherit the caller's Xcode SDKROOT. Apple ld folds that ambient path into a proc-macro's
        // content-derived UUID even when every section is otherwise identical, so neither side may inherit it.
        command.env_remove("SDKROOT");
    }
    if let Some(working_directory) = working_directory {
        command.current_dir(working_directory);
    }
    let Some(bytes) = stdin_source else {
        return command.status().map_err(|_| ());
    };
    let mut child = command.stdin(Stdio::piped()).spawn().map_err(|_| ())?;
    let mut stdin = child.stdin.take().ok_or(())?;
    let writer = thread::spawn(move || stdin.write_all(&bytes));
    let status = child.wait();
    let write_result = writer.join().map_err(|_| ())?;
    let status = status.map_err(|_| ())?;
    if write_result.is_err() && status.success() {
        return Err(());
    }
    Ok(status)
}

/// Replace an absolute Cargo crate root below `CARGO_MANIFEST_DIR` with its package-relative spelling.
fn normalize_cargo_crate_root(arguments: &mut [String]) -> Result<Option<PathBuf>, ()> {
    let Some(source) = rustc_positional_source(arguments) else {
        return Ok(None);
    };
    if Path::new(source).is_relative() || source == "-" {
        return Ok(None);
    }
    let manifest = env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from).ok_or(())?;
    normalize_cargo_crate_root_for_manifest(arguments, &manifest)
}

/// Normalize one absolute crate root against an already captured Cargo manifest directory.
fn normalize_cargo_crate_root_for_manifest(arguments: &mut [String], manifest: &Path) -> Result<Option<PathBuf>, ()> {
    let Some(index) = rustc_positional_source_index(arguments) else {
        return Ok(None);
    };
    let source = Path::new(&arguments[index]);
    if source.is_relative() || source == Path::new("-") {
        return Ok(None);
    }
    let relative = source.strip_prefix(manifest).map_err(|_| ())?;
    arguments[index] = relative.to_string_lossy().into_owned();
    Ok(Some(manifest.to_path_buf()))
}

/// Give Apple proc-macro dylibs reproducible linker identity before Cargo and Oven compile the captured unit.
///
/// Apple's linker otherwise derives `LC_ID_DYLIB` from rustc's physical output path. Cargo's capture directory and
/// Oven's closure directory are intentionally different, so that default changes the load-command size, section
/// offsets, UUID, and every downstream crate hash even when all compiler-visible source facts are identical. The
/// reproducible mode also excludes physical input properties from the content-derived UUID and ad-hoc signature.
fn append_stable_proc_macro_install_name(arguments: &mut Vec<String>, apple_host: bool) -> Result<Option<String>, ()> {
    if !apple_host || compiler_argument_value(arguments, "--crate-type") != Some("proc-macro") {
        return Ok(None);
    }
    let crate_name = compiler_argument_value(arguments, "--crate-name")
        .ok_or(())?
        .to_string();
    let extra_filename = codegen_argument_value(arguments, "extra-filename")
        .unwrap_or_default()
        .to_string();
    let install_name = format!("@rpath/lib{crate_name}{extra_filename}.dylib");
    arguments.extend([
        "-C".to_string(),
        "link-arg=-Wl,-reproducible".to_string(),
        "-C".to_string(),
        "link-arg=-install_name".to_string(),
        "-C".to_string(),
        format!("link-arg={install_name}"),
    ]);
    Ok(Some(format!("incan-oven:{crate_name}{extra_filename}")))
}

/// Read one joined or separate long rustc option without assigning meaning to positional arguments.
fn compiler_argument_value<'a>(arguments: &'a [String], option: &str) -> Option<&'a str> {
    arguments.iter().enumerate().find_map(|(index, argument)| {
        if argument == option {
            return arguments.get(index + 1).map(String::as_str);
        }
        argument.strip_prefix(option).and_then(|value| value.strip_prefix('='))
    })
}

/// Read the final value of one joined or separate `-C` rustc option.
fn codegen_argument_value<'a>(arguments: &'a [String], option: &str) -> Option<&'a str> {
    arguments
        .iter()
        .enumerate()
        .filter_map(|(index, argument)| {
            let value = if argument == "-C" || argument == "--codegen" {
                arguments.get(index + 1)?.as_str()
            } else if let Some(value) = argument.strip_prefix("-C") {
                value
            } else {
                argument.strip_prefix("--codegen=")?
            };
            value.strip_prefix(option)?.strip_prefix('=')
        })
        .next_back()
}

/// Return the single positional rustc source after excluding values consumed by known value-taking options.
pub(crate) fn rustc_positional_source(arguments: &[String]) -> Option<&str> {
    rustc_positional_source_index(arguments).map(|index| arguments[index].as_str())
}

/// Return the index of the single positional rustc source after excluding known option values.
fn rustc_positional_source_index(arguments: &[String]) -> Option<usize> {
    let mut source = None;
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        if rustc_option_takes_separate_value(argument) {
            index = index.saturating_add(2);
            continue;
        }
        if argument.starts_with('-') && argument != "-" {
            index += 1;
            continue;
        }
        if source.replace(index).is_some() {
            return None;
        }
        index += 1;
    }
    source
}

/// Value-taking options accepted in captured Cargo/rustc invocations.
fn rustc_option_takes_separate_value(argument: &str) -> bool {
    matches!(
        argument,
        "--allow"
            | "--cap-lints"
            | "--cfg"
            | "--check-cfg"
            | "--codegen"
            | "--color"
            | "--crate-name"
            | "--crate-type"
            | "--deny"
            | "--diagnostic-width"
            | "--edition"
            | "--emit"
            | "--error-format"
            | "--extern"
            | "--forbid"
            | "--json"
            | "--out-dir"
            | "--print"
            | "--remap-path-prefix"
            | "--sysroot"
            | "--target"
            | "--warn"
            | "-A"
            | "-C"
            | "-D"
            | "-F"
            | "-L"
            | "-W"
            | "-l"
            | "-o"
    )
}

/// Validate a bounded wrapper trace and append its records to the matching Cargo JSON stream.
pub(crate) fn append_rustc_trace(stdout: &mut Vec<u8>, trace: &Path) -> Result<(), OvenLegacyCargoError> {
    let file = match File::open(trace) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if cargo_output_is_entirely_fresh(stdout) {
                return Ok(());
            }
            return Err(OvenLegacyCargoError::Plan(
                "stable Cargo compilation emitted no rustc invocation trace".to_string(),
            ));
        }
        Err(source) => {
            return Err(OvenLegacyCargoError::Io {
                path: trace.to_path_buf(),
                source,
            });
        }
    };
    let size = file
        .metadata()
        .map_err(|source| OvenLegacyCargoError::Io {
            path: trace.to_path_buf(),
            source,
        })?
        .len();
    if size > MAX_RUSTC_TRACE_BYTES {
        return Err(OvenLegacyCargoError::Plan(
            "stable rustc invocation trace exceeds its byte limit".to_string(),
        ));
    }
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut records = 0usize;
    let mut total_read = 0u64;
    loop {
        line.clear();
        let mut terminated = false;
        loop {
            let available = reader.fill_buf().map_err(|source| OvenLegacyCargoError::Io {
                path: trace.to_path_buf(),
                source,
            })?;
            if available.is_empty() {
                break;
            }
            let consumed = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |position| position + 1);
            let payload = if available.get(consumed - 1) == Some(&b'\n') {
                &available[..consumed - 1]
            } else {
                &available[..consumed]
            };
            total_read = total_read.saturating_add(consumed as u64);
            if total_read > MAX_RUSTC_TRACE_BYTES {
                return Err(OvenLegacyCargoError::Plan(
                    "stable rustc invocation trace exceeds its byte limit".to_string(),
                ));
            }
            if line.len().saturating_add(payload.len()) > MAX_RUSTC_TRACE_RECORD_BYTES {
                return Err(OvenLegacyCargoError::Plan(
                    "stable rustc invocation trace record exceeds its byte limit".to_string(),
                ));
            }
            line.extend_from_slice(payload);
            terminated = consumed > payload.len();
            reader.consume(consumed);
            if terminated {
                break;
            }
        }
        if line.is_empty() && !terminated {
            break;
        }
        if !terminated {
            return Err(OvenLegacyCargoError::Plan(
                "stable rustc invocation trace ends with an incomplete record".to_string(),
            ));
        }
        if line.is_empty() {
            continue;
        }
        records += 1;
        if records > MAX_RUSTC_TRACE_RECORDS {
            return Err(OvenLegacyCargoError::Plan(
                "stable rustc invocation trace exceeds its record limit".to_string(),
            ));
        }
        serde_json::from_slice::<OvenLegacyRustcInvocation>(&line).map_err(|error| {
            OvenLegacyCargoError::Plan(format!("stable rustc invocation trace is invalid: {error}"))
        })?;
        stdout.extend_from_slice(&line);
        stdout.push(b'\n');
    }
    Ok(())
}

/// Return whether Cargo reported at least one compiler artifact and explicitly marked every artifact fresh.
fn cargo_output_is_entirely_fresh(stdout: &[u8]) -> bool {
    let mut artifacts = 0usize;
    for line in stdout.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()) {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("reason").and_then(serde_json::Value::as_str) != Some("compiler-artifact") {
            continue;
        }
        artifacts += 1;
        if value.get("fresh").and_then(serde_json::Value::as_bool) != Some(true) {
            return false;
        }
    }
    artifacts > 0
}

/// Return the exact current CLI only when it is one of the two binaries that dispatch the trace-wrapper protocol.
pub(crate) fn current_rustc_trace_wrapper() -> Result<Option<PathBuf>, OvenLegacyCargoError> {
    let executable = env::current_exe().map_err(|source| OvenLegacyCargoError::Io {
        path: PathBuf::from("current executable"),
        source,
    })?;
    let executable = std::fs::canonicalize(&executable).map_err(|source| OvenLegacyCargoError::Io {
        path: executable,
        source,
    })?;
    let stem = executable.file_stem().and_then(|name| name.to_str());
    Ok(matches!(stem, Some("incan" | "oven")).then_some(executable))
}

/// Return whether a publisher transaction contains at least one retained stable rustc invocation.
pub(crate) fn outputs_have_rustc_trace(outputs: &[super::CargoInvocationOutput]) -> bool {
    outputs.iter().any(|output| {
        output.stdout.split(|byte| *byte == b'\n').any(|line| {
            serde_json::from_slice::<serde_json::Value>(line)
                .ok()
                .and_then(|value| {
                    value
                        .get("reason")
                        .and_then(serde_json::Value::as_str)
                        .map(ToString::to_string)
                })
                .as_deref()
                == Some("incan-rustc-invocation")
        })
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn appended_trace_refuses_malformed_and_oversized_records() -> Result<(), Box<dyn std::error::Error>> {
        let scratch = tempfile::tempdir()?;
        let trace = scratch.path().join("trace.jsonl");
        fs::write(&trace, b"{malformed}\n")?;
        assert!(append_rustc_trace(&mut Vec::new(), &trace).is_err());

        let mut maximum = serde_json::to_vec(&OvenLegacyRustcInvocation {
            reason: "incan-rustc-invocation".to_string(),
            rustc: "/verified/rustc".to_string(),
            working_directory: "/fixture".to_string(),
            arguments: vec!["--crate-name".to_string(), "fixture".to_string()],
            environment: BTreeMap::new(),
            stdin_digest: None,
            stdin_probe_output: None,
        })?;
        maximum.resize(MAX_RUSTC_TRACE_RECORD_BYTES, b' ');
        maximum.push(b'\n');
        fs::write(&trace, &maximum)?;
        let mut stdout = Vec::new();
        append_rustc_trace(&mut stdout, &trace)?;
        assert_eq!(stdout, maximum);

        fs::write(&trace, vec![b'x'; MAX_RUSTC_TRACE_RECORD_BYTES + 1])?;
        assert!(append_rustc_trace(&mut Vec::new(), &trace).is_err());
        Ok(())
    }

    #[test]
    fn absent_trace_is_allowed_only_for_explicitly_fresh_artifacts() {
        let fresh = br#"{"reason":"compiler-artifact","fresh":true}
"#;
        let rebuilt = br#"{"reason":"compiler-artifact","fresh":false}
"#;
        assert!(cargo_output_is_entirely_fresh(fresh));
        assert!(!cargo_output_is_entirely_fresh(rebuilt));
        assert!(!cargo_output_is_entirely_fresh(b"not-json\n"));
    }

    /// Compile-time package metadata is captured while Cargo's ambient configuration remains excluded.
    #[test]
    fn compile_environment_filter_covers_cargo_package_contract() {
        for name in [
            "CARGO_CRATE_NAME",
            "CARGO_MANIFEST_DIR",
            "CARGO_MANIFEST_PATH",
            "CARGO_PKG_AUTHORS",
            "CARGO_PKG_DESCRIPTION",
            "CARGO_PKG_HOMEPAGE",
            "CARGO_PKG_LICENSE",
            "CARGO_PKG_LICENSE_FILE",
            "CARGO_PKG_NAME",
            "CARGO_PKG_README",
            "CARGO_PKG_REPOSITORY",
            "CARGO_PKG_RUST_VERSION",
            "CARGO_PKG_VERSION",
            "CARGO_PKG_VERSION_MAJOR",
            "CARGO_PKG_VERSION_MINOR",
            "CARGO_PKG_VERSION_PATCH",
            "CARGO_PKG_VERSION_PRE",
            "OUT_DIR",
        ] {
            assert!(
                rustc_compile_environment_name(name),
                "compile fact `{name}` was omitted"
            );
        }
        for name in ["CARGO_HOME", "CARGO_TARGET_DIR", "CARGO_ENCODED_RUSTFLAGS"] {
            assert!(
                !rustc_compile_environment_name(name),
                "ambient Cargo setting `{name}` leaked"
            );
        }
    }

    #[test]
    fn positional_stdin_is_distinct_from_an_option_value() {
        let rustix = vec![
            "--crate-type=rlib".to_string(),
            "--emit=metadata".to_string(),
            "--target".to_string(),
            "fixture-target".to_string(),
            "-o".to_string(),
            "probe.rmeta".to_string(),
            "-".to_string(),
        ];
        assert_eq!(rustc_positional_source(&rustix), Some("-"));

        let option_value = vec!["--extern".to_string(), "-".to_string(), "src/lib.rs".to_string()];
        assert_eq!(rustc_positional_source(&option_value), Some("src/lib.rs"));
    }

    #[test]
    fn cargo_crate_root_normalization_pins_the_relative_compiler_argument() -> Result<(), Box<dyn std::error::Error>> {
        let mut arguments = vec![
            "--crate-name".to_string(),
            "fixture".to_string(),
            "/cargo/registry/fixture/src/lib.rs".to_string(),
            "--out-dir".to_string(),
            "/cargo/target".to_string(),
        ];
        let working = normalize_cargo_crate_root_for_manifest(&mut arguments, Path::new("/cargo/registry/fixture"))
            .map_err(|()| "crate root normalization failed")?;
        assert_eq!(working, Some(PathBuf::from("/cargo/registry/fixture")));
        assert_eq!(rustc_positional_source(&arguments), Some("src/lib.rs"));
        Ok(())
    }

    /// Apple proc-macro capture pins the install name that would otherwise expose Cargo's staging directory.
    #[test]
    fn proc_macro_install_name_is_portable_and_captured() -> Result<(), Box<dyn std::error::Error>> {
        let mut arguments = vec![
            "--crate-name".to_string(),
            "fixture_macros".to_string(),
            "--crate-type=proc-macro".to_string(),
            "-C".to_string(),
            "extra-filename=-sealed".to_string(),
            "src/lib.rs".to_string(),
        ];
        let salt =
            append_stable_proc_macro_install_name(&mut arguments, true).map_err(|()| "install-name capture failed")?;
        assert_eq!(salt.as_deref(), Some("incan-oven:fixture_macros-sealed"));
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["-C", "link-arg=-Wl,-reproducible"])
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["-C", "link-arg=-install_name"])
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["-C", "link-arg=@rpath/libfixture_macros-sealed.dylib"])
        );
        Ok(())
    }

    #[test]
    fn bounded_stdin_refuses_the_first_byte_beyond_the_limit() {
        let exact = vec![b'x'; MAX_RUSTC_TRACE_RECORD_BYTES];
        assert_eq!(read_bounded_rustc_stdin(exact.as_slice()), Ok(exact));
        let oversized = vec![b'x'; MAX_RUSTC_TRACE_RECORD_BYTES + 1];
        assert!(read_bounded_rustc_stdin(oversized.as_slice()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn traced_rustc_forwards_exact_stdin_and_joins_an_early_exit() -> Result<(), Box<dyn std::error::Error>> {
        let scratch = tempfile::tempdir()?;
        let received = scratch.path().join("received.rs");
        let reader = scratch.path().join("reader.sh");
        fs::write(&reader, format!("#!/bin/sh\ncat > '{}'\n", received.display()))?;
        fs::set_permissions(&reader, fs::Permissions::from_mode(0o755))?;
        let source = b"pub fn exact() {}\n".to_vec();
        let status = run_traced_rustc(
            reader.to_str().ok_or("reader path is not UTF-8")?,
            &[],
            Some(source.clone()),
        )
        .map_err(|()| "stdin forwarding fixture failed")?;
        assert!(status.success());
        assert_eq!(fs::read(received)?, source);

        let failure = scratch.path().join("failure.sh");
        fs::write(&failure, "#!/bin/sh\nexit 23\n")?;
        fs::set_permissions(&failure, fs::Permissions::from_mode(0o755))?;
        let status = run_traced_rustc(
            failure.to_str().ok_or("failure path is not UTF-8")?,
            &[],
            Some(vec![b'x'; MAX_RUSTC_TRACE_RECORD_BYTES]),
        )
        .map_err(|()| "early-exit fixture failed")?;
        assert_eq!(status.code(), Some(23));
        Ok(())
    }
}
