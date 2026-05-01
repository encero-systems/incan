//! CLI module for the Incan compiler
//!
//! This module provides the command-line interface for the compiler.
//!
//! ## Commands
//!
//! - `build <file>` - Compile to Rust and build executable
//! - `build --lib` - Validate library-mode preconditions
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
use std::fmt;
use std::fs;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process;

use crate::manifest::ProjectManifest;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use commands::common::{CargoPolicy, CargoPolicyCliFlags};
use commands::lifecycle::{EnvOutputFormat, VersionBumpArg};
use commands::tools::{ToolsDoctorFormat, ToolsMetadataFormat, ToolsModelMetadataFormat};

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

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Compile to Rust and build executable
    Build {
        /// Source file to compile (optional in `--lib` mode)
        #[arg(value_name = "FILE")]
        file: Option<PathBuf>,
        /// Enable library mode precondition checks (`src/lib.incn` required)
        #[arg(long = "lib")]
        lib_mode: bool,
        /// Output directory (default: `target/incan/<name>`)
        #[arg(value_name = "OUTPUT_DIR")]
        output_dir: Option<PathBuf>,
        /// Require up-to-date incan.lock and pass --locked to Cargo
        #[arg(long)]
        locked: bool,
        /// Disable INCAN_LOCKED for this invocation
        #[arg(long = "no-locked", conflicts_with_all = ["locked", "frozen"])]
        no_locked: bool,
        /// Pass --offline to Cargo subprocesses
        #[arg(long)]
        offline: bool,
        /// Disable INCAN_OFFLINE for this invocation
        #[arg(long = "no-offline", conflicts_with_all = ["offline", "frozen"])]
        no_offline: bool,
        /// Require up-to-date incan.lock and pass --frozen to Cargo
        #[arg(long)]
        frozen: bool,
        /// Disable INCAN_FROZEN for this invocation
        #[arg(long = "no-frozen", conflicts_with = "frozen")]
        no_frozen: bool,
        /// Extra arguments forwarded to Cargo after policy and feature flags
        #[arg(long = "cargo-args", value_name = "ARG", num_args = 1.., allow_hyphen_values = true)]
        cargo_args: Vec<String>,
        /// Cargo features to enable (comma-separated)
        #[arg(long = "cargo-features", value_delimiter = ',')]
        cargo_features: Vec<String>,
        /// Disable Cargo default features
        #[arg(long = "cargo-no-default-features")]
        cargo_no_default_features: bool,
        /// Enable all Cargo features
        #[arg(long = "cargo-all-features")]
        cargo_all_features: bool,
        /// Extra arguments forwarded to Cargo after `--`
        #[arg(last = true)]
        cargo_passthrough: Vec<String>,
    },

    /// Compile and run the program (debug profile by default; opt into release with `--release`)
    Run {
        /// Source file to run
        #[arg(value_name = "FILE", conflicts_with = "command")]
        file: Option<PathBuf>,
        /// Run inline source code
        #[arg(short = 'c', long = "command", value_name = "CODE")]
        command: Option<String>,
        /// Require up-to-date incan.lock and pass --locked to Cargo
        #[arg(long)]
        locked: bool,
        /// Disable INCAN_LOCKED for this invocation
        #[arg(long = "no-locked", conflicts_with_all = ["locked", "frozen"])]
        no_locked: bool,
        /// Pass --offline to Cargo subprocesses
        #[arg(long)]
        offline: bool,
        /// Disable INCAN_OFFLINE for this invocation
        #[arg(long = "no-offline", conflicts_with_all = ["offline", "frozen"])]
        no_offline: bool,
        /// Require up-to-date incan.lock and pass --frozen to Cargo
        #[arg(long)]
        frozen: bool,
        /// Disable INCAN_FROZEN for this invocation
        #[arg(long = "no-frozen", conflicts_with = "frozen")]
        no_frozen: bool,
        /// Extra arguments forwarded to Cargo after policy and feature flags
        #[arg(long = "cargo-args", value_name = "ARG", num_args = 1.., allow_hyphen_values = true)]
        cargo_args: Vec<String>,
        /// Cargo features to enable (comma-separated)
        #[arg(long = "cargo-features", value_delimiter = ',')]
        cargo_features: Vec<String>,
        /// Disable Cargo default features
        #[arg(long = "cargo-no-default-features")]
        cargo_no_default_features: bool,
        /// Enable all Cargo features
        #[arg(long = "cargo-all-features")]
        cargo_all_features: bool,
        /// Build and run with Cargo release profile (optimized, slower cold-start builds)
        #[arg(long)]
        release: bool,
        /// Extra arguments forwarded to Cargo after `--`
        #[arg(last = true)]
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
    },

    /// Run named project environment scripts
    Env {
        #[command(subcommand)]
        command: EnvCommand,
    },

    /// Inspect local toolchain and editor integration state
    Tools {
        #[command(subcommand)]
        command: ToolsCommand,
    },

    /// Run tests (pytest-style)
    Test {
        /// Path to test file or directory
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
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
        /// Require up-to-date incan.lock and pass --locked to Cargo
        #[arg(long)]
        locked: bool,
        /// Disable INCAN_LOCKED for this invocation
        #[arg(long = "no-locked", conflicts_with_all = ["locked", "frozen"])]
        no_locked: bool,
        /// Pass --offline to Cargo subprocesses
        #[arg(long)]
        offline: bool,
        /// Disable INCAN_OFFLINE for this invocation
        #[arg(long = "no-offline", conflicts_with_all = ["offline", "frozen"])]
        no_offline: bool,
        /// Require up-to-date incan.lock and pass --frozen to Cargo
        #[arg(long)]
        frozen: bool,
        /// Disable INCAN_FROZEN for this invocation
        #[arg(long = "no-frozen", conflicts_with = "frozen")]
        no_frozen: bool,
        /// Extra arguments forwarded to Cargo after policy and feature flags
        #[arg(long = "cargo-args", value_name = "ARG", num_args = 1.., allow_hyphen_values = true)]
        cargo_args: Vec<String>,
        /// Cargo features to enable (comma-separated)
        #[arg(long = "cargo-features", value_delimiter = ',')]
        cargo_features: Vec<String>,
        /// Disable Cargo default features
        #[arg(long = "cargo-no-default-features")]
        cargo_no_default_features: bool,
        /// Enable all Cargo features
        #[arg(long = "cargo-all-features")]
        cargo_all_features: bool,
        /// Extra arguments forwarded to Cargo after `--`
        #[arg(last = true)]
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
        return commands::check_file(&file.to_string_lossy());
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
            cargo_passthrough,
        }) => {
            let out = output_dir.map(|p| p.to_string_lossy().to_string());
            let cargo_policy = CargoPolicy::from_cli_and_env(
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
            );
            if lib_mode {
                let file_arg = file.as_ref().map(|p| p.to_string_lossy().to_string());
                commands::build_library(
                    file_arg.as_deref(),
                    out.as_ref(),
                    cargo_policy,
                    cargo_features,
                    cargo_no_default_features,
                    cargo_all_features,
                )
            } else {
                let file = file.ok_or_else(|| CliError::failure("Error: build requires FILE unless `--lib` is set"))?;
                commands::build_file(
                    &file.to_string_lossy(),
                    out.as_ref(),
                    cargo_policy,
                    cargo_features,
                    cargo_no_default_features,
                    cargo_all_features,
                )
            }
        }
        Some(Command::Run {
            file,
            command,
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
            cargo_passthrough,
        }) => execute_run(
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
                cargo_features,
                cargo_no_default_features,
                cargo_all_features,
                release,
            },
        ),
        Some(Command::Fmt { path, check, diff }) => commands::format_files(&path.to_string_lossy(), check, diff),
        Some(Command::Test {
            path,
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
            cargo_passthrough,
        }) => test_runner::run_tests(test_runner::TestRunConfig {
            path: &path.to_string_lossy(),
            verbose,
            stop_on_fail,
            include_slow: slow,
            filter: filter.as_deref(),
            marker_expr: marker_expr.as_deref(),
            strict_markers,
            jobs,
            test_features,
            timeout: timeout.as_deref(),
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
        }),
        Some(Command::Version {
            bump,
            set,
            dry_run,
            keep_prerelease,
            project,
        }) => commands::version_project(commands::lifecycle::VersionCommandOptions {
            bump,
            set,
            dry_run,
            keep_prerelease,
            project,
        }),
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
        Some(Command::Tools { command }) => match command {
            ToolsCommand::Doctor { format } => commands::tools_doctor(format),
            ToolsCommand::Metadata { command } => match command {
                ToolsMetadataCommand::Api { path, format } => commands::tools_metadata_api(&path, format),
                ToolsMetadataCommand::Model { path, model, format } => {
                    commands::tools_metadata_model(&path, &model, format)
                }
            },
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
            cargo_features,
            cargo_no_default_features,
            cargo_all_features,
        }) => commands::lock_project(
            file.as_ref(),
            cargo_features,
            cargo_no_default_features,
            cargo_all_features,
        ),
        None => {
            // Default: type check the file if provided
            if let Some(file) = cli.file {
                commands::check_file(&file.to_string_lossy())
            } else {
                // No command and no file - show help
                Err(CliError::new(render_cli_help_text(), ExitCode::FAILURE))
            }
        }
    }
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
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    release: bool,
}

