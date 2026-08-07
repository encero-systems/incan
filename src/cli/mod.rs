//! CLI module for the Incan compiler
//!
//! This module provides the command-line interface for the compiler.
//!
//! ## Commands
//!
//! - `check <file>` - Type-check with optional stable JSON diagnostics
//! - `explain <code>` - Explain stable diagnostic codes
//! - `build <file>` - Compile to Rust and build executable
//! - `build --lib` - Validate library-mode preconditions
//! - `inspect rust <file|project>` - Inspect current generated Rust backend output
//! - `inspect codegraph <file|dir>` - Export compiler-backed codegraph records as JSONL
//! - `run [file]` - Compile and run the program, defaulting to `[project.scripts].main`
//! - `init [path]` - Create a starter project scaffold in an existing directory
//! - `new [name]` - Create a new Incan project directory, prompting when no name is provided
//! - `fmt <file|dir>` - Format Incan source files
//! - `test [path]` - Run tests (pytest-style)
//! - `version <bump>|--set <version>` - Update `[project].version` in `incan.toml`
//! - `env <subcommand>` - Inspect and run named project environments
//! - `tools doctor` - Inspect local CLI/LSP/editor toolchain resolution
//!
//! ## Modules
//!
//! - `commands` - Command implementations
//! - `prelude` - Stdlib/prelude loading
//! - `test_runner` - Test discovery and execution
//!
//! ## Design
//!
//! The CLI uses clap for argument parsing with derive macros.
//! Command functions return `CliResult<T>` instead of calling `process::exit`.
//! Only the top-level `run()` function handles errors and exits.

// Enforce explicit error handling - no panicking in production code
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

pub mod commands;
pub mod prelude;
pub mod test_runner;

use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process;

use crate::manifest::ProjectManifest;
use crate::provider::FeatureSelection;
use crate::workspace::{ResolvedWorkspaceScope, WorkspaceGraph, WorkspaceMember, WorkspaceScopeRequest};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use commands::binding_inspect::BindingInspectionFormat;
use commands::build_report::{BuildReportFormat, BuildReportOptions, RustInspectionFormat};
use commands::codegraph::CodegraphInspectionFormat;
use commands::common::{
    CargoPolicy, CargoPolicyCliFlags, INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV, INTERNAL_LIBRARY_DEPENDENCY_PREPARATION_ENV,
};
use commands::diagnostics::DiagnosticOutputFormat;
use commands::interop_plan::InteropPlanInspectionFormat;
use commands::lifecycle::{EnvOutputFormat, VersionBumpArg};
use commands::provider_inspect::ProviderInspectionFormat;
use commands::tools::{ToolsDoctorFormat, ToolsMetadataFormat, ToolsModelMetadataFormat};
use commands::workspace::WorkspaceInspectFormat;

// ============================================================================
// CLI Error handling
// ============================================================================

/// Exit code for CLI operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitCode(pub i32);

impl ExitCode {
    pub const SUCCESS: ExitCode = ExitCode(0);
    pub const FAILURE: ExitCode = ExitCode(1);
}

/// Error type for CLI operations.
///
/// Contains a user-facing message and an exit code. The CLI entry point
/// catches these errors, prints the message, and exits with the code.
#[derive(Debug)]
pub struct CliError {
    /// User-facing error message (already formatted for display)
    pub message: String,
    /// Exit code to return to the shell
    pub exit_code: ExitCode,
}

impl CliError {
    /// Create a new CLI error with a message and exit code.
    pub fn new(message: impl Into<String>, exit_code: ExitCode) -> Self {
        Self {
            message: message.into(),
            exit_code,
        }
    }

    /// Create a failure error (exit code 1).
    pub fn failure(message: impl Into<String>) -> Self {
        Self::new(message, ExitCode::FAILURE)
    }

    /// Create an error with a custom exit code.
    pub fn with_code(message: impl Into<String>, code: i32) -> Self {
        Self::new(message, ExitCode(code))
    }
}

impl fmt::Display for CliError {
    /// Render the user-facing CLI error message.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CliError {}

/// Result type for CLI operations.
pub type CliResult<T> = Result<T, CliError>;

/// ASCII art logo - embedded at compile time from assets/logo.txt
const LOGO: &str = include_str!("../../assets/logo.txt");
const VERSION: &str = crate::version::INCAN_VERSION;

// ============================================================================
// Clap CLI definition
// ============================================================================

/// The Incan programming language compiler
#[derive(Parser, Debug)]
#[command(name = "incan")]
#[command(version = VERSION)]
#[command(about = "The Incan programming language compiler", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// File to type check (default action when no subcommand given)
    #[arg(value_name = "FILE")]
    pub file: Option<PathBuf>,

    // Debug/development flags
    /// Tokenize only (debug)
    #[arg(long = "lex", value_name = "FILE", conflicts_with = "file")]
    pub lex_file: Option<PathBuf>,

    /// Parse only (debug)
    #[arg(long = "parse", value_name = "FILE", conflicts_with = "file")]
    pub parse_file: Option<PathBuf>,

    /// Type check only (debug)
    #[arg(long = "check", value_name = "FILE", conflicts_with = "file")]
    pub check_file: Option<PathBuf>,

    /// Output format for the legacy --check debug path
    #[arg(long = "format", value_enum, default_value = "text", requires = "check_file")]
    pub check_format: DiagnosticOutputFormat,

    /// Emit generated Rust code (debug)
    #[arg(long = "emit-rust", value_name = "FILE", conflicts_with = "file")]
    pub emit_rust_file: Option<PathBuf>,

    /// Enable strict mode for --emit-rust (warning-clean output)
    #[arg(long = "strict", requires = "emit_rust_file")]
    pub strict: bool,

    /// Disable the ASCII logo banner
    #[arg(long = "no-banner")]
    pub no_banner: bool,

    /// Control ANSI color output
    #[arg(long = "color", value_enum, default_value = "auto")]
    pub color: ColorMode,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

/// Output encoding for generated-cache inspection and pruning reports.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheOutputFormat {
    /// Human-readable cache summary.
    Text,
    /// Stable machine-readable JSON report.
    Json,
}

/// Managed cache category selected by cache-management commands.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheCategory {
    /// Cargo artifacts produced from generated Rust projects.
    GeneratedCargo,
}

/// Output encoding for explicit Oven Alpha receipts, plans, storage, test reports, and run reports.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OvenOutputFormat {
    /// Human-readable command result.
    Text,
    /// Stable machine-readable JSON report.
    Json,
}

/// Built-in compiler-owned Loaf envelope selected by the hidden baker.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OvenLoafEnvelopeArgument {
    /// Minimal Loafs shipped in a release toolchain.
    Release,
    /// Complete Loaf set used by the repository compiler suite.
    CompilerSuite,
}

/// Incan package-feature selection shared by compilation commands.
///
/// These flags select package-owned semantic features. They are intentionally separate from the explicitly prefixed
/// Cargo feature flags, which remain private backend controls.
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageFeatureCliFlags {
    /// Incan package features to enable (comma-separated)
    #[arg(long = "features", value_delimiter = ',', value_name = "FEATURE")]
    features: Vec<String>,
    /// Disable the Incan package's default features
    #[arg(long = "no-default-features")]
    no_default_features: bool,
    /// Enable every Incan package feature
    #[arg(long = "all-features")]
    all_features: bool,
}

impl From<PackageFeatureCliFlags> for FeatureSelection {
    fn from(flags: PackageFeatureCliFlags) -> Self {
        Self {
            requested: flags.features.into_iter().collect(),
            no_default_features: flags.no_default_features,
            all_features: flags.all_features,
        }
    }
}

/// Command-local SDK profile selection shared by compilation and inspection commands.
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct SdkProfileCliFlags {
    /// Replace the project's SDK profile for this invocation without changing explicit additions or exclusions
    #[arg(long = "sdk-profile", value_name = "PROFILE")]
    sdk_profile: Option<String>,
}

impl SdkProfileCliFlags {
    /// Return the command-local SDK profile override without changing persistent project selection.
    fn profile(&self) -> Option<&str> {
        self.sdk_profile.as_deref()
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Compile to Rust and build an executable through Oven Alpha direct-rustc
    Build {
        /// Source file to compile
        #[arg(value_name = "FILE")]
        file: Option<PathBuf>,
        /// Build the `src/lib.incn` library through Oven Alpha direct-rustc
        #[arg(long = "lib", hide = true)]
        lib_mode: bool,
        /// Output directory (default: `target/incan/<name>`)
        #[arg(value_name = "OUTPUT_DIR")]
        output_dir: Option<PathBuf>,
        /// Select Incan package features for this compilation
        #[command(flatten)]
        package_features: PackageFeatureCliFlags,
        /// Select a non-persistent SDK profile for this compilation
        #[command(flatten)]
        sdk_profile: SdkProfileCliFlags,
        /// Require up-to-date incan.lock; does not authorize a Cargo command
        #[arg(long, hide = true)]
        locked: bool,
        /// Disable INCAN_LOCKED for this invocation
        #[arg(long = "no-locked", conflicts_with_all = ["locked", "frozen"], hide = true)]
        no_locked: bool,
        /// Require offline-compatible locked inputs; does not authorize a Cargo command
        #[arg(long, hide = true)]
        offline: bool,
        /// Disable INCAN_OFFLINE for this invocation
        #[arg(long = "no-offline", conflicts_with_all = ["offline", "frozen"], hide = true)]
        no_offline: bool,
        /// Require an up-to-date frozen incan.lock; does not authorize a Cargo command
        #[arg(long, hide = true)]
        frozen: bool,
        /// Disable INCAN_FROZEN for this invocation
        #[arg(long = "no-frozen", conflicts_with = "frozen", hide = true)]
        no_frozen: bool,
        /// Retired Cargo argument surface; normal Oven commands reject it
        #[arg(long = "cargo-args", value_name = "ARG", num_args = 1.., allow_hyphen_values = true, hide = true)]
        cargo_args: Vec<String>,
        /// Retired Cargo feature surface; normal Oven commands reject it
        #[arg(long = "cargo-features", value_delimiter = ',', hide = true)]
        cargo_features: Vec<String>,
        /// Retired Cargo feature surface; normal Oven commands reject it
        #[arg(long = "cargo-no-default-features", hide = true)]
        cargo_no_default_features: bool,
        /// Retired Cargo feature surface; normal Oven commands reject it
        #[arg(long = "cargo-all-features", hide = true)]
        cargo_all_features: bool,
        /// Retired generated-Cargo target override; normal Oven commands reject it
        #[arg(long = "generated-cargo-target-dir", value_name = "PATH", hide = true)]
        generated_cargo_target_dir: Option<PathBuf>,
        /// Explicitly request the release build profile. This is the default for `incan build` and exists for
        /// first-contact command symmetry.
        #[arg(long)]
        release: bool,
        /// Emit a machine-readable build report
        #[arg(long = "report", value_enum)]
        report: Option<BuildReportFormat>,
        /// Write the build report to this path instead of stdout
        #[arg(long = "report-output", value_name = "PATH", requires = "report")]
        report_output: Option<PathBuf>,
        /// Select every member in the active workspace
        #[arg(long, conflicts_with = "members")]
        workspace: bool,
        /// Select one workspace member by name or root-relative path; may be repeated
        #[arg(long = "member", value_name = "NAME_OR_PATH", conflicts_with = "workspace")]
        members: Vec<String>,
        /// Retired Cargo passthrough surface; normal Oven commands reject it
        #[arg(last = true, hide = true)]
        cargo_passthrough: Vec<String>,
    },

    /// Type check a file or project entrypoint
    Check {
        /// File or project entrypoint to check
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: DiagnosticOutputFormat,
        /// Select Incan package features for this check
        #[command(flatten)]
        package_features: PackageFeatureCliFlags,
        /// Select a non-persistent SDK profile for this check
        #[command(flatten)]
        sdk_profile: SdkProfileCliFlags,
        /// Verify checked C declarations against this declared Oven interop target; this does not cross-compile Rust
        /// or package an app
        #[arg(long = "interop-target", value_name = "TRIPLE")]
        interop_target: Option<String>,
        /// Select every member in the active workspace
        #[arg(long, conflicts_with = "members")]
        workspace: bool,
        /// Select one workspace member by name or root-relative path; may be repeated
        #[arg(long = "member", value_name = "NAME_OR_PATH", conflicts_with = "workspace")]
        members: Vec<String>,
    },

    /// Explain a diagnostic code
    Explain {
        /// Diagnostic code, for example INCAN-P0001
        #[arg(value_name = "CODE")]
        code: String,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: DiagnosticOutputFormat,
    },

    /// Compile and run the program (debug profile by default; opt into release with `--release`)
    Run {
        /// Source file to run
        #[arg(value_name = "FILE", conflicts_with = "command")]
        file: Option<PathBuf>,
        /// Run inline source code
        #[arg(short = 'c', long = "command", value_name = "CODE")]
        command: Option<String>,
        /// Select Incan package features for this compilation
        #[command(flatten)]
        package_features: PackageFeatureCliFlags,
        /// Select a non-persistent SDK profile for this compilation
        #[command(flatten)]
        sdk_profile: SdkProfileCliFlags,
        /// Require up-to-date incan.lock; does not authorize a Cargo command
        #[arg(long, hide = true)]
        locked: bool,
        /// Disable INCAN_LOCKED for this invocation
        #[arg(long = "no-locked", conflicts_with_all = ["locked", "frozen"], hide = true)]
        no_locked: bool,
        /// Require offline-compatible locked inputs; does not authorize a Cargo command
        #[arg(long, hide = true)]
        offline: bool,
        /// Disable INCAN_OFFLINE for this invocation
        #[arg(long = "no-offline", conflicts_with_all = ["offline", "frozen"], hide = true)]
        no_offline: bool,
        /// Require an up-to-date frozen incan.lock; does not authorize a Cargo command
        #[arg(long, hide = true)]
        frozen: bool,
        /// Disable INCAN_FROZEN for this invocation
        #[arg(long = "no-frozen", conflicts_with = "frozen", hide = true)]
        no_frozen: bool,
        /// Retired Cargo argument surface; normal Oven commands reject it
        #[arg(long = "cargo-args", value_name = "ARG", num_args = 1.., allow_hyphen_values = true, hide = true)]
        cargo_args: Vec<String>,
        /// Retired Cargo feature surface; normal Oven commands reject it
        #[arg(long = "cargo-features", value_delimiter = ',', hide = true)]
        cargo_features: Vec<String>,
        /// Retired Cargo feature surface; normal Oven commands reject it
        #[arg(long = "cargo-no-default-features", hide = true)]
        cargo_no_default_features: bool,
        /// Retired Cargo feature surface; normal Oven commands reject it
        #[arg(long = "cargo-all-features", hide = true)]
        cargo_all_features: bool,
        /// Build and run with the optimized Oven release profile
        #[arg(long)]
        release: bool,
        /// Select every member in the active workspace (only valid when it resolves to one member)
        #[arg(long, conflicts_with = "members")]
        workspace: bool,
        /// Select one workspace member by name or root-relative path
        #[arg(long = "member", value_name = "NAME_OR_PATH", conflicts_with = "workspace")]
        members: Vec<String>,
        /// Retired Cargo passthrough surface; normal Oven commands reject it
        #[arg(last = true, hide = true)]
        cargo_passthrough: Vec<String>,
    },

    /// Format Incan source files
    Fmt {
        /// File or directory to format
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Check formatting without modifying files
        #[arg(long)]
        check: bool,
        /// Show diff of formatting changes
        #[arg(long)]
        diff: bool,
        /// Select every member in the active workspace
        #[arg(long, conflicts_with = "members")]
        workspace: bool,
        /// Select one workspace member by name or root-relative path; may be repeated
        #[arg(long = "member", value_name = "NAME_OR_PATH", conflicts_with = "workspace")]
        members: Vec<String>,
    },

    /// Update the project version in incan.toml
    Version {
        /// Version bump to apply
        #[arg(value_enum)]
        bump: Option<VersionBumpArg>,
        /// Explicit SemVer version to set
        #[arg(long = "set", value_name = "VERSION")]
        set: Option<String>,
        /// Print the planned change without writing incan.toml
        #[arg(long)]
        dry_run: bool,
        /// Keep prerelease metadata when applying major/minor/patch bumps
        #[arg(long)]
        keep_prerelease: bool,
        /// Project root containing incan.toml
        #[arg(long = "project", value_name = "PATH")]
        project: Option<PathBuf>,
        /// Select every member in the active workspace (only valid when it resolves to one member)
        #[arg(long, conflicts_with_all = ["members", "project"])]
        workspace: bool,
        /// Select one workspace member by name or root-relative path
        #[arg(long = "member", value_name = "NAME_OR_PATH", conflicts_with_all = ["workspace", "project"])]
        members: Vec<String>,
    },

    /// Run named project environment scripts
    Env {
        #[command(subcommand)]
        command: EnvCommand,
    },

    /// Inspect the validated RFC 077 workspace graph and selected command scope
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },

