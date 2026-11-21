//! Program-local positions of bodies selected by complete checker identity.

use std::collections::BTreeMap;
use std::sync::Arc;

use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::body_ir::BodyIrModule;

/// Shared immutable candidate positions across ordinary, instantiated, and lifted frames; duplicates keep source
/// order.
#[derive(Clone, Debug, Default)]
pub struct BodyIndex {
    pub entries: Arc<BTreeMap<CanonicalSymbolId, Vec<usize>>>,
    pub count: usize,
}

impl BodyIndex {
    /// Only the selected identity's positions, leaving ambiguity and receiver admission to each caller.
    pub fn candidates(&self, identity: &CanonicalSymbolId) -> Vec<usize> {
        self.entries.get(identity).cloned().unwrap_or_default()
    }

    /// A position only when exactly one body carries the complete checker identity.
    pub fn unique(&self, identity: &CanonicalSymbolId) -> Option<usize> {
        match self.entries.get(identity) {
            Some(indices) if indices.len() == 1 => Some(indices[0]),
            _ => None,
        }
    }
}

/// Read bodies by reference once after specialization, keeping canonical keys without copying statements.
pub fn body_index(module: &BodyIrModule) -> BodyIndex {
    let mut entries: BTreeMap<CanonicalSymbolId, Vec<usize>> = BTreeMap::new();
    for (index, body) in module.bodies.iter().enumerate() {
        if let Some(identity) = &body.canonical {
            entries.entry(identity.clone()).or_default().push(index);
        }
    }
    BodyIndex {
        entries: Arc::new(entries),
        count: module.bodies.len(),
    }
}

/// Index bodies appended since the retained context generation without revisiting earlier executable frames.
pub fn extend_body_index(index: &mut BodyIndex, module: &BodyIrModule) -> Result<(), String> {
    if index.count > module.bodies.len() {
        return Err("invalid Body IR body index generation".into());
    }
    for position in index.count..module.bodies.len() {
        if let Some(identity) = &module.bodies[position].canonical {
            Arc::make_mut(&mut index.entries)
                .entry(identity.clone())
                .or_default()
                .push(position);
        }
    }
    index.count = module.bodies.len();
    Ok(())
}

/// An empty lookup for synthetic external-only frames that cannot select source bodies.
pub fn empty_body_index() -> BodyIndex {
    BodyIndex::default()
}
