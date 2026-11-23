//! Provide frozen (deeply immutable) runtime types for RFC 008.
//!
//! These types reference baked `'static` data or atomically shared runtime storage released with its final handle.
//! Rust remains necessary because rustc MIR constants require concrete Rust carriers over program-lifetime storage.
//!
//! ## Notes
//! - Constants retain `'static` slices and runtime values retain `Arc`-backed collections.
//! - Cloning a wrapper is constant-time and never clones its elements.
//! - APIs are intentionally read-only to enforce deep immutability at the type level.
//!
//! ## Examples
//! ```rust
//! use incan_std_primitives::frozen::FrozenList;
//!
//! static NUMS: [i64; 3] = [1, 2, 3];
//! const L: FrozenList<i64> = FrozenList::new(&NUMS);
//! assert_eq!(L.len(), 3);
//! ```

use core::borrow::Borrow;
use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Represent immutable text with static or atomically shared storage.
///
/// Module constants borrow program-lifetime text; values produced at run time share owned text until their last clone.
#[repr(C, u8)]
pub enum FrozenStr {
    /// Program-lifetime text used by a source constant.
    Static(&'static str),
    /// Atomically shared text produced at run time.
    Shared(Arc<str>),
}

impl FrozenStr {
    /// Construct a frozen string from a program-lifetime string slice.
    pub const fn new(value: &'static str) -> Self {
        Self::Static(value)
    }

    /// Freeze runtime-owned text in shared storage released with the final handle.
    pub fn from_owned(value: String) -> Self {
        Self::Shared(Arc::from(value))
    }

    /// Return the underlying string slice.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Static(value) => value,
            Self::Shared(value) => value,
        }
    }

    /// Return the string length in bytes.
    pub fn len(&self) -> usize {
        self.as_str().len()
    }

    /// Return whether the string is empty.
    pub fn is_empty(&self) -> bool {
        self.as_str().is_empty()
    }
}

impl Clone for FrozenStr {
    fn clone(&self) -> Self {
        match self {
            Self::Static(value) => Self::new(value),
            Self::Shared(value) => Self::Shared(Arc::clone(value)),
        }
    }
}

impl PartialEq for FrozenStr {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for FrozenStr {}

impl Hash for FrozenStr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl PartialOrd for FrozenStr {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FrozenStr {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl fmt::Debug for FrozenStr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), formatter)
    }
}

impl fmt::Display for FrozenStr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl AsRef<str> for FrozenStr {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Let a frozen-text key answer a lookup through a borrowed text probe.
impl Borrow<str> for FrozenStr {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

/// Represent immutable bytes with static or atomically shared storage.
///
/// Module constants borrow program-lifetime bytes; values produced at run time share owned bytes until their last
/// clone.
#[repr(C, u8)]
pub enum FrozenBytes {
    /// Program-lifetime bytes used by a source constant.
    Static(&'static [u8]),
    /// Atomically shared bytes produced at run time.
    Shared(Arc<[u8]>),
}

impl FrozenBytes {
    /// Construct frozen bytes from a program-lifetime byte slice.
    pub const fn new(bytes: &'static [u8]) -> Self {
        Self::Static(bytes)
    }

    /// Freeze runtime-owned bytes in shared storage released with the final handle.
    pub fn from_owned(bytes: Vec<u8>) -> Self {
        Self::Shared(Arc::from(bytes))
    }

    /// Return the underlying byte slice.
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Static(bytes) => bytes,
            Self::Shared(bytes) => bytes,
        }
    }

    /// Return the number of bytes.
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// Return whether the byte slice is empty.
    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }

    /// Iterate over the bytes.
    pub fn iter(&self) -> core::slice::Iter<'_, u8> {
        self.as_slice().iter()
    }
}

impl Clone for FrozenBytes {
    fn clone(&self) -> Self {
        match self {
            Self::Static(bytes) => Self::new(bytes),
            Self::Shared(bytes) => Self::Shared(Arc::clone(bytes)),
        }
    }
}

