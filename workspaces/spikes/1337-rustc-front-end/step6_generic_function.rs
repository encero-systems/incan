//! Spike step 6: generic Incan functions over a generic Incan model. `choose` is declared with a type parameter and
//! a trait bound, and its MIR is written once against that parameter. Rust instantiates it at `&str` and `f64`;
//! Incan's own `larger_of` instantiates it at `i64`. rustc monomorphizes every instance.
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

use common::{Cfg, TySpec, function, generics, local_item, model, t};
use rustc_abi::{FieldIdx, VariantIdx};
use rustc_ast as ast;
use rustc_data_structures::steal::Steal;
use rustc_hir::def_id::LocalDefId;
use rustc_index::IndexVec;
use rustc_middle::mir::*;
use rustc_middle::ty::{self, TyCtxt};
use rustc_span::Spanned;

/// Field `index` of the model in `place`, typed from the checked ADT and its (possibly generic) arguments.
fn field<'tcx>(cfg: &Cfg<'tcx>, place: Place<'tcx>, index: u32) -> Place<'tcx> {
    let tcx = cfg.tcx;
    let ty::Adt(adt, args) = place.ty(&cfg.locals, tcx).ty.kind() else {
        tcx.dcx().fatal("the front end read a field of a value that is not a model");
    };
    let field = FieldIdx::from_u32(index);
    // Incan field types name no associated types, so there is nothing to normalize.
    let field_ty = adt.non_enum_variant().fields[field].ty(tcx, args).skip_normalization();
    tcx.mk_place_field(place, field, field_ty)
}

/// `choose[T: Copy](pair: Pair[T], take_first: bool) -> T`:
/// ```incan
/// if take_first:
///     return pair.first
/// return pair.second
/// ```
/// Written once; `T` stays a type parameter in the MIR.
fn choose_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Body<'tcx> {
    let mut cfg = Cfg::new(tcx, def);
    let span = cfg.span;
    let (pair, take_first) = (cfg.arg(0), cfg.arg(1));
    let entry = BasicBlock::from_u32(0);
    let (first, second, exit) = (cfg.block(), cfg.block(), cfg.block());
    let targets = SwitchTargets::static_if(0, second, first);
    cfg.terminate(entry, TerminatorKind::SwitchInt { discr: Operand::Copy(take_first), targets }, span);
    for (block, index) in [(first, 0), (second, 1)] {
        let value = Operand::Copy(field(&cfg, pair, index));
        cfg.assign(block, Place::return_place(), Rvalue::Use(value, WithRetag::Yes), span);
        cfg.terminate(block, TerminatorKind::Goto { target: exit }, span);
    }
    cfg.terminate(exit, TerminatorKind::Return, span);
    cfg.finish()
}

/// `larger_of(a: i64, b: i64) -> i64`: `return choose(Pair(first=a, second=b), a > b)` -- Incan building a generic
/// model at `i64` and calling a generic Incan function at `i64`.
fn larger_of_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Body<'tcx> {
    let mut cfg = Cfg::new(tcx, def);
    let span = cfg.span;
    let (a, b) = (cfg.arg(0), cfg.arg(1));
    let entry = BasicBlock::from_u32(0);
    let i64_args = tcx.mk_args(&[tcx.types.i64.into()]);

    let pair_def = local_item(tcx, "Pair");
    let pair = cfg.temp(ty::Ty::new_adt(tcx, tcx.adt_def(pair_def), i64_args));
    let kind = AggregateKind::Adt(pair_def, VariantIdx::from_u32(0), i64_args, None, None);
    cfg.assign(entry, pair, Rvalue::Aggregate(Box::new(kind), IndexVec::from_raw(vec![Operand::Copy(a), Operand::Copy(b)])), span);
    let a_wins = cfg.temp(tcx.types.bool);
    cfg.assign(entry, a_wins, Rvalue::BinaryOp(BinOp::Gt, Box::new((Operand::Copy(a), Operand::Copy(b)))), span);

    let exit = cfg.block();
    let arg = |place| Spanned { node: Operand::Move(place), span };
    let call = TerminatorKind::Call {
        func: Operand::function_handle(tcx, local_item(tcx, "choose"), i64_args.iter(), span),
        args: Box::new([arg(pair), arg(a_wins)]),
        destination: Place::return_place(),
        target: Some(exit),
        unwind: UnwindAction::Continue,
        call_source: CallSource::Normal,
        fn_span: span,
    };
    cfg.terminate(entry, call, span);
    cfg.terminate(exit, TerminatorKind::Return, span);
    cfg.finish()
}

fn mir_built<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> &'tcx Steal<Body<'tcx>> {
    match tcx.opt_item_name(def.to_def_id()).map(|n| n.to_string()).as_deref() {
        Some("choose") => tcx.alloc_steal_mir(choose_body(tcx, def)),
        Some("larger_of") => tcx.alloc_steal_mir(larger_of_body(tcx, def)),
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
        let pair_of_t = || TySpec("Pair".to_string(), vec![t("T")]);
        krate.items.push(model("Pair", generics(&[("T", &[])], span), &[("first", t("T")), ("second", t("T"))], span));
        let params = [("pair", pair_of_t()), ("take_first", t("bool"))];
        krate.items.push(function("choose", generics(&[("T", &["Copy"])], span), &params, t("T"), span));
        krate.items.push(function("larger_of", ast::Generics::default(), &[("a", t("i64")), ("b", t("i64"))], t("i64"), span));
        rustc_driver::Compilation::Continue
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    rustc_driver::run_compiler(&args, &mut Callbacks);
}
