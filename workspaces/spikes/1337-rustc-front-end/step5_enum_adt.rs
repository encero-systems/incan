//! Spike step 5: an Incan enum is a real Rust enum. Incan MIR matches on it (a discriminant switch over a CFG) and
//! constructs its variants; Rust matches what Incan built and passes Incan code values it built itself.
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

use common::{Cfg, TySpec, enumeration, function, local_item, t};
use rustc_abi::{FieldIdx, VariantIdx};
use rustc_ast as ast;
use rustc_data_structures::steal::Steal;
use rustc_hir::def_id::LocalDefId;
use rustc_index::IndexVec;
use rustc_middle::mir::interpret::Scalar;
use rustc_middle::mir::*;
use rustc_middle::ty::{self, TyCtxt};

/// The payload field `index` of variant `variant` of the enum in `place`, typed from the checked ADT.
fn payload<'tcx>(cfg: &Cfg<'tcx>, place: Place<'tcx>, variant: u32, index: u32) -> Place<'tcx> {
    let tcx = cfg.tcx;
    let ty::Adt(adt, args) = place.ty(&cfg.locals, tcx).ty.kind() else {
        tcx.dcx().fatal("the front end matched on a value that is not an enum");
    };
    let variant = VariantIdx::from_u32(variant);
    let field = FieldIdx::from_u32(index);
    // Incan field types name no associated types, so there is nothing to normalize.
    let field_ty = adt.variant(variant).fields[field].ty(tcx, args).skip_normalization();
    tcx.mk_place_field(tcx.mk_place_downcast(place, *adt, variant), field, field_ty)
}

/// `area(s: Shape) -> i64`:
/// ```incan
/// match s:
///     Circle(r) => return 3 * r * r
///     Rect(w, h) => return w * h
///     Empty => return 0
/// ```
fn area_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Body<'tcx> {
    let mut cfg = Cfg::new(tcx, def);
    let span = cfg.span;
    let s = cfg.arg(0);
    let ret = Place::return_place();
    let i64t = tcx.types.i64;
    let entry = BasicBlock::from_u32(0);

    let discr = cfg.temp(s.ty(&cfg.locals, tcx).ty.discriminant_ty(tcx));
    cfg.assign(entry, discr, Rvalue::Discriminant(s), span);
    let (circle, rect, empty, exit, unreachable) = (cfg.block(), cfg.block(), cfg.block(), cfg.block(), cfg.block());
    let targets = SwitchTargets::new([(0, circle), (1, rect), (2, empty)].into_iter(), unreachable);
    cfg.terminate(entry, TerminatorKind::SwitchInt { discr: Operand::Move(discr), targets }, span);

    let r = Operand::Copy(payload(&cfg, s, 0, 0));
    let r_squared = cfg.temp(i64t);
    cfg.assign(circle, r_squared, Rvalue::BinaryOp(BinOp::Mul, Box::new((r.clone(), r))), span);
    let three = Operand::const_from_scalar(tcx, i64t, Scalar::from_i64(3), span);
    cfg.assign(circle, ret, Rvalue::BinaryOp(BinOp::Mul, Box::new((three, Operand::Move(r_squared)))), span);
    cfg.terminate(circle, TerminatorKind::Goto { target: exit }, span);

    let (w, h) = (Operand::Copy(payload(&cfg, s, 1, 0)), Operand::Copy(payload(&cfg, s, 1, 1)));
    cfg.assign(rect, ret, Rvalue::BinaryOp(BinOp::Mul, Box::new((w, h))), span);
    cfg.terminate(rect, TerminatorKind::Goto { target: exit }, span);

    let zero = Operand::const_from_scalar(tcx, i64t, Scalar::from_i64(0), span);
    cfg.assign(empty, ret, Rvalue::Use(zero, WithRetag::Yes), span);
    cfg.terminate(empty, TerminatorKind::Goto { target: exit }, span);

    cfg.terminate(exit, TerminatorKind::Return, span);
    cfg.terminate(unreachable, TerminatorKind::Unreachable, span);
    cfg.finish()
}

/// `square(side: i64) -> Shape`: `return Rect(side, side)`, or `Empty` when `side` is 0.
fn square_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Body<'tcx> {
    let mut cfg = Cfg::new(tcx, def);
    let span = cfg.span;
    let side = cfg.arg(0);
    let shape = local_item(tcx, "Shape");
    let entry = BasicBlock::from_u32(0);
    let variant = |v: u32, fields: Vec<Operand<'tcx>>| {
        let kind = AggregateKind::Adt(shape, VariantIdx::from_u32(v), tcx.mk_args(&[]), None, None);
        Rvalue::Aggregate(Box::new(kind), IndexVec::from_raw(fields))
    };

    let (zero, nonzero, exit) = (cfg.block(), cfg.block(), cfg.block());
    let targets = SwitchTargets::static_if(0, zero, nonzero);
    cfg.terminate(entry, TerminatorKind::SwitchInt { discr: Operand::Copy(side), targets }, span);
    cfg.assign(zero, Place::return_place(), variant(2, vec![]), span);
    cfg.terminate(zero, TerminatorKind::Goto { target: exit }, span);
    cfg.assign(nonzero, Place::return_place(), variant(1, vec![Operand::Copy(side), Operand::Copy(side)]), span);
    cfg.terminate(nonzero, TerminatorKind::Goto { target: exit }, span);
    cfg.terminate(exit, TerminatorKind::Return, span);
    cfg.finish()
}

fn mir_built<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> &'tcx Steal<Body<'tcx>> {
    match tcx.opt_item_name(def.to_def_id()).map(|n| n.to_string()).as_deref() {
        Some("area") => tcx.alloc_steal_mir(area_body(tcx, def)),
        Some("square") => tcx.alloc_steal_mir(square_body(tcx, def)),
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
        let variants: [(&str, Vec<TySpec>); 3] = [("Circle", vec![t("i64")]), ("Rect", vec![t("i64"), t("i64")]), ("Empty", vec![])];
        krate.items.push(enumeration("Shape", &variants, span));
        krate.items.push(function("area", ast::Generics::default(), &[("s", t("Shape"))], t("i64"), span));
        krate.items.push(function("square", ast::Generics::default(), &[("side", t("i64"))], t("Shape"), span));
        rustc_driver::Compilation::Continue
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    rustc_driver::run_compiler(&args, &mut Callbacks);
}
