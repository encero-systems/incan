//! Opening the bounded Oven store every project command shares: the versioned root below `INCAN_HOME` or the user
//! home, and the byte limits the command line and the environment select over the product defaults, so `build`,
//! `test`, the explicit bake and the inspection commands all read and publish the same entries.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use crate::driver::error::{CliError, CliResult};
use crate::oven::store::{OvenStore, OvenStoreLimits};
use crate::oven::{
    DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES, DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES, DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
};

/// Environment override for aggregate physical allocation policy.
pub const OVEN_MAX_PHYSICAL_BYTES_ENV: &str = "INCAN_OVEN_MAX_PHYSICAL_BYTES";
/// Environment override for per-domain physical allocation policy.
pub const OVEN_MAX_DOMAIN_PHYSICAL_BYTES_ENV: &str = "INCAN_OVEN_MAX_DOMAIN_PHYSICAL_BYTES";
/// Environment override for per-domain logical artifact-byte policy.
pub const OVEN_MAX_DOMAIN_LOGICAL_BYTES_ENV: &str = "INCAN_OVEN_MAX_DOMAIN_LOGICAL_BYTES";

/// Shared bounded-store location and policy inputs for Oven Alpha commands.
#[derive(Debug, Clone)]
pub struct OvenStoreCommandOptions {
    /// Optional explicit store root; the versioned `INCAN_HOME`/home default is used otherwise.
    pub root: Option<PathBuf>,
    /// Optional aggregate physical allocation cap in bytes.
    pub max_physical_bytes: Option<u64>,
    /// Optional per-domain physical allocation cap in bytes.
    pub max_domain_physical_bytes: Option<u64>,
    /// Optional per-domain logical artifact-byte cap in bytes.
    pub max_domain_logical_bytes: Option<u64>,
}

impl OvenStoreCommandOptions {
    /// Whether a command will resolve the ordinary compiler-owned Oven store without caller-specific policy.
    pub(crate) fn is_ordinary_default(&self) -> bool {
        self.root.is_none()
            && self.max_physical_bytes.is_none()
            && self.max_domain_physical_bytes.is_none()
            && self.max_domain_logical_bytes.is_none()
    }
}

/// Resolve the one compiler-owned default store root or a caller-explicit root without consulting Cargo state.
pub(crate) fn open_store(options: &OvenStoreCommandOptions) -> CliResult<OvenStore> {
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
pub(crate) fn open_store_with_defaults(
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
pub(crate) fn resolve_limits_with_defaults(
    options: &OvenStoreCommandOptions,
    defaults: OvenStoreLimits,
) -> CliResult<OvenStoreLimits> {
    resolve_limits_with_environment_and_defaults(options, |name| env::var(name).ok(), defaults)
}

/// Apply CLI and environment overrides over one explicit product-owned default profile.
pub(crate) fn resolve_limits_with_environment_and_defaults(
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
pub(crate) fn parse_limit_value(name: &str, value: Option<String>, default: u64) -> CliResult<u64> {
    match value {
        Some(value) if !value.trim().is_empty() => value
            .trim()
            .parse::<u64>()
            .map_err(|error| CliError::failure(format!("invalid {name} value `{value}`; expected bytes: {error}"))),
        Some(_) | None => Ok(default),
    }
}

/// Resolve the versioned Oven store location below `INCAN_HOME` before the user home directory.
pub(crate) fn default_store_root(incan_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
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
pub(crate) fn user_home() -> Option<OsString> {
    env::var_os("HOME").or_else(|| env::var_os("USERPROFILE"))
}
