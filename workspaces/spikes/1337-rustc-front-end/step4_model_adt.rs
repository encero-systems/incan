//! Spike step 4: an Incan model is a real Rust struct. Rust constructs it and passes it to an Incan function whose MIR
//! reads its fields; Incan MIR constructs one and returns it to Rust. No conversion happens in either direction.
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

use rustc_abi::{FieldIdx, VariantIdx};
use rustc_ast as ast;
use rustc_data_structures::steal::Steal;
use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_index::IndexVec;
use rustc_middle::mir::interpret::Scalar;
use rustc_middle::mir::*;
use rustc_middle::ty::{Ty, TyCtxt};
use rustc_span::{Ident, Span, Symbol};
use thin_vec::{ThinVec, thin_vec};

// ---- Declarations: AST items, never source text ----

fn ident(name: &str, span: Span) -> Ident {
    Ident::new(Symbol::intern(name), span)
}

fn path_ty(name: &str, span: Span) -> Box<ast::Ty> {
    Box::new(ast::Ty { id: ast::DUMMY_NODE_ID, kind: ast::TyKind::Path(None, ast::Path::from_ident(ident(name, span))), span, tokens: None })
}

fn inherited(span: Span) -> ast::Visibility {
    ast::Visibility { kind: ast::VisibilityKind::Inherited, span, tokens: None }
}

fn item(kind: ast::ItemKind, span: Span) -> Box<ast::Item> {
    Box::new(ast::Item { attrs: ThinVec::new(), id: ast::DUMMY_NODE_ID, span, vis: inherited(span), kind, tokens: None })
}

/// `struct <name> { <field>: <ty>, .. }`
fn model(name: &str, fields: &[(&str, &str)], span: Span) -> Box<ast::Item> {
    let fields = fields.iter().map(|(f, t)| ast::FieldDef {
        attrs: ThinVec::new(), id: ast::DUMMY_NODE_ID, span, vis: inherited(span),
        mut_restriction: ast::MutRestriction { kind: ast::RestrictionKind::Unrestricted, span, tokens: None },
        safety: ast::Safety::Default, ident: Some(ident(f, span)), ty: path_ty(t, span), default: None, is_placeholder: false,
    }).collect();
    item(ast::ItemKind::Struct(ident(name, span), ast::Generics::default(), ast::VariantData::Struct { fields, recovered: ast::Recovered::No }), span)
}

/// `fn <name>(<param>: <ty>, ..) -> <ret> { loop {} }` -- the placeholder body is replaced by `mir_built`.
fn function(name: &str, params: &[(&str, &str)], ret: &str, span: Span) -> Box<ast::Item> {
    let id = ast::DUMMY_NODE_ID;
    let inputs = params.iter().map(|(p, t)| ast::Param {
        attrs: ThinVec::new(), ty: path_ty(t, span),
        pat: Box::new(ast::Pat { id, kind: ast::PatKind::Ident(ast::BindingMode::NONE, ident(p, span), None), span, tokens: None }),
        id, span, is_placeholder: false,
    }).collect();
    let empty = ast::Block { stmts: ThinVec::new(), id, rules: ast::BlockCheckMode::Default, span, tokens: None };
    let diverge = ast::Expr { id, kind: ast::ExprKind::Loop(Box::new(empty), None, span), span, attrs: ThinVec::new(), tokens: None };
    let body = ast::Block { stmts: thin_vec![ast::Stmt { id, kind: ast::StmtKind::Expr(Box::new(diverge)), span }], id, rules: ast::BlockCheckMode::Default, span, tokens: None };
    let sig = ast::FnSig { header: ast::FnHeader::default(), decl: Box::new(ast::FnDecl { inputs, output: ast::FnRetTy::Ty(path_ty(ret, span)) }), span };
    item(ast::ItemKind::Fn(Box::new(ast::Fn {
        defaultness: ast::Defaultness::Implicit, ident: ident(name, span), generics: ast::Generics::default(), sig,
        contract: None, define_opaque: None, body: Some(Box::new(body)), eii_impls: ThinVec::new(),
    })), span)
}

// ---- Bodies: MIR the front end builds ----

