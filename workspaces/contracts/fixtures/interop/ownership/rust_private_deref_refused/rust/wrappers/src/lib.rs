//! Public wrappers expose methods through private generic Deref targets.

use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

/// A public source alias of the wrapper.
pub use Vec as Wrapper;

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
