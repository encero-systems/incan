//! Check control-flow-ish expressions (`await`, `?`, `if`, and ranges).
//!
//! These helpers validate expressions that affect control flow or propagate errors, such as the `?` operator and
//! `if` expressions (treated as statement-like blocks in the current checker).

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::{ResolvedType, ScopeKind, Symbol, SymbolKind, TypeInfo, VariableInfo, union_ty};

use super::TypeChecker;
use crate::frontend::typechecker::LoopContextKind;
use crate::frontend::typechecker::helpers::result_ty;
use incan_core::interop::RustItemKind;
use incan_core::lang::surface::types::{self as surface_types, SurfaceTypeId, TASK_JOIN_ERROR_TYPE_NAME};
use incan_core::lang::traits::{self as builtin_traits, TraitId};

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

        let prev_await_operand_span = self.await_operand_span.replace((inner.span.start, inner.span.end));
        let inner_ty = self.check_expr(inner);
        self.await_operand_span = prev_await_operand_span;

        if let Some(output_ty) = self.await_output_type(inner, &inner_ty) {
            return output_ty;
        }

        self.errors
            .push(errors::type_mismatch("Awaitable[_]", &inner_ty.to_string(), span));
        ResolvedType::Unknown
    }

    /// Resolve the output type of one checked await operand.
    fn await_output_type(&mut self, expr: &Spanned<Expr>, ty: &ResolvedType) -> Option<ResolvedType> {
        if self.expr_is_async_call_realization(expr) {
            return Some(ty.clone());
        }
        self.await_output_type_from_type(ty)
    }

    /// Return whether this expression is a direct async call in an `await` operand.
    fn expr_is_async_call_realization(&mut self, expr: &Spanned<Expr>) -> bool {
        match &expr.node {
            Expr::Call(callee, _, _) => self.call_expr_is_async(callee),
            Expr::MethodCall(base, method, _, _) => self.method_call_expr_is_async(base, method),
            Expr::Paren(inner) | Expr::Try(inner) => self.expr_is_async_call_realization(inner),
            _ => false,
        }
    }

    /// Return whether a call callee resolves to an async function.
    fn call_expr_is_async(&mut self, callee: &Spanned<Expr>) -> bool {
        match &callee.node {
            Expr::Ident(name) => self.lookup_symbol(name).is_some_and(|sym| match &sym.kind {
                SymbolKind::Function(info) => info.is_async,
                SymbolKind::RustItem(info) => {
                    matches!(&info.metadata, Some(metadata) if matches!(&metadata.kind, RustItemKind::Function(sig) if sig.is_async))
                }
                _ => false,
            }),
            Expr::Field(base, member) => self
                .imported_module_for_expr(base)
                .and_then(|(_, module_path)| self.resolve_imported_module_function_member(&module_path, member))
                .is_some_and(|info| info.is_async),
            _ => false,
        }
    }

    /// Return whether a method-call receiver resolves to an async method.
    fn method_call_expr_is_async(&self, base: &Spanned<Expr>, method: &str) -> bool {
        let Some(base_ty) = self.type_info.expr_type(base.span) else {
            return false;
        };
        if self.known_surface_async_method(base_ty, method) {
            return true;
        }
        let type_name = match base_ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => name,
            _ => return false,
        };
        match self.lookup_semantic_type_info(type_name) {
            Some(TypeInfo::Model(info)) => {
                Self::method_set_has_async_method(&info.methods, &info.method_overloads, method)
            }
            Some(TypeInfo::Class(info)) => {
                Self::method_set_has_async_method(&info.methods, &info.method_overloads, method)
            }
            Some(TypeInfo::Enum(info)) => {
                Self::method_set_has_async_method(&info.methods, &info.method_overloads, method)
            }
            Some(TypeInfo::Newtype(info)) => info.methods.get(method).is_some_and(|method_info| method_info.is_async),
            _ => false,
        }
    }

    /// Return whether a method map or overload set contains an async method with this name.
    fn method_set_has_async_method(
        methods: &std::collections::HashMap<String, crate::frontend::symbols::MethodInfo>,
        overloads: &std::collections::HashMap<String, Vec<crate::frontend::symbols::MethodInfo>>,
        method: &str,
    ) -> bool {
        methods.get(method).is_some_and(|info| info.is_async)
            || overloads
                .get(method)
                .is_some_and(|items| items.iter().any(|info| info.is_async))
    }

    /// Return whether a known stdlib surface receiver exposes this async method.
    fn known_surface_async_method(&self, receiver_ty: &ResolvedType, method: &str) -> bool {
        match receiver_ty {
            ResolvedType::Named(name) if surface_types::from_str(name.as_str()) == Some(SurfaceTypeId::Semaphore) => {
                matches!(method, "acquire")
            }
            ResolvedType::Generic(name, _) if surface_types::from_str(name.as_str()) == Some(SurfaceTypeId::Mutex) => {
                matches!(method, "lock")
            }
            ResolvedType::Generic(name, _) if surface_types::from_str(name.as_str()) == Some(SurfaceTypeId::RwLock) => {
                matches!(method, "read" | "write")
            }
            _ => false,
        }
    }

    /// Resolve the output type yielded by awaiting a checked awaitable type.
    pub(in crate::frontend::typechecker) fn await_output_type_from_type(
        &self,
        ty: &ResolvedType,
    ) -> Option<ResolvedType> {
        if let Some(name) = self.generic_placeholder_name(ty) {
            return self.await_output_type_from_active_bound(name);
        }
        match ty {
            ResolvedType::Unknown | ResolvedType::RustPath(_) => Some(ResolvedType::Unknown),
            ResolvedType::TypeVar(name) => self.await_output_type_from_active_bound(name),
            ResolvedType::Generic(name, args)
                if builtin_traits::from_str(name.as_str()) == Some(TraitId::Awaitable) && args.len() == 1 =>
            {
                args.first().cloned()
            }
            ResolvedType::Generic(name, args)
                if surface_types::from_str(name.as_str()) == Some(SurfaceTypeId::JoinHandle) && args.len() == 1 =>
            {
                args.first().map(|output| {
                    result_ty(
                        output.clone(),
                        ResolvedType::Named(TASK_JOIN_ERROR_TYPE_NAME.to_string()),
                    )
                })
            }
            ResolvedType::Named(name) => self
                .instantiated_trait_args_for_type(name, &[], builtin_traits::as_str(TraitId::Awaitable))
                .and_then(|args| args.first().cloned()),
            ResolvedType::Generic(name, args) => self
                .instantiated_trait_args_for_type(name, args, builtin_traits::as_str(TraitId::Awaitable))
                .and_then(|args| args.first().cloned()),
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.await_output_type_from_type(inner),
            _ => None,
        }
    }

    /// Resolve `Awaitable[T]` output from active generic bounds for one placeholder.
    fn await_output_type_from_active_bound(&self, placeholder_name: &str) -> Option<ResolvedType> {
        let awaitable = builtin_traits::as_str(TraitId::Awaitable);
        for frame in self.current_type_param_bound_details.iter().rev() {
            let Some(bounds) = frame.get(placeholder_name) else {
                continue;
            };
            for bound in bounds {
                if bound.name == awaitable && bound.type_args.len() == 1 {
                    return bound.type_args.first().cloned();
                }
            }
        }
        None
    }

    /// Type-check an expression-form `race for value:` block.
    pub(in crate::frontend::typechecker::check_expr) fn check_race_for(
        &mut self,
        race: &RaceForExpr,
        span: Span,
    ) -> ResolvedType {
        if !self.in_async_body {
            self.errors.push(errors::await_outside_async(span));
            return ResolvedType::Unknown;
        }

        let mut arm_body_types = Vec::with_capacity(race.arms.len());
        for arm in &race.arms {
            let awaitable_ty = self.check_expr(&arm.awaitable);
            let Some(binding_ty) = self.await_output_type(&arm.awaitable, &awaitable_ty) else {
                self.errors.push(errors::type_mismatch(
                    "Awaitable[_]",
                    &awaitable_ty.to_string(),
                    arm.awaitable.span,
                ));
                arm_body_types.push(ResolvedType::Unknown);
                continue;
            };
            arm_body_types.push(self.check_race_arm_body(&race.binding, binding_ty, &arm.body));
        }

        let known_body_types: Vec<_> = arm_body_types
            .into_iter()
            .filter(|ty| !matches!(ty, ResolvedType::Unknown))
            .collect();
        if known_body_types.is_empty() {
            ResolvedType::Unknown
        } else {
            union_ty(known_body_types)
        }
    }

    /// Type-check one race arm body with its arm-local winner binding.
    fn check_race_arm_body(&mut self, binding: &str, binding_ty: ResolvedType, body: &RaceForBody) -> ResolvedType {
        self.symbols.enter_scope(ScopeKind::Block);
        self.symbols.define(Symbol {
            name: binding.to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: binding_ty,
                is_mutable: false,
                is_used: false,
            }),
            span: Span::default(),
            scope: 0,
        });

        let body_ty = match body {
            RaceForBody::Expr(expr) => self.check_expr(expr),
            RaceForBody::Block(stmts) => self.check_race_arm_block_body(stmts),
        };

        self.symbols.exit_scope();
        body_ty
    }

    /// Type-check a block race arm, using a trailing expression statement as the arm value.
    fn check_race_arm_block_body(&mut self, stmts: &[Spanned<Statement>]) -> ResolvedType {
        let Some((last, prefix)) = stmts.split_last() else {
            return ResolvedType::Unit;
        };

        for stmt in prefix {
            self.check_statement(stmt);
        }

        match &last.node {
            Statement::Expr(expr) => self.check_expr(expr),
            _ => {
                self.check_statement(last);
                ResolvedType::Unit
            }
        }
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
        self.check_try_with_expected(inner, span, None)
    }

    /// Validate the `?` operator while preserving contextual `Ok` type information.
    pub(in crate::frontend::typechecker::check_expr) fn check_try_with_expected(
        &mut self,
        inner: &Spanned<Expr>,
        span: Span,
        expected_ok_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let expected_result_ty = expected_ok_ty.map(|ok_ty| result_ty(ok_ty.clone(), ResolvedType::Unknown));
        let inner_ty = self.check_expr_with_expected(inner, expected_result_ty.as_ref());

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

    /// Type-check an expression-form `if`, including RFC 068 truthiness validation for its condition.
    pub(in crate::frontend::typechecker::check_expr) fn check_if_expr(
        &mut self,
        if_expr: &IfExpr,
        _span: Span,
    ) -> ResolvedType {
        let cond_ty = self.check_expr(&if_expr.condition);
        self.validate_truthiness_condition(&cond_ty, if_expr.condition.span);

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
