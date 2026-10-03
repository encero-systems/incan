//! A trait object behind a shared pointer, viewed through `as_ref` and handed to a function over `&dyn Array`, after
//! the Arrow shape the retired conversion test was written for.
use std::sync::Arc;

/// Something with a length.
pub trait Array {
    /// How many elements the array holds.
    fn len(&self) -> usize;
}

struct Fixed(usize);

impl Array for Fixed {
    fn len(&self) -> usize {
        self.0
    }
}

/// An array of three elements behind a shared pointer.
pub fn make() -> Arc<dyn Array> {
    Arc::new(Fixed(3))
}

/// How many elements the array holds.
pub fn count(array: &dyn Array) -> usize {
    array.len()
}
