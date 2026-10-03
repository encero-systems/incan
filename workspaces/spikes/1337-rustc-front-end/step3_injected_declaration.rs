//! Spike step 1+4: `answer` is declared by injecting an AST item (no source text), its body is MIR the front end
//! builds, and that MIR calls a user-written Rust function. Rust's `main` calls `answer` back.
#![feature(rustc_private)]
extern crate rustc_ast;
extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_index;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_span;
extern crate thin_vec;

use rustc_ast as ast;
use rustc_data_structures::steal::Steal;
use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_index::IndexVec;
use rustc_middle::mir::interpret::Scalar;
use rustc_middle::mir::*;
use rustc_middle::ty::TyCtxt;
use rustc_span::{Ident, Span, Spanned, Symbol};
use thin_vec::{ThinVec, thin_vec};

/// Declare `fn answer() -> i64` as an AST node. The placeholder body only has to type-check; it is never compiled,
/// because `mir_built` below replaces it.
fn answer_decl(span: Span) -> ast::Item {
    let id = ast::DUMMY_NODE_ID;
    let i64_ty = ast::Ty { id, kind: ast::TyKind::Path(None, ast::Path::from_ident(Ident::new(Symbol::intern("i64"), span))), span, tokens: None };
    let empty = ast::Block { stmts: ThinVec::new(), id, rules: ast::BlockCheckMode::Default, span, tokens: None };
    let diverge = ast::Expr { id, kind: ast::ExprKind::Loop(Box::new(empty), None, span), span, attrs: ThinVec::new(), tokens: None };
    let body = ast::Block {
        stmts: thin_vec![ast::Stmt { id, kind: ast::StmtKind::Expr(Box::new(diverge)), span }],
        id, rules: ast::BlockCheckMode::Default, span, tokens: None,
    };
    let sig = ast::FnSig {
        header: ast::FnHeader::default(),
        decl: Box::new(ast::FnDecl { inputs: ThinVec::new(), output: ast::FnRetTy::Ty(Box::new(i64_ty)) }),
        span,
    };
    let f = ast::Fn {
        defaultness: ast::Defaultness::Implicit, ident: Ident::new(Symbol::intern("answer"), span),
        generics: ast::Generics::default(), sig, contract: None, define_opaque: None,
        body: Some(Box::new(body)), eii_impls: ThinVec::new(),
    };
    ast::Item {
        attrs: ThinVec::new(), id, span,
        vis: ast::Visibility { kind: ast::VisibilityKind::Inherited, span, tokens: None },
        kind: ast::ItemKind::Fn(Box::new(f)), tokens: None,
    }
}

/// Find a free function in this crate by name -- standing in for the checked call plan's canonical Rust identity.
fn local_fn(tcx: TyCtxt<'_>, name: &str) -> DefId {
    tcx.hir_crate_items(()).free_items().map(|i| i.owner_id.to_def_id())
        .find(|d| tcx.opt_item_name(*d).is_some_and(|n| n.as_str() == name))
        .unwrap_or_else(|| tcx.dcx().fatal(format!("the front end calls Rust item `{name}`, and this crate has none")))
}

/// Body of `answer`: `return double(21)`, where `double` is user-written Rust in the same crate graph.
fn answer_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Body<'tcx> {
    let span = tcx.def_span(def);
    let si = SourceInfo::outermost(span);
    let i64t = tcx.types.i64;
    let call = Terminator {
        source_info: si,
        kind: TerminatorKind::Call {
            func: Operand::function_handle(tcx, local_fn(tcx, "double"), [], span),
            args: Box::new([Spanned { node: Operand::const_from_scalar(tcx, i64t, Scalar::from_i64(21), span), span }]),
            destination: Place::return_place(),
            target: Some(BasicBlock::from_u32(1)),
            unwind: UnwindAction::Continue,
            call_source: CallSource::Normal,
            fn_span: span,
        },
        attributes: ThinVec::new(),
    };
    let ret = Terminator { source_info: si, kind: TerminatorKind::Return, attributes: ThinVec::new() };
    let blocks = IndexVec::from_raw(vec![BasicBlockData::new(Some(call), false), BasicBlockData::new(Some(ret), false)]);
    let locals = IndexVec::from_raw(vec![LocalDecl::new(i64t, span)]);
    let scopes = IndexVec::from_raw(vec![SourceScopeData {
        span, parent_scope: None, inlined: None, inlined_parent_scope: None, local_data: ClearCrossCrate::Clear,
    }]);
    Body::new(MirSource::item(def.to_def_id()), blocks, scopes, locals, IndexVec::new(), 0, vec![], span, None, None)
}

fn mir_built<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> &'tcx Steal<Body<'tcx>> {
    if tcx.opt_item_name(def.to_def_id()).is_some_and(|n| n.as_str() == "answer") {
        eprintln!("[front end] supplying MIR for `answer`");
        return tcx.alloc_steal_mir(answer_body(tcx, def));
    }
    (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.mir_built)(tcx, def)
}

struct Callbacks;
impl rustc_driver::Callbacks for Callbacks {
    fn config(&mut self, config: &mut rustc_interface::interface::Config) {
        config.override_queries = Some(|_sess, providers| providers.queries.mir_built = mir_built);
    }
    fn after_crate_root_parsing(&mut self, _c: &rustc_interface::interface::Compiler, krate: &mut ast::Crate) -> rustc_driver::Compilation {
        eprintln!("[front end] declaring `answer` as an AST item (no source text)");
        krate.items.push(Box::new(answer_decl(krate.spans.inner_span)));
        rustc_driver::Compilation::Continue
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    rustc_driver::run_compiler(&args, &mut Callbacks);
}
