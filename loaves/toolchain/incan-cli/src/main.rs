//! Incan compiler CLI entry point

fn main() {
    if let Some(exit_code) = oven_cargo_compat::run_legacy_rustc_trace_wrapper() {
        std::process::exit(exit_code);
    }
    // Initialize structured logging with env-based filter, defaulting to warn.
    // This keeps rust-analyzer/salsa internals quiet unless explicitly requested via RUST_LOG.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();

    // Compilation recurses over the AST, so run it on a stack sized for deeply nested expressions rather than
    // the default main-thread stack, which a long operator chain can exhaust outright.
    incan_frontend::compiler_stack::run_on_compiler_stack(incan_cli::run);
}