    /// Inspect local toolchain and editor integration state
    Tools {
        #[command(subcommand)]
        command: ToolsCommand,
    },

    /// Inspect or prune Incan-managed generated-build caches
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },

    /// Run the explicit Oven Alpha receipt, bounded-store, and native direct-rustc workflow
    Oven {
        #[command(subcommand)]
        command: OvenCommand,
    },

    /// Inspect compiler artifacts and semantic projections
    Inspect {
        #[command(subcommand)]
        command: InspectCommand,
    },

    /// Run tests (pytest-style)
    Test {
        /// Path to test file or directory
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Select Incan package features for collection and generated test batches
        #[command(flatten)]
        package_features: PackageFeatureCliFlags,
        /// Select a non-persistent SDK profile for collection and generated test batches
        #[command(flatten)]
        sdk_profile: SdkProfileCliFlags,
        /// Verbose output
        #[arg(short, long)]
        verbose: bool,
        /// Stop on first failure
        #[arg(short = 'x', long = "exitfirst")]
        stop_on_fail: bool,
        /// Include slow tests
        #[arg(long)]
        slow: bool,
        /// Filter tests by keyword expression
        #[arg(short = 'k', value_name = "EXPR")]
        filter: Option<String>,
        /// Filter tests by marker expression
        #[arg(short = 'm', long = "markers", value_name = "EXPR")]
        marker_expr: Option<String>,
        /// Treat unknown marker names as collection errors
        #[arg(long = "strict-markers")]
        strict_markers: bool,
        /// Maximum number of runner execution units to run concurrently
        #[arg(short = 'j', long = "jobs", value_name = "N", default_value_t = 1)]
        jobs: usize,
        /// Enable a collection-time testing feature for std.testing.feature("name")
        #[arg(long = "feature", value_name = "NAME")]
        test_features: Vec<String>,
        /// Default generated test-batch timeout, such as 250ms, 5s, or 2m
        #[arg(long = "timeout", value_name = "DURATION")]
        timeout: Option<String>,
        /// Show test stdout/stderr even when tests pass
        #[arg(long = "nocapture")]
        no_capture: bool,
        /// Fail if no tests are collected
        #[arg(long = "fail-on-empty")]
        fail_on_empty: bool,
        /// List collected tests after filtering and do not execute them
        #[arg(long = "list")]
        list_only: bool,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "console")]
        report_format: test_runner::TestOutputFormat,
        /// Write a JUnit XML report to this path
        #[arg(long = "junit", value_name = "PATH")]
        junit_path: Option<PathBuf>,
        /// Show the slowest N test durations after the run
        #[arg(long = "durations", value_name = "N")]
        durations: Option<usize>,
        /// Shuffle test execution order
        #[arg(long)]
        shuffle: bool,
        /// Seed used with --shuffle
        #[arg(long, value_name = "N")]
        seed: Option<u64>,
        /// Run xfail tests as ordinary tests
        #[arg(long = "run-xfail")]
        run_xfail: bool,
        /// Require up-to-date incan.lock; does not authorize a Cargo command
        #[arg(long, hide = true)]
        locked: bool,
        /// Disable INCAN_LOCKED for this invocation
        #[arg(long = "no-locked", conflicts_with_all = ["locked", "frozen"], hide = true)]
        no_locked: bool,
        /// Require offline-compatible locked inputs; does not authorize a Cargo command
        #[arg(long, hide = true)]
        offline: bool,
        /// Disable INCAN_OFFLINE for this invocation
        #[arg(long = "no-offline", conflicts_with_all = ["offline", "frozen"], hide = true)]
        no_offline: bool,
        /// Require an up-to-date frozen incan.lock; does not authorize a Cargo command
        #[arg(long, hide = true)]
        frozen: bool,
        /// Disable INCAN_FROZEN for this invocation
        #[arg(long = "no-frozen", conflicts_with = "frozen", hide = true)]
        no_frozen: bool,
        /// Retired Cargo argument surface; normal Oven commands reject it
        #[arg(long = "cargo-args", value_name = "ARG", num_args = 1.., allow_hyphen_values = true, hide = true)]
        cargo_args: Vec<String>,
        /// Retired Cargo feature surface; normal Oven commands reject it
        #[arg(long = "cargo-features", value_delimiter = ',', hide = true)]
        cargo_features: Vec<String>,
        /// Retired Cargo feature surface; normal Oven commands reject it
        #[arg(long = "cargo-no-default-features", hide = true)]
        cargo_no_default_features: bool,
        /// Retired Cargo feature surface; normal Oven commands reject it
        #[arg(long = "cargo-all-features", hide = true)]
        cargo_all_features: bool,
        /// Select every member in the active workspace
        #[arg(long, conflicts_with = "members")]
        workspace: bool,
        /// Select one workspace member by name or root-relative path; may be repeated
        #[arg(long = "member", value_name = "NAME_OR_PATH", conflicts_with = "workspace")]
        members: Vec<String>,
        /// Retired Cargo passthrough surface; normal Oven commands reject it
        #[arg(last = true, hide = true)]
        cargo_passthrough: Vec<String>,
    },

    /// Create a new Incan project directory
    New {
        /// Project name; prompted for interactively when omitted on a terminal
        #[arg(value_name = "NAME")]
        name: Option<String>,
        /// Directory to create (default: `./<name>`)
        #[arg(long = "dir", value_name = "PATH")]
        dir: Option<PathBuf>,
        /// Project description
        #[arg(long, value_name = "TEXT")]
        description: Option<String>,
        /// Project author, usually `Name <email>`
        #[arg(long, value_name = "AUTHOR")]
        author: Option<String>,
        /// Project license identifier or expression
        #[arg(long, value_name = "LICENSE")]
        license: Option<String>,
        /// Reuse an existing directory and overwrite generated files
        #[arg(long)]
        force: bool,
        /// Use defaults without interactive prompts
        #[arg(short = 'y', long = "yes")]
        yes: bool,
    },

    /// Initialize a new incan.toml manifest
    Init {
        /// Directory to create incan.toml in
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Project name (defaults to directory name)
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
        /// Project version
        #[arg(long, value_name = "VERSION", default_value = "0.1.0")]
        version: String,
        /// Project description
        #[arg(long, value_name = "TEXT")]
        description: Option<String>,
        /// Project author, usually `Name <email>`
        #[arg(long, value_name = "AUTHOR")]
        author: Option<String>,
        /// Project license identifier or expression
        #[arg(long, value_name = "LICENSE")]
        license: Option<String>,
        /// Overwrite existing generated files
        #[arg(long)]
        force: bool,
        /// Preserve an existing `src/main.incn` and reuse source-derived defaults where possible
        #[arg(long)]
        detect: bool,
        /// Use defaults without interactive prompts
        #[arg(short = 'y', long = "yes")]
        yes: bool,
    },

    /// Generate or update incan.lock for a project
    Lock {
        /// Entry file used to resolve inline dependencies
        #[arg(value_name = "FILE")]
        file: Option<PathBuf>,
        /// Select Incan package features for the locked graph
        #[command(flatten)]
        package_features: PackageFeatureCliFlags,
        /// Select a non-persistent SDK profile for the locked graph
        #[command(flatten)]
        sdk_profile: SdkProfileCliFlags,
        /// Cargo features to enable (comma-separated)
        #[arg(long = "cargo-features", value_delimiter = ',')]
        cargo_features: Vec<String>,
        /// Disable Cargo default features
        #[arg(long = "cargo-no-default-features")]
        cargo_no_default_features: bool,
        /// Enable all Cargo features
        #[arg(long = "cargo-all-features")]
        cargo_all_features: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum InspectCommand {
    /// Inspect an Oven receipt's reusable build unit, stored-plan selection, and bounded storage evidence
    Oven {
        /// Receipt written by normal Oven preparation or explicit import
        #[arg(long, value_name = "PATH")]
        receipt: PathBuf,
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
    /// Generate and inspect current Rust backend output
    Rust {
        /// Source file or project root to inspect
        #[arg(value_name = "PATH")]
        path: PathBuf,
        /// Inspect the library build surface rooted at `src/lib.incn`
        #[arg(long = "lib")]
        lib_mode: bool,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: RustInspectionFormat,
    },
    /// Export compiler-backed codegraph records
    Codegraph {
        /// Source file or directory to inspect
        #[arg(value_name = "PATH")]
        path: PathBuf,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "jsonl")]
        format: CodegraphInspectionFormat,
        /// Emit partial graph records and diagnostics for broken source
        #[arg(long = "allow-errors")]
        allow_errors: bool,
        /// Select Incan package features for this graph projection
        #[command(flatten)]
        package_features: PackageFeatureCliFlags,
        /// Select a non-persistent SDK profile for this graph projection
        #[command(flatten)]
        sdk_profile: SdkProfileCliFlags,
    },
    /// Inspect active SDK components and compiled providers
    Providers {
        /// Source file or project directory whose provider projection should be inspected
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: ProviderInspectionFormat,
        #[command(flatten)]
        package_features: PackageFeatureCliFlags,
        /// Select a non-persistent SDK profile for this provider projection
        #[command(flatten)]
        sdk_profile: SdkProfileCliFlags,
    },
    /// Inspect public package-feature roots, closure, edges, and conditioned facts
    Features {
        /// Source file or project directory whose feature projection should be inspected
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: ProviderInspectionFormat,
        #[command(flatten)]
        package_features: PackageFeatureCliFlags,
        /// Select a non-persistent SDK profile for this feature projection
        #[command(flatten)]
        sdk_profile: SdkProfileCliFlags,
    },
    /// Inspect compiler-checked C binding declarations
    Bindings {
        /// Source file or project directory whose checked C declarations should be inspected
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: BindingInspectionFormat,
        /// Select Incan package features for this binding projection
        #[command(flatten)]
        package_features: PackageFeatureCliFlags,
        /// Select a non-persistent SDK profile for this binding projection
        #[command(flatten)]
        sdk_profile: SdkProfileCliFlags,
    },
    /// Inspect one locked Oven interop deployment handoff
    InteropPlan {
        /// Project path containing the Oven interop declaration and canonical lock
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Exact target triple to project
        #[arg(long, value_name = "TRIPLE")]
        target: String,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: InteropPlanInspectionFormat,
    },
    /// Inspect one complete compiler-checked typed registry without executing user modules
    Registry {
        /// Registry identity, such as `feature::functions` or the unambiguous `package::feature::functions`
        #[arg(value_name = "CANONICAL_IDENTITY")]
        identity: String,
        /// Source project root; defaults to the current directory
        #[arg(long, value_name = "PATH")]
        project: Option<PathBuf>,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "json")]
        format: commands::tools::RegistryInspectionFormat,
    },
}

