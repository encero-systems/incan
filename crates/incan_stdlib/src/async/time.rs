//! Tokio-backed time adapters for `std.async.time`.

use crate::r#async::task::{JoinHandle, TaskJoinError};
use std::fmt;
use std::time::Duration;

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

/// Result of waiting on a spawned task with a deadline.
#[must_use = "inspect the outcome to preserve timed-out task handles"]
pub enum TimeoutJoinOutcome<T> {
    /// The task completed successfully before the deadline.
    Completed(T),
    /// The task completed before the deadline but failed while joining.
    JoinFailed(TaskJoinError),
    /// The deadline expired and the task handle is still live.
    TimedOut(JoinHandle<T>),
}

/// Clamp a floating-point second value to a valid `Duration`, treating negative/NaN/infinity as zero.
pub fn clamp_seconds(seconds: f64) -> Duration {
    if !seconds.is_finite() || seconds.is_sign_negative() {
        return Duration::from_secs(0);
    }

    Duration::from_secs_f64(seconds)
}

/// Clamp an integer millisecond value to a valid `Duration`, treating negative values as zero.
pub fn clamp_millis(milliseconds: i64) -> Duration {
    let millis = if milliseconds.is_negative() {
        0
    } else {
        milliseconds as u64
    };

    Duration::from_millis(millis)
}

/// Wait for a spawned task up to a floating-point second deadline without cancelling the task on timeout.
pub async fn timeout_join<T>(seconds: f64, handle: JoinHandle<T>) -> TimeoutJoinOutcome<T>
where
    T: Send + 'static,
{
    timeout_join_duration(clamp_seconds(seconds), handle).await
}

/// Wait for a spawned task up to an integer millisecond deadline without cancelling the task on timeout.
pub async fn timeout_join_ms<T>(milliseconds: i64, handle: JoinHandle<T>) -> TimeoutJoinOutcome<T>
where
    T: Send + 'static,
{
    timeout_join_duration(clamp_millis(milliseconds), handle).await
}

/// Wait for a spawned task up to a concrete duration while preserving the handle when the deadline wins.
async fn timeout_join_duration<T>(duration: Duration, mut handle: JoinHandle<T>) -> TimeoutJoinOutcome<T>
where
    T: Send + 'static,
{
    tokio::select! {
        result = &mut handle => match result {
            Ok(value) => TimeoutJoinOutcome::Completed(value),
            Err(error) => TimeoutJoinOutcome::JoinFailed(error),
        },
        _ = tokio::time::sleep(duration) => TimeoutJoinOutcome::TimedOut(handle),
    }
}

#[cfg(test)]
mod tests {
    use super::{TimeoutJoinOutcome, clamp_millis, clamp_seconds, timeout_join, timeout_join_ms};
    use crate::r#async::task::spawn;
    use std::time::Duration;
    use tokio::sync::oneshot;

    fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
        Box::new(std::io::Error::other(message.into()))
    }

    #[tokio::test]
    async fn clamp_seconds_normalizes_negative_and_infinite() {
        assert_eq!(clamp_seconds(-1.0), Duration::from_secs(0));
        assert_eq!(clamp_seconds(f64::INFINITY), Duration::from_secs(0));
        assert_eq!(clamp_seconds(f64::NAN), Duration::from_secs(0));
        assert!(clamp_seconds(0.25).as_nanos() > 0);
    }

    #[tokio::test]
    async fn clamp_millis_normalizes_negative() {
        assert_eq!(clamp_millis(-1), Duration::from_millis(0));
        assert_eq!(clamp_millis(500), Duration::from_millis(500));
    }

    #[tokio::test]
    async fn timeout_join_returns_completed_task_result() -> Result<(), Box<dyn std::error::Error>> {
        let result = timeout_join(0.1, spawn(async { 7 })).await;

        match result {
            TimeoutJoinOutcome::Completed(value) => assert_eq!(value, 7),
            TimeoutJoinOutcome::JoinFailed(err) => {
                return Err(test_error(format!("expected task value, got join error: {err}")));
            }
            TimeoutJoinOutcome::TimedOut(_) => {
                return Err(test_error("expected task to complete before timeout"));
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn timeout_join_preserves_handle_when_deadline_wins() -> Result<(), Box<dyn std::error::Error>> {
        assert_timeout_preserves_handle(false, 11).await
    }

    #[tokio::test]
    async fn timeout_join_ms_preserves_handle_when_deadline_wins() -> Result<(), Box<dyn std::error::Error>> {
        assert_timeout_preserves_handle(true, 13).await
    }

    async fn assert_timeout_preserves_handle(
        use_millis: bool,
        expected: i32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (sender, receiver) = oneshot::channel::<i32>();
        let handle = spawn(async move { receiver.await.unwrap_or(0) });
        let result = if use_millis {
            timeout_join_ms(0, handle).await
        } else {
            timeout_join(0.0, handle).await
        };

        let handle = match result {
            TimeoutJoinOutcome::Completed(value) => {
                return Err(test_error(format!("expected timeout, got completed value: {value}")));
            }
            TimeoutJoinOutcome::JoinFailed(err) => {
                return Err(test_error(format!("expected timeout, got join error: {err}")));
            }
            TimeoutJoinOutcome::TimedOut(handle) => handle,
        };

        assert!(sender.send(expected).is_ok());
        match handle.await {
            Ok(value) => assert_eq!(value, expected),
            Err(err) => {
                return Err(test_error(format!(
                    "expected preserved handle to finish, got join error: {err}"
                )));
            }
        }
        Ok(())
    }
}
