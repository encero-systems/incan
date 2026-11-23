//! The Rust ABI cache: the facts of compiled Rust crates (every public item's [`RustItemMetadata`], keyed by path).
//!
//! A cache is a version, then three Snappy-compressed sections: an index naming each path's block and range, the string
//! table the records share, and the directory of record blocks. The records follow as Snappy-compressed blocks of about
//! [`BLOCK_TARGET`] bytes. Each record prefixes its full item with the item's small type-alias projection; both
//! postcard values store strings as table indexes. A reader decodes the sections once, and a record's block only when a
//! compilation asks for one of its items. Crate facts repeat a few hundred thousand distinct strings and many identical
//! signatures across items, so the table and the blocks keep a cache a small fraction of the plain encoding.

mod strings;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use incan_lang::interop::{RustItemKind, RustItemMetadata};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// The cache layout's version; a reader refuses any other. Version 10 stores each distinct string once and compresses
/// the sections and record blocks with Snappy. Version 11 prefixes each item with its small type-alias projection so
/// speculative alias checks do not restore and retain the complete item.
pub const RUST_ABI_CACHE_VERSION: u32 = 11;

/// A manifest-authenticated shipped cache, either current facts or the validated path inventory of version 10.
pub(crate) enum ShippedRustAbiCache {
    /// Current records can answer metadata queries directly.
    Current(RustAbiCache),
    /// Version-10 records are not decoded; their unchanged index keys are re-extracted by the candidate compiler.
    Previous(BTreeSet<String>),
}

/// The raw size a record block grows to before the next record starts a new one; a larger record is its own block.
const BLOCK_TARGET: usize = 64 << 10;

/// How many decompressed blocks a reader keeps for nearby lookups.
const RETAINED_BLOCKS: usize = 8;

/// The version, then the stored and raw length of each of the three sections.
const HEADER_LENGTH: usize = 4 + 3 * 16;

/// One item's record: its block and its range within the decompressed block.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Location {
    block: u32,
    offset: u32,
    length: u32,
}

/// One block's stored range after the sections and its decompressed length.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Block {
    offset: u64,
    length: u64,
    raw_length: u64,
}

/// The alias-only facts a type display needs, without a type item's methods, implementations, fields or variants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RustTypeAlias {
    /// The alias's declared type parameters in substitution order.
    pub(crate) type_params: Vec<String>,
    /// The native target display before substituting a use site's arguments.
    pub(crate) target: String,
}

/// Whether one addressed Rust item is a type alias, retaining its small projection only when it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RustTypeAliasLookup {
    /// The path exists but does not name a type alias.
    NotAlias,
    /// The path names a type alias with the projected substitution facts.
    Alias(RustTypeAlias),
}

impl RustTypeAliasLookup {
    /// Project alias facts from metadata that is already owned for another reader.
    pub(crate) fn from_item(item: &RustItemMetadata) -> Self {
        let RustItemKind::Type(info) = &item.kind else {
            return Self::NotAlias;
        };
        let Some(target) = &info.alias_target else {
            return Self::NotAlias;
        };
        Self::Alias(RustTypeAlias {
            type_params: info.type_params.clone(),
            target: target.clone(),
        })
    }

    /// Return the projected alias facts, or `None` for another item shape.
    pub(crate) fn into_alias(self) -> Option<RustTypeAlias> {
        match self {
            Self::NotAlias => None,
            Self::Alias(alias) => Some(alias),
        }
    }
}