fn local_item(tcx: TyCtxt<'_>, name: &str) -> DefId {
    tcx.hir_crate_items(()).free_items().map(|i| i.owner_id.to_def_id())
        .find(|d| tcx.opt_item_name(*d).is_some_and(|n| n.as_str() == name))
        .unwrap_or_else(|| tcx.dcx().fatal(format!("the front end needs item `{name}`, and this crate has none")))
}

/// Locals for a body: the return place, then one per parameter, typed from the checked signature.
fn signature_locals<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId, span: Span) -> (IndexVec<Local, LocalDecl<'tcx>>, usize) {
    let sig = tcx.fn_sig(def).instantiate_identity().skip_binder();
    let mut locals = IndexVec::new();
    locals.push(LocalDecl::new(sig.output(), span));
    for input in sig.inputs() {
        locals.push(LocalDecl::new(*input, span));
    }
    (locals, sig.inputs().len())
}

fn assign<'tcx>(si: SourceInfo, place: Place<'tcx>, rvalue: Rvalue<'tcx>) -> Statement<'tcx> {
    Statement::new(si, StatementKind::Assign(Box::new((place, rvalue))))
}

fn finish<'tcx>(def: LocalDefId, statements: Vec<Statement<'tcx>>, locals: IndexVec<Local, LocalDecl<'tcx>>, args: usize, span: Span) -> Body<'tcx> {
    let si = SourceInfo::outermost(span);
    let ret = Terminator { source_info: si, kind: TerminatorKind::Return, attributes: ThinVec::new() };
    let blocks = IndexVec::from_raw(vec![BasicBlockData::new_stmts(statements, Some(ret), false)]);
    let scopes = IndexVec::from_raw(vec![SourceScopeData { span, parent_scope: None, inlined: None, inlined_parent_scope: None, local_data: ClearCrossCrate::Clear }]);
    Body::new(MirSource::item(def.to_def_id()), blocks, scopes, locals, IndexVec::new(), args, vec![], span, None, None)
}

/// `manhattan(p: Point) -> i64`: `return p.x + p.y` -- Incan reading the fields of a Rust-constructed model.
fn manhattan_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Body<'tcx> {
    let span = tcx.def_span(def);
    let (locals, args) = signature_locals(tcx, def, span);
    let i64t: Ty<'tcx> = tcx.types.i64;
    let p = Place::from(Local::from_u32(1));
    let x = tcx.mk_place_field(p, FieldIdx::from_u32(0), i64t);
    let y = tcx.mk_place_field(p, FieldIdx::from_u32(1), i64t);
    let sum = Rvalue::BinaryOp(BinOp::Add, Box::new((Operand::Copy(x), Operand::Copy(y))));
    finish(def, vec![assign(SourceInfo::outermost(span), Place::return_place(), sum)], locals, args, span)
}

/// `make_point(dx: i64) -> Point`: `return Point(x=dx, y=10)` -- Incan constructing a model Rust then reads.
fn make_point_body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Body<'tcx> {
    let span = tcx.def_span(def);
    let (locals, args) = signature_locals(tcx, def, span);
    let point = local_item(tcx, "Point");
    let fields: IndexVec<FieldIdx, Operand<'tcx>> = IndexVec::from_raw(vec![
        Operand::Copy(Place::from(Local::from_u32(1))),
        Operand::const_from_scalar(tcx, tcx.types.i64, Scalar::from_i64(10), span),
    ]);
    let kind = AggregateKind::Adt(point, VariantIdx::from_u32(0), tcx.mk_args(&[]), None, None);
    let value = Rvalue::Aggregate(Box::new(kind), fields);
    finish(def, vec![assign(SourceInfo::outermost(span), Place::return_place(), value)], locals, args, span)
}

fn mir_built<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> &'tcx Steal<Body<'tcx>> {
    match tcx.opt_item_name(def.to_def_id()).map(|n| n.to_string()).as_deref() {
        Some("manhattan") => tcx.alloc_steal_mir(manhattan_body(tcx, def)),
        Some("make_point") => tcx.alloc_steal_mir(make_point_body(tcx, def)),
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
        krate.items.push(model("Point", &[("x", "i64"), ("y", "i64")], span));
        krate.items.push(function("manhattan", &[("p", "Point")], "i64", span));
        krate.items.push(function("make_point", &[("dx", "i64")], "Point", span));
        rustc_driver::Compilation::Continue
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    rustc_driver::run_compiler(&args, &mut Callbacks);
}
