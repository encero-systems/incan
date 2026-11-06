//! Provide frozen (deeply immutable) runtime types for RFC 008.
//!
//! These types reference baked `'static` data or computed constant storage kept for the lifetime of the program.
//!
//! ## Notes
//! - All frozen collection wrappers are backed by `'static` slices.
//! - APIs are intentionally read-only to enforce deep immutability at the type level.
//!
//! ## Examples
//! ```rust
//! use incan_std_core::prelude::*;
//!
//! static NUMS: [i64; 3] = [1, 2, 3];
//! const L: FrozenList<i64> = FrozenList::new(&NUMS);
//! assert_eq!(L.len(), 3);
//! ```

use core::borrow::Borrow;
use core::fmt;

pub use incan_std_primitives::frozen::{FrozenBytes, FrozenStr};

/// Represent an immutable list backed by a baked `'static` slice.
///
/// ## Notes
/// - Backed by `&'static [T]`, so `T` must be `'static`.
/// - This is a *read-only* wrapper (no `push`, `pop`, etc.).
/// - Copying or cloning the wrapper shares its immutable storage without requiring either trait on its elements.
///
/// ## Examples
/// ```rust
/// use incan_std_core::prelude::*;
///
/// static DATA: [i64; 3] = [10, 20, 30];
/// const L: FrozenList<i64> = FrozenList::new(&DATA);
/// assert_eq!(L.get(1), Some(&20));
/// ```
#[derive(PartialEq, Eq)]
pub struct FrozenList<T: 'static> {
    data: &'static [T],
}

// Copying the immutable slice handle never copies or clones an element.
// Deriving these traits would require T: Copy or T: Clone, rejecting a frozen model list read from a descriptor field.
impl<T: 'static> Copy for FrozenList<T> {}

impl<T: 'static> Clone for FrozenList<T> {
    /// Share the same immutable program-lifetime storage without cloning its elements.
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: 'static> FrozenList<T> {
    /// Keep values computed once (a computed constant's) as the frozen list's storage for the rest of the program.
    pub fn leak(values: Vec<T>) -> Self {
        Self::new(Box::leak(values.into_boxed_slice()))
    }

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
    /// Format the items as a list, like the `list` a frozen list was baked from.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.data).finish()
    }
}

impl<T: fmt::Debug> fmt::Display for FrozenList<T> {
    /// Print a frozen list as its `Debug` form, the printed form a `list` has.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
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
    use super::{FrozenDict, FrozenList, FrozenSet, FrozenStr};
    use crate::collections::__private::{list_max_copy, list_min_copy};

    static NUMS: [i64; 3] = [3, 1, 4];
    static NAMES: [FrozenStr; 2] = [FrozenStr::new("a"), FrozenStr::new("b")];
    static FLOATS: [f64; 1] = [100.0];
    static ENTRIES: [(FrozenStr, f64); 1] = [(FrozenStr::new("n"), 100.0)];

    #[test]
    fn frozen_list_coerces_to_slice_for_runtime_helpers() {
        let numbers = FrozenList::new(&NUMS);

        assert_eq!(numbers.as_ref(), &[3, 1, 4]);
        assert_eq!(list_min_copy(&numbers), 1);
        assert_eq!(list_max_copy(&numbers), 4);
    }

    /// A text-keyed frozen dict answers a `str` probe of any lifetime, whether its keys are `&'static str` or
    /// `FrozenStr`, and a stored-key probe still works (#1757).
    #[test]
    fn frozen_dict_lookups_accept_borrowed_text_probes() {
        const NAMES: FrozenDict<&'static str, i64> = FrozenDict::new(&[("alpha", 1), ("beta", 2)]);
        const CODES: FrozenDict<FrozenStr, i64> = FrozenDict::new(&[(FrozenStr::new("a"), 10)]);
        let runtime_probe = String::from("beta");

        assert_eq!(NAMES.get(runtime_probe.as_str()), Some(&2));
        assert!(NAMES.contains_key("alpha"));
        assert!(!NAMES.contains_key("gamma"));
        assert!(NAMES.contains_key(&"alpha"));
        assert_eq!(CODES.get("a"), Some(&10));
        assert!(CODES.contains_key(&FrozenStr::new("a")));
    }

    #[test]
    fn frozen_values_match_mutable_display_shapes_issue1838() {
        assert_eq!(format!("{:?}", FrozenStr::new("x")), "\"x\"");
        assert_eq!(format!("{:?}", Some(FrozenStr::new("x"))), "Some(\"x\")");
        assert_eq!(format!("{}", FrozenList::new(&NAMES)), "[\"a\", \"b\"]");
        assert_eq!(format!("{}", FrozenList::new(&FLOATS)), "[100.0]");
        assert_eq!(format!("{}", FrozenSet::new(&NAMES)), "{\"a\", \"b\"}");
        assert_eq!(format!("{}", FrozenDict::new(&ENTRIES)), "{\"n\": 100.0}");
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
/// use incan_std_core::prelude::*;
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
    /// Format the items as a set, like the `set` a frozen set was baked from.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.data).finish()
    }
}

impl<T: fmt::Debug> fmt::Display for FrozenSet<T> {
    /// Print a frozen set as its `Debug` form, the printed form a `set` has.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
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
/// use incan_std_core::prelude::*;
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
    ///
    /// Like `HashMap::get`, the probe may be any form the stored key type borrows as, so a `&'static str`- or
    /// `FrozenStr`-keyed dict answers a borrowed `str` probe that is not itself `'static`.
    pub fn get<Q>(&self, key: &Q) -> Option<&'static V>
    where
        K: Borrow<Q>,
        Q: PartialEq + ?Sized,
    {
        self.data
            .iter()
            .find_map(|(k, v)| if k.borrow() == key { Some(v) } else { None })
    }

    /// Return true if `key` exists; the probe follows the same borrowing rule as [`Self::get`].
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: PartialEq + ?Sized,
    {
        self.get(key).is_some()
    }

    /// Return the underlying slice of entries.
    pub fn as_slice(&self) -> &'static [(K, V)] {
        self.data
    }
}

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for FrozenDict<K, V> {
    /// Format the entries as a map, like the `dict` a frozen dict was baked from.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.data.iter().map(|(key, value)| (key, value)))
            .finish()
    }
}

impl<K: fmt::Debug, V: fmt::Debug> fmt::Display for FrozenDict<K, V> {
    /// Print a frozen dict as its `Debug` form, the printed form a `dict` has.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}
