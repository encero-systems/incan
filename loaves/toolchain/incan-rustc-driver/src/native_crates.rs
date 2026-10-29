//! Resolve checked native crate identities to exact unit and selected Loaf artifacts, using rustc's extern
//! declarations.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::plan::{CalleeKind, Constant, Operand, OperandKind, Plan, PlanType, RvalueKind, StatementKind};
use incan_driver::unit::UnitLoaf;
use incan_frontend::loaf_index::LoafArtifact;
use rustc_ast::{
    self as ast,
    visit::{self, Visitor},
};
use rustc_hir::def_id::DefId;
use rustc_middle::ty::TyCtxt;

/// Load the explicit crate aliases of retained native constants even when their value types name no native owner.
/// Body IR materializes each constant read into a Use assignment before other operations consume that temporary.
fn constant_crate_roots(plan: &Plan) -> impl Iterator<Item = &str> {
    plan.functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.statements)
        .filter_map(|statement| {
            let StatementKind::Assign(_, value) = &statement.kind else {
                return None;
            };
            let RvalueKind::Use(Operand {
                kind: OperandKind::Literal(Constant::RustItem(path, _)),
                ..
            }) = &value.kind
            else {
                return None;
            };
            callee_root(path)
        })
}

/// The checked native roots shared by AST loading and optional development-library preparation.
pub(crate) fn native_roots(plan: &Plan) -> BTreeSet<&str> {
    let mut roots: BTreeSet<_> = plan
        .externals
        .iter()
        .filter_map(|external| callee_root(&external.path))
        .chain(
            plan.models
                .iter()
                .map(|model| &model.library_item)
                .chain(plan.enums.iter().map(|value| &value.library_item))
                .filter_map(|item| item.split("::").next())
                .filter(|root| !root.is_empty()),
        )
        .collect();
    roots.extend(constant_crate_roots(plan));
    if plan.functions.iter().any(|function| {
        function.locals.iter().any(|local| {
            matches!(
                local.ty,
                PlanType::Generator(..)
                    | PlanType::GeneratorMutRef(..)
                    | PlanType::GeneratorYield(..)
                    | PlanType::GeneratorYieldRef(..)
            )
        })
    }) {
        roots.insert("incan_std_core");
    }
    for function in &plan.functions {
        let Some(callee) = &function.future_callee else {
            continue;
        };
        let path = match &callee.kind {
            CalleeKind::External(path) | CalleeKind::Instantiated(path, _) | CalleeKind::FutureMethod(path, _) => path,
            _ => continue,
        };
        if let Some(root) = callee_root(path) {
            roots.insert(root);
        }
    }
    roots
}

/// Native wrapper paths in declarations require explicit aliases even when no executable call names their crate.
pub(crate) fn declaration_roots(krate: &ast::Crate) -> BTreeSet<&'static str> {
    let mut visitor = DeclarationRoots { roots: BTreeSet::new() };
    visitor.visit_crate(krate);
    visitor.roots
}

/// Read the backend's actual wrapper paths, rather than reconstructing carrier usage from incidental calls.
struct DeclarationRoots {
    roots: BTreeSet<&'static str>,
}

impl<'ast> Visitor<'ast> for DeclarationRoots {
    /// Keep wrapper crate bindings independent of call and constant requirements, including nested type arguments.
    fn visit_path(&mut self, path: &'ast ast::Path) {
        if path.segments.len() > 1
            && let Some(segment) = path
                .segments
                .iter()
                .find(|segment| segment.ident.name != rustc_span::kw::PathRoot)
        {
            match segment.ident.name.as_str() {
                "incan_native_runtime" => {
                    self.roots.insert("incan_native_runtime");
                }
                "incan_std_core" => {
                    self.roots.insert("incan_std_core");
                }
                _ => {}
            }
        }
        visit::walk_path(self, path);
    }
}

/// Select exact artifacts needed by a checked plan from direct inputs and its reached Loaves' native closures.
/// Every offered binding of a required root must name the same complete artifact. Compiled providers and checked
/// source interfaces both contribute their exact native dependencies, including dependencies used by governed source
/// templates. A direct input cannot replace a provider's native type identity: its exported fields and signatures
/// still refer to the selected artifact. Private provider aliases enter only native compilation, after source
/// checking; transitive imports remain private, and conflicting artifacts require rebuilding and publishing a
/// coherent dependency closure.
/// Ambient standard packages contribute bindings only when the checked executable declaration closure reaches
/// them. Explicit non-standard providers retain their existing exact-binding authority.
pub(crate) fn link_externs(
    plan: &crate::plan::Plan,
    direct: &[(String, PathBuf)],
    loaves: &[UnitLoaf],
    reached: &BTreeSet<String>,
) -> Result<Vec<(String, PathBuf)>, String> {
    let roots = native_roots(plan)
        .into_iter()
        .filter(|root| !matches!(*root, "" | "std" | "core" | "alloc" | "crate"))
        .collect::<BTreeSet<_>>();
    let closures = loaves
        .iter()
        .filter(|loaf| !loaf.standard || reached.contains(&loaf.name))
        .map(|loaf| {
            LoafArtifact::read(&loaf.dir, &loaf.name)
                .and_then(|artifact| artifact.native_closure())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut selected = direct.to_vec();
    for root in roots {
        let mut candidates = direct
            .iter()
            .chain(closures.iter().flat_map(|closure| &closure.externs))
            .filter(|(name, _)| name == root);
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

/// The crate a retained callee needs loaded. A primitive method's internal owner is not a crate; its primitive
/// impl bucket belongs to the always-linked standard closure. Full path and signature validation remain separate.
pub(crate) fn callee_root(path: &str) -> Option<&str> {
    path.split("::").next().map(|root| {
        if crate::rust_facts::primitive_owner(root) {
            "core"
        } else {
            root
        }
    })
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
