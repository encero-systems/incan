//! Reuse of a row writer's admitted description set across the writes of one process.
//!
//! Every row write first decodes its complete D descriptions, admits their kinds and references, orders the dictionary
//! by K and encodes the dictionary entries. A compiler writes thousands of single-row envelopes against a handful of
//! fixed description sets, so that admission ran again for every row. The admitted dictionary and entries are a pure
//! function of the encoded descriptions, their modes and the registered kinds, and the two steps charge the same bytes
//! and elements whatever allowance they start from. A retained set therefore answers any write whose allowance covers
//! those charges and the nesting depth the set was admitted under; any other write admits from scratch and reports
//! its own refusal. A refused admission is never retained.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use super::columns::Budget;
use super::rows::Stored;

/// Distinct description sets one process retains; a writer past the bound starts again from none.
const RETAINED_SETS: usize = 64;

/// Bytes of each description's start and end that select a retained set's bucket before the exact comparison.
const FINGERPRINT_BYTES: usize = 32;

/// Bytes and elements one completed step charged.
#[derive(Clone, Copy)]
pub(super) struct Charge {
    bytes: u64,
    elements: u64,
}

impl Charge {
    /// What `budget` lost since it held `before`.
    pub(super) fn since(before: (u64, u64), budget: &Budget) -> Self {
        let (bytes, elements) = budget.left();
        Self {
            bytes: before.0 - bytes,
            elements: before.1 - elements,
        }
    }
}

/// One admitted description set, what its admission and entry encoding charge, and the depth it was admitted under.
pub(super) struct AdmittedSchema {
    pub(super) dictionary: Vec<Stored>,
    pub(super) entries: Vec<Vec<u8>>,
    admission: Charge,
    encoding: Charge,
    depth: u64,
}

impl AdmittedSchema {
    /// Whether a write starting from `allowance` admits and encodes this set exactly as it did before.
    pub(super) fn covers(&self, (bytes, elements, depth): (u64, u64, u64)) -> bool {
        let charged = (
            self.admission.bytes.checked_add(self.encoding.bytes),
            self.admission.elements.checked_add(self.encoding.elements),
        );
        matches!(charged, (Some(need_bytes), Some(need_elements))
            if bytes >= need_bytes && elements >= need_elements && depth >= self.depth)
    }

    /// Charge `budget` what admitting this set charged.
    pub(super) fn charge_admission(&self, budget: &mut Budget) {
        charge(budget, self.admission);
    }

    /// Charge `budget` what encoding this set's entries charged.
    pub(super) fn charge_entries(&self, budget: &mut Budget) {
        charge(budget, self.encoding);
    }
}

/// The exact inputs a retained admission answers for.
struct Retained {
    descriptions: Vec<(Vec<u8>, bool)>,
    kinds: Vec<(String, bool)>,
    schema: Arc<AdmittedSchema>,
}

static RETAINED: LazyLock<Mutex<HashMap<Vec<u8>, Vec<Retained>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The admission an earlier write made for exactly these descriptions and kinds, if this process retains it.
pub(super) fn retained(descriptions: &[(Vec<u8>, bool)], kinds: &[(String, bool)]) -> Option<Arc<AdmittedSchema>> {
    let sets = RETAINED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    sets.get(&fingerprint(descriptions, kinds))?
        .iter()
        .find(|known| known.kinds == kinds && known.descriptions == descriptions)
        .map(|known| Arc::clone(&known.schema))
}

/// Retain one completed admission with its two charges and the depth it was admitted under.
pub(super) fn retain(
    descriptions: &[(Vec<u8>, bool)],
    kinds: &[(String, bool)],
    dictionary: Vec<Stored>,
    entries: Vec<Vec<u8>>,
    (admission, encoding, depth): (Charge, Charge, u64),
) -> Arc<AdmittedSchema> {
    let schema = Arc::new(AdmittedSchema {
        dictionary,
        entries,
        admission,
        encoding,
        depth,
    });
    let mut sets = RETAINED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if sets.values().map(Vec::len).sum::<usize>() >= RETAINED_SETS {
        sets.clear();
    }
    let bucket = sets.entry(fingerprint(descriptions, kinds)).or_default();
    // Equal inputs charge the same, and an admission under depth D covers every write allowed at least D. The set
    // admitted under less depth covers every write the other covers, so it is the one retained.
    if let Some(known) = bucket
        .iter_mut()
        .find(|known| known.kinds == kinds && known.descriptions == descriptions)
    {
        if schema.depth < known.schema.depth {
            known.schema = Arc::clone(&schema);
        }
        return schema;
    }
    bucket.push(Retained {
        descriptions: descriptions.to_vec(),
        kinds: kinds.to_vec(),
        schema: Arc::clone(&schema),
    });
    schema
}

/// A cheap bucket key; equal inputs always share it, and the exact comparison settles every match.
fn fingerprint(descriptions: &[(Vec<u8>, bool)], kinds: &[(String, bool)]) -> Vec<u8> {
    let mut key = Vec::with_capacity(16 + descriptions.len() * (2 * FINGERPRINT_BYTES + 9));
    key.extend_from_slice(&(descriptions.len() as u64).to_le_bytes());
    key.extend_from_slice(&(kinds.len() as u64).to_le_bytes());
    for (encoded, inline) in descriptions {
        key.extend_from_slice(&(encoded.len() as u64).to_le_bytes());
        key.push(u8::from(*inline));
        key.extend_from_slice(&encoded[..encoded.len().min(FINGERPRINT_BYTES)]);
        key.extend_from_slice(&encoded[encoded.len().saturating_sub(FINGERPRINT_BYTES)..]);
    }
    key
}

/// Charge a step `covers` already proved fits; a successful step records no exhausted cause.
fn charge(budget: &mut Budget, charge: Charge) {
    let (bytes, elements) = budget.left();
    *budget = Budget::new(bytes - charge.bytes, elements - charge.elements, budget.depth());
}