/// Resolve the run target for `incan run`, falling back to project metadata when available.
fn resolve_run_entry_file(file: Option<PathBuf>) -> CliResult<PathBuf> {
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

    Err(CliError::failure(
        "Error: run requires a file path, -c/--command, or [project.scripts].main",
    ))
}

/// Handle the `run` subcommand with its various forms.
/// Compile and execute one run request.
fn execute_run(input: RunInput, opts: RunOptions) -> CliResult<ExitCode> {
    // ---- Context: inline source execution (`incan run -c ...`) ----
    if let Some(code) = input.code {
        // Run inline code
        if code.is_empty() {
            return Err(CliError::failure("Error: -c/--command requires source code string"));
        }
        // If the snippet already declares a main, leave as-is; otherwise, append a stub main.
        let wrapped = if code.contains("def main") {
            code
        } else {
            format!("{code}\n\ndef main() -> Unit:\n  pass\n")
        };
        // Write code to a temporary file and run it.
        let tmp_path = env::temp_dir().join(format!(
            "incan_cmd_{}_{}.incn",
            process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        fs::write(&tmp_path, wrapped)
            .map_err(|e| CliError::failure(format!("Error writing temporary command file: {}", e)))?;

        let result = commands::run_file(
            &tmp_path.to_string_lossy(),
            opts.cargo_policy.clone(),
            opts.cargo_features.clone(),
            opts.cargo_no_default_features,
            opts.cargo_all_features,
            opts.release,
        );
        let _ = fs::remove_file(&tmp_path);
        result
    // ---- Context: file execution (`incan run path/to/file.incn`) ----
    } else {
        let file = resolve_run_entry_file(input.file)?;
        commands::run_file(
            &file.to_string_lossy(),
            opts.cargo_policy,
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