impl PartialEq for FrozenBytes {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Eq for FrozenBytes {}

impl Hash for FrozenBytes {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

impl PartialOrd for FrozenBytes {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FrozenBytes {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

impl fmt::Debug for FrozenBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("FrozenBytes").field(&self.as_slice()).finish()
    }
}

impl fmt::Display for FrozenBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("b\"")?;
        for byte in self.as_slice() {
            if *byte == b'\\' || *byte == b'"' {
                write!(formatter, "\\{}", *byte as char)?;
            } else if byte.is_ascii_graphic() || *byte == b' ' {
                write!(formatter, "{}", *byte as char)?;
            } else {
                write!(formatter, "\\x{:02x}", byte)?;
            }
        }
        formatter.write_str("\"")
    }
}

impl AsRef<[u8]> for FrozenBytes {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl core::ops::Index<usize> for FrozenBytes {
    type Output = u8;

    fn index(&self, index: usize) -> &Self::Output {
        &self.as_slice()[index]
    }
}

/// Represent an immutable list backed by a baked slice or atomically shared runtime storage.
///
/// ## Notes
/// - Constants use `&'static [T]`; runtime values use `Arc<[T]>`, so `T` must be `'static`.
/// - This is a *read-only* wrapper (no `push`, `pop`, etc.).
/// - Copying or cloning the wrapper shares its immutable storage without requiring either trait on its elements.
///
/// ## Examples
/// ```rust
/// use incan_std_primitives::frozen::FrozenList;
///
/// static DATA: [i64; 3] = [10, 20, 30];
/// const L: FrozenList<i64> = FrozenList::new(&DATA);
/// assert_eq!(L.get(1), Some(&20));
/// ```
#[repr(C, u8)]
pub enum FrozenList<T: 'static> {
    /// Program-lifetime elements used by a source constant.
    Static(&'static [T]),
    /// Atomically shared elements produced at run time.
    Shared(Arc<[T]>),
}

impl<T: 'static> Clone for FrozenList<T> {
    /// Share the same immutable static or runtime storage without cloning its elements.
    fn clone(&self) -> Self {
        match self {
            Self::Static(data) => Self::new(data),
            Self::Shared(data) => Self::Shared(Arc::clone(data)),
        }
    }
}

impl<T: PartialEq> PartialEq for FrozenList<T> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: Eq> Eq for FrozenList<T> {}

impl<T: Hash> Hash for FrozenList<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

impl<T: 'static> FrozenList<T> {
    /// Keep values computed once (a computed constant's) as the frozen list's storage for the rest of the program.
    pub fn leak(values: Vec<T>) -> Self {
        Self::new(Box::leak(values.into_boxed_slice()))
    }

    /// Freeze runtime-owned values in shared storage released with the final handle.
    pub fn from_owned(values: Vec<T>) -> Self {
        Self::Shared(Arc::from(values))
    }

    /// Construct a frozen list from a baked `'static` slice.
    pub const fn new(data: &'static [T]) -> Self {
        Self::Static(data)
    }

    /// Return the number of elements.
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// Return true if the list is empty.
    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }

    /// Iterate over elements.
    pub fn iter(&self) -> core::slice::Iter<'_, T> {
        self.as_slice().iter()
    }

    /// Get an element by index.
    pub fn get(&self, idx: usize) -> Option<&T> {
        self.as_slice().get(idx)
    }

    /// Return the underlying slice.
    pub fn as_slice(&self) -> &[T] {
        match self {
            Self::Static(data) => data,
            Self::Shared(data) => data,
        }
    }

    /// Return whether this list contains a borrowed probe without copying an element.
    pub fn contains<Q>(&self, probe: &Q) -> bool
    where
        T: Borrow<Q>,
        Q: PartialEq + ?Sized,
    {
        self.iter().any(|value| value.borrow() == probe)
    }
}

impl<T: 'static> AsRef<[T]> for FrozenList<T> {
    /// Return a borrowed view of this value.
    fn as_ref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T: 'static> core::ops::Deref for FrozenList<T> {
    type Target = [T];

    /// Return the underlying target for deref coercions.
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<T: fmt::Debug> fmt::Debug for FrozenList<T> {
    /// Format the items as a list, like the `list` a frozen list was baked from.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.as_slice()).finish()
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
        &self.as_slice()[idx]
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
        self.as_slice().iter()
    }
}

