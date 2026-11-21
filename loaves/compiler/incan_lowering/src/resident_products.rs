//! Plan products this worker process has already placed, by their bytes and the nodes they were placed at.
//!
//! A resident worker lowers one program edit after edit, and every edit restores each unchanged module's bodies from
//! the same product bytes at the same node positions. Reading and placing a product through the Crumb codec costs
//! more than lowering many bodies, so a placement is made once per worker: a later restore of the same bytes at the
//! same nodes, file and line reads the placed record the worker kept. The record is immutable and fully determined by
//! that key, so what it restores is exactly what a fresh placement restores.

use std::collections::HashMap;
use std::sync::Mutex;

use incan_mir_plan_native::PlacedRecord;
use sha2::{Digest, Sha256};

use crate::record_positions::placed_record;

/// Distinct placements one worker keeps; the store starts over rather than grow past it.
const MAX_PLACEMENTS: usize = 16384;

static PLACED: Mutex<Option<HashMap<[u8; 32], PlacedRecord>>> = Mutex::new(None);

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
        && let Some(kept) = guard.as_ref().and_then(|placements| placements.get(&key))
    {
        return Ok((kept.clone(), true));
    }
    let placed = placed_record(encoded, positions, file, line)?;
    if let Ok(mut guard) = PLACED.lock() {
        let placements = guard.get_or_insert_with(HashMap::new);
        if placements.len() >= MAX_PLACEMENTS {
            placements.clear();
        }
        placements.insert(key, placed.clone());
    }
    Ok((placed, false))
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
