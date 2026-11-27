//! Table-driven unit tests: every row is named, and a table's test fails once, naming each failing row with its
//! problem, whether the row returned an error or panicked.

use std::fmt::Debug;

/// One row of a table whose rows each set up a subject from a source text and check it.
pub(crate) struct SourceRow<S, E = Box<dyn std::error::Error>> {
    /// The row's name, reported when it fails.
    pub name: &'static str,
    pub source: &'static str,
    /// Checks the subject set up from `source`; receives the source too.
    pub check: fn(&str, &S) -> Result<(), E>,
}

/// Runs `check` on every row, treating a panic as a failure, and fails once naming each failing row.
pub(crate) fn run_rows<T>(rows: &[T], name: fn(&T) -> &'static str, check: impl Fn(&T) -> Result<(), String>) {
    let failures: Vec<String> = rows
        .iter()
        .filter_map(|row| {
            let problem = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(row))) {
                Ok(Ok(())) => return None,
                Ok(Err(problem)) => problem,
                Err(payload) => payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|text| text.to_string()))
                    .unwrap_or_else(|| "panicked".to_string()),
            };
            Some(format!("{}: {problem}", name(row)))
        })
        .collect();
    assert!(failures.is_empty(), "{} row(s) failed:\n{}", failures.len(), failures.join("\n\n"));
}

/// Sets up each row's subject with `setup` and runs the row's check on it.
pub(crate) fn run_source_rows<S, E: Debug, F: Debug>(rows: &[SourceRow<S, E>], setup: impl Fn(&str) -> Result<S, F>) {
    run_rows(
        rows,
        |row| row.name,
        |row| {
            let subject = setup(row.source).map_err(|error| format!("setup failed: {error:?}"))?;
            (row.check)(row.source, &subject).map_err(|error| format!("{error:?}"))
        },
    );
}
