//! Spike step 7: spans point at `.incn` sources. The driver loads `scores.incn` into rustc's source map, declares
//! `add_scores` at its `def` line and gives the MIR for `a + b` the span of that expression. When the addition
//! overflows, the panic names the Incan file, line and column, because rustc derives panic locations from MIR spans.
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

use common::{Cfg, function, t};
use rustc_abi::FieldIdx;
use rustc_ast as ast;
use rustc_data_structures::steal::Steal;
use rustc_hir::def_id::LocalDefId;
use rustc_middle::mir::*;
use rustc_middle::ty::{Ty, TyCtxt};
use rustc_span::{BytePos, Span};
use std::path::PathBuf;
use std::sync::OnceLock;

/// The span of `a + b` in the loaded `.incn` file. Query providers are plain function pointers, so the span the
/// parsing callback found reaches `mir_built` through this cell rather than through a closure.
static ADDITION: OnceLock<Span> = OnceLock::new();

/// The span of the first occurrence of `needle` in the source file starting at `start`.
fn span_of(source: &str, start: BytePos, needle: &str) -> Option<Span> {
    let offset = u32::try_from(source.find(needle)?).ok()?;
    let len = u32::try_from(needle.len()).ok()?;
    Some(Span::with_root_ctxt(start + BytePos(offset), start + BytePos(offset + len)))
}

/// `add_scores(a: i64, b: i64) -> i64`: `return a + b` with Incan's overflow check, as rustc's own `mir_build`
/// lowers a checked addition: `AddWithOverflow` into a `(i64, bool)` temporary, then an `Assert` on the flag.
fn add_scores_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Body<'tcx> {
    let mut cfg = Cfg::new(tcx, def);
    let span = ADDITION.get().copied().unwrap_or(cfg.span);
    let (a, b) = (Operand::Copy(cfg.arg(0)), Operand::Copy(cfg.arg(1)));
    let entry = BasicBlock::from_u32(0);
    let checked = cfg.temp(Ty::new_tup(tcx, &[tcx.types.i64, tcx.types.bool]));
    cfg.assign(entry, checked, Rvalue::BinaryOp(BinOp::AddWithOverflow, Box::new((a.clone(), b.clone()))), span);

    let exit = cfg.block();
    let overflowed = Operand::Move(tcx.mk_place_field(checked, FieldIdx::from_u32(1), tcx.types.bool));
    let msg = Box::new(AssertKind::Overflow(BinOp::Add, a, b));
    cfg.terminate(entry, TerminatorKind::Assert { cond: overflowed, expected: false, msg, target: exit, unwind: UnwindAction::Continue }, span);
    let sum = Operand::Move(tcx.mk_place_field(checked, FieldIdx::from_u32(0), tcx.types.i64));
    cfg.assign(exit, Place::return_place(), Rvalue::Use(sum, WithRetag::Yes), span);
    cfg.terminate(exit, TerminatorKind::Return, span);
    cfg.finish()
}

fn mir_built<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> &'tcx Steal<Body<'tcx>> {
    match tcx.opt_item_name(def.to_def_id()).map(|n| n.to_string()).as_deref() {
        Some("add_scores") => tcx.alloc_steal_mir(add_scores_body(tcx, def)),
        _ => (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.mir_built)(tcx, def),
    }
}

struct Callbacks {
    source: PathBuf,
}

impl rustc_driver::Callbacks for Callbacks {
    fn config(&mut self, config: &mut rustc_interface::interface::Config) {
        config.override_queries = Some(|_sess, providers| providers.queries.mir_built = mir_built);
    }
    fn after_crate_root_parsing(&mut self, compiler: &rustc_interface::interface::Compiler, krate: &mut ast::Crate) -> rustc_driver::Compilation {
        let dcx = compiler.sess.dcx();
        let file = compiler.sess.source_map().load_file(&self.source)
            .unwrap_or_else(|e| dcx.fatal(format!("cannot load Incan source {}: {e}", self.source.display())));
        let Some(text) = file.src.as_deref() else { dcx.fatal("the loaded Incan source has no text") };
        let (Some(declaration), Some(addition)) = (span_of(text, file.start_pos, "def add_scores"), span_of(text, file.start_pos, "a + b")) else {
            dcx.fatal("scores.incn does not contain `add_scores` as the spike expects");
        };
        let _ = ADDITION.set(addition);
        krate.items.push(function("add_scores", ast::Generics::default(), &[("a", t("i64")), ("b", t("i64"))], t("i64"), declaration));
        rustc_driver::Compilation::Continue
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(source) = std::env::var_os("INCAN_SOURCE").map(PathBuf::from) else {
        eprintln!("set INCAN_SOURCE to the .incn file to load");
        std::process::exit(2);
    };
    rustc_driver::run_compiler(&args, &mut Callbacks { source });
}
