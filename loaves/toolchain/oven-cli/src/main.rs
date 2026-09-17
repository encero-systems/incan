//! The `oven` command: RFC 118's Oven surface as it exists today, over the same handlers `incan oven` mounts.

use std::process;

use clap::{Parser, Subcommand};
use oven_cli::{ExitCode, LockArgs, OvenCommand};

/// The `oven` command line.
#[derive(Parser, Debug)]
#[command(
    name = "oven",
    version,
    about = "Oven: Loaf project receipts, stores, plans and native builds"
)]
struct OvenCli {
    #[command(subcommand)]
    command: OvenTopLevelCommand,
}

/// Everything `oven` exposes at its root: the Oven command family flattened, plus `lock`.
#[derive(Subcommand, Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "the Oven family carries every command's arguments inline; the value is parsed once and consumed at once"
)]
enum OvenTopLevelCommand {
    #[command(flatten)]
    Oven(OvenCommand),
    /// Generate or update oven.lock for a project
    Lock(LockArgs),
}

fn main() {
    if let Some(exit_code) = oven_cargo_compat::run_legacy_rustc_trace_wrapper() {
        process::exit(exit_code);
    }
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();
    // Compilation recurses over the AST, so run it on a stack sized for deeply nested expressions rather than
    // the default main-thread stack, which a long operator chain can exhaust outright.
    incan_frontend::compiler_stack::run_on_compiler_stack(run);
}

/// Parse the command line and run it, terminating the process with the command's exit code.
fn run() {
    let cli = match OvenCli::try_parse() {
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
    let result = match cli.command {
        OvenTopLevelCommand::Oven(command) => oven_cli::run_oven_command(command),
        OvenTopLevelCommand::Lock(args) => oven_cli::run_lock_command(args),
    };
    match result {
        Ok(exit_code) => {
            if exit_code.0 != 0 {
                process::exit(exit_code.0);
            }
        }
        Err(error) => {
            if !error.message.is_empty() {
                eprintln!("{}", error.message);
            }
            process::exit(error.exit_code.0);
        }
    }
}
