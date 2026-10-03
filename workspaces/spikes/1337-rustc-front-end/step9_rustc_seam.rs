//! Spike step 9: Incan code drives the rustc seam, natively, so the Body IR lowering can be written in Incan.
//!
//! One driver compiles every unit, as Oven would invoke it:
//! - `mir_seam`, the Rust layer that owns rustc's types. It uses `#![feature(rustc_private)]`, which the driver allows
//!   only for a unit invoked with the declared flag `--incan-allow-rustc-private`. The permission is part of the
//!   invocation, and `unstable_features` is a tracked session option, so it enters the unit's identity; no ambient
//!   `RUSTC_BOOTSTRAP` is involved.
//! - `planner`, an Incan unit invoked with `--incan-unit`: an empty crate root, every item injected. Its function
//!   fills the seam's `BodyPlan` and calls `compile`:
//!
//! ```incan
//! from rust::mir_seam import BodyPlan, compile
//!
//! pub def compile_answer(value: int, program: str, output: str, sysroot: str) -> int:
//!     mut plan = BodyPlan.new()
//!     plan.add_constant_return("answer", value)
//!     return compile(plan, program, output, sysroot)
//! ```
#![feature(rustc_private)]
extern crate rustc_abi;
extern crate rustc_ast;
extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_feature;
extern crate rustc_hir;
extern crate rustc_index;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;
extern crate thin_vec;

mod common;

use common::{Cfg, extern_crate, extern_item, function, glob_use, inherent_method, module, public, str_literal, t};
use rustc_ast as ast;
use rustc_data_structures::steal::Steal;
use rustc_hir::def_id::LocalDefId;
use rustc_middle::mir::*;
use rustc_middle::ty::{Ty, TyCtxt};
use rustc_span::{FileName, Symbol};
use thin_vec::thin_vec;

/// Borrow the owned `str` (a `String`) in `text` as `&str`, through `<String as Deref>::deref` as in step 8.
fn borrow_as_str<'tcx>(cfg: &mut Cfg<'tcx>, bb: BasicBlock, text: Place<'tcx>) -> (BasicBlock, Place<'tcx>) {
    let tcx = cfg.tcx;
    let erased = tcx.lifetimes.re_erased;
    let string_ty = text.ty(&cfg.locals, tcx).ty;
    let borrowed = cfg.temp(Ty::new_imm_ref(tcx, erased, string_ty));
    cfg.assign(bb, borrowed, Rvalue::Ref(erased, BorrowKind::Shared, text), cfg.span);
    let Some(deref_trait) = tcx.lang_items().deref_trait() else { tcx.dcx().fatal("this sysroot has no `Deref` lang item") };
    let Some(deref) = tcx.associated_item_def_ids(deref_trait).iter().copied().find(|d| tcx.item_name(*d) == Symbol::intern("deref")) else {
        tcx.dcx().fatal("`Deref` has no `deref` method");
    };
    let as_str = cfg.temp(Ty::new_imm_ref(tcx, erased, tcx.types.str_));
    let next = cfg.call(bb, deref, &[string_ty.into()], vec![Operand::Move(borrowed)], as_str);
    (next, as_str)
}

/// Drop each owned place in turn, ending in a block that returns.
fn drop_and_return<'tcx>(cfg: &mut Cfg<'tcx>, mut bb: BasicBlock, owned: &[Place<'tcx>]) {
    for place in owned {
        let next = cfg.block();
        cfg.terminate(bb, TerminatorKind::Drop { place: *place, target: next, unwind: UnwindAction::Continue, replace: false, drop: None }, cfg.span);
        bb = next;
    }
    cfg.terminate(bb, TerminatorKind::Return, cfg.span);
}

