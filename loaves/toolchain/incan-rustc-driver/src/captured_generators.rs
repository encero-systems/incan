//! Native closure environments for explicitly planned lazy producer captures.
//!
//! AST placeholders establish closure identities and capture layouts. Their executable blocks are replaced
//! with MIR that calls the Incan-planned producer; no generated source or eager producer execution is used.

use crate::error::PlanError;
use crate::plan::{Callee, CalleeKind, Function, TerminatorKind};
use crate::{bodies, callees, declarations, spans::Sources, types, values};
use rustc_ast as ast;
use rustc_hir::def_id::LocalDefId;
use rustc_index::IndexVec;
use rustc_middle::ty::TyCtxt;
use rustc_middle::{mir, ty};
use rustc_span::{Ident, Span, Spanned, Symbol};
use thin_vec::{ThinVec, thin_vec};

/// Identify constructors whose spawn operation has an explicit construction-time environment.
pub fn producer(function: &Function) -> Option<&Callee> {
    function.blocks.iter().find_map(|block| match &block.terminator.kind {
        TerminatorKind::Call(callee, captures, _, _, _)
            if matches!(callee.kind, CalleeKind::SpawnGenerator(..)) && !captures.is_empty() =>
        {
            Some(callee)
        }
        _ => None,
    })
}

/// Build an AST expression with identities left for rustc's normal node allocation.
fn expression(kind: ast::ExprKind, span: Span) -> Box<ast::Expr> {
    Box::new(ast::Expr {
        id: ast::DUMMY_NODE_ID,
        kind,
        span,
        attrs: ThinVec::new(),
        tokens: None,
    })
}

/// Name an injected source parameter without parsing source text.
fn identifier(name: &str, span: Span) -> Ident {
    Ident::new(Symbol::intern(name), span)
}

/// Declare a closure whose references establish exactly the source-ordered owned capture fields.
/// The placeholder never runs: both the constructor and the closure receive explicit MIR below.
pub fn declaration_body(function: &Function, span: Span) -> Option<Box<ast::Block>> {
    let callee = producer(function)?;
    let CalleeKind::SpawnGenerator(_, leaf, depth) = &callee.kind else {
        return None;
    };
    let captures = function
        .parameters
        .iter()
        .map(|parameter| {
            expression(
                ast::ExprKind::Path(None, ast::Path::from_ident(identifier(&parameter.name, span))),
                span,
            )
        })
        .collect();
    let closure_body = block(
        thin_vec![
            statement(expression(ast::ExprKind::Tup(captures), span), span),
            statement(diverge(span), span),
        ],
        span,
    );
    let parameter = ast::Param {
        attrs: ThinVec::new(),
        ty: declarations::ty(&crate::plan::PlanType::GeneratorYield(leaf.clone(), *depth), span),
        pat: Box::new(ast::Pat {
            id: ast::DUMMY_NODE_ID,
            kind: ast::PatKind::Wild,
            span,
            tokens: None,
        }),
        id: ast::DUMMY_NODE_ID,
        span,
        is_placeholder: false,
    };
    let closure = expression(
        ast::ExprKind::Closure(Box::new(ast::Closure {
            binder: ast::ClosureBinder::NotPresent,
            capture_clause: ast::CaptureBy::Value { move_kw: span },
            constness: ast::Const::No,
            coroutine_kind: None,
            movability: ast::Movability::Movable,
            fn_decl: Box::new(ast::FnDecl {
                inputs: thin_vec![parameter],
                output: ast::FnRetTy::Ty(declarations::ty(&crate::plan::PlanType::Unit, span)),
            }),
            body: expression(ast::ExprKind::Block(closure_body, None), span),
            fn_decl_span: span,
            fn_arg_span: span,
        })),
        span,
    );
    // Type-check the runtime's Send and 'static obligations before replacing the constructor's MIR.
    let generator = declarations::ty(&crate::plan::PlanType::Generator(leaf.clone(), *depth), span);
    let ast::TyKind::Path(None, mut path) = generator.kind else {
        return None;
    };
    path.segments
        .push(ast::PathSegment::from_ident(identifier("spawn", span)));
    let spawn = expression(
        ast::ExprKind::Call(expression(ast::ExprKind::Path(None, path), span), thin_vec![closure]),
        span,
    );
    Some(block(
        thin_vec![statement(spawn, span), statement(diverge(span), span)],
        span,
    ))
}

