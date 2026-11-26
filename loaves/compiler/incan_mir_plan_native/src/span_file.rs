//! The plan's shared source-file handle, kept shared across the serialized rustc handoff.
//!
//! A plan span names its file through [`SpanFile`], so copying a span copies a pointer, never the path text. The
//! driver hands a finished plan to a fresh rustc process as serde data. serde serializes `Arc` only behind a feature
//! the registry's serde unit does not carry, and decoding through that feature would give every span its own
//! allocation. This handle serializes as its path text and decodes each distinct path once, so a decoded plan shares
//! one allocation per file exactly as the lowered plan does. Incan cannot implement serde's generic traits, so the
//! handle lives in this facet.

use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fmt;
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One source file's path text behind a shared allocation.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpanFile(Arc<str>);

thread_local! {
    /// The paths this thread decoded, so each decoded file keeps one allocation for the life of the thread.
    static DECODED: RefCell<HashSet<Arc<str>>> = RefCell::new(HashSet::new());
}

impl SpanFile {
    /// Own `path` behind a new shared allocation.
    pub fn new(path: String) -> Self {
        Self(Arc::from(path))
    }

    /// The path text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether both handles share one allocation.
    pub fn shares(&self, other: &SpanFile) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// The shared handle for a decoded path: the one this thread already decoded, or a new one it remembers.
    fn decoded(path: &str) -> Self {
        DECODED.with(|decoded| {
            let mut decoded = decoded.borrow_mut();
            if let Some(shared) = decoded.get(path) {
                return Self(Arc::clone(shared));
            }
            let shared: Arc<str> = Arc::from(path);
            decoded.insert(Arc::clone(&shared));
            Self(shared)
        })
    }
}

impl Deref for SpanFile {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for SpanFile {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<OsStr> for SpanFile {
    fn as_ref(&self) -> &OsStr {
        OsStr::new(&*self.0)
    }
}

impl AsRef<Path> for SpanFile {
    fn as_ref(&self) -> &Path {
        Path::new(&*self.0)
    }
}

impl fmt::Display for SpanFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for SpanFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.0, formatter)
    }
}

impl Serialize for SpanFile {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SpanFile {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(PathVisitor)
    }
}

/// Decodes a path text into its shared handle without an intermediate owned string.
struct PathVisitor;

impl Visitor<'_> for PathVisitor {
    type Value = SpanFile;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a source file path")
    }

    fn visit_str<E: de::Error>(self, path: &str) -> Result<SpanFile, E> {
        Ok(SpanFile::decoded(path))
    }
}

#[cfg(test)]
mod tests {
    use super::SpanFile;

    #[test]
    fn decoded_spans_share_one_allocation_per_file() -> Result<(), serde_json::Error> {
        let first = SpanFile::new("/source/main.incn".to_owned());
        let encoded = serde_json::to_string(&[first.clone(), first.clone()])?;
        let decoded: Vec<SpanFile> = serde_json::from_str(&encoded)?;
        assert_eq!(decoded[0].as_str(), "/source/main.incn");
        assert!(decoded[0].shares(&decoded[1]), "decoding gave one file two allocations");
        assert!(!decoded[0].shares(&first), "decoding reused the encoder's allocation");
        Ok(())
    }
}
