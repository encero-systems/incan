//! Cargo policy for the compatibility path: which Cargo controls a command admits, from CLI flags and environment,
//! and the flag vector they become.
//!
//! Oven does not take these; the Cargo-compatibility baker does. The policy is decided here, once per command,
//! and rendered to flags in one place so `build`, `lock`, and `test` cannot drift.

use std::env;

use crate::driver::error::{CliError, CliResult};
use crate::lockfile::CargoFeatureSelection;
use crate::manifest::ProjectManifest;
use crate::project_lifecycle::toolchain::ToolchainConstraintSet;
/// Cargo execution policy resolved from CLI inputs and environment defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CargoPolicy {
    pub(crate) offline: bool,
    pub(crate) locked: bool,
    pub(crate) frozen: bool,
    pub(crate) extra_args: Vec<String>,
}

/// CLI policy flags, including explicit disables for environment defaults.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CargoPolicyCliFlags {
    pub offline: bool,
    pub no_offline: bool,
    pub locked: bool,
    pub no_locked: bool,
    pub frozen: bool,
    pub no_frozen: bool,
}

impl CargoPolicy {
    /// Resolve policy for a user-facing build/run/test command.
    pub(crate) fn from_cli_and_env(
        cli_flags: CargoPolicyCliFlags,
        cli_cargo_args: Vec<String>,
        cli_passthrough_args: Vec<String>,
    ) -> Self {
        Self::from_sources(cli_flags, cli_cargo_args, cli_passthrough_args, |name| {
            env::var(name).ok()
        })
    }

    /// Build an explicit policy for internal Cargo invocations that should not read RFC 020 env defaults.
    pub(crate) fn explicit(offline: bool, locked: bool, frozen: bool, extra_args: Vec<String>) -> Self {
        let mut policy = Self {
            offline,
            locked,
            frozen,
            extra_args,
        };
        policy.normalize();
        policy
    }

    /// Resolve policy from injected sources; used by tests to avoid mutating process env.
    fn from_sources<F>(
        cli_flags: CargoPolicyCliFlags,
        mut cli_cargo_args: Vec<String>,
        cli_passthrough_args: Vec<String>,
        env_value: F,
    ) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let env_frozen = env_flag_value(env_value("INCAN_FROZEN").as_deref());
        let env_offline = env_flag_value(env_value("INCAN_OFFLINE").as_deref());
        let env_locked = env_flag_value(env_value("INCAN_LOCKED").as_deref());

        cli_cargo_args.extend(cli_passthrough_args);
        let extra_args = if cli_cargo_args.is_empty() {
            split_env_cargo_args(env_value("INCAN_CARGO_ARGS").as_deref())
        } else {
            cli_cargo_args
        };

        Self::explicit(
            resolve_cli_env_flag(env_offline, cli_flags.offline, cli_flags.no_offline),
            resolve_cli_env_flag(env_locked, cli_flags.locked, cli_flags.no_locked),
            resolve_cli_env_flag(env_frozen, cli_flags.frozen, cli_flags.no_frozen),
            extra_args,
        )
    }

    /// Apply derived policy semantics after raw source resolution.
    fn normalize(&mut self) {
        if self.frozen {
            self.offline = true;
            self.locked = true;
        }
    }
}

/// Enforce the project-level `requires-incan` constraint for a project-aware command.
pub(crate) fn enforce_project_toolchain_constraint(manifest: &ProjectManifest) -> CliResult<()> {
    enforce_toolchain_constraints(&ToolchainConstraintSet::from_project_manifest(manifest))
}

/// Enforce an already-resolved effective `requires-incan` constraint set.
pub(crate) fn enforce_toolchain_constraints(constraints: &ToolchainConstraintSet) -> CliResult<()> {
    constraints
        .enforce(crate::version::INCAN_VERSION)
        .map_err(|error| CliError::failure(error.to_string()))
}

/// Resolve one boolean policy input with CLI enable/disable flags over env defaults.
fn resolve_cli_env_flag(env_default: bool, cli_enable: bool, cli_disable: bool) -> bool {
    if cli_enable {
        true
    } else if cli_disable {
        false
    } else {
        env_default
    }
}

/// Parse a boolean RFC 020 environment flag value.
fn env_flag_value(value: Option<&str>) -> bool {
    value.is_some_and(|value| matches!(value, "1" | "true" | "TRUE" | "on" | "ON"))
}

