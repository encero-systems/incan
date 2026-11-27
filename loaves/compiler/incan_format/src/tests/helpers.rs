//! Helpers that more than one formatter test module uses: parsing a source string to a `Program` and the
//! format-then-lex-and-parse round trip. A helper only one module needs stays private in that module.

use super::*;

pub(super) fn program_from_source(source: &str) -> Result<Program, FormatError> {
    let tokens = lexer::lex(source).map_err(|errs| {
        FormatError::SyntaxError(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n"))
    })?;
    parser::parse(&tokens)
        .map_err(|errs| FormatError::SyntaxError(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))
}

/// Formats `source` and checks the result lexes and parses (regression harness for formatter output validity).
pub(super) fn assert_format_round_trip_lex_parse(source: &str) -> Result<String, FormatError> {
    let formatted = format_source(source)?;
    let tokens = incan_syntax::lexer::lex(&formatted).map_err(|errs| {
        FormatError::SyntaxError(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n"))
    })?;
    incan_syntax::parser::parse(&tokens).map_err(|errs| {
        FormatError::SyntaxError(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n"))
    })?;
    Ok(formatted)
}

/// One formatter table row: a source, the configuration it is formatted with, and the checks its output must pass.
pub(super) struct FormatCase {
    /// The row's name, reported when one of its checks fails.
    pub name: &'static str,
    pub source: &'static str,
    pub line_length: Option<usize>,
    pub trailing_commas: Option<bool>,
    pub indent_width: Option<usize>,
    /// Checked in order against the formatted output.
    pub checks: &'static [Check],
}

impl FormatCase {
    pub const DEFAULT: FormatCase = FormatCase {
        name: "",
        source: "",
        line_length: None,
        trailing_commas: None,
        indent_width: None,
        checks: &[],
    };

    fn config(&self) -> FormatConfig {
        let mut config = FormatConfig::new();
        if let Some(length) = self.line_length {
            config = config.with_line_length(length);
        }
        if let Some(trailing) = self.trailing_commas {
            config = config.with_trailing_commas(trailing);
        }
        if let Some(width) = self.indent_width {
            config = config.with_indent_width(width);
        }
        config
    }

    fn format(&self, source: &str) -> Result<String, String> {
        format_source_with_config(source, self.config()).map_err(|error| format!("formatting failed: {error:?}"))
    }

    fn run(&self) -> Result<(), String> {
        let formatted = self.format(self.source)?;
        for check in self.checks {
            check
                .holds(self, &formatted)
                .map_err(|problem| format!("{problem}; output:\n{formatted}"))?;
        }
        Ok(())
    }
}

/// A property of a formatted output.
#[derive(Debug)]
pub(super) enum Check {
    /// The output is exactly this text.
    Is(&'static str),
    /// The output with trailing whitespace removed is exactly this text.
    IsTrimmed(&'static str),
    /// The output is the unchanged source.
    Unchanged,
    /// The output contains this text.
    Has(&'static str),
    /// The output does not contain this text.
    Lacks(&'static str),
    /// Formatting the output again leaves it unchanged.
    Stable,
    /// The output lexes and parses.
    Parses,
    /// No output line is longer than this many columns.
    Width(usize),
}

impl Check {
    fn holds(&self, case: &FormatCase, formatted: &str) -> Result<(), String> {
        let holds = match self {
            Check::Is(text) => formatted == *text,
            Check::IsTrimmed(text) => formatted.trim_end() == *text,
            Check::Unchanged => formatted == case.source,
            Check::Has(text) => formatted.contains(text),
            Check::Lacks(text) => !formatted.contains(text),
            Check::Stable => case.format(formatted)? == formatted,
            Check::Parses => program_from_source(formatted).is_ok(),
            Check::Width(columns) => formatted.lines().all(|line| line.len() <= *columns),
        };
        if holds {
            Ok(())
        } else {
            Err(format!("{self:?} does not hold"))
        }
    }
}

/// Runs every row and fails once, naming each failing row.
pub(super) fn run_format_cases(cases: &[FormatCase]) {
    let failures: Vec<String> = cases
        .iter()
        .filter_map(|case| case.run().err().map(|problem| format!("{}: {problem}", case.name)))
        .collect();
    assert!(
        failures.is_empty(),
        "{} formatter row(s) failed:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