/// Encode `items` as one ABI cache.
pub fn encode_rust_abi_cache(items: &BTreeMap<String, RustItemMetadata>) -> Result<Vec<u8>, String> {
    let table = RefCell::new(strings::StringTable::default());
    let mut index = Vec::with_capacity(items.len());
    let mut raw_blocks: Vec<Vec<u8>> = Vec::new();
    for (path, item) in items {
        let alias = RustTypeAliasLookup::from_item(item).into_alias();
        let alias = strings::encode(&alias, &table).map_err(|error| format!("cannot encode {path}: {error}"))?;
        let item = strings::encode(item, &table).map_err(|error| format!("cannot encode {path}: {error}"))?;
        let alias_length = u32::try_from(alias.len())
            .map_err(|_| "a Rust ABI cache alias projection exceeds its 32-bit range".to_owned())?;
        let mut record = Vec::with_capacity(4 + alias.len() + item.len());
        record.extend_from_slice(&alias_length.to_le_bytes());
        record.extend_from_slice(&alias);
        record.extend_from_slice(&item);
        if raw_blocks
            .last()
            .is_none_or(|block| !block.is_empty() && block.len() + record.len() > BLOCK_TARGET)
        {
            raw_blocks.push(Vec::new());
        }
        let block = raw_blocks.len() - 1;
        let current = &mut raw_blocks[block];
        index.push((
            path.clone(),
            Location {
                block: narrow(block)?,
                offset: narrow(current.len())?,
                length: narrow(record.len())?,
            },
        ));
        current.extend_from_slice(&record);
    }
    let mut payload = Vec::new();
    let mut directory = Vec::with_capacity(raw_blocks.len());
    for raw in &raw_blocks {
        let packed = pack(raw)?;
        directory.push(Block {
            offset: payload.len() as u64,
            length: packed.len() as u64,
            raw_length: raw.len() as u64,
        });
        payload.extend_from_slice(&packed);
    }
    let sections = [
        postcard::to_allocvec(&index).map_err(|error| format!("cannot encode the index: {error}"))?,
        postcard::to_allocvec(&table.into_inner().into_strings())
            .map_err(|error| format!("cannot encode the string table: {error}"))?,
        postcard::to_allocvec(&directory).map_err(|error| format!("cannot encode the block directory: {error}"))?,
    ];
    let packed: Vec<Vec<u8>> = sections.iter().map(|section| pack(section)).collect::<Result<_, _>>()?;
    let mut bytes = RUST_ABI_CACHE_VERSION.to_le_bytes().to_vec();
    for (section, stored) in sections.iter().zip(&packed) {
        bytes.extend_from_slice(&(stored.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(section.len() as u64).to_le_bytes());
    }
    for stored in &packed {
        bytes.extend_from_slice(stored);
    }
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

/// A position or length that the layout stores in 32 bits.
fn narrow(value: usize) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| "a Rust ABI cache section exceeds its 32-bit range".to_owned())
}

/// Split one version-11 record into its alias prefix and full item payload.
fn split_record(record: &[u8]) -> Result<(&[u8], &[u8]), String> {
    let alias_length = usize::try_from(u32::from_le_bytes(
        record
            .get(..4)
            .ok_or_else(malformed_cache)?
            .try_into()
            .map_err(|_| malformed_cache())?,
    ))
    .map_err(|_| malformed_cache())?;
    let alias_end = 4usize.checked_add(alias_length).ok_or_else(malformed_cache)?;
    let alias = record.get(4..alias_end).ok_or_else(malformed_cache)?;
    let item = record
        .get(alias_end..)
        .filter(|item| !item.is_empty())
        .ok_or_else(malformed_cache)?;
    Ok((alias, item))
}

/// A decoded ABI cache index, string table and block directory over retained bytes or an open file; items decode on
/// demand.
pub struct RustAbiCache {
    index: BTreeMap<String, Location>,
    strings: Vec<String>,
    directory: Vec<Block>,
    payload: Payload,
    blocks: Mutex<Vec<(u32, Arc<Vec<u8>>)>>,
    #[cfg(test)]
    full_item_restorations: AtomicUsize,
}

impl fmt::Debug for RustAbiCache {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RustAbiCache")
            .field("items", &self.index.len())
            .field("strings", &self.strings.len())
            .field("blocks", &self.directory.len())
            .field("payload", &self.payload)
            .finish()
    }
}

