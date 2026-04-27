//! Tokio-backed channel adapters for `std.async.channel`.

use std::fmt;
use std::sync::{Arc, Mutex as StdMutex};

enum SenderInner<T> {
    Bounded(tokio::sync::mpsc::Sender<T>),
    Unbounded(tokio::sync::mpsc::UnboundedSender<T>),
}

enum SenderPermitInner<T> {
    Bounded(tokio::sync::mpsc::OwnedPermit<T>),
    Unbounded(tokio::sync::mpsc::UnboundedSender<T>),
}

enum ReceiverInner<T> {
    Bounded(tokio::sync::mpsc::Receiver<T>),
    Unbounded(tokio::sync::mpsc::UnboundedReceiver<T>),
}

/// Public sender surface used by generated Incan code.
pub struct Sender<T>(SenderInner<T>);

/// Single-use sender permit used by generated Incan code after reserving channel capacity.
#[must_use]
pub struct SenderPermit<T>(Arc<StdMutex<Option<SenderPermitInner<T>>>>);

/// Send error surface used by generated Incan code.
#[must_use]
pub struct SendError<T> {
    /// Value that could not be delivered to the channel.
    pub value: T,
}

impl<T> SendError<T> {
    /// Recover the value that failed to send.
    pub fn into_value(self) -> T {
        self.value
    }
}

impl<T> fmt::Debug for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SendError(..)")
    }
}

impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("channel closed: send failed")
    }
}

impl<T> std::error::Error for SendError<T> {}

/// Public receiver surface used by generated Incan code.
pub struct Receiver<T>(Arc<tokio::sync::Mutex<ReceiverInner<T>>>);

/// Receive error surface used by generated Incan code.
#[must_use]
pub struct RecvError;

/// Public oneshot sender surface used by generated Incan code.
pub struct OneshotSender<T>(Arc<StdMutex<Option<tokio::sync::oneshot::Sender<T>>>>);

/// Public oneshot receiver surface used by generated Incan code.
pub struct OneshotReceiver<T>(Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<T>>>>);

impl<T> Clone for SenderInner<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Bounded(sender) => Self::Bounded(sender.clone()),
            Self::Unbounded(sender) => Self::Unbounded(sender.clone()),
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Clone for OneshotSender<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Clone for OneshotReceiver<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sender(..)")
    }
}

impl<T> fmt::Debug for SenderPermit<T> {
    /// Format the sender permit without exposing runtime internals.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SenderPermit(..)")
    }
}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Receiver(..)")
    }
}

impl fmt::Debug for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecvError")
    }
}

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("channel closed: receive failed")
    }
}

impl std::error::Error for RecvError {}

impl<T> fmt::Debug for OneshotSender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OneshotSender(..)")
    }
}

impl<T> fmt::Debug for OneshotReceiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OneshotReceiver(..)")
    }
}

fn normalize_bounded_capacity(buffer: i64) -> usize {
    if buffer <= 0 {
        return 1;
    }

    match usize::try_from(buffer) {
        Ok(value) => value,
        Err(_) => usize::MAX,
    }
}

fn try_lock_receiver_sync<T>(
    receiver: &tokio::sync::Mutex<ReceiverInner<T>>,
) -> Option<tokio::sync::MutexGuard<'_, ReceiverInner<T>>> {
    if tokio::runtime::Handle::try_current().is_ok() {
        receiver.try_lock().ok()
    } else {
        Some(receiver.blocking_lock())
    }
}

impl<T> Sender<T> {
    /// Send a value, awaiting for capacity when the channel is bounded.
    ///
    /// Use [`Sender::reserve`] and [`SenderPermit::send`] for critical sends that must not lose a value if an outer
    /// cancellation boundary drops the send future after capacity becomes available.
    pub async fn send(&self, value: T) -> Result<(), SendError<T>> {
        match &self.0 {
            SenderInner::Bounded(sender) => match sender.send(value).await {
                Ok(()) => Ok(()),
                Err(error) => Err(SendError { value: error.0 }),
            },
            SenderInner::Unbounded(sender) => match sender.send(value) {
                Ok(()) => Ok(()),
                Err(error) => Err(SendError { value: error.0 }),
            },
        }
    }

    /// Reserve capacity for one future send.
    ///
    /// Bounded senders wait for capacity using Tokio's owned permit API. Unbounded senders do not need capacity, so
    /// they return an immediate pseudo-permit when the receiver is still open.
    pub async fn reserve(&self) -> Result<SenderPermit<T>, SendError<()>> {
        let permit = match &self.0 {
            SenderInner::Bounded(sender) => match sender.clone().reserve_owned().await {
                Ok(permit) => SenderPermitInner::Bounded(permit),
                Err(error) => return Err(SendError { value: error.0 }),
            },
            SenderInner::Unbounded(sender) => {
                if sender.is_closed() {
                    return Err(SendError { value: () });
                }

                SenderPermitInner::Unbounded(sender.clone())
            }
        };

        Ok(SenderPermit(Arc::new(StdMutex::new(Some(permit)))))
    }

