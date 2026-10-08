//! Boxed native environments for planned capturing closures.
//!
//! A closure constructor's AST boxes one `move` closure whose placeholder body only borrows each capture, so rustc
//! infers an `Fn` environment holding the captures by value and coerces the box to the declared `dyn Fn` object.
//! The constructor keeps rustc's own MIR; only the closure's executable body is replaced, with a call that forwards
//! the captures and then the closure's inputs to the Incan-planned lifted function.

use crate::error::PlanError;
use crate::plan::{Callee, CalleeKind, Function, PlanType, TerminatorKind};
use crate::{callees, declarations};
use rustc_ast as ast;
use rustc_hir::def_id::LocalDefId;
use rustc_index::IndexVec;
use rustc_middle::ty::TyCtxt;
use rustc_middle::{mir, ty};
use rustc_span::{Ident, Span, Spanned, Symbol};
use thin_vec::{ThinVec, thin_vec};

/// Identify a closure constructor by its one `ClosureBody` call, returning the lifted function and its capture count.
pub fn constructor(function: &Function) -> Option<(&str, i64)> {
    function.blocks.iter().find_map(|block| match &block.terminator.kind {
        TerminatorKind::Call(callee, _, _, _, _) => match &callee.kind {
            CalleeKind::ClosureBody(lifted, captures) => Some((lifted.as_str(), *captures)),
            _ => None,
        },
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

/// Name an injected parameter or path segment without parsing source text.
fn identifier(name: &str, span: Span) -> Ident {
    Ident::new(Symbol::intern(name), span)
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

/// Declare `Box::new(move |_: A, ...| -> R { let _ = (&capture, ...); loop {} })` as the constructor's tail.
///
/// The placeholder borrows every capture so the `move` closure owns each one by value in parameter order while staying
/// `Fn`; it never runs, because the closure's MIR is replaced by [`callback`]. Preflight has proved the constructor
/// returns `Closure(signature)` and takes exactly its captures.
pub fn declaration_body(function: &Function, span: Span) -> Option<Box<ast::Block>> {
    constructor(function)?;
    let PlanType::Closure(signature) = &function.return_type else {
        return None;
    };
    let (result, inputs) = signature.split_last()?;
    let borrows = function
        .parameters
        .iter()
        .map(|parameter| {
            let capture = expression(
                ast::ExprKind::Path(None, ast::Path::from_ident(identifier(&parameter.name, span))),
                span,
            );
            expression(
                ast::ExprKind::AddrOf(ast::BorrowKind::Ref, ast::Mutability::Not, capture),
                span,
            )
        })
        .collect();
    let diverge = expression(ast::ExprKind::Loop(block(ThinVec::new(), span), None, span), span);
    let closure_body = block(
        thin_vec![
            ast::Stmt {
                id: ast::DUMMY_NODE_ID,
                kind: ast::StmtKind::Semi(expression(ast::ExprKind::Tup(borrows), span)),
                span,
            },
            ast::Stmt {
                id: ast::DUMMY_NODE_ID,
                kind: ast::StmtKind::Expr(diverge),
                span,
            },
        ],
        span,
    );
    let parameters = inputs
        .iter()
        .map(|input| ast::Param {
            attrs: ThinVec::new(),
            ty: declarations::ty(input, span),
            pat: Box::new(ast::Pat {
                id: ast::DUMMY_NODE_ID,
                kind: ast::PatKind::Wild,
                span,
                tokens: None,
            }),
            id: ast::DUMMY_NODE_ID,
            span,
            is_placeholder: false,
        })
        .collect();
    let closure = expression(
        ast::ExprKind::Closure(Box::new(ast::Closure {
            binder: ast::ClosureBinder::NotPresent,
            capture_clause: ast::CaptureBy::Value { move_kw: span },
            constness: ast::Const::No,
            coroutine_kind: None,
            movability: ast::Movability::Movable,
            fn_decl: Box::new(ast::FnDecl {
                inputs: parameters,
                output: ast::FnRetTy::Ty(declarations::ty(result, span)),
            }),
            body: expression(ast::ExprKind::Block(closure_body, None), span),
            fn_decl_span: span,
            fn_arg_span: span,
        })),
        span,
    );
    let mut boxing = ast::Path::from_ident(identifier("Box", span));
    boxing
        .segments
        .push(ast::PathSegment::from_ident(identifier("new", span)));
    let boxed = expression(
        ast::ExprKind::Call(expression(ast::ExprKind::Path(None, boxing), span), thin_vec![closure]),
        span,
    );
    Some(block(
        thin_vec![ast::Stmt {
            id: ast::DUMMY_NODE_ID,
            kind: ast::StmtKind::Expr(boxed),
            span,
        }],
        span,
    ))
}

/// Replace the inferred closure's placeholder blocks with a call to the lifted function.
///
/// rustc retains the closure's `Fn` ABI and by-value capture layout. The call passes each capture, copied from the
/// shared environment, then each input in order; the lifted function's result is the closure's result.
pub fn callback<'tcx>(
    tcx: TyCtxt<'tcx>,
    def: LocalDefId,
    constructor_function: &Function,
) -> Result<mir::Body<'tcx>, PlanError> {
    let (lifted, count) =
        constructor(constructor_function).ok_or_else(|| PlanError::UnknownCallee(constructor_function.name.clone()))?;
    let mut body = (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.mir_built)(tcx, def)
        .borrow()
        .clone();
    let span = body.span;
    let environment = body.local_decls[mir::Local::from_usize(1)].ty;
    let ty::Ref(_, closure, _) = environment.kind() else {
        return Err(PlanError::Invalid {
            function: constructor_function.name.clone(),
            reason: "closure environment is not a shared Fn borrow".into(),
        });
    };
    let ty::Closure(_, arguments) = closure.kind() else {
        return Err(PlanError::UnknownCallee(lifted.into()));
    };
    let captures = arguments.as_closure().upvar_tys();
    if i64::try_from(captures.len()).ok() != Some(count) {
        return Err(PlanError::Invalid {
            function: constructor_function.name.clone(),
            reason: "closure environment differs from the planned capture count".into(),
        });
    }
    let environment_place =
        mir::Place::from(mir::Local::from_usize(1)).project_deeper(&[mir::ProjectionElem::Deref], tcx);
    let mut operands = Vec::new();
    for (index, ty) in captures.iter().enumerate() {
        let field = environment_place.project_deeper(
            &[mir::ProjectionElem::Field(rustc_abi::FieldIdx::from_usize(index), ty)],
            tcx,
        );
        operands.push(Spanned {
            node: mir::Operand::Copy(field),
            span,
        });
    }
    for index in 2..=body.arg_count {
        operands.push(Spanned {
            node: mir::Operand::Move(mir::Local::from_usize(index).into()),
            span,
        });
    }
    let callee = Callee {
        kind: CalleeKind::Planned(lifted.into()),
        span: constructor_function.span.clone(),
    };
    let call = mir::TerminatorKind::Call {
        func: mir::Operand::function_handle(tcx, callees::resolve(tcx, &callee)?, [], span),
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
