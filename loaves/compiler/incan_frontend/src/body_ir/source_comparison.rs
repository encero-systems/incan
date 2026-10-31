//! Body IR-only native ABI facts for checked standard comparison implementations.

use std::collections::BTreeSet;

use incan_semantics_core::body_ir::{Body, BodyIrModule, LocalOrigin};
use incan_semantics_core::{
    CanonicalSymbolId, IncanPrimitiveType, IncanType, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
};

/// Recognize a standard comparison slot by its checked provider identity, never an ambient method spelling.
pub(super) fn is_standard_slot(identity: &CanonicalSymbolId, name: &str) -> bool {
    if identity.kind != SemanticSourceTargetKind::Method
        || identity.namespace != SymbolNamespace::Member
        || identity.declaration_name != name
    {
        return false;
    }
    match &identity.origin {
        SymbolOrigin::Module(path) => path.iter().map(String::as_str).eq(["std", "derives", "comparison"]),
        SymbolOrigin::Package { library, module_path } => {
            library == "incan_stdlib_core" && module_path.iter().map(String::as_str).eq(["derives", "comparison"])
        }
        _ => false,
    }
}

/// Return published comparison defaults selected by the source modules' checked implementation records.
/// Native comparison bridges need the complete retained slot set, including defaults no source call names directly.
/// This grants no implementation from a matching spelling and supplies no replacement for an absent published body.
pub fn required_published_defaults(modules: &[BodyIrModule]) -> BTreeSet<CanonicalSymbolId> {
    modules
        .iter()
        .flat_map(|module| &module.trait_implementations)
        .filter(|implementation| implementation.implementation == implementation.method)
        .filter(|implementation| {
            matches!(implementation.method.origin, SymbolOrigin::Package { .. })
                && is_standard_slot(&implementation.method, &implementation.method.declaration_name)
        })
        .map(|implementation| implementation.method.clone())
        .collect()
}

/// Retain legacy's borrowed other operand only for a concrete nominal or enum body selected for standard Eq's
/// native slot. The checker and legacy tables remain untouched. An ordinary same-spelled member or incomplete signature
/// grants no ABI.
pub(super) fn retain_equality_borrows(module: &mut BodyIrModule) {
    for implementation in &module.trait_implementations {
        if !is_standard_slot(&implementation.method, "__eq__") || implementation.implementation == implementation.method
        {
            continue;
        }
        let nominal = module
            .nominal_declarations
            .iter()
            .find(|owner| owner.canonical == implementation.owner);
        let name = if let Some(owner) = nominal {
            if !matches!(
                owner.canonical.kind,
                SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class | SemanticSourceTargetKind::Newtype
            ) || !owner.type_parameters.is_empty()
            {
                continue;
            }
            &owner.name
        } else if let Some(owner) = module
            .enum_declarations
            .iter()
            .find(|owner| owner.canonical == implementation.owner)
        {
            if !owner.type_parameters.is_empty() {
                continue;
            }
            &owner.name
        } else {
            continue;
        };
        let Some(body) = module
            .bodies
            .iter_mut()
            .find(|body| body.canonical.as_ref() == Some(&implementation.implementation))
        else {
            continue;
        };
        if !retain_equality_body(body, &implementation.owner, name) {
            continue;
        }
        if let Some(owner) = module
            .nominal_declarations
            .iter_mut()
            .find(|owner| owner.canonical == implementation.owner)
        {
            owner.comparison_equality = Some(implementation.implementation.clone());
        }
    }
}

/// Apply a selected concrete standard equality contract to its own body. The caller has authenticated the standard
/// slot, checked owner shape and implementation identity; success separately authorizes the owner's equality facet.
/// No nominal inventory or other body is read or mutated here.
pub(super) fn retain_equality_body(body: &mut Body, owner: &CanonicalSymbolId, name: &str) -> bool {
    let Some(parameter) = body.params.get(1) else {
        return false;
    };
    let owned = parameter.ty.clone();
    let IncanType::Named(parameter_name) = &owned else {
        return false;
    };
    let selected = body.named_type_identities.get(parameter_name);
    let canonical_key = incan_semantics_core::body_ir::nominal_carrier_key(owner);
    let exact_owner = selected == Some(owner)
        || (selected.is_none() && (parameter_name == name || canonical_key.as_ref() == Some(parameter_name)));
    if !body.type_parameters.is_empty()
        || body.params.len() != 2
        || body.return_type != IncanType::Primitive(IncanPrimitiveType::Bool)
        || !exact_owner
        || body.params[1].mutable
        || !body
            .locals
            .first()
            .is_some_and(|local| matches!(local.origin, LocalOrigin::Receiver { mutable: false }))
    {
        return false;
    }
    let parameter = &mut body.params[1];
    let Some(local) = body.locals.get_mut(parameter.local.index()) else {
        return false;
    };
    if local.ty != owned || local.origin != LocalOrigin::Parameter {
        return false;
    }
    parameter.ty = IncanType::Ref(Box::new(owned));
    local.ty = parameter.ty.clone();
    true
}
