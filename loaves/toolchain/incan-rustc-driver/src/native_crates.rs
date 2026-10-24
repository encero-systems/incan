//! Resolve checked native crate identities to exact unit and selected Loaf artifacts, using rustc's extern
//! declarations.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rustc_hir::def_id::DefId;
use rustc_middle::ty::TyCtxt;

use incan_driver::unit::UnitLoaf;
use incan_frontend::loaf_index::LoafArtifact;

/// Select exact artifacts needed by a checked plan from direct inputs and its reached Loaves' native closures.
/// A direct input binds its crate for the whole compilation, so a Loaf closure that recorded another build of the
/// same crate (as every Loaf built against the bootstrap runtime does) neither overrides nor contradicts it; closures
/// supply only the roots the unit leaves unbound. Private provider aliases enter only native compilation, after source
/// checking; conflicting artifacts remain refused instead of choosing a crate from a search directory or exposing
/// transitive imports to the checker.
/// A plan whose roots are already bound by direct inputs does not need closure discovery; the checker has already
/// admitted its Loaf manifests, and reopening them cannot contribute another binding.
pub(crate) fn link_externs(
    plan: &crate::plan::Plan,
    direct: &[(String, PathBuf)],
    loaves: &[UnitLoaf],
) -> Result<Vec<(String, PathBuf)>, String> {
    let roots = plan
        .externals
        .iter()
        .filter_map(|external| callee_root(&external.path))
        .chain(
            plan.models
                .iter()
                .map(|model| &model.library_item)
                .chain(plan.enums.iter().map(|value| &value.library_item))
                .filter_map(|item| item.split("::").next()),
        )
        .filter(|root| !matches!(*root, "" | "std" | "core" | "alloc" | "crate"))
        .collect::<BTreeSet<_>>();
    let closures = if roots.iter().any(|root| !direct.iter().any(|(name, _)| name == root)) {
        loaves
            .iter()
            .map(|loaf| {
                LoafArtifact::read(&loaf.dir, &loaf.name)
                    .and_then(|artifact| artifact.native_closure())
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };
    let mut selected = direct.to_vec();
    for root in roots {
        let mut offered = direct.iter().filter(|(name, _)| name == root).collect::<Vec<_>>();
        if offered.is_empty() {
            offered = closures
                .iter()
                .flat_map(|closure| &closure.externs)
                .filter(|(name, _)| name == root)
                .collect();
        }
        let mut candidates = offered.into_iter();
        let Some((_, artifact)) = candidates.next() else {
            return Err(format!(
                "unsupported Body IR native dependency `{root}` in the selected Loaf closure"
            ));
        };
        for (_, other) in candidates {
            if !same_artifact(artifact, other)? {
                return Err(format!(
                    "native dependency `{root}` names conflicting artifacts in the selected Loaf closure"
                ));
            }
        }
        if !artifact.is_file() {
            return Err(format!(
                "native dependency `{root}` artifact does not exist: {}",
                artifact.display()
            ));
        }
        if !selected.iter().any(|(name, _)| name == root) {
            selected.push((root.to_owned(), artifact.clone()));
        }
    }
    Ok(selected)
}

/// Accept relocated copies only after comparing their complete bytes; filenames and crate hashes are not proof.
fn same_artifact(left: &Path, right: &Path) -> Result<bool, String> {
    if left == right {
        return Ok(true);
    }
    let read = |path: &Path| {
        std::fs::read(path).map_err(|error| format!("cannot read native artifact {}: {error}", path.display()))
    };
    Ok(read(left)? == read(right)?)
}

/// The crate a retained callee needs loaded. A slice or `str` method's internal owner is not a crate; its primitive
/// impl bucket belongs to the always-linked standard closure. Full path and signature validation remain separate.
pub(crate) fn callee_root(path: &str) -> Option<&str> {
    path.split("::")
        .next()
        .map(|root| if matches!(root, "[]" | "str") { "core" } else { root })
}

/// Find an explicitly imported alias, including an anonymous metadata import, before a unique canonical crate name.
/// Renamed dependencies and multiple versions must retain rustc's binding rather than selecting the first crate with a
/// matching package name. A `crate` root names this compilation's injected declarations, rather than an external
/// dependency. Repeated extern declarations must select the same artifact; conflicting bindings grant no root.
pub(crate) fn root(tcx: TyCtxt<'_>, name: &str) -> Option<DefId> {
    if name == "crate" {
        return Some(rustc_hir::def_id::LOCAL_CRATE.as_def_id());
    }
    let mut imported = tcx.hir_crate_items(()).free_items().filter_map(|item| {
        let rustc_hir::ItemKind::ExternCrate(original, binding) = tcx.hir_item(item).kind else {
            return None;
        };
        let alias = if binding.name == rustc_span::kw::Underscore {
            original.unwrap_or(binding.name)
        } else {
            binding.name
        };
        if alias.as_str() != name {
            return None;
        }
        tcx.extern_mod_stmt_cnum(item.owner_id.def_id)
    });
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