/// `compile_answer(value, program, output, sysroot)`, lowered from the Incan above.
fn compile_answer_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Body<'tcx> {
    let mut cfg = Cfg::new(tcx, def);
    let span = cfg.span;
    let erased = tcx.lifetimes.re_erased;
    let (value, program, output, sysroot) = (cfg.arg(0), cfg.arg(1), cfg.arg(2), cfg.arg(3));
    let body_plan = extern_item(tcx, &["mir_seam", "BodyPlan"]);
    let plan_ty = tcx.type_of(body_plan).instantiate_identity().skip_normalization();

    // ---- mut plan = BodyPlan.new() ----
    let plan = cfg.temp(plan_ty);
    let bb = cfg.call(BasicBlock::from_u32(0), inherent_method(tcx, body_plan, "new"), &[], vec![], plan);

    // ---- plan.add_constant_return("answer", value) ----
    let plan_mut = cfg.temp(Ty::new_mut_ref(tcx, erased, plan_ty));
    cfg.assign(bb, plan_mut, Rvalue::Ref(erased, BorrowKind::Mut { kind: MutBorrowKind::Default }, plan), span);
    let unit = cfg.temp(tcx.types.unit);
    let args = vec![Operand::Move(plan_mut), str_literal(tcx, "answer", span), Operand::Copy(value)];
    let bb = cfg.call(bb, inherent_method(tcx, body_plan, "add_constant_return"), &[], args, unit);

    // ---- return compile(plan, program, output, sysroot) ----
    let plan_ref = cfg.temp(Ty::new_imm_ref(tcx, erased, plan_ty));
    cfg.assign(bb, plan_ref, Rvalue::Ref(erased, BorrowKind::Shared, plan), span);
    let (bb, program_str) = borrow_as_str(&mut cfg, bb, program);
    let (bb, output_str) = borrow_as_str(&mut cfg, bb, output);
    let (bb, sysroot_str) = borrow_as_str(&mut cfg, bb, sysroot);
    let args = vec![Operand::Move(plan_ref), Operand::Move(program_str), Operand::Move(output_str), Operand::Move(sysroot_str)];
    let bb = cfg.call(bb, extern_item(tcx, &["mir_seam", "compile"]), &[], args, Place::return_place());
    drop_and_return(&mut cfg, bb, &[plan, program, output, sysroot]);
    cfg.finish()
}

fn mir_built<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> &'tcx Steal<Body<'tcx>> {
    match tcx.opt_item_name(def.to_def_id()).map(|n| n.to_string()).as_deref() {
        Some("compile_answer") if def.to_def_id().krate == rustc_hir::def_id::LOCAL_CRATE && is_incan_unit() => {
            tcx.alloc_steal_mir(compile_answer_body(tcx, def))
        }
        _ => (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.mir_built)(tcx, def),
    }
}

/// Whether this invocation compiles the Incan unit. Set once in `main`, before rustc starts.
static INCAN_UNIT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn is_incan_unit() -> bool {
    INCAN_UNIT.get().copied().unwrap_or(false)
}

struct Callbacks {
    incan_unit: bool,
    allow_rustc_private: bool,
}

impl rustc_driver::Callbacks for Callbacks {
    fn config(&mut self, config: &mut rustc_interface::interface::Config) {
        if self.allow_rustc_private {
            config.opts.unstable_features = rustc_feature::UnstableFeatures::Allow;
        }
        if self.incan_unit {
            config.input = rustc_session::config::Input::Str { name: FileName::Custom("planner.incn".to_string()), input: String::new() };
            config.override_queries = Some(|_sess, providers| providers.queries.mir_built = mir_built);
        }
    }
    fn after_crate_root_parsing(&mut self, _c: &rustc_interface::interface::Compiler, krate: &mut ast::Crate) -> rustc_driver::Compilation {
        if !self.incan_unit {
            return rustc_driver::Compilation::Continue;
        }
        let span = krate.spans.inner_span;
        let params = [("value", t("i64")), ("program", t("String")), ("output", t("String")), ("sysroot", t("String"))];
        let planner = thin_vec![public(function("compile_answer", ast::Generics::default(), &params, t("i64"), span))];
        krate.items.push(extern_crate("mir_seam", span));
        krate.items.push(public(module("planner", planner, span)));
        krate.items.push(public(module("caller", thin_vec![public(module("incan", thin_vec![public(glob_use(&["crate", "planner"], span))], span))], span)));
        rustc_driver::Compilation::Continue
    }
}

fn main() {
    // The driver's own flags are declared by Oven per unit and never reach rustc's option parser.
    let mut callbacks = Callbacks { incan_unit: false, allow_rustc_private: false };
    let args: Vec<String> = std::env::args()
        .filter(|arg| match arg.as_str() {
            "--incan-unit" => { callbacks.incan_unit = true; false }
            "--incan-allow-rustc-private" => { callbacks.allow_rustc_private = true; false }
            _ => true,
        })
        .collect();
    let _ = INCAN_UNIT.set(callbacks.incan_unit);
    rustc_driver::run_compiler(&args, &mut callbacks);
}
