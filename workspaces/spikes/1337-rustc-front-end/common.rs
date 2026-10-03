//! Shared by spike steps 5 and later: AST builders for declarations and a small CFG builder for MIR bodies.
//! Each step is still its own driver; this module is included with `mod common;`.

use rustc_ast as ast;
use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_index::IndexVec;
use rustc_middle::mir::*;
use rustc_middle::ty::{self, Ty, TyCtxt};
use rustc_span::{Ident, Span, Symbol};
use thin_vec::{ThinVec, thin_vec};

// ---- Declarations: AST items, never source text ----

/// A type as the front end names it: a path with optional generic arguments, `Name` or `Name<A, B>`.
pub struct TySpec(pub &'static str, pub Vec<TySpec>);

pub fn t(name: &'static str) -> TySpec {
    TySpec(name, Vec::new())
}

pub fn ident(name: &str, span: Span) -> Ident {
    Ident::new(Symbol::intern(name), span)
}

fn segment(name: &str, args: &[TySpec], span: Span) -> ast::PathSegment {
    let args = (!args.is_empty()).then(|| {
        let args = args.iter().map(|a| ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty(a, span)))).collect();
        Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs { span, args }))
    });
    ast::PathSegment { ident: ident(name, span), id: ast::DUMMY_NODE_ID, args }
}

pub fn ty(spec: &TySpec, span: Span) -> Box<ast::Ty> {
    let path = ast::Path { span, segments: thin_vec![segment(spec.0, &spec.1, span)], tokens: None };
    Box::new(ast::Ty { id: ast::DUMMY_NODE_ID, kind: ast::TyKind::Path(None, path), span, tokens: None })
}

fn inherited(span: Span) -> ast::Visibility {
    ast::Visibility { kind: ast::VisibilityKind::Inherited, span, tokens: None }
}

fn item(kind: ast::ItemKind, span: Span) -> Box<ast::Item> {
    Box::new(ast::Item { attrs: ThinVec::new(), id: ast::DUMMY_NODE_ID, span, vis: inherited(span), kind, tokens: None })
}

/// `<T: Bound + .., U, ..>`: each parameter with its trait bounds, resolved through the prelude.
pub fn generics(params: &[(&str, &[&str])], span: Span) -> ast::Generics {
    let params = params.iter().map(|(name, bounds)| ast::GenericParam {
        id: ast::DUMMY_NODE_ID, ident: ident(name, span), attrs: ThinVec::new(), is_placeholder: false,
        bounds: bounds.iter().map(|b| ast::GenericBound::Trait(ast::PolyTraitRef::new(
            ThinVec::new(), ast::Path::from_ident(ident(b, span)), ast::TraitBoundModifiers::NONE, span, ast::Parens::No,
        ))).collect(),
        kind: ast::GenericParamKind::Type { default: None }, colon_span: None,
    }).collect();
    ast::Generics { params, where_clause: ast::WhereClause { has_where_token: false, predicates: ThinVec::new(), span }, span }
}

/// A field; `name: None` makes it positional, as in a tuple variant.
fn field(name: Option<&str>, spec: &TySpec, span: Span) -> ast::FieldDef {
    ast::FieldDef {
        attrs: ThinVec::new(), id: ast::DUMMY_NODE_ID, span, vis: inherited(span),
        mut_restriction: ast::MutRestriction { kind: ast::RestrictionKind::Unrestricted, span, tokens: None },
        safety: ast::Safety::Default, ident: name.map(|n| ident(n, span)), ty: ty(spec, span), default: None, is_placeholder: false,
    }
}

/// `struct <name><generics> { <field>: <ty>, .. }`
pub fn model(name: &str, generics: ast::Generics, fields: &[(&str, TySpec)], span: Span) -> Box<ast::Item> {
    let fields = fields.iter().map(|(f, spec)| field(Some(f), spec, span)).collect();
    item(ast::ItemKind::Struct(ident(name, span), generics, ast::VariantData::Struct { fields, recovered: ast::Recovered::No }), span)
}

/// `enum <name> { <Variant>(<ty>, ..), <Unit>, .. }`: an Incan enum, whose variants carry positional payloads.
pub fn enumeration(name: &str, variants: &[(&str, Vec<TySpec>)], span: Span) -> Box<ast::Item> {
    let variants = variants.iter().map(|(v, payload)| ast::Variant {
        attrs: ThinVec::new(), id: ast::DUMMY_NODE_ID, span, vis: inherited(span), ident: ident(v, span),
        data: if payload.is_empty() {
            ast::VariantData::Unit(ast::DUMMY_NODE_ID)
        } else {
            ast::VariantData::Tuple(payload.iter().map(|spec| field(None, spec, span)).collect(), ast::DUMMY_NODE_ID)
        },
        disr_expr: None, is_placeholder: false,
    }).collect();
    item(ast::ItemKind::Enum(ident(name, span), ast::Generics::default(), ast::EnumDef { variants }), span)
}

