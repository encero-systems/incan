//! The `incan` command line.
//!
//! - `build [FILE] [--lib]`: compile a program, or the project's Loaf as a library unit
//! - `run [FILE] [-c CODE] [-- ARGS]`: compile and run a program, defaulting to `[project.scripts].main`
//! - `check [PATH]`: check a file or project entrypoint, with optional stable JSON diagnostics
//! - `test [PATH]`: run tests
//! - `fmt [PATH]`, `new [NAME]`, `init [PATH]`, `version`, `env`, `explain CODE`
//! - `inspect codegraph|features|bindings|representation`: compiler-backed projections
//!
//! Every compilation goes through one unit (`incan_driver::compile::compile_unit`), which prepares the Loaves the unit
//! reaches first. Command functions return `CliResult`; only [`run`] exits the process.

#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

pub mod commands;

use std::env;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use commands::binding_inspect::BindingInspectionFormat;
use commands::build::BuildRequest;
use commands::codegraph::CodegraphInspectionFormat;
use commands::diagnostics::DiagnosticOutputFormat;
use commands::feature_inspect::FeatureInspectionFormat;
use commands::lifecycle::{EnvOutputFormat, VersionBumpArg};
use commands::representation_inspect::RepresentationInspectionFormat;
use incan_driver::features::FeatureSelection;
use oven_model::manifest::ProjectManifest;

pub use incan_driver::error::{CliError, CliResult, ExitCode};
/// Shared opt-in invocation diagnostics used by both CLI entrypoints.
pub use oven_cli::invocation_trace;

/// ASCII art logo, embedded at compile time from the package's assets/logo.txt.
const LOGO: &str = include_str!("assets/logo.txt");
const VERSION: &str = incan_lang::version::INCAN_VERSION;

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

    /// Tokenize only (debug)
    #[arg(long = "lex", value_name = "FILE", conflicts_with = "file")]
    pub lex_file: Option<PathBuf>,

    /// Parse only (debug)
    #[arg(long = "parse", value_name = "FILE", conflicts_with = "file")]
    pub parse_file: Option<PathBuf>,

    /// Type check only (debug)
    #[arg(long = "check", value_name = "FILE", conflicts_with = "file")]
    pub check_file: Option<PathBuf>,

    /// Output format for the --check debug path
    #[arg(long = "format", value_enum, default_value = "text", requires = "check_file")]
    pub check_format: DiagnosticOutputFormat,

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

/// The Incan package-feature selection a compiling command takes.
#[derive(Args, Debug, Clone, Default)]
pub struct PackageFeatureCliFlags {
    /// Incan package features to enable (comma-separated)
    #[arg(long = "features", value_delimiter = ',', value_name = "FEATURE")]
    pub features: Vec<String>,
    /// Disable the Incan package's default features
    #[arg(long = "no-default-features")]
    pub no_default_features: bool,
    /// Enable every Incan package feature
    #[arg(long = "all-features")]
    pub all_features: bool,
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

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Compile a program, or the project's Loaf with `--lib` (release profile by default)
    Build {
        /// Program entry file (default: `[project.scripts].main`)
        #[arg(value_name = "FILE")]
        file: Option<PathBuf>,
        /// Compile the project's `src/lib.incn` as a library unit
        #[arg(long = "lib")]
        lib_mode: bool,
        /// Output directory (default: `target/incan/<name>/<profile>`)
        #[arg(value_name = "OUTPUT_DIR")]
        output_dir: Option<PathBuf>,
        #[command(flatten)]
        package_features: PackageFeatureCliFlags,
        /// Compile with the debug profile
        #[arg(long, conflicts_with = "release")]
        debug: bool,
        /// Compile with the release profile (the default for `incan build`)
        #[arg(long)]
        release: bool,
    },

