//! Tokio-backed select helpers for `std.async.select`.

use std::future::Future;

use super::time::clamp_seconds;

/// Run a task with a timeout, returning `None` when the timeout expires.
pub async fn select_timeout<T, TaskFuture>(seconds: f64, task: TaskFuture) -> Option<T>
where
    TaskFuture: Future<Output = T>,
{
    tokio::time::timeout(clamp_seconds(seconds), task).await.ok()
}

pub use select_timeout as runtime_select_timeout;
