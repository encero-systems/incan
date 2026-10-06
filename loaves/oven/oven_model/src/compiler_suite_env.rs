//! Typed process-boundary capability used by stored Oven compiler-suite children.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Environment variable carrying the generated-code warning-check closure.
pub const OVEN_COMPILER_SUITE_CAPABILITY_ENV: &str = "INCAN_OVEN_COMPILER_SUITE_CAPABILITY";
/// Environment variable carrying the separate vocabulary-companion closure.
pub const OVEN_COMPILER_SUITE_VOCAB_CAPABILITY_ENV: &str = "INCAN_OVEN_COMPILER_SUITE_VOCAB_CAPABILITY";
/// Exact workspace variants compiled by the suite for a nested Rust-unit bake.
pub const OVEN_COMPILER_SUITE_RUST_UNIT_CAPABILITY_ENV: &str = "INCAN_OVEN_COMPILER_SUITE_RUST_UNIT_CAPABILITY";
/// Stable activation/compiler marker retained for narrow test helpers that need only the selected Rustc path.
pub const OVEN_COMPILER_SUITE_RUSTC_ENV: &str = "INCAN_OVEN_COMPILER_SUITE_RUSTC";
/// Exact Cargo executable admitted only to roots whose tests exercise Cargo compatibility.
pub const OVEN_COMPILER_SUITE_FIXTURE_CARGO_REAL_ENV: &str = "INCAN_INTERNAL_OVEN_FIXTURE_CARGO_REAL";
/// Matching Rustc executable used only by the admitted nightly Cargo child.
pub const OVEN_COMPILER_SUITE_FIXTURE_RUSTC_REAL_ENV: &str = "INCAN_INTERNAL_OVEN_FIXTURE_RUSTC_REAL";
/// Invocation log written by the compiler-suite-owned fixture Cargo proxy.
pub const OVEN_COMPILER_SUITE_FIXTURE_CARGO_LOG_ENV: &str = "INCAN_INTERNAL_OVEN_FIXTURE_CARGO_LOG";
/// Exact Cargo executable exposed only to a test helper performing `incan oven bake`.
pub const OVEN_COMPILER_SUITE_EXPLICIT_BAKE_CARGO_ENV: &str = "INCAN_INTERNAL_OVEN_EXPLICIT_BAKE_CARGO";
/// Offline Cargo-home authority exposed only to a test helper performing `incan oven bake`.
pub const OVEN_COMPILER_SUITE_EXPLICIT_BAKE_HOME_ENV: &str = "INCAN_INTERNAL_OVEN_EXPLICIT_BAKE_HOME";
/// Checkout-owned directory, kept across suite runs, where one explicit-bake root reuses its baked fixture graph.
pub const OVEN_COMPILER_SUITE_EXPLICIT_BAKE_WORKSPACE_ENV: &str = "INCAN_INTERNAL_OVEN_EXPLICIT_BAKE_WORKSPACE";

/// CLI test roots whose explicit bakes require the publisher's offline Cargo authority.
const EXPLICIT_CLI_BAKE_ROOTS: &[&str] = &[
    "loaves/toolchain/incan-cli/tests/cli_decorator_and_partial_tests.rs",
    "loaves/toolchain/incan-cli/tests/cli_language_call_regression_tests.rs",
    "loaves/toolchain/incan-cli/tests/cli_language_reflection_tests.rs",
    "loaves/toolchain/incan-cli/tests/cli_language_union_regression_tests.rs",
    "loaves/toolchain/incan-cli/tests/cli_language_value_regression_tests.rs",
    "loaves/toolchain/incan-cli/tests/cli_provider_boundary_tests.rs",
    "loaves/toolchain/incan-cli/tests/cli_rust_borrow_interop_tests.rs",
    "loaves/toolchain/incan-cli/tests/cli_rust_expression_interop_tests.rs",
    "loaves/toolchain/incan-cli/tests/cli_rust_generic_interop_tests.rs",
    "loaves/toolchain/incan-cli/tests/cli_interop_target_tests.rs",
    "loaves/toolchain/incan-cli/tests/cli_surface_tests.rs",
    "loaves/toolchain/incan-cli/tests/cli_lock_policy_tests.rs",
    "loaves/toolchain/incan-cli/tests/cli_workspace_command_tests.rs",
    "loaves/toolchain/incan-cli/tests/cli_workspace_scope_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_codegen_async_and_modules_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_codegen_constants_and_interop_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_codegen_hash_and_io_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_codegen_imports_and_results_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_codegen_language_and_fs_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_codegen_ownership_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_codegen_stdlib_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_format_and_cli_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_language_runtime_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_lexer_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_numeric_semantics_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_runtime_types_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_sdk_interop_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_test_runner_basics_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_test_runner_fixtures_tests.rs",
    "loaves/toolchain/incan-cli/tests/integration_test_runner_scheduling_tests.rs",
    "loaves/toolchain/incan-cli/tests/canonical_item_imports.rs",
    "loaves/toolchain/incan-cli/tests/package_boundary_facade_tests.rs",
    "loaves/toolchain/incan-cli/tests/package_executable_representation.rs",
    "loaves/toolchain/incan-cli/tests/rfc031_boundary_parity_tests.rs",
    "loaves/toolchain/incan-cli/tests/rfc031_checked_c_resource_tests.rs",
    "loaves/toolchain/incan-cli/tests/rfc031_checked_c_span_tests.rs",
    "loaves/toolchain/incan-cli/tests/rfc031_manifest_diagnostics_tests.rs",
    "loaves/toolchain/incan-cli/tests/rfc031_pub_model_tests.rs",
    "loaves/toolchain/incan-cli/tests/rfc031_rust_interop_tests.rs",
    "loaves/toolchain/incan-cli/tests/rfc031_stdlib_facade_tests.rs",
    "loaves/toolchain/incan-cli/tests/rfc031_vocab_desugar_tests.rs",
    "loaves/toolchain/incan-cli/tests/rfc031_vocab_integration_tests.rs",
];

