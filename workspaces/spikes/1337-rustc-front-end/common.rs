//! Shared by spike steps 5 and later: AST builders for declarations and a small CFG builder for MIR bodies.
//! Each step is still its own driver; this module is included with `mod common;`.

use rustc_ast as ast;
use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_index::IndexVec;
use rustc_middle::mir::*;
use rustc_middle::ty::{Ty, TyCtxt};
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
    let id = ast::DUMMY_NODE_ID;
    let inputs = params.iter().map(|(p, spec)| ast::Param {
        attrs: ThinVec::new(), ty: ty(spec, span),
        pat: Box::new(ast::Pat { id, kind: ast::PatKind::Ident(ast::BindingMode::NONE, ident(p, span), None), span, tokens: None }),
        id, span, is_placeholder: false,
    }).collect();
    let empty = ast::Block { stmts: ThinVec::new(), id, rules: ast::BlockCheckMode::Default, span, tokens: None };
    let diverge = ast::Expr { id, kind: ast::ExprKind::Loop(Box::new(empty), None, span), span, attrs: ThinVec::new(), tokens: None };
    let body = ast::Block { stmts: thin_vec![ast::Stmt { id, kind: ast::StmtKind::Expr(Box::new(diverge)), span }], id, rules: ast::BlockCheckMode::Default, span, tokens: None };
    let sig = ast::FnSig { header: ast::FnHeader::default(), decl: Box::new(ast::FnDecl { inputs, output: ast::FnRetTy::Ty(ty(&ret, span)) }), span };
    item(ast::ItemKind::Fn(Box::new(ast::Fn {
        defaultness: ast::Defaultness::Implicit, ident: ident(name, span), generics, sig,
        contract: None, define_opaque: None, body: Some(Box::new(body)), eii_impls: ThinVec::new(),
    })), span)
}

// ---- Bodies: MIR the front end builds ----

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
    pub fn new(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Self {
        let span = tcx.def_span(def);
        let sig = tcx.fn_sig(def).instantiate_identity().skip_binder();
        let mut locals = IndexVec::new();
        locals.push(LocalDecl::new(sig.output(), span));
        for input in sig.inputs() {
            locals.push(LocalDecl::new(*input, span));
        }
        let mut cfg = Cfg { tcx, def, span, arg_count: sig.inputs().len(), locals, blocks: IndexVec::new() };
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

    pub fn finish(self) -> Body<'tcx> {
        let span = self.span;
        let scopes = IndexVec::from_raw(vec![SourceScopeData { span, parent_scope: None, inlined: None, inlined_parent_scope: None, local_data: ClearCrossCrate::Clear }]);
        Body::new(MirSource::item(self.def.to_def_id()), self.blocks, scopes, self.locals, IndexVec::new(), self.arg_count, vec![], span, None, None)
    }
}
