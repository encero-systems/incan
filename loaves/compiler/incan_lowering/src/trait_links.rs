//! Validation of trait implementation records whose trait another module, the standard library or an SDK declares.

use incan_semantics_core::body_ir::{BodyIrModule, TraitImplementation, loaf_item_path};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

use crate::nominal_frames::closed_arguments;

/// A generic owner's open slot or inherited default arguments, before concrete records are validated.
pub fn has_open_trait_arguments(implementation: &TraitImplementation) -> bool {
    let inherited: Vec<_> = implementation
        .type_arguments
        .iter()
        .map(|binding| binding.argument.clone())
        .collect();
    !closed_arguments(&implementation.slot_arguments) || !closed_arguments(&inherited)
}

/// Validate one trait implementation record against the current module slice and retained joined project authority.
pub fn validate_project_implementation(
    module: &BodyIrModule,
    implementation: &TraitImplementation,
    modules: &[BodyIrModule],
    retained: &BodyIrModule,
) -> Result<(), String> {
    if module.is_well_formed_trait_implementation(implementation) {
        return Ok(());
    }
    if !module.trait_arguments_are_owner_bound(implementation) {
        return Err("invalid Body IR trait implementation arguments".into());
    }
    if !fills_its_slot(implementation) {
        return Err("invalid Body IR trait implementation slot filling".into());
    }
    if !retains_owner(module, &implementation.owner) {
        return Err("invalid Body IR trait implementation owner".into());
    }
    let filled = &implementation.implementation;
    let retained_body = |identity| retains_body(modules, identity) || retained_body(retained, identity);
    if is_imported_slot(&implementation.method) {
        if implementation.implementation == implementation.method || retained_body(filled) {
            return Ok(());
        }
        return Err("invalid Body IR trait implementation without a retained declaration".into());
    }
    // A Rust trait has no declaring project module; the adopter's own adoption record names its slots instead.
    if adopted_rust_slot(module, implementation) {
        if retained_body(filled) {
            return Ok(());
        }
        return Err("invalid Body IR Rust trait implementation without a retained declaration".into());
    }
    if !declares_slot(modules, &implementation.method) && !retained_declares_slot(retained, &implementation.method) {
        return Err("invalid Body IR trait implementation without its declaring trait".into());
    }
    if !retained_body(filled) {
        return Err("invalid Body IR trait implementation without a retained declaration".into());
    }
    Ok(())
}

/// Whether a slot belongs to a standard module or an authenticated package, by its checked provider identity.
fn is_imported_slot(slot: &CanonicalSymbolId) -> bool {
    match &slot.origin {
        SymbolOrigin::Module(path) => path.first().is_some_and(|first| first == "std"),
        SymbolOrigin::Package { .. } => true,
        _ => false,
    }
}

/// Require a method slot filled by its default or checked own method, or a property filled by its own getter.
fn fills_its_slot(implementation: &TraitImplementation) -> bool {
    let slot = &implementation.method;
    let filled = &implementation.implementation;
    if slot.namespace != SymbolNamespace::Member {
        return false;
    }
    if slot.kind != SemanticSourceTargetKind::Method && slot.kind != SemanticSourceTargetKind::Property {
        return false;
    }
    if filled.kind != slot.kind {
        return false;
    }
    if filled == slot {
        return slot.kind == SemanticSourceTargetKind::Method;
    }
    if slot.kind == SemanticSourceTargetKind::Property && slot.declaration_name != filled.declaration_name {
        return false;
    }
    filled.origin == implementation.owner.origin && contains(&implementation.owner, filled)
}

/// Whether the adopter's own Rust trait adoption lists this slot; rustc checks the implementation itself.
fn adopted_rust_slot(module: &BodyIrModule, implementation: &TraitImplementation) -> bool {
    if !matches!(implementation.method.origin, SymbolOrigin::RustCrate(_)) {
        return false;
    }
    for declaration in &module.nominal_declarations {
        if declaration.canonical != implementation.owner {
            continue;
        }
        for adoption in &declaration.trait_adoptions {
            if adoption.trait_identity.origin != implementation.method.origin {
                continue;
            }
            if let Some(slots) = &adoption.methods
                && slots.contains(&implementation.method)
            {
                return true;
            }
        }
    }
    false
}

/// Whether this module retains the adopter's well-formed model, class or enum layout.
fn retains_owner(module: &BodyIrModule, owner: &CanonicalSymbolId) -> bool {
    module
        .nominal_declarations
        .iter()
        .any(|declaration| declaration.canonical == *owner && module.is_well_formed_nominal_declaration(declaration))
        || module.enum_declarations.iter().any(|declaration| {
            declaration.canonical == *owner && module.is_well_formed_native_enum_declaration(declaration)
        })
}

fn declares_slot(modules: &[BodyIrModule], slot: &CanonicalSymbolId) -> bool {
    modules.iter().any(|module| module_declares_slot(module, slot))
}

/// Whether one module declares, as its own trait, the trait whose span encloses the slot.
fn module_declares_slot(module: &BodyIrModule, slot: &CanonicalSymbolId) -> bool {
    module.trait_declarations.iter().any(|declaration| {
        declaration.origin == slot.origin
            && contains(declaration, slot)
            && module
                .declaration_id_for_canonical(
                    declaration,
                    SymbolNamespace::OrdinaryLexical,
                    SemanticSourceTargetKind::Trait,
                )
                .is_some()
    })
}

fn retained_declares_slot(retained: &BodyIrModule, slot: &CanonicalSymbolId) -> bool {
    retained
        .trait_declarations
        .iter()
        .any(|declaration| declaration.origin == slot.origin && contains(declaration, slot))
}

fn retains_body(modules: &[BodyIrModule], identity: &CanonicalSymbolId) -> bool {
    modules.iter().any(|module| module_retains_body(module, identity))
}

/// Whether one module retains a checked body or the exact compiled Loaf declaration that implements the slot.
fn module_retains_body(module: &BodyIrModule, identity: &CanonicalSymbolId) -> bool {
    if module
        .bodies
        .iter()
        .any(|body| body.canonical.as_ref() == Some(identity) && module.body_has_canonical_direct_call_id(body))
    {
        return true;
    }
    for declaration in &module.loaf_callables {
        if declaration.canonical != *identity {
            continue;
        }
        if let Some(path) = loaf_item_path(identity)
            && declaration.item == path
            && identity.namespace == SymbolNamespace::Member
            && identity.scope_discriminant.is_none()
            && (identity.kind == SemanticSourceTargetKind::Method
                || identity.kind == SemanticSourceTargetKind::Property)
        {
            return true;
        }
    }
    false
}

fn retained_body(retained: &BodyIrModule, identity: &CanonicalSymbolId) -> bool {
    retained
        .bodies
        .iter()
        .any(|body| body.canonical.as_ref() == Some(identity))
        || module_retains_body(retained, identity)
}

/// Whether the owner's declaration span encloses the member's; callers compare origins, as spans of files nest.
pub fn contains(owner: &CanonicalSymbolId, member: &CanonicalSymbolId) -> bool {
    owner.declaration_span.start <= member.declaration_span.start
        && member.declaration_span.end <= owner.declaration_span.end
}