#[derive(Subcommand, Debug)]
pub enum EnvCommand {
    /// List configured environments
    List {
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: EnvOutputFormat,
        /// Project root containing incan.toml
        #[arg(long = "project", value_name = "PATH")]
        project: Option<PathBuf>,
    },
    /// Show the fully resolved environment
    Show {
        /// Environment name (defaults to an overview of available environments)
        env: Option<String>,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: EnvOutputFormat,
        /// Project root containing incan.toml
        #[arg(long = "project", value_name = "PATH")]
        project: Option<PathBuf>,
    },
    /// Run a configured script in an environment
    Run {
        /// Environment name
        env: String,
        /// Script name
        script: String,
        /// Print the resolved command without executing it
        #[arg(long)]
        dry_run: bool,
        /// Extra arguments passed to the configured script
        #[arg(last = true)]
        args: Vec<String>,
        /// Project root containing incan.toml
        #[arg(long = "project", value_name = "PATH")]
        project: Option<PathBuf>,
    },
}

/// Workspace-management commands.
#[derive(Subcommand, Debug)]
pub enum WorkspaceCommand {
    /// Report the active workspace graph and resolved member scope
    Inspect {
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: WorkspaceInspectFormat,
        /// Select every member in the active workspace
        #[arg(long, conflicts_with = "members")]
        workspace: bool,
        /// Select one member by name or root-relative path; may be repeated
        #[arg(long = "member", value_name = "NAME_OR_PATH", conflicts_with = "workspace")]
        members: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ToolsCommand {
    /// Inspect local `incan` / `incan-lsp` path resolution
    Doctor {
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: ToolsDoctorFormat,
    },
    /// Extract checked metadata for tooling and documentation consumers
    Metadata {
        #[command(subcommand)]
        command: ToolsMetadataCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum ToolsMetadataCommand {
    /// Emit checked public API metadata as JSON
    Api {
        /// Incan source file or project directory to inspect
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "json")]
        format: ToolsMetadataFormat,
    },
    /// Emit a contract-backed model from checked model metadata
    Model {
        /// Project directory, bundle JSON, or `.incnlib` artifact to inspect
        #[arg(value_name = "PATH")]
        path: PathBuf,
        /// Logical type name or stable model id to emit
        #[arg(value_name = "MODEL")]
        model: String,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "incan")]
        format: ToolsModelMetadataFormat,
    },
}

/// Generated-build cache management subcommands.
#[derive(Subcommand, Debug)]
pub enum CacheCommand {
    /// Report managed generated-build cache usage and active domains
    Inspect {
        /// Cache category to inspect
        #[arg(long, value_enum, default_value = "generated-cargo")]
        category: CacheCategory,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: CacheOutputFormat,
    },
    /// Prune idle domains toward a soft limit or remove selected exact identities
    Prune {
        /// Cache category to prune
        #[arg(long, value_enum, default_value = "generated-cargo")]
        category: CacheCategory,
        /// Preview removals without changing the cache
        #[arg(long)]
        dry_run: bool,
        /// Override the configured cache limit for this prune, in bytes
        #[arg(long = "max-bytes", value_name = "BYTES")]
        max_bytes: Option<u64>,
        /// Remove only the selected compatibility identity (repeatable)
        #[arg(long = "identity", value_name = "SHA256", conflicts_with = "max_bytes")]
        identities: Vec<String>,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: CacheOutputFormat,
    },
}

