//! Stable physical rustc-invocation capture for the explicit Cargo compatibility publisher.

use std::collections::BTreeMap;
use std::env;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::OvenLegacyCargoError;

pub const OVEN_RUSTC_TRACE_PATH_ENV: &str = "INCAN_OVEN_RUSTC_TRACE_PATH";
pub const OVEN_RUSTC_TRACE_WRAPPER_ENV: &str = "INCAN_OVEN_RUSTC_TRACE_WRAPPER";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyRustcInvocation {
    pub reason: String,
    pub rustc: String,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
}

/// Run the current executable as Cargo's stable `RUSTC_WRAPPER`, returning `None` for every ordinary invocation.
pub fn run_legacy_rustc_trace_wrapper() -> Option<i32> {
    if env::var_os(OVEN_RUSTC_TRACE_WRAPPER_ENV).as_deref() != Some(std::ffi::OsStr::new("1")) {
        return None;
    }
    let trace = env::var_os(OVEN_RUSTC_TRACE_PATH_ENV).map(PathBuf::from)?;
    let mut arguments = env::args_os().skip(1);
    let rustc = arguments.next()?;
    let rustc = rustc.into_string().ok()?;
    let arguments = arguments
        .map(|argument| argument.into_string())
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let environment = env::vars()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "CARGO_CRATE_NAME"
                    | "CARGO_MANIFEST_DIR"
                    | "CARGO_PKG_NAME"
                    | "CARGO_PKG_VERSION"
                    | "CARGO_PRIMARY_PACKAGE"
                    | "HOST"
                    | "OUT_DIR"
                    | "PROFILE"
                    | "TARGET"
            ) || name.starts_with("CARGO_FEATURE_")
        })
        .collect();
    let status = Command::new(&rustc).args(&arguments).status().ok()?;
    let exit_code = status.code().unwrap_or(1);
    if !status.success()
        || arguments
            .iter()
            .any(|argument| argument == "-vV" || argument == "--version")
        || arguments
            .iter()
            .any(|argument| argument == "--print" || argument.starts_with("--print="))
    {
        return Some(exit_code);
    }
    let record = OvenLegacyRustcInvocation {
        reason: "incan-rustc-invocation".to_string(),
        rustc: rustc.clone(),
        arguments: arguments.clone(),
        environment,
    };
    let encoded = serde_json::to_vec(&record).ok()?;
    let mut file = OpenOptions::new().create(true).append(true).open(trace).ok()?;
    File::lock(&file).ok()?;
    let written = file.write_all(&encoded).and_then(|_| file.write_all(b"\n"));
    let _ = File::unlock(&file);
    if written.is_err() {
        return Some(1);
    }
    Some(exit_code)
}

pub(crate) fn append_rustc_trace(stdout: &mut Vec<u8>, trace: &Path) -> Result<(), OvenLegacyCargoError> {
    let bytes = match std::fs::read(trace) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
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
    for line in bytes.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()) {
        serde_json::from_slice::<OvenLegacyRustcInvocation>(line).map_err(|error| {
            OvenLegacyCargoError::Plan(format!("stable rustc invocation trace is invalid: {error}"))
        })?;
        stdout.extend_from_slice(line);
        stdout.push(b'\n');
    }
    Ok(())
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
