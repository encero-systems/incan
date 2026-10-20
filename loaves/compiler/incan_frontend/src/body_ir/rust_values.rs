//! Rust values a program holds (DD-0006).
//!
//! A checked Rust type with no Incan meaning (`std::sync::Arc<T>`, `core::str::error::Utf8Error`) becomes the opaque
//! nominal of its Rust item, generic over the written arguments. Instantiations close like any generic nominal, and the
//! driver stores each one as the item itself. Rust types with an Incan meaning (collections, strings, options) never
//! reach this module: the checker already resolved them to their Incan types.

use std::cell::RefCell;
use std::collections::BTreeMap;

use incan_semantics_core::CanonicalSymbolId;

use super::{IncanType, TypeCheckInfo, bir, semantic_type_from_resolved};
use crate::resolved_type_subst::map_resolved_type_leaves;
use crate::symbols::ResolvedType;

/// Project a Rust type display onto the opaque nominal of its item, or `None` when the display names no Rust item
/// (`dyn` objects, references and function types keep their Rust spelling and refuse where they are laid out).
pub(super) fn project(display: &str, facts: &TypeCheckInfo) -> Option<IncanType> {
    if let Some(checked) = checked_name(display.trim(), facts) {
        return Some(checked);
    }
    let (identity, arguments) = rust_item(display, facts)?;
    let name = identity.declaration_name;
    if arguments.is_empty() {
        return Some(IncanType::Named(name));
    }
    let args = arguments
        .into_iter()
        .map(|argument| argument_type(argument, facts))
        .collect();
    Some(IncanType::Generic { base: name, args })
}

/// Opaque declarations for every Rust item the module's checked types hold, one per item.
///
/// An item written with two different argument counts has no single layout and yields none; its uses refuse.
pub(super) fn collect(type_info: &TypeCheckInfo) -> Vec<bir::NominalDeclaration> {
    let mut arities: BTreeMap<CanonicalSymbolId, Option<usize>> = BTreeMap::new();
    for ty in checked_types(type_info) {
        for display in rust_displays(ty) {
            record_items(display.as_str(), type_info, &mut arities);
        }
    }
    arities
        .into_iter()
        .filter_map(|(identity, arity)| bir::native_opaque_nominal(&identity, arity?))
        .collect()
}

/// Every checked type a module's Body IR lays out: expressions, call results, signatures and model fields.
fn checked_types(type_info: &TypeCheckInfo) -> impl Iterator<Item = &ResolvedType> {
    let signatures = type_info.declarations.function_bindings.values().flat_map(|binding| {
        binding
            .params
            .iter()
            .map(|param| &param.ty)
            .chain([&binding.return_type])
    });
    type_info
        .expressions
        .expr_types
        .values()
        .chain(type_info.calls.inferred_call_result_types.values())
        .chain(signatures)
        .chain(type_info.declarations.model_field_types.values())
}

/// The Rust displays anywhere inside `ty`, through the one structural type walk.
fn rust_displays(ty: &ResolvedType) -> Vec<String> {
    let found = RefCell::new(Vec::new());
    map_resolved_type_leaves(ty, &|node| {
        if let ResolvedType::RustPath(display) = node {
            found.borrow_mut().push(display.clone());
        }
        None
    });
    found.into_inner()
}

/// Record the Rust item a display names and, through its argument displays, every nested Rust item.
fn record_items(display: &str, facts: &TypeCheckInfo, arities: &mut BTreeMap<CanonicalSymbolId, Option<usize>>) {
    let Some((identity, arguments)) = rust_item(display, facts) else {
        return;
    };
    let arity = arguments.len();
    arities
        .entry(identity)
        .and_modify(|known| {
            if *known != Some(arity) {
                *known = None;
            }
        })
        .or_insert(Some(arity));
    for argument in arguments {
        record_items(argument, facts, arities);
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
