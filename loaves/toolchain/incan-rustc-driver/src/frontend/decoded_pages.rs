//! Graph body pages this driver process has already decoded, by the digest of their bytes.
//!
//! A resident worker checks one program keystroke after keystroke, and the engine offers it the same unchanged pages
//! each time. Decoding and authenticating a page through the shared checker codec costs more than checking the body
//! afresh, so a page is decoded once per process: a later request for the same bytes and owner returns the record
//! the codec produced from them. Changed bytes have another digest and are decoded again.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use incan_frontend::checker_records::GraphBodyRecord;

/// Distinct pages kept at once. A worker that meets more starts again from none rather than evicting one by one.
const MAX_DECODED_PAGES: usize = 4096;

/// Decoded records by page digest.
static DECODED: Mutex<Option<HashMap<[u8; 32], GraphBodyRecord>>> = Mutex::new(None);

/// Pages this process decoded through the codec, so a check can report how many it decoded itself.
static DECODES: AtomicUsize = AtomicUsize::new(0);

/// The record decoded earlier from exactly these bytes, when it belongs to `identity`.
pub(super) fn decoded(digest: &[u8; 32], identity: &str) -> Option<GraphBodyRecord> {
    let pages = DECODED.lock().ok()?;
    pages
        .as_ref()?
        .get(digest)
        .filter(|record| record.identity == identity)
        .cloned()
}

/// Keep one page's record, decoded and authenticated just now, for later requests of the same bytes.
pub(super) fn remember(digest: [u8; 32], record: &GraphBodyRecord) {
    DECODES.fetch_add(1, Ordering::Relaxed);
    let Ok(mut guard) = DECODED.lock() else {
        return;
    };
    let pages = guard.get_or_insert_with(HashMap::new);
    if pages.len() >= MAX_DECODED_PAGES {
        pages.clear();
    }
    pages.insert(digest, record.clone());
}

/// Pages decoded by this process so far; a check reports the difference across its own activation.
pub(crate) fn decodes() -> usize {
    DECODES.load(Ordering::Relaxed)
}
