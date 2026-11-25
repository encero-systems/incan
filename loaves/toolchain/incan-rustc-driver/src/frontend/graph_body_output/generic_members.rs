//! Select the existing checked member authority for eligible generic lowering requests.

use super::{BodyIrModule, CanonicalSymbolId, CompilerNodeId, GraphBodyOutput, ParsedModule, Path};
use crate::frontend::CompilationAnalysis;
use incan_semantics_core::SemanticSourceTargetKind;
use std::collections::BTreeMap;

impl GraphBodyOutput {
    /// Carry checker-qualified member names only for modules defining generic methods.
    ///
    /// Board 3.1 uses this private adapter until lowering can consume the checked interface directly.
    /// A restored module uses the interface from the same admission that supplied its Body IR.
    pub(in crate::frontend) fn selected_generic_members(
        &self,
        modules: &[ParsedModule],
        bodies: &[BodyIrModule],
        analysis: &CompilationAnalysis,
        witnessed: &[(CompilerNodeId, String)],
    ) -> Result<BTreeMap<CanonicalSymbolId, String>, String> {
        if !self.enabled() {
            return Ok(BTreeMap::new());
        }
        let fresh: BTreeMap<_, _> = self
            .namespaces
            .iter()
            .map(|namespace| (namespace.module.as_str(), &namespace.member_owners))
            .collect();
        let mut selected = BTreeMap::new();
        for (module, lowered) in modules.iter().zip(bodies) {
            let mut candidates = lowered
                .bodies
                .iter()
                .filter_map(|body| {
                    let id = body.canonical.as_ref()?;
                    (id.kind == SemanticSourceTargetKind::Method
                        && !body.type_parameters.is_empty()
                        && witnessed
                            .binary_search_by(|(id, _)| id.cmp(&body.direct_call_id))
                            .is_ok())
                    .then_some(id)
                })
                .peekable();
            if candidates.peek().is_none() {
                continue;
            }
            let Some(source) = module
                .file_path
                .strip_prefix(&self.project_root)
                .ok()
                .and_then(Path::to_str)
            else {
                continue;
            };
            let restored;
            let names = if let Some(names) = fresh.get(source) {
                *names
            } else if let Some(interface) = analysis.lowering_namespace_interface(source) {
                restored = interface.lowering_member_owners()?;
                &restored
            } else {
                continue;
            };
            for id in candidates {
                if let Some(stable) = names.get(id) {
                    selected.insert(id.clone(), stable.clone());
                }
            }
        }
        Ok(selected)
    }
}