/// The decoded sections of one cache, and where its first block starts.
struct Sections {
    index: BTreeMap<String, Location>,
    strings: Vec<String>,
    directory: Vec<Block>,
    payload_start: usize,
}

impl RustAbiCache {
    /// Read borrowed cache bytes, retaining an owned copy for on-demand item decoding.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        Self::decode_owned(bytes.to_vec())
    }

    /// Decode a manifest-authenticated shipped cache, treating an older layout as an absent regenerable cache.
    ///
    /// A release seed necessarily reads native artifacts emitted before its candidate compiler exists. Falling back
    /// from that older cache lets the candidate extract the artifact's Rust facts through its own driver and publish
    /// the current layout. A cache from a newer compiler remains an error because this reader cannot establish that
    /// its semantics are safely reproducible.
    pub(crate) fn decode_shipped(bytes: Vec<u8>) -> Result<ShippedRustAbiCache, String> {
        match cache_version(&bytes)? {
            RUST_ABI_CACHE_VERSION => Self::decode_owned(bytes).map(ShippedRustAbiCache::Current),
            10 => shipped_index_paths(&bytes).map(ShippedRustAbiCache::Previous),
            version if version < RUST_ABI_CACHE_VERSION => Err(format!(
                "Rust ABI cache version {version} predates the re-extractable version 10 index"
            )),
            version => Err(format!(
                "Rust ABI cache version {version}; this compiler reads version {RUST_ABI_CACHE_VERSION}"
            )),
        }
    }

    /// Read a cache's version and sections while retaining the input allocation for on-demand item decoding.
    ///
    /// File readers transfer their buffer here so a large cache never needs a second payload allocation. Block ranges
    /// are validated before the original encoded bytes are retained.
    pub fn decode_owned(bytes: Vec<u8>) -> Result<Self, String> {
        let lengths = section_lengths(&bytes)?;
        let mut sections = Vec::with_capacity(lengths.len());
        let mut at = HEADER_LENGTH;
        for (stored, raw) in lengths {
            let end = at.checked_add(stored).ok_or_else(malformed_cache)?;
            sections.push(unpack(bytes.get(at..end).ok_or_else(malformed_cache)?, raw)?);
            at = end;
        }
        let sections = Sections::decode(&sections, at, bytes.len())?;
        Ok(Self::new(sections, Payload::Bytes(bytes)))
    }

    /// Read only a file's sections, retaining its open handle for bounded on-demand block reads.
    ///
    /// The sections validate every block range against the file's length. Reads check the backing file's generation
    /// before and after collecting a block; changed, inaccessible or malformed blocks provide no facts. No mapping or
    /// unchecked access can fault if the backing file is truncated.
    pub fn open(path: &Path) -> Result<Self, String> {
        let file =
            File::open(path).map_err(|error| format!("cannot read the ABI cache {}: {error}", path.display()))?;
        Self::from_file(path.to_path_buf(), file)
    }

    /// Admit one file generation after validating its header, sections and every declared block range.
    fn from_file(path: PathBuf, mut file: File) -> Result<Self, String> {
        let stamp = FileStamp::observe(&file)
            .map_err(|error| format!("cannot inspect ABI cache {}: {error}", path.display()))?;
        let total = usize::try_from(stamp.length).map_err(|_| malformed_cache())?;
        let mut header = [0; HEADER_LENGTH];
        file.read_exact(&mut header).map_err(|_| malformed_cache())?;
        let mut sections = Vec::new();
        let mut at = HEADER_LENGTH;
        for (stored, raw) in section_lengths(&header)? {
            at = at
                .checked_add(stored)
                .filter(|&end| end <= total)
                .ok_or_else(malformed_cache)?;
            let mut encoded = vec![0; stored];
            file.read_exact(&mut encoded).map_err(|_| malformed_cache())?;
            sections.push(unpack(&encoded, raw)?);
        }
        let sections = Sections::decode(&sections, at, total)?;
        if FileStamp::observe(&file).map_err(|error| format!("cannot inspect ABI cache {}: {error}", path.display()))?
            != stamp
        {
            return Err(format!(
                "cannot read ABI cache {}: file changed while reading its index",
                path.display()
            ));
        }
        Ok(Self::new(
            sections,
            Payload::File {
                path,
                file: Mutex::new(file),
                stamp,
            },
        ))
    }

    /// A reader over decoded sections and their payload.
    fn new(sections: Sections, payload: Payload) -> Self {
        Self {
            index: sections.index,
            strings: sections.strings,
            directory: sections
                .directory
                .into_iter()
                .map(|block| Block {
                    offset: block.offset + sections.payload_start as u64,
                    ..block
                })
                .collect(),
            payload,
            blocks: Mutex::new(Vec::with_capacity(RETAINED_BLOCKS)),
            #[cfg(test)]
            full_item_restorations: AtomicUsize::new(0),
        }
    }

    /// The facts of one path, decoded from its record; `Ok(None)` when the cache does not hold it.
    ///
    /// A record that cannot be read or decoded (an I/O error, a poisoned handle, a file changed since its sections were
    /// read, a malformed block or record) is an `Err`, never a missing item.
    pub fn get(&self, path: &str) -> Result<Option<RustItemMetadata>, String> {
        let Some(&location) = self.index.get(path) else {
            return Ok(None);
        };
        self.record(location)
            .map(Some)
            .map_err(|error| format!("Rust ABI cache record for `{path}` does not decode: {error}"))
    }

    /// Read whether one path is a type alias without restoring the item's complete owned metadata record.
    ///
    /// The record prefix contains only the alias target and its type parameters. It shares the record's string table,
    /// while the full metadata payload remains untouched for readers that need it.
    pub(crate) fn get_type_alias(&self, path: &str) -> Result<Option<RustTypeAliasLookup>, String> {
        let Some(&location) = self.index.get(path) else {
            return Ok(None);
        };
        self.alias_record(location)
            .map(Some)
            .map_err(|error| format!("Rust ABI cache record for `{path}` does not decode: {error}"))
    }

    /// Count complete item restorations in focused cache tests.
    #[cfg(test)]
    fn full_item_restorations(&self) -> usize {
        self.full_item_restorations.load(Ordering::Relaxed)
    }

    /// Decode every retained item of a bounded fallback result. Shipped complete-crate caches should keep using
    /// on-demand lookup; this consuming operation is for restoring a previously requested extraction exactly.
    pub fn into_items(self) -> Result<BTreeMap<String, RustItemMetadata>, String> {
        let mut items = BTreeMap::new();
        for (path, &location) in &self.index {
            items.insert(path.clone(), self.record(location)?);
        }
        Ok(items)
    }

    /// Decode one record from its block.
    fn record<T: DeserializeOwned>(&self, location: Location) -> Result<T, String> {
        #[cfg(test)]
        self.full_item_restorations.fetch_add(1, Ordering::Relaxed);
        let (block, range) = self.record_bytes(location)?;
        let (_, item) = split_record(block.get(range).ok_or_else(malformed_cache)?)?;
        strings::decode(item, &self.strings)
    }

    /// Decode one record's alias prefix without restoring its full metadata payload.
    fn alias_record(&self, location: Location) -> Result<RustTypeAliasLookup, String> {
        let (block, range) = self.record_bytes(location)?;
        let (alias, _) = split_record(block.get(range).ok_or_else(malformed_cache)?)?;
        let alias = strings::decode::<Option<RustTypeAlias>>(alias, &self.strings)?;
        Ok(match alias {
            Some(alias) => RustTypeAliasLookup::Alias(alias),
            None => RustTypeAliasLookup::NotAlias,
        })
    }

    /// Retain a record's block while returning the validated range of that record within it.
    fn record_bytes(&self, location: Location) -> Result<(Arc<Vec<u8>>, std::ops::Range<usize>), String> {
        let block = self.block(location.block)?;
        let start = location.offset as usize;
        let end = start
            .checked_add(location.length as usize)
            .filter(|&end| end <= block.len())
            .ok_or_else(malformed_cache)?;
        Ok((block, start..end))
    }

    /// One decompressed block, from the retained ones when a recent lookup used it.
    fn block(&self, id: u32) -> Result<Arc<Vec<u8>>, String> {
        let mut retained = self
            .blocks
            .lock()
            .map_err(|_| "Rust ABI cache block store is poisoned".to_owned())?;
        if let Some(position) = retained.iter().position(|(block, _)| *block == id) {
            let entry = retained.remove(position);
            let bytes = Arc::clone(&entry.1);
            retained.insert(0, entry);
            return Ok(bytes);
        }
        let block = self.directory.get(id as usize).ok_or_else(malformed_cache)?;
        let stored = self.payload.range(block.offset as usize, block.length as usize)?;
        let bytes = Arc::new(unpack(&stored, block.raw_length as usize)?);
        retained.truncate(RETAINED_BLOCKS - 1);
        retained.insert(0, (id, Arc::clone(&bytes)));
        Ok(bytes)
    }

    /// Whether the cache holds `path`.
    pub fn contains(&self, path: &str) -> bool {
        self.index.contains_key(path)
    }

    /// Whether this complete public-item cache covers a crate's namespace.
    ///
    /// Crate roots themselves need not have an item payload. Their public descendants identify coverage; an empty
    /// crate has no public facts to answer and may be extracted again without duplicating an item's authority.
    pub fn covers_crate(&self, name: &str) -> bool {
        let prefix = format!("{name}::");
        self.index
            .range(prefix.clone()..)
            .next()
            .is_some_and(|(path, _)| path.starts_with(&prefix))
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

impl Sections {
    /// Decode the decompressed index, string table and block directory, validating every block range against the
    /// `total` length of a cache whose first block starts at `payload_start`.
    fn decode(sections: &[Vec<u8>], payload_start: usize, total: usize) -> Result<Self, String> {
        let [index, strings, directory] = sections else {
            return Err(malformed_cache());
        };
        let index: Vec<(String, Location)> = postcard::from_bytes(index).map_err(|error| error.to_string())?;
        let strings: Vec<String> = postcard::from_bytes(strings)
            .map_err(|error| format!("Rust ABI cache string table does not decode: {error}"))?;
        let directory: Vec<Block> = postcard::from_bytes(directory).map_err(|error| error.to_string())?;
        for block in &directory {
            let end = (payload_start as u64)
                .checked_add(block.offset)
                .and_then(|start| start.checked_add(block.length))
                .ok_or_else(malformed_cache)?;
            // Record offsets within a block are 32-bit, so no valid block decompresses to more.
            if end > total as u64 || block.raw_length > u64::from(u32::MAX) {
                return Err(malformed_cache());
            }
        }
        for (_, location) in &index {
            let block = directory.get(location.block as usize).ok_or_else(malformed_cache)?;
            if u64::from(location.offset) + u64::from(location.length) > block.raw_length {
                return Err(malformed_cache());
            }
        }
        Ok(Self {
            index: index.into_iter().collect(),
            strings,
            directory,
            payload_start,
        })
    }
}

/// Validate the shared header and read the stored and raw length of each section without interpreting any record.
fn section_lengths(header: &[u8]) -> Result<[(usize, usize); 3], String> {
    let version = cache_version(header)?;
    if version != RUST_ABI_CACHE_VERSION {
        return Err(format!(
            "Rust ABI cache version {version}; this compiler reads version {RUST_ABI_CACHE_VERSION}"
        ));
    }
    encoded_section_lengths(header)
}

/// Read the three section lengths shared by cache versions 10 and 11 after the caller validates the version.
fn encoded_section_lengths(header: &[u8]) -> Result<[(usize, usize); 3], String> {
    let mut lengths = [(0, 0); 3];
    for (section, slot) in lengths.iter_mut().enumerate() {
        *slot = (
            header_length(header, 4 + 16 * section)?,
            header_length(header, 12 + 16 * section)?,
        );
    }
    Ok(lengths)
}

/// Validate a version-10 cache's unchanged index sections and retain only the paths that need candidate extraction.
fn shipped_index_paths(bytes: &[u8]) -> Result<BTreeSet<String>, String> {
    let lengths = encoded_section_lengths(bytes)?;
    let mut decoded = Vec::with_capacity(lengths.len());
    let mut at = HEADER_LENGTH;
    for (stored, raw) in lengths {
        let end = at.checked_add(stored).ok_or_else(malformed_cache)?;
        decoded.push(unpack(bytes.get(at..end).ok_or_else(malformed_cache)?, raw)?);
        at = end;
    }
    let sections = Sections::decode(&decoded, at, bytes.len())?;
    Ok(sections.index.into_keys().collect())
}

/// Read the cache layout version without admitting any section or record from that layout.
fn cache_version(header: &[u8]) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        header
            .get(0..4)
            .ok_or_else(malformed_cache)?
            .try_into()
            .map_err(|_| malformed_cache())?,
    ))
}