    /// Try to send immediately.
    pub fn try_send(&self, value: T) -> Result<(), SendError<T>> {
        match &self.0 {
            SenderInner::Bounded(sender) => match sender.try_send(value) {
                Ok(()) => Ok(()),
                Err(tokio::sync::mpsc::error::TrySendError::Full(value))
                | Err(tokio::sync::mpsc::error::TrySendError::Closed(value)) => Err(SendError { value }),
            },
            SenderInner::Unbounded(sender) => match sender.send(value) {
                Ok(()) => Ok(()),
                Err(error) => Err(SendError { value: error.0 }),
            },
        }
    }

    /// Whether the receiving side has been closed.
    pub fn is_closed(&self) -> bool {
        match &self.0 {
            SenderInner::Bounded(sender) => sender.is_closed(),
            SenderInner::Unbounded(sender) => sender.is_closed(),
        }
    }
}

impl<T> SenderPermit<T> {
    /// Send a value using this permit.
    ///
    /// Bounded permits consume capacity that was already reserved by [`Sender::reserve`]. Unbounded pseudo-permits send
    /// synchronously and can still fail if the receiver closes after reservation but before this call.
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        let mut permit_slot = match self.0.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };

        match permit_slot.take() {
            Some(SenderPermitInner::Bounded(permit)) => {
                drop(permit.send(value));
                Ok(())
            }
            Some(SenderPermitInner::Unbounded(sender)) => match sender.send(value) {
                Ok(()) => Ok(()),
                Err(error) => Err(SendError { value: error.0 }),
            },
            None => Err(SendError { value }),
        }
    }
}

impl<T> Receiver<T> {
    /// Receive the next message, waiting until one is available or the channel closes.
    pub async fn recv(&self) -> Option<T> {
        let mut receiver = self.0.lock().await;
        match &mut *receiver {
            ReceiverInner::Bounded(inner) => inner.recv().await,
            ReceiverInner::Unbounded(inner) => inner.recv().await,
        }
    }

    /// Try to receive without waiting.
    pub fn try_recv(&self) -> Option<T> {
        let mut receiver = try_lock_receiver_sync(&self.0)?;
        match &mut *receiver {
            ReceiverInner::Bounded(inner) => inner.try_recv().ok(),
            ReceiverInner::Unbounded(inner) => inner.try_recv().ok(),
        }
    }

    /// Close the channel from the receiving side.
    pub fn close(&self) {
        if let Some(mut receiver) = try_lock_receiver_sync(&self.0) {
            match &mut *receiver {
                ReceiverInner::Bounded(inner) => inner.close(),
                ReceiverInner::Unbounded(inner) => inner.close(),
            }
        }
    }
}

impl<T> OneshotSender<T> {
    /// Send the oneshot value.
    pub fn send(&self, value: T) -> Result<(), T> {
        let mut sender_slot = match self.0.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };

        match sender_slot.take() {
            Some(sender) => match sender.send(value) {
                Ok(()) => Ok(()),
                Err(value) => Err(value),
            },
            None => Err(value),
        }
    }
}

impl<T> OneshotReceiver<T> {
    /// Receive the oneshot value.
    pub async fn recv(&self) -> Result<T, RecvError> {
        let mut slot = self.0.lock().await;
        let result = match slot.as_mut() {
            Some(receiver) => receiver.await,
            None => return Err(RecvError),
        };

        *slot = None;
        match result {
            Ok(value) => Ok(value),
            Err(_) => Err(RecvError),
        }
    }
}

/// Create a bounded multi-producer single-consumer channel.
pub fn channel<T>(buffer: i64) -> (Sender<T>, Receiver<T>) {
    let (sender, receiver) = tokio::sync::mpsc::channel(normalize_bounded_capacity(buffer));
    (
        Sender(SenderInner::Bounded(sender)),
        Receiver(Arc::new(tokio::sync::Mutex::new(ReceiverInner::Bounded(receiver)))),
    )
}

/// Create an unbounded multi-producer single-consumer channel.
pub fn unbounded_channel<T>() -> (Sender<T>, Receiver<T>) {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    (
        Sender(SenderInner::Unbounded(sender)),
        Receiver(Arc::new(tokio::sync::Mutex::new(ReceiverInner::Unbounded(receiver)))),
    )
}

/// Create a oneshot channel.
pub fn oneshot<T>() -> (OneshotSender<T>, OneshotReceiver<T>) {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    (
        OneshotSender(Arc::new(StdMutex::new(Some(sender)))),
        OneshotReceiver(Arc::new(tokio::sync::Mutex::new(Some(receiver)))),
    )
}

/// Runtime shim for `Sender::send`.
pub async fn sender_send<T>(sender: &Sender<T>, value: T) -> Result<(), SendError<T>> {
    sender.send(value).await
}

