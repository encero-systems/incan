//! Stable C/C++ compiler and archiver capture for the explicit Cargo compatibility publisher.

use std::collections::BTreeMap;
use std::env;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use serde::{Deserialize, Serialize};

use super::OvenLegacyCargoError;

/// Marker selecting the native compiler wrapper protocol.
pub const OVEN_NATIVE_TRACE_WRAPPER_ENV: &str = "INCAN_OVEN_NATIVE_TRACE_WRAPPER";
/// Trace file receiving successful native compiler and archiver invocations.
pub const OVEN_NATIVE_TRACE_PATH_ENV: &str = "INCAN_OVEN_NATIVE_TRACE_PATH";
/// Exact executable the wrapper delegates to.
pub const OVEN_NATIVE_TRACE_EXECUTABLE_ENV: &str = "INCAN_OVEN_NATIVE_TRACE_EXECUTABLE";
/// Exact C++ executable the C++ wrapper delegates to.
pub const OVEN_NATIVE_TRACE_CXX_EXECUTABLE_ENV: &str = "INCAN_OVEN_NATIVE_TRACE_CXX_EXECUTABLE";
const MAX_NATIVE_TRACE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_NATIVE_TRACE_RECORD_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeTraceRole {
    Compiler,
    CxxCompiler,
    Archiver,
}

/// One exact native compiler or archiver invocation made by a Cargo build script.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyNativeInvocation {
    /// `incan-native-compile-invocation` or `incan-native-archive-invocation`.
    pub reason: String,
    /// Exact real executable selected by the compatibility publisher.
    pub executable: String,
    /// Canonical working directory used to resolve relative arguments.
    pub working_directory: String,
    /// Ordered invocation arguments excluding argv zero.
    pub arguments: Vec<String>,
    /// Build-script package and target context carried through by Cargo.
    pub environment: BTreeMap<String, String>,
    /// Output captured immediately after a successful invocation, when it is a regular file below `OUT_DIR`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<OvenLegacyNativeProbeOutput>,
}

/// Physical output evidence retained before a native compiler probe can remove or overwrite its file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyNativeProbeOutput {
    /// Output path relative to the verified build-script `OUT_DIR`.
    pub relative_path: String,
    /// Digest of the regular output file before control returns to the build script.
    pub digest: String,
}

/// Run the current executable as a marked native trace wrapper, returning `None` for ordinary invocations.
pub fn run_legacy_native_trace_wrapper() -> Option<i32> {
    if env::var_os(OVEN_NATIVE_TRACE_WRAPPER_ENV).as_deref() != Some(std::ffi::OsStr::new("1")) {
        return None;
    }
    let wrapper_name = env::args_os().next()?;
    let role = native_trace_role(Path::new(&wrapper_name))?;
    Some(run_marked_native_trace_wrapper(role).unwrap_or(1))
}

/// Execute one native tool and append its successful invocation atomically.
fn run_marked_native_trace_wrapper(role: NativeTraceRole) -> Result<i32, ()> {
    let trace = env::var_os(OVEN_NATIVE_TRACE_PATH_ENV).map(PathBuf::from).ok_or(())?;
    let arguments = env::args_os()
        .skip(1)
        .map(|argument| argument.into_string())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ())?;
    let reason = match role {
        NativeTraceRole::Compiler => "incan-native-compile-invocation",
        NativeTraceRole::CxxCompiler => "incan-native-compile-invocation",
        NativeTraceRole::Archiver => "incan-native-archive-invocation",
    };
    let executable = native_executable(role, |name| env::var(name).ok()).ok_or(())?;
    let status = run_native_command(&executable, &arguments)?;
    let exit_code = status.code().unwrap_or(1);
    if !status.success() {
        return Ok(exit_code);
    }
    let environment = env::vars()
        .filter(|(name, _)| {
            matches!(name.as_str(), "CARGO_MANIFEST_DIR" | "HOST" | "OUT_DIR" | "TARGET")
                || name.starts_with("CARGO_PKG_")
                || name.starts_with("CARGO_FEATURE_")
        })
        .collect();
    let working_directory = env::current_dir()
        .and_then(std::fs::canonicalize)
        .map_err(|_| ())?
        .into_os_string()
        .into_string()
        .map_err(|_| ())?;
    let output = native_output_evidence(&arguments, &environment);
    let record = OvenLegacyNativeInvocation {
        reason: reason.to_string(),
        executable,
        working_directory,
        arguments,
        environment,
        output,
    };
    let encoded = serde_json::to_vec(&record).map_err(|_| ())?;
    if encoded.len() > MAX_NATIVE_TRACE_RECORD_BYTES {
        return Err(());
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(trace)
        .map_err(|_| ())?;
    File::lock(&file).map_err(|_| ())?;
    let existing = file.metadata().map_err(|_| ())?.len();
    if existing.saturating_add(encoded.len() as u64).saturating_add(1) > MAX_NATIVE_TRACE_BYTES {
        let _ = File::unlock(&file);
        return Err(());
    }
    let written = file.write_all(&encoded).and_then(|_| file.write_all(b"\n"));
    let _ = File::unlock(&file);
    written.map_err(|_| ())?;
    Ok(exit_code)
}