    /// Type check a file or project entrypoint
    Check {
        /// File or project entrypoint to check
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: DiagnosticOutputFormat,
        #[command(flatten)]
        package_features: PackageFeatureCliFlags,
        /// Verify checked C declarations against this declared interop target; this does not cross-compile
        #[arg(long = "interop-target", value_name = "TRIPLE")]
        interop_target: Option<String>,
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

    /// Compile and run a program (debug profile by default)
    Run {
        /// Program entry file (default: `[project.scripts].main`)
        #[arg(value_name = "FILE", conflicts_with = "command")]
        file: Option<PathBuf>,
        /// Run inline source code
        #[arg(short = 'c', long = "command", value_name = "CODE")]
        command: Option<String>,
        #[command(flatten)]
        package_features: PackageFeatureCliFlags,
        /// Compile with the release profile
        #[arg(long)]
        release: bool,
        /// Arguments passed to the program after `--`
        #[arg(last = true, value_name = "PROGRAM_ARG")]
        program_args: Vec<String>,
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

    /// Update the project version in loaf.toml
    Version {
        /// Version bump to apply
        #[arg(value_enum)]
        bump: Option<VersionBumpArg>,
        /// Explicit SemVer version to set
        #[arg(long = "set", value_name = "VERSION")]
        set: Option<String>,
        /// Print the planned change without writing loaf.toml
        #[arg(long)]
        dry_run: bool,
        /// Keep prerelease metadata when applying major/minor/patch bumps
        #[arg(long)]
        keep_prerelease: bool,
        /// Project root containing loaf.toml
        #[arg(long = "project", value_name = "PATH")]
        project: Option<PathBuf>,
    },

    /// Run named project environment scripts
    Env {
        #[command(subcommand)]
        command: EnvCommand,
    },

    /// Inspect compiler-backed projections of a project
    Inspect {
        #[command(subcommand)]
        command: InspectCommand,
    },

    /// Run Incan tests
    Test {
        #[command(flatten)]
        arguments: oven_cli::cli::TestArgs,
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

    /// Initialize a new loaf.toml manifest
    Init {
        /// Directory to create loaf.toml in
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
}

#[derive(Subcommand, Debug)]
pub enum InspectCommand {
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
        #[command(flatten)]
        package_features: PackageFeatureCliFlags,
    },
    /// Inspect public package-feature roots, closure, edges, and conditioned facts
    Features {
        /// Source file or project directory whose feature projection should be inspected
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: FeatureInspectionFormat,
        #[command(flatten)]
        package_features: PackageFeatureCliFlags,
    },
    /// Inspect compiler-checked C binding declarations
    Bindings {
        /// Source file or project directory whose checked C declarations should be inspected
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: BindingInspectionFormat,
        #[command(flatten)]
        package_features: PackageFeatureCliFlags,
    },
    /// Inspect a Loaf's executable representation: its version and the declarations it covers
    Representation {
        /// Loaf manifest or project root to inspect
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: RepresentationInspectionFormat,
    },
}

#[derive(Subcommand, Debug)]
pub enum EnvCommand {
    /// List configured environments
    List {
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: EnvOutputFormat,
        /// Project root containing loaf.toml
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
        /// Project root containing loaf.toml
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
        /// Project root containing loaf.toml
        #[arg(long = "project", value_name = "PATH")]
        project: Option<PathBuf>,
    },
}

// ============================================================================
// CLI entry point
// ============================================================================

/// Parse the arguments, execute the selected command, and exit the process: the only place that exits.
pub fn run() {
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_cli::run", input = ?std::env::args_os().skip(1).collect::<Vec<_>>()).entered();
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

/// Execute one already-parsed request without terminating the process.
fn execute(cli: Cli, use_color: bool) -> CliResult<ExitCode> {
    if let Some(file) = cli.lex_file {
        return commands::lex_file(&file.to_string_lossy());
    }
    if let Some(file) = cli.parse_file {
        return commands::parse_file(&file.to_string_lossy());
    }
    if let Some(file) = cli.check_file {
        return commands::check_path(&file, cli.check_format);
    }

    match cli.command {
        Some(Command::Build {
            file,
            lib_mode,
            output_dir,
            package_features,
            debug,
            release: _,
        }) => commands::build::build(&BuildRequest {
            source: if lib_mode {
                file.unwrap_or_else(|| PathBuf::from("."))
            } else {
                main_script_entry(file, "build", "FILE unless `--lib` is set")?
            },
            library: lib_mode,
            output_dir,
            features: package_features.into(),
            profile: if debug { "debug" } else { "release" },
        }),
        Some(Command::Check {
            path,
            format,
            package_features,
            interop_target,
        }) => commands::diagnostics::check_path_with_interop_target_selection(
            &path,
            format,
            &package_features.into(),
            interop_target.as_deref(),
        ),
        Some(Command::Explain { code, format }) => commands::explain_diagnostic(&code, format),
        Some(Command::Run {
            file,
            command,
            package_features,
            release,
            program_args,
        }) => {
            let features = package_features.into();
            let profile = if release { "release" } else { "debug" };
            match command {
                Some(code) if code.is_empty() => Err(CliError::failure("-c/--command requires source code")),
                Some(code) => commands::build::run_inline_source(&code, &features, profile, &program_args),
                None => commands::build::run_file(
                    &main_script_entry(file, "run", "a file path or -c/--command")?,
                    &features,
                    profile,
                    &program_args,
                ),
            }
        }
        Some(Command::Fmt { path, check, diff }) => commands::format_files(&path.to_string_lossy(), check, diff),
        Some(Command::Test { arguments }) => {
            oven_cli::test_command::run(arguments, use_color, oven_cli::test_runner::TestLanguages::Incan)
        }
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
        Some(Command::Inspect { command }) => match command {
            InspectCommand::Codegraph {
                path,
                format,
                allow_errors,
                package_features,
            } => commands::inspect_codegraph(&path, format, allow_errors, &package_features.into()),
            InspectCommand::Features {
                path,
                format,
                package_features,
            } => commands::inspect_features(&path, format, &package_features.into()),
            InspectCommand::Bindings {
                path,
                format,
                package_features,
            } => commands::inspect_bindings(&path, format, &package_features.into(), None),
            InspectCommand::Representation { path, format } => commands::inspect_representation(&path, format),
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
        None => match cli.file {
            Some(file) => commands::check_path(&file, DiagnosticOutputFormat::Text),
            None => Err(CliError::new(render_cli_help_text(), ExitCode::FAILURE)),
        },
    }
}

/// An explicit entry file, or the project's `[project.scripts].main`.
fn main_script_entry(file: Option<PathBuf>, command_name: &str, explicit_target: &str) -> CliResult<PathBuf> {
    if let Some(file) = file {
        return Ok(file);
    }
    let cwd = env::current_dir().map_err(|e| CliError::failure(format!("failed to read current directory: {e}")))?;
    let manifest = ProjectManifest::discover(&cwd).map_err(|e| CliError::failure(e.to_string()))?;
    if let Some(manifest) = manifest
        && let Some(project) = &manifest.project
        && let Some(main) = project.scripts.get("main")
    {
        return Ok(manifest.project_root().join(main));
    }
    Err(CliError::failure(format!(
        "{command_name} requires {explicit_target} or [project.scripts].main"
    )))
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
/// Banner suppression (`--no-banner` / `INCAN_NO_BANNER`) always wins. Banners are also suppressed when output is not a
/// TTY (script-friendly). By default, branding is shown only for interactive `build` and `run` flows.
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
