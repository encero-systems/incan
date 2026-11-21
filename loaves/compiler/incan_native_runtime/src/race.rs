//! Native source races through the SDK's source-order polling and cancellation boundary.

use incan_std_async::race::{arm, race};
use incan_std_primitives::task::RuntimeFuture;

/// Race two owned futures: build owned arms with their callbacks, poll in source order and cancel the loser.
pub async fn race_pair<T: 'static, U: 'static, R: Send + 'static, F: RuntimeFuture<T>, G: RuntimeFuture<U>>(
    first: F,
    on_first: fn(T) -> R,
    second: G,
    on_second: fn(U) -> R,
) -> R {
    race(vec![arm(first, on_first), arm(second, on_second)]).await
}