/// Retain a placeholder expression as a statement solely for type and capture checking.
fn statement(value: Box<ast::Expr>, span: Span) -> ast::Stmt {
    ast::Stmt {
        id: ast::DUMMY_NODE_ID,
        kind: ast::StmtKind::Semi(value),
        span,
    }
}

/// Create an AST block without source tokens.
fn block(stmts: ThinVec<ast::Stmt>, span: Span) -> Box<ast::Block> {
    Box::new(ast::Block {
        stmts,
        id: ast::DUMMY_NODE_ID,
        rules: ast::BlockCheckMode::Default,
        span,
        tokens: None,
    })
}

/// Give a declaration a diverging placeholder rather than an authored executable body.
fn diverge(span: Span) -> Box<ast::Expr> {
    expression(ast::ExprKind::Loop(block(ThinVec::new(), span), None, span), span)
}

/// Locate an injected closure within its constructor without querying whole-crate analysis.
struct ClosureLookup {
    /// The constructor contains exactly one declaration-time environment.
    def: Option<LocalDefId>,
}

impl<'hir> rustc_hir::intravisit::Visitor<'hir> for ClosureLookup {
    /// Walk only the constructor's expressions; closure bodies have separate MIR ownership.
    fn visit_expr(&mut self, expression: &'hir rustc_hir::Expr<'hir>) {
        if let rustc_hir::ExprKind::Closure(closure) = expression.kind {
            self.def = Some(closure.def_id);
        } else {
            rustc_hir::intravisit::walk_expr(self, expression);
        }
    }
}

/// Obtain the one injected environment belonging to this explicit constructor.
fn closure_type<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> Result<ty::Ty<'tcx>, PlanError> {
    let mut lookup = ClosureLookup { def: None };
    rustc_hir::intravisit::Visitor::visit_body(&mut lookup, tcx.hir_body_owned_by(def));
    let closure = lookup
        .def
        .ok_or_else(|| PlanError::UnknownCallee("captured generator environment".into()))?;
    let typeck = tcx.typeck(def);
    Ok(typeck.node_type(tcx.local_def_id_to_hir_id(closure)))
}

/// Fill the constructor's native closure aggregate and invoke the existing lazy spawn operation.
pub fn constructor<'tcx>(
    tcx: TyCtxt<'tcx>,
    def: LocalDefId,
    function: &Function,
) -> Result<mir::Body<'tcx>, PlanError> {
    let sources = Sources::new(tcx.sess.source_map());
    let callee = producer(function).ok_or_else(|| PlanError::UnknownCallee(function.name.clone()))?;
    let environment = closure_type(tcx, def)?;
    let ty::Closure(closure, args) = environment.kind() else {
        return Err(PlanError::UnknownCallee("captured generator closure type".into()));
    };
    let mut body = bodies::body(tcx, def, function)?;
    let local = body.local_decls.push(mir::LocalDecl::new(environment, body.span));
    for (index, block) in function.blocks.iter().enumerate() {
        let TerminatorKind::Call(_, captures, _, _, _) = &block.terminator.kind else {
            continue;
        };
        if !matches!(&block.terminator.kind, TerminatorKind::Call(call, _, _, _, _) if matches!(call.kind, CalleeKind::SpawnGenerator(..)))
        {
            continue;
        }
        let span = sources.span(&block.terminator.span)?;
        let operands = captures
            .iter()
            .map(|capture| values::operand(tcx, &sources, capture))
            .collect::<Result<_, _>>()?;
        let native = &mut body.basic_blocks_mut()[mir::BasicBlock::from_usize(index)];
        native.statements.push(mir::Statement::new(
            mir::SourceInfo::outermost(span),
            mir::StatementKind::Assign(Box::new((
                local.into(),
                mir::Rvalue::Aggregate(Box::new(mir::AggregateKind::Closure(*closure, args)), operands),
            ))),
        ));
        let Some(terminator) = native.terminator.as_mut() else {
            continue;
        };
        let mir::TerminatorKind::Call { func, args, .. } = &mut terminator.kind else {
            continue;
        };
        let CalleeKind::SpawnGenerator(_, leaf, depth) = &callee.kind else {
            continue;
        };
        let element = types::native_type(tcx, &crate::plan::PlanType::GeneratorYield(leaf.clone(), *depth))?;
        let ty::Adt(_, element_args) = element.kind() else {
            continue;
        };
        *func = mir::Operand::function_handle(
            tcx,
            callees::resolve(tcx, callee)?,
            [element_args.type_at(0).into(), environment.into()],
            span,
        );
        *args = [Spanned {
            node: mir::Operand::Move(local.into()),
            span,
        }]
        .into_iter()
        .collect();
    }
    Ok(body)
}