/// Run exactly the declared native executable with the captured arguments.
fn run_native_command(executable: &str, arguments: &[String]) -> Result<ExitStatus, ()> {
    Command::new(executable).args(arguments).status().map_err(|_| ())
}

/// Select the exact declared executable for one wrapper role without performing discovery.
fn native_executable(role: NativeTraceRole, environment: impl FnOnce(&str) -> Option<String>) -> Option<String> {
    environment(match role {
        NativeTraceRole::Compiler => OVEN_NATIVE_TRACE_EXECUTABLE_ENV,
        NativeTraceRole::CxxCompiler => OVEN_NATIVE_TRACE_CXX_EXECUTABLE_ENV,
        NativeTraceRole::Archiver => "INCAN_OVEN_NATIVE_ARCHIVER",
    })
}

/// Capture one `-o` regular file below `OUT_DIR` before returning control to the build script.
fn native_output_evidence(
    arguments: &[String],
    environment: &BTreeMap<String, String>,
) -> Option<OvenLegacyNativeProbeOutput> {
    let outputs = arguments
        .windows(2)
        .filter(|pair| pair[0] == "-o")
        .map(|pair| pair[1].as_str())
        .collect::<Vec<_>>();
    let [output] = outputs.as_slice() else {
        return None;
    };
    let out_dir = std::fs::canonicalize(environment.get("OUT_DIR")?).ok()?;
    let output = std::fs::canonicalize(output).ok()?;
    let relative = output.strip_prefix(out_dir).ok()?;
    if !output.is_file() {
        return None;
    }
    Some(OvenLegacyNativeProbeOutput {
        relative_path: relative.to_string_lossy().into_owned(),
        digest: super::digest_bytes(&std::fs::read(output).ok()?),
    })
}

/// Classify only private native aliases so the co-resident rustc wrapper continues to its own protocol.
fn native_trace_role(wrapper: &Path) -> Option<NativeTraceRole> {
    let name = wrapper.file_name().and_then(std::ffi::OsStr::to_str)?;
    if name.contains("native-cc-trace") {
        Some(NativeTraceRole::Compiler)
    } else if name.contains("native-cxx-trace") {
        Some(NativeTraceRole::CxxCompiler)
    } else if name.contains("native-ar-trace") {
        Some(NativeTraceRole::Archiver)
    } else {
        None
    }
}

/// Validate the native trace and append its records to Cargo's structured message stream.
pub(crate) fn append_native_trace(stdout: &mut Vec<u8>, trace: &Path) -> Result<(), OvenLegacyCargoError> {
    let file = match File::open(trace) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(OvenLegacyCargoError::Io {
                path: trace.to_path_buf(),
                source,
            });
        }
    };
    if file
        .metadata()
        .map_err(|source| OvenLegacyCargoError::Io {
            path: trace.to_path_buf(),
            source,
        })?
        .len()
        > MAX_NATIVE_TRACE_BYTES
    {
        return Err(OvenLegacyCargoError::Plan(
            "native invocation trace exceeds its byte limit".to_string(),
        ));
    }
    for line in BufReader::new(file).split(b'\n') {
        let line = line.map_err(|source| OvenLegacyCargoError::Io {
            path: trace.to_path_buf(),
            source,
        })?;
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_NATIVE_TRACE_RECORD_BYTES {
            return Err(OvenLegacyCargoError::Plan(
                "native invocation trace record exceeds its byte limit".to_string(),
            ));
        }
        serde_json::from_slice::<OvenLegacyNativeInvocation>(&line)
            .map_err(|error| OvenLegacyCargoError::Plan(format!("native invocation trace is invalid: {error}")))?;
        stdout.extend_from_slice(&line);
        stdout.push(b'\n');
    }
    Ok(())
}