/// `fn <name><generics>(<param>: <ty>, ..) -> <ret> { loop {} }` -- the placeholder body is replaced by `mir_built`.
pub fn function(name: &str, generics: ast::Generics, params: &[(&str, TySpec)], ret: TySpec, span: Span) -> Box<ast::Item> {
    function_with_closures(name, generics, params, ret, ThinVec::new(), span)
}

fn expr(kind: ast::ExprKind, span: Span) -> Box<ast::Expr> {
    Box::new(ast::Expr { id: ast::DUMMY_NODE_ID, kind, span, attrs: ThinVec::new(), tokens: None })
}

fn block(stmts: ThinVec<ast::Stmt>, span: Span) -> Box<ast::Block> {
    Box::new(ast::Block { stmts, id: ast::DUMMY_NODE_ID, rules: ast::BlockCheckMode::Default, span, tokens: None })
}

/// `{ <stmts> loop {} }`: a body that diverges, so it type-checks against any return type.
fn placeholder(mut stmts: ThinVec<ast::Stmt>, span: Span) -> Box<ast::Block> {
    let diverge = expr(ast::ExprKind::Loop(block(ThinVec::new(), span), None, span), span);
    stmts.push(ast::Stmt { id: ast::DUMMY_NODE_ID, kind: ast::StmtKind::Expr(diverge), span });
    block(stmts, span)
}

fn params(list: &[(&str, TySpec)], span: Span) -> ThinVec<ast::Param> {
    let id = ast::DUMMY_NODE_ID;
    list.iter().map(|(p, spec)| ast::Param {
        attrs: ThinVec::new(), ty: ty(spec, span),
        pat: Box::new(ast::Pat { id, kind: ast::PatKind::Ident(ast::BindingMode::NONE, ident(p, span), None), span, tokens: None }),
        id, span, is_placeholder: false,
    }).collect()
}

/// `move |<params>| -> <ret> { <capture>; .. loop {} };` -- a closure skeleton inside a function's placeholder body.
/// It gives the closure a `DefId`, and typeck infers its kind and upvars from the captured names it mentions, so the
/// capture list is part of the declaration Incan supplies. The real closure body is MIR, supplied like any other.
pub fn closure_skeleton(params_list: &[(&str, TySpec)], ret: TySpec, captures: &[&str], span: Span) -> ast::Stmt {
    let reads = captures.iter().map(|c| ast::Stmt {
        id: ast::DUMMY_NODE_ID, kind: ast::StmtKind::Semi(expr(ast::ExprKind::Path(None, ast::Path::from_ident(ident(c, span))), span)), span,
    }).collect();
    let body = expr(ast::ExprKind::Block(placeholder(reads, span), None), span);
    let closure = ast::Closure {
        binder: ast::ClosureBinder::NotPresent, capture_clause: ast::CaptureBy::Value { move_kw: span }, constness: ast::Const::No,
        coroutine_kind: None, movability: ast::Movability::Movable,
        fn_decl: Box::new(ast::FnDecl { inputs: params(params_list, span), output: ast::FnRetTy::Ty(ty(&ret, span)) }),
        body, fn_decl_span: span, fn_arg_span: span,
    };
    ast::Stmt { id: ast::DUMMY_NODE_ID, kind: ast::StmtKind::Semi(expr(ast::ExprKind::Closure(Box::new(closure)), span)), span }
}

/// A function whose placeholder body holds closure skeletons ahead of its diverging `loop {}`.
pub fn function_with_closures(name: &str, generics: ast::Generics, params_list: &[(&str, TySpec)], ret: TySpec, closures: ThinVec<ast::Stmt>, span: Span) -> Box<ast::Item> {
    let inputs = params(params_list, span);
    let body = placeholder(closures, span);
    let sig = ast::FnSig { header: ast::FnHeader::default(), decl: Box::new(ast::FnDecl { inputs, output: ast::FnRetTy::Ty(ty(&ret, span)) }), span };
    item(ast::ItemKind::Fn(Box::new(ast::Fn {
        defaultness: ast::Defaultness::Implicit, ident: ident(name, span), generics, sig,
        contract: None, define_opaque: None, body: Some(body), eii_impls: ThinVec::new(),
    })), span)
}

