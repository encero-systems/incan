//! Runtime helpers for compiler-generated module statics.
//!
//! RFC 052 models `static` declarations as compiler-owned storage cells with explicit read/update operations.

use std::sync::{Arc, LazyLock, RwLock};

fn read_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    match lock.read() {
        Ok(guard) => guard,
        Err(poison) => poison.into_inner(),
    }
}

fn write_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    match lock.write() {
        Ok(guard) => guard,
        Err(poison) => poison.into_inner(),
    }
}

/// Shared storage backing for one compiler-emitted module static.
#[derive(Debug)]
pub struct StaticCell<T> {
    inner: Arc<RwLock<T>>,
}

impl<T> StaticCell<T> {
    /// Create a new storage cell initialized with `value`.
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(RwLock::new(value)),
        }
    }

    /// Borrow the current value immutably for the duration of `f`.
    pub fn with_ref<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        let guard = read_lock(&self.inner);
        f(&guard)
    }

    /// Borrow the current value mutably for the duration of `f`.
    pub fn with_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let mut guard = write_lock(&self.inner);
        f(&mut guard)
    }

    /// Read the current value by cloning it out of the storage cell.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.with_ref(Clone::clone)
    }

    fn binding(&self) -> StaticBinding<T> {
        StaticBinding::Handle(self.inner.clone())
    }
}

/// Hidden local binding wrapper used for direct aliases created from a module static.
#[derive(Debug)]
pub enum StaticBinding<T> {
    Handle(Arc<RwLock<T>>),
    Value(T),
}

impl<T> StaticBinding<T> {
    /// Create a live local alias for a compiler-emitted static item.
    pub fn from_static(cell: &LazyLock<StaticCell<T>>) -> Self {
        cell.binding()
    }

    /// Wrap a plain local value.
    pub fn from_value(value: T) -> Self {
        Self::Value(value)
    }

    /// Borrow the current value immutably for the duration of `f`.
    pub fn with_ref<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        match self {
            Self::Handle(inner) => {
                let guard = read_lock(inner);
                f(&guard)
            }
            Self::Value(value) => f(value),
        }
    }

    /// Borrow the current value mutably for the duration of `f`.
    pub fn with_mut<R>(&mut self, f: impl FnOnce(&mut T) -> R) -> R {
        match self {
            Self::Handle(inner) => {
                let mut guard = write_lock(inner);
                f(&mut guard)
            }
            Self::Value(value) => f(value),
        }
    }

    /// Read the current value by cloning it out of the binding.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.with_ref(Clone::clone)
    }
}
