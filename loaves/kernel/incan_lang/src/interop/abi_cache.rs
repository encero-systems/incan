//! The Rust ABI cache: the facts of compiled Rust crates (every public item's [`RustItemMetadata`], keyed by path),
//! stored the way `.incnbody` stores executable bodies: a version, a postcard index of each path's payload range, then
//! each item's postcard payload. A reader decodes the index once and an item only when a compilation asks for it.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::metadata::RustItemMetadata;

/// The cache layout's version; a reader refuses any other. Version 9 retains native owners' lifetime-parameter
/// presence so checking can refuse borrowed signatures without a lifetime contract.
pub const RUST_ABI_CACHE_VERSION: u32 = 9;

/// The version and encoded index length precede every cache index.
const HEADER_LENGTH: usize = 12;

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

/// A decoded ABI cache index over retained bytes or an open file; items decode on demand.
#[derive(Debug)]
pub struct RustAbiCache {
    index: BTreeMap<String, (usize, usize)>,
    payload: Payload,
}

impl RustAbiCache {
    /// Read borrowed cache bytes, retaining an owned copy for on-demand item decoding.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        Self::decode_owned(bytes.to_vec())
    }

    /// Read a cache's version and index while retaining the input allocation for on-demand item decoding.
    ///
    /// File readers transfer their buffer here so a large cache never needs a second payload allocation. Index ranges
    /// are validated and translated to absolute byte offsets before the original encoded bytes are retained.
    pub fn decode_owned(bytes: Vec<u8>) -> Result<Self, String> {
        let end = payload_start(&bytes)?;
        let index = decode_index(
            bytes.get(HEADER_LENGTH..end).ok_or_else(malformed_cache)?,
            end,
            bytes.len(),
        )?;
        Ok(Self {
            index,
            payload: Payload::Bytes(bytes),
        })
    }

    /// Read only a file's index, retaining its open handle for bounded on-demand record reads.
    ///
    /// The index validates every payload range against the file's length. Reads check the backing file's generation
    /// before and after collecting a record; changed, inaccessible or malformed records provide no facts. No mapping or
    /// unchecked access can fault if the backing file is truncated.
    pub fn open(path: &Path) -> Result<Self, String> {
        let file =
            File::open(path).map_err(|error| format!("cannot read the ABI cache {}: {error}", path.display()))?;
        Self::from_file(file).map_err(|error| format!("{}: {error}", path.display()))
    }

    /// Admit one file generation after validating its header, complete index and every declared payload range.
    fn from_file(mut file: File) -> Result<Self, String> {
        let stamp = FileStamp::observe(&file).map_err(|error| error.to_string())?;
        let total = usize::try_from(stamp.length).map_err(|_| malformed_cache())?;
        let mut header = [0; HEADER_LENGTH];
        file.read_exact(&mut header).map_err(|_| malformed_cache())?;
        let end = payload_start(&header)?;
        if end > total {
            return Err(malformed_cache());
        }
        let mut encoded = vec![0; end - HEADER_LENGTH];
        file.read_exact(&mut encoded).map_err(|_| malformed_cache())?;
        let index = decode_index(&encoded, end, total)?;
        if FileStamp::observe(&file).map_err(|error| error.to_string())? != stamp {
            return Err("Rust ABI cache changed while reading its index".into());
        }
        Ok(Self {
            index,
            payload: Payload::File {
                file: Mutex::new(file),
                stamp,
            },
        })
    }

    /// The facts of one path, decoded from its payload; `None` when the cache does not hold it.
    pub fn get(&self, path: &str) -> Option<RustItemMetadata> {
        let (offset, length) = *self.index.get(path)?;
        postcard::from_bytes(&self.payload.record(offset, length)?).ok()
    }

    /// Decode every retained item of a bounded fallback result. Shipped complete-crate caches should keep using
    /// on-demand lookup; this consuming operation is for restoring a previously requested extraction exactly.
    pub fn into_items(self) -> Result<BTreeMap<String, RustItemMetadata>, String> {
        self.index
            .into_iter()
            .map(|(path, (offset, length))| {
                let record = self.payload.record(offset, length).ok_or_else(malformed_cache)?;
                postcard::from_bytes(&record)
                    .map(|metadata| (path, metadata))
                    .map_err(|error| error.to_string())
            })
            .collect()
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

/// The single range decoder shared by owned buffers and file-backed cache indexes.
fn decode_index(encoded: &[u8], start: usize, total: usize) -> Result<BTreeMap<String, (usize, usize)>, String> {
    let entries: Vec<IndexEntry> = postcard::from_bytes(encoded).map_err(|error| error.to_string())?;
    let mut index = BTreeMap::new();
    for entry in entries {
        let offset = usize::try_from(entry.offset)
            .map_err(|_| malformed_cache())?
            .checked_add(start)
            .ok_or_else(malformed_cache)?;
        let length = usize::try_from(entry.length).map_err(|_| malformed_cache())?;
        if offset.checked_add(length).is_none_or(|stop| stop > total) {
            return Err(malformed_cache());
        }
        index.insert(entry.path, (offset, length));
    }
    Ok(index)
}

/// Validate the shared header and locate the first payload byte without interpreting any record.
fn payload_start(header: &[u8]) -> Result<usize, String> {
    let version = u32::from_le_bytes(
        header
            .get(0..4)
            .ok_or_else(malformed_cache)?
            .try_into()
            .map_err(|_| malformed_cache())?,
    );
    if version != RUST_ABI_CACHE_VERSION {
        return Err(format!(
            "Rust ABI cache version {version}; this compiler reads version {RUST_ABI_CACHE_VERSION}"
        ));
    }
    let length = u64::from_le_bytes(
        header
            .get(4..HEADER_LENGTH)
            .ok_or_else(malformed_cache)?
            .try_into()
            .map_err(|_| malformed_cache())?,
    );
    HEADER_LENGTH
        .checked_add(usize::try_from(length).map_err(|_| malformed_cache())?)
        .ok_or_else(malformed_cache)
}

/// Preserve the cache decoder's named malformed-input refusal across both storage forms.
fn malformed_cache() -> String {
    "malformed Rust ABI cache".to_owned()
}

/// Owned encoded bytes or a generation-checked file, with one authority for index interpretation.
#[derive(Debug)]
enum Payload {
    /// In-memory clients retain their original allocation.
    Bytes(Vec<u8>),
    /// File readers keep the opened inode and serialize seeks, never reopening a potentially replaced path.
    File { file: Mutex<File>, stamp: FileStamp },
}

impl Payload {
    /// Read one admitted range; changed files, short reads and unavailable locks cannot supply an item.
    fn record(&self, offset: usize, length: usize) -> Option<Cow<'_, [u8]>> {
        match self {
            Self::Bytes(bytes) => Some(Cow::Borrowed(bytes.get(offset..offset.checked_add(length)?)?)),
            Self::File { file, stamp } => {
                let mut file = file.lock().ok()?;
                if FileStamp::observe(&file).ok()? != *stamp {
                    return None;
                }
                file.seek(SeekFrom::Start(u64::try_from(offset).ok()?)).ok()?;
                let mut bytes = vec![0; length];
                file.read_exact(&mut bytes).ok()?;
                if FileStamp::observe(&file).ok()? != *stamp {
                    return None;
                }
                Some(Cow::Owned(bytes))
            }
        }
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
    /// Observe the same open handle used for index and record reads, independently of path replacement.
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