/// Return the current CLI only when it can dispatch the native trace protocol.
pub(crate) fn current_native_trace_wrapper() -> Result<Option<PathBuf>, OvenLegacyCargoError> {
    super::rustc_trace::current_rustc_trace_wrapper()
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Preserve a valid native record byte-for-byte in Cargo's structured stream.
    #[test]
    fn appends_a_valid_native_trace_record() -> TestResult {
        let fixture = tempfile::tempdir()?;
        let trace = fixture.path().join("native.jsonl");
        let record = OvenLegacyNativeInvocation {
            reason: "incan-native-compile-invocation".to_string(),
            executable: "/owner/usr/bin/clang".to_string(),
            working_directory: "/crate".to_string(),
            arguments: vec!["-c".to_string(), "source.c".to_string()],
            environment: BTreeMap::from([
                ("CARGO_MANIFEST_DIR".to_string(), "/crate".to_string()),
                ("OUT_DIR".to_string(), "/target/out".to_string()),
                ("TARGET".to_string(), "aarch64-apple-darwin".to_string()),
            ]),
            output: None,
        };
        let mut encoded = serde_json::to_vec(&record)?;
        encoded.push(b'\n');
        std::fs::write(&trace, &encoded)?;
        let mut stdout = b"cargo-record\n".to_vec();

        append_native_trace(&mut stdout, &trace)?;

        assert_eq!(stdout, [b"cargo-record\n".as_slice(), encoded.as_slice()].concat());
        Ok(())
    }

    /// Refuse malformed wrapper output before it reaches selected-unit capture.
    #[test]
    fn refuses_a_malformed_native_trace_record() -> TestResult {
        let fixture = tempfile::tempdir()?;
        let trace = fixture.path().join("native.jsonl");
        std::fs::write(&trace, b"{not-json}\n")?;

        let error = append_native_trace(&mut Vec::new(), &trace)
            .err()
            .ok_or("malformed native trace was accepted")?;

        assert!(error.to_string().contains("native invocation trace is invalid"));
        Ok(())
    }

    /// Dispatch only aliases installed in `CC`/`CXX` and `AR`, never the ordinary CLI or rustc wrapper path.
    #[test]
    fn classifies_only_native_trace_aliases() {
        assert_eq!(
            native_trace_role(Path::new("/target/.oven.native-cc-trace")),
            Some(NativeTraceRole::Compiler)
        );
        assert_eq!(
            native_trace_role(Path::new("/target/.oven.native-cxx-trace")),
            Some(NativeTraceRole::CxxCompiler)
        );
        assert_eq!(
            native_trace_role(Path::new("/target/.oven.native-ar-trace")),
            Some(NativeTraceRole::Archiver)
        );
        assert_eq!(native_trace_role(Path::new("/toolchain/bin/incan")), None);
    }

    /// Select the declared C++ driver for the CXX alias rather than reusing the C driver.
    #[test]
    #[cfg(unix)]
    fn cxx_alias_selects_the_declared_cxx_driver() -> TestResult {
        use std::os::unix::fs::PermissionsExt;

        let fixture = tempfile::tempdir()?;
        let c_driver = fixture.path().join("clang");
        let cxx_driver = fixture.path().join("clang++");
        let marker = fixture.path().join("selected");
        std::fs::write(&c_driver, "#!/bin/sh\nexit 9\n")?;
        std::fs::write(&cxx_driver, "#!/bin/sh\nprintf cxx > \"$1\"\n")?;
        std::fs::set_permissions(&c_driver, std::fs::Permissions::from_mode(0o755))?;
        std::fs::set_permissions(&cxx_driver, std::fs::Permissions::from_mode(0o755))?;
        let selected = native_executable(NativeTraceRole::CxxCompiler, |name| {
            BTreeMap::from([
                (
                    OVEN_NATIVE_TRACE_EXECUTABLE_ENV,
                    c_driver.to_string_lossy().into_owned(),
                ),
                (
                    OVEN_NATIVE_TRACE_CXX_EXECUTABLE_ENV,
                    cxx_driver.to_string_lossy().into_owned(),
                ),
            ])
            .get(name)
            .cloned()
        })
        .ok_or("CXX alias did not select a driver")?;

        let status = run_native_command(&selected, &[marker.to_string_lossy().into_owned()])
            .map_err(|()| "declared C++ driver did not run")?;
        assert!(status.success());
        assert_eq!(std::fs::read_to_string(marker)?, "cxx");
        Ok(())
    }
}
