//! Members of imported nominals that a body record may name.

use super::*;
use crate::symbols::{SymbolKind, TypeInfo};
use incan_semantics_core::SymbolOrigin;

/// Resolve recorded member references to the current declaration anchors carried by their imported nominal.
/// Package methods and variants have their own executable anchors, distinct from the enclosing nominal.
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
        let (fields, properties, methods) = match &symbol.kind {
            SymbolKind::Type(TypeInfo::Model(info)) => (Some(&info.fields), Some(&info.properties), &info.methods),
            SymbolKind::Type(TypeInfo::Class(info)) => (Some(&info.fields), Some(&info.properties), &info.methods),
            SymbolKind::Type(TypeInfo::Newtype(info)) => (None, None, &info.methods),
            SymbolKind::Trait(info) => (None, Some(&info.properties), &info.methods),
            SymbolKind::Type(TypeInfo::Enum(info)) => {
                for variant in info.variant_identities.values() {
                    record(variant, SemanticSourceTargetKind::Variant);
                }
                (None, None, &info.methods)
            }
            _ => continue,
        };
        for field in fields
            .into_iter()
            .flat_map(|fields| fields.values())
            .filter_map(|field| field.identity.as_ref())
        {
            record(field, SemanticSourceTargetKind::Field);
        }
        for property in properties
            .into_iter()
            .flat_map(|properties| properties.values())
            .filter_map(|property| property.identity.as_ref())
        {
            record(property, SemanticSourceTargetKind::Property);
        }
        for method in methods.values().filter_map(|method| method.identity.as_ref()) {
            record(method, SemanticSourceTargetKind::Method);
        }
    }
    identities
}
