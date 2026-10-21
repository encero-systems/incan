//! Rust values a program holds (DD-0006).
//!
//! A checked Rust type with no Incan meaning (`std::sync::Arc<T>`, `core::str::error::Utf8Error`) becomes the opaque
//! nominal of its Rust item, generic over its checked arguments and named by the item's path. Metadata defaults
//! complete omitted owner arguments at this boundary. Instantiations close like
//! any generic nominal, and the driver stores each one as the item itself; the items a compilation holds are read from
//! its laid-out Body IR types (`incan_semantics_core::body_ir::held_rust_items`). Rust types with an Incan meaning
//! (collections, strings, options) never reach this module: the checker already resolved them to their Incan types.

use super::{IncanType, TypeCheckInfo, bir};

/// Whether an owned payload contains a retained native owner, including a tuple or collection that contains one.
/// References alone grant no ownership transfer; a whole fresh Option/Result payload supplies that proof separately.
pub(super) fn contains_native_owner(ty: &IncanType) -> bool {
    match ty {
        IncanType::Named(name) => bir::rust_item_identity(name).is_some(),
        IncanType::Generic { base, args } => {
            bir::rust_item_identity(base).is_some() || args.iter().any(contains_native_owner)
        }
        IncanType::Tuple(fields) => fields.iter().any(contains_native_owner),
        _ => false,
    }
}

/// Retain selected public native layouts using their checked field identities and type binders. The adapter verifies
/// the complete instantiated foreign layout; these facts never authorize a guessed partial field projection.
pub(super) fn declarations(facts: &TypeCheckInfo) -> Vec<bir::NominalDeclaration> {
    let mut layouts: Vec<_> = facts.rust.checked_layouts.iter().collect();
    layouts.sort_by_key(|(path, _)| *path);
    layouts
        .into_iter()
        .filter_map(|(path, layout)| {
            let identity = bir::rust_item_identity(path)?;
            let mut declaration = bir::native_opaque_nominal(&identity, layout.type_parameters.len())?;
            declaration.type_parameters = layout.type_parameters.clone();
            declaration.fields = layout.fields.iter().filter_map(|field| field.name.clone()).collect();
            declaration.field_identities = layout.field_identities.clone();
            declaration.field_types = layout
                .fields
                .iter()
                .map(|field| super::parameter_types::checked_parameter_type(&field.ty, facts, &layout.type_parameters))
                .collect();
            declaration.field_public = vec![true; declaration.fields.len()];
            Some(declaration)
        })
        .collect()
}

/// Project complete checked native enum templates through the same storage boundary used by native call signatures.
pub(super) fn enum_declarations(facts: &TypeCheckInfo) -> Vec<bir::EnumDeclaration> {
    let mut layouts = facts.rust.checked_enums.iter().collect::<Vec<_>>();
    layouts.sort_by_key(|(path, _)| *path);
    layouts
        .into_iter()
        .filter_map(|(path, layout)| {
            let variants = layout
                .variants
                .iter()
                .map(|(name, fields)| {
                    (
                        name.clone(),
                        fields
                            .iter()
                            .map(|field| {
                                super::parameter_types::checked_parameter_type(field, facts, &layout.type_parameters)
                            })
                            .collect(),
                    )
                })
                .collect();
            bir::native_enum_declaration(path, layout.type_parameters.clone(), variants)
        })
        .collect()
}

/// Copy the accepting checker's structural storage. Bare native item paths already carry their complete identity;
/// displays with arguments or references require a retained checker fact and never start another parser.
pub(super) fn project(display: &str, facts: &TypeCheckInfo) -> Option<IncanType> {
    if let Some(checked) = facts.rust.storage_types.get(display) {
        return Some(checked.clone());
    }
    let path = facts
        .rust
        .item_public_paths
        .get(display)
        .map_or(display, String::as_str);
    let identity = bir::rust_item_identity(path)?;
    let name = bir::native_opaque_name(&identity)?;
    Some(complete_owner(name, Vec::new(), facts))
}

/// Complete opaque native storage arguments from checked declaration defaults, substituting earlier binders. Rustc
/// elides defaults in displays; a public field template can expose them again, so written arity is not storage arity.
pub(super) fn complete_owner(base: String, mut args: Vec<IncanType>, facts: &TypeCheckInfo) -> IncanType {
    if let Some(parameters) = facts.rust.owner_parameters.get(&base)
        && args.len() <= parameters.len()
        && parameters[args.len()..].iter().all(|(_, default)| default.is_some())
    {
        let names = parameters.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>();
        for (_, default) in &parameters[args.len()..] {
            let Some(default) = default else { break };
            let template = super::parameter_types::checked_parameter_type(default, facts, &names);
            args.push(super::enum_types::close_payload_type(&template, &names, &args));
        }
    }
    if args.is_empty() {
        IncanType::Named(base)
    } else {
        IncanType::Generic { base, args }
    }
}
