//! Debug and development commands: lex, parse, check, and emit.
//!
//! These commands expose individual compiler pipeline stages for debugging and development purposes.

use crate::backend::IrCodegen;
use crate::cli::{CliError, CliResult, ExitCode};
use crate::compiled_sdk::CompiledSdkModules;
use crate::frontend::{diagnostics, lexer, parser};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::common::{CompilationSession, collect_modules_detailed_with_session, read_source};
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
    let normalized_file_path = if Path::new(file_path).is_absolute() {
        PathBuf::from(file_path)
    } else {
        env::current_dir()
            .map_err(|e| CliError::failure(format!("failed to determine current directory: {e}")))?
            .join(file_path)
    };
    let session = CompilationSession::discover_with_feature_selection(
        &normalized_file_path,
        &crate::provider::FeatureSelection::default(),
    )?;
    let modules = collect_modules_detailed_with_session(normalized_file_path, &session)
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let Some(main_module) = modules.last() else {
        return Err(CliError::failure("No modules found"));
    };

    let provider_plan = session.provider_plan_for_modules(&modules)?;
    let compiled_sdk_modules = CompiledSdkModules::from_provider_plan(&provider_plan);
    let mut codegen = IrCodegen::new();
    codegen.set_strict_generated_lints(strict);
    if let Some(m) = session.manifest.as_ref() {
        codegen.set_declared_crate_names(m.declared_rust_crate_names());
    }
    codegen.set_provider_plan(Arc::clone(&provider_plan));

    let dep_modules = &modules[..modules.len() - 1];
    for module in dep_modules
        .iter()
        .filter(|module| compiled_sdk_modules.contains_emission_path(&module.path_segments))
    {
        codegen.add_dependency_symbol_module_with_path_segments(
            &module.name,
            &module.ast,
            module.path_segments.clone(),
        );
    }
    for module in dep_modules
        .iter()
        .filter(|module| !compiled_sdk_modules.contains_emission_path(&module.path_segments))
    {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }

    let analysis = session
        .analyze_modules(
            &modules,
            #[cfg(feature = "rust_inspect")]
            None,
        )
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let main_type_info = analysis
        .type_info_for_path(&main_module.file_path)
        .cloned()
        .ok_or_else(|| {
            CliError::failure(format!(
                "missing session analysis for {}",
                main_module.file_path.display()
            ))
        })?;
    let mut dependency_type_info = std::collections::HashMap::with_capacity(dep_modules.len());
    for module in dep_modules {
        let type_info = analysis
            .type_info_for_path(&module.file_path)
            .cloned()
            .ok_or_else(|| CliError::failure(format!("missing session analysis for {}", module.file_path.display())))?;
        dependency_type_info.insert(module.path_segments.clone(), type_info);
    }
    codegen.set_stdlib_cache(analysis.stdlib_cache().clone());
    codegen.set_prechecked_type_info(main_type_info, dependency_type_info);

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