#[cfg(test)]
mod tests {
    use super::{FrozenDict, FrozenList, FrozenSet, FrozenStr};
    use crate::collections::__private::{list_max_copy, list_min_copy};
    use std::collections::{HashMap, HashSet};
    use std::hash::{DefaultHasher, Hash, Hasher};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    static NUMS: [i64; 3] = [3, 1, 4];
    static NAMES: [FrozenStr; 2] = [FrozenStr::new("a"), FrozenStr::new("b")];
    static FLOATS: [f64; 1] = [100.0];
    static ENTRIES: [(FrozenStr, f64); 1] = [(FrozenStr::new("n"), 100.0)];
    static CODE_ENTRIES: [(FrozenStr, i64); 1] = [(FrozenStr::new("a"), 10)];

    #[derive(Debug)]
    struct DropProbe(Arc<AtomicUsize>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[derive(Debug)]
    struct LookupProbe {
        value: usize,
        comparisons: Arc<AtomicUsize>,
        clones: Arc<AtomicUsize>,
    }

    impl Clone for LookupProbe {
        fn clone(&self) -> Self {
            self.clones.fetch_add(1, Ordering::SeqCst);
            Self {
                value: self.value,
                comparisons: Arc::clone(&self.comparisons),
                clones: Arc::clone(&self.clones),
            }
        }
    }

    impl PartialEq for LookupProbe {
        fn eq(&self, other: &Self) -> bool {
            self.comparisons.fetch_add(1, Ordering::SeqCst);
            self.value == other.value
        }
    }

    impl Eq for LookupProbe {}

    impl Hash for LookupProbe {
        fn hash<H: Hasher>(&self, state: &mut H) {
            self.value.hash(state);
        }
    }

    fn content_hash(value: &impl Hash) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

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
        const CODES: FrozenDict<FrozenStr, i64> = FrozenDict::new(&CODE_ENTRIES);
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

    #[test]
    fn runtime_frozen_storage_clones_handles_and_releases_on_the_last_drop() {
        let drops = Arc::new(AtomicUsize::new(0));
        let values = FrozenList::from_owned(vec![DropProbe(Arc::clone(&drops))]);
        let retained = values.clone();

        drop(values);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(retained);
        assert_eq!(drops.load(Ordering::SeqCst), 1);

        let static_text = FrozenStr::new("same");
        let shared_text = FrozenStr::from_owned(String::from("same"));
        assert_eq!(static_text, shared_text);
        assert_eq!(content_hash(&static_text), content_hash(&shared_text));

        let static_list = FrozenList::new(&NUMS);
        let shared_list = FrozenList::from_owned(vec![3, 1, 4]);
        assert_eq!(static_list, shared_list);
        assert_eq!(content_hash(&static_list), content_hash(&shared_list));
    }

    #[test]
    fn runtime_frozen_set_membership_uses_shared_hash_storage_without_copies_issue2944() {
        let comparisons = Arc::new(AtomicUsize::new(0));
        let clones = Arc::new(AtomicUsize::new(0));
        let values = (0..9)
            .map(|value| LookupProbe {
                value,
                comparisons: Arc::clone(&comparisons),
                clones: Arc::clone(&clones),
            })
            .collect::<HashSet<_>>();
        let frozen = FrozenSet::from_owned(values);
        comparisons.store(0, Ordering::SeqCst);
        clones.store(0, Ordering::SeqCst);
        let probe = LookupProbe {
            value: 8,
            comparisons: Arc::clone(&comparisons),
            clones: Arc::clone(&clones),
        };

        assert!(frozen.contains(&probe));
        assert_eq!(clones.load(Ordering::SeqCst), 0);
        assert!(comparisons.load(Ordering::SeqCst) < 9);

        let dictionary = FrozenDict::from_owned(HashMap::from([(8_usize, "found")]));
        assert_eq!(dictionary.get(&8), Some(&"found"));
    }
}

/// Represent an immutable set backed by a baked slice or an atomically shared hash set.
///
/// ## Notes
/// - Baked constants use a linear borrowed scan.
/// - Runtime values use hashed lookup without copying their elements.
///
/// ## Examples
/// ```rust
/// use incan_std_primitives::frozen::FrozenSet;
///
/// static DATA: [i64; 2] = [1, 3];
/// const S: FrozenSet<i64> = FrozenSet::new(&DATA);
/// assert!(S.contains(&3));
/// ```
#[repr(C, u8)]
pub enum FrozenSet<T: 'static> {
    /// Program-lifetime elements used by a source constant.
    Static(&'static [T]),
    /// Atomically shared hashed elements produced at run time.
    Shared(Arc<HashSet<T>>),
}

/// Borrowed iteration over either baked or runtime-owned frozen set storage.
pub enum FrozenSetIter<'a, T> {
    Static(core::slice::Iter<'a, T>),
    Shared(std::collections::hash_set::Iter<'a, T>),
}

impl<'a, T> Iterator for FrozenSetIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Static(iter) => iter.next(),
            Self::Shared(iter) => iter.next(),
        }
    }
}