/// Split `INCAN_CARGO_ARGS` using the RFC 020 whitespace-only rule.
fn split_env_cargo_args(value: Option<&str>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(str::split_whitespace)
        .map(str::to_string)
        .collect()
}

/// Build Cargo policy flags (`--offline` / `--locked` / `--frozen`).
pub(crate) fn cargo_policy_flags(policy: &CargoPolicy) -> Vec<String> {
    if policy.frozen {
        return vec!["--frozen".to_string()];
    }

    let mut flags = Vec::new();
    if policy.offline {
        flags.push("--offline".to_string());
    }
    if policy.locked {
        flags.push("--locked".to_string());
    }
    flags
}

/// Build Cargo feature-selection flags without policy or arbitrary extra args.
fn cargo_feature_flags(cargo_features: &CargoFeatureSelection) -> Vec<String> {
    let mut flags = Vec::new();
    if cargo_features.cargo_all_features {
        flags.push("--all-features".to_string());
    }
    if cargo_features.cargo_no_default_features {
        flags.push("--no-default-features".to_string());
    }
    if !cargo_features.cargo_features.is_empty() {
        flags.push("--features".to_string());
        flags.push(cargo_features.cargo_features.join(","));
    }
    flags
}

/// Build flags for lockfile-oriented Cargo commands.
pub(crate) fn cargo_lockfile_flags(policy: &CargoPolicy, cargo_features: &CargoFeatureSelection) -> Vec<String> {
    let mut flags = cargo_policy_flags(policy);
    flags.extend(cargo_feature_flags(cargo_features));
    flags
}

/// Build Cargo command flags (policy flags + feature flags + extra Cargo args).
pub(crate) fn cargo_command_flags(policy: &CargoPolicy, cargo_features: &CargoFeatureSelection) -> Vec<String> {
    let mut flags = cargo_lockfile_flags(policy, cargo_features);
    flags.extend(policy.extra_args.clone());
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_policy_resolves_env_defaults_and_frozen_implication() {
        let policy = CargoPolicy::from_sources(
            CargoPolicyCliFlags::default(),
            Vec::new(),
            Vec::new(),
            |name| match name {
                "INCAN_FROZEN" => Some("1".to_string()),
                "INCAN_CARGO_ARGS" => Some("--timings --verbose".to_string()),
                _ => None,
            },
        );

        assert!(policy.frozen);
        assert!(policy.offline);
        assert!(policy.locked);
        assert_eq!(policy.extra_args, vec!["--timings", "--verbose"]);
    }

    #[test]
    fn cargo_policy_uses_cli_extra_args_before_env_extra_args() {
        let policy = CargoPolicy::from_sources(
            CargoPolicyCliFlags {
                offline: true,
                ..CargoPolicyCliFlags::default()
            },
            vec!["--features".to_string(), "cli".to_string()],
            vec!["--no-default-features".to_string()],
            |name| match name {
                "INCAN_CARGO_ARGS" => Some("--features env".to_string()),
                _ => None,
            },
        );

        assert!(policy.offline);
        assert_eq!(policy.extra_args, vec!["--features", "cli", "--no-default-features"]);
    }

    #[test]
    fn cargo_policy_cli_disable_flags_override_env_defaults() {
        let policy = CargoPolicy::from_sources(
            CargoPolicyCliFlags {
                no_offline: true,
                no_locked: true,
                no_frozen: true,
                ..CargoPolicyCliFlags::default()
            },
            Vec::new(),
            Vec::new(),
            |name| match name {
                "INCAN_OFFLINE" | "INCAN_LOCKED" | "INCAN_FROZEN" => Some("1".to_string()),
                _ => None,
            },
        );

        assert!(!policy.offline);
        assert!(!policy.locked);
        assert!(!policy.frozen);
    }

    #[test]
    fn cargo_command_flags_order_policy_features_then_extra_args() {
        let policy = CargoPolicy::explicit(
            true,
            true,
            false,
            vec!["--timings".to_string(), "--color=always".to_string()],
        );
        let features = CargoFeatureSelection {
            cargo_features: vec!["json".to_string(), "web".to_string()],
            cargo_no_default_features: true,
            cargo_all_features: false,
        };

        assert_eq!(
            cargo_command_flags(&policy, &features),
            vec![
                "--offline",
                "--locked",
                "--no-default-features",
                "--features",
                "json,web",
                "--timings",
                "--color=always"
            ]
        );
    }
}
