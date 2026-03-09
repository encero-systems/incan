//! Tokio-backed task adapters for `std.async.task`.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Error returned when a spawned task does not complete successfully.
#[must_use]
#[derive(Clone)]
pub struct TaskJoinError {
    message: String,
    cancelled: bool,
    panicked: bool,
}

impl TaskJoinError {
    /// Human-readable join failure message.
    pub fn message(&self) -> String {
        self.message.clone()
    }

    /// Join failures currently surface only a top-level message.
    pub fn source(&self) -> Option<String> {
        None
    }

    /// Whether the task was cancelled before producing a value.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    /// Whether the task failed because it panicked.
    pub fn is_panic(&self) -> bool {
        self.panicked
    }
}

impl From<tokio::task::JoinError> for TaskJoinError {
    fn from(error: tokio::task::JoinError) -> Self {
        Self {
            message: error.to_string(),
            cancelled: error.is_cancelled(),
            panicked: error.is_panic(),
        }
    }
}

impl fmt::Debug for TaskJoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskJoinError")
            .field("message", &self.message)
            .field("cancelled", &self.cancelled)
            .field("panicked", &self.panicked)
            .finish()
    }
}

impl fmt::Display for TaskJoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TaskJoinError {}

/// Runtime task handle exposed to generated Incan code.
pub struct JoinHandle<T>(tokio::task::JoinHandle<T>);

impl<T> JoinHandle<T> {
    /// Await the task and surface join failures as a typed result.
    pub async fn await_result(self) -> Result<T, TaskJoinError> {
        self.0.await.map_err(TaskJoinError::from)
    }

    /// Abort the underlying Tokio task.
    pub fn abort(&self) {
        self.0.abort();
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, TaskJoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.0).poll(cx) {
            Poll::Ready(Ok(value)) => Poll::Ready(Ok(value)),
            Poll::Ready(Err(error)) => Poll::Ready(Err(TaskJoinError::from(error))),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Spawn an async task and return a runtime join handle.
pub fn spawn<T, TaskFuture>(task: TaskFuture) -> JoinHandle<T>
where
    TaskFuture: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    JoinHandle(tokio::spawn(task))
}

/// Schedule blocking work on Tokio's blocking pool.
pub fn spawn_blocking<T, TaskFn>(task: TaskFn) -> JoinHandle<T>
where
    TaskFn: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    JoinHandle(tokio::task::spawn_blocking(task))
}

/// Yield execution back to the scheduler.
pub async fn yield_now() {
    tokio::task::yield_now().await;
}

#[cfg(test)]
mod tests {
    use super::{spawn, spawn_blocking};

    #[tokio::test]
    async fn join_handle_await_surfaces_task_join_error() {
        let handle = spawn(async {
            panic!("boom");
        });
        let result: Result<(), _> = handle.await;
        assert!(result.is_err(), "expected task join error from panicked task");

        if let Err(err) = result {
            assert!(err.is_panic());
            assert!(!err.message().is_empty());
        }
    }

    #[tokio::test]
    async fn spawn_blocking_surfaces_task_join_error() {
        let result: Result<(), _> = spawn_blocking(|| panic!("boom")).await;
        assert!(
            result.is_err(),
            "expected task join error from blocking task that panicked"
        );

        if let Err(err) = result {
            assert!(err.is_panic());
            assert!(!err.message().is_empty());
        }
    }
}