/// Makes an item `pub`, and a model's fields with it, as an Incan `pub` export is.
pub fn public(mut item: Box<ast::Item>) -> Box<ast::Item> {
    let span = item.span;
    item.vis = ast::Visibility { kind: ast::VisibilityKind::Public, span, tokens: None };
    if let ast::ItemKind::Struct(_, _, ast::VariantData::Struct { fields, .. }) = &mut item.kind {
        for field in fields.iter_mut() {
            field.vis = ast::Visibility { kind: ast::VisibilityKind::Public, span, tokens: None };
        }
    }
    item
}

/// `mod <name> { <items> }`, inline.
pub fn module(name: &str, items: ThinVec<Box<ast::Item>>, span: Span) -> Box<ast::Item> {
    let spans = ast::ModSpans { inner_span: span, inject_use_span: span };
    item(ast::ItemKind::Mod(ast::Safety::Default, ident(name, span), ast::ModKind::Loaded(items, ast::Inline::Yes, spans)), span)
}

/// `extern crate <name>;` -- loads a dependency the checked program names, so its items have `DefId`s to call.
pub fn extern_crate(name: &str, span: Span) -> Box<ast::Item> {
    item(ast::ItemKind::ExternCrate(None, ident(name, span)), span)
}

/// `use <segments>::*;`
pub fn glob_use(segments: &[&str], span: Span) -> Box<ast::Item> {
    let path = ast::Path { span, segments: segments.iter().map(|s| ast::PathSegment::from_ident(ident(s, span))).collect(), tokens: None };
    item(ast::ItemKind::Use(ast::UseTree { prefix: path, kind: ast::UseTreeKind::Glob(span) }), span)
}

// ---- Bodies: MIR the front end builds ----

/// The item at `crate::module::..::name` in a dependency, by walking its public module tree. This is how a checked
/// call plan's canonical path becomes the `DefId` a `Call` terminator names.
pub fn extern_item(tcx: TyCtxt<'_>, path: &[&str]) -> DefId {
    let missing = || -> ! { tcx.dcx().fatal(format!("the front end needs `{}`, and no loaded crate has it", path.join("::"))) };
    let Some((krate, rest)) = path.split_first() else { missing() };
    let Some(cnum) = tcx.crates(()).iter().copied().find(|c| tcx.crate_name(*c).as_str() == *krate) else { missing() };
    let mut current = cnum.as_def_id();
    for segment in rest {
        let child = tcx.module_children(current).iter().find(|c| c.ident.name.as_str() == *segment);
        let Some(def_id) = child.and_then(|c| c.res.opt_def_id()) else { missing() };
        current = def_id;
    }
    current
}

/// The associated function `name` in an inherent `impl` of the type `ty_def` -- `BodyPlan.new()` in Incan. Inherent
/// methods are not module children, so a call plan's path to one resolves through the type's impls.
pub fn inherent_method(tcx: TyCtxt<'_>, ty_def: DefId, name: &str) -> DefId {
    tcx.inherent_impls(ty_def).iter()
        .flat_map(|imp| tcx.associated_item_def_ids(*imp).iter().copied())
        .find(|d| tcx.item_name(*d).as_str() == name)
        .unwrap_or_else(|| tcx.dcx().fatal(format!("`{}` has no inherent method `{name}`", tcx.def_path_str(ty_def))))
}

/// A string literal: a `&'static str` constant over interned bytes, as rustc's own `mir_build` lowers `"..."`.
pub fn str_literal<'tcx>(tcx: TyCtxt<'tcx>, text: &str, span: Span) -> Operand<'tcx> {
    let alloc_id = tcx.allocate_bytes_dedup(text.as_bytes(), rustc_middle::mir::interpret::CTFE_ALLOC_SALT);
    let value = ConstValue::Slice { alloc_id, meta: text.len() as u64 };
    let ty = Ty::new_imm_ref(tcx, tcx.lifetimes.re_static, tcx.types.str_);
    Operand::Constant(Box::new(ConstOperand { span, user_ty: None, const_: Const::Val(value, ty) }))
}

/// The closures declared inside `parent`'s placeholder body, in declaration order.
pub fn closures_of(tcx: TyCtxt<'_>, parent: LocalDefId) -> Vec<LocalDefId> {
    tcx.nested_bodies_within(parent).iter().filter(|d| tcx.def_kind(*d) == rustc_hir::def::DefKind::Closure).collect()
}

pub fn local_item(tcx: TyCtxt<'_>, name: &str) -> DefId {
    tcx.hir_crate_items(()).free_items().map(|i| i.owner_id.to_def_id())
        .find(|d| tcx.opt_item_name(*d).is_some_and(|n| n.as_str() == name))
        .unwrap_or_else(|| tcx.dcx().fatal(format!("the front end needs item `{name}`, and this crate has none")))
}

