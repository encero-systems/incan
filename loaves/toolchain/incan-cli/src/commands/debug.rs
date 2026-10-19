//! Debug and development commands: lex, parse, check, and emit.
//!
//! These commands expose individual compiler pipeline stages for debugging and development purposes.

use crate::{CliError, CliResult, ExitCode};
use incan_frontend::{diagnostics, lexer, parser};
use std::path::Path;

use super::diagnostics::{DiagnosticOutputFormat, check_path};
use incan_driver::project::read_source;

/// Lex and display tokens.
pub fn lex_file(file_path: &str) -> CliResult<ExitCode> {
    let source = read_source(file_path)?;
    let tokens = match lexer::lex(&source) {
        Ok(toks) => toks,
        Err(errs) => {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(file_path, &source, err));
            }
            return Err(CliError::failure(msg.trim_end()));
        }
    };

    for tok in &tokens {
        println!("{:?}", tok);
    }
    Ok(ExitCode::SUCCESS)
}

/// Parse and display AST.
pub fn parse_file(file_path: &str) -> CliResult<ExitCode> {
    let source = read_source(file_path)?;
    let tokens = match lexer::lex(&source) {
        Ok(t) => t,
        Err(errs) => {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(file_path, &source, err));
            }
            return Err(CliError::failure(msg.trim_end()));
        }
    };

    match parser::parse_with_module_path(&tokens, Some(file_path)) {
        Ok(ast) => {
            println!("{:#?}", ast);
            Ok(ExitCode::SUCCESS)
        }
        Err(errs) => {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(file_path, &source, err));
            }
            Err(CliError::failure(msg.trim_end()))
        }
    }
}

/// Type check a file.
pub fn check_file(file_path: &str) -> CliResult<ExitCode> {
    check_path(Path::new(file_path), DiagnosticOutputFormat::Text)
}

/// Emit generated Rust code.
///
/// If `strict` is true, the output uses stricter clippy attributes to produce warning-clean code suitable for direct
/// use in Rust projects.
pub fn emit_rust(file_path: &str, strict: bool) -> CliResult<ExitCode> {
    let source = incan_driver::build::checked_emission::CheckedRustSource::discover(Path::new(file_path))?;
    let emitted = source.emit(
        strict,
        #[cfg(feature = "rust_inspect")]
        None,
    )?;
    println!("{}", emitted.source);
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use incan_test_support::{incan_command, strip_ansi_escapes};
    use std::fs;

    /// Exercise the real command owner and require the intended diagnostic after a valid source changes.
    #[test]
    fn check_file_reports_type_errors() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let source_path = tmp.path().join("main.incn");
        fs::write(&source_path, "def main() -> None:\n    pass\n")?;
        let valid = incan_command()
            .current_dir(tmp.path())
            .arg("--check")
            .arg(&source_path)
            .output()?;
        assert!(
            valid.status.success(),
            "valid source must pass the actual CLI: {}",
            String::from_utf8_lossy(&valid.stderr)
        );

        fs::write(&source_path, "def main() -> None:\n    missing_symbol()\n")?;
        let invalid = incan_command()
            .current_dir(tmp.path())
            .arg("--check")
            .arg(&source_path)
            .output()?;
        assert!(!invalid.status.success(), "expected unresolved symbol to fail the CLI");
        let diagnostic = strip_ansi_escapes(&String::from_utf8_lossy(&invalid.stderr));
        assert!(
            diagnostic.contains("Unknown symbol 'missing_symbol'"),
            "expected unresolved-symbol diagnostic, got: {diagnostic}"
        );
        Ok(())
    }
}
