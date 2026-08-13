//! Typed process-boundary capability used by stored Oven compiler-suite children.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Environment variable carrying the generated-code warning-check closure.
pub(crate) const OVEN_COMPILER_SUITE_CAPABILITY_ENV: &str = "INCAN_OVEN_COMPILER_SUITE_CAPABILITY";
/// Environment variable carrying the separate vocabulary-companion closure.
pub(crate) const OVEN_COMPILER_SUITE_VOCAB_CAPABILITY_ENV: &str = "INCAN_OVEN_COMPILER_SUITE_VOCAB_CAPABILITY";
/// Stable activation/compiler marker retained for narrow test helpers that need only the selected Rustc path.
pub(crate) const OVEN_COMPILER_SUITE_RUSTC_ENV: &str = "INCAN_OVEN_COMPILER_SUITE_RUSTC";
/// Exact Cargo executable admitted only to roots whose tests exercise Cargo compatibility.
pub(crate) const OVEN_COMPILER_SUITE_FIXTURE_CARGO_REAL_ENV: &str = "INCAN_INTERNAL_OVEN_FIXTURE_CARGO_REAL";
/// Matching Rustc executable used only by the admitted nightly Cargo child.
pub(crate) const OVEN_COMPILER_SUITE_FIXTURE_RUSTC_REAL_ENV: &str = "INCAN_INTERNAL_OVEN_FIXTURE_RUSTC_REAL";
/// Invocation log written by the compiler-suite-owned fixture Cargo proxy.
pub(crate) const OVEN_COMPILER_SUITE_FIXTURE_CARGO_LOG_ENV: &str = "INCAN_INTERNAL_OVEN_FIXTURE_CARGO_LOG";

const OVEN_COMPILER_SUITE_CAPABILITY_SCHEMA_VERSION: u32 = 1;
const MAX_COMPILER_SUITE_DIRECT_RUSTC_INPUTS: usize = 1024;

/// Explicit process capabilities for one stored compiler-suite root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OvenCompilerSuiteTargetCapabilities {
    pub(crate) generated_rust_closure: bool,
    pub(crate) legacy_cargo_fixture: bool,
}

impl OvenCompilerSuiteTargetCapabilities {
    /// Resolve the narrow, package-qualified capability registry for one receipt-bound root.
    pub(crate) fn for_target(package_name: &str, target_kind: &str, source_relative_path: &str) -> Self {
        let generated_rust_closure = source_relative_path != "tests/toolchain_installer_tests.rs";
        let legacy_cargo_fixture = matches!(
            (package_name, target_kind, source_relative_path),
            ("incan", "lib", "src/lib.rs") | ("rust_inspect", "lib", "crates/rust_inspect/src/lib.rs")
        );
        Self {
            generated_rust_closure,
            legacy_cargo_fixture,
        }
    }
}

/// One complete, receipt-selected direct-Rustc closure exported to a suite child.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenCompilerSuiteCapability {
    schema_version: u32,
    pub(crate) rustc: PathBuf,
    pub(crate) dependency_search_paths: Vec<PathBuf>,
    pub(crate) externs: BTreeMap<String, PathBuf>,
}

impl OvenCompilerSuiteCapability {
    /// Construct the complete typed direct-Rustc closure selected for one suite child.
    pub(crate) fn new(
        rustc: PathBuf,
        dependency_search_paths: Vec<PathBuf>,
        externs: BTreeMap<String, PathBuf>,
    ) -> Self {
        Self {
            schema_version: OVEN_COMPILER_SUITE_CAPABILITY_SCHEMA_VERSION,
            rustc,
            dependency_search_paths,
            externs,
        }
    }

    /// Encode one capability for a child process without indexed environment-key conventions.
    pub(crate) fn encode(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| format!("could not encode compiler-suite capability: {error}"))
    }

    /// Decode one complete capability and reject schema drift before any consumer uses a partial closure.
    pub(crate) fn decode(payload: &str) -> Result<Self, String> {
        let capability = serde_json::from_str::<Self>(payload)
            .map_err(|error| format!("invalid compiler-suite capability: {error}"))?;
        if capability.schema_version != OVEN_COMPILER_SUITE_CAPABILITY_SCHEMA_VERSION {
            return Err(format!(
                "unsupported compiler-suite capability schema {}",
                capability.schema_version
            ));
        }
        if capability.dependency_search_paths.len() > MAX_COMPILER_SUITE_DIRECT_RUSTC_INPUTS
            || capability.externs.len() > MAX_COMPILER_SUITE_DIRECT_RUSTC_INPUTS
        {
            return Err(format!(
                "compiler-suite capability exceeds the {MAX_COMPILER_SUITE_DIRECT_RUSTC_INPUTS}-input limit"
            ));
        }
        Ok(capability)
    }

    /// Read one optional typed capability from the current child environment.
    pub(crate) fn from_environment(name: &str) -> Result<Option<Self>, String> {
        let Some(payload) = std::env::var_os(name).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        Self::decode(&payload.to_string_lossy()).map(Some)
    }
}