/// A body under construction: locals start as the return place and one per parameter, typed from the checked
/// signature (generic parameters stay parameters); blocks are added and terminated as structured code is lowered.
pub struct Cfg<'tcx> {
    pub tcx: TyCtxt<'tcx>,
    def: LocalDefId,
    pub span: Span,
    arg_count: usize,
    pub locals: IndexVec<Local, LocalDecl<'tcx>>,
    blocks: IndexVec<BasicBlock, BasicBlockData<'tcx>>,
}

impl<'tcx> Cfg<'tcx> {
    /// A closure's body takes its environment first (`&Closure`, `&mut Closure` or `Closure`, by the closure kind
    /// typeck inferred), then its declared parameters, as rustc's own `mir_build` lays it out.
    pub fn new(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Self {
        let span = tcx.def_span(def);
        let sig = tcx.typeck(def).liberated_fn_sigs()[tcx.local_def_id_to_hir_id(def)];
        let mut locals = IndexVec::new();
        locals.push(LocalDecl::new(sig.output(), span));
        let closure_ty = tcx.type_of(def).instantiate_identity().skip_normalization();
        let env = match closure_ty.kind() {
            ty::Closure(_, args) => Some(tcx.closure_env_ty(closure_ty, args.as_closure().kind(), tcx.lifetimes.re_erased)),
            _ => None,
        };
        for input in env.iter().chain(sig.inputs()) {
            locals.push(LocalDecl::new(*input, span));
        }
        let arg_count = locals.len() - 1;
        let mut cfg = Cfg { tcx, def, span, arg_count, locals, blocks: IndexVec::new() };
        cfg.block();
        cfg
    }

    pub fn arg(&self, index: u32) -> Place<'tcx> {
        Place::from(Local::from_u32(index + 1))
    }

    pub fn temp(&mut self, ty: Ty<'tcx>) -> Place<'tcx> {
        Place::from(self.locals.push(LocalDecl::new(ty, self.span)))
    }

    pub fn block(&mut self) -> BasicBlock {
        self.blocks.push(BasicBlockData::new(None, false))
    }

    pub fn assign(&mut self, bb: BasicBlock, place: Place<'tcx>, rvalue: Rvalue<'tcx>, span: Span) {
        let stmt = Statement::new(SourceInfo::outermost(span), StatementKind::Assign(Box::new((place, rvalue))));
        self.blocks[bb].statements.push(stmt);
    }

    pub fn terminate(&mut self, bb: BasicBlock, kind: TerminatorKind<'tcx>, span: Span) {
        self.blocks[bb].terminator = Some(Terminator { source_info: SourceInfo::outermost(span), kind, attributes: ThinVec::new() });
    }

    /// A block on the unwind path. rustc requires cleanup code to live in blocks marked as cleanup.
    pub fn cleanup_block(&mut self) -> BasicBlock {
        self.blocks.push(BasicBlockData::new(None, true))
    }

    /// Ends `bb` with a call of `callee::<generic_args>(args)` into `destination`, and returns the block the call
    /// continues in. Unwinding continues to the caller.
    pub fn call(&mut self, bb: BasicBlock, callee: DefId, generic_args: &[ty::GenericArg<'tcx>], args: Vec<Operand<'tcx>>, destination: Place<'tcx>) -> BasicBlock {
        self.call_unwinding_to(bb, callee, generic_args, args, destination, UnwindAction::Continue)
    }

    /// As [`Cfg::call`], but a panic in the callee unwinds into `unwind`, typically a cleanup block that drops what
    /// this body owns.
    pub fn call_unwinding_to(&mut self, bb: BasicBlock, callee: DefId, generic_args: &[ty::GenericArg<'tcx>], args: Vec<Operand<'tcx>>, destination: Place<'tcx>, unwind: UnwindAction) -> BasicBlock {
        let span = self.span;
        let next = self.block();
        let kind = TerminatorKind::Call {
            func: Operand::function_handle(self.tcx, callee, generic_args.iter().copied(), span),
            args: args.into_iter().map(|node| rustc_span::Spanned { node, span }).collect(),
            destination,
            target: Some(next),
            unwind,
            call_source: CallSource::Normal,
            fn_span: span,
        };
        self.terminate(bb, kind, span);
        next
    }

    pub fn finish(self) -> Body<'tcx> {
        let span = self.span;
        let scopes = IndexVec::from_raw(vec![SourceScopeData { span, parent_scope: None, inlined: None, inlined_parent_scope: None, local_data: ClearCrossCrate::Clear }]);
        Body::new(MirSource::item(self.def.to_def_id()), self.blocks, scopes, self.locals, IndexVec::new(), self.arg_count, vec![], span, None, None)
    }
}