/// Runtime shim for `Sender::try_send`.
pub fn sender_try_send<T>(sender: &Sender<T>, value: T) -> Result<(), SendError<T>> {
    sender.try_send(value)
}

/// Runtime shim for `Sender::reserve`.
pub async fn sender_reserve<T>(sender: &Sender<T>) -> Result<SenderPermit<T>, SendError<()>> {
    sender.reserve().await
}

/// Runtime shim for `SenderPermit::send`.
pub fn sender_permit_send<T>(permit: &SenderPermit<T>, value: T) -> Result<(), SendError<T>> {
    permit.send(value)
}

/// Runtime shim for `Sender::is_closed`.
pub fn sender_is_closed<T>(sender: &Sender<T>) -> bool {
    sender.is_closed()
}

/// Runtime shim for `Receiver::recv`.
pub async fn receiver_recv<T>(receiver: &Receiver<T>) -> Option<T> {
    receiver.recv().await
}

/// Runtime shim for `Receiver::try_recv`.
pub fn receiver_try_recv<T>(receiver: &Receiver<T>) -> Option<T> {
    receiver.try_recv()
}

/// Runtime shim for `Receiver::close`.
pub fn receiver_close<T>(receiver: &Receiver<T>) {
    receiver.close()
}

/// Runtime shim for `OneshotSender::send`.
pub fn oneshot_sender_send<T>(sender: &OneshotSender<T>, value: T) -> Result<(), T> {
    sender.send(value)
}

/// Runtime shim for `OneshotReceiver::recv`.
pub async fn oneshot_receiver_recv<T>(receiver: &OneshotReceiver<T>) -> Result<T, RecvError> {
    receiver.recv().await
}

pub use OneshotReceiver as RawOneshotReceiver;
pub use OneshotSender as RawOneshotSender;
pub use Receiver as RawReceiver;
pub use Sender as RawSender;
pub use SenderPermit as RawSenderPermit;
pub use channel as runtime_channel;
pub use oneshot as runtime_oneshot;
pub use oneshot_receiver_recv as runtime_oneshot_receiver_recv;
pub use oneshot_sender_send as runtime_oneshot_sender_send;
pub use receiver_close as runtime_receiver_close;
pub use receiver_recv as runtime_receiver_recv;
pub use receiver_try_recv as runtime_receiver_try_recv;
pub use sender_is_closed as runtime_sender_is_closed;
pub use sender_permit_send as runtime_sender_permit_send;
pub use sender_reserve as runtime_sender_reserve;
pub use sender_send as runtime_sender_send;
pub use sender_try_send as runtime_sender_try_send;
pub use unbounded_channel as runtime_unbounded_channel;

#[cfg(test)]
mod tests {
    use super::{channel, oneshot, unbounded_channel};

    #[tokio::test(flavor = "current_thread")]
    async fn reserve_sends_through_bounded_channel() -> Result<(), Box<dyn std::error::Error>> {
        let (tx, rx) = channel::<i32>(1);

        let permit = tx.reserve().await?;
        permit.send(7)?;

        assert_eq!(rx.recv().await, Some(7));
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reserve_reports_closed_bounded_channel() {
        let (tx, rx) = channel::<i32>(1);
        drop(rx);

        assert!(tx.reserve().await.is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reserve_reports_closed_unbounded_channel() {
        let (tx, rx) = unbounded_channel::<i32>();
        drop(rx);

        assert!(tx.reserve().await.is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reserved_bounded_permit_holds_capacity_until_send() -> Result<(), Box<dyn std::error::Error>> {
        let (tx, rx) = channel::<i32>(1);

        let permit = tx.reserve().await?;
        assert!(tx.try_send(99).is_err());

        permit.send(42)?;

        assert_eq!(rx.recv().await, Some(42));
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reserve_sends_through_unbounded_channel() -> Result<(), Box<dyn std::error::Error>> {
        let (tx, rx) = unbounded_channel::<i32>();

        let permit = tx.reserve().await?;
        permit.send(11)?;

        assert_eq!(rx.recv().await, Some(11));
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelling_oneshot_receive_preserves_value() {
        let (tx, rx) = oneshot::<i32>();
        let waiting_rx = rx.clone();

        let waiting_task = tokio::spawn(async move { waiting_rx.recv().await });
        tokio::task::yield_now().await;
        waiting_task.abort();
        let _ = waiting_task.await;

        assert!(tx.send(17).is_ok());
        assert_eq!(rx.recv().await.ok(), Some(17));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn try_recv_and_close_do_not_block_inside_runtime() {
        let (_tx, rx) = channel::<i32>(1);
        let waiting_rx = rx.clone();

        let waiting_task = tokio::spawn(async move { waiting_rx.recv().await });
        tokio::task::yield_now().await;

        assert_eq!(rx.try_recv(), None);
        rx.close();

        waiting_task.abort();
    }
}
