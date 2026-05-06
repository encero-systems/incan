//! Iteration helpers for Incan-generated Rust code.
//!
//! Most RFC 088 iterator adapter semantics are dogfooded in `std.derives.collection` as Incan protocol defaults.
//! This Rust module remains the runtime boundary for behavior that the current backend still emits directly as Rust
//! support:
//!
//! - `Generator<T>` gives RFC 006 generator functions and generator expressions one stable emitted Rust return type.
//! - [`range`] implements Python-like `range(start, end, step)` semantics, including negative steps and zero-step
//!   diagnostics.
//! - [`nonnegative_count`] centralizes the signed Incan `int` to Rust `usize` conversion used when the backend lowers
//!   count-limited adapters to native Rust iterator chains.
//! - [`batch`] provides the lazy RFC 088 batch adapter for the native Rust iterator path. The same semantics are also
//!   represented in Incan by `BatchIterator[T]`; this helper exists because known iterator methods currently lower to
//!   Rust iterator chains instead of calling the Incan protocol defaults.

use crate::errors::{raise, raise_value_error};
use incan_core::errors::IncanError;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread;

/// Runtime representation for RFC 006 lazy generator values.
///
/// Lowering and emission use this wrapper for both `yield`-based generator functions and generator expressions. The
/// wrapper intentionally owns a boxed iterator so emitted code has one stable concrete return type for `Generator[T]`
/// without leaking the Rust adapter chain that produced it.
pub struct Generator<T> {
    iter: Box<dyn Iterator<Item = T>>,
}

impl<T> Generator<T> {
    /// Create a generator from an owned Rust iterator.
    ///
    /// The iterator must be `'static` because the generated `Generator[T]` value is allowed to escape the local
    /// expression where it was created. Backend lowering should move or clone captured inputs into the iterator chain
    /// before calling this constructor.
    #[must_use]
    pub fn new<I>(iter: I) -> Self
    where
        I: Iterator<Item = T> + 'static,
    {
        Self { iter: Box::new(iter) }
    }

    /// Create a lazy generator from a compiler-emitted body closure.
    #[must_use]
    pub fn spawn<F>(f: F) -> Self
    where
        T: Send + 'static,
        F: FnOnce(GeneratorYield<T>) + Send + 'static,
    {
        Self::new(SpawnedGenerator {
            producer: Some(Box::new(f)),
            receiver: None,
        })
    }

    /// Lazily transform each yielded value.
    #[must_use]
    pub fn map<U, F>(self, f: F) -> Generator<U>
    where
        T: 'static,
        U: 'static,
        F: FnMut(T) -> U + 'static,
    {
        Generator::new(self.iter.map(f))
    }

    /// Lazily keep yielded values accepted by `predicate`.
    #[must_use]
    pub fn filter<F>(self, predicate: F) -> Self
    where
        T: 'static,
        F: FnMut(&T) -> bool + 'static,
    {
        Generator::new(self.iter.filter(predicate))
    }

    /// Lazily stop after at most `count` yielded values.
    ///
    /// Negative counts produce an empty generator, matching Python's empty slice limits.
    #[must_use]
    pub fn take(self, count: i64) -> Self
    where
        T: 'static,
    {
        let limit = match usize::try_from(count) {
            Ok(limit) => limit,
            Err(_) if count < 0 => 0,
            Err(_) => usize::MAX,
        };
        Generator::new(self.iter.take(limit))
    }

    /// Materialize the remaining yielded values into an owned list.
    pub fn collect(self) -> Vec<T> {
        self.iter.collect()
    }
}

/// Iterator adapter that starts compiler-emitted generator bodies on first consumption.
struct SpawnedGenerator<T> {
    producer: Option<Box<dyn FnOnce(GeneratorYield<T>) + Send>>,
    receiver: Option<Receiver<T>>,
}

impl<T> SpawnedGenerator<T>
where
    T: Send + 'static,
{
    /// Start the producer thread once the consumer asks for the first item.
    fn ensure_started(&mut self) {
        if self.receiver.is_some() {
            return;
        }

        let Some(producer) = self.producer.take() else {
            return;
        };
        let (sender, receiver) = sync_channel(0);
        thread::spawn(move || producer(GeneratorYield { sender }));
        self.receiver = Some(receiver);
    }
}

impl<T> Iterator for SpawnedGenerator<T>
where
    T: Send + 'static,
{
    type Item = T;

    /// Start the producer if needed and receive the next yielded item.
    fn next(&mut self) -> Option<Self::Item> {
        self.ensure_started();
        self.receiver.as_ref().and_then(|receiver| receiver.recv().ok())
    }
}

/// Yield handle passed into compiler-emitted generator bodies.
pub struct GeneratorYield<T> {
    sender: SyncSender<T>,
}

impl<T> GeneratorYield<T> {
    /// Suspend the producer until the consumer is ready for the next value.
    pub fn yield_value(&self, value: T) {
        let _ = self.sender.send(value);
    }
}