/// Explicit Oven Alpha lifecycle commands.
#[derive(Subcommand, Debug)]
pub enum OvenCommand {
    /// Import frozen Cargo declarations as receipt evidence without launching Cargo
    Import {
        /// Root of the frozen Cargo package to import
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        /// Explicit target triple recorded in the receipt
        #[arg(long, value_name = "TRIPLE")]
        target: String,
        /// Exact selected Rust toolchain identity recorded in the receipt
        #[arg(long, value_name = "IDENTITY")]
        toolchain: String,
        /// Build profile recorded in the receipt
        #[arg(long, default_value = "release")]
        profile: String,
        /// Explicit feature selected for the build unit; may be repeated
        #[arg(long = "feature", value_name = "NAME")]
        features: Vec<String>,
        /// Generated source evidence expressed as `NAME=PATH`; paths are digested, never persisted
        #[arg(long = "source", value_name = "NAME=PATH")]
        source_inputs: Vec<String>,
        /// Receipt output path; defaults to `.incan/oven/receipt.json` below --project
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
    /// Hidden `legacy_cargo` publisher; never used by normal build, run, or test execution
    LegacyCargo {
        #[command(subcommand)]
        command: OvenLegacyCargoCommand,
    },
    /// Compile and run the stored compiler workspace native suite through a direct-rustc plan
    CompilerLibtests {
        /// Repository root containing the compiler Cargo package and src/lib.rs
        #[arg(long = "compiler-root", value_name = "PATH", default_value = ".")]
        compiler_root: PathBuf,
        /// Explicit rustc executable; the active toolchain is used when omitted
        #[arg(long, value_name = "PATH")]
        rustc: Option<PathBuf>,
        /// Root-package feature to include; default Cargo features are always included
        #[arg(long = "feature", value_name = "NAME")]
        features: Vec<String>,
        /// Receipt-bound test source path to execute; may be repeated. Omitting this runs the complete stored suite.
        #[arg(long = "target", value_name = "SOURCE")]
        targets: Vec<String>,
        /// Caller-owned direct-rustc libtest output path
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
    /// Validate and store a receipt-bound direct-rustc artifact plan
    Plan {
        #[command(subcommand)]
        command: OvenPlanCommand,
    },
    /// Inspect or prune bounded Oven artifact storage
    Store {
        #[command(subcommand)]
        command: OvenStoreCommand,
    },
    /// Compile with a stored direct-rustc plan and run exact native tests only after inventory verification
    Test {
        /// Receipt authorizing the generated source and stored plan
        #[arg(long, value_name = "PATH")]
        receipt: PathBuf,
        /// Immutable stored direct-rustc plan identity
        #[arg(long = "plan", value_name = "SHA256")]
        plan_identity: String,
        /// Explicit rustc executable path
        #[arg(long, value_name = "PATH")]
        rustc: PathBuf,
        /// Receipt-authorized generated Rust test source
        #[arg(long, value_name = "PATH")]
        source: PathBuf,
        /// Caller-owned native libtest output path
        #[arg(long, value_name = "PATH")]
        output: PathBuf,
        /// Rust test crate name
        #[arg(long = "crate-name", value_name = "NAME")]
        crate_name: String,
        /// Rust edition for the native test binary
        #[arg(long, default_value = "2024")]
        edition: String,
        /// Named receipt source evidence authorizing --source
        #[arg(long = "source-evidence", value_name = "NAME")]
        source_evidence_key: String,
        /// Exact test name; may be repeated and is verified against native inventory before execution
        #[arg(long = "exact", value_name = "TEST")]
        exact_names: Vec<String>,
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
    /// Compile and run one stored direct-rustc binary without a Cargo consumer
    Run {
        /// Receipt authorizing the generated source and stored plan
        #[arg(long, value_name = "PATH")]
        receipt: PathBuf,
        /// Immutable stored direct-rustc plan identity
        #[arg(long = "plan", value_name = "SHA256")]
        plan_identity: String,
        /// Explicit rustc executable path
        #[arg(long, value_name = "PATH")]
        rustc: PathBuf,
        /// Receipt-authorized generated Rust binary source
        #[arg(long, value_name = "PATH")]
        source: PathBuf,
        /// Caller-owned native binary output path
        #[arg(long, value_name = "PATH")]
        output: PathBuf,
        /// Rust binary crate name
        #[arg(long = "crate-name", value_name = "NAME")]
        crate_name: String,
        /// Rust edition for the native binary
        #[arg(long, default_value = "2024")]
        edition: String,
        /// Named receipt source evidence authorizing --source
        #[arg(long = "source-evidence", value_name = "NAME")]
        source_evidence_key: String,
        /// Arguments forwarded after compilation only to the native binary
        #[arg(last = true, allow_hyphen_values = true, value_name = "ARG")]
        arguments: Vec<OsString>,
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
}

/// Hidden `legacy_cargo` commands for baking immutable Oven inputs.
#[derive(Subcommand, Debug)]
pub enum OvenLegacyCargoCommand {
    /// Prepare one receipt-bound direct-rustc closure and retain only the bounded Oven result
    Prepare {
        /// Generated-project receipt authorizing this preparation
        #[arg(long, value_name = "PATH")]
        receipt: PathBuf,
        /// Caller-owned generated Rust project with Cargo.toml and src/main.rs
        #[arg(long = "generated-project", value_name = "PATH")]
        generated_project: PathBuf,
        /// Explicit Cargo executable used only for this publisher transition
        #[arg(long, value_name = "PATH")]
        cargo: PathBuf,
        /// Explicit Rust compiler required to match the receipt
        #[arg(long, value_name = "PATH")]
        rustc: PathBuf,
        /// Stable compatibility domain for bounded Oven storage
        #[arg(long, value_name = "NAME")]
        domain: String,
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
    /// Bake or reuse one complete compiler-owned Alpha Loaf envelope
    #[command(hide = true)]
    BakeLoafs {
        /// Compiler checkout or staged toolchain root used for runtime-source identity
        #[arg(long = "compiler-root", value_name = "PATH", default_value = ".")]
        compiler_root: PathBuf,
        /// Destination directory for immutable `<identity>.loaf` bundles
        #[arg(long, value_name = "PATH")]
        output: PathBuf,
        /// Bounded compiler-suite store baked with the compiler-suite envelope
        #[arg(long = "suite-store", value_name = "PATH")]
        suite_store: Option<PathBuf>,
        /// Built-in release or compiler-suite Loaf envelope
        #[arg(long, value_enum)]
        envelope: OvenLoafEnvelopeArgument,
        /// Exact compiler-owned SDK provider inventory
        #[arg(long = "sdk-inventory", value_name = "PATH")]
        sdk_inventory: PathBuf,
        /// Explicit Cargo executable used only for a genuine Loaf miss
        #[arg(long, value_name = "PATH")]
        cargo: PathBuf,
        /// Explicit Rust compiler recorded by each Loaf receipt
        #[arg(long, value_name = "PATH")]
        rustc: PathBuf,
        /// Aggregate physical Loaf-envelope allowance
        #[arg(long = "max-physical-bytes", value_name = "BYTES")]
        max_physical_bytes: Option<u64>,
        /// Physical allowance for one Loaf compatibility domain
        #[arg(long = "max-domain-physical-bytes", value_name = "BYTES")]
        max_domain_physical_bytes: Option<u64>,
        /// Logical allowance for one Loaf compatibility domain
        #[arg(long = "max-domain-logical-bytes", value_name = "BYTES")]
        max_domain_logical_bytes: Option<u64>,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
}

/// Direct-rustc plan publication commands.
#[derive(Subcommand, Debug)]
pub enum OvenPlanCommand {
    /// Validate and retain a direct-rustc artifact manifest under bounded Oven policy
    Publish {
        /// Receipt authorizing the plan
        #[arg(long, value_name = "PATH")]
        receipt: PathBuf,
        /// JSON direct-rustc artifact manifest
        #[arg(long, value_name = "PATH")]
        manifest: PathBuf,
        /// Immutable artifact root used to validate the manifest content
        #[arg(long = "artifact-root", value_name = "PATH")]
        artifact_root: PathBuf,
        /// Stable compatibility domain for capacity policy and selection
        #[arg(long, value_name = "NAME")]
        domain: String,
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
}

/// Bounded Oven store management commands.
#[derive(Subcommand, Debug)]
pub enum OvenStoreCommand {
    /// Report physical allocation separately from logical artifact bytes
    Inspect {
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
    /// Prune inactive immutable artifacts to the configured physical allocation policy
    Prune {
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Preview policy-selected removals without changing the Oven store
        #[arg(long)]
        dry_run: bool,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
}

/// Shared CLI policy flags for a bounded Oven artifact store.
#[derive(Args, Debug, Clone)]
pub struct OvenStoreCliFlags {
    /// Explicit Oven store root; defaults below INCAN_HOME or the user home directory
    #[arg(long = "store", value_name = "PATH")]
    root: Option<PathBuf>,
    /// Maximum aggregate physical allocation in bytes
    #[arg(long = "max-physical-bytes", value_name = "BYTES")]
    max_physical_bytes: Option<u64>,
    /// Maximum physical allocation for one compatibility domain in bytes
    #[arg(long = "max-domain-physical-bytes", value_name = "BYTES")]
    max_domain_physical_bytes: Option<u64>,
    /// Maximum logical artifact bytes for one compatibility domain in bytes
    #[arg(long = "max-domain-logical-bytes", value_name = "BYTES")]
    max_domain_logical_bytes: Option<u64>,
}

impl From<OvenStoreCliFlags> for commands::OvenStoreCommandOptions {
    /// Convert command-line bounded store selections into command-owned request data.
    fn from(value: OvenStoreCliFlags) -> Self {
        Self {
            root: value.root,
            max_physical_bytes: value.max_physical_bytes,
            max_domain_physical_bytes: value.max_domain_physical_bytes,
            max_domain_logical_bytes: value.max_domain_logical_bytes,
        }
    }
}

// ============================================================================
// CLI entry point
// ============================================================================

/// Main CLI entry point.
///
/// This is the only place where `process::exit` is called. All command implementations return `CliResult` and errors
/// are handled here. Parse CLI arguments, execute the selected command, and exit the process.
pub fn run() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            let kind = err.kind();
            let _ = err.print();
            let exit_code = match kind {
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => ExitCode::SUCCESS,
                _ => ExitCode::FAILURE,
            };
            process::exit(exit_code.0);
        }
    };

    let use_color = should_use_color(cli.color);
    if should_print_banner(&cli, use_color) {
        print_logo(use_color);
    }

    match execute(cli, use_color) {
        Ok(exit_code) => {
            if exit_code.0 != 0 {
                process::exit(exit_code.0);
            }
        }
        Err(e) => {
            if !e.message.is_empty() {
                eprintln!("{}", e.message);
            }
            process::exit(e.exit_code.0);
        }
    }
}

/// Execute the CLI command and return result.
/// Execute one already-parsed CLI request without terminating the process.
fn execute(cli: Cli, use_color: bool) -> CliResult<ExitCode> {
    // Handle debug flags first
    if let Some(file) = cli.lex_file {
        return commands::lex_file(&file.to_string_lossy());
    }
    if let Some(file) = cli.parse_file {
        return commands::parse_file(&file.to_string_lossy());
    }
    if let Some(file) = cli.check_file {
        return commands::check_path(&file, cli.check_format);
    }
    if let Some(file) = cli.emit_rust_file {
        return commands::emit_rust(&file.to_string_lossy(), cli.strict);
    }

    // Handle subcommands
    match cli.command {
        Some(Command::Build {
            file,
            lib_mode,
            output_dir,
            package_features,
            sdk_profile,
            locked,
            offline,
            no_offline,
            frozen,
            no_frozen,
            no_locked,
            cargo_args,
            cargo_features,
            cargo_no_default_features,
            cargo_all_features,
            generated_cargo_target_dir,
            release: _,
            report,
            report_output,
            workspace,
            members,
            cargo_passthrough,
        }) => execute_build(
            BuildCommandRequest {
                file,
                lib_mode,
                output_dir: output_dir.map(|path| path.to_string_lossy().to_string()),
                options: commands::build::BuildCommandOptions {
                    cargo_policy: CargoPolicy::from_cli_and_env(
                        CargoPolicyCliFlags {
                            offline,
                            no_offline,
                            locked,
                            no_locked,
                            frozen,
                            no_frozen,
                        },
                        cargo_args,
                        cargo_passthrough,
                    ),
                    package_features: package_features.into(),
                    sdk_profile: sdk_profile.sdk_profile,
                    cargo_features,
                    cargo_no_default_features,
                    cargo_all_features,
                    generated_cargo_target_dir,
                },
                report_options: BuildReportOptions {
                    format: report,
                    output_path: report_output,
                },
            },
            workspace,
            members,
        ),
        Some(Command::Check {
            path,
            format,
            package_features,
            sdk_profile,
            interop_target,
            workspace,
            members,
        }) => execute_check(
            path,
            format,
            package_features.into(),
            sdk_profile.sdk_profile,
            interop_target,
            workspace,
            members,
        ),
        Some(Command::Explain { code, format }) => commands::explain_diagnostic(&code, format),
        Some(Command::Inspect { command }) => match command {
            InspectCommand::Oven { receipt, store, format } => {
                commands::inspect_oven_receipt(commands::OvenReceiptInspectCommandOptions {
                    receipt,
                    store: store.into(),
                    format,
                })
            }
            InspectCommand::Rust { path, lib_mode, format } => commands::inspect_rust(&path, lib_mode, format),
            InspectCommand::Codegraph {
                path,
                format,
                allow_errors,
                package_features,
                sdk_profile,
            } => commands::inspect_codegraph(
                &path,
                format,
                allow_errors,
                &package_features.into(),
                sdk_profile.profile(),
            ),
            InspectCommand::Registry {
                identity,
                project,
                format,
            } => commands::tools::inspect_registry(&identity, project.as_deref(), format),
            InspectCommand::Providers {
                path,
                format,
                package_features,
                sdk_profile,
            } => commands::inspect_providers(&path, format, &package_features.into(), sdk_profile.profile()),
            InspectCommand::Features {
                path,
                format,
                package_features,
                sdk_profile,
            } => commands::inspect_features(&path, format, &package_features.into(), sdk_profile.profile()),
            InspectCommand::Bindings {
                path,
                format,
                package_features,
                sdk_profile,
            } => commands::inspect_bindings(&path, format, &package_features.into(), sdk_profile.profile()),
            InspectCommand::InteropPlan { path, target, format } => {
                commands::inspect_interop_plan(&path, &target, format)
            }
        },
        Some(Command::Run {
            file,
            command,
            package_features,
            sdk_profile,
            locked,
            offline,
            no_offline,
            frozen,
            no_frozen,
            no_locked,
            cargo_args,
            cargo_features,
            cargo_no_default_features,
            cargo_all_features,
            release,
            workspace,
            members,
            cargo_passthrough,
        }) => execute_workspace_run(
            RunInput { file, code: command },
            RunOptions {
                cargo_policy: CargoPolicy::from_cli_and_env(
                    CargoPolicyCliFlags {
                        offline,
                        no_offline,
                        locked,
                        no_locked,
                        frozen,
                        no_frozen,
                    },
                    cargo_args,
                    cargo_passthrough,
                ),
                package_features: package_features.into(),
                sdk_profile: sdk_profile.sdk_profile,
                cargo_features,
                cargo_no_default_features,
                cargo_all_features,
                release,
            },
            workspace,
            members,
        ),
        Some(Command::Fmt {
            path,
            check,
            diff,
            workspace,
            members,
        }) => execute_format(path, check, diff, workspace, members),
        Some(Command::Test {
            path,
            package_features,
            sdk_profile,
            verbose,
            stop_on_fail,
            slow,
            filter,
            marker_expr,
            strict_markers,
            jobs,
            test_features,
            timeout,
            no_capture,
            fail_on_empty,
            list_only,
            report_format,
            junit_path,
            durations,
            shuffle,
            seed,
            run_xfail,
            locked,
            offline,
            no_offline,
            frozen,
            no_frozen,
            no_locked,
            cargo_args,
            cargo_features,
            cargo_no_default_features,
            cargo_all_features,
            workspace,
            members,
            cargo_passthrough,
        }) => execute_tests(
            TestCommandOptions {
                path,
                verbose,
                stop_on_fail,
                include_slow: slow,
                filter,
                marker_expr,
                strict_markers,
                jobs,
                test_features,
                timeout,
                no_capture,
                use_color,
                fail_on_empty,
                list_only,
                report_format,
                junit_path,
                durations,
                shuffle,
                seed,
                run_xfail,
                package_features: package_features.into(),
                sdk_profile: sdk_profile.sdk_profile,
                cargo_policy: CargoPolicy::from_cli_and_env(
                    CargoPolicyCliFlags {
                        offline,
                        no_offline,
                        locked,
                        no_locked,
                        frozen,
                        no_frozen,
                    },
                    cargo_args,
                    cargo_passthrough,
                ),
                cargo_features,
                cargo_no_default_features,
                cargo_all_features,
            },
            workspace,
            members,
        ),
        Some(Command::Version {
            bump,
            set,
            dry_run,
            keep_prerelease,
            project,
            workspace,
            members,
        }) => execute_workspace_version(
            commands::lifecycle::VersionCommandOptions {
                bump,
                set,
                dry_run,
                keep_prerelease,
                project,
            },
            workspace,
            members,
        ),
        Some(Command::Env { command }) => match command {
            EnvCommand::List { format, project } => commands::env_list(format, project.as_deref()),
            EnvCommand::Show { env, format, project } => commands::env_show(env.as_deref(), format, project.as_deref()),
            EnvCommand::Run {
                env,
                script,
                dry_run,
                args,
                project,
            } => commands::env_run(&env, &script, dry_run, &args, project.as_deref()),
        },
        Some(Command::Workspace { command }) => match command {
            WorkspaceCommand::Inspect {
                format,
                workspace,
                members,
            } => commands::workspace_inspect(format, workspace, members),
        },
        Some(Command::Tools { command }) => match command {
            ToolsCommand::Doctor { format } => commands::tools_doctor(format),
            ToolsCommand::Metadata { command } => match command {
                ToolsMetadataCommand::Api { path, format } => commands::tools_metadata_api(&path, format),
                ToolsMetadataCommand::Model { path, model, format } => {
                    commands::tools_metadata_model(&path, &model, format)
                }
            },
        },
        Some(Command::Cache { command }) => match command {
            CacheCommand::Inspect { category, format } => {
                commands::inspect_generated_cache(category, matches!(format, CacheOutputFormat::Json))
            }
            CacheCommand::Prune {
                category,
                dry_run,
                max_bytes,
                identities,
                format,
            } => commands::prune_generated_cache(
                category,
                max_bytes,
                dry_run,
                &identities,
                matches!(format, CacheOutputFormat::Json),
            ),
        },
        Some(Command::Oven { command }) => match command {
            OvenCommand::Import {
                project,
                target,
                toolchain,
                profile,
                features,
                source_inputs,
                output,
                format,
            } => commands::oven_import(commands::OvenImportCommandOptions {
                project,
                target,
                toolchain,
                profile,
                features,
                source_inputs,
                output,
                format,
            }),
            OvenCommand::LegacyCargo { command } => match command {
                OvenLegacyCargoCommand::Prepare {
                    receipt,
                    generated_project,
                    cargo,
                    rustc,
                    domain,
                    store,
                    format,
                } => commands::oven_legacy_cargo_prepare(commands::OvenLegacyCargoPrepareCommandOptions {
                    receipt,
                    generated_project,
                    cargo,
                    rustc,
                    domain,
                    store: store.into(),
                    format,
                }),
                OvenLegacyCargoCommand::BakeLoafs {
                    compiler_root,
                    output,
                    suite_store,
                    envelope,
                    sdk_inventory,
                    cargo,
                    rustc,
                    max_physical_bytes,
                    max_domain_physical_bytes,
                    max_domain_logical_bytes,
                    format,
                } => commands::oven_legacy_cargo_bake_loafs(commands::OvenLoafBakeCommandOptions {
                    compiler_root,
                    output,
                    suite_store,
                    envelope,
                    sdk_inventory,
                    cargo,
                    rustc,
                    max_physical_bytes,
                    max_domain_physical_bytes,
                    max_domain_logical_bytes,
                    format,
                }),
            },
            OvenCommand::CompilerLibtests {
                compiler_root,
                rustc,
                features,
                targets,
                output,
                store,
                format,
            } => commands::oven_run_compiler_libtests(commands::OvenCompilerLibtestsRunCommandOptions {
                compiler_root,
                rustc,
                features,
                targets,
                output,
                store: store.into(),
                format,
            }),
            OvenCommand::Plan { command } => match command {
                OvenPlanCommand::Publish {
                    receipt,
                    manifest,
                    artifact_root,
                    domain,
                    store,
                    format,
                } => commands::oven_publish_direct_rustc_plan(commands::OvenPlanPublishCommandOptions {
                    receipt,
                    manifest,
                    artifact_root,
                    domain,
                    store: store.into(),
                    format,
                }),
            },
            OvenCommand::Store { command } => match command {
                OvenStoreCommand::Inspect { store, format } => commands::inspect_oven_store(store.into(), format),
                OvenStoreCommand::Prune { store, dry_run, format } => {
                    commands::prune_oven_store(store.into(), dry_run, format)
                }
            },
            OvenCommand::Test {
                receipt,
                plan_identity,
                rustc,
                source,
                output,
                crate_name,
                edition,
                source_evidence_key,
                exact_names,
                store,
                format,
            } => commands::oven_test(commands::OvenTestCommandOptions {
                receipt,
                plan_identity,
                rustc,
                source,
                output,
                crate_name,
                edition,
                source_evidence_key,
                exact_names,
                store: store.into(),
                format,
            }),
            OvenCommand::Run {
                receipt,
                plan_identity,
                rustc,
                source,
                output,
                crate_name,
                edition,
                source_evidence_key,
                arguments,
                store,
                format,
            } => commands::oven_run(commands::OvenRunCommandOptions {
                receipt,
                plan_identity,
                rustc,
                source,
                output,
                crate_name,
                edition,
                source_evidence_key,
                arguments,
                store: store.into(),
                format,
            }),
        },
        Some(Command::New {
            name,
            dir,
            description,
            author,
            license,
            force,
            yes,
        }) => commands::init::new_project(commands::init::NewOptions {
            name: name.as_deref(),
            dir: dir.as_deref(),
            description: description.as_deref(),
            author: author.as_deref(),
            license: license.as_deref(),
            force,
            yes,
        }),
        Some(Command::Init {
            path,
            name,
            version,
            description,
            author,
            license,
            force,
            detect,
            yes,
        }) => commands::init_project(
            &path,
            commands::init::InitOptions {
                name: name.as_deref(),
                version: &version,
                description: description.as_deref(),
                author: author.as_deref(),
                license: license.as_deref(),
                force,
                yes,
                detect,
            },
        ),
        Some(Command::Lock {
            file,
            package_features,
            sdk_profile,
            cargo_features,
            cargo_no_default_features,
            cargo_all_features,
        }) => commands::lock_project(
            file.as_ref(),
            &package_features.into(),
            sdk_profile.profile(),
            cargo_features,
            cargo_no_default_features,
            cargo_all_features,
        ),
        None => {
            // Default: type check the file if provided
            if let Some(file) = cli.file {
                commands::check_path(&file, DiagnosticOutputFormat::Text)
            } else {
                // No command and no file - show help
                Err(CliError::new(render_cli_help_text(), ExitCode::FAILURE))
            }
        }
    }
}

/// Resolve the active RFC 077 scope once for a command, retaining compiler-owned selection identity after graph
/// discovery has finished. Commands without workspace selectors preserve their historical single-project behavior
/// when discovery finds no containing workspace.
fn resolve_workspace_command_scope(
    select_workspace: bool,
    member_selectors: &[String],
) -> CliResult<Option<ResolvedWorkspaceScope>> {
    let current_dir = env::current_dir()
        .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?;
    let workspace = WorkspaceGraph::discover(&current_dir).map_err(|error| CliError::failure(error.to_string()))?;
    let Some(workspace) = workspace else {
        if select_workspace || !member_selectors.is_empty() {
            return Err(CliError::failure(
                "--workspace and --member require an active RFC 077 workspace",
            ));
        }
        return Ok(None);
    };
    let selection = workspace
        .resolve_scope(WorkspaceScopeRequest::new(
            &current_dir,
            select_workspace,
            member_selectors,
        ))
        .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(Some(selection.to_owned_scope()))
}

/// Owned build inputs that can be applied once per compiler-selected workspace member.
struct BuildCommandRequest {
    file: Option<PathBuf>,
    lib_mode: bool,
    output_dir: Option<String>,
    options: commands::build::BuildCommandOptions,
    report_options: BuildReportOptions,
}

impl BuildCommandRequest {
    /// Preserve the existing single-project build behavior when workspace discovery is inactive.
    fn run_single(self) -> CliResult<ExitCode> {
        if self.lib_mode {
            let file = self.file.as_ref().map(|path| path.to_string_lossy().to_string());
            return commands::build::build_library(
                file.as_deref(),
                self.output_dir.as_ref(),
                self.options,
                self.report_options,
            );
        }
        let file = resolve_build_entry_file(self.file)?;
        commands::build_file(
            &file.to_string_lossy(),
            self.output_dir.as_ref(),
            self.options,
            self.report_options,
        )
    }
}

/// Return whether this build should resolve and fan out an RFC 077 workspace scope.
///
/// Compiler-spawned dependency library builds target one dependency project even when it owns a workspace.
///
/// Artifact-only children and Oven direct-rustc children differ in what they emit, but neither may rediscover the
/// default member scope: that would make a root package also build unrelated workspace members. Ordinary library and
/// executable builds retain workspace selection semantics.
fn build_uses_workspace_scope(lib_mode: bool, artifact_only: bool, dependency_preparation: bool) -> bool {
    !lib_mode || !(artifact_only || dependency_preparation)
}

/// Fan out builds after resolving the exact RFC 077 member set, producing one aggregate report when JSON is requested.
///
/// Internal artifact-only library children bypass workspace selection because their current directory is the exact
/// dependency project selected by the parent compiler process.
fn execute_build(
    request: BuildCommandRequest,
    select_workspace: bool,
    member_selectors: Vec<String>,
) -> CliResult<ExitCode> {
    let artifact_only = env::var_os(INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV).is_some();
    let dependency_preparation =
        env::var_os(INTERNAL_LIBRARY_DEPENDENCY_PREPARATION_ENV).is_some_and(|value| value == "1");
    if !build_uses_workspace_scope(request.lib_mode, artifact_only, dependency_preparation) {
        return request.run_single();
    }

    let Some(scope) = resolve_workspace_command_scope(select_workspace, &member_selectors)? else {
        return request.run_single();
    };
    if !scope.is_single_member() && request.output_dir.is_some() {
        return Err(CliError::failure(
            "an explicit OUTPUT_DIR requires exactly one workspace member; select one with --member <name-or-path>",
        ));
    }

    let mut results = Vec::new();
    let mut failures = Vec::new();
    for member in scope.members() {
        if request.report_options.enabled() {
            eprintln!("workspace member {}: {}", member.name(), member.root().display());
        } else {
            println!("workspace member {}: {}", member.name(), member.root().display());
        }

        let target = if request.lib_mode {
            match request.file.as_ref() {
                Some(path) => workspace_member_relative_path(path, member.root()),
                None => Ok(member.root().to_path_buf()),
            }
        } else {
            match request.file.as_ref() {
                Some(path) => workspace_member_relative_path(path, member.root()),
                None => workspace_member_main_script_target(member, "build"),
            }
        };
        let target = match target {
            Ok(target) => target,
            Err(error) => {
                failures.push(member.name().to_string());
                results.push(serde_json::json!({
                    "member": {
                        "name": member.name(),
                        "root": member.root().display().to_string(),
                    },
                    "ok": false,
                    "error": error.message,
                }));
                continue;
            }
        };

        let report = if request.lib_mode {
            commands::build::build_library_report(
                Some(target.to_string_lossy().as_ref()),
                request.output_dir.as_ref(),
                request.options.clone(),
                &request.report_options,
            )
        } else {
            commands::build::build_file_report(
                target.to_string_lossy().as_ref(),
                request.output_dir.as_ref(),
                request.options.clone(),
                &request.report_options,
            )
        };
        match report {
            Ok(report) => {
                let report = report.with_workspace_context(commands::build_report::BuildWorkspaceContext {
                    root: scope.workspace_root().display().to_string(),
                    scope_origin: scope.origin().as_str().to_string(),
                    member_name: member.name().to_string(),
                    member_root: member.root().display().to_string(),
                });
                results.push(serde_json::json!({
                    "member": {
                        "name": member.name(),
                        "root": member.root().display().to_string(),
                    },
                    "ok": true,
                    "report": report,
                }));
            }
            Err(error) => {
                if !request.report_options.enabled() && !error.message.is_empty() {
                    eprintln!("{}", error.message);
                }
                failures.push(member.name().to_string());
                results.push(serde_json::json!({
                    "member": {
                        "name": member.name(),
                        "root": member.root().display().to_string(),
                    },
                    "ok": false,
                    "error": error.message,
                }));
            }
        }
    }

    let aggregate = serde_json::json!({
        "schema_version": "incan.workspace.build.v1",
        "workspace": {
            "root": scope.workspace_root().display().to_string(),
            "selected_scope": {
                "origin": scope.origin().as_str(),
                "members": scope.members().map(|member| serde_json::json!({
                    "name": member.name(),
                    "root": member.root().display().to_string(),
                })).collect::<Vec<_>>(),
            },
        },
        "ok": failures.is_empty(),
        "results": results,
    });
    commands::build_report::emit_workspace_build_report(&aggregate, &request.report_options)?;

    if failures.is_empty() {
        Ok(ExitCode::SUCCESS)
    } else {
        Err(CliError::new("", ExitCode::FAILURE))
    }
}

struct WorkspaceCheckMemberOutcome {
    member: WorkspaceMember,
    target: Option<PathBuf>,
    report: Option<commands::diagnostics::DiagnosticReport>,
    error: Option<String>,
}

/// Run `incan check` across the compiler-selected workspace scope, preserving a single JSON document for tooling.
fn execute_check(
    path: PathBuf,
    format: DiagnosticOutputFormat,
    package_features: FeatureSelection,
    sdk_profile: Option<String>,
    interop_target: Option<String>,
    select_workspace: bool,
    member_selectors: Vec<String>,
) -> CliResult<ExitCode> {
    let Some(scope) = resolve_workspace_command_scope(select_workspace, &member_selectors)? else {
        return commands::diagnostics::check_path_with_interop_target_selection(
            &path,
            format,
            &package_features,
            sdk_profile.as_deref(),
            interop_target.as_deref(),
        );
    };

    let mut outcomes = Vec::new();
    for member in scope.members() {
        let target = match workspace_member_check_target(&path, member) {
            Ok(target) => target,
            Err(error) => {
                outcomes.push(WorkspaceCheckMemberOutcome {
                    member: member.clone(),
                    target: None,
                    report: None,
                    error: Some(error.message),
                });
                continue;
            }
        };
        match commands::diagnostics::check_path_report_with_interop_target_selection(
            &target,
            &package_features,
            sdk_profile.as_deref(),
            interop_target.as_deref(),
        ) {
            Ok(report) => outcomes.push(WorkspaceCheckMemberOutcome {
                member: member.clone(),
                target: Some(target),
                report: Some(report),
                error: None,
            }),
            Err(error) => outcomes.push(WorkspaceCheckMemberOutcome {
                member: member.clone(),
                target: Some(target),
                report: None,
                error: Some(error.message),
            }),
        }
    }

    let ok = outcomes
        .iter()
        .all(|outcome| outcome.error.is_none() && outcome.report.as_ref().is_some_and(|report| report.ok()));

    match format {
        DiagnosticOutputFormat::Text => {
            for outcome in &outcomes {
                let target = outcome
                    .target
                    .as_ref()
                    .map(|target| target.display().to_string())
                    .unwrap_or_else(|| "<unresolved>".to_string());
                println!("workspace member {}: {target}", outcome.member.name());
                if let Some(report) = &outcome.report {
                    if report.ok() {
                        println!("✓ Type check passed!");
                    } else {
                        eprint!(
                            "{}",
                            report
                                .human_message()
                                .unwrap_or("type check failed without diagnostics\n")
                        );
                    }
                }
                if let Some(error) = &outcome.error {
                    eprintln!("{error}");
                }
            }
        }
        DiagnosticOutputFormat::Json => {
            let results = outcomes
                .iter()
                .map(|outcome| {
                    let mut result = serde_json::json!({
                        "member": {
                            "name": outcome.member.name(),
                            "root": outcome.member.root().display().to_string(),
                        },
                        "target": outcome.target.as_ref().map(|target| target.display().to_string()),
                    });
                    if let Some(object) = result.as_object_mut() {
                        if let Some(report) = &outcome.report {
                            object.insert("report".to_string(), serde_json::json!(report));
                        }
                        if let Some(error) = &outcome.error {
                            object.insert("ok".to_string(), serde_json::Value::Bool(false));
                            object.insert("error".to_string(), serde_json::Value::String(error.clone()));
                        }
                    }
                    result
                })
                .collect::<Vec<_>>();
            let report = serde_json::json!({
                "schema_version": "incan.workspace.check.v1",
                "workspace": {
                    "root": scope.workspace_root().display().to_string(),
                    "selected_scope": {
                        "origin": scope.origin().as_str(),
                        "members": scope.members().map(|member| serde_json::json!({
                            "name": member.name(),
                            "root": member.root().display().to_string(),
                        })).collect::<Vec<_>>(),
                    },
                },
                "ok": ok,
                "results": results,
            });
            let rendered = serde_json::to_string_pretty(&report)
                .map_err(|error| CliError::failure(format!("failed to serialize workspace check report: {error}")))?;
            println!("{rendered}");
        }
    }

    if ok {
        Ok(ExitCode::SUCCESS)
    } else {
        Err(CliError::new("", ExitCode::FAILURE))
    }
}

/// Resolve one member's check target. An omitted path means the member's configured `main` script, never the
/// workspace root, so a virtual workspace cannot accidentally compile a non-member directory.
fn workspace_member_check_target(path: &Path, member: &WorkspaceMember) -> CliResult<PathBuf> {
    if path != Path::new(".") {
        return workspace_member_relative_path(path, member.root());
    }
    workspace_member_main_script_target(member, "check")
}

/// Resolve a member's configured `main` script without treating a virtual workspace root as a project entrypoint.
fn workspace_member_main_script_target(member: &WorkspaceMember, command_name: &str) -> CliResult<PathBuf> {
    let manifest = commands::common::discover_effective_project_manifest(member.root())?.ok_or_else(|| {
        CliError::failure(format!(
            "workspace member `{}` has no project manifest available for `incan {command_name}`",
            member.name()
        ))
    })?;
    let main = manifest
        .project
        .as_ref()
        .and_then(|project| project.scripts.get("main"))
        .ok_or_else(|| {
            CliError::failure(format!(
                "workspace member `{}` has no [project.scripts].main; pass a member-relative PATH to `incan {command_name}`",
                member.name()
            ))
        })?;
    Ok(manifest.project_root().join(main))
}

/// Owned test command inputs that can be replayed once for each validated workspace member.
struct TestCommandOptions {
    path: PathBuf,
    verbose: bool,
    stop_on_fail: bool,
    include_slow: bool,
    filter: Option<String>,
    marker_expr: Option<String>,
    strict_markers: bool,
    jobs: usize,
    test_features: Vec<String>,
    timeout: Option<String>,
    no_capture: bool,
    use_color: bool,
    fail_on_empty: bool,
    list_only: bool,
    report_format: test_runner::TestOutputFormat,
    junit_path: Option<PathBuf>,
    durations: Option<usize>,
    shuffle: bool,
    seed: Option<u64>,
    run_xfail: bool,
    package_features: FeatureSelection,
    sdk_profile: Option<String>,
    cargo_policy: CargoPolicy,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
}

impl TestCommandOptions {
    /// Run one member-local test batch without asking the runner to rediscover workspace topology.
    fn run_for(
        &self,
        path: &Path,
        workspace_context: Option<test_runner::WorkspaceTestContext>,
    ) -> CliResult<ExitCode> {
        let path = path.to_string_lossy();
        test_runner::run_tests(test_runner::TestRunConfig {
            path: &path,
            verbose: self.verbose,
            stop_on_fail: self.stop_on_fail,
            include_slow: self.include_slow,
            filter: self.filter.as_deref(),
            marker_expr: self.marker_expr.as_deref(),
            strict_markers: self.strict_markers,
            jobs: self.jobs,
            test_features: self.test_features.clone(),
            package_features: self.package_features.clone(),
            sdk_profile: self.sdk_profile.clone(),
            timeout: self.timeout.as_deref(),
            no_capture: self.no_capture,
            use_color: self.use_color,
            fail_on_empty: self.fail_on_empty,
            list_only: self.list_only,
            report_format: self.report_format,
            junit_path: self.junit_path.clone(),
            durations: self.durations,
            shuffle: self.shuffle,
            seed: self.seed,
            run_xfail: self.run_xfail,
            cargo_policy: self.cargo_policy.clone(),
            cargo_features: self.cargo_features.clone(),
            cargo_no_default_features: self.cargo_no_default_features,
            cargo_all_features: self.cargo_all_features,
            workspace_context,
        })
    }
}

/// Fan out test batches only after compiler-owned workspace scope selection has completed.
fn execute_tests(
    options: TestCommandOptions,
    select_workspace: bool,
    member_selectors: Vec<String>,
) -> CliResult<ExitCode> {
    let Some(scope) = resolve_workspace_command_scope(select_workspace, &member_selectors)? else {
        return options.run_for(&options.path, None);
    };
    if !scope.is_single_member() && options.junit_path.is_some() {
        return Err(CliError::failure(
            "--junit requires exactly one workspace member; select one with --member <name-or-path>",
        ));
    }

    if options.report_format == test_runner::TestOutputFormat::Json {
        println!(
            "{}",
            serde_json::json!({
                "schema_version": "incan.test.v1",
                "event": "workspace_scope",
                "workspace": {
                    "root": scope.workspace_root().display().to_string(),
                    "selected_scope": {
                        "origin": scope.origin().as_str(),
                        "members": scope.members().map(|member| serde_json::json!({
                            "name": member.name(),
                            "root": member.root().display().to_string(),
                        })).collect::<Vec<_>>(),
                    },
                },
            })
        );
    }

    let mut failures = Vec::new();
    for member in scope.members() {
        let target = match workspace_member_relative_path(&options.path, member.root()) {
            Ok(target) => target,
            Err(error) => {
                failures.push(format!("{}: {}", member.name(), error.message));
                continue;
            }
        };
        if options.report_format == test_runner::TestOutputFormat::Console {
            println!("workspace member {}: {}", member.name(), target.display());
        }
        let workspace_context = test_runner::WorkspaceTestContext {
            workspace_root: scope.workspace_root().to_path_buf(),
            scope_origin: scope.origin().as_str().to_string(),
            member_name: member.name().to_string(),
            member_root: member.root().to_path_buf(),
        };
        if let Err(error) = options.run_for(&target, Some(workspace_context)) {
            if options.report_format == test_runner::TestOutputFormat::Json {
                println!(
                    "{}",
                    serde_json::json!({
                        "schema_version": "incan.test.v1",
                        "event": "workspace_member_error",
                        "workspace": {
                            "root": scope.workspace_root().display().to_string(),
                            "scope_origin": scope.origin().as_str(),
                            "member": {
                                "name": member.name(),
                                "root": member.root().display().to_string(),
                            },
                        },
                        "error": error.message,
                    })
                );
            } else if !error.message.is_empty() {
                eprintln!("{}", error.message);
            }
            failures.push(member.name().to_string());
        }
    }

    if failures.is_empty() {
        Ok(ExitCode::SUCCESS)
    } else {
        Err(CliError::new("", ExitCode::FAILURE))
    }
}

/// Apply `incan fmt` to the deterministic RFC 077 member scope when a workspace is active.
fn execute_format(
    path: PathBuf,
    check: bool,
    diff: bool,
    select_workspace: bool,
    member_selectors: Vec<String>,
) -> CliResult<ExitCode> {
    let Some(scope) = resolve_workspace_command_scope(select_workspace, &member_selectors)? else {
        return commands::format_files(&path.to_string_lossy(), check, diff);
    };
    let mut failures = Vec::new();
    for member in scope.members() {
        let target = workspace_member_relative_path(&path, member.root())?;
        println!("workspace member {}: {}", member.name(), target.display());
        if let Err(error) = commands::format_files(&target.to_string_lossy(), check, diff) {
            let message = if error.message.is_empty() {
                "formatting failed".to_string()
            } else {
                error.message
            };
            failures.push(format!("{}: {message}", member.name()));
        }
    }
    if failures.is_empty() {
        Ok(ExitCode::SUCCESS)
    } else {
        Err(CliError::failure(format!(
            "workspace formatting failed\n{}",
            failures.join("\n")
        )))
    }
}

/// Resolve a formatting path against one selected member without allowing a multi-member command to escape scope.
fn workspace_member_relative_path(path: &Path, member_root: &Path) -> CliResult<PathBuf> {
    if path == Path::new(".") {
        return Ok(member_root.to_path_buf());
    }
    if path.is_absolute() {
        if path.starts_with(member_root) {
            return Ok(path.to_path_buf());
        }
        return Err(CliError::failure(format!(
            "format path {} is outside selected workspace member {}",
            path.display(),
            member_root.display()
        )));
    }
    Ok(member_root.join(path))
}

/// Render top-level CLI help text.
fn render_cli_help_text() -> String {
    let mut command = Cli::command();
    let mut out = Vec::new();
    if command.write_help(&mut out).is_ok() {
        String::from_utf8_lossy(&out).to_string()
    } else {
        "Run `incan --help` for usage.".to_string()
    }
}

struct RunInput {
    file: Option<PathBuf>,
    code: Option<String>,
}

struct RunOptions {
    cargo_policy: CargoPolicy,
    package_features: FeatureSelection,
    sdk_profile: Option<String>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    release: bool,
}

/// Resolve an explicit file or the project `main` script for project-aware commands.
fn resolve_main_script_entry_file(
    file: Option<PathBuf>,
    command_name: &str,
    explicit_target: &str,
) -> CliResult<PathBuf> {
    if let Some(file) = file {
        return Ok(file);
    }

    let cwd =
        env::current_dir().map_err(|e| CliError::failure(format!("Error: failed to read current directory: {e}")))?;
    let manifest = ProjectManifest::discover(&cwd).map_err(|e| CliError::failure(e.to_string()))?;

    if let Some(manifest) = manifest
        && let Some(project) = &manifest.project
        && let Some(main) = project.scripts.get("main")
    {
        return Ok(manifest.project_root().join(main));
    }

    Err(CliError::failure(format!(
        "Error: {command_name} requires {explicit_target} or [project.scripts].main"
    )))
}

/// Resolve the build target for `incan build`, falling back to project metadata when available.
fn resolve_build_entry_file(file: Option<PathBuf>) -> CliResult<PathBuf> {
    resolve_main_script_entry_file(file, "build", "FILE unless `--lib` is set")
}

/// Resolve the run target for `incan run`, falling back to project metadata when available.
fn resolve_run_entry_file(file: Option<PathBuf>) -> CliResult<PathBuf> {
    resolve_main_script_entry_file(file, "run", "a file path, -c/--command")
}

/// Handle the `run` subcommand with its various forms.
/// Resolve RFC 077 scope before delegating a file-backed run to the existing single-project pipeline.
fn execute_workspace_run(
    input: RunInput,
    opts: RunOptions,
    select_workspace: bool,
    member_selectors: Vec<String>,
) -> CliResult<ExitCode> {
    if input.code.is_some() && (select_workspace || !member_selectors.is_empty()) {
        return Err(CliError::failure(
            "incan run -c/--command cannot select a workspace member; pass a member source file instead",
        ));
    }
    if input.code.is_some() {
        return execute_run(input, opts);
    }
    let Some(scope) = resolve_workspace_command_scope(select_workspace, &member_selectors)? else {
        return execute_run(input, opts);
    };
    let member = scope
        .require_single_member("incan run")
        .map_err(|error| CliError::failure(error.to_string()))?;
    let file = match input.file {
        Some(path) => workspace_member_relative_path(&path, member.root())?,
        None => workspace_member_main_script_target(member, "run")?,
    };
    execute_run(
        RunInput {
            file: Some(file),
            code: None,
        },
        opts,
    )
}

/// Resolve a single RFC 077 member before mutating its project version.
fn execute_workspace_version(
    mut options: commands::lifecycle::VersionCommandOptions,
    select_workspace: bool,
    member_selectors: Vec<String>,
) -> CliResult<ExitCode> {
    let Some(scope) = resolve_workspace_command_scope(select_workspace, &member_selectors)? else {
        return commands::version_project(options);
    };
    let member = scope
        .require_single_member("incan version")
        .map_err(|error| CliError::failure(error.to_string()))?;
    options.project = Some(member.root().to_path_buf());
    commands::version_project(options)
}

/// Compile and execute one run request.
fn execute_run(input: RunInput, opts: RunOptions) -> CliResult<ExitCode> {
    // ---- Context: inline source execution (`incan run -c ...`) ----
    if let Some(code) = input.code {
        if code.is_empty() {
            return Err(CliError::failure("Error: -c/--command requires source code string"));
        }
        commands::run_inline_source(
            &code,
            opts.cargo_policy.clone(),
            opts.package_features.clone(),
            opts.sdk_profile.clone(),
            opts.cargo_features.clone(),
            opts.cargo_no_default_features,
            opts.cargo_all_features,
            opts.release,
        )
    // ---- Context: file execution (`incan run path/to/file.incn`) ----
    } else {
        let file = resolve_run_entry_file(input.file)?;
        commands::run_file(
            &file.to_string_lossy(),
            opts.cargo_policy,
            opts.package_features,
            opts.sdk_profile,
            opts.cargo_features,
            opts.cargo_no_default_features,
            opts.cargo_all_features,
            opts.release,
        )
    }
}

/// Print the ASCII logo banner to stderr (colored or not)
fn print_logo(use_color: bool) {
    // Color scheme inspired by the wordmark:
    // - Solid blocks (█) = Gold
    // - Shadow blocks (░) = Cyan/Magenta based on position
    let gold = "\x1b[1;33m";
    let cyan = "\x1b[1;36m";
    let magenta = "\x1b[1;35m";
    let reset = "\x1b[0m";

    for line in LOGO.lines() {
        let mut colored_line = String::new();
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len();

        for (i, ch) in chars.iter().enumerate() {
            if use_color {
                let color = if *ch == '░' {
                    // Shadow chars: cyan on left half, magenta on right half (diagonal effect)
                    if i < len / 2 { cyan } else { magenta }
                } else {
                    // Solid blocks and all other characters get gold
                    gold
                };
                colored_line.push_str(color);
                colored_line.push(*ch);
            } else {
                colored_line.push(*ch);
            }
        }
        if use_color {
            eprintln!("{}{}", colored_line, reset);
        } else {
            eprintln!("{}", colored_line);
        }
    }
}

/// Decide whether ANSI color output is enabled.
///
/// Note: `NO_COLOR` only affects `ColorMode::Auto`; explicit user flags (`--color=always` / `--color=never`) override
/// the environment.
fn should_use_color(color: ColorMode) -> bool {
    match color {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => {
            if env::var_os("NO_COLOR").is_some() {
                return false;
            }
            io::stdout().is_terminal() && io::stderr().is_terminal()
        }
    }
}

/// Decide whether this command should show the banner when running interactively.
fn command_prefers_banner(cli: &Cli) -> bool {
    matches!(cli.command, Some(Command::Build { .. }) | Some(Command::Run { .. }))
}

/// Decide whether to print the ASCII logo banner.
///
/// Banner suppression (`--no-banner` / `INCAN_NO_BANNER`) always wins.
/// Banners are also suppressed when output is not a TTY (script-friendly).
/// By default, branding is shown only for interactive `build` and `run` flows.
fn should_print_banner(cli: &Cli, _use_color: bool) -> bool {
    if cli.no_banner || env::var_os("INCAN_NO_BANNER").is_some() {
        return false;
    }

    if !command_prefers_banner(cli) {
        return false;
    }

    if !io::stdout().is_terminal() || !io::stderr().is_terminal() {
        return false;
    }

    true
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    fn parse_cli(args: impl IntoIterator<Item = &'static str>) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(args)
    }

    fn expected_command(name: &str) -> clap::Error {
        clap::Error::raw(ErrorKind::InvalidSubcommand, format!("expected {name} command"))
    }

    #[test]
    fn test_cli_parse_build() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "build", "test.incn"])?;
        let Some(Command::Build { file, lib_mode, .. }) = cli.command else {
            return Err(expected_command("build"));
        };
        assert_eq!(file, Some(PathBuf::from("test.incn")));
        assert!(!lib_mode);
        Ok(())
    }

    #[test]
    fn test_cli_parse_build_lib() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "build", "--lib"])?;
        let Some(Command::Build { file, lib_mode, .. }) = cli.command else {
            return Err(expected_command("build"));
        };
        assert!(file.is_none());
        assert!(lib_mode);
        Ok(())
    }

