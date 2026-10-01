//! Decoding rustc JSON diagnostics into the bounded Oven report.

use super::{Command, Deserialize, OvenRustcDiagnostic, OvenRustcDiagnosticReport, OvenRustcDiagnosticSpan};

/// Decode rustc JSON diagnostics from both captured streams while retaining unstructured lines.
pub(super) fn parse_rustc_diagnostics(stdout: &[u8], stderr: &[u8]) -> OvenRustcDiagnosticReport {
    let mut diagnostics = Vec::new();
    let mut unstructured = String::new();
    for line in [stdout, stderr].into_iter().flat_map(|stream| {
        String::from_utf8_lossy(stream)
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    }) {
        match serde_json::from_str::<RustcJsonDiagnostic>(&line) {
            Ok(record) if record.is_compiler_message() => diagnostics.push(OvenRustcDiagnostic {
                level: record.message.level,
                message: record.message.message,
                code: record.message.code.map(|code| code.code),
                spans: record
                    .message
                    .spans
                    .into_iter()
                    .map(|span| OvenRustcDiagnosticSpan {
                        file_name: span.file_name,
                        line_start: span.line_start,
                        column_start: span.column_start,
                        line_end: span.line_end,
                        column_end: span.column_end,
                        is_primary: span.is_primary,
                    })
                    .collect(),
                rendered: record.message.rendered,
            }),
            _ if line.trim().is_empty() => {}
            _ => {
                unstructured.push_str(&line);
                unstructured.push('\n');
            }
        }
    }
    OvenRustcDiagnosticReport {
        diagnostics,
        unstructured_output: unstructured,
        invocation: None,
    }
}

impl OvenRustcDiagnosticReport {
    /// Attach bounded process evidence to a report after Rustc has exited unsuccessfully.
    pub(super) fn with_invocation(mut self, command: &Command) -> Self {
        // A legacy-Cargo-published dependency closure with many build-script crates can produce a single-line
        // invocation with hundreds of `-L dependency=...` entries before the first `--extern`. The previous 12,000
        // char bound routinely truncated evidence before any `--extern` flag appeared at all, making failure
        // reports misleading for exactly the invocations most worth diagnosing.
        const MAX_INVOCATION_CHARS: usize = 100_000;

        // Do not render `Command` itself: its debug format may include explicit compile-time environment values.
        // Rustc program and argument evidence is enough to replay the artifact closure without exposing that state.
        let mut rendered = format!("{:?}", command.get_program());
        for argument in command.get_args() {
            rendered.push(' ');
            rendered.push_str(&format!("{argument:?}"));
        }
        let mut invocation = rendered.chars().take(MAX_INVOCATION_CHARS).collect::<String>();
        if rendered.chars().count() > MAX_INVOCATION_CHARS {
            invocation.push_str(" … invocation truncated");
        }
        self.invocation = Some(invocation);
        self
    }
}

/// Minimal rustc JSON envelope for diagnostic preservation.
#[derive(Deserialize)]
struct RustcJsonDiagnostic {
    #[serde(default)]
    reason: String,
    #[serde(default, rename = "$message_type")]
    message_type: String,
    message: RustcJsonMessage,
}

impl RustcJsonDiagnostic {
    /// Cargo wraps rustc diagnostics with `reason`; direct rustc emits the `$message_type` envelope.
    pub(super) fn is_compiler_message(&self) -> bool {
        self.reason == "compiler-message" || self.message_type == "diagnostic"
    }
}

/// Minimal structured rustc diagnostic message.
#[derive(Deserialize)]
struct RustcJsonMessage {
    level: String,
    message: String,
    code: Option<RustcJsonCode>,
    #[serde(default)]
    spans: Vec<RustcJsonSpan>,
    rendered: Option<String>,
}

/// Minimal rustc error-code representation.
#[derive(Deserialize)]
struct RustcJsonCode {
    code: String,
}

/// Minimal rustc span representation.
#[derive(Deserialize)]
struct RustcJsonSpan {
    file_name: String,
    line_start: u32,
    column_start: u32,
    line_end: u32,
    column_end: u32,
    is_primary: bool,
}
