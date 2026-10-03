//! Spike step 2: a function's body comes from MIR the front end constructs, never from Rust source.
#![feature(rustc_private)]
extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_index;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_span;

use rustc_data_structures::steal::Steal;
use rustc_hir::def_id::LocalDefId;
use rustc_index::IndexVec;
use rustc_middle::mir::interpret::Scalar;
use rustc_middle::mir::*;
use rustc_middle::ty::TyCtxt;
use rustc_span::Spanned;
use rustc_span::Symbol;
use thin_vec::ThinVec;
extern crate thin_vec;

/// Build the body of `answer`: `return core::cmp::max::<i64>(41, 42)`.
fn answer_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Body<'tcx> {
    let span = tcx.def_span(def);
    let si = SourceInfo::outermost(span);
    let i64t = tcx.types.i64;
    let Some(max) = tcx.get_diagnostic_item(Symbol::intern("cmp_max")) else {
        tcx.dcx().fatal("the front end needs `core::cmp::max`, and this sysroot does not name it");
    };
    let lit = |v: i64| Spanned { node: Operand::const_from_scalar(tcx, i64t, Scalar::from_i64(v), span), span };
    let call = Terminator {
        source_info: si,
        kind: TerminatorKind::Call {
            func: Operand::function_handle(tcx, max, [i64t.into()], span),
            args: Box::new([lit(41), lit(42)]),
            destination: Place::return_place(),
            target: Some(BasicBlock::from_u32(1)),
            unwind: UnwindAction::Continue,
            call_source: CallSource::Normal,
            fn_span: span,
        },
        attributes: ThinVec::new(),
    };
    let ret = Terminator { source_info: si, kind: TerminatorKind::Return, attributes: ThinVec::new() };
    let blocks: IndexVec<BasicBlock, BasicBlockData<'tcx>> =
        IndexVec::from_raw(vec![BasicBlockData::new(Some(call), false), BasicBlockData::new(Some(ret), false)]);
    let locals: IndexVec<Local, LocalDecl<'tcx>> = IndexVec::from_raw(vec![LocalDecl::new(i64t, span)]);
    let scopes = IndexVec::from_raw(vec![SourceScopeData {
        span, parent_scope: None, inlined: None, inlined_parent_scope: None, local_data: ClearCrossCrate::Clear,
    }]);
    Body::new(MirSource::item(def.to_def_id()), blocks, scopes, locals, IndexVec::new(), 0, vec![], span, None, None)
}

fn mir_built<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> &'tcx Steal<Body<'tcx>> {
    if tcx.opt_item_name(def.to_def_id()).is_some_and(|n| n.as_str() == "answer") {
        eprintln!("[front end] supplying MIR for `answer` (no Rust source body used)");
        return tcx.alloc_steal_mir(answer_body(tcx, def));
    }
    (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.mir_built)(tcx, def)
}

struct Callbacks;
impl rustc_driver::Callbacks for Callbacks {
    fn config(&mut self, config: &mut rustc_interface::interface::Config) {
        config.override_queries = Some(|_sess, providers| providers.queries.mir_built = mir_built);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    rustc_driver::run_compiler(&args, &mut Callbacks);
}