    #[test]
    fn test_cli_parse_generated_cache_commands() -> Result<(), clap::Error> {
        let inspect = parse_cli(["incan", "cache", "inspect", "--format", "json"])?;
        assert!(matches!(
            inspect.command,
            Some(Command::Cache {
                command: CacheCommand::Inspect {
                    category: CacheCategory::GeneratedCargo,
                    format: CacheOutputFormat::Json
                }
            })
        ));

        let prune = parse_cli(["incan", "cache", "prune", "--dry-run", "--max-bytes", "1048576"])?;
        let Some(Command::Cache {
            command:
                CacheCommand::Prune {
                    category,
                    dry_run,
                    max_bytes,
                    identities,
                    format,
                },
        }) = prune.command
        else {
            return Err(expected_command("cache prune"));
        };
        assert_eq!(category, CacheCategory::GeneratedCargo);
        assert!(dry_run);
        assert_eq!(max_bytes, Some(1_048_576));
        assert!(identities.is_empty());
        assert_eq!(format, CacheOutputFormat::Text);

        let selective = parse_cli([
            "incan",
            "cache",
            "prune",
            "--category",
            "generated-cargo",
            "--identity",
            "abc",
            "--identity",
            "def",
        ])?;
        assert!(matches!(
            selective.command,
            Some(Command::Cache {
                command: CacheCommand::Prune {
                    category: CacheCategory::GeneratedCargo,
                    identities,
                    ..
                }
            }) if identities == ["abc", "def"]
        ));
        Ok(())
    }

