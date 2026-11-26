//! Boxed native environments for planned capturing closures.
//!
//! A closure constructor's AST boxes one `move` closure. The placeholder borrows its captures with the access
//! of the planned `Fn` or `FnMut` receiver, or consumes them for a proven `FnOnce` environment.
//! The constructor keeps rustc's own MIR; only the closure's executable body is replaced, with a call that forwards
//! the captures and then the closure's inputs to the Incan-planned lifted function.

use crate::ast_build::ident as identifier;
use crate::error::PlanError;
use crate::plan::{Callee, CalleeKind, Function, PlanType, TerminatorKind};
use crate::{callees, declarations};
use rustc_ast as ast;
use rustc_hir::def_id::LocalDefId;
use rustc_index::IndexVec;
use rustc_middle::ty::TyCtxt;
use rustc_middle::{mir, ty};
use rustc_span::{Span, Spanned};
use thin_vec::{ThinVec, thin_vec};

/// Whether this synthesized function constructs an environment whose references bound the returned callable.
fn borrowing_constructor(function: &Function) -> Option<&[PlanType]> {
    match &function.return_type {
        PlanType::BorrowingClosure(signature) | PlanType::ExclusiveClosure(signature) => Some(signature),
        _ => None,
    }
}

/// Give a borrowing constructor one result lifetime bounded by every independently borrowed capture.
pub(crate) fn constructor_generics(function: &Function, span: Span) -> ast::Generics {
    let mut generics = ast::Generics::default();
    if borrowing_constructor(function).is_none() {
        return generics;
    }
    generics.params.push(lifetime_parameter("'closure", Vec::new(), span));
    for (index, parameter) in function.parameters.iter().enumerate() {
        let ty = declarations::ty(&parameter.ty, span);
        if matches!(ty.kind, ast::TyKind::Ref(..)) {
            let bound = ast::GenericBound::Outlives(lifetime("'closure", span));
            generics
                .params
                .push(lifetime_parameter(&format!("'capture_{index}"), vec![bound], span));
        }
    }
    generics
}

/// Preserve a constructor capture's type while naming each outer reference lifetime independently.
pub(crate) fn constructor_parameter_type(function: &Function, index: usize, span: Span) -> Box<ast::Ty> {
    let mut ty = crate::futures::constructor_parameter_type(function, index, span);
    if borrowing_constructor(function).is_some()
        && let ast::TyKind::Ref(region, _) = &mut ty.kind
    {
        *region = Some(lifetime(&format!("'capture_{index}"), span));
    }
    ty
}

/// Bind a borrowing closure result to the common lifetime all of its capture loans outlive.
pub(crate) fn constructor_return_type(function: &Function, span: Span) -> Option<Box<ast::Ty>> {
    let exclusive = matches!(&function.return_type, PlanType::ExclusiveClosure(_));
    borrowing_constructor(function)
        .map(|signature| declarations::borrowing_closure_ast(signature, exclusive, "'closure", span))
}

/// Build one named lifetime use for a generated signature.
fn lifetime(name: &str, span: Span) -> ast::Lifetime {
    ast::Lifetime {
        id: ast::DUMMY_NODE_ID,
        ident: rustc_span::Ident::new(rustc_span::Symbol::intern(name), span),
    }
}

