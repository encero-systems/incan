//! Which import item of a module binds a re-exported projection.
//!
//! Every import item of a projected declaration (a function, partial or static) binds that declaration's one
//! identity projection, and a Rust module binds a name once. Emission keeps the first `use` of a projection in a
//! module and gives each later item of it only its own name. A module that re-exports such a declaration is read
//! through that projection (a consumer's import of the re-exported name binds the projection from this module), so
//! the first item of the projection has to bind it, and bind it publicly. Lowering decides which item that is from
//! the module's import declarations alone: their order, their visibility and the identities they carry. A public item
//! is never pruned for want of a use, so the decision does not depend on what emission later finds reachable.

use std::collections::{HashMap, HashSet};

use super::super::super::decl::{IrDecl, IrDeclKind, IrImportItem, Visibility, is_projected_source_symbol};
use super::super::AstLowering;
use incan_frontend::symbols::is_overload_emitted_name;
use incan_semantics_core::encode_incan_symbol_identity;

/// One import item of a projected declaration, by its position among the module's declarations.
#[derive(Debug, Clone, Copy)]
struct ProjectionSite {
    /// Index of the import declaration among the module's declarations.
    decl: usize,
    /// Index of the item within that import declaration.
    item: usize,
    /// Whether the import declaration re-exports its items.
    public: bool,
    /// Whether the item is spelled by a name other than the declaration's own (see [`is_alias_spelled`]).
    alias_spelled: bool,
    /// Whether the item binds a static storage cell.
    is_static: bool,
}

/// How one projection's binding item is handed to the emitter.
#[derive(Debug, Clone, Copy)]
struct BindingPlan {
    /// The public item that binds the projection.
    binder: ProjectionSite,
    /// The declaration the binder moves ahead of, in an import of its own, when another item came first.
    move_before: Option<usize>,
}

/// Return whether an import item is spelled by a name other than its declaration's own.
///
/// An `alias` declaration (`pub scale_alias = alias scale`) carries the identity of the declaration it renames, and
/// so does every re-export of it. Emission reads such an item as a second name over a projection another item binds.
fn is_alias_spelled(item: &IrImportItem) -> bool {
    item.canonical
        .as_ref()
        .is_some_and(|identity| identity.declaration_name != item.name)
}

