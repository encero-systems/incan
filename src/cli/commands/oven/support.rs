//! Store, limit and reporting helpers shared by the `incan oven` commands.
//!
//! Opening a store with the right defaults, resolving byte limits from the environment, and the small formatting
//! and error-wrapping helpers the command implementations reach for.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use std::env;
use std::fs;

use crate::cli::{CliError, CliResult};
use crate::oven::store::{OvenStore, OvenStoreLimits};
use crate::oven::{
    DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES, DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES, DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
    OvenReceipt,
};

use super::options::OvenStoreCommandOptions;
use super::{OVEN_MAX_DOMAIN_LOGICAL_BYTES_ENV, OVEN_MAX_DOMAIN_PHYSICAL_BYTES_ENV, OVEN_MAX_PHYSICAL_BYTES_ENV};

/// Read and verify a persisted receipt before it authorizes another Oven stage.
pub(super) fn read_receipt(path: &Path) -> CliResult<OvenReceipt> {
    let bytes = fs::read(path)
        .map_err(|error| CliError::failure(format!("failed to read Oven receipt {}: {error}", path.display())))?;
    let receipt = serde_json::from_slice::<OvenReceipt>(&bytes)
        .map_err(|error| CliError::failure(format!("failed to parse Oven receipt {}: {error}", path.display())))?;
    receipt.verify_identity().map_err(oven_error)?;
    Ok(receipt)
}

/// Resolve the one compiler-owned default store root or a caller-explicit root without consulting Cargo state.
pub(super) fn open_store(options: &OvenStoreCommandOptions) -> CliResult<OvenStore> {
    open_store_with_defaults(
        options,
        OvenStoreLimits::new(
            DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
            DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES,
            DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES,
        ),
    )
}

/// Open one bounded store using the product profile owned by its command surface.
pub(super) fn open_store_with_defaults(
    options: &OvenStoreCommandOptions,
    defaults: OvenStoreLimits,
) -> CliResult<OvenStore> {
    let root = match &options.root {
        Some(root) => root.clone(),
        None => default_store_root(env::var_os("INCAN_HOME"), user_home()).ok_or_else(|| {
            CliError::failure("cannot resolve the Oven store root; set INCAN_HOME, HOME, or pass --store")
        })?,
    };
    Ok(OvenStore::new(root, resolve_limits_with_defaults(options, defaults)?))
}

/// Open the one policy-bounded Oven store used by ordinary Alpha commands.
///
/// This keeps normal `build`, `run`, and `test` on the same receipt-owned store as the explicit inspection commands;
/// normal execution never accepts a generated-Cargo target directory as a storage selector.
pub(crate) fn open_default_oven_store() -> CliResult<OvenStore> {
    open_store(&OvenStoreCommandOptions {
        root: None,
        max_physical_bytes: None,
        max_domain_physical_bytes: None,
        max_domain_logical_bytes: None,
    })
}

/// Resolve bounded policy with one command-owned product profile and the real process environment.
pub(super) fn resolve_limits_with_defaults(
    options: &OvenStoreCommandOptions,
    defaults: OvenStoreLimits,
) -> CliResult<OvenStoreLimits> {
    resolve_limits_with_environment_and_defaults(options, |name| env::var(name).ok(), defaults)
}

/// Apply CLI and environment overrides over one explicit product-owned default profile.
pub(super) fn resolve_limits_with_environment_and_defaults(
    options: &OvenStoreCommandOptions,
    environment_value: impl Fn(&str) -> Option<String>,
    defaults: OvenStoreLimits,
) -> CliResult<OvenStoreLimits> {
    let aggregate = match options.max_physical_bytes {
        Some(value) => value,
        None => parse_limit_value(
            OVEN_MAX_PHYSICAL_BYTES_ENV,
            environment_value(OVEN_MAX_PHYSICAL_BYTES_ENV),
            defaults.max_physical_bytes,
        )?,
    };
    let environment_domain_physical = environment_value(OVEN_MAX_DOMAIN_PHYSICAL_BYTES_ENV);
    let domain_physical_was_explicit = options.max_domain_physical_bytes.is_some()
        || environment_domain_physical
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
    let mut domain_physical = match options.max_domain_physical_bytes {
        Some(value) => value,
        None => parse_limit_value(
            OVEN_MAX_DOMAIN_PHYSICAL_BYTES_ENV,
            environment_domain_physical,
            defaults.max_domain_physical_bytes,
        )?,
    };
    let domain_logical = match options.max_domain_logical_bytes {
        Some(value) => value,
        None => parse_limit_value(
            OVEN_MAX_DOMAIN_LOGICAL_BYTES_ENV,
            environment_value(OVEN_MAX_DOMAIN_LOGICAL_BYTES_ENV),
            defaults.max_domain_logical_bytes,
        )?,
    };
    if aggregate == 0 || domain_physical == 0 || domain_logical == 0 {
        return Err(CliError::failure(
            "Oven storage policy limits must be greater than zero",
        ));
    }
    if domain_physical > aggregate {
        if domain_physical_was_explicit {
            return Err(CliError::failure(
                "Oven per-domain physical policy must not exceed aggregate physical policy",
            ));
        }
        domain_physical = aggregate;
    }
    Ok(OvenStoreLimits::new(aggregate, domain_physical, domain_logical))
}

/// Parse one explicit byte-count environment variable without accepting ambiguous unit suffixes.
pub(super) fn parse_limit_value(name: &str, value: Option<String>, default: u64) -> CliResult<u64> {
    match value {
        Some(value) if !value.trim().is_empty() => value
            .trim()
            .parse::<u64>()
            .map_err(|error| CliError::failure(format!("invalid {name} value `{value}`; expected bytes: {error}"))),
        Some(_) | None => Ok(default),
    }
}

/// Resolve the versioned Oven store location below `INCAN_HOME` before the user home directory.
pub(super) fn default_store_root(incan_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    incan_home
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(path).join(".incan"))
        })
        .map(|root| crate::oven::store::store_root_for_home(&root))
}

/// Return the platform home environment used by installed Incan binaries.
pub(super) fn user_home() -> Option<OsString> {
    env::var_os("HOME").or_else(|| env::var_os("USERPROFILE"))
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
