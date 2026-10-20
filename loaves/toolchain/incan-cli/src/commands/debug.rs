//! Debug and development commands: lex, parse, check, and emit.
//!
//! These commands expose individual compiler pipeline stages for debugging and development purposes.

use crate::{CliError, CliResult, ExitCode};
use incan_frontend::{diagnostics, lexer, parser};

use incan_driver::sources::read_source;

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