/// One little-endian length of the header, starting at `at`.
fn header_length(header: &[u8], at: usize) -> Result<usize, String> {
    let length = u64::from_le_bytes(
        header
            .get(at..at + 8)
            .ok_or_else(malformed_cache)?
            .try_into()
            .map_err(|_| malformed_cache())?,
    );
    usize::try_from(length).map_err(|_| malformed_cache())
}

/// Compress one section or block.
fn pack(raw: &[u8]) -> Result<Vec<u8>, String> {
    snap::raw::Encoder::new()
        .compress_vec(raw)
        .map_err(|error| format!("cannot compress a Rust ABI cache block: {error}"))
}

/// Decompress one stored section or block, refusing any whose decompressed length differs from the declared one
/// before allocating for it.
fn unpack(stored: &[u8], raw_length: usize) -> Result<Vec<u8>, String> {
    if snap::raw::decompress_len(stored).map_err(|_| malformed_cache())? != raw_length {
        return Err(malformed_cache());
    }
    snap::raw::Decoder::new()
        .decompress_vec(stored)
        .map_err(|_| malformed_cache())
}

/// Preserve the cache decoder's named malformed-input refusal across both storage forms.
fn malformed_cache() -> String {
    "malformed Rust ABI cache".to_owned()
}

/// Owned encoded bytes or a generation-checked file, with one authority for range reads.
#[derive(Debug)]
enum Payload {
    /// In-memory clients retain their original allocation.
    Bytes(Vec<u8>),
    /// File readers keep the opened inode and serialize seeks, never reopening a potentially replaced path.
    File {
        path: PathBuf,
        file: Mutex<File>,
        stamp: FileStamp,
    },
}

