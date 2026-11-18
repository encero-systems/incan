//! Private editable namespace snapshots share each checked nominal environment across its bodies.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::CanonicalSymbolId;
use crate::body_ir::{BodyIrModule, SharedIdentities, share_equal_identity_maps};

use super::{ExecutableRepresentationError, decode_module, encode_module, malformed};

// This prefix belongs to the editable namespace record. Published Loaf representations use their own version.
const MAGIC: &[u8] = b"INCNNS\x01";

/// Encode an editable module once with an ordered directory of its distinct checked nominal environments.
/// On return every body still owns its original bindings, including when serialization fails.
pub fn encode_namespace_module(module: &mut BodyIrModule) -> Result<Vec<u8>, ExecutableRepresentationError> {
    share_equal_identity_maps(&mut module.bodies);
    let mut by_address = HashMap::new();
    let mut scopes = Vec::new();
    let mut slots = Vec::with_capacity(module.bodies.len());
    let empty = SharedIdentities::default();
    for body in &mut module.bodies {
        let scope = std::mem::replace(&mut body.named_type_identities, Arc::clone(&empty));
        let index = *by_address.entry(Arc::as_ptr(&scope)).or_insert_with(|| {
            let index = scopes.len();
            scopes.push(scope);
            index
        });
        slots.push(index);
    }
    let encoded = encode_module(module);
    for (body, &index) in module.bodies.iter_mut().zip(&slots) {
        body.named_type_identities = Arc::clone(&scopes[index]);
    }
    let encoded = encoded?;
    let maps: Vec<_> = scopes.iter().map(Arc::as_ref).collect();
    let mut output = MAGIC.to_vec();
    let directory = postcard::to_allocvec(&(maps, slots)).map_err(|error| malformed(error.to_string()))?;
    output.reserve(directory.len() + encoded.len());
    output.extend_from_slice(&directory);
    output.extend_from_slice(&encoded);
    Ok(output)
}

/// Recover exactly one identity environment for each body. Earlier snapshots carry their maps inline.
pub fn decode_namespace_module(bytes: &[u8]) -> Result<BodyIrModule, ExecutableRepresentationError> {
    let Some(bytes) = bytes.strip_prefix(MAGIC) else {
        return decode_module(bytes);
    };
    type ScopeDirectory = (Vec<BTreeMap<String, CanonicalSymbolId>>, Vec<usize>);
    let ((maps, slots), frame) = postcard::take_from_bytes::<ScopeDirectory>(bytes)
        .map_err(|error| malformed(format!("invalid namespace scope directory: {error}")))?;
    let mut module = decode_module(frame)?;
    if slots.len() != module.bodies.len() {
        return Err(malformed("namespace scope count differs from its body count"));
    }
    let scopes: Vec<_> = maps.into_iter().map(Arc::new).collect();
    let mut used = 0;
    for (body, slot) in module.bodies.iter_mut().zip(slots) {
        let Some(scope) = scopes.get(slot) else {
            return Err(malformed("namespace body names an unknown nominal scope"));
        };
        if slot > used || !body.named_type_identities.is_empty() {
            return Err(malformed("namespace nominal scopes have conflicting ownership"));
        }
        if slot == used {
            used += 1;
        }
        body.named_type_identities = Arc::clone(scope);
    }
    if used != scopes.len() {
        return Err(malformed("namespace scope directory contains unreferenced values"));
    }
    Ok(module)
}
