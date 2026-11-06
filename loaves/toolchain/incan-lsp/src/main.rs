//! Incan Language Server binary entry point
//!
//! Run with: incan-lsp
//!
//! `--version` and `--help` are handled before the server starts so install docs and tooling can verify the binary
//! without speaking LSP over stdio.
//!
//! The LSP communicates via stdin/stdout using the Language Server Protocol.

use incan_lsp::IncanLanguageServer;
use std::ffi::OsString;
use std::process::ExitCode;
use std::sync::Arc;
use tower_lsp::{LspService, Server};

/// Run standalone flags or the stdio server on threads with the compiler's stack budget.
fn main() -> ExitCode {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let profile = match handle_cli_args(&args) {
        Ok(profile) => profile,
        Err(exit_code) => return exit_code,
    };

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
        let (service, socket) = LspService::new(move |client| IncanLanguageServer::new(client, Arc::clone(&profile)));
        Server::new(stdin, stdout, socket).serve(service).await;
        ExitCode::SUCCESS
    })
}

/// Handle standalone flags or return the one artifact profile fixed for the stdio server's lifetime.
fn handle_cli_args(args: &[OsString]) -> Result<Arc<str>, ExitCode> {
    match args {
        [] => Ok(Arc::from("debug")),
        [flag, profile] if flag == "--profile" => match profile.to_str() {
            Some(profile) => Ok(Arc::from(profile)),
            None => {
                eprintln!("error: incan-lsp profile must be valid Unicode");
                eprintln!("usage: incan-lsp [--profile NAME | --version | --help]");
                Err(ExitCode::from(2))
            }
        },
        [arg] if arg == "--version" || arg == "-V" => {
            println!("incan-lsp {}", incan_lang::version::INCAN_VERSION);
            Err(ExitCode::SUCCESS)
        }
        [arg] if arg == "--help" || arg == "-h" => {
            println!("Usage: incan-lsp [--profile NAME | --version | --help]");
            println!();
            println!("Starts the Incan language server over stdio; --profile defaults to debug.");
            Err(ExitCode::SUCCESS)
        }
        _ => {
            eprintln!("error: unsupported incan-lsp option");
            eprintln!("usage: incan-lsp [--profile NAME | --version | --help]");
            Err(ExitCode::from(2))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct an operating-system argument for the standalone flag checks.
    fn arg(value: &str) -> OsString {
        OsString::from(value)
    }

    #[test]
    /// The stdio server defaults to debug and accepts one explicit artifact profile.
    fn server_profile_is_parsed_once() -> Result<(), Box<dyn std::error::Error>> {
        let Ok(default) = handle_cli_args(&[]) else {
            return Err("default profile was refused".into());
        };
        let Ok(release) = handle_cli_args(&[arg("--profile"), arg("release")]) else {
            return Err("release profile was refused".into());
        };
        assert_eq!(default.as_ref(), "debug");
        assert_eq!(release.as_ref(), "release");
        Ok(())
    }

    #[test]
    /// Informational flags exit before initializing the server runtime.
    fn version_and_help_exit_successfully() {
        assert_eq!(handle_cli_args(&[arg("--version")]), Err(ExitCode::SUCCESS));
        assert_eq!(handle_cli_args(&[arg("-V")]), Err(ExitCode::SUCCESS));
        assert_eq!(handle_cli_args(&[arg("--help")]), Err(ExitCode::SUCCESS));
        assert_eq!(handle_cli_args(&[arg("-h")]), Err(ExitCode::SUCCESS));
    }

    #[test]
    /// Unknown flags preserve the command-line usage failure status.
    fn unknown_args_exit_with_usage_error() {
        assert_eq!(handle_cli_args(&[arg("--bogus")]), Err(ExitCode::from(2)));
        assert_eq!(
            handle_cli_args(&[arg("--version"), arg("--help")]),
            Err(ExitCode::from(2))
        );
    }
}
