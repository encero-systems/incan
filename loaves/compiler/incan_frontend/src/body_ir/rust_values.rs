//! Rust values a program holds (DD-0006).
//!
//! A checked Rust type with no Incan meaning (`std::sync::Arc<T>`, `core::str::error::Utf8Error`) becomes the opaque
//! nominal of its Rust item, generic over the written arguments and named by the item's path. Instantiations close like
//! any generic nominal, and the driver stores each one as the item itself; the items a compilation holds are read from
//! its laid-out Body IR types (`incan_semantics_core::body_ir::held_rust_items`). Rust types with an Incan meaning
//! (collections, strings, options) never reach this module: the checker already resolved them to their Incan types.

use incan_semantics_core::CanonicalSymbolId;

use super::{IncanType, TypeCheckInfo, bir, semantic_type_from_resolved};

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

/// Project a Rust type display onto the opaque nominal of its item, or `None` when the display names no Rust item
/// (`dyn` objects, references and function types keep their Rust spelling and refuse where they are laid out).
pub(super) fn project(display: &str, facts: &TypeCheckInfo) -> Option<IncanType> {
    if let Some(checked) = checked_name(display.trim(), facts) {
        return Some(checked);
    }
    if display.contains('<') {
        let resolved = crate::typechecker::TypeChecker::new().resolved_type_from_rust_display(display);
        if !matches!(resolved, crate::symbols::ResolvedType::RustPath(_)) {
            return Some(super::runtime_nominals::project_type(
                semantic_type_from_resolved(&resolved),
                facts,
            ));
        }
    }
    if let Some((base, inner)) = display.trim().split_once('<')
        && let Some(IncanType::Named(base)) = checked_name(base, facts)
    {
        let args = incan_lang::interop::split_top_level_rust_args(inner.strip_suffix('>')?)
            .into_iter()
            .map(|argument| argument_type(argument, facts))
            .collect();
        return Some(IncanType::Generic { base, args });
    }
    let (identity, arguments) = rust_item(display, facts)?;
    let name = bir::native_opaque_name(&identity)?;
    if arguments.is_empty() {
        return Some(IncanType::Named(name));
    }
    let args = arguments
        .into_iter()
        .map(|argument| argument_type(argument, facts))
        .collect();
    Some(IncanType::Generic { base: name, args })
}

/// The Rust item a display names with its top-level argument displays, or `None` for a display that is not a path.
///
/// The item is keyed on its public path whichever spelling the display uses (an import or a definition path), so one
/// Rust item has one identity and the driver resolves it through public modules.
fn rust_item<'a>(display: &'a str, facts: &TypeCheckInfo) -> Option<(CanonicalSymbolId, Vec<&'a str>)> {
    let display = display.trim().trim_start_matches("::");
    let (path, arguments) = match display.split_once('<') {
        Some((path, rest)) => (
            path.trim(),
            incan_lang::interop::split_top_level_rust_args(rest.strip_suffix('>')?),
        ),
        None => (display, Vec::new()),
    };
    // A builtin's carrier and an SDK runtime surface already have their own storage; they are never a second layout.
    if incan_semantics_core::RuntimeBuiltinType::from_carrier_path(path).is_some()
        || incan_lang::lang::surface::types::from_runtime_rust_path(path).is_some()
    {
        return None;
    }
    let path = facts.rust.item_public_paths.get(path).map_or(path, String::as_str);
    let identity = bir::rust_item_identity(path)?;
    bir::native_opaque_nominal(&identity, arguments.len())?;
    let arguments = arguments
        .into_iter()
        .filter(|argument| !crate::typechecker::TypeChecker::rust_generic_arg_is_lifetime(argument))
        .collect();
    Some((identity, arguments))
}

/// The Body IR type of one written argument display: the checker's own Rust display conversion, projected again so
/// nested Rust items and checked Incan names close too.
fn argument_type(argument: &str, facts: &TypeCheckInfo) -> IncanType {
    if let Some(checked) = checked_name(argument.trim(), facts) {
        return checked;
    }
    let resolved = crate::typechecker::TypeChecker::new().resolved_type_from_rust_display(argument);
    super::runtime_nominals::project_type(semantic_type_from_resolved(&resolved), facts)
}

/// A checked Incan alias or nominal that a Rust display spells by its bare name (`HashMap<String, TextCallback>`).
///
/// The checker writes Incan type arguments of a Rust generic into its display by name; this reads them back from the
/// checked facts rather than from the spelling's shape.
fn checked_name(name: &str, facts: &TypeCheckInfo) -> Option<IncanType> {
    if let Some(target) = facts.type_alias_target(name) {
        return Some(super::parameter_types::checked_semantic_type(target, facts));
    }
    facts
        .declarations
        .named_type_identities
        .contains_key(name)
        .then(|| IncanType::Named(name.to_owned()))
}
