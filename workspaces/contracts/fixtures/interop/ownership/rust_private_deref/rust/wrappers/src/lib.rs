//! Public wrappers expose methods through private generic Deref targets.

use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

/// A public source alias of the wrapper.
pub use Vec as Wrapper;

/// An owned token whose access implementation exists only on a shared reference.
pub struct Owner;

impl Owner {
    /// Return an owned token.
    pub fn new() -> Self {
        Self
    }
}

/// Read through a token borrowed for the trait's lifetime.
pub trait Access<'a> {
    /// Return a scalar observation.
    fn value(&self) -> u32;
}

impl<'a> Access<'a> for &'a Owner {
    /// Observe the token without taking its owner.
    fn value(&self) -> u32 {
        7
    }
}

/// Borrow a generic input whose trait implementation requires another shared reference.
pub fn read<'a>(owner: &impl Access<'a>) -> u32 {
    owner.value()
}

/// A token that supports both owned and reference implementations with distinct observations.
pub struct Both;

impl Both {
    /// Return an owned token whose implementation should remain preferred.
    pub fn new() -> Self {
        Self
    }
}

impl<'a> Access<'a> for Both {
    /// Identify the owned implementation.
    fn value(&self) -> u32 {
        3
    }
}

impl<'a> Access<'a> for &'a Both {
    /// Identify the reference implementation when explicitly selected.
    fn value(&self) -> u32 {
        4
    }
}

/// A public owner whose written const arguments still need a value contract.
pub struct Fixed<const N: usize = 1>;

impl<const N: usize> Fixed<N> {
    /// Create a fixed owner using the impl's const parameter.
    pub fn new() -> Self {
        Self
    }
}

/// A public owner whose target cannot be named outside this crate.
pub struct Vec<'a, T>(private::Alias<T>, PhantomData<&'a ()>);

impl<T> Vec<'_, T> {
    /// Create an empty owner through the reference-only access contract.
    pub fn new_in<'a>(owner: &impl Access<'a>) -> Self {
        let _ = owner.value();
        Self::new()
    }

    /// Create an empty owner.
    pub fn new() -> Self {
        Self(
            private::Target {
                values: std::vec::Vec::new(),
                allocation: PhantomData,
            },
            PhantomData,
        )
    }
}

impl<T> Deref for Vec<'_, T> {
    type Target = private::Alias<T>;

    /// Lend the hidden target for shared method lookup.
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Vec<'_, T> {
    /// Lend the hidden target for mutable method lookup.
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

mod private {
    use std::marker::PhantomData;

    /// A dependency whose omitted const parameter uses its Rust declaration's default.
    pub struct Allocation<const ALIGN: usize = 1>;

    /// An alias used by the public associated type.
    pub type Alias<T> = Target<T>;

    /// Generic storage reachable only by compiler-selected adjustments.
    pub struct Target<T, A = Allocation> {
        pub(super) values: Vec<T>,
        pub(super) allocation: PhantomData<A>,
    }

    impl<T, A> Target<T, A> {
        /// Return the current length through a shared receiver.
        pub fn len(&self) -> usize {
            self.values.len()
        }

        /// Append a value through a mutable receiver.
        pub fn push(&mut self, value: T) {
            self.values.push(value);
        }

        /// Return an owned element with the target's specialized argument.
        pub fn remove(&mut self) -> T {
            self.values.remove(0)
        }

        /// Return a second hidden type whose methods need dependency metadata too.
        pub fn receipt(&self) -> Receipt {
            Receipt(self.len())
        }

        /// A private method must never enter admitted method lookup.
        fn secret(&self) -> usize {
            99
        }
    }

    /// Another dependency-only owner reached through an admitted signature.
    pub struct Receipt(usize);

    impl Receipt {
        /// Read the receipt through its admitted shared signature.
        pub fn count(&self) -> usize {
            self.0
        }
    }
}
