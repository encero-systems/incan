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

#[cfg(test)]
mod tests {
    use super::select_timeout;

    #[tokio::test]
    async fn select_timeout_returns_some_when_task_completes() {
        let result = select_timeout(0.1, async { 99 }).await;
        assert_eq!(result, Some(99));
    }

    #[tokio::test]
    async fn select_timeout_returns_none_when_deadline_expires() {
        let result = select_timeout(0.001, async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            1
        })
        .await;
        assert_eq!(result, None);
    }
}
