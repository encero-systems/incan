//! Spike step 8: #1337's four boundary rows, natively, across a three-crate graph built with no Cargo.
//!
//! - `host` is a Rust rlib built by plain rustc: `increment`, `text_len(&str)` and `apply(impl FnOnce)`.
//! - `policy` is the Incan library, built by this driver from **no Rust source at all**: the crate root is empty and
//!   every item is injected. Its functions call `host` (free call, borrowed coercion, owned callable) and it exposes
//!   them under RFC 097's caller namespace, `policy::caller::incan`.
//! - `app` is a Rust binary built by plain rustc that calls `policy` through that namespace (the Rust-hosted caller),
//!   including the generic `choose`, which rustc instantiates in `app` from the MIR `policy`'s metadata carries.
#![feature(rustc_private)]
extern crate rustc_abi;
extern crate rustc_ast;
extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_index;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;
extern crate thin_vec;

mod common;

use common::{Cfg, TySpec, closure_skeleton, closures_of, extern_item, extern_crate, function, function_with_closures, generics, glob_use, model, module, public, t};
use rustc_abi::FieldIdx;
use rustc_ast as ast;
use rustc_data_structures::steal::Steal;
use rustc_hir::def::DefKind;
use rustc_hir::def_id::LocalDefId;
use rustc_index::IndexVec;
use rustc_middle::mir::*;
use rustc_middle::ty::{self, Ty, TyCtxt};
use rustc_span::{FileName, Symbol};
use thin_vec::thin_vec;

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

/// Row 1, free call. `bump(value: int) -> int`: `return increment(value)`.
fn bump_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Body<'tcx> {
    let mut cfg = Cfg::new(tcx, def);
    let entry = BasicBlock::from_u32(0);
    let value = Operand::Copy(cfg.arg(0));
    let exit = cfg.call(entry, extern_item(tcx, &["host", "increment"]), &[], vec![value], Place::return_place());
    cfg.terminate(exit, TerminatorKind::Return, cfg.span);
    cfg.finish()
}

/// Row 2, borrowed coercion. `measure(text: str) -> int`: `return text_len(text)`. Incan's `str` is an owned
/// `String`; the checked projection to `&str` is a shared borrow and then `<String as Deref>::deref`, exactly the call
/// rustc's own `mir_build` makes for `&*text`. `text` is owned by `measure`, so it is dropped before returning.
fn measure_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Body<'tcx> {
    let mut cfg = Cfg::new(tcx, def);
    let span = cfg.span;
    let entry = BasicBlock::from_u32(0);
    let text = cfg.arg(0);
    let string_ty = text.ty(&cfg.locals, tcx).ty;
    let erased = tcx.lifetimes.re_erased;

    let borrowed = cfg.temp(Ty::new_imm_ref(tcx, erased, string_ty));
    cfg.assign(entry, borrowed, Rvalue::Ref(erased, BorrowKind::Shared, text), span);
    let Some(deref_trait) = tcx.lang_items().deref_trait() else { tcx.dcx().fatal("this sysroot has no `Deref` lang item") };
    let Some(deref) = tcx.associated_item_def_ids(deref_trait).iter().copied().find(|d| tcx.item_name(*d) == Symbol::intern("deref")) else {
        tcx.dcx().fatal("`Deref` has no `deref` method");
    };
    let as_str = cfg.temp(Ty::new_imm_ref(tcx, erased, tcx.types.str_));
    let after_deref = cfg.call(entry, deref, &[string_ty.into()], vec![Operand::Move(borrowed)], as_str);
    let after_call = cfg.call(after_deref, extern_item(tcx, &["host", "text_len"]), &[], vec![Operand::Move(as_str)], Place::return_place());

    let exit = cfg.block();
    let drop = TerminatorKind::Drop { place: text, target: exit, unwind: UnwindAction::Continue, replace: false, drop: None };
    cfg.terminate(after_call, drop, span);
    cfg.terminate(exit, TerminatorKind::Return, span);
    cfg.finish()
}

/// Row 3, owned callable. `shift(value: int, offset: int) -> int`: `return apply(value, (x) => x + offset)`. The
/// lambda is a real Rust closure: its `DefId` and capture list come from the skeleton in `shift`'s placeholder body,
/// and `apply`'s `impl FnOnce` parameter is instantiated at its closure type.
fn shift_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Body<'tcx> {
    let mut cfg = Cfg::new(tcx, def);
    let span = cfg.span;
    let entry = BasicBlock::from_u32(0);
    let Some(&lambda) = closures_of(tcx, def).first() else { tcx.dcx().fatal("`shift` declares no closure") };
    let closure_ty = tcx.type_of(lambda).instantiate_identity().skip_normalization();
    let ty::Closure(_, closure_args) = closure_ty.kind() else { tcx.dcx().fatal("the lambda is not a closure") };

    let callback = cfg.temp(closure_ty);
    let kind = AggregateKind::Closure(lambda.to_def_id(), closure_args);
    let offset = Operand::Copy(cfg.arg(1));
    cfg.assign(entry, callback, Rvalue::Aggregate(Box::new(kind), IndexVec::from_raw(vec![offset])), span);
    let value = Operand::Copy(cfg.arg(0));
    let exit = cfg.call(entry, extern_item(tcx, &["host", "apply"]), &[closure_ty.into()], vec![value, Operand::Move(callback)], Place::return_place());
    cfg.terminate(exit, TerminatorKind::Return, span);
    cfg.finish()
}

