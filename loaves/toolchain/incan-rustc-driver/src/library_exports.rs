//! The `pub use` items a library unit publishes its re-exported names with.
//!
//! The Incan lowering resolves each name to a planned item of this crate (`crate::item`) or a Rust item (`::path`);
//! this module only spells those resolutions as items at the crate root.

use rustc_ast as ast;
use rustc_span::{Ident, Span, kw};
use std::collections::BTreeSet;
use thin_vec::ThinVec;

use crate::ast_build::ident;
use crate::error::PlanError;
use crate::plan::Plan;
use incan_frontend::library_manifest::{LibraryManifest, TypeRef};
use incan_semantics_core::body_ir::loaf_item_path;
use incan_semantics_core::{SymbolOrigin, encode_incan_symbol_identity};

/// Publish canonical ABI items, nongeneric public Loaf facades and checked Rust item reexports. Facade aliases point
/// to those same items; they introduce neither layouts nor callable implementations. An executable has no manifest.
pub(crate) fn items(
    plan: &Plan,
    manifest: Option<&LibraryManifest>,
    span: Span,
) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut items = plan
        .exports
        .iter()
        .map(|export| public_use(&export.name, &export.path, span))
        .collect::<Vec<_>>();
    let Some(manifest) = manifest else {
        return Ok(items);
    };
    for name in public_names(manifest) {
        let Some(canonical) = manifest
            .contract_metadata
            .identity_graph
            .canonical_for_public_name(name)
        else {
            continue;
        };
        let path = match &canonical.origin {
            SymbolOrigin::RustCrate(path) => format!("::{}", path.join("::")),
            SymbolOrigin::Package { library, .. } if library == &manifest.name => {
                let encoded = encode_incan_symbol_identity(&canonical);
                let Some(export) = plan.exports.iter().find(|export| export.name == encoded) else {
                    continue;
                };
                export.path.clone()
            }
            _ => {
                let Some(native) = loaf_item_path(&canonical) else {
                    continue;
                };
                format!("::{native}")
            }
        };
        if let Some(existing) = root_target(plan, name) {
            if existing.trim_start_matches("::") != path.trim_start_matches("::") {
                return Err(PlanError::Invalid {
                    function: name.into(),
                    reason: "checked public facade collides with another native root item".into(),
                });
            }
            continue;
        }
        items.push(public_use(name, &path, span));
    }
    Ok(items)
}

/// Select concrete Loaf facades and explicit aliases of checked Rust items. Rust reexports retain the original item's
/// parameters; generic Incan bodies and open Incan nominals still have no single Rust item to name. Overload ambiguity
/// remains the identity graph's decision.
fn public_names(manifest: &LibraryManifest) -> BTreeSet<&str> {
    let exports = &manifest.exports;
    let mut names = BTreeSet::new();
    names.extend(
        exports
            .functions
            .iter()
            .filter(|item| item.type_params.is_empty())
            .map(|item| item.name.as_str()),
    );
    names.extend(
        exports
            .models
            .iter()
            .filter(|item| item.type_params.is_empty())
            .map(|item| item.name.as_str()),
    );
    names.extend(
        exports
            .classes
            .iter()
            .filter(|item| item.type_params.is_empty())
            .map(|item| item.name.as_str()),
    );
    names.extend(
        exports
            .enums
            .iter()
            .filter(|item| item.type_params.is_empty())
            .map(|item| item.name.as_str()),
    );
    names.extend(
        exports
            .newtypes
            .iter()
            .filter(|item| item.type_params.is_empty())
            .map(|item| item.name.as_str()),
    );
    for alias in &exports.aliases {
        let rust_item = manifest
            .contract_metadata
            .identity_graph
            .canonical_for_public_name(&alias.name)
            .is_some_and(|canonical| matches!(canonical.origin, SymbolOrigin::RustCrate(_)));
        if rust_item
            || alias
                .projected_function
                .as_ref()
                .is_some_and(|function| function.type_params.is_empty())
            || matches!(alias.projected_type, Some(TypeRef::Named { .. }))
        {
            names.insert(alias.name.as_str());
        }
    }
    names
}

/// Resolve an already injected root's retained target so identical checked aliases need no duplicate Rust binding.
fn root_target(plan: &Plan, name: &str) -> Option<String> {
    if plan.functions.iter().any(|function| function.name == name) {
        return Some(format!("crate::{name}"));
    }
    if let Some(model) = plan.models.iter().find(|model| model.name == name) {
        return Some(if model.library_item.is_empty() {
            format!("crate::{name}")
        } else {
            format!("::{}", model.library_item)
        });
    }
    plan.enums.iter().find(|value| value.name == name).map(|value| {
        if value.library_item.is_empty() {
            format!("crate::{name}")
        } else {
            format!("::{}", value.library_item)
        }
    })
}

/// Spell a checked alias as one Rust item, shared by canonical exports and public facade projections.
fn public_use(name: &str, path: &str, span: Span) -> Box<ast::Item> {
    let tree = ast::UseTree {
        prefix: export_path(path, span),
        kind: ast::UseTreeKind::Simple(Some(ident(name, span))),
    };
    crate::declarations::item(ast::ItemKind::Use(tree), span)
}

/// Spell `crate::a::b` or `::a::b` as a path; the lowering emits only these two forms.
fn export_path(path: &str, span: Span) -> ast::Path {
    let mut segments = ThinVec::new();
    let rest = if let Some(rest) = path.strip_prefix("::") {
        segments.push(ast::PathSegment::path_root(span));
        rest
    } else if let Some(rest) = path.strip_prefix("crate::") {
        segments.push(ast::PathSegment::from_ident(Ident::new(kw::Crate, span)));
        rest
    } else {
        path
    };
    for segment in rest.split("::") {
        segments.push(ast::PathSegment::from_ident(ident(segment, span)));
    }
    ast::Path {
        span,
        segments,
        tokens: None,
    }
}