    #[test]
    fn test_cli_parse_oven_commands() -> Result<(), clap::Error> {
        let import = parse_cli([
            "incan",
            "oven",
            "import",
            "--target",
            "aarch64-apple-darwin",
            "--toolchain",
            "rustc 1.96.0",
            "--source",
            "generated=target/oven/test.rs",
        ])?;
        assert!(matches!(
            import.command,
            Some(Command::Oven {
                command: OvenCommand::Import {
                    target,
                    toolchain,
                    source_inputs,
                    ..
                }
            }) if target == "aarch64-apple-darwin"
                && toolchain == "rustc 1.96.0"
                && source_inputs == ["generated=target/oven/test.rs"]
        ));

        let test = parse_cli([
            "incan",
            "oven",
            "test",
            "--receipt",
            "receipt.json",
            "--plan",
            "sha256:plan",
            "--rustc",
            "rustc",
            "--source",
            "generated.rs",
            "--output",
            "native-tests",
            "--crate-name",
            "native_tests",
            "--source-evidence",
            "generated",
            "--exact",
            "smoke",
            "--max-physical-bytes",
            "1048576",
        ])?;
        let Some(Command::Oven {
            command:
                OvenCommand::Test {
                    plan_identity,
                    exact_names,
                    store,
                    ..
                },
        }) = test.command
        else {
            return Err(expected_command("oven test"));
        };
        assert_eq!(plan_identity, "sha256:plan");
        assert_eq!(exact_names, ["smoke"]);
        assert_eq!(store.max_physical_bytes, Some(1_048_576));

        let store_prune = parse_cli([
            "incan",
            "oven",
            "store",
            "prune",
            "--dry-run",
            "--max-physical-bytes",
            "1048576",
        ])?;
        assert!(matches!(
            store_prune.command,
            Some(Command::Oven {
                command: OvenCommand::Store {
                    command: OvenStoreCommand::Prune { dry_run: true, store, .. },
                },
            }) if store.max_physical_bytes == Some(1_048_576)
        ));

        let run = parse_cli([
            "incan",
            "oven",
            "run",
            "--receipt",
            "receipt.json",
            "--plan",
            "sha256:plan",
            "--rustc",
            "rustc",
            "--source",
            "generated.rs",
            "--output",
            "native-run",
            "--crate-name",
            "native_run",
            "--source-evidence",
            "generated",
            "--",
            "--consumer-flag",
        ])?;
        let Some(Command::Oven {
            command:
                OvenCommand::Run {
                    plan_identity,
                    arguments,
                    ..
                },
        }) = run.command
        else {
            return Err(expected_command("oven run"));
        };
        assert_eq!(plan_identity, "sha256:plan");
        assert_eq!(arguments, [OsString::from("--consumer-flag")]);
        Ok(())
    }