/// The lambda's own body: `x + offset`, reading the captured `offset` through the closure environment.
fn lambda_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Body<'tcx> {
    let mut cfg = Cfg::new(tcx, def);
    let span = cfg.span;
    let entry = BasicBlock::from_u32(0);
    let (env, x) = (cfg.arg(0), cfg.arg(1));
    let captured = tcx.mk_place_field(tcx.mk_place_deref(env), FieldIdx::from_u32(0), tcx.types.i64);
    cfg.assign(entry, Place::return_place(), Rvalue::BinaryOp(BinOp::Add, Box::new((Operand::Copy(x), Operand::Copy(captured)))), span);
    cfg.terminate(entry, TerminatorKind::Return, span);
    cfg.finish()
}

/// `choose[T: Copy](pair: Pair[T], take_first: bool) -> T`, as in step 6; here a Rust crate downstream instantiates it.
fn choose_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Body<'tcx> {
    let mut cfg = Cfg::new(tcx, def);
    let span = cfg.span;
    let (pair, take_first) = (cfg.arg(0), cfg.arg(1));
    let entry = BasicBlock::from_u32(0);
    let (first, second, exit) = (cfg.block(), cfg.block(), cfg.block());
    cfg.terminate(entry, TerminatorKind::SwitchInt { discr: Operand::Copy(take_first), targets: SwitchTargets::static_if(0, second, first) }, span);
    for (block, index) in [(first, 0), (second, 1)] {
        let value = Operand::Copy(field(&cfg, pair, index));
        cfg.assign(block, Place::return_place(), Rvalue::Use(value, WithRetag::Yes), span);
        cfg.terminate(block, TerminatorKind::Goto { target: exit }, span);
    }
    cfg.terminate(exit, TerminatorKind::Return, span);
    cfg.finish()
}

fn mir_built<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> &'tcx Steal<Body<'tcx>> {
    if tcx.def_kind(def) == DefKind::Closure {
        return tcx.alloc_steal_mir(lambda_body(tcx, def));
    }
    match tcx.opt_item_name(def.to_def_id()).map(|n| n.to_string()).as_deref() {
        Some("bump") => tcx.alloc_steal_mir(bump_body(tcx, def)),
        Some("measure") => tcx.alloc_steal_mir(measure_body(tcx, def)),
        Some("shift") => tcx.alloc_steal_mir(shift_body(tcx, def)),
        Some("choose") => tcx.alloc_steal_mir(choose_body(tcx, def)),
        _ => (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.mir_built)(tcx, def),
    }
}

struct Callbacks;
impl rustc_driver::Callbacks for Callbacks {
    /// The crate root is empty: no Rust source exists for the Incan library, under any name.
    fn config(&mut self, config: &mut rustc_interface::interface::Config) {
        config.input = rustc_session::config::Input::Str { name: FileName::Custom("policy.incn".to_string()), input: String::new() };
        config.override_queries = Some(|_sess, providers| providers.queries.mir_built = mir_built);
    }
    fn after_crate_root_parsing(&mut self, _c: &rustc_interface::interface::Compiler, krate: &mut ast::Crate) -> rustc_driver::Compilation {
        let span = krate.spans.inner_span;
        let pair_of_t = || TySpec("Pair", vec![t("T")]);
        let lambda = closure_skeleton(&[("x", t("i64"))], t("i64"), &["offset"], span);
        let scores = thin_vec![
            public(model("Pair", generics(&[("T", &[])], span), &[("first", t("T")), ("second", t("T"))], span)),
            public(function("bump", ast::Generics::default(), &[("value", t("i64"))], t("i64"), span)),
            public(function("measure", ast::Generics::default(), &[("text", t("String"))], t("i64"), span)),
            public(function_with_closures("shift", ast::Generics::default(), &[("value", t("i64")), ("offset", t("i64"))], t("i64"), thin_vec![lambda], span)),
            public(function("choose", generics(&[("T", &["Copy"])], span), &[("pair", pair_of_t()), ("take_first", t("bool"))], t("T"), span)),
        ];
        let caller = public(module("caller", thin_vec![public(module("incan", thin_vec![public(glob_use(&["crate", "scores"], span))], span))], span));
        krate.items.push(extern_crate("host", span));
        krate.items.push(public(module("scores", scores, span)));
        krate.items.push(caller);
        rustc_driver::Compilation::Continue
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    rustc_driver::run_compiler(&args, &mut Callbacks);
}
