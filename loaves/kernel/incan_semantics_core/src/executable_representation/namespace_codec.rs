//! Editable namespace snapshots share each checked nominal environment across its owners.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::CanonicalSymbolId;
use crate::body_ir::{BodyIrModule, SharedIdentities, share_equal_identity_maps};

use super::{ExecutableRepresentationError, decode_module, encode_module, malformed};

// The editable namespace profile is independent of the published Loaf representation.
const BODY_SCOPES: &[u8] = b"INCNNS\x01";
const OWNER_SCOPES: &[u8] = b"INCNNS\x02";

type Bindings = BTreeMap<String, CanonicalSymbolId>;
type EncodedScopes = (Vec<Bindings>, Vec<usize>);

/// Encode each distinct owner scope once. The rest of the module uses its ordinary executable encoding.
/// On return the caller's module has the same bindings, including when encoding fails.
pub fn encode_namespace_module(module: &mut BodyIrModule) -> Result<Vec<u8>, ExecutableRepresentationError> {
    let mut scopes = ScopeDirectory::default();
    let empty = SharedIdentities::default();
    let slots: Vec<_> = owner_scopes(module)
        .map(|scope| scopes.add(std::mem::replace(scope, Arc::clone(&empty))))
        .collect();
    let encoded = encode_module(module);
    for (scope, &slot) in owner_scopes(module).zip(&slots) {
        *scope = Arc::clone(&scopes.maps[slot]);
    }
    let encoded = encoded?;
    let maps: Vec<_> = scopes.maps.iter().map(Arc::as_ref).collect();
    let mut output = OWNER_SCOPES.to_vec();
    let directory = postcard::to_allocvec(&(maps, slots)).map_err(|error| malformed(error.to_string()))?;
    output.reserve(directory.len() + encoded.len());
    output.extend_from_slice(&directory);
    output.extend_from_slice(&encoded);
    Ok(output)
}

/// Recover each owner scope by reference. Accept the earlier whole-module and body-only directory formats.
pub fn decode_namespace_module(bytes: &[u8]) -> Result<BodyIrModule, ExecutableRepresentationError> {
    let (bytes, include_declarations) = if let Some(frame) = bytes.strip_prefix(OWNER_SCOPES) {
        (frame, true)
    } else if let Some(frame) = bytes.strip_prefix(BODY_SCOPES) {
        (frame, false)
    } else {
        let mut module = decode_module(bytes)?;
        share_equal_identity_maps(&mut module.bodies);
        return Ok(module);
    };
    let ((maps, slots), frame) = postcard::take_from_bytes::<EncodedScopes>(bytes)
        .map_err(|error| malformed(format!("invalid namespace scope directory: {error}")))?;
    let mut module = decode_module(frame)?;
    let expected = module.bodies.len()
        + if include_declarations {
            module.nominal_declarations.len() + module.enum_declarations.len() + module.type_alias_declarations.len()
        } else {
            0
        };
    if slots.len() != expected {
        return Err(malformed("namespace scope count differs from its owner count"));
    }
    let maps: Vec<_> = maps.into_iter().map(Arc::new).collect();
    let mut used = 0;
    for (scope, slot) in owner_scopes(&mut module).zip(slots) {
        let Some(map) = maps.get(slot) else {
            return Err(malformed("namespace owner names an unknown nominal scope"));
        };
        if slot > used || !scope.is_empty() {
            return Err(malformed("namespace nominal scopes have conflicting ownership"));
        }
        if slot == used {
            used += 1;
        }
        *scope = Arc::clone(map);
    }
    if used != maps.len() {
        return Err(malformed("namespace scope directory contains unreferenced values"));
    }
    Ok(module)
}

/// Visit exactly the directory's ordered owners. Older directories ended after the final body.
fn owner_scopes(module: &mut BodyIrModule) -> impl Iterator<Item = &mut SharedIdentities> {
    module
        .bodies
        .iter_mut()
        .map(|body| &mut body.named_type_identities)
        .chain(
            module
                .nominal_declarations
                .iter_mut()
                .map(|layout| &mut layout.named_type_identities),
        )
        .chain(
            module
                .enum_declarations
                .iter_mut()
                .map(|layout| &mut layout.named_type_identities),
        )
        .chain(
            module
                .type_alias_declarations
                .iter_mut()
                .map(|alias| &mut alias.named_type_identities),
        )
}

/// First-use order defines the wire slots. An address match needs no content hash or map comparison.
#[derive(Default)]
struct ScopeDirectory {
    maps: Vec<SharedIdentities>,
    by_address: HashMap<*const Bindings, usize>,
    by_hash: HashMap<u64, Vec<usize>>,
}

impl ScopeDirectory {
    /// Move one scope into the directory or reuse the slot of its exact equal. During encoding all scope
    /// allocations already exist, so discarded duplicate addresses cannot be reissued to new scopes.
    fn add(&mut self, scope: SharedIdentities) -> usize {
        let address = Arc::as_ptr(&scope);
        if let Some(&slot) = self.by_address.get(&address) {
            return slot;
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        scope.as_ref().hash(&mut hasher);
        let candidates = self.by_hash.entry(hasher.finish()).or_default();
        let slot = candidates
            .iter()
            .copied()
            .find(|&slot| self.maps[slot] == scope)
            .unwrap_or_else(|| {
                let slot = self.maps.len();
                self.maps.push(scope);
                candidates.push(slot);
                slot
            });
        self.by_address.insert(address, slot);
        slot
    }
}