    #[test]
    fn internal_library_preparation_bypasses_workspace_scope_issue908() {
        assert!(!build_uses_workspace_scope(true, true, false));
        assert!(!build_uses_workspace_scope(true, false, true));
        assert!(build_uses_workspace_scope(true, false, false));
        assert!(build_uses_workspace_scope(false, true, true));
    }

    #[test]
    fn test_cli_parse_build_cargo_policy_and_args() -> Result<(), clap::Error> {
        let cli = parse_cli([
            "incan",
            "build",
            "test.incn",
            "--offline",
            "--locked",
            "--cargo-args",
            "--timings",
            "--color=always",
        ])?;
        let Some(Command::Build {
            offline,
            locked,
            frozen,
            no_offline,
            no_locked,
            no_frozen,
            cargo_args,
            ..
        }) = cli.command
        else {
            return Err(expected_command("build"));
        };
        assert!(offline);
        assert!(locked);
        assert!(!frozen);
        assert!(!no_offline);
        assert!(!no_locked);
        assert!(!no_frozen);
        assert_eq!(cargo_args, vec!["--timings", "--color=always"]);
        Ok(())
    }

    #[test]
    fn test_cli_keeps_incan_and_cargo_feature_flags_separate() -> Result<(), clap::Error> {
        let cli = parse_cli([
            "incan",
            "build",
            "test.incn",
            "--features",
            "json,http",
            "--no-default-features",
            "--cargo-features",
            "serde,tokio",
        ])?;
        let Some(Command::Build {
            package_features,
            cargo_features,
            cargo_no_default_features,
            ..
        }) = cli.command
        else {
            return Err(expected_command("build"));
        };
        assert_eq!(package_features.features, ["json", "http"]);
        assert!(package_features.no_default_features);
        assert!(!package_features.all_features);
        assert_eq!(cargo_features, ["serde", "tokio"]);
        assert!(!cargo_no_default_features);
        Ok(())
    }

    #[test]
    fn test_cli_parse_build_generated_cargo_target_dir() -> Result<(), clap::Error> {
        let cli = parse_cli([
            "incan",
            "build",
            "--lib",
            "--generated-cargo-target-dir",
            "target/generated-shared",
        ])?;
        let Some(Command::Build {
            generated_cargo_target_dir,
            ..
        }) = cli.command
        else {
            return Err(expected_command("build"));
        };
        assert_eq!(
            generated_cargo_target_dir,
            Some(PathBuf::from("target/generated-shared"))
        );
        Ok(())
    }

    #[test]
    fn test_cli_parse_policy_negative_flags() -> Result<(), clap::Error> {
        let cli = parse_cli([
            "incan",
            "build",
            "test.incn",
            "--no-offline",
            "--no-locked",
            "--no-frozen",
        ])?;
        let Some(Command::Build {
            no_offline,
            no_locked,
            no_frozen,
            ..
        }) = cli.command
        else {
            return Err(expected_command("build"));
        };
        assert!(no_offline);
        assert!(no_locked);
        assert!(no_frozen);
        Ok(())
    }

