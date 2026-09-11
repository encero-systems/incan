//! The `incan inspect codegraph` command surface.
//!
//! Selecting an input and formatting an answer is all this does. The analysis behind it lives in
//! [`crate::inspect::codegraph`], behind the compiler boundary, so one rule cannot end up with two
//! implementations that disagree.
//!
//! RFC 118 assigns codegraph to the `incan` surface rather than `oven`: `incan` answers what declarations, types,
//! imports and source relationships exist, while `oven inspect` answers which members, providers, targets and
//! receipts were selected. That RFC names the confusion directly — "`oven inspect` must not masquerade as a
//! semantic codegraph API".

use clap::ValueEnum;
use std::path::Path;

use crate::cli::{CliError, CliResult, ExitCode};
use crate::inspect::codegraph::{collect_codegraph_records, normalize_input_path};
use crate::provider::FeatureSelection;
use incan_codegraph::to_jsonl;

/// Output formats for `incan inspect codegraph`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodegraphInspectionFormat {
    /// Newline-delimited JSON, one record per line.
    Jsonl,
}

/// Export the codegraph for one path.
pub fn inspect_codegraph(
    path: &Path,
    format: CodegraphInspectionFormat,
    allow_errors: bool,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<ExitCode> {
    let normalized = normalize_input_path(path).map_err(CliError::from)?;
    let records = collect_codegraph_records(&normalized, allow_errors, feature_selection, sdk_profile_override)
        .map_err(CliError::from)?;
    match format {
        CodegraphInspectionFormat::Jsonl => {
            let jsonl = to_jsonl(&records)
                .map_err(|error| CliError::failure(format!("failed to serialize codegraph JSONL: {error}")))?;
            print!("{jsonl}");
        }
    }
    Ok(ExitCode::SUCCESS)
}
