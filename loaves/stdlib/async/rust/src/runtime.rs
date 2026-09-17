//! Async runtime entry helpers for generated programs.
//!
//! Generated user programs should depend on `incan_std_async`, not directly on Tokio. This module provides the small
//! runtime bootstrap surface needed by the compiler.

use std::fmt;
use std::future::Future;
use std::sync::OnceLock;

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

/// Process-wide Tokio runtime shared by every `block_on` call.
///
/// A generated program calls `block_on` once per async operation (one for reading a source, another for
/// collecting a result, and so on), not once for the whole program. An async library's session state can outlive
/// any single call and hold resources tied to the runtime that was active when they were created (a spawned
/// background task, a cached `tokio::runtime::Handle`). Building and dropping a fresh runtime per call breaks that:
/// a later call's new runtime does not satisfy state left behind by an earlier call's now-dropped one, surfacing as
/// "there is no reactor running" even though every individual `block_on` call looks correctly wrapped. Reusing one
/// runtime for the process's lifetime keeps every async resource valid for as long as the program can observe it.
fn shared_runtime() -> Result<&'static tokio::runtime::Runtime, RuntimeInitError> {
    static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, std::io::Error>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| tokio::runtime::Builder::new_multi_thread().enable_all().build())
        .as_ref()
        .map_err(|source| RuntimeInitError {
            source: std::io::Error::new(source.kind(), source.to_string()),
        })
}

/// Run an async entrypoint to completion on the process's shared Tokio multi-thread runtime.
pub fn block_on<F>(future: F) -> Result<F::Output, RuntimeInitError>
where
    F: Future,
{
    Ok(shared_runtime()?.block_on(future))
}

#[cfg(test)]
mod tests {
    use super::block_on;

    #[test]
    fn reuses_one_runtime_across_sequential_calls() -> Result<(), Box<dyn std::error::Error>> {
        // A generated program calls `block_on` once per async operation, not once for the whole program (register a
        // source, then separately collect a result). If each call built and dropped its own runtime, a resource an
        // earlier call's runtime left behind (a spawned background task, a cached `Handle`) would outlive that
        // runtime and later calls would not restore it, which is exactly the "no reactor running" failure this
        // guards against. Comparing the runtime's own id proves the same runtime instance serves both calls.
        let first = block_on(async { tokio::runtime::Handle::current().id() })?;
        let second = block_on(async { tokio::runtime::Handle::current().id() })?;
        assert_eq!(first, second);
        Ok(())
    }

    #[test]
    fn a_task_spawned_in_one_call_can_still_be_reached_from_a_later_call() -> Result<(), Box<dyn std::error::Error>> {
        // Mirrors a session that registers a source (spawning background work while collecting one call) and later
        // collects a result in a separate call: the background task must still be alive and reachable once the call
        // that spawned it has already returned.
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<u32>(1);
        block_on(async move {
            tokio::spawn(async move {
                let _ = sender.send(7).await;
            });
        })?;
        let received = block_on(async move { receiver.recv().await })?;
        assert_eq!(received, Some(7));
        Ok(())
    }
}
