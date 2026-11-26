//! Stable diagnostic CLI surface: `incan check` renders the compiler's diagnostics as human text or as the stable
//! JSON report for tooling consumers. `incan explain` reads the same catalog without checking a source unit.

use incan_driver::error::{CliError, CliResult, ExitCode};
use incan_frontend::diagnostics::{DiagnosticCatalogEntry, catalog_entries};

/// Read the compiler-owned catalog without preparing a source unit. Unknown codes remain a lookup failure and name
/// the nearest stable codes, ordered by character edit distance and then code for deterministic suggestions.
pub(crate) fn explain(arguments: &[String]) -> Result<i32, Box<dyn std::error::Error>> {
    let [code] = arguments else {
        return Err("usage: incan-rustc-driver --explain CODE".into());
    };
    if let Some(entry) = incan_frontend::diagnostics::explain(code) {
        print!("{}", explanation(entry));
        return Ok(0);
    }
    let mut matches: Vec<_> = catalog_entries().iter().collect();
    let query = code.to_ascii_uppercase();
    matches.sort_by_key(|entry| (edit_distance(&query, entry.code), entry.code));
    let suggestions = matches
        .iter()
        .take(3)
        .map(|entry| entry.code)
        .collect::<Vec<_>>()
        .join(", ");
    eprintln!("incan explain: unknown diagnostic code `{code}`; close matches: {suggestions}");
    Ok(1)
}

/// Render every explanatory catalog field, preserving authored examples and an optional documentation link.
fn explanation(entry: &DiagnosticCatalogEntry) -> String {
    let mut text = format!(
        "{}: {}\nSeverity: {}\nPhase: {}\n\n{}\n\n{}\n",
        entry.code, entry.title, entry.severity, entry.phase, entry.summary, entry.explanation
    );
    for (heading, items) in [
        ("Examples", entry.examples),
        ("Common causes", entry.common_causes),
        ("Fixes", entry.fixes),
    ] {
        text.push_str(&format!("\n{heading}:\n"));
        for item in items {
            text.push_str(item);
            text.push('\n');
        }
    }
    if let Some(url) = entry.docs_url {
        text.push_str(&format!("\nDocumentation: {url}\n"));
    }
    text
}

/// Compare Unicode scalar spellings with linear scratch storage; code suggestions never infer a new catalog entry.
fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<_> = right.chars().collect();
    let mut row: Vec<_> = (0..=right.len()).collect();
    for (i, a) in left.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, b) in right.iter().enumerate() {
            let previous = row[j + 1];
            row[j + 1] = (row[j] + 1).min(previous + 1).min(diagonal + usize::from(a != *b));
            diagonal = previous;
        }
    }
    row[right.len()]
}

/// Output format for stable diagnostics commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiagnosticOutputFormat {
    /// Human-readable diagnostics.
    Text,
    /// The stable JSON check report.
    Json,
}

/// Render one already-evaluated check result (see `incan_driver::check::check_unit`) in either human text or the
/// stable JSON report shape.
pub(crate) fn render_check_report(
    report: incan_driver::check::CheckOutput,
    format: DiagnosticOutputFormat,
) -> CliResult<ExitCode> {
    for warning in report.warnings {
        eprint!("{warning}");
    }
    if format == DiagnosticOutputFormat::Json
        && let Some(json) = report.json
    {
        println!("{json}");
        return if report.exit_code == 0 {
            Ok(ExitCode::SUCCESS)
        } else {
            Err(CliError::new("", ExitCode(report.exit_code)))
        };
    }
    if report.exit_code == 0 {
        println!("✓ Type check passed!");
        Ok(ExitCode::SUCCESS)
    } else {
        Err(CliError::new(
            report
                .human_message
                .unwrap_or_else(|| "type check failed without diagnostics".to_string()),
            ExitCode(report.exit_code),
        ))
    }
}