const OVEN_COMPILER_SUITE_CAPABILITY_SCHEMA_VERSION: u32 = 1;
const MAX_COMPILER_SUITE_DIRECT_RUSTC_INPUTS: usize = 1024;

/// Explicit process capabilities for one stored compiler-suite root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OvenCompilerSuiteTargetCapabilities {
    pub generated_rust_closure: bool,
    pub cargo_fixture: bool,
    /// A narrow test-only authority for helpers that explicitly invoke the named Loaf baker.
    ///
    /// This deliberately does not set `CARGO` for the whole libtest root: its normal command probes must continue
    /// to encounter the outer exit-97 Cargo guard if they accidentally try to fall back.
    pub explicit_bake_cargo: bool,
}

impl OvenCompilerSuiteTargetCapabilities {
    /// Resolve the narrow, package-qualified capability registry for one receipt-bound root.
    pub fn for_target(package_name: &str, target_kind: &str, source_relative_path: &str) -> Self {
        let generated_rust_closure =
            source_relative_path != "loaves/toolchain/incan-cli/tests/toolchain_installer_tests.rs";
        // The ring libraries' and the command lines' unit tests inspect Rust through Cargo metadata exactly as they
        // did when the root library carried them; the authority followed the tests to the packages that own them
        // (the compiler-suite fixture tests went with the `oven` command family into `oven-cli`, the baker's own
        // tests with `legacy_cargo` into `oven_cargo_compat`).
        let cargo_fixture = matches!(
            (package_name, target_kind, source_relative_path),
            ("rust_inspect", "lib", "loaves/compiler/rust_inspect/src/lib.rs")
                | ("incan_frontend", "lib", "loaves/compiler/incan_frontend/src/lib.rs")
                | ("incan_emit", "lib", "loaves/compiler/incan_emit/src/lib.rs")
                | ("incan_driver", "lib", "loaves/compiler/incan_driver/src/lib.rs")
                | ("oven_rustc", "lib", "loaves/oven/oven_rustc/src/lib.rs")
                | ("oven_cargo_compat", "lib", "loaves/oven/oven_cargo_compat/src/lib.rs")
                | ("incan-cli", "lib", "loaves/toolchain/incan-cli/src/lib.rs")
                | ("oven-cli", "lib", "loaves/toolchain/oven-cli/src/lib.rs")
                | (
                    "incan_driver",
                    "test",
                    "loaves/compiler/incan_driver/tests/generated_rust_artifact_tests.rs"
                )
                | (
                    "incan_driver",
                    "test",
                    "loaves/compiler/incan_driver/tests/generated_rust_callability_artifact_tests.rs"
                )
                | (
                    "incan_driver",
                    "test",
                    "loaves/compiler/incan_driver/tests/generated_cache_integration.rs"
                )
        );
        // No behavior-fixture root (`loaves/toolchain/incan-cli/tests/behavior_*_tests.rs`) is registered here, by
        // design: their programs run on the sealed stdlib Loaf, and the provider bakes of the `cli_dependencies`
        // area run with no Cargo authority, so a bake that reaches for Cargo meets the scheduler's guard instead.
        let explicit_bake_cargo = target_kind == "test"
            && match package_name {
                "incan_driver" => {
                    matches!(
                        source_relative_path,
                        "loaves/compiler/incan_driver/tests/native_driver_project_tests.rs"
                            | "loaves/compiler/incan_driver/tests/body_ir_caller_project_tests.rs"
                    )
                }
                "incan-cli" => EXPLICIT_CLI_BAKE_ROOTS.contains(&source_relative_path),
                _ => false,
            };
        Self {
            generated_rust_closure,
            cargo_fixture,
            explicit_bake_cargo,
        }
    }
}

/// One complete, receipt-selected direct-Rustc closure exported to a suite child.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerSuiteCapability {
    schema_version: u32,
    pub rustc: PathBuf,
    pub dependency_search_paths: Vec<PathBuf>,
    pub externs: BTreeMap<String, PathBuf>,
}

