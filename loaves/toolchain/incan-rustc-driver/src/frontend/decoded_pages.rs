//! Graph body pages this driver process has already checked, by observed file generation and byte digest.
//!
//! A resident worker checks one program keystroke after keystroke, and the engine offers it the same unchanged pages
//! each time. Decoding and authenticating a page through the shared checker codec costs more than checking the body
//! afresh, so a page is decoded once per process: a later request for the same bytes and owner returns the record
//! the codec produced from them. A stable file is read and hashed once, while replacement is observed independently.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use incan_frontend::checker_records::GraphBodyRecord;

use crate::native_search::FileGeneration;

/// Distinct decoded values and file observations retained by this worker.
const MAX_DECODED_PAGES: usize = 4096;

#[derive(Default)]
struct DecodedPages {
    pages: HashMap<[u8; 32], GraphBodyRecord>,
    generations: HashMap<FileGeneration, [u8; 32]>,
    /// The bytes the kept pages were decoded from.
    bytes: u64,
}

static DECODED: Mutex<Option<DecodedPages>> = Mutex::new(None);

/// Pages admitted through the codec, so each check reports exactly its own new decodes.
static DECODES: AtomicUsize = AtomicUsize::new(0);

/// Read a record from that exact file generation without opening, reading or hashing its payload.
pub(super) fn observed(generation: FileGeneration, identity: &str) -> Option<GraphBodyRecord> {
    let guard = DECODED.lock().ok()?;
    let pages = guard.as_ref()?;
    let digest = pages.generations.get(&generation)?;
    lookup(pages, digest, identity)
}

/// Recover the earlier logical record when another file contains those exact bytes under the same owner.
pub(super) fn decoded(digest: [u8; 32], identity: &str, generation: Option<FileGeneration>) -> Option<GraphBodyRecord> {
    let mut guard = DECODED.lock().ok()?;
    let pages = guard.as_mut()?;
    let record = lookup(pages, &digest, identity)?;
    remember_generation(pages, generation, digest);
    Some(record)
}

/// Borrow the exact authenticated identity before sharing its immutable payload with a new invocation.
fn lookup(pages: &DecodedPages, digest: &[u8; 32], identity: &str) -> Option<GraphBodyRecord> {
    pages
        .pages
        .get(digest)
        .filter(|record| record.identity == identity)
        .cloned()
}

/// Keep one newly admitted record and the exact file generation that delivered it.
pub(super) fn remember(digest: [u8; 32], generation: Option<FileGeneration>, record: &GraphBodyRecord, bytes: usize) {
    DECODES.fetch_add(1, Ordering::Relaxed);
    let Ok(mut guard) = DECODED.lock() else { return };
    let pages = guard.get_or_insert_with(DecodedPages::default);
    if pages.pages.len() >= MAX_DECODED_PAGES {
        pages.pages.clear();
        pages.generations.clear();
        pages.bytes = 0;
    }
    if pages.pages.insert(digest, record.clone()).is_none() {
        pages.bytes += bytes as u64;
    }
    remember_generation(pages, generation, digest);
}

/// One content identity can appear at several filesystem generations without retaining duplicate payloads.
fn remember_generation(pages: &mut DecodedPages, generation: Option<FileGeneration>, digest: [u8; 32]) {
    let Some(generation) = generation else { return };
    if pages.generations.len() >= MAX_DECODED_PAGES {
        pages.generations.clear();
    }
    pages.generations.insert(generation, digest);
}

/// Pages decoded by this process so far; a check reports the difference across its own activation.
pub(crate) fn decodes() -> usize {
    DECODES.load(Ordering::Relaxed)
}

/// The checked pages this worker keeps and the bytes they were decoded from.
pub(crate) fn inventory() -> (usize, u64) {
    DECODED
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|pages| (pages.pages.len(), pages.bytes)))
        .unwrap_or((0, 0))
}
