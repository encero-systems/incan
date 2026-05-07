//! Tokio-backed race helpers for `std.async.race`.

use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::task::Poll;

use super::time::clamp_seconds;

/// Runtime-erased race arm.
///
/// Each arm owns one awaitable plus its winner callback. The awaited payload type is intentionally erased here so
/// `std.async.race(*arms: RaceArm[R])` can accept arms with different awaitable output types as long as each callback
/// maps its payload to the shared result type `R`.
pub struct RaceArm<R> {
    future: Pin<Box<dyn Future<Output = R> + Send + 'static>>,
}

/// Runtime bridge trait for one-argument race callbacks.
///
/// This trait encodes `FnOnce(T) -> R + Send + 'static` in a shape Incan generic bounds can reference directly.
pub trait RuntimeRaceCallback<T, R>: FnOnce(T) -> R + Send + 'static {}

impl<T, R, Callback> RuntimeRaceCallback<T, R> for Callback where Callback: FnOnce(T) -> R + Send + 'static {}

/// Package an awaitable and winner callback into a homogeneous [`RaceArm`].
pub fn arm<T, R, TaskFuture, Callback>(awaitable: TaskFuture, on_win: Callback) -> RaceArm<R>
where
    TaskFuture: Future<Output = T> + Send + 'static,
    Callback: FnOnce(T) -> R + Send + 'static,
    R: Send + 'static,
{
    RaceArm {
        future: Box::pin(async move { on_win(awaitable.await) }),
    }
}

/// Await the first completed arm and drop all losing arms.
///
/// Arms are polled in source order on each wake. If multiple arms are ready in the same poll, the earliest arm wins.
///
/// # Panics
///
/// Panics when called with no arms. The Incan surface should normally provide at least one arm.
pub async fn race<R>(mut arms: Vec<RaceArm<R>>) -> R
where
    R: Send + 'static,
{
    assert!(!arms.is_empty(), "std.async.race requires at least one arm");
    poll_fn(|cx| {
        for arm in &mut arms {
            if let Poll::Ready(value) = arm.future.as_mut().poll(cx) {
                return Poll::Ready(value);
            }
        }
        Poll::Pending
    })
    .await
}

/// Run a task with a timeout, returning `None` when the timeout wins.
pub async fn race_timeout<T, TaskFuture>(seconds: f64, task: TaskFuture) -> Option<T>
where
    TaskFuture: Future<Output = T>,
{
    tokio::time::timeout(clamp_seconds(seconds), task).await.ok()
}

#[cfg(test)]
mod tests {
    use std::future::ready;

    use super::{arm, race, race_timeout};

    #[tokio::test]
    async fn race_timeout_returns_some_when_task_completes() {
        let result = race_timeout(0.1, async { 99 }).await;
        assert_eq!(result, Some(99));
    }

    #[tokio::test]
    async fn race_timeout_returns_none_when_deadline_expires() {
        let result = race_timeout(0.001, async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            1
        })
        .await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn race_returns_first_completed_arm() {
        let result = race(vec![
            arm(
                async {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    "slow"
                },
                |value| value.to_string(),
            ),
            arm(ready("fast"), |value| value.to_string()),
        ])
        .await;

        assert_eq!(result, "fast");
    }

    #[tokio::test]
    async fn race_uses_source_order_for_ready_ties() {
        let result = race(vec![arm(ready(1), |value| value), arm(ready(2), |value| value)]).await;

        assert_eq!(result, 1);
    }
}
