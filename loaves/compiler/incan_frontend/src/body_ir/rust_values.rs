//! Rust values a program holds (DD-0006).
//!
//! A checked Rust type with no Incan meaning (`std::sync::Arc<T>`, `core::str::error::Utf8Error`) becomes the opaque
//! nominal of its Rust item, generic over the arguments and metadata defaults retained by its accepting checker.
//! Instantiations close like
//! any generic nominal, and the driver stores each one as the item itself; the items a compilation holds are read from
//! its laid-out Body IR types (`incan_semantics_core::body_ir::held_rust_items`). Native types with an admitted Incan
//! meaning (collections, strings, options) already have that checked storage; copying it never reclassifies the type.

use super::{IncanType, TypeCheckInfo, bir};

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
            if layout.is_copy {
                declaration.derives.push("Copy".into());
            }
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
                .map(|(name, fields, _)| {
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
    if let Some(checked) = facts
        .rust
        .storage_types
        .get(&IncanType::RustInteropPath(display.to_owned()))
    {
        return Some(checked.clone());
    }
    let path = facts
        .rust
        .item_public_paths
        .get(display)
        .map_or(display, String::as_str);
    let identity = bir::rust_item_identity(path)?;
    let name = bir::native_opaque_name(&identity)?;
    Some(IncanType::Named(name))
}