impl AstLowering {
    /// Make a module's first import item of each re-exported projection a public item that binds it.
    ///
    /// Left as written, two shapes give a re-exported projection no public binding, and a consumer importing the
    /// re-exported name fails in rustc:
    ///
    /// - The first item is alias-spelled: a facade re-exporting an `alias` declaration without its target (`pub from
    ///   provider import scale_alias`, #1750), a further re-export renaming it again, or a module re-exporting a
    ///   package's renamed export under another name (`pub from pub::calc_lib import facade_calculate as b_calculate`,
    ///   #1751). Emission binds only the alias's own name for such an item. The binder is respelled by the
    ///   declaration's own name with the written name as its alias, the shape `pub from provider import scale as
    ///   scale_alias` lowers to, so it binds the projection and keeps its name.
    /// - An earlier private import of the same declaration takes the module's one binding (`from provider import scale`
    ///   before `pub from provider import scale_alias`), so the binding is private (E0603). The public binder moves
    ///   into an import of its own ahead of the first item, and the private item becomes the repeat.
    ///
    /// The binder is the first public item spelled by the declaration's own name when there is one, so every public
    /// spelling keeps its Rust-facing name, and otherwise the first public item. A projection that a public source
    /// alias in the module republishes is left alone, because that alias takes the binding (see the `SymbolAlias`
    /// emission). Overload members are left alone: each has its own projection and binding rule. A static binder is
    /// respelled but never moved, because the private item it would displace carries the static's initialization
    /// import.
    pub(in crate::lower) fn bind_reexported_projections(declarations: &mut Vec<IrDecl>) {
        let republished_by_alias: HashSet<String> = declarations
            .iter()
            .filter_map(|decl| match &decl.kind {
                IrDeclKind::SymbolAlias {
                    visibility: Visibility::Public,
                    target_canonical: Some(identity),
                    ..
                } if is_projected_source_symbol(identity) => Some(encode_incan_symbol_identity(identity)),
                _ => None,
            })
            .collect();

        let mut projections: Vec<String> = Vec::new();
        let mut sites: HashMap<String, Vec<ProjectionSite>> = HashMap::new();
        for (decl_index, decl) in declarations.iter().enumerate() {
            let IrDeclKind::Import { visibility, items, .. } = &decl.kind else {
                continue;
            };
            for (item_index, item) in items.iter().enumerate() {
                let Some(identity) = item
                    .canonical
                    .as_ref()
                    .filter(|identity| is_projected_source_symbol(identity))
                else {
                    continue;
                };
                if is_overload_emitted_name(&item.name) || item.force_reexport {
                    continue;
                }
                let projection = encode_incan_symbol_identity(identity);
                if republished_by_alias.contains(&projection) {
                    continue;
                }
                let site = ProjectionSite {
                    decl: decl_index,
                    item: item_index,
                    public: !matches!(visibility, Visibility::Private),
                    alias_spelled: is_alias_spelled(item),
                    is_static: item.is_static,
                };
                sites
                    .entry(projection.clone())
                    .or_insert_with(|| {
                        projections.push(projection.clone());
                        Vec::new()
                    })
                    .push(site);
            }
        }

        let plans: Vec<BindingPlan> = projections
            .iter()
            .filter_map(|projection| Self::projection_binding_plan(sites.get(projection)?))
            .collect();
        if plans.is_empty() {
            return;
        }

        for plan in &plans {
            if let IrDeclKind::Import { items, .. } = &mut declarations[plan.binder.decl].kind
                && let Some(item) = items.get_mut(plan.binder.item)
                && plan.binder.alias_spelled
                && let Some(declaration_name) = item
                    .canonical
                    .as_ref()
                    .map(|identity| identity.declaration_name.clone())
            {
                let exposed_as = item.source_binding_name().to_string();
                item.name = declaration_name;
                item.alias = Some(exposed_as);
            }
        }

        let moved: HashSet<(usize, usize)> = plans
            .iter()
            .filter(|plan| plan.move_before.is_some())
            .map(|plan| (plan.binder.decl, plan.binder.item))
            .collect();
        if moved.is_empty() {
            return;
        }
        let mut inserted_before: HashMap<usize, Vec<IrDecl>> = HashMap::new();
        for plan in &plans {
            let Some(target) = plan.move_before else {
                continue;
            };
            let source = &declarations[plan.binder.decl];
            let IrDeclKind::Import {
                visibility,
                origin,
                qualifier,
                path,
                alias,
                items,
            } = &source.kind
            else {
                continue;
            };
            let Some(item) = items.get(plan.binder.item) else {
                continue;
            };
            inserted_before.entry(target).or_default().push(IrDecl {
                kind: IrDeclKind::Import {
                    visibility: *visibility,
                    origin: origin.clone(),
                    qualifier: *qualifier,
                    path: path.clone(),
                    alias: alias.clone(),
                    items: vec![item.clone()],
                },
                span: source.span,
            });
        }

        let original = std::mem::take(declarations);
        for (decl_index, mut decl) in original.into_iter().enumerate() {
            if let Some(binders) = inserted_before.remove(&decl_index) {
                declarations.extend(binders);
            }
            if let IrDeclKind::Import { items, .. } = &mut decl.kind
                && moved.iter().any(|(moved_decl, _)| *moved_decl == decl_index)
            {
                let mut item_index = 0;
                items.retain(|_| {
                    let keep = !moved.contains(&(decl_index, item_index));
                    item_index += 1;
                    keep
                });
                if items.is_empty() {
                    continue;
                }
            }
            declarations.push(decl);
        }
    }

    /// Plan the binding of one projection from its import items, in declaration order.
    ///
    /// Returns `None` when no public item re-exports the projection, when the binder already comes first and is
    /// spelled by the declaration's own name, and for a static whose first item is private or is public and spelled by
    /// the static's own name.
    fn projection_binding_plan(sites: &[ProjectionSite]) -> Option<BindingPlan> {
        let first = *sites.first()?;
        if first.is_static {
            return (first.public && first.alias_spelled).then_some(BindingPlan {
                binder: first,
                move_before: None,
            });
        }
        let binder = sites
            .iter()
            .find(|site| site.public && !site.alias_spelled)
            .or_else(|| sites.iter().find(|site| site.public))
            .copied()?;
        let binder_is_first = binder.decl == first.decl && binder.item == first.item;
        let move_before = (!binder_is_first).then_some(first.decl);
        (move_before.is_some() || binder.alias_spelled).then_some(BindingPlan { binder, move_before })
    }
}