impl<T> Iterator for Generator<T> {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

/// A Python-like `range(start, end, step)` iterator over `i64`.
#[derive(Debug, Clone)]
pub struct PyRange {
    cur: i64,
    end: i64,
    step: i64,
}

impl Iterator for PyRange {
    type Item = i64;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.step > 0 {
            if self.cur >= self.end {
                return None;
            }
        } else if self.cur <= self.end {
            return None;
        }
        let out = self.cur;
        self.cur += self.step;
        Some(out)
    }
}

/// Create a Python-like `range(start, end, step)`.
///
/// - End is **exclusive**.
/// - Supports negative steps.
///
/// TODO(perf): Extend lowering/codegen specialization beyond literal `step == 1`(for example, constant-folded
///             expressions that evaluate to `1`) so more loops can use native Rust ranges where semantics are
/// identical.
///
/// ## Panics
/// - `ValueError: range() arg 3 must not be zero` if `step == 0`.
#[inline]
pub fn range(start: i64, end: i64, step: i64) -> PyRange {
    if step == 0 {
        raise(IncanError::range_step_zero());
    }
    PyRange { cur: start, end, step }
}

/// Convert an Incan iterator count argument to a Rust `usize` for nonnegative-count adapters.
///
/// RFC 088 defines `take(n)` and `skip(n)` so values less than or equal to zero do not create large wrapped counts.
/// This helper centralizes the signed-to-`usize` boundary for generated Rust.
#[inline]
pub fn nonnegative_count(n: i64) -> usize {
    if n <= 0 {
        return 0;
    }
    match usize::try_from(n) {
        Ok(value) => value,
        Err(_) => usize::MAX,
    }
}

/// Lazy fixed-size batch adapter used by generated Rust for RFC 088 `.batch(size)`.
#[derive(Debug, Clone)]
pub struct Batch<I> {
    iter: I,
    size: usize,
}

impl<I> Iterator for Batch<I>
where
    I: Iterator,
{
    type Item = Vec<I::Item>;

    /// Yield the next non-empty batch, including a final short batch when the source iterator is exhausted.
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let mut items = Vec::with_capacity(self.size);
        for _ in 0..self.size {
            let Some(item) = self.iter.next() else {
                break;
            };
            items.push(item);
        }
        if items.is_empty() { None } else { Some(items) }
    }
}

/// Create a lazy fixed-size batch adapter.
///
/// The final non-empty batch is yielded even when it contains fewer than `size` items. Invalid sizes raise
/// `ValueError: iterator batch size must be greater than zero`.
#[inline]
pub fn batch<I>(iter: I, size: i64) -> Batch<I::IntoIter>
where
    I: IntoIterator,
{
    if size <= 0 {
        raise_value_error("iterator batch size must be greater than zero");
    }
    let size = match usize::try_from(size) {
        Ok(value) => value,
        Err(_) => raise_value_error("iterator batch size is too large"),
    };
    Batch {
        iter: iter.into_iter(),
        size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_positive_step() {
        let xs: Vec<i64> = range(0, 5, 1).collect();
        assert_eq!(xs, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn range_negative_step() {
        let xs: Vec<i64> = range(5, 0, -2).collect();
        assert_eq!(xs, vec![5, 3, 1]);
    }

    #[test]
    #[should_panic(expected = "ValueError: range() arg 3 must not be zero")]
    fn range_zero_step_panics_with_value_error() {
        let _ = range(0, 5, 0);
    }

    #[test]
    fn generator_helpers_are_lazy_and_chainable() {
        let values = Generator::new(0..)
            .map(|value| value * 2)
            .filter(|value| value % 3 == 0)
            .take(4)
            .collect();

        assert_eq!(values, vec![0, 6, 12, 18]);
    }

    #[test]
    fn generator_take_negative_count_yields_empty_list() {
        let values = Generator::new(0..5).take(-1).collect();

        assert_eq!(values, Vec::<i32>::new());
    }

    #[test]
    fn generator_spawn_yields_in_order() {
        let values = Generator::spawn(|yielder| {
            yielder.yield_value(1);
            yielder.yield_value(2);
        })
        .collect();

        assert_eq!(values, vec![1, 2]);
    }

    #[test]
    fn generator_spawn_defers_body_until_first_next() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let mut values = Generator::spawn(move |yielder| {
            observed.fetch_add(1, Ordering::SeqCst);
            yielder.yield_value(1);
        });

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(values.next(), Some(1));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn nonnegative_count_clamps_negative_values_to_zero() {
        assert_eq!(nonnegative_count(-3), 0);
        assert_eq!(nonnegative_count(0), 0);
        assert_eq!(nonnegative_count(4), 4);
    }

    #[test]
    fn batch_yields_fixed_size_batches_with_final_short_batch() {
        let batches: Vec<Vec<i64>> = batch(0..5, 2).collect();
        assert_eq!(batches, vec![vec![0, 1], vec![2, 3], vec![4]]);
    }

    #[test]
    fn batch_is_lazy() {
        let mut batches = batch(0.., 3);
        assert_eq!(batches.next(), Some(vec![0, 1, 2]));
        assert_eq!(batches.next(), Some(vec![3, 4, 5]));
    }

    #[test]
    #[should_panic(expected = "ValueError: iterator batch size must be greater than zero")]
    fn batch_zero_size_panics_with_value_error() {
        let _ = batch(0..5, 0);
    }
}
