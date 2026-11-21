//! Public paths for the nominals of crates a facts session reaches only through another crate.
//!
//! rustc shows an item of a crate the stub links directly by its visible path, but an item of an indirect crate by its
//! definition path (`levels::private::Level`), which no source can name. The checker compares Rust types by path, so
//! one item needs one spelling in every session: an indirect crate's type or trait is shown by the shortest public path
//! its own crate's module tree gives it. The standard crates keep rustc's spelling, which the checker already reads.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};

use rustc_data_structures::unord::UnordSet;
use rustc_hir::def::DefKind;
use rustc_hir::def_id::{CrateNum, DefId, DefPathHash, StableCrateId};
use rustc_middle::ty::{self, Ty, TyCtxt};

thread_local! {
    /// Public paths by stable definition identity, filled one crate at a time. Stable identities stay valid across
    /// the facts sessions of one process.
    static PUBLIC_PATHS: RefCell<PublicPaths> = RefCell::new(PublicPaths::default());
}

/// The crates already walked and the shortest public path found for each of their nominals.
#[derive(Default)]
struct PublicPaths {
    walked: HashSet<StableCrateId>,
    paths: HashMap<DefPathHash, String>,
}

impl PublicPaths {
    /// Record each public type and trait of `krate` under its shortest public path, breadth first through the
    /// crate's own public modules; re-exported modules of other crates are not entered.
    fn walk(&mut self, tcx: TyCtxt<'_>, krate: CrateNum) {
        if !self.walked.insert(tcx.stable_crate_id(krate)) {
            return;
        }
        let root = krate.as_def_id();
        let mut pending = VecDeque::from([(tcx.crate_name(krate).to_string(), root)]);
        let mut entered = HashSet::from([root]);
        while let Some((path, module)) = pending.pop_front() {
            for child in tcx.module_children(module).iter() {
                let Some(def) = child.res.opt_def_id() else {
                    continue;
                };
                let name = child.ident.name;
                if !child.vis.is_public() || def.krate != krate || name == rustc_span::kw::Underscore {
                    continue;
                }
                let child_path = format!("{path}::{name}");
                match tcx.def_kind(def) {
                    DefKind::Mod if entered.insert(def) => pending.push_back((child_path, def)),
                    DefKind::Struct
                    | DefKind::Enum
                    | DefKind::Union
                    | DefKind::TyAlias
                    | DefKind::Trait
                    | DefKind::ForeignTy => {
                        self.paths.entry(tcx.def_path_hash(def)).or_insert(child_path);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// The public path of an indirect crate's type or trait, or `None` where rustc's own spelling already serves.
pub(super) fn public_path(tcx: TyCtxt<'_>, def: DefId) -> Option<String> {
    if def.is_local() || !indirect_crate(tcx, def.krate) {
        return None;
    }
    PUBLIC_PATHS.with(|memo| {
        let mut memo = memo.borrow_mut();
        memo.walk(tcx, def.krate);
        memo.paths.get(&tcx.def_path_hash(def)).cloned()
    })
}

/// A crate the stub does not name with `extern crate`, other than the standard crates.
fn indirect_crate(tcx: TyCtxt<'_>, krate: CrateNum) -> bool {
    if matches!(
        tcx.crate_name(krate).as_str(),
        "std" | "core" | "alloc" | "proc_macro" | "test"
    ) {
        return false;
    }
    let direct: UnordSet<CrateNum> = tcx
        .resolutions(())
        .extern_crate_map
        .items()
        .map(|(_, linked)| *linked)
        .collect();
    !direct.contains(&krate)
}

/// Show each indirect crate's type or trait in `printed`, rustc's display of `ty`, by its public path.
pub(super) fn public_display<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>, printed: String) -> String {
    let mut display = printed;
    for def in nominals(ty) {
        if let Some(public) = public_path(tcx, def) {
            let shown = ty::print::with_no_trimmed_paths!(tcx.def_path_str(def));
            display = incan_frontend::rust_type_display::replace_rust_display_path(&display, &shown, &public);
        }
    }
    display
}

/// The types and traits a type names anywhere inside it; each replacement is independent, so order is free.
fn nominals(ty: Ty<'_>) -> HashSet<DefId> {
    let mut found = HashSet::new();
    for argument in ty.walk() {
        let Some(inner) = argument.as_type() else {
            continue;
        };
        match inner.kind() {
            ty::Adt(adt, _) => {
                found.insert(adt.did());
            }
            ty::Foreign(def) => {
                found.insert(*def);
            }
            ty::Dynamic(predicates, _) => {
                found.extend(predicates.principal().map(|principal| principal.skip_binder().def_id));
                found.extend(predicates.auto_traits());
            }
            _ => {}
        }
    }
    found
}
