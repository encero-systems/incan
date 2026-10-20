//! The compiler's commands: the part of the `incan` command line that checks, formats and inspects source.
//!
//! Building, running and testing belong to the Oven (DD-0008), which compiles each unit through `incan-rustc-driver`.
//!
//! - `check [PATH]`: check a file or project entrypoint, with optional stable JSON diagnostics
//! - `fmt [PATH]`, `new [NAME]`, `init [PATH]`, `version`, `env`, `explain CODE`
//! - `inspect codegraph|features|bindings|representation`: compiler-backed projections
//!
//! Command functions return `CliResult`; only [`run`] exits the process.

#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

pub mod commands;

use std::path::PathBuf;
use std::process;

use clap::{Args, CommandFactory, Parser, Subcommand};
use commands::binding_inspect::BindingInspectionFormat;
use commands::codegraph::CodegraphInspectionFormat;
use commands::diagnostics::DiagnosticOutputFormat;
use commands::feature_inspect::FeatureInspectionFormat;
use commands::lifecycle::{EnvOutputFormat, VersionBumpArg};
use commands::representation_inspect::RepresentationInspectionFormat;
use incan_driver::features::FeatureSelection;

pub use incan_driver::error::{CliError, CliResult, ExitCode};
/// Shared opt-in invocation diagnostics.
pub use incan_driver::invocation_trace;

const VERSION: &str = incan_lang::version::INCAN_VERSION;

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
}

/// The Incan package-feature selection a checking command takes.
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
    match execute(cli) {
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
fn execute(cli: Cli) -> CliResult<ExitCode> {
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
        Some(Command::Fmt { path, check, diff }) => commands::format_files(&path.to_string_lossy(), check, diff),
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
            } => commands::inspect_bindings(&path, format, &package_features.into()),
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
