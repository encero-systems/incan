//! Plan products this worker process has already read, by the bytes they were read from.
//!
//! A resident worker lowers one program edit after edit, and every edit restores each unchanged module's bodies from
//! the same module page. Decoding a page's events and placing their products through the Crumb codec costs more than
//! lowering many bodies, so the worker keeps what it read: each module page by the digest of its bytes, its module
//! name and its placement, and on that page each restored product by the inputs it was restored with. A product
//! placed on the keyed route is kept by its bytes and the nodes it was placed at. Every kept value is immutable and
//! fully determined by its key, so what it restores is exactly what a fresh read restores; the stores start over
//! rather than grow past their bounds.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

use incan_mir_plan_native::PlacedRecord;
use sha2::{Digest, Sha256};

use crate::plan_modules::AdmittedPage;
use crate::record_positions::placed_record;
use crate::scalar_store::AdmittedPayload;

/// Distinct placements one worker keeps.
const MAX_PLACEMENTS: usize = 16384;
/// Distinct module pages one worker keeps.
const MAX_PAGES: usize = 4096;

static PLACED: Mutex<Option<HashMap<[u8; 32], (PlacedRecord, u64)>>> = Mutex::new(None);
static PAGES: Mutex<Option<HashMap<[u8; 32], Arc<ResidentPage>>>> = Mutex::new(None);

/// What a product restored from a module page is a function of besides the page's bytes: the body's frame name, its
/// file and line, its overflow policy, its checker witness, the producer profile, and the body's current node
/// positions when its page asks for placement (`None` keeps the event's own positions).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RestoreInputs {
    pub frame: String,
    pub file: String,
    pub line: i64,
    pub overflow: bool,
    pub reads: String,
    pub generation: String,
    pub positions: Option<Vec<(i64, i64)>>,
}

impl RestoreInputs {
    /// The digest of every input, each length-prefixed, the positions after whether there are any.
    fn digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for text in [&self.frame, &self.file, &self.reads, &self.generation] {
            hasher.update((text.len() as u64).to_le_bytes());
            hasher.update(text.as_bytes());
        }
        hasher.update(self.line.to_le_bytes());
        hasher.update([u8::from(self.overflow)]);
        match &self.positions {
            None => hasher.update([0]),
            Some(positions) => {
                hasher.update([1]);
                hasher.update((positions.len() as u64).to_le_bytes());
                for (node, column) in positions {
                    hasher.update(node.to_le_bytes());
                    hasher.update(column.to_le_bytes());
                }
            }
        }
        hasher.finalize().into()
    }
}

/// One module page as this worker read it, and each product restored from it by its restore inputs. A product is
/// added once and never changed.
pub struct ResidentPage {
    page: AdmittedPage,
    /// The bytes the page was read from; none for a page read for one request only.
    bytes: u64,
    /// Each restored product by the digest of its restore inputs, so a kept product holds no copy of them.
    restored: Mutex<HashMap<[u8; 32], Arc<AdmittedPayload>>>,
}

impl Deref for ResidentPage {
    type Target = AdmittedPage;

    fn deref(&self) -> &AdmittedPage {
        &self.page
    }
}

impl ResidentPage {
    /// A page read for this request only.
    pub fn unkept(page: AdmittedPage) -> Arc<Self> {
        Arc::new(Self {
            page,
            bytes: 0,
            restored: Mutex::new(HashMap::new()),
        })
    }

    /// The product restored from this page for `inputs`, restored with `restore` the first time; a product that does
    /// not restore is not kept, so its reason is given again.
    pub fn restored<E>(
        &self,
        inputs: &RestoreInputs,
        restore: impl FnOnce() -> Result<AdmittedPayload, E>,
    ) -> Result<Arc<AdmittedPayload>, E> {
        let key = inputs.digest();
        if let Ok(guard) = self.restored.lock()
            && let Some(kept) = guard.get(&key)
        {
            return Ok(kept.clone());
        }
        let payload = Arc::new(restore()?);
        if let Ok(mut guard) = self.restored.lock() {
            guard.insert(key, payload.clone());
        }
        Ok(payload)
    }
}

