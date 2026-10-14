//! Typed transport of explicitly declared codegen options into receipt-bound native execution.

use serde::{Deserialize, Serialize};

use super::OvenRustcError;

/// Codegen arguments supplied by the owning Incan policy, never inferred from a dependency name or ambient flags.
///
/// The seed transport is removable #1698 debt until Incan owns adopted-unit orchestration. The execution adapter
/// still requires typed, receipt-bound options; it does not select which units or profiles receive them.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OvenRustcCodegenOptions {
    /// Exact rustc optimization level.
    pub opt_level: String,
    /// Whether rustc enables development assertions and its matching conditional compilation flag.
    pub debug_assertions: bool,
    /// Whether integer arithmetic retains overflow checks.
    pub overflow_checks: bool,
}

impl OvenRustcCodegenOptions {
    /// Refuse values rustc cannot interpret before selecting an output or starting compilation.
    pub fn validate(&self) -> Result<(), OvenRustcError> {
        if !matches!(self.opt_level.as_str(), "0" | "1" | "2" | "3" | "s" | "z") {
            return Err(OvenRustcError::InvalidInput {
                field: "SDK codegen options",
                message: "unsupported optimization level".to_string(),
            });
        }
        Ok(())
    }

    /// Apply the declared options after named-profile defaults; the exact declaration is already in the receipt.
    pub(super) fn apply(&self, command: &mut std::process::Command) -> Result<(), OvenRustcError> {
        self.validate()?;
        command.args(["-C", &format!("opt-level={}", self.opt_level)]);
        command.args([
            "-C",
            if self.debug_assertions {
                "debug-assertions=on"
            } else {
                "debug-assertions=off"
            },
            "-C",
            if self.overflow_checks {
                "overflow-checks=on"
            } else {
                "overflow-checks=off"
            },
        ]);
        Ok(())
    }
}
