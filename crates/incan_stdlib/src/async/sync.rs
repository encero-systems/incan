//! Tokio-backed synchronization adapters for `std.async.sync`.

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

fn lock_std_mutex<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn normalize_non_negative(value: i64) -> usize {
    if value <= 0 {
        return 0;
    }

    match usize::try_from(value) {
        Ok(value) => value,
        Err(_) => usize::MAX,
    }
}

fn normalize_barrier_count(value: i64) -> usize {
    let normalized = normalize_non_negative(value);
    if normalized == 0 { 1 } else { normalized }
}

/// Runtime mutex wrapper.
pub struct Mutex<T>(Arc<tokio::sync::Mutex<T>>);

/// Runtime mutex guard wrapper.
pub struct MutexGuard<T>(StdMutex<tokio::sync::OwnedMutexGuard<T>>);

/// Runtime read-write lock wrapper.
pub struct RwLock<T>(Arc<tokio::sync::RwLock<T>>);

/// Runtime read guard wrapper.
pub struct RwLockReadGuard<T>(StdMutex<tokio::sync::OwnedRwLockReadGuard<T>>);

/// Runtime write guard wrapper.
pub struct RwLockWriteGuard<T>(StdMutex<tokio::sync::OwnedRwLockWriteGuard<T>>);

/// Runtime semaphore wrapper.
pub struct Semaphore(Arc<tokio::sync::Semaphore>);

/// Runtime semaphore permit wrapper.
///
/// Holds an `OwnedSemaphorePermit` for RAII release: the permit is returned to the semaphore automatically when this
/// value is dropped. The inner field is never read directly — its purpose is the `Drop` implementation.
pub struct SemaphorePermit(#[allow(dead_code)] tokio::sync::OwnedSemaphorePermit);

/// Error returned when a semaphore is closed while waiting for a permit.
#[must_use]
#[derive(Clone, Copy, Default)]
pub struct SemaphoreAcquireError;

struct BarrierState {
    barrier: tokio::sync::Barrier,
    parties: usize,
    arrivals: AtomicUsize,
}

/// Runtime barrier wrapper.
pub struct Barrier(Arc<BarrierState>);

impl<T> Clone for Mutex<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Clone for RwLock<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl Clone for Semaphore {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl Clone for Barrier {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Mutex(..)")
    }
}

impl<T> fmt::Debug for MutexGuard<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MutexGuard(..)")
    }
}

impl<T> fmt::Debug for RwLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RwLock(..)")
    }
}

impl<T> fmt::Debug for RwLockReadGuard<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RwLockReadGuard(..)")
    }
}

impl<T> fmt::Debug for RwLockWriteGuard<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RwLockWriteGuard(..)")
    }
}

impl fmt::Debug for Semaphore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Semaphore(..)")
    }
}

impl fmt::Debug for SemaphorePermit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SemaphorePermit(..)")
    }
}

impl fmt::Debug for SemaphoreAcquireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SemaphoreAcquireError")
    }
}

impl fmt::Debug for Barrier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Barrier(..)")
    }
}

impl fmt::Display for SemaphoreAcquireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("failed to acquire semaphore permit: semaphore closed")
    }
}

impl std::error::Error for SemaphoreAcquireError {}

impl SemaphoreAcquireError {
    /// Human-readable error message for Incan-facing wrappers.
    pub fn message(&self) -> String {
        self.to_string()
    }

    /// Semaphore acquire failures do not currently expose an underlying cause.
    pub fn source(&self) -> Option<String> {
        None
    }
}

impl<T> Mutex<T> {
    /// Create a new async mutex.
    pub fn new(value: T) -> Self {
        Self(Arc::new(tokio::sync::Mutex::new(value)))
    }

    /// Acquire the mutex asynchronously.
    pub async fn lock(&self) -> MutexGuard<T> {
        MutexGuard(StdMutex::new(self.0.clone().lock_owned().await))
    }

    /// Try to acquire the mutex immediately.
    pub fn try_lock(&self) -> Option<MutexGuard<T>> {
        match self.0.clone().try_lock_owned() {
            Ok(guard) => Some(MutexGuard(StdMutex::new(guard))),
            Err(_) => None,
        }
    }
}

impl<T> MutexGuard<T> {
    /// Clone the guarded value.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        let guard = lock_std_mutex(&self.0);
        (**guard).clone()
    }

    /// Replace the guarded value.
    pub fn set(&self, value: T) {
        let mut guard = lock_std_mutex(&self.0);
        **guard = value;
    }
}

