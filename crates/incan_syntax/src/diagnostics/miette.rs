use miette::{Diagnostic, LabeledSpan, SourceSpan};

use super::{CompileError, ErrorKind, format_error};

// ============================================================================
// miette Integration
// ============================================================================

/// Rich diagnostic for miette integration
///
/// Wraps a `CompileError` with source code to provide rich terminal output with highlighted source spans, hints,
/// and related diagnostics.
#[derive(Debug)]
pub struct IncanDiagnostic {
    /// The error message
    pub message: String,
    /// Error code for documentation lookup
    pub code: Option<String>,
    /// The source code where the error occurred
    pub source: miette::NamedSource<String>,
    /// Primary span highlighting the error location
    pub span: SourceSpan,
    /// Label text for the primary span
    pub label: String,
    /// Help text displayed after the error
    pub help: Option<String>,
    /// Related spans (for secondary labels)
    pub related: Vec<LabeledSpan>,
}

impl std::fmt::Display for IncanDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for IncanDiagnostic {}

impl Diagnostic for IncanDiagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.code
            .as_ref()
            .map(|c| Box::new(c.clone()) as Box<dyn std::fmt::Display>)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        let mut labels = vec![LabeledSpan::new(
            Some(self.label.clone()),
            self.span.offset(),
            self.span.len(),
        )];
        labels.extend(self.related.iter().cloned());
        Some(Box::new(labels.into_iter()))
    }

    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.help
            .as_ref()
            .map(|h| Box::new(h.clone()) as Box<dyn std::fmt::Display>)
    }

    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        Some(&self.source)
    }
}

impl IncanDiagnostic {
    /// Create a new diagnostic from a CompileError and source code
    pub fn from_error(error: &CompileError, file_name: &str, source: &str) -> Self {
        let span_start = error.span.start;
        let span_len = (error.span.end - error.span.start).max(1);

        // Combine hints into help text
        let help = if error.hints.is_empty() && error.notes.is_empty() {
            None
        } else {
            let mut help_text = String::new();
            for note in &error.notes {
                help_text.push_str("note: ");
                help_text.push_str(note);
                help_text.push('\n');
            }
            for hint in &error.hints {
                help_text.push_str("hint: ");
                help_text.push_str(hint);
                help_text.push('\n');
            }
            Some(help_text.trim_end().to_string())
        };

        // Generate error code based on kind
        let code = match error.kind {
            ErrorKind::Type => Some("E0001".to_string()),
            ErrorKind::Syntax => Some("E0002".to_string()),
            ErrorKind::Error => Some("E0000".to_string()),
            ErrorKind::Warning => Some("W0001".to_string()),
            ErrorKind::Lint => Some("L0001".to_string()),
        };

        Self {
            message: error.message.clone(),
            code,
            source: miette::NamedSource::new(file_name, source.to_string()),
            span: SourceSpan::new(span_start.into(), span_len),
            label: error.kind.to_string(),
            help,
            related: vec![],
        }
    }

    /// Add a related span (for multi-location errors)
    pub fn with_related(mut self, message: impl Into<String>, start: usize, len: usize) -> Self {
        self.related.push(LabeledSpan::new(Some(message.into()), start, len));
        self
    }
}

/// Render a CompileError using miette's fancy reporter
pub fn render_miette(error: &CompileError, file_name: &str, source: &str) -> String {
    let diagnostic = IncanDiagnostic::from_error(error, file_name, source);
    format!("{:?}", miette::Report::new(diagnostic))
}

/// Format an error, using miette if INCAN_FANCY_ERRORS is set
///
/// Set `INCAN_FANCY_ERRORS=1` to enable miette's fancy error output.
pub fn format_error_smart(file_name: &str, source: &str, error: &CompileError) -> String {
    if std::env::var("INCAN_FANCY_ERRORS").is_ok() {
        render_miette(error, file_name, source)
    } else {
        format_error(file_name, source, error)
    }
}
