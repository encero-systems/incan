//! Rendering a build report to the terminal or a file; the report itself is the driver's.

use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::cli::{CliError, CliResult};
use crate::driver::build_report::{
    BuildReport, BuildReportFormat, BuildReportOptions, RustInspectionFormat, RustInspectionReport,
};

/// Write or print a build report according to CLI options.
pub(crate) fn emit_build_report(report: &BuildReport, options: &BuildReportOptions) -> CliResult<()> {
    let Some(format) = options.format else {
        return Ok(());
    };
    match format {
        BuildReportFormat::Json => {
            let json = serde_json::to_string_pretty(report)
                .map_err(|error| CliError::failure(format!("failed to serialize build report JSON: {error}")))?;
            if let Some(path) = &options.output_path {
                write_json_file(path, &json)?;
            } else {
                println!("{json}");
            }
        }
    }
    Ok(())
}

/// Write or print one aggregate RFC 077 workspace build report using the same user-facing `--report` contract.
pub(crate) fn emit_workspace_build_report(report: &Value, options: &BuildReportOptions) -> CliResult<()> {
    let Some(format) = options.format else {
        return Ok(());
    };
    match format {
        BuildReportFormat::Json => {
            let json = serde_json::to_string_pretty(report).map_err(|error| {
                CliError::failure(format!("failed to serialize workspace build report JSON: {error}"))
            })?;
            if let Some(path) = &options.output_path {
                write_json_file(path, &json)?;
            } else {
                println!("{json}");
            }
        }
    }
    Ok(())
}

/// Render generated Rust inspection output.
pub(crate) fn emit_rust_inspection_report(
    report: &RustInspectionReport,
    format: RustInspectionFormat,
) -> CliResult<()> {
    match format {
        RustInspectionFormat::Text => {
            println!("Generated Rust project: {}", report.generated.project_path);
            println!("Crate root: {}", report.generated.crate_root);
            println!("Rust files:");
            for file in &report.rust_files {
                println!("- {}", file.path);
            }
        }
        RustInspectionFormat::Json => {
            let json = serde_json::to_string_pretty(report).map_err(|error| {
                CliError::failure(format!("failed to serialize generated Rust inspection JSON: {error}"))
            })?;
            println!("{json}");
        }
    }
    Ok(())
}

/// Write pretty JSON to disk with a trailing newline for friendly diffs and shell output.
fn write_json_file(path: &Path, json: &str) -> CliResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            CliError::failure(format!(
                "failed to create report output directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    fs::write(path, format!("{json}\n"))
        .map_err(|error| CliError::failure(format!("failed to write report output {}: {error}", path.display())))
}
