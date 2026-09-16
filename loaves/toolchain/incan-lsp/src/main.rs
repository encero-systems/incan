//! Incan Language Server binary entry point
//!
//! Run with: incan-lsp
//!
//! `--version` and `--help` are handled before the server starts so install docs
//! and tooling can verify the binary without speaking LSP over stdio.
//!
//! The LSP communicates via stdin/stdout using the Language Server Protocol.

use incan_core::version::INCAN_VERSION;
use incan_lsp::IncanLanguageServer;
use std::ffi::OsString;
use std::process::ExitCode;
use tower_lsp::{LspService, Server};

fn main() -> ExitCode {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if let Some(exit_code) = handle_cli_args(&args) {
        return exit_code;
    }

    // The server typechecks on Tokio's worker threads, and typechecking recurses over the AST, so those threads
    // need the same deep stack the CLI runs compilation on. A default worker stack turns a deeply nested
    // expression in an open file into a hard abort of the language server rather than a diagnostic.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .thread_stack_size(incan_frontend::compiler_stack::compiler_stack_bytes())
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("incan-lsp: could not start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    runtime.block_on(async {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let (service, socket) = LspService::new(IncanLanguageServer::new);
        Server::new(stdin, stdout, socket).serve(service).await;
        ExitCode::SUCCESS
    })
}

/// Handle standalone CLI flags before falling back to the stdio LSP server.
fn handle_cli_args(args: &[OsString]) -> Option<ExitCode> {
    match args {
        [] => None,
        [arg] if arg == "--version" || arg == "-V" => {
            println!("incan-lsp {INCAN_VERSION}");
            Some(ExitCode::SUCCESS)
        }
        [arg] if arg == "--help" || arg == "-h" => {
            println!("Usage: incan-lsp [--version|--help]");
            println!();
            println!("Starts the Incan language server over stdio when no options are supplied.");
            Some(ExitCode::SUCCESS)
        }
        _ => {
            eprintln!("error: unsupported incan-lsp option");
            eprintln!("usage: incan-lsp [--version|--help]");
            Some(ExitCode::from(2))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arg(value: &str) -> OsString {
        OsString::from(value)
    }

    #[test]
    fn no_args_start_lsp_stdio_server() {
        assert_eq!(handle_cli_args(&[]), None);
    }

    #[test]
    fn version_and_help_exit_successfully() {
        assert_eq!(handle_cli_args(&[arg("--version")]), Some(ExitCode::SUCCESS));
        assert_eq!(handle_cli_args(&[arg("-V")]), Some(ExitCode::SUCCESS));
        assert_eq!(handle_cli_args(&[arg("--help")]), Some(ExitCode::SUCCESS));
        assert_eq!(handle_cli_args(&[arg("-h")]), Some(ExitCode::SUCCESS));
    }

    #[test]
    fn unknown_args_exit_with_usage_error() {
        assert_eq!(handle_cli_args(&[arg("--bogus")]), Some(ExitCode::from(2)));
        assert_eq!(
            handle_cli_args(&[arg("--version"), arg("--help")]),
            Some(ExitCode::from(2))
        );
    }
}