/// Declare one generated lifetime parameter with its outlives bounds.
fn lifetime_parameter(name: &str, bounds: Vec<ast::GenericBound>, span: Span) -> ast::GenericParam {
    ast::GenericParam {
        id: ast::DUMMY_NODE_ID,
        ident: rustc_span::Ident::new(rustc_span::Symbol::intern(name), span),
        attrs: ThinVec::new(),
        bounds,
        is_placeholder: false,
        kind: ast::GenericParamKind::Lifetime,
        colon_span: None,
    }
}

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
/// The placeholder selects closure access from the captures. It never runs because [`callback`] replaces its MIR.
/// Preflight proves the constructor's object kind and exact capture parameters.
pub fn declaration_body(function: &Function, span: Span) -> Option<Box<ast::Block>> {
    constructor(function)?;
    let (PlanType::Closure(signature)
    | PlanType::ClosureOnce(signature)
    | PlanType::NativeClosure(signature, _, _)
    | PlanType::BorrowingClosure(signature)
    | PlanType::ExclusiveClosure(signature)) = &function.return_type
    else {
        return None;
    };
    let consumes_environment = matches!(function.return_type, PlanType::ClosureOnce(_));
    let exclusive = matches!(function.return_type, PlanType::ExclusiveClosure(_));
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
            let mutable_capture = exclusive && crate::plan::exclusive_capture(&parameter.ty);
            let (mode, place) = if mutable_capture {
                (
                    ast::Mutability::Mut,
                    expression(ast::ExprKind::Unary(ast::UnOp::Deref, capture), span),
                )
            } else {
                (ast::Mutability::Not, capture)
            };
            expression(ast::ExprKind::AddrOf(ast::BorrowKind::Ref, mode, place), span)
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

/// How a native callback supplies one environment field to its lifted function.
pub(crate) enum CaptureTransfer {
    Move,
    Reborrow,
    Clone,
    Copy,
}

/// Select capture transfer once for both the callback and its native trait check.
pub(crate) fn capture_transfer(function: &Function, capture: &PlanType) -> CaptureTransfer {
    if matches!(function.return_type, PlanType::ClosureOnce(_)) {
        CaptureTransfer::Move
    } else if crate::plan::exclusive_capture(capture) {
        CaptureTransfer::Reborrow
    } else if crate::plan::owned_capture(capture.clone()) {
        CaptureTransfer::Clone
    } else {
        CaptureTransfer::Copy
    }
}

/// Replace the inferred closure's placeholder blocks with a call to the lifted function.
///
/// rustc retains the closure's inferred ABI and by-value capture layout. Each call forwards captures followed by
/// inputs, preserving the lifted result. Reusable environments copy scalars, clone owned snapshots, and reborrow
/// exclusive captures. Consuming environments move their fields exactly once.
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
    let has_exclusive_capture = constructor_function
        .parameters
        .iter()
        .any(|parameter| crate::plan::exclusive_capture(&parameter.ty));
    let closure = if consumes_environment {
        environment
    } else {
        let ty::Ref(_, closure, mutability) = environment.kind() else {
            return Err(PlanError::Invalid {
                function: constructor_function.name.clone(),
                reason: "reusable closure environment is not borrowed".into(),
            });
        };
        // The FnMut carrier can lend a shared callable. Only an exclusive capture requires a mutable receiver.
        let accepts_receiver = if mutability.is_mut() {
            matches!(constructor_function.return_type, PlanType::ExclusiveClosure(_))
        } else {
            !has_exclusive_capture
        };
        if !accepts_receiver {
            return Err(PlanError::Invalid {
                function: constructor_function.name.clone(),
                reason: "closure receiver disagrees with its planned access".into(),
            });
        }
        *closure
    };
    let ty::Closure(_, arguments) = closure.kind() else {
        return Err(PlanError::UnknownCallee(lifted.into()));
    };
    let captures = arguments.as_closure().upvar_tys();
    if i64::try_from(captures.len()).ok() != Some(count) || captures.len() != constructor_function.parameters.len() {
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
    let mut loans = Vec::new();
    for (index, ty) in captures.iter().enumerate() {
        let field = environment_place.project_deeper(
            &[mir::ProjectionElem::Field(rustc_abi::FieldIdx::from_usize(index), ty)],
            tcx,
        );
        let capture_type = &constructor_function.parameters[index].ty;
        let node = match capture_transfer(constructor_function, capture_type) {
            CaptureTransfer::Move => mir::Operand::Move(field),
            CaptureTransfer::Reborrow => {
                let (loan, reference) = reborrow_capture(tcx, &mut body, field, ty, span);
                loans.push(loan);
                mir::Operand::Move(reference.into())
            }
            CaptureTransfer::Clone => {
                let (block, clone) = clone_capture(tcx, &mut body, field, ty, blocks.len() + 1, span)?;
                blocks.push(block);
                mir::Operand::Move(clone.into())
            }
            CaptureTransfer::Copy => mir::Operand::Copy(field),
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
    blocks.push(terminated(loans, call, span));
    blocks.push(terminated(vec![], mir::TerminatorKind::Return, span));
    *body.basic_blocks_mut() = IndexVec::from_raw(blocks);
    Ok(body)
}

/// Reborrow the referent for this call without moving its capture loan out of the exclusive receiver.
fn reborrow_capture<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    field: mir::Place<'tcx>,
    ty: ty::Ty<'tcx>,
    span: Span,
) -> (mir::Statement<'tcx>, mir::Local) {
    let local = body.local_decls.push(mir::LocalDecl::new(ty, span));
    let value = mir::Rvalue::Ref(
        tcx.lifetimes.re_erased,
        mir::BorrowKind::Mut {
            kind: mir::MutBorrowKind::Default,
        },
        field.project_deeper(&[mir::ProjectionElem::Deref], tcx),
    );
    let loan = mir::Statement::new(
        mir::SourceInfo::outermost(span),
        mir::StatementKind::Assign(Box::new((local.into(), value))),
    );
    (loan, local)
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
