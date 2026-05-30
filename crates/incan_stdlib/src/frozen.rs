//! Provide frozen (deeply immutable) runtime types for RFC 008.
//!
//! These types are designed for **compile-time construction** by referencing baked `'static` data that the Incan
//! compiler emits for module-level consts.
//!
//! ## Notes
//! - All frozen collection wrappers are backed by `'static` slices.
//! - APIs are intentionally read-only to enforce deep immutability at the type level.
//!
//! ## Examples
//! ```rust
//! use incan_stdlib::prelude::*;
//!
//! static NUMS: [i64; 3] = [1, 2, 3];
//! const L: FrozenList<i64> = FrozenList::new(&NUMS);
//! assert_eq!(L.len(), 3);
//! ```

use core::fmt;

/// Represent an immutable string baked into the binary.
///
/// ## Notes
/// - Backed by a `'static` string slice, so it can be used in `const` contexts.
/// - Intended to be produced by the compiler for module-level `const` initializers.
///
/// ## Examples
/// ```rust
/// use incan_stdlib::prelude::*;
///
/// const S: FrozenStr = FrozenStr::new("hello");
/// assert_eq!(S.as_str(), "hello");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrozenStr(&'static str);

impl FrozenStr {
    /// Construct a frozen string from a `'static` string slice.
    pub const fn new(s: &'static str) -> Self {
        Self(s)
    }

    /// Return the underlying string slice.
    pub const fn as_str(&self) -> &'static str {
        self.0
    }

    /// Return the string length in bytes.
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Return true if the string is empty.
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for FrozenStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FrozenStr").field(&self.0).finish()
    }
}

impl fmt::Display for FrozenStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl AsRef<str> for FrozenStr {
    fn as_ref(&self) -> &str {
        self.0
    }
}

/// Represent an immutable byte string baked into the binary.
///
/// ## Notes
/// - Backed by a `'static` byte slice, so it can be used in `const` contexts.
/// - Intended to be produced by the compiler for module-level `const` initializers.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrozenBytes(&'static [u8]);

impl FrozenBytes {
    /// Construct frozen bytes from a baked `'static` byte slice.
    pub const fn new(bytes: &'static [u8]) -> Self {
        Self(bytes)
    }

    /// Return the underlying byte slice.
    pub const fn as_slice(&self) -> &'static [u8] {
        self.0
    }

    /// Return the length in bytes.
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Return true if empty.
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterate over bytes.
    pub fn iter(&self) -> core::slice::Iter<'static, u8> {
        self.0.iter()
    }
}

impl fmt::Debug for FrozenBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FrozenBytes").field(&self.0).finish()
    }
}

impl fmt::Display for FrozenBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Render similarly to a byte string literal
        f.write_str("b\"")?;
        for b in self.0 {
            // Use escaped representation for non-printables
            if *b == b'\\' || *b == b'"' {
                write!(f, "\\{}", *b as char)?;
            } else if b.is_ascii_graphic() || *b == b' ' {
                write!(f, "{}", *b as char)?;
            } else {
                write!(f, "\\x{:02x}", b)?;
            }
        }
        f.write_str("\"")
    }
}

impl AsRef<[u8]> for FrozenBytes {
    fn as_ref(&self) -> &[u8] {
        self.0
    }
}

impl core::ops::Index<usize> for FrozenBytes {
    type Output = u8;

    fn index(&self, idx: usize) -> &Self::Output {
        &self.0[idx]
    }
}

/// Represent an immutable list backed by a baked `'static` slice.
///
/// ## Notes
/// - Backed by `&'static [T]`, so `T` must be `'static`.
/// - This is a *read-only* wrapper (no `push`, `pop`, etc.).
///
/// ## Examples
/// ```rust
/// use incan_stdlib::prelude::*;
///
/// static DATA: [i64; 3] = [10, 20, 30];
/// const L: FrozenList<i64> = FrozenList::new(&DATA);
/// assert_eq!(L.get(1), Some(&20));
/// ```
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FrozenList<T: 'static> {
    data: &'static [T],
}

impl<T: 'static> FrozenList<T> {
    /// Construct a frozen list from a baked `'static` slice.
    pub const fn new(data: &'static [T]) -> Self {
        Self { data }
    }

    /// Return the number of elements.
    pub const fn len(&self) -> usize {
        self.data.len()
    }

    /// Return true if the list is empty.
    pub const fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Iterate over elements.
    pub fn iter(&self) -> core::slice::Iter<'static, T> {
        self.data.iter()
    }

    /// Get an element by index.
    pub fn get(&self, idx: usize) -> Option<&'static T> {
        self.data.get(idx)
    }

    /// Return the underlying slice.
    pub fn as_slice(&self) -> &'static [T] {
        self.data
    }
}

impl<T: 'static> AsRef<[T]> for FrozenList<T> {
    /// Return a borrowed view of this value.
    fn as_ref(&self) -> &[T] {
        self.data
    }
}

impl<T: 'static> core::ops::Deref for FrozenList<T> {
    type Target = [T];

    /// Return the underlying target for deref coercions.
    fn deref(&self) -> &Self::Target {
        self.data
    }
}

impl<T: fmt::Debug> fmt::Debug for FrozenList<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FrozenList").field(&self.data).finish()
    }
}

impl<T: fmt::Display> fmt::Display for FrozenList<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[")?;
        for (i, item) in self.data.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{}", item)?;
        }
        f.write_str("]")
    }
}

