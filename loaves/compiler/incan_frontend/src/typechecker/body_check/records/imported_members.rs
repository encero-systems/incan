//! Members of imported nominals that a body record may name.

use super::*;
use crate::symbols::{SymbolKind, TypeInfo};
use incan_semantics_core::SymbolOrigin;

/// The checked fields of every imported model and class, each under its stable identity within its nominal. A
/// constructor of an imported nominal names the field each argument initializes, so its caller's record names them.
pub(super) fn field_identities(checker: &TypeChecker) -> Vec<(String, CanonicalSymbolId)> {
    let mut identities = Vec::new();
    for (index, symbol) in checker.symbols.all_symbols().iter().enumerate() {
        let Some(owner) = checker.symbols.identity_of(index) else {
            continue;
        };
        if !matches!(owner.origin, SymbolOrigin::Module(_) | SymbolOrigin::Package { .. }) {
            continue;
        }
        let fields = match &symbol.kind {
            SymbolKind::Type(TypeInfo::Model(info)) => &info.fields,
            SymbolKind::Type(TypeInfo::Class(info)) => &info.fields,
            _ => continue,
        };
        let Some(stable_owner) = StableDeclarationId::from_canonical(owner, None, None) else {
            continue;
        };
        for field in fields.values().filter_map(|field| field.identity.as_ref()) {
            let context = StableDeclarationContext {
                owner: stable_owner.clone(),
                binding_ordinal: 0,
            };
            if field.kind == SemanticSourceTargetKind::Field
                && let Some(stable) = StableDeclarationId::from_canonical(field, None, Some(context))
            {
                identities.push((stable.render_compact(), field.clone()));
            }
        }
    }
    identities
}
