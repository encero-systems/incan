//! Stable physical rustc-invocation capture for the explicit Cargo compatibility publisher.

use std::collections::BTreeMap;
use std::env;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::OvenLegacyCargoError;

/// Trace file selected by the explicit compatibility publisher for its wrapper subprocesses.
pub const OVEN_RUSTC_TRACE_PATH_ENV: &str = "INCAN_OVEN_RUSTC_TRACE_PATH";
/// Marker that makes the `incan` or `oven` executable dispatch the rustc wrapper protocol before CLI parsing.
pub const OVEN_RUSTC_TRACE_WRAPPER_ENV: &str = "INCAN_OVEN_RUSTC_TRACE_WRAPPER";
const MAX_RUSTC_TRACE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RUSTC_TRACE_RECORD_BYTES: usize = 1024 * 1024;
const MAX_RUSTC_TRACE_RECORDS: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyRustcInvocation {
    /// Stable discriminator used when the record is mixed into Cargo's JSON message stream.
    pub reason: String,
    /// Exact compiler executable invoked by Cargo, later checked against the publisher request.
    pub rustc: String,
    /// Ordered rustc arguments, excluding the compiler executable itself.
    pub arguments: Vec<String>,
    /// Allowlisted Cargo compilation context needed to bind the invocation to a package and physical variant.
    pub environment: BTreeMap<String, String>,
}

/// Run the current executable as Cargo's stable `RUSTC_WRAPPER`, returning `None` for every ordinary invocation.
pub fn run_legacy_rustc_trace_wrapper() -> Option<i32> {
    if env::var_os(OVEN_RUSTC_TRACE_WRAPPER_ENV).as_deref() != Some(std::ffi::OsStr::new("1")) {
        return None;
    }
    Some(run_marked_rustc_trace_wrapper().unwrap_or(1))
}

fn run_marked_rustc_trace_wrapper() -> Result<i32, ()> {
    let trace = env::var_os(OVEN_RUSTC_TRACE_PATH_ENV).map(PathBuf::from).ok_or(())?;
    let mut arguments = env::args_os().skip(1);
    let rustc = arguments.next().ok_or(())?;
    let rustc = rustc.into_string().map_err(|_| ())?;
    let arguments = arguments
        .map(|argument| argument.into_string())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ())?;
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
    let status = Command::new(&rustc).args(&arguments).status().map_err(|_| ())?;
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
    let record = OvenLegacyRustcInvocation {
        reason: "incan-rustc-invocation".to_string(),
        rustc: rustc.clone(),
        arguments: arguments.clone(),
        environment,
    };
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
            arguments: vec!["--crate-name".to_string(), "fixture".to_string()],
            environment: BTreeMap::new(),
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
}
