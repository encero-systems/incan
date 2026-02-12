//! Debug and development commands: lex, parse, check, and emit.
//!
//! These commands expose individual compiler pipeline stages for debugging and development purposes.

use crate::backend::IrCodegen;
use crate::cli::{CliError, CliResult, ExitCode};
use crate::frontend::ast::Program;
use crate::frontend::{diagnostics, lexer, parser, typechecker};
use crate::manifest::ProjectManifest;
use std::path::Path;

use super::common::{collect_modules, read_source, resolve_project_root};

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

    match parser::parse(&tokens) {
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
    let modules = collect_modules(file_path)?;

    let project_root = resolve_project_root(Path::new(file_path));
    let manifest = ProjectManifest::discover(&project_root).map_err(|e| CliError::failure(e.to_string()))?;
    let declared = manifest.as_ref().map(|m| m.declared_crate_names());

    let mut all_errors: String = String::new();
    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module: Vec<(&str, &Program)> = modules[..idx].iter().map(|m| (m.name.as_str(), &m.ast)).collect();

        let mut checker = typechecker::TypeChecker::new();
        if let Some(names) = declared.clone() {
            checker.set_declared_crate_names(names);
        }
        match checker.check_with_imports(&module.ast, &deps_for_module) {
            Ok(()) => {
                for warn in checker.warnings() {
                    eprint!(
                        "{}",
                        diagnostics::format_error(module.file_path.to_string_lossy().as_ref(), &module.source, warn)
                    );
                }
            }
            Err(errs) => {
                for err in &errs {
                    all_errors.push_str(&diagnostics::format_error(
                        module.file_path.to_string_lossy().as_ref(),
                        &module.source,
                        err,
                    ));
                }
            }
        }
    }

    if !all_errors.is_empty() {
        return Err(CliError::failure(all_errors.trim_end()));
    }

    println!("✓ Type check passed!");
    Ok(ExitCode::SUCCESS)
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
    let project_root = resolve_project_root(Path::new(file_path));
    let manifest = ProjectManifest::discover(&project_root).map_err(|e| CliError::failure(e.to_string()))?;
    if let Some(m) = manifest.as_ref() {
        codegen.set_declared_crate_names(m.declared_crate_names());
    }

    for module in &modules[..modules.len() - 1] {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }

    let rust_code = codegen
        .try_generate(&main_module.ast)
        .map_err(|e| CliError::failure(format!("Code generation error: {}", e)))?;

    // In strict mode, replace permissive allow attributes with stricter ones
    let output = if strict {
        rust_code.replace(
            "#![allow(unused_imports, unused_parens, dead_code, unused_variables, unused_mut, unused_assignments)]",
            "#![deny(unused_imports, unused_variables)]",
        )
    } else {
        rust_code
    };

    println!("{}", output);
    Ok(ExitCode::SUCCESS)
}
