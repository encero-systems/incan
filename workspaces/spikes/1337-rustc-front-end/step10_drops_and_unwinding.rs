//! Spike step 10: drops and unwinding in front-end MIR, with rustc's drop elaboration doing the conditional part.
//!
//! The Incan function under test:
//!
//! ```incan
//! def route(item: Tracked, take: bool, fail: bool) -> int:
//!     checked = may_panic(item, fail)
//!     if take:
//!         return consume(item) + checked
//!     return checked
//! ```
//!
//! `item` is owned by `route`. It is moved into `consume` on one path and must be dropped on the other, and a panic in
//! `may_panic` or `consume` must still drop it. The MIR below drops `item` *unconditionally* at the exit and in one
//! cleanup block, exactly as rustc's own `mir_build` does, and leaves it to rustc's drop elaboration to remove the drop
//! on the path where `item` was moved. The program counts drops of `Tracked` to prove each value is dropped exactly once.
//!
//! THIR is not an alternative injection point: THIR names variables by `HirId` and `mir_build` schedules their drops
//! through the region scope tree computed from the HIR body, which for an Incan function is only a placeholder.
#![feature(rustc_private)]
extern crate rustc_abi;
extern crate rustc_ast;
extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_index;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_span;
extern crate thin_vec;

mod common;

use common::{Cfg, function, local_item, t};
use rustc_ast as ast;
use rustc_data_structures::steal::Steal;
use rustc_hir::def_id::LocalDefId;
use rustc_middle::mir::*;
use rustc_middle::ty::{Ty, TyCtxt};

fn route_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Body<'tcx> {
    let mut cfg = Cfg::new(tcx, def);
    let span = cfg.span;
    let erased = tcx.lifetimes.re_erased;
    let (item, take, fail) = (cfg.arg(0), cfg.arg(1), cfg.arg(2));
    let item_ty = item.ty(&cfg.locals, tcx).ty;

    // ---- One cleanup path: drop what this body owns, then keep unwinding ----
    let cleanup = cfg.cleanup_block();
    let resume = cfg.cleanup_block();
    let terminate = UnwindAction::Terminate(UnwindTerminateReason::InCleanup);
    cfg.terminate(cleanup, TerminatorKind::Drop { place: item, target: resume, unwind: terminate, replace: false, drop: None }, span);
    cfg.terminate(resume, TerminatorKind::UnwindResume, span);
    let unwind = UnwindAction::Cleanup(cleanup);

    // ---- checked = may_panic(item, fail) ----
    let borrowed = cfg.temp(Ty::new_imm_ref(tcx, erased, item_ty));
    cfg.assign(BasicBlock::from_u32(0), borrowed, Rvalue::Ref(erased, BorrowKind::Shared, item), span);
    let checked = cfg.temp(tcx.types.i64);
    let args = vec![Operand::Move(borrowed), Operand::Copy(fail)];
    let after_check = cfg.call_unwinding_to(BasicBlock::from_u32(0), local_item(tcx, "may_panic"), &[], args, checked, unwind);

    // ---- if take: return consume(item) + checked ----
    let (taken, kept, exit) = (cfg.block(), cfg.block(), cfg.block());
    cfg.terminate(after_check, TerminatorKind::SwitchInt { discr: Operand::Copy(take), targets: SwitchTargets::static_if(0, kept, taken) }, span);
    let consumed = cfg.temp(tcx.types.i64);
    let after_consume = cfg.call_unwinding_to(taken, local_item(tcx, "consume"), &[], vec![Operand::Move(item)], consumed, unwind);
    cfg.assign(after_consume, Place::return_place(), Rvalue::BinaryOp(BinOp::Add, Box::new((Operand::Move(consumed), Operand::Copy(checked)))), span);
    cfg.terminate(after_consume, TerminatorKind::Goto { target: exit }, span);

    // ---- return checked ----
    cfg.assign(kept, Place::return_place(), Rvalue::Use(Operand::Copy(checked), WithRetag::Yes), span);
    cfg.terminate(kept, TerminatorKind::Goto { target: exit }, span);

    // ---- Scope exit: drop `item` unconditionally; drop elaboration removes it where `item` was moved ----
    let done = cfg.block();
    cfg.terminate(exit, TerminatorKind::Drop { place: item, target: done, unwind: UnwindAction::Continue, replace: false, drop: None }, span);
    cfg.terminate(done, TerminatorKind::Return, span);
    cfg.finish()
}

fn mir_built<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> &'tcx Steal<Body<'tcx>> {
    match tcx.opt_item_name(def.to_def_id()).map(|n| n.to_string()).as_deref() {
        Some("route") => tcx.alloc_steal_mir(route_body(tcx, def)),
        _ => (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.mir_built)(tcx, def),
    }
}

struct Callbacks;
impl rustc_driver::Callbacks for Callbacks {
    fn config(&mut self, config: &mut rustc_interface::interface::Config) {
        config.override_queries = Some(|_sess, providers| providers.queries.mir_built = mir_built);
    }
    fn after_crate_root_parsing(&mut self, _c: &rustc_interface::interface::Compiler, krate: &mut ast::Crate) -> rustc_driver::Compilation {
        let span = krate.spans.inner_span;
        let params = [("item", t("Tracked")), ("take", t("bool")), ("fail", t("bool"))];
        krate.items.push(function("route", ast::Generics::default(), &params, t("i64"), span));
        rustc_driver::Compilation::Continue
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    rustc_driver::run_compiler(&args, &mut Callbacks);
}
