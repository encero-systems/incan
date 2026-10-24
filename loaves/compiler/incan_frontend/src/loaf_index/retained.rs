//! Process-local immutable artifact readings, guarded again for every request. Sessions, resolved source facts and
//! checking outcomes never enter these stores. Replacements and restored-mtime edits invalidate through ctime;
//! unsupported observations keep the ordinary reader. Each store has bounded payload bytes and entry count.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use incan_lang::interop::RustAbiCache;

use super::manifest_input::{Generation, generation};
use super::{LoafArtifact, LoafIndexError};

const MAX_ENTRIES: usize = 32;
const MAX_BYTES: u64 = 384 * 1024 * 1024;

static ARTIFACTS: OnceLock<Mutex<Store<LoafArtifact>>> = OnceLock::new();
static ABI: OnceLock<Mutex<Store<Arc<RustAbiCache>>>> = OnceLock::new();

/// The current generations of all files a decoded value depends on, including optional assets' absence.
#[derive(Clone, PartialEq, Eq)]
struct Inputs {
    files: Vec<(PathBuf, Option<Generation>)>,
    bytes: u64,
}

/// One immutable reading and the exact generations that authorized it.
struct Reading<T> {
    inputs: Inputs,
    value: T,
}

/// A bounded store: replacing a generation replaces its old reading, and capacity pressure drops retained readings.
struct Store<T> {
    readings: BTreeMap<PathBuf, Reading<T>>,
    bytes: u64,
}

impl<T> Default for Store<T> {
    /// Start with no retained inputs or values.
    fn default() -> Self {
        Self {
            readings: BTreeMap::new(),
            bytes: 0,
        }
    }
}

impl<T: Clone> Store<T> {
    /// Borrow a decoded value only for its complete current input generation.
    fn get(&self, key: &Path, inputs: &Inputs) -> Option<T> {
        self.readings
            .get(key)
            .filter(|reading| reading.inputs == *inputs)
            .map(|reading| reading.value.clone())
    }

    /// Keep one stable reading within the count and payload budget; eviction changes performance only.
    fn insert(&mut self, key: PathBuf, inputs: Inputs, value: T) {
        if let Some(previous) = self.readings.remove(&key) {
            self.bytes = self.bytes.saturating_sub(previous.inputs.bytes);
        }
        if inputs.bytes > MAX_BYTES {
            return;
        }
        if self.readings.len() >= MAX_ENTRIES || self.bytes.saturating_add(inputs.bytes) > MAX_BYTES {
            self.readings.clear();
            self.bytes = 0;
        }
        self.bytes += inputs.bytes;
        self.readings.insert(key, Reading { inputs, value });
    }
}

/// Observe a canonical artifact's required files and optional ABI asset, without making absence into acceptance.
fn observe(paths: &[PathBuf]) -> Option<Inputs> {
    let mut files = Vec::new();
    let mut bytes = 0_u64;
    for path in paths {
        let state = match fs::metadata(path) {
            Ok(metadata) => {
                bytes = bytes.checked_add(metadata.len())?;
                Some(generation(&metadata)?)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return None,
        };
        files.push((path.clone(), state));
    }
    Some(Inputs { files, bytes })
}

/// Reuse a Loaf's validated manifest and authenticated ABI only while both current files match their reading.
pub(super) fn artifact(dir: &Path, name: &str) -> Result<LoafArtifact, LoafIndexError> {
    let canonical = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_owned());
    let key = canonical.join(format!("{name}.incnlib"));
    let paths = [key.clone(), canonical.join(format!("{name}.rustabi"))];
    let before = observe(&paths);
    let store = ARTIFACTS.get_or_init(Mutex::default);
    if let Some(inputs) = &before
        && let Some(mut value) = store.lock().ok().and_then(|store| store.get(&key, inputs))
        && observe(&paths).as_ref() == Some(inputs)
    {
        value.dir = dir.to_owned();
        tracing::debug!(target: "incan::invocation", loaf = name, "retained immutable Loaf");
        return Ok(value);
    }
    let value = LoafArtifact::read_current(dir, name)?;
    if let Some(inputs) = before
        && observe(&paths).as_ref() == Some(&inputs)
        && let Ok(mut store) = store.lock()
    {
        store.insert(key, inputs, value.clone());
    }
    Ok(value)
}

/// Open the file-backed toolchain or unit ABI index once per unchanged generation; malformed inputs are never retained,
/// and item payloads stay lazy.
pub(super) fn abi(path: &Path) -> Result<Arc<RustAbiCache>, String> {
    let canonical = fs::canonicalize(path).map_err(|error| error.to_string())?;
    let paths = [canonical.clone()];
    let before = observe(&paths);
    let store = ABI.get_or_init(Mutex::default);
    if let Some(inputs) = &before
        && let Some(value) = store.lock().ok().and_then(|store| store.get(&canonical, inputs))
        && observe(&paths).as_ref() == Some(inputs)
    {
        tracing::debug!(target: "incan::invocation", path = %path.display(), "retained immutable Rust ABI index");
        return Ok(value);
    }
    let value = Arc::new(RustAbiCache::open(&canonical)?);
    if let Some(inputs) = before
        && observe(&paths).as_ref() == Some(&inputs)
        && let Ok(mut store) = store.lock()
    {
        store.insert(canonical, inputs, Arc::clone(&value));
    }
    Ok(value)
}