impl Payload {
    /// Read one admitted range; a changed file, a short read or an unavailable lock is an error, not a missing item.
    fn range(&self, offset: usize, length: usize) -> Result<Vec<u8>, String> {
        match self {
            Self::Bytes(bytes) => offset
                .checked_add(length)
                .and_then(|end| bytes.get(offset..end))
                .map(<[u8]>::to_vec)
                .ok_or_else(malformed_cache),
            Self::File { path, file, stamp } => {
                let mut file = file
                    .lock()
                    .map_err(|_| "Rust ABI cache handle is poisoned".to_owned())?;
                Self::require_stamp(&file, stamp, path)?;
                let start = u64::try_from(offset).map_err(|_| malformed_cache())?;
                file.seek(SeekFrom::Start(start))
                    .map_err(|error| format!("cannot seek ABI cache {}: {error}", path.display()))?;
                let mut bytes = vec![0; length];
                file.read_exact(&mut bytes)
                    .map_err(|error| format!("cannot read ABI cache {}: {error}", path.display()))?;
                Self::require_stamp(&file, stamp, path)?;
                Ok(bytes)
            }
        }
    }

    /// Refuse a read from a cache file that changed since its sections were read.
    fn require_stamp(file: &File, stamp: &FileStamp, path: &Path) -> Result<(), String> {
        if FileStamp::observe(file).map_err(|error| format!("cannot inspect ABI cache {}: {error}", path.display()))?
            != *stamp
        {
            return Err(format!(
                "cannot read ABI cache {}: file changed while reading a record",
                path.display()
            ));
        }
        Ok(())
    }
}

