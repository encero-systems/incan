//! Lock strictness for one command: whether it may change the lock, refresh it, or reach the network.
//!
//! The policy is decided once per command from CLI flags over `INCAN_OFFLINE`, `INCAN_LOCKED` and `INCAN_FROZEN`
//! defaults, so `build`, `lock`, `run` and `test` cannot drift.

use std::env;

/// Lock strictness resolved from CLI inputs and environment defaults. `frozen` implies `locked` and `offline`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LockPolicy {
    /// Never reach the network for resolution.
    pub offline: bool,
    /// Refuse a lock that would have to change.
    pub locked: bool,
    /// Both of the above.
    pub frozen: bool,
}

/// The CLI's enable/disable flag pairs; a disable flag overrides an environment default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LockPolicyCliFlags {
    pub offline: bool,
    pub no_offline: bool,
    pub locked: bool,
    pub no_locked: bool,
    pub frozen: bool,
    pub no_frozen: bool,
}

impl LockPolicy {
    /// Resolve the policy for a user-facing command from its flags and the process environment.
    pub fn from_cli_and_env(cli_flags: LockPolicyCliFlags) -> Self {
        Self::from_sources(cli_flags, |name| env::var(name).ok())
    }

    /// Build an explicit policy that reads no environment defaults.
    pub fn explicit(offline: bool, locked: bool, frozen: bool) -> Self {
        Self {
            offline,
            locked,
            frozen,
        }
        .normalized()
    }

    /// Whether the command must refuse to change the lock.
    pub fn strict(&self) -> bool {
        self.locked || self.frozen
    }

    /// Resolve the policy from injected sources; tests use this to avoid mutating the process environment.
    fn from_sources(cli_flags: LockPolicyCliFlags, env_value: impl Fn(&str) -> Option<String>) -> Self {
        let env_flag = |name: &str| env_flag_value(env_value(name).as_deref());
        Self::explicit(
            resolve_cli_env_flag(env_flag("INCAN_OFFLINE"), cli_flags.offline, cli_flags.no_offline),
            resolve_cli_env_flag(env_flag("INCAN_LOCKED"), cli_flags.locked, cli_flags.no_locked),
            resolve_cli_env_flag(env_flag("INCAN_FROZEN"), cli_flags.frozen, cli_flags.no_frozen),
        )
    }

    /// Apply `frozen`'s implication after the raw inputs are resolved.
    fn normalized(mut self) -> Self {
        if self.frozen {
            self.offline = true;
            self.locked = true;
        }
        self
    }
}

/// Resolve one boolean input: a CLI enable or disable flag wins over the environment default.
fn resolve_cli_env_flag(env_default: bool, cli_enable: bool, cli_disable: bool) -> bool {
    if cli_enable {
        true
    } else if cli_disable {
        false
    } else {
        env_default
    }
}

/// Parse a boolean environment flag value.
fn env_flag_value(value: Option<&str>) -> bool {
    value.is_some_and(|value| matches!(value, "1" | "true" | "TRUE" | "on" | "ON"))
}

#[cfg(test)]
mod tests {
    use super::{LockPolicy, LockPolicyCliFlags};

    #[test]
    fn frozen_from_the_environment_implies_locked_and_offline() {
        let policy = LockPolicy::from_sources(LockPolicyCliFlags::default(), |name| {
            (name == "INCAN_FROZEN").then(|| "1".to_string())
        });
        assert_eq!(policy, LockPolicy::explicit(true, true, true));
        assert!(policy.strict());
    }

    #[test]
    fn a_cli_disable_flag_overrides_an_environment_default() {
        let policy = LockPolicy::from_sources(
            LockPolicyCliFlags {
                no_locked: true,
                ..LockPolicyCliFlags::default()
            },
            |name| (name == "INCAN_LOCKED").then(|| "1".to_string()),
        );
        assert!(!policy.locked);
        assert!(!policy.strict());
    }
}
