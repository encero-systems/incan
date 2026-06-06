//! Debug and development commands: lex, parse, check, and emit.
//!
//! These commands expose individual compiler pipeline stages for debugging and development purposes.

use crate::backend::IrCodegen;
use crate::cli::{CliError, CliResult, ExitCode};
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::{diagnostics, lexer, parser};
use crate::manifest::ProjectManifest;
use std::env;
use std::path::{Path, PathBuf};

use super::common::{collect_modules, read_source, resolve_project_root};
use super::diagnostics::{DiagnosticOutputFormat, check_path};

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
    let modules = collect_modules(file_path)?;

    let Some(main_module) = modules.last() else {
        return Err(CliError::failure("No modules found"));
    };

    let mut codegen = IrCodegen::new();
    codegen.set_strict_generated_lints(strict);
    let normalized_file_path = if Path::new(file_path).is_absolute() {
        PathBuf::from(file_path)
    } else {
        env::current_dir()
            .map_err(|e| CliError::failure(format!("failed to determine current directory: {e}")))?
            .join(file_path)
    };
    let project_root = resolve_project_root(&normalized_file_path);
    let manifest = ProjectManifest::discover(&project_root).map_err(|e| CliError::failure(e.to_string()))?;
    if let Some(m) = manifest.as_ref() {
        codegen.set_declared_crate_names(m.declared_rust_crate_names());
    }
    let library_manifest_index = manifest
        .as_ref()
        .map(LibraryManifestIndex::from_project_manifest)
        .unwrap_or_default();
    codegen.set_library_manifest_index(library_manifest_index);

    for module in &modules[..modules.len() - 1] {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }

    let rust_code = codegen
        .try_generate(&main_module.ast)
        .map_err(|e| CliError::failure(format!("Code generation error: {}", e)))?;

    println!("{}", rust_code);
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn check_file_reports_type_errors() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let source_path = tmp.path().join("main.incn");
        fs::write(
            &source_path,
            r#"
def main() -> None:
    missing_symbol()
"#,
        )?;

        let result = check_file(source_path.to_string_lossy().as_ref());
        assert!(result.is_err(), "expected unresolved symbol to fail check_file");
        Ok(())
    }
}
