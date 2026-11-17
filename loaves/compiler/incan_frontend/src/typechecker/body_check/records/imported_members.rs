//! Members of imported nominals that a body record may name.

use super::*;
use crate::symbols::{SymbolKind, TypeInfo};
use incan_semantics_core::SymbolOrigin;

/// Resolve recorded member references to the current declaration anchors carried by their imported nominal.
/// A package variant's own anchor identifies its executable representation.
pub(super) fn identities(checker: &TypeChecker) -> Vec<(String, CanonicalSymbolId)> {
    let mut identities = Vec::new();
    for (index, symbol) in checker.symbols.all_symbols().iter().enumerate() {
        let Some(owner) = checker.symbols.identity_of(index) else {
            continue;
        };
        if !matches!(owner.origin, SymbolOrigin::Module(_) | SymbolOrigin::Package { .. }) {
            continue;
        }
        let mut record = |member: &CanonicalSymbolId, kind: SemanticSourceTargetKind| {
            if member.kind == kind
                && let Ok(identity) = stable_member_identity(member, owner, 0)
            {
                identities.push((identity, member.clone()));
            }
        };
        let fields = match &symbol.kind {
            SymbolKind::Type(TypeInfo::Model(info)) => &info.fields,
            SymbolKind::Type(TypeInfo::Class(info)) => &info.fields,
            SymbolKind::Type(TypeInfo::Enum(info)) => {
                for variant in info.variant_identities.values() {
                    record(variant, SemanticSourceTargetKind::Variant);
                }
                continue;
            }
            _ => continue,
        };
        for field in fields.values().filter_map(|field| field.identity.as_ref()) {
            record(field, SemanticSourceTargetKind::Field);
        }
    }
    identities
}
