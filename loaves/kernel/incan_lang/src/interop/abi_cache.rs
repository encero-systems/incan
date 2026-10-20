//! The Rust ABI cache: the facts of compiled Rust crates (every public item's [`RustItemMetadata`], keyed by path),
//! stored the way `.incnbody` stores executable bodies: a version, a postcard index of each path's payload range, then
//! each item's postcard payload. A reader decodes the index once and an item only when a compilation asks for it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::metadata::RustItemMetadata;

/// The cache layout's version; a reader refuses any other.
pub const RUST_ABI_CACHE_VERSION: u32 = 1;

/// One item's payload range within the cache.
#[derive(Serialize, Deserialize)]
struct IndexEntry {
    path: String,
    offset: u64,
    length: u64,
}

/// Encode `items` as one ABI cache.
pub fn encode_rust_abi_cache(items: &BTreeMap<String, RustItemMetadata>) -> Result<Vec<u8>, String> {
    let mut payload = Vec::new();
    let mut index = Vec::with_capacity(items.len());
    for (path, item) in items {
        let bytes = postcard::to_allocvec(item).map_err(|error| format!("cannot encode {path}: {error}"))?;
        index.push(IndexEntry {
            path: path.clone(),
            offset: payload.len() as u64,
            length: bytes.len() as u64,
        });
        payload.extend_from_slice(&bytes);
    }
    let index = postcard::to_allocvec(&index).map_err(|error| format!("cannot encode the index: {error}"))?;
    let mut bytes = RUST_ABI_CACHE_VERSION.to_le_bytes().to_vec();
    bytes.extend_from_slice(&(index.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&index);
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

/// A decoded ABI cache index over its payload; items decode on demand.
#[derive(Debug)]
pub struct RustAbiCache {
    index: BTreeMap<String, (usize, usize)>,
    payload: Vec<u8>,
}

impl RustAbiCache {
    /// Read a cache's version and index, keeping its payload for on-demand decoding.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let malformed = || "malformed Rust ABI cache".to_owned();
        let version = u32::from_le_bytes(bytes.get(0..4).ok_or_else(malformed)?.try_into().map_err(|_| malformed())?);
        if version != RUST_ABI_CACHE_VERSION {
            return Err(format!(
                "Rust ABI cache version {version}; this compiler reads version {RUST_ABI_CACHE_VERSION}"
            ));
        }
        let length = u64::from_le_bytes(bytes.get(4..12).ok_or_else(malformed)?.try_into().map_err(|_| malformed())?);
        let end = 12usize.checked_add(usize::try_from(length).map_err(|_| malformed())?).ok_or_else(malformed)?;
        let entries: Vec<IndexEntry> =
            postcard::from_bytes(bytes.get(12..end).ok_or_else(malformed)?).map_err(|error| error.to_string())?;
        let payload = bytes.get(end..).ok_or_else(malformed)?.to_vec();
        let mut index = BTreeMap::new();
        for entry in entries {
            let offset = usize::try_from(entry.offset).map_err(|_| malformed())?;
            let length = usize::try_from(entry.length).map_err(|_| malformed())?;
            if offset.checked_add(length).is_none_or(|stop| stop > payload.len()) {
                return Err(malformed());
            }
            index.insert(entry.path, (offset, length));
        }
        Ok(Self { index, payload })
    }

    /// The facts of one path, decoded from its payload; `None` when the cache does not hold it.
    pub fn get(&self, path: &str) -> Option<RustItemMetadata> {
        let (offset, length) = *self.index.get(path)?;
        postcard::from_bytes(&self.payload[offset..offset + length]).ok()
    }

    /// Whether the cache holds `path`.
    pub fn contains(&self, path: &str) -> bool {
        self.index.contains_key(path)
    }

    /// How many items the cache holds.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Whether the cache holds no item.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}
