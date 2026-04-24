//! Check control-flow-ish expressions (`await`, `?`, `if`, and ranges).
//!
//! These helpers validate expressions that affect control flow or propagate errors, such as the `?` operator and
//! `if` expressions (treated as statement-like blocks in the current checker).

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::{ResolvedType, ScopeKind};

use super::TypeChecker;
use crate::frontend::typechecker::LoopContextKind;
use crate::frontend::typechecker::helpers::ensure_bool_condition;
use incan_core::lang::surface::types::{self as surface_types, SurfaceTypeId, TASK_JOIN_ERROR_TYPE_NAME};

impl TypeChecker {
    /// Type-check an `await` expression.
    ///
    /// By the time we reach this method the registry has already confirmed that the `await` feature is active (via
    /// `typecheck_surface_expr_action`), so no additional feature-gate check is needed here. The enclosing callable
    /// must be `async` (`in_async_body`). Closure bodies are not async contexts: `check_closure` clears
    /// `in_async_body` while typechecking the body.
    pub(in crate::frontend::typechecker::check_expr) fn check_await(
        &mut self,
        inner: &Spanned<Expr>,
        span: Span,
    ) -> ResolvedType {
        if !self.in_async_body {
            self.errors.push(errors::await_outside_async(span));
            return ResolvedType::Unknown;
        }

        let inner_ty = self.check_expr(inner);

        if let ResolvedType::Generic(name, args) = &inner_ty
            && surface_types::from_str(name.as_str()) == Some(SurfaceTypeId::JoinHandle)
            && let Some(output) = args.first()
        {
            return ResolvedType::Generic(
                "Result".to_string(),
                vec![
                    output.clone(),
                    ResolvedType::Named(TASK_JOIN_ERROR_TYPE_NAME.to_string()),
                ],
            );
        }

        inner_ty
    }

    /// Validate the `?` (try) operator.
    ///
    /// Ensures the operand is a `Result` and that its error type is compatible with
    /// the enclosing function's declared error type.
    ///
    /// ## Returns
    ///
    /// The `Ok` type of the `Result`, or [`ResolvedType::Unknown`] on error.
    pub(in crate::frontend::typechecker::check_expr) fn check_try(
        &mut self,
        inner: &Spanned<Expr>,
        span: Span,
    ) -> ResolvedType {
        let inner_ty = self.check_expr(inner);

        if !inner_ty.is_result() {
            self.errors.push(errors::try_on_non_result(&inner_ty.to_string(), span));
            return ResolvedType::Unknown;
        }

        if let (Some(inner_err), Some(expected_err)) = (inner_ty.result_err_type(), &self.current_return_error_type)
            && !self.types_compatible(inner_err, expected_err)
        {
            self.errors.push(errors::incompatible_error_type(
                &expected_err.to_string(),
                &inner_err.to_string(),
                span,
            ));
        }

        inner_ty.result_ok_type().cloned().unwrap_or(ResolvedType::Unknown)
    }

    pub(in crate::frontend::typechecker::check_expr) fn check_if_expr(
        &mut self,
        if_expr: &IfExpr,
        _span: Span,
    ) -> ResolvedType {
        let cond_ty = self.check_expr(&if_expr.condition);
        let is_compatible = self.types_compatible(&cond_ty, &ResolvedType::Bool);
        ensure_bool_condition(&cond_ty, if_expr.condition.span, is_compatible, &mut self.errors);

        self.symbols.enter_scope(ScopeKind::Block);
        for stmt in &if_expr.then_body {
            self.check_statement(stmt);
        }
        self.symbols.exit_scope();

        if let Some(else_body) = &if_expr.else_body {
            self.symbols.enter_scope(ScopeKind::Block);
            for stmt in else_body {
                self.check_statement(stmt);
            }
            self.symbols.exit_scope();
        }

        ResolvedType::Unit
    }

    /// Type-check a value-producing `loop:` expression.
    ///
    /// The loop body runs in its own block scope so bindings introduced inside the loop do not leak outward.
    /// `break <value>` statements feed their inferred types into the active loop context, which is resolved after
    /// the body finishes checking.
    pub(in crate::frontend::typechecker::check_expr) fn check_loop_expr(
        &mut self,
        loop_expr: &LoopExpr,
        expected: Option<&ResolvedType>,
        span: Span,
    ) -> ResolvedType {
        self.symbols.enter_scope(ScopeKind::Block);
        self.push_loop_context(LoopContextKind::Expression, expected.cloned());
        for stmt in &loop_expr.body {
            self.check_statement(stmt);
        }
        let loop_ctx = self.pop_loop_context();
        self.symbols.exit_scope();
        let Some(loop_ctx) = loop_ctx else {
            return ResolvedType::Unknown;
        };

        self.resolve_loop_break_result_type(span, expected, &loop_ctx.break_types)
    }

    /// Type-check a range expression (`start..end`) and return `Range[int]`.
    pub(in crate::frontend::typechecker::check_expr) fn check_range_expr(
        &mut self,
        start: &Spanned<Expr>,
        end: &Spanned<Expr>,
    ) -> ResolvedType {
        let start_ty = self.check_expr(start);
        let end_ty = self.check_expr(end);

        if start_ty != ResolvedType::Int {
            self.errors
                .push(errors::type_mismatch("int", &start_ty.to_string(), start.span));
        }
        if end_ty != ResolvedType::Int {
            self.errors
                .push(errors::type_mismatch("int", &end_ty.to_string(), end.span));
        }

        ResolvedType::Generic("Range".to_string(), vec![ResolvedType::Int])
    }
}