impl<T> RwLock<T> {
    /// Create a new async read-write lock.
    pub fn new(value: T) -> Self {
        Self(Arc::new(tokio::sync::RwLock::new(value)))
    }

    /// Acquire a read guard.
    pub async fn read(&self) -> RwLockReadGuard<T> {
        RwLockReadGuard(StdMutex::new(self.0.clone().read_owned().await))
    }

    /// Acquire a write guard.
    pub async fn write(&self) -> RwLockWriteGuard<T> {
        RwLockWriteGuard(StdMutex::new(self.0.clone().write_owned().await))
    }

    /// Try to acquire a read guard immediately.
    pub fn try_read(&self) -> Option<RwLockReadGuard<T>> {
        match self.0.clone().try_read_owned() {
            Ok(guard) => Some(RwLockReadGuard(StdMutex::new(guard))),
            Err(_) => None,
        }
    }

    /// Try to acquire a write guard immediately.
    pub fn try_write(&self) -> Option<RwLockWriteGuard<T>> {
        match self.0.clone().try_write_owned() {
            Ok(guard) => Some(RwLockWriteGuard(StdMutex::new(guard))),
            Err(_) => None,
        }
    }
}

impl<T> RwLockReadGuard<T> {
    /// Clone the guarded value.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        let guard = lock_std_mutex(&self.0);
        (**guard).clone()
    }
}

impl<T> RwLockWriteGuard<T> {
    /// Clone the guarded value.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        let guard = lock_std_mutex(&self.0);
        (**guard).clone()
    }

    /// Replace the guarded value.
    pub fn set(&self, value: T) {
        let mut guard = lock_std_mutex(&self.0);
        **guard = value;
    }
}

impl Semaphore {
    /// Create a new semaphore.
    pub fn new(permits: i64) -> Self {
        Self(Arc::new(tokio::sync::Semaphore::new(normalize_non_negative(permits))))
    }

    /// Acquire a semaphore permit asynchronously.
    pub async fn acquire(&self) -> Result<SemaphorePermit, SemaphoreAcquireError> {
        match self.0.clone().acquire_owned().await {
            Ok(permit) => Ok(SemaphorePermit(permit)),
            Err(_) => Err(SemaphoreAcquireError),
        }
    }

    /// Try to acquire a semaphore permit immediately.
    pub fn try_acquire(&self) -> Option<SemaphorePermit> {
        match self.0.clone().try_acquire_owned() {
            Ok(permit) => Some(SemaphorePermit(permit)),
            Err(_) => None,
        }
    }

    /// Report the number of currently available permits.
    pub fn available_permits(&self) -> i64 {
        i64::try_from(self.0.available_permits()).unwrap_or(i64::MAX)
    }
}

impl Barrier {
    /// Create a new barrier.
    pub fn new(count: i64) -> Self {
        let parties = normalize_barrier_count(count);
        Self(Arc::new(BarrierState {
            barrier: tokio::sync::Barrier::new(parties),
            parties,
            arrivals: AtomicUsize::new(0),
        }))
    }

    /// Wait for the remaining participants and return the caller's arrival index.
    pub async fn wait(&self) -> i64 {
        let arrival = self.0.arrivals.fetch_add(1, Ordering::SeqCst) % self.0.parties;
        self.0.barrier.wait().await;
        i64::try_from(arrival).unwrap_or(i64::MAX)
    }
}

/// Runtime shim for constructing a `Mutex`.
pub fn mutex_new<T>(value: T) -> Mutex<T> {
    Mutex::new(value)
}

/// Runtime shim for `Mutex::lock`.
pub async fn mutex_lock<T>(mutex: &Mutex<T>) -> MutexGuard<T> {
    mutex.lock().await
}

/// Runtime shim for `Mutex::try_lock`.
pub fn mutex_try_lock<T>(mutex: &Mutex<T>) -> Option<MutexGuard<T>> {
    mutex.try_lock()
}

/// Runtime shim for `MutexGuard::get`.
pub fn mutex_guard_get<T>(guard: &MutexGuard<T>) -> T
where
    T: Clone,
{
    guard.get()
}

/// Runtime shim for `MutexGuard::set`.
pub fn mutex_guard_set<T>(guard: &MutexGuard<T>, value: T) {
    guard.set(value)
}

/// Runtime shim for constructing an `RwLock`.
pub fn rwlock_new<T>(value: T) -> RwLock<T> {
    RwLock::new(value)
}

