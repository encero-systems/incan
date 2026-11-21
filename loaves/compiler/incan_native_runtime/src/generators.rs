//! Lazy generator sources, adapters and consumers over the primitive `Generator` carrier.
//!
//! Every adapter draws its source only when it is polled, on the consumer's thread, with no prefetching.

use std::collections::HashSet;

use incan_std_primitives::iter::{Generator, batch, enumerate, nonnegative_count, range, zip};

/// Poll once, appending the item on success and leaving `output` unchanged on exhaustion.
pub fn poll_generator<T>(values: &mut Generator<T>, output: &mut Vec<T>) -> bool {
    match values.poll() {
        Some(value) => {
            output.push(value);
            true
        }
        None => false,
    }
}

/// Yield owned list storage lazily, without another snapshot.
pub fn list_generator<T: 'static>(values: Vec<T>) -> Generator<T> {
    Generator::new(values.into_iter())
}

/// Yield one set snapshot's elements in the set's iteration order.
pub fn set_generator<T: 'static>(values: HashSet<T>) -> Generator<T> {
    list_generator(values.into_iter().collect())
}

/// Construct the builtin range now, refusing a zero step, and yield its integers lazily.
pub fn range_generator(start: i64, end: i64, step: i64) -> Generator<i64> {
    Generator::new(range(start, end, step))
}

/// Yield the one item a generator-expression step accepted.
pub fn once_generator<T: 'static>(value: T) -> Generator<T> {
    Generator::new(std::iter::once(value))
}

/// Yield nothing, for an item a generator-expression step filtered out.
pub fn empty_generator<T: 'static>() -> Generator<T> {
    list_generator(Vec::new())
}

/// Expand each integer item through a step generator, drawing both only when polled.
pub fn int_step_generator<T: 'static>(source: Generator<i64>, step: fn(i64) -> Generator<T>) -> Generator<T> {
    Generator::new(source.flat_map(step))
}

/// Expand each float item through a step generator, drawing both only when polled.
pub fn float_step_generator<T: 'static>(source: Generator<f64>, step: fn(f64) -> Generator<T>) -> Generator<T> {
    Generator::new(source.flat_map(step))
}

/// Expand each boolean item through a step generator, drawing both only when polled.
pub fn bool_step_generator<T: 'static>(source: Generator<bool>, step: fn(bool) -> Generator<T>) -> Generator<T> {
    Generator::new(source.flat_map(step))
}

/// Forward an integer source and its owned callable to the lazy chain.
pub fn int_callable_step_generator<T: 'static, F: Fn(i64) -> Generator<T> + 'static>(
    source: Generator<i64>,
    step: F,
) -> Generator<T> {
    item_callable_step_generator(source, step)
}

/// Forward a float source and its owned callable to the lazy chain.
pub fn float_callable_step_generator<T: 'static, F: Fn(f64) -> Generator<T> + 'static>(
    source: Generator<f64>,
    step: F,
) -> Generator<T> {
    item_callable_step_generator(source, step)
}

/// Forward a boolean source and its owned callable to the lazy chain.
pub fn bool_callable_step_generator<T: 'static, F: Fn(bool) -> Generator<T> + 'static>(
    source: Generator<bool>,
    step: F,
) -> Generator<T> {
    item_callable_step_generator(source, step)
}

/// Forward owned text and its callable to the lazy chain.
pub fn str_callable_step_generator<T: 'static, F: Fn(String) -> Generator<T> + 'static>(
    source: Generator<String>,
    step: F,
) -> Generator<T> {
    item_callable_step_generator(source, step)
}

/// Expand source items only as they are polled; the chain owns its source and callable on the consumer's thread.
pub fn item_callable_step_generator<S: 'static, T: 'static, F: Fn(S) -> Generator<T> + 'static>(
    source: Generator<S>,
    step: F,
) -> Generator<T> {
    Generator::new(source.flat_map(step))
}

/// Stop after at most `count` items; a negative count yields nothing.
pub fn take_generator<T: 'static>(values: Generator<T>, count: i64) -> Generator<T> {
    values.take(count)
}

/// Skip lazily with a nonnegative count.
pub fn skip_generator<T: 'static>(values: Generator<T>, count: i64) -> Generator<T> {
    Generator::new(values.skip(nonnegative_count(count)))
}

/// Draw fixed-size batches only when polled.
pub fn batch_generator<T: 'static>(values: Generator<T>, size: i64) -> Generator<Vec<T>> {
    Generator::new(batch(values, size))
}

/// Yield both sources in order.
pub fn chain_generators<T: 'static>(first: Generator<T>, second: Generator<T>) -> Generator<T> {
    Generator::new(first.chain(second))
}

/// The cursor of a predicate prefix: the source, the predicate and whether the prefix has ended.
struct PrefixCursor {
    source: Generator<i64>,
    predicate: fn(i64) -> bool,
    finished: bool,
}

/// Yield items until the first rejected one, then stay exhausted; source exhaustion also ends the prefix.
struct TakePrefix(PrefixCursor);

impl Iterator for TakePrefix {
    type Item = i64;

    fn next(&mut self) -> Option<i64> {
        let cursor = &mut self.0;
        if cursor.finished {
            return None;
        }
        if let Some(value) = cursor.source.poll()
            && (cursor.predicate)(value)
        {
            return Some(value);
        }
        cursor.finished = true;
        None
    }
}

/// Test only the initial prefix; after the first rejected item the source yields as it ordinarily does.
struct SkipPrefix(PrefixCursor);

impl Iterator for SkipPrefix {
    type Item = i64;