/// Replace the inferred closure's placeholder blocks with a direct call to the explicit planned producer.
/// rustc retains the closure ABI and owned capture layout; only producer execution comes from the plan.
pub fn callback<'tcx>(
    tcx: TyCtxt<'tcx>,
    def: LocalDefId,
    constructor: &Function,
) -> Result<mir::Body<'tcx>, PlanError> {
    let callee = producer(constructor).ok_or_else(|| PlanError::UnknownCallee(constructor.name.clone()))?;
    let CalleeKind::SpawnGenerator(name, _, _) = &callee.kind else {
        return Err(PlanError::UnknownCallee(constructor.name.clone()));
    };
    let mut body = (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.mir_built)(tcx, def)
        .borrow()
        .clone();
    let span = body.span;
    let environment = body.local_decls[mir::Local::from_usize(1)].ty;
    let mut capture_place = mir::Place::from(mir::Local::from_usize(1));
    let closure = match environment.kind() {
        ty::Ref(_, closure, _) => {
            capture_place = capture_place.project_deeper(&[mir::ProjectionElem::Deref], tcx);
            *closure
        }
        _ => environment,
    };
    let ty::Closure(_, args) = closure.kind() else {
        return Err(PlanError::UnknownCallee(name.clone()));
    };
    let captures = args.as_closure().upvar_tys();
    let mut operands = vec![Spanned {
        node: mir::Operand::Move(mir::Local::from_usize(2).into()),
        span,
    }];
    for (index, ty) in captures.iter().enumerate() {
        let field = capture_place.project_deeper(
            &[mir::ProjectionElem::Field(rustc_abi::FieldIdx::from_usize(index), ty)],
            tcx,
        );
        operands.push(Spanned {
            node: if matches!(environment.kind(), ty::Ref(..)) {
                mir::Operand::Copy(field)
            } else {
                mir::Operand::Move(field)
            },
            span,
        });
    }
    let producer = Callee {
        kind: CalleeKind::Planned(name.clone()),
        span: callee.span.clone(),
    };
    let call = mir::TerminatorKind::Call {
        func: mir::Operand::function_handle(tcx, callees::resolve(tcx, &producer)?, [], span),
        args: operands.into_iter().collect(),
        destination: mir::RETURN_PLACE.into(),
        target: Some(mir::BasicBlock::from_usize(1)),
        unwind: mir::UnwindAction::Continue,
        call_source: mir::CallSource::Normal,
        fn_span: span,
    };
    *body.basic_blocks_mut() = IndexVec::from_raw(vec![
        mir::BasicBlockData::new_stmts(
            vec![],
            Some(mir::Terminator {
                source_info: mir::SourceInfo::outermost(span),
                kind: call,
                attributes: ThinVec::new(),
            }),
            false,
        ),
        mir::BasicBlockData::new_stmts(
            vec![],
            Some(mir::Terminator {
                source_info: mir::SourceInfo::outermost(span),
                kind: mir::TerminatorKind::Return,
                attributes: ThinVec::new(),
            }),
            false,
        ),
    ]);
    Ok(body)
}
