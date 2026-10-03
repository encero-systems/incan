//! Step 9's rustc seam: the narrow Rust layer that owns rustc's types. Built against rustc's internal interface, but
//! its public API holds no rustc type: Incan code fills a `BodyPlan` and calls `compile`, which runs rustc in-process.
#![feature(rustc_private)]
extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_index;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_span;
extern crate thin_vec;

use rustc_data_structures::steal::Steal;
use rustc_hir::def_id::LocalDefId;
use rustc_index::IndexVec;
use rustc_middle::mir::interpret::Scalar;
use rustc_middle::mir::*;
use rustc_middle::ty::TyCtxt;
use std::sync::Mutex;
use thin_vec::ThinVec;

/// Bodies the Incan side planned: each named function returns a constant.
pub struct BodyPlan {
    constants: Vec<(String, i64)>,
}

impl BodyPlan {
    /// An empty plan.
    pub fn new() -> BodyPlan {
        BodyPlan { constants: Vec::new() }
    }

    /// Plan `function`'s body as `return value`.
    pub fn add_constant_return(&mut self, function: &str, value: i64) {
        self.constants.push((function.to_string(), value));
    }

    /// How many bodies are planned.
    pub fn len(&self) -> i64 {
        self.constants.len() as i64
    }
}

/// The plan for the compilation in progress; query providers are plain function pointers.
static PLAN: Mutex<Vec<(String, i64)>> = Mutex::new(Vec::new());

fn planned(name: &str) -> Option<i64> {
    PLAN.lock().ok()?.iter().find(|(n, _)| n == name).map(|(_, v)| *v)
}

fn constant_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId, value: i64) -> Body<'tcx> {
    let span = tcx.def_span(def);
    let si = SourceInfo::outermost(span);
    let i64t = tcx.types.i64;
    let assign = Statement::new(si, StatementKind::Assign(Box::new((
        Place::return_place(),
        Rvalue::Use(Operand::const_from_scalar(tcx, i64t, Scalar::from_i64(value), span), WithRetag::Yes),
    ))));
    let ret = Terminator { source_info: si, kind: TerminatorKind::Return, attributes: ThinVec::new() };
    let blocks = IndexVec::from_raw(vec![BasicBlockData::new_stmts(vec![assign], Some(ret), false)]);
    let scopes = IndexVec::from_raw(vec![SourceScopeData { span, parent_scope: None, inlined: None, inlined_parent_scope: None, local_data: ClearCrossCrate::Clear }]);
    let locals = IndexVec::from_raw(vec![LocalDecl::new(i64t, span)]);
    Body::new(MirSource::item(def.to_def_id()), blocks, scopes, locals, IndexVec::new(), 0, vec![], span, None, None)
}

fn mir_built<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> &'tcx Steal<Body<'tcx>> {
    let name = tcx.opt_item_name(def.to_def_id()).map(|n| n.to_string());
    match name.as_deref().and_then(planned) {
        Some(value) => tcx.alloc_steal_mir(constant_body(tcx, def, value)),
        None => (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.mir_built)(tcx, def),
    }
}

struct Callbacks;
impl rustc_driver::Callbacks for Callbacks {
    fn config(&mut self, config: &mut rustc_interface::interface::Config) {
        config.override_queries = Some(|_sess, providers| providers.queries.mir_built = mir_built);
    }
}

/// Compile the Rust program at `input` to `output` with the planned bodies, in this process. Returns 0 on success.
pub fn compile(plan: &BodyPlan, input: &str, output: &str, sysroot: &str) -> i64 {
    if let Ok(mut slot) = PLAN.lock() {
        *slot = plan.constants.clone();
    }
    let args: Vec<String> = ["incan-driver", "--sysroot", sysroot, "--edition", "2024", "--cap-lints", "allow", input, "-o", output]
        .iter().map(|s| s.to_string()).collect();
    match rustc_driver::catch_fatal_errors(|| rustc_driver::run_compiler(&args, &mut Callbacks)) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}