    #[test]
    fn test_cli_parse_run() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "run", "test.incn"])?;
        let Some(Command::Run { release, .. }) = cli.command else {
            return Err(expected_command("run"));
        };
        assert!(!release, "run should default to debug profile");
        Ok(())
    }

    #[test]
    fn test_cli_parse_new() -> Result<(), clap::Error> {
        let cli = parse_cli([
            "incan",
            "new",
            "demo",
            "--dir",
            "apps/demo",
            "--description",
            "Demo app",
            "--author",
            "Danny <danny@example.com>",
            "--license",
            "MIT",
            "-y",
        ])?;
        let Some(Command::New {
            name,
            dir,
            description,
            author,
            license,
            yes,
            ..
        }) = cli.command
        else {
            return Err(expected_command("new"));
        };
        assert_eq!(name.as_deref(), Some("demo"));
        assert_eq!(dir, Some(PathBuf::from("apps/demo")));
        assert_eq!(description.as_deref(), Some("Demo app"));
        assert_eq!(author.as_deref(), Some("Danny <danny@example.com>"));
        assert_eq!(license.as_deref(), Some("MIT"));
        assert!(yes);
        Ok(())
    }

    #[test]
    fn test_cli_parse_new_without_name_for_interactive_mode() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "new"])?;
        let Some(Command::New { name, dir, .. }) = cli.command else {
            return Err(expected_command("new"));
        };
        assert!(name.is_none());
        assert!(dir.is_none());
        Ok(())
    }

    #[test]
    fn test_cli_parse_new_rejects_unsupported_project_kind_flags() {
        assert!(parse_cli(["incan", "new", "--bin"]).is_err());
        assert!(parse_cli(["incan", "new", "--lib"]).is_err());
    }

    #[test]
    fn test_cli_parse_run_release() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "run", "--release", "test.incn"])?;
        let Some(Command::Run { release, .. }) = cli.command else {
            return Err(expected_command("run"));
        };
        assert!(release, "run --release should enable release profile");
        Ok(())
    }

    #[test]
    fn test_cli_parse_run_cargo_passthrough_args() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "run", "test.incn", "--", "--timings", "--color=always"])?;
        let Some(Command::Run { cargo_passthrough, .. }) = cli.command else {
            return Err(expected_command("run"));
        };
        assert_eq!(cargo_passthrough, vec!["--timings", "--color=always"]);
        Ok(())
    }

    #[test]
    fn test_cli_parse_run_with_code() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "run", "-c", "print(1)"])?;
        let Some(Command::Run { command, .. }) = cli.command else {
            return Err(expected_command("run"));
        };
        assert_eq!(command.as_deref(), Some("print(1)"));
        Ok(())
    }

    #[test]
    fn test_cli_parse_fmt() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "fmt", "src/", "--check"])?;
        let Some(Command::Fmt { check, .. }) = cli.command else {
            return Err(expected_command("fmt"));
        };
        assert!(check);
        Ok(())
    }

    #[test]
    fn test_cli_parse_fmt_workspace_member_selection() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "fmt", "--member", "alpha", "--member", "packages/zebra"])?;
        let Some(Command::Fmt { workspace, members, .. }) = cli.command else {
            return Err(expected_command("fmt"));
        };
        assert!(!workspace);
        assert_eq!(members, vec!["alpha", "packages/zebra"]);
        Ok(())
    }

    #[test]
    fn test_cli_parse_test() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "test", "-v", "-x", "-k", "unit"])?;
        let Some(Command::Test {
            verbose,
            stop_on_fail,
            filter,
            ..
        }) = cli.command
        else {
            return Err(expected_command("test"));
        };
        assert!(verbose);
        assert!(stop_on_fail);
        assert_eq!(filter.as_deref(), Some("unit"));
        Ok(())
    }

    #[test]
    fn test_cli_parse_test_cargo_policy() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "test", "tests/", "--frozen", "--cargo-args", "--timings"])?;
        let Some(Command::Test { frozen, cargo_args, .. }) = cli.command else {
            return Err(expected_command("test"));
        };
        assert!(frozen);
        assert_eq!(cargo_args, vec!["--timings"]);
        Ok(())
    }

    #[test]
    fn test_cli_parse_version() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "version", "patch", "--dry-run"])?;
        let Some(Command::Version { bump, dry_run, .. }) = cli.command else {
            return Err(expected_command("version"));
        };
        assert_eq!(bump, Some(VersionBumpArg::Patch));
        assert!(dry_run);
        Ok(())
    }

    #[test]
    fn test_cli_parse_version_project_override() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "version", "--set", "1.2.3", "--project", "examples/greeter"])?;
        let Some(Command::Version { set, project, .. }) = cli.command else {
            return Err(expected_command("version"));
        };
        assert_eq!(set.as_deref(), Some("1.2.3"));
        assert_eq!(project.as_deref(), Some(std::path::Path::new("examples/greeter")));
        Ok(())
    }

    #[test]
    fn test_cli_parse_env_run_passthrough_args() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "env", "run", "unit", "test", "--dry-run", "--", "-k", "greet"])?;
        let Some(Command::Env {
            command:
                EnvCommand::Run {
                    env,
                    script,
                    dry_run,
                    args,
                    ..
                },
        }) = cli.command
        else {
            return Err(expected_command("env run"));
        };
        assert_eq!(env, "unit");
        assert_eq!(script, "test");
        assert!(dry_run);
        assert_eq!(args, vec!["-k".to_string(), "greet".to_string()]);
        Ok(())
    }

    #[test]
    fn test_cli_parse_env_show_without_name() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "env", "show"])?;
        let Some(Command::Env {
            command: EnvCommand::Show { env, .. },
        }) = cli.command
        else {
            return Err(expected_command("env show"));
        };
        assert!(env.is_none());
        Ok(())
    }

    #[test]
    fn test_cli_parse_env_list_json_with_project_override() -> Result<(), clap::Error> {
        let cli = parse_cli([
            "incan",
            "env",
            "list",
            "--format",
            "json",
            "--project",
            "examples/greeter",
        ])?;
        let Some(Command::Env {
            command: EnvCommand::List { format, project },
        }) = cli.command
        else {
            return Err(expected_command("env list"));
        };
        assert_eq!(format, EnvOutputFormat::Json);
        assert_eq!(project.as_deref(), Some(std::path::Path::new("examples/greeter")));
        Ok(())
    }

    #[test]
    fn test_cli_parse_tools_doctor_json() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "tools", "doctor", "--format", "json"])?;
        let Some(Command::Tools {
            command: ToolsCommand::Doctor { format },
        }) = cli.command
        else {
            return Err(expected_command("tools doctor"));
        };
        assert_eq!(format, ToolsDoctorFormat::Json);
        Ok(())
    }

    #[test]
    fn test_cli_parse_tools_metadata_api_json() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "tools", "metadata", "api", "src/lib.incn", "--format", "json"])?;
        let Some(Command::Tools {
            command:
                ToolsCommand::Metadata {
                    command: ToolsMetadataCommand::Api { path, format },
                },
        }) = cli.command
        else {
            return Err(expected_command("tools metadata api"));
        };
        assert_eq!(path, std::path::PathBuf::from("src/lib.incn"));
        assert_eq!(format, ToolsMetadataFormat::Json);
        Ok(())
    }

    #[test]
    fn test_cli_parse_tools_metadata_api_markdown() -> Result<(), clap::Error> {
        let cli = parse_cli([
            "incan",
            "tools",
            "metadata",
            "api",
            "src/lib.incn",
            "--format",
            "markdown",
        ])?;
        let Some(Command::Tools {
            command:
                ToolsCommand::Metadata {
                    command: ToolsMetadataCommand::Api { path, format },
                },
        }) = cli.command
        else {
            return Err(expected_command("tools metadata api"));
        };
        assert_eq!(path, std::path::PathBuf::from("src/lib.incn"));
        assert_eq!(format, ToolsMetadataFormat::Markdown);
        Ok(())
    }

    #[test]
    fn test_cli_parse_workspace_inspect_scope_selectors() -> Result<(), clap::Error> {
        let cli = parse_cli([
            "incan",
            "workspace",
            "inspect",
            "--format",
            "json",
            "--member",
            "alpha",
            "--member",
            "packages/zebra",
        ])?;
        let Some(Command::Workspace {
            command:
                WorkspaceCommand::Inspect {
                    format,
                    workspace,
                    members,
                },
        }) = cli.command
        else {
            return Err(expected_command("workspace inspect"));
        };
        assert_eq!(format, WorkspaceInspectFormat::Json);
        assert!(!workspace);
        assert_eq!(members, vec!["alpha", "packages/zebra"]);
        Ok(())
    }

    #[test]
    fn test_cli_parse_workspace_selectors_for_supported_commands() -> Result<(), clap::Error> {
        let build = parse_cli(["incan", "build", "--workspace", "--report", "json"])?;
        let Some(Command::Build { workspace, members, .. }) = build.command else {
            return Err(expected_command("build"));
        };
        assert!(workspace);
        assert!(members.is_empty());

        let check = parse_cli(["incan", "check", "--member", "alpha"])?;
        let Some(Command::Check { workspace, members, .. }) = check.command else {
            return Err(expected_command("check"));
        };
        assert!(!workspace);
        assert_eq!(members, vec!["alpha"]);

        let run = parse_cli(["incan", "run", "--member", "packages/demo"])?;
        let Some(Command::Run { workspace, members, .. }) = run.command else {
            return Err(expected_command("run"));
        };
        assert!(!workspace);
        assert_eq!(members, vec!["packages/demo"]);

        let version = parse_cli(["incan", "version", "patch", "--member", "alpha"])?;
        let Some(Command::Version { workspace, members, .. }) = version.command else {
            return Err(expected_command("version"));
        };
        assert!(!workspace);
        assert_eq!(members, vec!["alpha"]);
        Ok(())
    }

    #[test]
    fn test_cli_parse_check_interop_target() -> Result<(), clap::Error> {
        let check = parse_cli([
            "incan",
            "check",
            "--interop-target",
            "aarch64-linux-android",
            "src/main.incn",
        ])?;
        let Some(Command::Check {
            interop_target, path, ..
        }) = check.command
        else {
            return Err(expected_command("check"));
        };
        assert_eq!(interop_target.as_deref(), Some("aarch64-linux-android"));
        assert_eq!(path, std::path::PathBuf::from("src/main.incn"));
        Ok(())
    }

    #[test]
    fn test_cli_parse_inspect_interop_plan() -> Result<(), clap::Error> {
        let inspect = parse_cli([
            "incan",
            "inspect",
            "interop-plan",
            "--target",
            "aarch64-linux-android",
            "--format",
            "json",
            ".",
        ])?;
        let Some(Command::Inspect {
            command: InspectCommand::InteropPlan { path, target, format },
        }) = inspect.command
        else {
            return Err(expected_command("inspect interop-plan"));
        };
        assert_eq!(path, std::path::PathBuf::from("."));
        assert_eq!(target, "aarch64-linux-android");
        assert_eq!(format, InteropPlanInspectionFormat::Json);
        Ok(())
    }

    #[test]
    fn test_cli_parse_debug_flags() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan", "--lex", "test.incn"])?;
        assert!(cli.lex_file.is_some());

        let cli = parse_cli(["incan", "--parse", "test.incn"])?;
        assert!(cli.parse_file.is_some());

        let cli = parse_cli(["incan", "--check", "test.incn"])?;
        assert!(cli.check_file.is_some());

        let cli = parse_cli(["incan", "--emit-rust", "test.incn"])?;
        assert!(cli.emit_rust_file.is_some());
        Ok(())
    }

    #[test]
    fn test_banner_policy_prefers_run_and_build_only() -> Result<(), clap::Error> {
        assert!(command_prefers_banner(&parse_cli(["incan", "run", "main.incn"])?));
        assert!(command_prefers_banner(&parse_cli(["incan", "build", "main.incn"])?));
        assert!(!command_prefers_banner(&parse_cli(["incan", "test"])?));
        assert!(!command_prefers_banner(&parse_cli(["incan", "env", "list"])?));
        assert!(!command_prefers_banner(&parse_cli(["incan", "version", "patch"])?));
        assert!(!command_prefers_banner(&parse_cli(["incan", "new", "demo"])?));
        assert!(!command_prefers_banner(&parse_cli(["incan", "--check", "main.incn"])?));
        Ok(())
    }

    #[test]
    fn test_execute_without_args_returns_help_text() -> Result<(), clap::Error> {
        let cli = parse_cli(["incan"])?;
        let result = execute(cli, false);
        let Err(err) = result else {
            return Err(expected_command("help failure"));
        };
        assert_eq!(err.exit_code, ExitCode::FAILURE);
        assert!(
            !err.message.trim().is_empty(),
            "expected help text for no-arg invocation"
        );
        assert!(
            err.message.contains("Usage:"),
            "expected clap usage block in help output"
        );
        assert!(
            err.message.contains("build") && err.message.contains("run"),
            "expected top-level command tokens in help output"
        );
        Ok(())
    }
}
