//! Encoded D descriptions for the compiler's fixed description sets, kept for the life of the process.
//!
//! The compiler writes thousands of single-row envelopes against a few description sets that its schemas rebuild on
//! every call. A set's encoding depends on its metadata and modes alone, so each distinct set is encoded once and
//! every later write of an equal set reuses the same bytes, as `PreparedWireReader` already does for reads.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use internal_models_bindings::{Description, DescriptionMode, create_budget, crumb_message, encode_metadata};

/// Distinct description sets one process retains; a set past the bound is encoded on each write.
const RETAINED_SETS: usize = 256;

/// One set's encoded descriptions, each with its inline (true) or reference (false) mode.
pub(crate) type EncodedSet = Arc<Vec<(Vec<u8>, bool)>>;

static ENCODED: LazyLock<Mutex<HashMap<Vec<Description>, EncodedSet>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The encoded descriptions of `descriptions`, encoding an equal set only the first time this process writes it.
pub(crate) fn encoded_descriptions(descriptions: &[Description]) -> Result<EncodedSet, String> {
    if let Some(known) = ENCODED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(descriptions)
    {
        return Ok(Arc::clone(known));
    }
    // The writer encodes descriptions under no allowance of the caller's; a refused encoding is never retained.
    let mut boundary = create_budget(i64::MAX, i64::MAX, i64::MAX).map_err(|error| crumb_message(&error))?;
    let encoded = descriptions
        .iter()
        .map(|description| {
            let bytes = encode_metadata(&description.metadata, &mut boundary).map_err(|error| crumb_message(&error))?;
            Ok((bytes, description.mode == DescriptionMode::Inline))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let encoded = Arc::new(encoded);
    let mut sets = ENCODED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if sets.len() < RETAINED_SETS {
        sets.insert(descriptions.to_vec(), Arc::clone(&encoded));
    }
    Ok(encoded)
}
