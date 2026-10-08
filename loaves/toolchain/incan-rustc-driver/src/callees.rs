//! Canonical path resolution against rustc metadata, never generated function names.

use crate::error::PlanError;
use crate::plan::{Callee, CalleeKind};
use rustc_hir::def::DefKind;
use rustc_hir::def_id::DefId;
use rustc_middle::ty::TyCtxt;

/// Resolve a planned source name or an external canonical path to a free function; explicit plan arguments are checked
/// separately.
pub fn resolve(tcx: TyCtxt<'_>, callee: &Callee) -> Result<DefId, PlanError> {
    match &callee.kind {
<<<<<<< ours
        CalleeKind::Planned(name) => tcx
            .hir_crate_items(())
            .free_items()
            .map(|item| item.owner_id.to_def_id())
            .find(|def| tcx.opt_item_name(*def).is_some_and(|symbol| symbol.as_str() == name))
            .ok_or_else(|| PlanError::UnknownCallee(name.clone())),
        CalleeKind::Instantiated(path, _) | CalleeKind::InstantiatedPair(path, _, _) => external(tcx, path),
=======
        CalleeKind::Planned(name) => planned(tcx, name),
        CalleeKind::Value(_) => Err(PlanError::UnknownCallee(
            "local callable has no declaration callee".into(),
        )),
>>>>>>> theirs
        CalleeKind::External(path) => external(tcx, path),
        CalleeKind::CloneModel(_, _) | CalleeKind::CloneEnum(_, _) => {
            let trait_id = tcx
                .lang_items()
                .clone_trait()
                .ok_or_else(|| PlanError::UnknownCallee("Clone".into()))?;
            tcx.associated_item_def_ids(trait_id)
                .iter()
                .copied()
                .find(|def| tcx.item_name(*def).as_str() == "clone")
                .ok_or_else(|| PlanError::UnknownCallee("Clone::clone".into()))
        }
    }
}

<<<<<<< ours
/// Walk only public module children, rejecting nonfunction callees before constructing MIR.
=======
/// Resolve only a source-local function item admitted by the plan's canonical frontend identity.
pub fn planned(tcx: TyCtxt<'_>, name: &str) -> Result<DefId, PlanError> {
    tcx.hir_crate_items(())
        .free_items()
        .map(|item| item.owner_id.to_def_id())
        .find(|def| {
            tcx.def_kind(*def) == DefKind::Fn && tcx.opt_item_name(*def).is_some_and(|symbol| symbol.as_str() == name)
        })
        .ok_or_else(|| PlanError::UnknownCallee(name.into()))
}

/// Walk only public module children, rejecting nonfunction and generic callees before constructing MIR.
>>>>>>> theirs
pub fn external(tcx: TyCtxt<'_>, path: &str) -> Result<DefId, PlanError> {
    let mut segments = path.split("::");
    let root = segments.next().ok_or_else(|| PlanError::UnknownCallee(path.into()))?;
    let krate = tcx
        .crates(())
        .iter()
        .find(|krate| tcx.crate_name(**krate).as_str() == root)
        .ok_or_else(|| PlanError::UnknownCallee(path.into()))?;
    let mut current = krate.as_def_id();
    for segment in segments {
        if tcx.def_kind(current) != DefKind::Mod {
            return Err(PlanError::UnknownCallee(path.into()));
        }
        current = tcx
            .module_children(current)
            .iter()
            .find(|child| child.ident.name.as_str() == segment && child.vis.is_public())
            .and_then(|child| child.res.opt_def_id())
            .ok_or_else(|| PlanError::UnknownCallee(path.into()))?;
    }
    if tcx.def_kind(current) != DefKind::Fn {
        return Err(PlanError::UnknownCallee(path.into()));
    }
    Ok(current)
}

/// Instantiate explicit plan arguments after checking their count against the resolved metadata declaration.
pub fn arguments<'tcx>(
    tcx: TyCtxt<'tcx>,
    def: DefId,
    types: &[crate::plan::PlanType],
) -> Result<rustc_middle::ty::GenericArgsRef<'tcx>, PlanError> {
    if tcx.generics_of(def).count() != types.len() {
        return Err(PlanError::Invalid {
            function: tcx.def_path_str(def),
            reason: "generic argument count differs from metadata".into(),
        });
    }
    let arguments = types
        .iter()
        .map(|ty| crate::types::native_type(tcx, ty).map(Into::into))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tcx.mk_args(&arguments))
}