impl<T: 'static> core::ops::Index<usize> for FrozenList<T> {
    type Output = T;

    /// Index into the frozen list.
    ///
    /// ## Panics
    /// Panics if the index is out of bounds.
    fn index(&self, idx: usize) -> &Self::Output {
        &self.data[idx]
    }
}

// Clippy can suggest eliding `'a` here, but this `IntoIterator` impl needs a named lifetime because it is referenced
// in the associated types (`Item` / `IntoIter`). Keeping `'a` explicit makes the relationship between
// `&FrozenList<T>` and the yielded `&T` clear.
#[allow(clippy::needless_lifetimes)]
impl<'a, T: 'static> IntoIterator for &'a FrozenList<T> {
    type Item = &'a T;
    type IntoIter = core::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.data.iter()
    }
}

/// Iterate over a frozen list by value.
///
/// Yields references because the backing storage is `'static`.
impl<T: 'static> IntoIterator for FrozenList<T> {
    type Item = &'static T;
    type IntoIter = core::slice::Iter<'static, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.data.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::FrozenList;
    use crate::collections::__private::{list_max_copy, list_min_copy};

    static NUMS: [i64; 3] = [3, 1, 4];

    #[test]
    fn frozen_list_coerces_to_slice_for_runtime_helpers() {
        let numbers = FrozenList::new(&NUMS);

        assert_eq!(numbers.as_ref(), &[3, 1, 4]);
        assert_eq!(list_min_copy(&numbers), 1);
        assert_eq!(list_max_copy(&numbers), 4);
    }
}

/// Represent an immutable set backed by a baked `'static` slice.
///
/// ## Notes
/// - Membership checks are **linear-time** (`O(n)`) by design (no hashing at runtime).
/// - This is intended for small, baked sets.
///
/// ## Examples
/// ```rust
/// use incan_stdlib::prelude::*;
///
/// static DATA: [i64; 2] = [1, 3];
/// const S: FrozenSet<i64> = FrozenSet::new(&DATA);
/// assert!(S.contains(&3));
/// ```
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FrozenSet<T: 'static> {
    data: &'static [T],
}

impl<T: 'static> FrozenSet<T> {
    /// Construct a frozen set from a baked `'static` slice.
    pub const fn new(data: &'static [T]) -> Self {
        Self { data }
    }

    /// Return the number of elements.
    pub const fn len(&self) -> usize {
        self.data.len()
    }

    /// Return true if the set is empty.
    pub const fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Iterate over elements.
    pub fn iter(&self) -> core::slice::Iter<'static, T> {
        self.data.iter()
    }

    /// Return true if the set contains `item` (linear scan).
    pub fn contains(&self, item: &T) -> bool
    where
        T: PartialEq,
    {
        self.data.iter().any(|x| x == item)
    }

    /// Return the underlying slice.
    pub fn as_slice(&self) -> &'static [T] {
        self.data
    }
}

impl<T: fmt::Debug> fmt::Debug for FrozenSet<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FrozenSet").field(&self.data).finish()
    }
}

impl<T: fmt::Display> fmt::Display for FrozenSet<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("{")?;
        for (i, item) in self.data.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{}", item)?;
        }
        f.write_str("}")
    }
}

/// Represent an immutable dictionary backed by baked `'static` key-value pairs.
///
/// ## Notes
/// - Lookups are **linear-time** (`O(n)`) by design (no hashing at runtime).
/// - This is intended for small, baked dictionaries.
///
/// ## Examples
/// ```rust
/// use incan_stdlib::prelude::*;
///
/// static DATA: [(FrozenStr, i64); 2] = [(FrozenStr::new("a"), 1), (FrozenStr::new("b"), 2)];
/// const D: FrozenDict<FrozenStr, i64> = FrozenDict::new(&DATA);
/// assert_eq!(D.get(&FrozenStr::new("b")), Some(&2));
/// ```
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FrozenDict<K: 'static, V: 'static> {
    data: &'static [(K, V)],
}

impl<K: 'static, V: 'static> FrozenDict<K, V> {
    /// Construct a frozen dict from baked key-value pairs.
    pub const fn new(data: &'static [(K, V)]) -> Self {
        Self { data }
    }

    /// Return the number of entries.
    pub const fn len(&self) -> usize {
        self.data.len()
    }

    /// Return true if the dict is empty.
    pub const fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Iterate over entries.
    pub fn iter(&self) -> core::slice::Iter<'static, (K, V)> {
        self.data.iter()
    }

    /// Return the value for `key`, if present (linear scan).
    pub fn get(&self, key: &K) -> Option<&'static V>
    where
        K: PartialEq,
    {
        self.data
            .iter()
            .find_map(|(k, v)| if k == key { Some(v) } else { None })
    }

    /// Return true if `key` exists.
    pub fn contains_key(&self, key: &K) -> bool
    where
        K: PartialEq,
    {
        self.get(key).is_some()
    }

    /// Return the underlying slice of entries.
    pub fn as_slice(&self) -> &'static [(K, V)] {
        self.data
    }
}

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for FrozenDict<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FrozenDict").field(&self.data).finish()
    }
}

impl<K: fmt::Display, V: fmt::Display> fmt::Display for FrozenDict<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("{")?;
        for (i, (k, v)) in self.data.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{}: {}", k, v)?;
        }
        f.write_str("}")
    }
}