impl<T: 'static> Clone for FrozenSet<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Static(data) => Self::new(data),
            Self::Shared(data) => Self::Shared(Arc::clone(data)),
        }
    }
}

impl<T: PartialEq> PartialEq for FrozenSet<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .all(|value| other.iter().any(|candidate| candidate == value))
    }
}

impl<T: Eq> Eq for FrozenSet<T> {}

impl<T: Hash> Hash for FrozenSet<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let mut hashes = self
            .iter()
            .map(|value| {
                let mut item = std::collections::hash_map::DefaultHasher::new();
                value.hash(&mut item);
                item.finish()
            })
            .collect::<Vec<_>>();
        hashes.sort_unstable();
        hashes.hash(state);
    }
}

impl<T: 'static> FrozenSet<T> {
    /// Construct a frozen set from a baked `'static` slice.
    pub const fn new(data: &'static [T]) -> Self {
        Self::Static(data)
    }

    /// Freeze runtime-owned set storage behind a shared handle.
    pub fn from_owned(data: HashSet<T>) -> Self {
        Self::Shared(Arc::new(data))
    }

    /// Return the number of elements.
    pub fn len(&self) -> usize {
        match self {
            Self::Static(data) => data.len(),
            Self::Shared(data) => data.len(),
        }
    }

    /// Return true if the set is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Iterate over elements.
    pub fn iter(&self) -> FrozenSetIter<'_, T> {
        match self {
            Self::Static(data) => FrozenSetIter::Static(data.iter()),
            Self::Shared(data) => FrozenSetIter::Shared(data.iter()),
        }
    }

    /// Return true if the set contains `item`, using hashed lookup for shared runtime storage.
    pub fn contains<Q>(&self, item: &Q) -> bool
    where
        T: Borrow<Q> + Eq + Hash,
        Q: Eq + Hash + ?Sized,
    {
        match self {
            Self::Static(data) => data.iter().any(|value| value.borrow() == item),
            Self::Shared(data) => data.contains(item),
        }
    }

    /// Borrow the element at one iteration position without materializing the set.
    pub fn get(&self, index: usize) -> Option<&T> {
        self.iter().nth(index)
    }
}

impl<T: fmt::Debug> fmt::Debug for FrozenSet<T> {
    /// Format the items as a set, like the `set` a frozen set was baked from.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl<T: fmt::Debug> fmt::Display for FrozenSet<T> {
    /// Print a frozen set as its `Debug` form, the printed form a `set` has.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// Represent an immutable dictionary backed by baked pairs or an atomically shared hash map.
///
/// ## Notes
/// - Baked constants use a linear borrowed scan.
/// - Runtime values use hashed lookup without copying their entries.
///
/// ## Examples
/// ```rust
/// use incan_std_primitives::frozen::{FrozenDict, FrozenStr};
///
/// static DATA: [(FrozenStr, i64); 2] = [(FrozenStr::new("a"), 1), (FrozenStr::new("b"), 2)];
/// const D: FrozenDict<FrozenStr, i64> = FrozenDict::new(&DATA);
/// assert_eq!(D.get(&FrozenStr::new("b")), Some(&2));
/// ```
#[repr(C, u8)]
pub enum FrozenDict<K: 'static, V: 'static> {
    /// Program-lifetime entries used by a source constant.
    Static(&'static [(K, V)]),
    /// Atomically shared hashed entries produced at run time.
    Shared(Arc<HashMap<K, V>>),
}

/// Borrowed iteration over either baked or runtime-owned frozen dictionary storage.
pub enum FrozenDictIter<'a, K, V> {
    Static(core::slice::Iter<'a, (K, V)>),
    Shared(std::collections::hash_map::Iter<'a, K, V>),
}

impl<'a, K, V> Iterator for FrozenDictIter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Static(iter) => iter.next().map(|(key, value)| (key, value)),
            Self::Shared(iter) => iter.next(),
        }
    }
}

