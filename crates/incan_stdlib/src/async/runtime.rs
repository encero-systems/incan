//! Async runtime entry helpers for generated programs.
//!
//! Generated user programs should depend on `incan_stdlib`, not directly on Tokio.
//! This module provides the small runtime bootstrap surface needed by the compiler.

use std::fmt;
use std::future::Future;

/// Error returned when the async runtime cannot be initialized.
#[must_use]
pub struct RuntimeInitError {
    source: std::io::Error,
}

impl RuntimeInitError {
    /// Human-readable initialization failure.
    pub fn message(&self) -> String {
        format!("failed to build async runtime: {}", self.source)
    }

    /// Underlying runtime initialization cause.
    pub fn source(&self) -> Option<String> {
        Some(self.source.to_string())
    }
}

impl fmt::Debug for RuntimeInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeInitError")
            .field("source", &self.source)
            .finish()
    }
}

impl fmt::Display for RuntimeInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for RuntimeInitError {}

/// Run an async entrypoint to completion on a Tokio multi-thread runtime.
pub fn block_on<F>(future: F) -> Result<F::Output, RuntimeInitError>
where
    F: Future,
{
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|source| RuntimeInitError { source })?;

    Ok(runtime.block_on(future))
}