    fn next(&mut self) -> Option<i64> {
        let cursor = &mut self.0;
        while !cursor.finished {
            let value = cursor.source.poll()?;
            if !(cursor.predicate)(value) {
                cursor.finished = true;
                return Some(value);
            }
        }
        cursor.source.poll()
    }
}

/// Yield until the predicate rejects an item, polling lazily on the consumer's thread.
pub fn take_while_int_generator(values: Generator<i64>, predicate: fn(i64) -> bool) -> Generator<i64> {
    Generator::new(TakePrefix(PrefixCursor {
        source: values,
        predicate,
        finished: false,
    }))
}

/// Skip accepted items, testing no item after the first rejection.
pub fn skip_while_int_generator(values: Generator<i64>, predicate: fn(i64) -> bool) -> Generator<i64> {
    Generator::new(SkipPrefix(PrefixCursor {
        source: values,
        predicate,
        finished: false,
    }))
}

/// Expand each integer item into a list when it is polled.
pub fn flat_map_int_generator<U: 'static>(values: Generator<i64>, expand: fn(i64) -> Vec<U>) -> Generator<U> {
    Generator::new(values.flat_map(expand))
}

/// Expand each integer item into a generator when it is polled, drawing its items one at a time.
pub fn flat_map_int_generators<U: 'static>(values: Generator<i64>, expand: fn(i64) -> Generator<U>) -> Generator<U> {
    Generator::new(values.flat_map(expand))
}

/// Expand each integer into a set and yield its distinct values when the expansion is polled.
pub fn flat_map_int_set_generator(values: Generator<i64>, expand: fn(i64) -> HashSet<i64>) -> Generator<i64> {
    Generator::new(values.flat_map(expand))
}

/// Transform owned text through a callable taking each item by value.
pub fn map_text_callable_generator<U: 'static, F: Fn(String) -> U + 'static>(
    values: Generator<String>,
    transform: F,
) -> Generator<U> {
    Generator::new(values.map(transform))
}

/// Expand owned text lazily through a list-returning callable.
pub fn flat_map_text_callable_generator<U: 'static, F: Fn(String) -> Vec<U> + 'static>(
    values: Generator<String>,
    expand: F,
) -> Generator<U> {
    Generator::new(values.flat_map(expand))
}

/// Stop at the first accepted text item, leaving later items unpolled.
pub fn any_text_callable_generator<F: Fn(String) -> bool>(values: Generator<String>, predicate: F) -> bool {
    let mut values = values;
    values.any(predicate)
}

/// Pair source items with signed positions lazily.
pub fn enumerate_generator<T: 'static>(values: Generator<T>) -> Generator<(i64, T)> {
    enumerate(values)
}

/// Pair items lazily, polling left before right until either is exhausted.
pub fn zip_generators<T: 'static, U: 'static>(left: Generator<T>, right: Generator<U>) -> Generator<(T, U)> {
    zip(left, right)
}

/// Transform each integer item when it is polled.
pub fn map_int_generator<U: 'static>(values: Generator<i64>, transform: fn(i64) -> U) -> Generator<U> {
    Generator::new(values.map(transform))
}

/// Transform each float item when it is polled.
pub fn map_float_generator<U: 'static>(values: Generator<f64>, transform: fn(f64) -> U) -> Generator<U> {
    Generator::new(values.map(transform))
}

/// Transform each boolean item when it is polled.
pub fn map_bool_generator<U: 'static>(values: Generator<bool>, transform: fn(bool) -> U) -> Generator<U> {
    Generator::new(values.map(transform))
}

/// Keep the integer items the predicate accepts, testing each when it is polled.
pub fn filter_int_generator(values: Generator<i64>, predicate: fn(i64) -> bool) -> Generator<i64> {
    Generator::new(values.filter(move |value| predicate(*value)))
}

/// Keep the float items the predicate accepts, testing each when it is polled.
pub fn filter_float_generator(values: Generator<f64>, predicate: fn(f64) -> bool) -> Generator<f64> {
    Generator::new(values.filter(move |value| predicate(*value)))
}

/// Keep the boolean items the predicate accepts, testing each when it is polled.
pub fn filter_bool_generator(values: Generator<bool>, predicate: fn(bool) -> bool) -> Generator<bool> {
    Generator::new(values.filter(move |value| predicate(*value)))
}

/// Drain and count the remaining items.
pub fn count_generator<T: 'static>(values: Generator<T>) -> i64 {
    values.count() as i64
}

/// Drain and sum the remaining integers with the release runtime's wrapping addition.
pub fn sum_int_generator(values: Generator<i64>) -> i64 {
    values.fold(0_i64, i64::wrapping_add)
}

/// Combine items from `initial`, in order.
pub fn fold_int_generator(values: Generator<i64>, initial: i64, combine: fn(i64, i64) -> i64) -> i64 {
    values.fold(initial, combine)
}

/// Reduce items from the explicit `initial`, in order.
pub fn reduce_int_generator(values: Generator<i64>, initial: i64, combine: fn(i64, i64) -> i64) -> i64 {
    values.fold(initial, combine)
}

/// Stop at the first accepted item.
pub fn any_int_generator(values: Generator<i64>, predicate: fn(i64) -> bool) -> bool {
    let mut values = values;
    values.any(predicate)
}

/// Stop at the first rejected item.
pub fn all_int_generator(values: Generator<i64>, predicate: fn(i64) -> bool) -> bool {
    let mut values = values;
    values.all(predicate)
}

/// The first accepted item, or None at exhaustion.
pub fn find_int_generator(values: Generator<i64>, predicate: fn(i64) -> bool) -> Option<i64> {
    let mut values = values;
    values.find(|value| predicate(*value))
}

/// Run the action on every remaining item, in order.
pub fn for_each_int_generator(values: Generator<i64>, action: fn(i64)) {
    values.for_each(action);
}
