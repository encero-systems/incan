//! Tokio-backed time adapters for `std.async.time`.

use std::fmt;
use std::future::Future;

/// Timeout error used by the public async timing helpers.
#[must_use]
#[derive(Clone, Copy, Default)]
pub struct TimeoutError;

impl TimeoutError {
    /// Incan-facing error message.
    pub fn message(&self) -> String {
        "operation timed out".to_string()
    }

    /// Timeout errors do not have an underlying cause.
    pub fn source(&self) -> Option<String> {
        None
    }
}

impl fmt::Debug for TimeoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TimeoutError")
    }
}

impl fmt::Display for TimeoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("operation timed out")
    }
}

impl std::error::Error for TimeoutError {}

/// Clamp a floating-point second value to a valid `Duration`, treating negative/NaN/infinity as zero.
pub(crate) fn clamp_seconds(seconds: f64) -> std::time::Duration {
    if !seconds.is_finite() || seconds.is_sign_negative() {
        return std::time::Duration::from_secs(0);
    }

    std::time::Duration::from_secs_f64(seconds)
}

fn clamp_millis(milliseconds: i64) -> std::time::Duration {
    let millis = if milliseconds.is_negative() {
        0
    } else {
        u64::try_from(milliseconds).unwrap_or(u64::MAX)
    };

    std::time::Duration::from_millis(millis)
}

/// Sleep for a floating-point second duration.
pub async fn sleep(seconds: f64) {
    tokio::time::sleep(clamp_seconds(seconds)).await;
}

/// Sleep for an integer millisecond duration.
pub async fn sleep_ms(milliseconds: i64) {
    tokio::time::sleep(clamp_millis(milliseconds)).await;
}

/// Run a task with a timeout.
pub async fn timeout<T, TaskFuture>(seconds: f64, task: TaskFuture) -> Result<T, TimeoutError>
where
    TaskFuture: Future<Output = T>,
{
    match tokio::time::timeout(clamp_seconds(seconds), task).await {
        Ok(value) => Ok(value),
        Err(_) => Err(TimeoutError),
    }
}

/// Run a task with a millisecond timeout.
pub async fn timeout_ms<T, TaskFuture>(milliseconds: i64, task: TaskFuture) -> Result<T, TimeoutError>
where
    TaskFuture: Future<Output = T>,
{
    match tokio::time::timeout(clamp_millis(milliseconds), task).await {
        Ok(value) => Ok(value),
        Err(_) => Err(TimeoutError),
    }
}

#[cfg(test)]
mod tests {
    use super::{TimeoutError, timeout, timeout_ms};

    #[tokio::test]
    async fn timeout_returns_ok_when_task_completes_before_deadline() {
        let result = timeout(0.1, async { 42 }).await;
        assert!(matches!(result, Ok(42)));
    }

    #[tokio::test]
    async fn timeout_returns_err_when_deadline_expires() {
        let result: Result<(), TimeoutError> = timeout(0.001, async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        })
        .await;
        assert!(result.is_err(), "expected timeout to return TimeoutError");
    }

    #[tokio::test]
    async fn timeout_ms_returns_ok_when_task_completes_before_deadline() {
        let result = timeout_ms(100, async { 7 }).await;
        assert!(matches!(result, Ok(7)));
    }

    #[tokio::test]
    async fn timeout_ms_returns_err_when_deadline_expires() {
        let result: Result<(), TimeoutError> = timeout_ms(1, async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        })
        .await;
        assert!(result.is_err(), "expected timeout_ms to return TimeoutError");
    }
}
