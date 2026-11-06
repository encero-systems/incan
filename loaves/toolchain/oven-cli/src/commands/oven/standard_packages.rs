//! Explicit cold toolchain preparation through the ordinary standard-package producer.
use crate::{CliResult, ExitCode};
use std::path::Path;

/// Render existing package bake reports after the driver has completed ordinary registry-owned publication.
pub fn oven_prepare_standard_packages(native_graph_output: &Path) -> CliResult<ExitCode> {
    let reports = incan_driver::build::library_project::prepare_standard_packages(native_graph_output)?;
    super::print_json(&reports)?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use crate::cli::OvenCommand;
    use clap::{CommandFactory, Parser};

    /// Exercise the parser shared by the Incan wrapper and standalone Oven entrypoint.
    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: OvenCommand,
    }

    /// The producer requires an explicit graph output but never accepts a source or namespace authority override.
    #[test]
    fn standard_preparation_requires_graph_output_without_source_override() -> Result<(), Box<dyn std::error::Error>> {
        assert!(matches!(
            Cli::try_parse_from([
                "oven",
                "prepare-standard-packages",
                "--native-graph-output",
                "graph.json"
            ])?
            .command,
            OvenCommand::PrepareStandardPackages { .. }
        ));
        assert!(Cli::try_parse_from(["oven", "prepare-standard-packages"]).is_err());
        assert!(
            Cli::try_parse_from([
                "oven",
                "prepare-standard-packages",
                "--native-graph-output",
                "graph.json",
                "--project",
                "/untrusted"
            ])
            .is_err()
        );
        assert!(
            !Cli::command()
                .render_help()
                .to_string()
                .contains("prepare-standard-packages")
        );
        Ok(())
    }
}
