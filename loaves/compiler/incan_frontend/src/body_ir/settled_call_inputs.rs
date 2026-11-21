//! Project the argument ownership settled by the frontend onto actual authored call targets.
use super::{bir, shared_reads};
use incan_semantics_core::{
    CanonicalSymbolId, encode_incan_symbol_identity, semantic_digest::semantic_digest,
    stable_identity::StableDeclarationId,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A consumer reads the positional access contract of the declaration selected by the checker.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SettledCallRead {
    pub reader: String,
    pub target: String,
    pub digest: String,
}

/// Exact checked targets, indexed once for an activation. A stable locator collision keeps the physical
/// identity of each candidate so moving an ambiguous declaration can only cause a cache miss.
pub struct SettledCallInputs {
    by_owner: HashMap<CanonicalSymbolId, String>,
    contracts: BTreeMap<String, String>,
    ambiguous: BTreeSet<String>,
}

impl SettledCallInputs {
    pub fn from_modules(modules: &[bir::BodyIrModule]) -> Result<Self, String> {
        let mut entries = BTreeMap::new();
        for body in modules.iter().flat_map(|m| &m.bodies) {
            let Some(id) = &body.canonical else { continue };
            let proposed = StableDeclarationId::from_canonical(id, None, None)
                .map(|s| s.render_compact())
                .unwrap_or_else(|| encode_incan_symbol_identity(id));
            let parameters = body
                .params
                .iter()
                .map(|p| (p.input_access, p.native_owned, p.mutable))
                .collect::<Vec<_>>();
            let digest = semantic_digest(&parameters).map_err(|e| e.to_string())?;
            entries
                .entry(proposed)
                .or_insert_with(BTreeSet::new)
                .insert((id.clone(), digest));
        }
        let mut by_owner = HashMap::new();
        let mut contracts = BTreeMap::new();
        let mut ambiguous = BTreeSet::new();
        for (name, targets) in entries {
            let multiple = targets.len() > 1;
            for (id, digest) in targets {
                let key = if multiple {
                    encode_incan_symbol_identity(&id)
                } else {
                    name.clone()
                };
                if contracts
                    .insert(key.clone(), digest.clone())
                    .is_some_and(|prior| prior != digest)
                {
                    ambiguous.insert(key.clone());
                }
                by_owner.insert(id, key);
            }
        }
        Ok(Self {
            by_owner,
            contracts,
            ambiguous,
        })
    }
    /// The checked target and its current parameter contract, independent of any caller's stored witness.
    pub fn supplied_by(&self, owner: &CanonicalSymbolId) -> Option<(&str, &str)> {
        let target = self.by_owner.get(owner)?;
        Some((target, self.contract(target)?))
    }

    pub fn contract(&self, target: &str) -> Option<&str> {
        if self.ambiguous.contains(target) {
            return None;
        }
        self.contracts.get(target).map(String::as_str)
    }

    pub fn matches(&self, read: &SettledCallRead) -> bool {
        !self.ambiguous.contains(&read.target) && self.contracts.get(&read.target) == Some(&read.digest)
    }

    pub fn reads(&self, module: &mut bir::BodyIrModule) -> Vec<SettledCallRead> {
        let mut result = BTreeSet::new();
        for body in &mut module.bodies {
            let reader = body.direct_call_id.path();
            shared_reads::walk_calls_mut(&mut body.block.stmts, &mut |statement| {
                let bir::StatementKind::Call { callee, .. } = &statement.kind else {
                    return;
                };
                let owner = match callee {
                    bir::Callee::Function(bir::CallableTarget::Named(target)) => target.canonical.as_ref(),
                    bir::Callee::Method(target) => target.canonical.as_ref(),
                    _ => None,
                };
                if let Some(target) = owner.and_then(|owner| self.by_owner.get(owner)) {
                    if let Some(digest) = self.contracts.get(target) {
                        result.insert(SettledCallRead {
                            reader: reader.to_owned(),
                            target: target.clone(),
                            digest: digest.clone(),
                        });
                    }
                }
            });
        }
        result.into_iter().collect()
    }
}

/// Before any per-owner read witness has been staged, a partial restore can still keep a body
/// without project callsites. The source checker and its settled semantic fields remain admissible otherwise.
pub fn needs_call_witness(body: &mut bir::Body) -> bool {
    let mut needed = false;
    shared_reads::walk_calls_mut(&mut body.block.stmts, &mut |statement| {
        let bir::StatementKind::Call { callee, .. } = &statement.kind else {
            return;
        };
        let owner = match callee {
            bir::Callee::Function(bir::CallableTarget::Named(t)) => t.canonical.as_ref(),
            bir::Callee::Method(t) => t.canonical.as_ref(),
            _ => None,
        };
        needed |= owner.is_some_and(|id| {
            matches!(
                id.origin,
                incan_semantics_core::SymbolOrigin::Module(_) | incan_semantics_core::SymbolOrigin::Package { .. }
            )
        });
    });
    needed
}
