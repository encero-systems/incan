//! Boxed native environments for planned capturing closures.
//!
//! A closure constructor's AST boxes one `move` closure. Its placeholder borrows captures for a reusable `Fn`
//! environment, or consumes them for a proven `FnOnce` environment, matching the planned object type.
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

/// Identify a reusable or consuming closure constructor, returning the lifted function and its capture count.
pub fn constructor(function: &Function) -> Option<(&str, i64)> {
    function.blocks.iter().find_map(|block| match &block.terminator.kind {
        TerminatorKind::Call(callee, _, _, _, _) => match &callee.kind {
            CalleeKind::ClosureBody(lifted, captures) | CalleeKind::ClosureBodyOnce(lifted, captures) => {
                Some((lifted.as_str(), *captures))
            }
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

/// Declare `::std::boxed::Box::new(move |_: A, ...| -> R { let _ = (&capture, ...); loop {} })` as the constructor's
/// tail, spelled from the crate root so a source declaration named `Box` cannot shadow it.
///
/// The placeholder borrows captures for `Fn` or consumes them for `FnOnce`; it never runs because [`callback`]
/// replaces its MIR. Preflight proves the constructor's object kind and exact capture parameters.
pub fn declaration_body(function: &Function, span: Span) -> Option<Box<ast::Block>> {
    constructor(function)?;
    let (PlanType::Closure(signature) | PlanType::ClosureOnce(signature)) = &function.return_type else {
        return None;
    };
    let consumes_environment = matches!(function.return_type, PlanType::ClosureOnce(_));
    let (result, inputs) = signature.split_last()?;
    let borrows = function
        .parameters
        .iter()
        .map(|parameter| {
            let capture = expression(
                ast::ExprKind::Path(None, ast::Path::from_ident(identifier(&parameter.name, span))),
                span,
            );
            if consumes_environment {
                return capture;
            }
            expression(
                ast::ExprKind::AddrOf(ast::BorrowKind::Ref, ast::Mutability::Not, capture),
                span,
            )
        })
        .collect();
    let capture_use = expression(ast::ExprKind::Tup(borrows), span);
    let capture_use = if consumes_environment {
        consume_captures(capture_use, span)
    } else {
        capture_use
    };
    let diverge = expression(ast::ExprKind::Loop(block(ThinVec::new(), span), None, span), span);
    let closure_body = block(
        thin_vec![
            ast::Stmt {
                id: ast::DUMMY_NODE_ID,
                kind: ast::StmtKind::Semi(capture_use),
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
    let mut boxing = declarations::boxed_path(span);
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

/// Force rustc to infer a consuming environment by moving its captures into a never-executed drop placeholder.
fn consume_captures(captures: Box<ast::Expr>, span: Span) -> Box<ast::Expr> {
    let mut segments = thin_vec![ast::PathSegment::path_root(span)];
    for name in ["std", "mem", "drop"] {
        segments.push(ast::PathSegment::from_ident(identifier(name, span)));
    }
    let callee = expression(
        ast::ExprKind::Path(
            None,
            ast::Path {
                span,
                segments,
                tokens: None,
            },
        ),
        span,
    );
    expression(ast::ExprKind::Call(callee, thin_vec![captures]), span)
}

/// Replace the inferred closure's placeholder blocks with a call to the lifted function.
///
/// rustc retains the closure's inferred ABI and by-value capture layout. Each call forwards captures followed by
/// inputs, preserving the lifted result. Reusable environments copy scalars and clone owned text, lists, and models;
/// consuming environments move their fields exactly once.
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
    let consumes_environment = matches!(constructor_function.return_type, PlanType::ClosureOnce(_));
    let closure = if consumes_environment {
        environment
    } else {
        let ty::Ref(_, closure, _) = environment.kind() else {
            return Err(PlanError::Invalid {
                function: constructor_function.name.clone(),
                reason: "closure environment is not a shared Fn borrow".into(),
            });
        };
        *closure
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
    let environment_place = if consumes_environment {
        mir::Place::from(mir::Local::from_usize(1))
    } else {
        mir::Place::from(mir::Local::from_usize(1)).project_deeper(&[mir::ProjectionElem::Deref], tcx)
    };
    let mut blocks = Vec::new();
    let mut operands = Vec::new();
    for (index, ty) in captures.iter().enumerate() {
        let field = environment_place.project_deeper(
            &[mir::ProjectionElem::Field(rustc_abi::FieldIdx::from_usize(index), ty)],
            tcx,
        );
        let owned = constructor_function
            .parameters
            .get(index)
            .is_some_and(|parameter| crate::plan::owned_capture(parameter.ty.clone()));
        let node = if consumes_environment {
            mir::Operand::Move(field)
        } else if owned {
            let (block, clone) = clone_capture(tcx, &mut body, field, ty, blocks.len() + 1, span)?;
            blocks.push(block);
            mir::Operand::Move(clone.into())
        } else {
            mir::Operand::Copy(field)
        };
        operands.push(Spanned { node, span });
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
        target: Some(mir::BasicBlock::from_usize(blocks.len() + 1)),
        unwind: mir::UnwindAction::Continue,
        call_source: mir::CallSource::Normal,
        fn_span: span,
    };
    blocks.push(terminated(vec![], call, span));
    blocks.push(terminated(vec![], mir::TerminatorKind::Return, span));
    *body.basic_blocks_mut() = IndexVec::from_raw(blocks);
    Ok(body)
}

/// Clone one owned capture out of the shared environment into a fresh local, continuing at block `next`.
///
/// The block borrows the environment field and calls `Clone::clone` on that borrow, so the environment keeps its own
/// value for the closure's next call while the lifted function receives and drops the clone.
fn clone_capture<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    field: mir::Place<'tcx>,
    ty: ty::Ty<'tcx>,
    next: usize,
    span: Span,
) -> Result<(mir::BasicBlockData<'tcx>, mir::Local), PlanError> {
    let reference = body.local_decls.push(mir::LocalDecl::new(
        ty::Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, ty),
        span,
    ));
    let clone = body.local_decls.push(mir::LocalDecl::new(ty, span));
    let borrow = mir::Statement::new(
        mir::SourceInfo::outermost(span),
        mir::StatementKind::Assign(Box::new((
            reference.into(),
            mir::Rvalue::Ref(tcx.lifetimes.re_erased, mir::BorrowKind::Shared, field),
        ))),
    );
    let call = mir::TerminatorKind::Call {
        func: mir::Operand::function_handle(tcx, callees::clone_method(tcx)?, [ty.into()], span),
        args: [Spanned {
            node: mir::Operand::Move(reference.into()),
            span,
        }]
        .into_iter()
        .collect(),
        destination: clone.into(),
        target: Some(mir::BasicBlock::from_usize(next)),
        unwind: mir::UnwindAction::Continue,
        call_source: mir::CallSource::Normal,
        fn_span: span,
    };
    Ok((terminated(vec![borrow], call, span), clone))
}

/// Build one basic block from its statements and terminator.
fn terminated<'tcx>(
    statements: Vec<mir::Statement<'tcx>>,
    kind: mir::TerminatorKind<'tcx>,
    span: Span,
) -> mir::BasicBlockData<'tcx> {
    mir::BasicBlockData::new_stmts(
        statements,
        Some(mir::Terminator {
            source_info: mir::SourceInfo::outermost(span),
            kind,
            attributes: ThinVec::new(),
        }),
        false,
    )
}
