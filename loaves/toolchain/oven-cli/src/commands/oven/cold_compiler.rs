//! Explicit cold compiler bootstrap transport for #1337; ordinary commands never fall back to this producer.

use std::path::Path;
use std::process::{Command, ExitStatus};

use incan_driver::build::cold_bootstrap::prepare_cold_compiler_bootstrap;
use incan_driver::oven_store::{OvenStoreCommandOptions, open_store};

use crate::{CliError, CliResult, ExitCode};

/// Prepare the fixed Incan bootstrap and retain its engine leases until the source compiler finishes building.
/// The child inherits stdout and stderr, preserving its diagnostic and JSON output without a second formatter.
/// All paths are resolved before changing its working directory; only the actual invoking executable may act as
/// stage zero. This hidden command is an explicit producer, never a missing-receipt fallback for ordinary builds.
pub fn oven_bootstrap_compiler(
    compiler_root: &Path,
    preparation_output: &Path,
    output: &Path,
    rustc: &Path,
    store_root: &Path,
    profile: &str,
) -> CliResult<ExitCode> {
    let checkout = compiler_root.canonicalize().map_err(|error| {
        CliError::failure(format!(
            "cold compiler bootstrap checkout {}: {error}",
            compiler_root.display()
        ))
    })?;
    let rustc = rustc.canonicalize().map_err(|error| {
        CliError::failure(format!(
            "cold compiler bootstrap Rust compiler {}: {error}",
            rustc.display()
        ))
    })?;
    let preparation_output = absolute(preparation_output)?;
    let output = absolute(output)?;
    let store = open_store(&OvenStoreCommandOptions {
        root: Some(absolute(store_root)?),
        max_physical_bytes: None,
        max_domain_physical_bytes: None,
        max_domain_logical_bytes: None,
    })?;
    let stage_zero = std::env::current_exe()
        .map_err(|error| CliError::failure(format!("cold compiler bootstrap invoking executable: {error}")))?;
    let prepared = prepare_cold_compiler_bootstrap(&checkout, &preparation_output, &rustc, &store)
        .map_err(|error| CliError::new(format!("cold compiler bootstrap preparation: {error}"), error.exit_code))?;
    let status = bootstrap_command(
        prepared.program_path(),
        &checkout,
        &output,
        &rustc,
        store.root(),
        &stage_zero,
        profile,
    )
    .status()
    .map_err(|error| {
        CliError::failure(format!(
            "could not execute cold compiler bootstrap {}: {error}",
            prepared.program_path().display()
        ))
    })?;
    // The three original publication leases must survive the entire child, not merely command construction.
    drop(prepared);
    bootstrap_status(status)
}

/// Resolve destinations against the invocation directory without requiring not-yet-created output files.
fn absolute(path: &Path) -> CliResult<std::path::PathBuf> {
    std::path::absolute(path)
        .map_err(|error| CliError::failure(format!("cold compiler bootstrap path {}: {error}", path.display())))
}

/// Preserve the authored bootstrap's path inputs and explicit compiler profile, binding stage zero to this invocation.
fn bootstrap_command(
    program: &Path,
    checkout: &Path,
    output: &Path,
    rustc: &Path,
    store: &Path,
    stage_zero: &Path,
    profile: &str,
) -> Command {
    let mut command = Command::new(program);
    command.args([checkout, output, rustc, store]).arg(profile);
    command.current_dir(checkout.join("workspaces/compiler-bootstrap"));
    command.env("INCAN_COMPILER_STAGE_ZERO", stage_zero);
    command
}