/// A retained handle's generation; Unix status time also catches writes that restore the modification time.
#[derive(Debug, PartialEq, Eq)]
struct FileStamp {
    length: u64,
    modified: SystemTime,
    #[cfg(unix)]
    changed: (i64, i64),
}

impl FileStamp {
    /// Observe the same open handle used for section and block reads, independently of path replacement.
    fn observe(file: &File) -> std::io::Result<Self> {
        let metadata: Metadata = file.metadata()?;
        Ok(Self {
            length: metadata.len(),
            modified: metadata.modified()?,
            #[cfg(unix)]
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use incan_lang::interop::{
        RustAssociatedTypeBinding, RustImplementedTrait, RustItemKind, RustItemMetadata, RustModuleChild,
        RustModuleChildKind, RustModuleInfo, RustTypeInfo, RustVisibility,
    };

    use super::{RUST_ABI_CACHE_VERSION, RustAbiCache, ShippedRustAbiCache, encode_rust_abi_cache};

    /// A module item whose children repeat the same names, as crate facts do.
    fn module(path: &str, children: usize) -> RustItemMetadata {
        RustItemMetadata {
            canonical_path: path.to_owned(),
            definition_path: Some(format!("{path}::inner")),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Module(RustModuleInfo {
                children: (0..children)
                    .map(|child| RustModuleChild {
                        name: format!("child_{}", child % 7),
                        kind_hint: RustModuleChildKind::Function,
                    })
                    .collect(),
            }),
        }
    }

    /// A type item carrying deliberately large trait facts that an alias-only read must skip.
    fn type_item(path: &str, alias_target: Option<&str>, implementations: usize) -> RustItemMetadata {
        RustItemMetadata {
            canonical_path: path.to_owned(),
            definition_path: None,
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                type_params: vec!["T".to_owned()],
                alias_target: alias_target.map(str::to_owned),
                implemented_traits: (0..implementations)
                    .map(|index| RustImplementedTrait {
                        path: format!("demo::Trait{index}"),
                        mutable_reference: false,
                        self_type: format!("{path}<T>"),
                        type_params: vec!["T".to_owned()],
                        type_arguments: vec!["T".to_owned()],
                        bounds: Vec::new(),
                        associated_type_bindings: vec![RustAssociatedTypeBinding {
                            name: "Output".to_owned(),
                            value_path: "T".to_owned(),
                        }],
                    })
                    .collect(),
                ..RustTypeInfo::default()
            }),
        }
    }

    /// Enough items to span several record blocks.
    fn items() -> BTreeMap<String, RustItemMetadata> {
        (0..3000)
            .map(|item| {
                let path = format!("demo::module_{item:04}");
                (path.clone(), module(&path, 40))
            })
            .collect()
    }

    #[test]
    fn an_alias_lookup_does_not_restore_the_whole_item() -> Result<(), String> {
        let alias_path = "demo::Result";
        let nominal_path = "demo::Container";
        let items = BTreeMap::from([
            (
                alias_path.to_owned(),
                type_item(alias_path, Some("core::result::Result<T, std::io::Error>"), 10_000),
            ),
            (nominal_path.to_owned(), type_item(nominal_path, None, 10_000)),
        ]);
        let bytes = encode_rust_abi_cache(&items)?;
        let cache = RustAbiCache::decode(&bytes)?;

        let alias = cache
            .get_type_alias(alias_path)?
            .ok_or_else(|| "the cache omitted its alias item".to_owned())?
            .into_alias()
            .ok_or_else(|| "the type alias was classified as another item".to_owned())?;

        assert_eq!(alias.type_params, ["T"]);
        assert_eq!(alias.target, "core::result::Result<T, std::io::Error>");
        let nominal = cache
            .get_type_alias(nominal_path)?
            .ok_or_else(|| "the cache omitted its nominal item".to_owned())?;
        assert_eq!(nominal, super::RustTypeAliasLookup::NotAlias);
        assert_eq!(cache.full_item_restorations(), 0);
        assert!(cache.get(nominal_path)?.is_some());
        assert_eq!(cache.full_item_restorations(), 1);
        Ok(())
    }

    #[test]
    fn a_cache_round_trips_across_blocks_and_stays_small() -> Result<(), String> {
        let items = items();
        let bytes = encode_rust_abi_cache(&items)?;
        let cache = RustAbiCache::decode(&bytes)?;
        assert!(cache.directory.len() > 1, "the records span more than one block");
        assert_eq!(cache.len(), items.len());
        assert_eq!(cache.get("demo::module_1500")?, items.get("demo::module_1500").cloned());
        assert_eq!(cache.get("demo::absent")?, None);
        assert!(cache.covers_crate("demo"));
        let plain: usize = items
            .values()
            .map(|item| postcard::to_allocvec(item).map_or(0, |bytes| bytes.len()))
            .sum();
        assert!(
            bytes.len() * 10 < plain,
            "{} encoded bytes against {plain} plain",
            bytes.len()
        );
        assert_eq!(cache.into_items()?, items);
        Ok(())
    }

    #[test]
    fn a_file_backed_cache_reads_records_on_demand() -> Result<(), String> {
        let items = items();
        let directory = std::env::temp_dir().join(format!("incan-abi-cache-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let path = directory.join("demo.rustabi");
        std::fs::write(&path, encode_rust_abi_cache(&items)?).map_err(|error| error.to_string())?;
        let cache = RustAbiCache::open(&path)?;
        for probe in ["demo::module_0000", "demo::module_2999", "demo::module_0001"] {
            assert_eq!(cache.get(probe)?, items.get(probe).cloned());
        }
        let changed_cache = RustAbiCache::open(&path)?;
        std::fs::write(&path, b"changed")
            .map_err(|error| format!("cannot replace test ABI cache {}: {error}", path.display()))?;
        let changed = changed_cache
            .get("demo::module_0000")
            .err()
            .ok_or_else(|| "a changed file-backed cache was accepted".to_owned())?;
        if !changed.contains("read") || !changed.contains(&path.display().to_string()) {
            return Err(format!("changed ABI cache error lacks operation or path: {changed}"));
        }
        std::fs::remove_dir_all(&directory).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[test]
    fn an_older_shipped_cache_is_a_miss_and_a_newer_one_is_refused() -> Result<(), String> {
        let items = items();
        let current = encode_rust_abi_cache(&items)?;
        let mut older = current.clone();
        older[0..4].copy_from_slice(&(RUST_ABI_CACHE_VERSION - 1).to_le_bytes());
        let stale_paths = match RustAbiCache::decode_shipped(older)? {
            ShippedRustAbiCache::Previous(paths) => paths,
            ShippedRustAbiCache::Current(_) => return Err("an older layout was decoded as current records".to_owned()),
        };
        assert_eq!(stale_paths, items.keys().cloned().collect());

        let mut newer = current;
        newer[0..4].copy_from_slice(&(RUST_ABI_CACHE_VERSION + 1).to_le_bytes());
        let error = match RustAbiCache::decode_shipped(newer) {
            Ok(_) => return Err("a newer layout was accepted".to_owned()),
            Err(error) => error,
        };
        assert!(error.contains("version"), "{error}");
        Ok(())
    }
}