/// The page this worker read from the same bytes for the same module and placement, or the one `read` makes from
/// them now; `None` when `read` refuses them, which is not kept.
pub fn resident_page(
    encoded: &[u8],
    module: &str,
    inspect: bool,
    read: impl FnOnce() -> Option<AdmittedPage>,
) -> Option<Arc<ResidentPage>> {
    let mut hasher = Sha256::new();
    hasher.update((module.len() as u64).to_le_bytes());
    hasher.update(module.as_bytes());
    hasher.update([u8::from(inspect)]);
    hasher.update(encoded);
    let key: [u8; 32] = hasher.finalize().into();
    if let Ok(guard) = PAGES.lock()
        && let Some(kept) = guard.as_ref().and_then(|pages| pages.get(&key))
    {
        return Some(kept.clone());
    }
    let page = Arc::new(ResidentPage {
        page: read()?,
        bytes: encoded.len() as u64,
        restored: Mutex::new(HashMap::new()),
    });
    if let Ok(mut guard) = PAGES.lock() {
        let pages = guard.get_or_insert_with(HashMap::new);
        if pages.len() >= MAX_PAGES {
            pages.clear();
        }
        pages.insert(key, page.clone());
    }
    Some(page)
}

/// Place a product's bytes at `positions` in `file` from `line`, or read the placement this worker already made of
/// the same bytes at the same nodes; the flag says whether the worker kept it.
pub fn resident_placed(
    encoded: &[u8],
    positions: &[(i64, i64)],
    file: &str,
    line: i64,
) -> Result<(PlacedRecord, bool), String> {
    let key = placement_key(encoded, positions, file, line);
    if let Ok(guard) = PLACED.lock()
        && let Some((kept, _)) = guard.as_ref().and_then(|placements| placements.get(&key))
    {
        return Ok((kept.clone(), true));
    }
    let placed = placed_record(encoded, positions, file, line)?;
    if let Ok(mut guard) = PLACED.lock() {
        let placements = guard.get_or_insert_with(HashMap::new);
        if placements.len() >= MAX_PLACEMENTS {
            placements.clear();
        }
        placements.insert(key, (placed.clone(), encoded.len() as u64));
    }
    Ok((placed, false))
}

/// What this worker keeps between requests, store by store: the values kept and the bytes they were read from (a
/// restored product is read from its page's bytes, so it adds none of its own).
pub fn resident_inventory() -> Vec<(&'static str, usize, u64)> {
    let mut inventory = Vec::new();
    if let Ok(guard) = PAGES.lock() {
        let pages: Vec<&Arc<ResidentPage>> = guard.as_ref().map(|pages| pages.values().collect()).unwrap_or_default();
        let bytes = pages.iter().map(|page| page.bytes).sum();
        let restored = pages
            .iter()
            .map(|page| page.restored.lock().map_or(0, |restored| restored.len()))
            .sum();
        inventory.push(("lowering module pages", pages.len(), bytes));
        inventory.push(("lowering restored products", restored, 0));
    }
    if let Ok(guard) = PLACED.lock() {
        let placements: Vec<u64> = guard
            .as_ref()
            .map(|placements| placements.values().map(|(_, bytes)| *bytes).collect())
            .unwrap_or_default();
        inventory.push(("lowering placed products", placements.len(), placements.iter().sum()));
    }
    inventory
}

/// The digest of everything a placement is a function of: the product bytes, each node position, the file and line.
fn placement_key(encoded: &[u8], positions: &[(i64, i64)], file: &str, line: i64) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update((encoded.len() as u64).to_le_bytes());
    hasher.update(encoded);
    hasher.update((positions.len() as u64).to_le_bytes());
    for (node, column) in positions {
        hasher.update(node.to_le_bytes());
        hasher.update(column.to_le_bytes());
    }
    hasher.update((file.len() as u64).to_le_bytes());
    hasher.update(file.as_bytes());
    hasher.update(line.to_le_bytes());
    hasher.finalize().into()
}
