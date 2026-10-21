//! Resolve the crate aliases Oven binds to exact artifacts, using rustc's own extern declarations.

use rustc_hir::def_id::DefId;
use rustc_middle::ty::TyCtxt;

/// The crate a retained callee needs loaded. A slice method's internal owner is not a crate; its primitive impl
/// bucket belongs to the always-linked standard closure. Full path and signature validation remain separate.
pub(crate) fn callee_root(path: &str) -> Option<&str> {
    path.split("::")
        .next()
        .map(|root| if root == "[]" { "core" } else { root })
}

/// Find an explicitly imported alias before considering a unique canonical crate name. Renamed dependencies and
/// multiple versions must retain rustc's binding rather than selecting the first crate with a matching package name.
/// A `crate` root names this compilation's injected declarations, rather than an external dependency.
pub(crate) fn root(tcx: TyCtxt<'_>, name: &str) -> Option<DefId> {
    if name == "crate" {
        return Some(rustc_hir::def_id::LOCAL_CRATE.as_def_id());
    }
    for item in tcx.hir_crate_items(()).free_items() {
        let local = item.owner_id.def_id;
        if tcx.opt_item_name(local).is_some_and(|symbol| symbol.as_str() == name)
            && let Some(krate) = tcx.extern_mod_stmt_cnum(local)
        {
            return Some(krate.as_def_id());
        }
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
