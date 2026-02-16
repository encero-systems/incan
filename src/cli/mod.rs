//! CLI module for the Incan compiler
//!
//! This module provides the command-line interface for the compiler.
//!
//! ## Commands
//!
//! - `build <file>` - Compile to Rust and build executable
//! - `run <file>` - Compile and run the program
//! - `init [path]` - Create a starter incan.toml
//! - `fmt <file|dir>` - Format Incan source files
//! - `test [path]` - Run tests (pytest-style)
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

use clap::{Parser, Subcommand, ValueEnum};

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
        /// Source file to compile
        #[arg(value_name = "FILE")]
        file: PathBuf,
        /// Output directory (default: target/incan/<name>)
        #[arg(value_name = "OUTPUT_DIR")]
        output_dir: Option<PathBuf>,
        /// Require up-to-date incan.lock and pass --locked to Cargo
        #[arg(long)]
        locked: bool,
        /// Require up-to-date incan.lock and pass --frozen to Cargo
        #[arg(long)]
        frozen: bool,
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

    /// Compile and run the program
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
        /// Require up-to-date incan.lock and pass --frozen to Cargo
        #[arg(long)]
        frozen: bool,
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
        /// Fail if no tests are collected
        #[arg(long = "fail-on-empty")]
        fail_on_empty: bool,
        /// Require up-to-date incan.lock and pass --locked to Cargo
        #[arg(long)]
        locked: bool,
        /// Require up-to-date incan.lock and pass --frozen to Cargo
        #[arg(long)]
        frozen: bool,
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

// ============================================================================
// CLI entry point
// ============================================================================

/// Main CLI entry point.
///
/// This is the only place where `process::exit` is called. All command
/// implementations return `CliResult` and errors are handled here.
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
            output_dir,
            locked,
            frozen,
            cargo_features,
            cargo_no_default_features,
            cargo_all_features,
        }) => {
            let out = output_dir.map(|p| p.to_string_lossy().to_string());
            commands::build_file(
                &file.to_string_lossy(),
                out.as_ref(),
                locked,
                frozen,
                cargo_features,
                cargo_no_default_features,
                cargo_all_features,
            )
        }
        Some(Command::Run {
            file,
            command,
            locked,
            frozen,
            cargo_features,
            cargo_no_default_features,
            cargo_all_features,
        }) => execute_run(
            file,
            command,
            locked,
            frozen,
            cargo_features,
            cargo_no_default_features,
            cargo_all_features,
        ),
        Some(Command::Fmt { path, check, diff }) => commands::format_files(&path.to_string_lossy(), check, diff),
        Some(Command::Test {
            path,
            verbose,
            stop_on_fail,
            slow,
            filter,
            fail_on_empty,
            locked,
            frozen,
            cargo_features,
            cargo_no_default_features,
            cargo_all_features,
        }) => test_runner::run_tests(test_runner::TestRunConfig {
            path: &path.to_string_lossy(),
            verbose,
            stop_on_fail,
            include_slow: slow,
            filter: filter.as_deref(),
            use_color,
            fail_on_empty,
            locked,
            frozen,
            cargo_features,
            cargo_no_default_features,
            cargo_all_features,
        }),
        Some(Command::Init { path, name, version }) => commands::init_project(&path, name.as_deref(), &version),
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
                Err(CliError::new("", ExitCode::FAILURE))
            }
        }
    }
}

/// Handle the `run` subcommand with its various forms.
fn execute_run(
    file: Option<PathBuf>,
    code: Option<String>,
    locked: bool,
    frozen: bool,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
) -> CliResult<ExitCode> {
    if let Some(code) = code {
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
            locked,
            frozen,
            cargo_features.clone(),
            cargo_no_default_features,
            cargo_all_features,
        );
        let _ = fs::remove_file(&tmp_path);
        result
    } else if let Some(file) = file {
        commands::run_file(
            &file.to_string_lossy(),
            locked,
            frozen,
            cargo_features,
            cargo_no_default_features,
            cargo_all_features,
        )
    } else {
        Err(CliError::failure("Error: run requires a file path or -c \"code\""))
    }
}

/// Print logo to stderr (colored or not)
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

/// Decide whether to print the ASCII logo banner.
///
/// Banner suppression (`--no-banner` / `INCAN_NO_BANNER`) always wins.
/// Banners are also suppressed when output is not a TTY (script-friendly).
fn should_print_banner(cli: &Cli, _use_color: bool) -> bool {
    if cli.no_banner || env::var_os("INCAN_NO_BANNER").is_some() {
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

    fn must_cli(args: impl IntoIterator<Item = &'static str>) -> Cli {
        match Cli::try_parse_from(args) {
            Ok(cli) => cli,
            Err(err) => panic!("cli parse failed: {err}"),
        }
    }

    #[test]
    fn test_cli_parse_build() {
        let cli = must_cli(["incan", "build", "test.incn"]);
        assert!(matches!(cli.command, Some(Command::Build { .. })));
    }

    #[test]
    fn test_cli_parse_run() {
        let cli = must_cli(["incan", "run", "test.incn"]);
        assert!(matches!(cli.command, Some(Command::Run { .. })));
    }

    #[test]
    fn test_cli_parse_run_with_code() {
        let cli = must_cli(["incan", "run", "-c", "print(1)"]);
        if let Some(Command::Run { command, .. }) = cli.command {
            assert_eq!(command.as_deref(), Some("print(1)"));
        } else {
            panic!("Expected Run command");
        }
    }

    #[test]
    fn test_cli_parse_fmt() {
        let cli = must_cli(["incan", "fmt", "src/", "--check"]);
        if let Some(Command::Fmt { check, .. }) = cli.command {
            assert!(check);
        } else {
            panic!("Expected Fmt command");
        }
    }

    #[test]
    fn test_cli_parse_test() {
        let cli = must_cli(["incan", "test", "-v", "-x", "-k", "unit"]);
        if let Some(Command::Test {
            verbose,
            stop_on_fail,
            filter,
            ..
        }) = cli.command
        {
            assert!(verbose);
            assert!(stop_on_fail);
            assert_eq!(filter.as_deref(), Some("unit"));
        } else {
            panic!("Expected Test command");
        }
    }

    #[test]
    fn test_cli_parse_debug_flags() {
        let cli = must_cli(["incan", "--lex", "test.incn"]);
        assert!(cli.lex_file.is_some());

        let cli = must_cli(["incan", "--parse", "test.incn"]);
        assert!(cli.parse_file.is_some());

        let cli = must_cli(["incan", "--check", "test.incn"]);
        assert!(cli.check_file.is_some());

        let cli = must_cli(["incan", "--emit-rust", "test.incn"]);
        assert!(cli.emit_rust_file.is_some());
    }
}
