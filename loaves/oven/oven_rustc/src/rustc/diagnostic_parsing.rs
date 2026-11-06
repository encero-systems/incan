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
        match serde_json::from_str::<RustcJsonDiagnostic>(&line)
            .ok()
            .and_then(RustcJsonDiagnostic::into_compiler_message)
        {
            Some(message) => diagnostics.push(OvenRustcDiagnostic {
                level: message.level,
                message: message.message,
                code: message.code.map(|code| code.code),
                spans: message
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
                rendered: message.rendered,
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
#[serde(untagged)]
enum RustcJsonDiagnostic {
    Wrapped {
        reason: String,
        message: RustcJsonMessage,
    },
    Direct {
        #[serde(rename = "$message_type")]
        message_type: String,
        #[serde(flatten)]
        message: RustcJsonMessage,
    },
}

impl RustcJsonDiagnostic {
    /// Preserve the different payload shapes of wrapped messages and actual direct rustc diagnostics.
    fn into_compiler_message(self) -> Option<RustcJsonMessage> {
        match self {
            Self::Wrapped { reason, message } if reason == "compiler-message" => Some(message),
            Self::Direct { message_type, message } if message_type == "diagnostic" => Some(message),
            _ => None,
        }
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

#[cfg(test)]
mod tests {
    /// Actual direct compiler errors retain their codes, spans and complete rendering through either wire shape.
    #[test]
    fn direct_rustc_errors_preserve_structured_diagnostics_and_wrapped_compatibility()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("broken.rs");
        std::fs::write(&source, "pub fn broken() -> u8 { \"wrong\" }\n")?;
        let output = std::process::Command::new(crate::rustc::resolve_active_rustc()?)
            .args(["--crate-type=lib", "--emit=metadata", "--error-format=json"])
            .arg(&source)
            .arg("-o")
            .arg(root.path().join("broken.rmeta"))
            .output()?;
        assert!(!output.status.success());
        let direct = super::parse_rustc_diagnostics(&output.stdout, &output.stderr);
        let diagnostic = direct
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code.as_deref() == Some("E0308"))
            .ok_or("actual direct compiler error was not decoded")?;
        assert_eq!(diagnostic.level, "error");
        assert!(
            diagnostic
                .spans
                .iter()
                .any(|span| span.is_primary && span.line_start == 1)
        );
        assert!(
            diagnostic
                .rendered
                .as_deref()
                .is_some_and(|rendered| rendered.contains("expected `u8`"))
        );
        assert!(direct.unstructured_output.is_empty());
        assert!(direct.to_string().contains("error[E0308]"));
        let mut wrapped = Vec::new();
        for line in String::from_utf8(output.stderr)?.lines() {
            let message: serde_json::Value = serde_json::from_str(line)?;
            wrapped.extend(serde_json::to_vec(&serde_json::json!({
                "reason": "compiler-message", "message": message,
            }))?);
            wrapped.push(b'\n');
        }
        wrapped.extend_from_slice(b"{\"reason\":\"build-finished\",\"success\":false}\nraw tool error\n");
        let compatible = super::parse_rustc_diagnostics(&wrapped, b"");
        assert_eq!(compatible.diagnostics, direct.diagnostics);
        assert_eq!(
            compatible.unstructured_output,
            "{\"reason\":\"build-finished\",\"success\":false}\nraw tool error\n"
        );
        Ok(())
    }
}
