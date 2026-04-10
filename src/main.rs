//! Incan compiler CLI entry point

fn main() {
    // Initialize structured logging with env-based filter, defaulting to warn.
    // This keeps rust-analyzer/salsa internals quiet unless explicitly requested via RUST_LOG.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();

    incan::cli::run();
}