/// One suite-built workspace artifact and the declared selector its unified compilation satisfies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerSuiteRustUnitLibrary {
    /// Crate name declared by the authored Loaf.
    pub crate_name: String,
    /// Canonical source package selected by the authored Loaf.
    pub package_root: PathBuf,
    /// Explicit features declared by the authored Loaf.
    pub requested_features: Vec<String>,
    /// Whether the authored dependency enables its package defaults.
    pub default_features: bool,
    /// Resolved workspace features, including requests forwarded by other workspace dependencies.
    pub features: Vec<String>,
    /// Native library compiled by the suite.
    pub output: PathBuf,
    /// Content digest checked again by the nested consumer.
    pub digest: String,
}

/// One transitive workspace instance selected by the parent compiler-suite invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerSuiteRustUnitArtifact {
    /// Rust crate name encoded in the compiled library.
    pub crate_name: String,
    /// Exact native artifact used by every dependent in this cohort.
    pub output: PathBuf,
    /// Content identity verified again before composing the graph.
    pub digest: String,
}

/// One registry unit compiled into the suite foundation, kept distinct by package version and compilation domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerSuiteRustUnitRegistryArtifact {
    /// Exact package name from the publisher's artifact index.
    pub package: String,
    /// Exact locked package version.
    pub version: String,
    /// Whether this is a host artifact rather than a target library.
    pub host: bool,
    /// Selected workspace units that directly consume this exact foundation artifact.
    pub consumers: Vec<String>,
    /// Native artifact and content identity used by the compiled workspace consumers.
    pub artifact: OvenCompilerSuiteRustUnitArtifact,
}

/// Profile-specific native workspace closure held by the parent compiler-suite invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerSuiteRustUnitCapability {
    /// Wire version; consumers refuse any other version.
    pub schema_version: u32,
    /// Compiler target of every transported unit.
    pub target: String,
    /// Build profile requested by the explicit bake.
    pub profile: String,
    /// Compiler executable used to build every transported unit.
    pub rustc: PathBuf,
    /// Exact declared roots and their feature-unified native artifacts.
    pub libraries: Vec<OvenCompilerSuiteRustUnitLibrary>,
    /// Every compiled workspace instance, including units reached only through metadata.
    pub workspace_instances: Vec<OvenCompilerSuiteRustUnitArtifact>,
    /// Direct dependency names for each exact workspace instance, bounding one consumer's reachable graph.
    pub workspace_dependency_graph: BTreeMap<String, Vec<String>>,
    /// Exact registry instances inherited by the workspace variants from their selected foundation.
    pub registry_instances: Vec<OvenCompilerSuiteRustUnitRegistryArtifact>,
    /// Parent-verified metadata search closure, including transitive workspace units.
    pub dependency_search_paths: Vec<PathBuf>,
}

impl OvenCompilerSuiteRustUnitCapability {
    /// Serialize the exact parent-owned native closure for a nested explicit baker.
    pub fn encode(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| format!("cannot encode Rust-unit capability: {error}"))
    }

    /// Read and validate an optional native workspace closure without consulting Cargo.
    pub fn from_environment() -> Result<Option<Self>, String> {
        let Some(payload) = std::env::var_os(OVEN_COMPILER_SUITE_RUST_UNIT_CAPABILITY_ENV) else {
            return Ok(None);
        };
        let capability: Self = serde_json::from_str(&payload.to_string_lossy())
            .map_err(|error| format!("invalid Rust-unit capability: {error}"))?;
        if capability.schema_version != 3 {
            return Err(format!(
                "unsupported Rust-unit capability schema {}",
                capability.schema_version
            ));
        }
        Ok(Some(capability))
    }
}

impl OvenCompilerSuiteCapability {
    /// Construct the complete typed direct-Rustc closure selected for one suite child.
    pub fn new(rustc: PathBuf, dependency_search_paths: Vec<PathBuf>, externs: BTreeMap<String, PathBuf>) -> Self {
        Self {
            schema_version: OVEN_COMPILER_SUITE_CAPABILITY_SCHEMA_VERSION,
            rustc,
            dependency_search_paths,
            externs,
        }
    }

    /// Encode one capability for a child process without indexed environment-key conventions.
    pub fn encode(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| format!("could not encode compiler-suite capability: {error}"))
    }

    /// Decode one complete capability and reject schema drift before any consumer uses a partial closure.
    pub fn decode(payload: &str) -> Result<Self, String> {
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
    pub fn from_environment(name: &str) -> Result<Option<Self>, String> {
        let Some(payload) = std::env::var_os(name).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        Self::decode(&payload.to_string_lossy()).map(Some)
    }
}

/// Internal marker enabled only while the named legacy publisher creates a compiler-owned Loaf.
///
/// This is deliberately distinct from normal Oven command selection: it grants compiler source emission the same
/// trusted standard-provider identity as the SDK publisher, but it never authorizes Cargo for a caller command.
pub const OVEN_LOAF_ENV: &str = "INCAN_OVEN_LOAF";