/// Runtime shim for `RwLock::read`.
pub async fn rwlock_read<T>(lock: &RwLock<T>) -> RwLockReadGuard<T> {
    lock.read().await
}

/// Runtime shim for `RwLock::write`.
pub async fn rwlock_write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<T> {
    lock.write().await
}

/// Runtime shim for `RwLock::try_read`.
pub fn rwlock_try_read<T>(lock: &RwLock<T>) -> Option<RwLockReadGuard<T>> {
    lock.try_read()
}

/// Runtime shim for `RwLock::try_write`.
pub fn rwlock_try_write<T>(lock: &RwLock<T>) -> Option<RwLockWriteGuard<T>> {
    lock.try_write()
}

/// Runtime shim for `RwLockReadGuard::get`.
pub fn rwlock_read_guard_get<T>(guard: &RwLockReadGuard<T>) -> T
where
    T: Clone,
{
    guard.get()
}

/// Runtime shim for `RwLockWriteGuard::get`.
pub fn rwlock_write_guard_get<T>(guard: &RwLockWriteGuard<T>) -> T
where
    T: Clone,
{
    guard.get()
}

/// Runtime shim for `RwLockWriteGuard::set`.
pub fn rwlock_write_guard_set<T>(guard: &RwLockWriteGuard<T>, value: T) {
    guard.set(value)
}

/// Runtime shim for constructing a `Semaphore`.
pub fn semaphore_new(permits: i64) -> Semaphore {
    Semaphore::new(permits)
}

/// Runtime shim for `Semaphore::acquire`.
pub async fn semaphore_acquire(semaphore: &Semaphore) -> Result<SemaphorePermit, SemaphoreAcquireError> {
    semaphore.acquire().await
}

/// Runtime shim for `Semaphore::try_acquire`.
pub fn semaphore_try_acquire(semaphore: &Semaphore) -> Option<SemaphorePermit> {
    semaphore.try_acquire()
}

/// Runtime shim for `Semaphore::available_permits`.
pub fn semaphore_available_permits(semaphore: &Semaphore) -> i64 {
    semaphore.available_permits()
}

/// Runtime shim for constructing a `Barrier`.
pub fn barrier_new(count: i64) -> Barrier {
    Barrier::new(count)
}

/// Runtime shim for `Barrier::wait`.
pub async fn barrier_wait(barrier: &Barrier) -> i64 {
    barrier.wait().await
}

pub use Barrier as RawBarrier;
pub use Mutex as RawMutex;
pub use MutexGuard as RawMutexGuard;
pub use RwLock as RawRwLock;
pub use RwLockReadGuard as RawRwLockReadGuard;
pub use RwLockWriteGuard as RawRwLockWriteGuard;
pub use Semaphore as RawSemaphore;
pub use SemaphorePermit as RawSemaphorePermit;
pub use barrier_new as runtime_barrier_new;
pub use barrier_wait as runtime_barrier_wait;
pub use mutex_guard_get as runtime_mutex_guard_get;
pub use mutex_guard_set as runtime_mutex_guard_set;
pub use mutex_lock as runtime_mutex_lock;
pub use mutex_new as runtime_mutex_new;
pub use mutex_try_lock as runtime_mutex_try_lock;
pub use rwlock_new as runtime_rwlock_new;
pub use rwlock_read as runtime_rwlock_read;
pub use rwlock_read_guard_get as runtime_rwlock_read_guard_get;
pub use rwlock_try_read as runtime_rwlock_try_read;
pub use rwlock_try_write as runtime_rwlock_try_write;
pub use rwlock_write as runtime_rwlock_write;
pub use rwlock_write_guard_get as runtime_rwlock_write_guard_get;
pub use rwlock_write_guard_set as runtime_rwlock_write_guard_set;
pub use semaphore_acquire as runtime_semaphore_acquire;
pub use semaphore_available_permits as runtime_semaphore_available_permits;
pub use semaphore_new as runtime_semaphore_new;
pub use semaphore_try_acquire as runtime_semaphore_try_acquire;

#[cfg(test)]
mod tests {
    use super::Semaphore;

    #[tokio::test]
    async fn semaphore_acquire_returns_error_when_closed() {
        let semaphore = Semaphore::new(1);
        semaphore.0.close();

        let result = semaphore.acquire().await;
        assert!(
            result.is_err(),
            "expected semaphore acquire error when semaphore is closed"
        );

        if let Err(err) = result {
            assert_eq!(err.message(), "failed to acquire semaphore permit: semaphore closed");
            assert!(err.source().is_none());
        }
    }
}
