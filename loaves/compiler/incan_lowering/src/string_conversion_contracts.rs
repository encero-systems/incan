//! Consumer checks for one compilation's selected source conversion contract and exact raw chains.

use incan_semantics_core::body_ir::{BodyIrModule, StringConversionContract, loaf_item_path};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

/// Only this invocation's entry can supply a fresh import union; package and dependency fragments supply none.
pub fn validate_conversion_placement(modules: &[BodyIrModule], entry: usize) -> Result<(), String> {
    for (index, module) in modules.iter().enumerate() {
        if index == entry {
            continue;
        }
        if !matches!(module.string_conversion_contract, StringConversionContract::Unselected) {
            return Err("unsupported Body IR misplaced string conversion contract".into());
        }
    }
    Ok(())
}

/// Absent and inactive states grant no conversion; a selected source trait keeps its encoded canonical identity.
pub fn conversion_trait(module: &BodyIrModule) -> Result<String, String> {
    let selection = match &module.string_conversion_contract {
        StringConversionContract::Unselected | StringConversionContract::Inactive => return Ok(String::new()),
        StringConversionContract::Selected(selection) => selection,
    };
    let refused = || "unsupported Body IR string conversion contract identity".to_owned();
    let owner = &selection.trait_identity;
    let method = &selection.method_identity;
    if !is_conversion_trait(owner) || !is_conversion_method(method) || !method_belongs_to_owner(method, owner) {
        return Err(refused());
    }
    if owner.scope_discriminant.is_some() || method.scope_discriminant.is_some() {
        return Err(refused());
    }
    match &owner.origin {
        SymbolOrigin::Package { library, module_path } => {
            if library != "incan_stdlib_primitives" || module_path != &["traits", "convert"] {
                return Err(refused());
            }
            loaf_item_path(owner).ok_or_else(|| "unsupported Body IR string conversion contract item".to_owned())
        }
        _ => Err(refused()),
    }
}

/// The selected lexical TryFrom trait rather than a same-name value or member.
fn is_conversion_trait(identity: &CanonicalSymbolId) -> bool {
    identity.kind == SemanticSourceTargetKind::Trait
        && identity.namespace == SymbolNamespace::OrdinaryLexical
        && identity.declaration_name == "TryFrom"
}

/// The selected member method, not a free function or differently named declaration.
fn is_conversion_method(identity: &CanonicalSymbolId) -> bool {
    identity.kind == SemanticSourceTargetKind::Method
        && identity.namespace == SymbolNamespace::Member
        && identity.declaration_name == "try_from"
}

/// Keep the method's origin and complete source span inside the selected trait's declaration.
fn method_belongs_to_owner(method: &CanonicalSymbolId, owner: &CanonicalSymbolId) -> bool {
    method.origin == owner.origin
        && method.declaration_span.start >= owner.declaration_span.start
        && method.declaration_span.end <= owner.declaration_span.end
}