impl<K: 'static, V: 'static> Clone for FrozenDict<K, V> {
    fn clone(&self) -> Self {
        match self {
            Self::Static(data) => Self::new(data),
            Self::Shared(data) => Self::Shared(Arc::clone(data)),
        }
    }
}

impl<K: PartialEq, V: PartialEq> PartialEq for FrozenDict<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self.iter().all(|(key, value)| {
                other
                    .iter()
                    .any(|(other_key, other_value)| other_key == key && other_value == value)
            })
    }
}

impl<K: Eq, V: Eq> Eq for FrozenDict<K, V> {}

impl<K: Hash, V: Hash> Hash for FrozenDict<K, V> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let mut hashes = self
            .iter()
            .map(|(key, value)| {
                let mut entry = std::collections::hash_map::DefaultHasher::new();
                key.hash(&mut entry);
                value.hash(&mut entry);
                entry.finish()
            })
            .collect::<Vec<_>>();
        hashes.sort_unstable();
        hashes.hash(state);
    }
}

impl<K: 'static, V: 'static> FrozenDict<K, V> {
    /// Construct a frozen dict from baked key-value pairs.
    pub const fn new(data: &'static [(K, V)]) -> Self {
        Self::Static(data)
    }

    /// Freeze runtime-owned dictionary storage behind a shared handle.
    pub fn from_owned(data: HashMap<K, V>) -> Self {
        Self::Shared(Arc::new(data))
    }

    /// Return the number of entries.
    pub fn len(&self) -> usize {
        match self {
            Self::Static(data) => data.len(),
            Self::Shared(data) => data.len(),
        }
    }

    /// Return true if the dict is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Iterate over entries.
    pub fn iter(&self) -> FrozenDictIter<'_, K, V> {
        match self {
            Self::Static(data) => FrozenDictIter::Static(data.iter()),
            Self::Shared(data) => FrozenDictIter::Shared(data.iter()),
        }
    }

    /// Return the value for `key`, using hashed lookup for shared runtime storage.
    ///
    /// Like `HashMap::get`, the probe may be any form the stored key type borrows as, so a `&'static str`- or
    /// `FrozenStr`-keyed dict answers a borrowed `str` probe that is not itself `'static`.
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q> + Eq + Hash,
        Q: Eq + Hash + ?Sized,
    {
        match self {
            Self::Static(data) => data
                .iter()
                .find_map(|(stored, value)| (stored.borrow() == key).then_some(value)),
            Self::Shared(data) => data.get(key),
        }
    }

    /// Return true if `key` exists; the probe follows the same borrowing rule as [`Self::get`].
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q> + Eq + Hash,
        Q: Eq + Hash + ?Sized,
    {
        self.get(key).is_some()
    }

    /// Borrow the key at one iteration position without materializing the dictionary.
    pub fn key_at(&self, index: usize) -> Option<&K> {
        self.iter().nth(index).map(|(key, _value)| key)
    }

    /// Borrow the value at one iteration position without materializing the dictionary.
    pub fn value_at(&self, index: usize) -> Option<&V> {
        self.iter().nth(index).map(|(_key, value)| value)
    }
}

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for FrozenDict<K, V> {
    /// Format the entries as a map, like the `dict` a frozen dict was baked from.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K: fmt::Debug, V: fmt::Debug> fmt::Display for FrozenDict<K, V> {
    /// Print a frozen dict as its `Debug` form, the printed form a `dict` has.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}