/// Preserve ordinary child exit codes and report signal termination with its conventional shell status on Unix.
fn bootstrap_status(status: ExitStatus) -> CliResult<ExitCode> {
    if let Some(code) = status.code() {
        return Ok(ExitCode(code));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return Err(CliError::with_code(
                format!("cold compiler bootstrap terminated by signal {signal}"),
                128 + signal,
            ));
        }
    }
    Err(CliError::failure(
        "cold compiler bootstrap terminated without an exit status",
    ))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;

    use clap::{CommandFactory, Parser};

    use crate::cli::OvenCommand;

    use super::bootstrap_command;

    /// Exercise the same shared subcommand parser mounted by both command entrypoints.
    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: OvenCommand,
    }

    /// Every authority path is explicit; no ambient path can silently supply a missing cold-bootstrap input.
    #[test]
    fn cold_bootstrap_cli_requires_all_authority_paths() -> Result<(), Box<dyn std::error::Error>> {
        let arguments = [
            "oven",
            "bootstrap-compiler",
            "--compiler-root",
            "checkout",
            "--preparation-output",
            "preparation",
            "--output",
            "compiler",
            "--rustc",
            "rustc",
            "--store",
            "store",
        ];
        let parsed = Cli::try_parse_from(arguments)?;
        let OvenCommand::BootstrapCompiler {
            compiler_root,
            preparation_output,
            output,
            profile,
            rustc,
            store,
        } = parsed.command
        else {
            return Err("cold bootstrap parsed as a different command".into());
        };
        assert_eq!(
            [compiler_root, preparation_output, output, rustc, store],
            ["checkout", "preparation", "compiler", "rustc", "store"].map(std::path::PathBuf::from)
        );
        assert_eq!(profile, "debug");
        let release = Cli::try_parse_from(arguments.into_iter().chain(["--profile", "release"]))?;
        assert!(matches!(release.command, OvenCommand::BootstrapCompiler { profile, .. } if profile == "release"));
        assert!(Cli::try_parse_from(arguments.into_iter().chain(["--profile", "custom"])).is_err());
        for missing in [2, 4, 6, 8, 10] {
            let reduced = arguments
                .iter()
                .enumerate()
                .filter_map(|(index, argument)| (index != missing && index != missing + 1).then_some(*argument));
            assert!(
                Cli::try_parse_from(reduced).is_err(),
                "missing required pair at {missing}"
            );
        }
        assert!(!Cli::command().render_help().to_string().contains("bootstrap-compiler"));
        Ok(())
    }

    /// Paths containing spaces stay separate positional values, and a caller's inherited stage zero is replaced.
    #[test]
    fn cold_bootstrap_child_keeps_exact_inputs() {
        let program = Path::new("/prepared program");
        let checkout = Path::new("/compiler checkout");
        let output = Path::new("/output compiler");
        let rustc = Path::new("/toolchain/rustc");
        let store = Path::new("/store root");
        let stage_zero = Path::new("/invoking compiler");
        let command = bootstrap_command(program, checkout, output, rustc, store, stage_zero, "release");
        assert_eq!(command.get_program(), program.as_os_str());
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                checkout.as_os_str(),
                output.as_os_str(),
                rustc.as_os_str(),
                store.as_os_str(),
                OsStr::new("release")
            ]
        );
        assert_eq!(
            command.get_current_dir(),
            Some(checkout.join("workspaces/compiler-bootstrap").as_path())
        );
        assert_eq!(
            command.get_envs().collect::<Vec<_>>(),
            [(OsStr::new("INCAN_COMPILER_STAGE_ZERO"), Some(stage_zero.as_os_str()))]
        );
    }

    /// The transport returns actual child success/failure codes and does not collapse signal termination to success.
    #[cfg(unix)]
    #[test]
    fn cold_bootstrap_status_preserves_child_outcomes() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::process::ExitStatusExt;
        use std::process::ExitStatus;

        assert_eq!(super::bootstrap_status(ExitStatus::from_raw(0))?.0, 0);
        assert_eq!(super::bootstrap_status(ExitStatus::from_raw(7 << 8))?.0, 7);
        let error = super::bootstrap_status(ExitStatus::from_raw(9))
            .err()
            .ok_or("signal became success")?;
        assert_eq!(error.exit_code.0, 137);
        assert!(error.message.contains("signal 9"));
        Ok(())
    }
}
