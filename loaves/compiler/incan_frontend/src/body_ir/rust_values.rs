//! Rust values a program holds (DD-0006).
//!
//! A checked Rust type with no Incan meaning (`std::sync::Arc<T>`, `core::str::error::Utf8Error`) becomes the opaque
//! nominal of its Rust item, generic over its checked arguments and named by the item's path. Metadata defaults
//! complete omitted owner arguments at this boundary. Instantiations close like
//! any generic nominal, and the driver stores each one as the item itself; the items a compilation holds are read from
//! its laid-out Body IR types (`incan_semantics_core::body_ir::held_rust_items`). Rust types with an Incan meaning
//! (collections, strings, options) never reach this module: the checker already resolved them to their Incan types.

use incan_semantics_core::CanonicalSymbolId;

use super::{IncanType, TypeCheckInfo, bir, semantic_type_from_resolved};

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

/// Project a native display into its structural slice/reference carrier or the opaque nominal of its item. Trait
/// objects and unsupported function types retain their refusal; the internal slice constructor is already structural.
pub(super) fn project(display: &str, facts: &TypeCheckInfo) -> Option<IncanType> {
    if (display.trim().starts_with('[') && display.trim() != "[]") || display.trim().starts_with('&') {
        let resolved = crate::typechecker::TypeChecker::new().native_rust_signature_type(display, "");
        if !matches!(&resolved, crate::symbols::ResolvedType::RustPath(path) if path == display) {
            return Some(super::runtime_nominals::project_type(
                semantic_type_from_resolved(&resolved),
                facts,
            ));
        }
    }
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
    let args = arguments
        .into_iter()
        .map(|argument| argument_type(argument, facts))
        .collect();
    Some(complete_owner(name, args, facts))
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
    let resolved = crate::typechecker::TypeChecker::new().native_rust_signature_type(argument, "");
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
    let identity = facts.declarations.named_type_identities.get(name)?;
    if matches!(identity.origin, incan_semantics_core::SymbolOrigin::RustCrate(_)) {
        return bir::native_opaque_name(identity).map(IncanType::Named);
    }
    Some(IncanType::Named(name.to_owned()))
}
