//! Store, limit and reporting helpers shared by the `incan oven` commands.
//!
//! Opening a store with the right defaults, resolving byte limits from the environment, and the small formatting
//! and error-wrapping helpers the command implementations reach for.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use std::fs;

use crate::{CliError, CliResult};
use oven_store::OvenReceipt;

pub(crate) use incan_driver::oven_store::{open_store, open_store_with_defaults, user_home};

/// Read and verify a persisted receipt before it authorizes another Oven stage.
pub(super) fn read_receipt(path: &Path) -> CliResult<OvenReceipt> {
    let bytes = fs::read(path)
        .map_err(|error| CliError::failure(format!("failed to read Oven receipt {}: {error}", path.display())))?;
    let receipt = serde_json::from_slice::<OvenReceipt>(&bytes)
        .map_err(|error| CliError::failure(format!("failed to parse Oven receipt {}: {error}", path.display())))?;
    receipt.verify_identity().map_err(oven_error)?;
    Ok(receipt)
}

/// Resolve the toolchain-manager state needed when a compiler self-test deliberately exercises Rustup fallback.
///
/// Stored normal commands receive a verified absolute `RUSTC`; this path exists solely because compiler tests also
/// verify Rustup discovery after removing that explicit variable. It is intentionally separate from Cargo state.
pub(super) fn default_rustup_home(rustup_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    rustup_home
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(path).join(".rustup"))
        })
}

/// Parse a named source argument with a portable digest key and a filesystem input path.
pub(super) fn parse_named_path(value: &str) -> CliResult<(String, PathBuf)> {
    let Some((name, path)) = value.split_once('=') else {
        return Err(CliError::failure(format!(
            "invalid Oven --source `{value}`; expected NAME=PATH"
        )));
    };
    let name = name.trim();
    let path = path.trim();
    if name.is_empty() || path.is_empty() {
        return Err(CliError::failure(format!(
            "invalid Oven --source `{value}`; expected NAME=PATH"
        )));
    }
    Ok((name.to_string(), PathBuf::from(path)))
}

/// Persist a complete scheduler aggregate beside caller-owned test outputs.
///
/// The terminal is intentionally a convenience surface and can be detached by a CI or desktop-session wrapper.
/// The report is therefore a normal caller-owned output, not an immutable-store artifact, and remains available for
/// a failed batch as well as a green batch. Atomic replacement prevents a reader from observing a partial summary.
pub(super) fn write_compiler_suite_report(path: &Path, report: &serde_json::Value) -> CliResult<()> {
    let parent = path.parent().ok_or_else(|| {
        CliError::failure(format!(
            "compiler-suite report path {} has no parent directory",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        CliError::failure(format!(
            "cannot create compiler-suite report directory {}: {error}",
            parent.display()
        ))
    })?;
    let encoded = serde_json::to_vec_pretty(report)
        .map_err(|error| CliError::failure(format!("failed to serialize compiler-suite report: {error}")))?;
    let temporary = parent.join(format!(".compiler-suite-report-{}.tmp", std::process::id()));
    fs::write(&temporary, encoded).map_err(|error| {
        CliError::failure(format!(
            "cannot write compiler-suite report temporary file {}: {error}",
            temporary.display()
        ))
    })?;
    fs::rename(&temporary, path).map_err(|error| {
        CliError::failure(format!(
            "cannot publish compiler-suite report {}: {error}",
            path.display()
        ))
    })
}

/// Serialize a stable JSON report or convert the failure into standard CLI error vocabulary.
pub(super) fn print_json(value: &impl serde::Serialize) -> CliResult<()> {
    let payload = serde_json::to_string_pretty(value)
        .map_err(|error| CliError::failure(format!("failed to serialize Oven JSON report: {error}")))?;
    println!("{payload}");
    Ok(())
}

/// Render binary byte units for physical allocation and logical artifact-byte accounting without Cargo-cache
/// terminology.
pub(super) fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Translate all Oven typed failures through the top-level CLI error boundary.
pub(super) fn oven_error(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

/// Convert a scheduler-selected path into an absolute environment value before a test changes directory.
///
/// Stored suite entries may be selected from a relative `--store` path, while compiler tests deliberately create
/// nested fixture directories. Forwarding those paths verbatim would make a receipt-authorized inventory or native
/// Loaf disappear from a nested `incan` process despite still being held by the parent suite lease.
fn compiler_suite_environment_path(path: &Path) -> CliResult<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|directory| directory.join(path))
        .map_err(|error| {
            CliError::failure(format!(
                "cannot resolve compiler-suite environment path {} from the current directory: {error}",
                path.display()
            ))
        })
}

/// Export one checked direct-Rustc closure under its existing consumer capability names.
///
/// The generated-code warning checker and the compiler CLI can legitimately use different dependency closures. Keep
/// them separate instead of merging same-named Rust crates from independent receipt-bound foundations into an
/// ambiguous `--extern` list.
pub(crate) fn append_compiler_suite_direct_rustc_environment(
    environment: &mut BTreeMap<String, String>,
    variables: &[&str],
    rustc: &Path,
    artifacts: &oven_rustc::rustc::OvenRustcArtifactPlan,
) -> CliResult<()> {
    let capability = oven_store::compiler_suite_env::OvenCompilerSuiteCapability::new(
        compiler_suite_environment_path(rustc)?,
        artifacts
            .dependency_search_paths
            .iter()
            .map(|path| compiler_suite_environment_path(path))
            .collect::<CliResult<Vec<_>>>()?,
        artifacts
            .externs
            .iter()
            .map(|(crate_name, path)| Ok((crate_name.clone(), compiler_suite_environment_path(path)?)))
            .collect::<CliResult<BTreeMap<_, _>>>()?,
    );
    let encoded = capability.encode().map_err(CliError::failure)?;
    for variable in variables {
        environment.insert((*variable).to_string(), encoded.clone());
    }
    Ok(())
}

