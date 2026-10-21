//! Resolve the crate aliases Oven binds to exact artifacts, using rustc's own extern declarations.

use rustc_hir::def_id::DefId;
use rustc_middle::ty::TyCtxt;

/// Find an explicitly imported alias before considering a unique canonical crate name. Renamed dependencies and
/// multiple versions must retain rustc's binding rather than selecting the first crate with a matching package name.
/// The reserved `crate` root identifies local HIR definitions rather than an external dependency.
/// Repeated extern declarations must select the same artifact; conflicting bindings grant no root.
pub(crate) fn root(tcx: TyCtxt<'_>, name: &str) -> Option<DefId> {
    if name == "crate" {
        return Some(rustc_hir::def_id::LOCAL_CRATE.as_def_id());
    }
    let mut imported = tcx
        .hir_crate_items(())
        .free_items()
        .filter(|item| {
            let definition = item.owner_id.to_def_id();
            tcx.def_kind(definition) == rustc_hir::def::DefKind::ExternCrate
                && tcx
                    .opt_item_name(definition)
                    .is_some_and(|symbol| symbol.as_str() == name)
        })
        .filter_map(|item| tcx.extern_mod_stmt_cnum(item.owner_id.def_id));
    if let Some(first) = imported.next() {
        return imported.all(|other| other == first).then(|| first.as_def_id());
    }
    let mut candidates = tcx
        .crates(())
        .iter()
        .filter(|krate| tcx.crate_name(**krate).as_str() == name);
    let found = candidates.next()?.as_def_id();
    if candidates.next().is_some() {
        return None;
    }
    Some(found)
}
