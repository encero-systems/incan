//! Canonical path resolution against rustc metadata, never generated function names.

use crate::error::PlanError;
use crate::plan::{Callee, CalleeKind};
use rustc_hir::def::DefKind;
use rustc_hir::def_id::DefId;
use rustc_middle::ty::TyCtxt;

/// Resolve a planned source name, canonical external path, or explicit runtime operation to its exact function.
/// Explicit plan arguments are checked separately.
pub fn resolve(tcx: TyCtxt<'_>, callee: &Callee) -> Result<DefId, PlanError> {
    match &callee.kind {
        CalleeKind::Planned(name) => tcx
            .hir_crate_items(())
            .free_items()
            .map(|item| item.owner_id.to_def_id())
            .find(|def| tcx.opt_item_name(*def).is_some_and(|symbol| symbol.as_str() == name))
            .ok_or_else(|| PlanError::UnknownCallee(name.clone())),
        CalleeKind::Instantiated(path, _) | CalleeKind::InstantiatedPair(path, _, _) => external(tcx, path),
        CalleeKind::SpawnGenerator(_, leaf, depth) => generator_method(tcx, "Generator", "spawn", leaf, *depth),
        CalleeKind::YieldGenerator(leaf, depth) => generator_method(tcx, "GeneratorYield", "yield_value", leaf, *depth),
        CalleeKind::CollectGenerator(leaf, depth) => generator_method(tcx, "Generator", "collect", leaf, *depth),
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

/// Select an inherent runtime method from the exact ADT, rather than from a source-spelled method name.
fn generator_method(
    tcx: TyCtxt<'_>,
    owner: &str,
    member: &str,
    leaf: &crate::plan::ListLeaf,
    depth: i64,
) -> Result<DefId, PlanError> {
    let ty = crate::types::generator_type(tcx, owner, leaf, depth)?;
    let rustc_middle::ty::Adt(adt, _) = ty.kind() else {
        return Err(PlanError::UnknownCallee(owner.into()));
    };
    tcx.inherent_impls(adt.did())
        .iter()
        .flat_map(|implementation| tcx.associated_item_def_ids(*implementation))
        .copied()
        .find(|def| tcx.item_name(*def).as_str() == member)
        .ok_or_else(|| PlanError::UnknownCallee(format!("{owner}::{member}")))
}

/// Walk only public module children, rejecting nonfunction callees before constructing MIR.
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
