//! Table-driven unit tests: every row is named, and a table's test fails once, naming each failing row with its
//! problem, whether the row returned an error or panicked.

// Each Loaf carries the same copy; not every Loaf uses every row kind.
#![allow(dead_code)]

use std::fmt::Debug;

/// One row of a table whose rows each set up a subject from a source text and check it.
pub(crate) struct SourceRow<S, E = Box<dyn std::error::Error>> {
    /// The row's name, reported when it fails.
    pub name: &'static str,
    pub source: &'static str,
    /// Checks the subject set up from `source`; receives the source too.
    pub check: fn(&str, &S) -> Result<(), E>,
}

/// One row of a table whose rows each start from the same freshly set-up subject and change and check it.
pub(crate) struct MutRow<S, E = Box<dyn std::error::Error>> {
    /// The row's name, reported when it fails.
    pub name: &'static str,
    /// Receives the fresh subject by value, as the test that set it up held it.
    pub check: fn(S) -> Result<(), E>,
}

/// One row of a table whose rows each set up a fresh subject from a source text, then change and check it.
pub(crate) struct SourceMutRow<S, E = Box<dyn std::error::Error>> {
    /// The row's name, reported when it fails.
    pub name: &'static str,
    pub source: &'static str,
    /// Receives the source and the fresh subject set up from it, by value.
    pub check: fn(&str, S) -> Result<(), E>,
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
    assert!(
        failures.is_empty(),
        "{} row(s) failed:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
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

/// Gives each row a fresh subject from `setup` and runs the row's check on it.
pub(crate) fn run_mut_rows<S, E: Debug>(rows: &[MutRow<S, E>], setup: impl Fn() -> S) {
    run_rows(
        rows,
        |row| row.name,
        |row| (row.check)(setup()).map_err(|error| format!("{error:?}")),
    );
}

/// One row of a table whose rows each run an action that must panic with a message containing `expected`.
pub(crate) struct PanicRow {
    /// The row's name, reported when it fails.
    pub name: &'static str,
    /// Text the panic message must contain, as `#[should_panic(expected = ...)]` requires.
    pub expected: &'static str,
    pub run: fn(),
}

/// Runs every row's action, requiring a panic whose message contains the row's expected text.
pub(crate) fn run_panic_rows(rows: &[PanicRow]) {
    let failures: Vec<String> = rows
        .iter()
        .filter_map(|row| {
            let problem = match std::panic::catch_unwind(row.run) {
                Ok(()) => "did not panic".to_string(),
                Err(payload) => {
                    let message = payload
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| payload.downcast_ref::<&str>().map(|text| text.to_string()))
                        .unwrap_or_default();
                    if message.contains(row.expected) {
                        return None;
                    }
                    format!("panicked with {message:?}, expected text {:?}", row.expected)
                }
            };
            Some(format!("{}: {problem}", row.name))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} panic row(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Sets up each row's subject from its source with `setup` and runs the row's check on it.
pub(crate) fn run_source_mut_rows<S, E: Debug>(rows: &[SourceMutRow<S, E>], setup: impl Fn(&str) -> S) {
    run_rows(
        rows,
        |row| row.name,
        |row| (row.check)(row.source, setup(row.source)).map_err(|error| format!("{error:?}")),
    );
}
