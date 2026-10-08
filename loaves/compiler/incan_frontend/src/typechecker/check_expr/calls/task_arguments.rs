//! Arguments that must be a task: the `RuntimeFuture[T]` parameters of `spawn`, `timeout`, `race_timeout` and `arm`.
//!
//! The checker gives an `async def` call's result its output type (`work()` is an `int`), so the type alone cannot tell
//! a task from a value. A task argument is therefore admitted exactly when `await` would admit the same expression: a
//! direct async call, a value whose type is awaitable (a `JoinHandle[T]`, a Rust-origin future), or a type parameter,
//! which its own bounds govern. Anything else is refused with `INCAN-T0115`, naming the argument as written (#1772).

use super::TypeChecker;
use crate::ast::{CallArg, Expr, Span, Spanned};
use crate::diagnostics::errors::{self, TaskArgument};
use crate::symbols::{CallableParam, FunctionInfo, ResolvedType, SymbolKind, TypeBoundInfo};
use incan_lang::lang::surface::types::{self as surface_types, SurfaceTypeId};
use incan_semantics_core::SymbolOrigin;

impl TypeChecker {
    /// Project a catalog-owned spawn result from its checked binding or task-argument output, leaving legacy types
    /// intact.
    ///
    /// SDK metadata preserves the native return display's binder. Only the exact declaring SDK function and its
    /// registered JoinHandle carrier grant this projection; a source shadow, unrelated Rust path, or output not
    /// proven by the checker cannot acquire a concrete task output through this helper.
    pub(in crate::typechecker::check_expr::calls) fn record_sdk_task_carrier_type(
        &mut self,
        callee: &str,
        function: &FunctionInfo,
        bindings: &std::collections::HashMap<String, ResolvedType>,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) {
        let Some(identity) = self
            .type_info
            .declarations
            .function_bindings
            .get(callee)
            .and_then(|binding| binding.identity.clone())
            .or_else(|| {
                self.symbols
                    .lookup(callee)
                    .and_then(|id| self.symbols.identity_of(id))
                    .cloned()
            })
        else {
            return;
        };
        let Some(source) = self.stdlib_cache.callable_source_identity(&identity) else {
            return;
        };
        let SymbolOrigin::Module(path) = &source.origin else {
            return;
        };
        if path.as_slice() != ["std", "async", "task"]
            || !matches!(source.declaration_name.as_str(), "spawn" | "spawn_blocking")
        {
            return;
        }
        let parameter = match &function.return_type {
            ResolvedType::RustPath(path)
                if surface_types::from_runtime_rust_path(path) == Some(SurfaceTypeId::JoinHandle) =>
            {
                let Some((_, arguments)) = Self::rust_generic_base_and_args(path) else {
                    return;
                };
                let [parameter] = arguments.as_slice() else {
                    return;
                };
                (*parameter).to_owned()
            }
            ResolvedType::Generic(base, arguments)
                if surface_types::from_str(base) == Some(SurfaceTypeId::JoinHandle) =>
            {
                let [ResolvedType::Named(parameter) | ResolvedType::TypeVar(parameter)] = arguments.as_slice() else {
                    return;
                };
                parameter.clone()
            }
            _ => return,
        };
        let Some(SymbolKind::Function(declaration)) =
            self.stdlib_cache.lookup_function_symbol(path, &source.declaration_name)
        else {
            return;
        };
        if !declaration.type_params.contains(&parameter) || function.params.len() != 1 {
            return;
        }
        if source.declaration_name == "spawn" {
            self.record_sdk_task_source_future(args, span);
        }
        let output = bindings
            .get(&parameter)
            .filter(|ty| !matches!(ty, ResolvedType::Unknown))
            .cloned()
            .or_else(|| self.sdk_task_argument_output(&source.declaration_name, args, arg_types));
        let Some(output) = output else {
            return;
        };
        self.type_info.calls.sdk_task_carrier_types.insert(
            (span.start, span.end),
            ResolvedType::Generic(
                surface_types::as_str(SurfaceTypeId::JoinHandle).to_owned(),
                vec![output],
            ),
        );
    }

    /// Retain only direct async calls whose callee identity the checker resolved. Parentheses preserve that proof;
    /// an awaitable value or an unresolved callee does not acquire a guessed source declaration.
    fn record_sdk_task_source_future(&mut self, args: &[CallArg], span: Span) {
        let [argument] = args else {
            return;
        };
        let mut expr = Self::call_arg_expr(argument);
        while let Expr::Paren(inner) = &expr.node {
            expr = inner;
        }
        if !self.expr_is_async_call_realization(expr) {
            return;
        }
        let Expr::Call(callee, _, _) = &expr.node else {
            return;
        };
        if let Some(identity) = self.type_info.resolved_identity(callee.span).cloned() {
            self.type_info
                .calls
                .sdk_task_source_futures
                .insert((span.start, span.end), identity);
        }
    }

    /// Retain the output the checker already assigned to the SDK's single admitted task or callable argument.
    /// Direct async calls are typed by their eventual output; other awaitables use the same checked output relation
    /// as `await`. Opaque Rust inputs stay opaque rather than gaining a guessed result.
    fn sdk_task_argument_output(
        &mut self,
        operation: &str,
        args: &[CallArg],
        types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        let [argument] = args else {
            return None;
        };
        let [ty] = types else {
            return None;
        };
        if operation == "spawn_blocking" {
            let ResolvedType::Function(_, output) = ty else {
                return None;
            };
            return Some(output.as_ref().clone());
        }
        if self.expr_is_async_call_realization(Self::call_arg_expr(argument)) {
            return Some(ty.clone());
        }
        self.await_output_type_from_type(ty)
            .filter(|output| !matches!(output, ResolvedType::Unknown))
    }

    /// Refuse the arguments bound to a parameter whose type parameter is bounded by a future capability (#1772).
    ///
    /// `spawn(work)` binds `TaskFuture with RuntimeFuture[T]` to the function `work` instead of to the task `work()`
    /// creates. The explicit bound check refuses a function value as well, but only this pass sees the argument as
    /// written. A reported parameter's binding becomes `Unknown`, which every bound admits, so the same argument is not
    /// reported twice.
    pub(in crate::typechecker::check_expr::calls) fn refuse_non_task_arguments(
        &mut self,
        callee: &str,
        params: &[CallableParam],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        bound_details: &std::collections::HashMap<String, Vec<TypeBoundInfo>>,
        type_bindings: &mut std::collections::HashMap<String, ResolvedType>,
    ) {
        let paired = Self::arguments_with_parameters(params, args);
        for ((expr, param), arg_ty) in paired.into_iter().zip(arg_types) {
            let Some(type_param) = param.and_then(|param| match &param.ty {
                ResolvedType::TypeVar(name) | ResolvedType::Named(name) => Some(name.as_str()),
                _ => None,
            }) else {
                continue;
            };
            let requires_future = bound_details
                .get(type_param)
                .is_some_and(|bounds| bounds.iter().any(|bound| Self::bound_requires_future(&bound.name)));
            if requires_future && self.refuse_non_task_argument(callee, expr, arg_ty) {
                type_bindings.insert(type_param.to_string(), ResolvedType::Unknown);
            }
        }
    }

    /// Refuse one argument given where a task belongs unless it is one, and report whether it was refused (#1772).
    ///
    /// Shared by the generic call path and by the stdlib task helpers typed on their surface path (`spawn`, `timeout`,
    /// `timeout_ms`, `race_timeout`), so every spelling of the call refuses the same arguments with the same remedy.
    pub(in crate::typechecker::check_expr::calls) fn refuse_non_task_argument(
        &mut self,
        callee: &str,
        expr: &Spanned<Expr>,
        arg_ty: &ResolvedType,
    ) -> bool {
        if self.task_argument_is_admitted(expr, arg_ty) {
            return false;
        }
        let spelling = Self::function_value_spelling(&expr.node);
        let rendered_type = arg_ty.to_string();
        let argument = match (arg_ty, spelling.as_deref()) {
            (ResolvedType::Function(_, _), Some(value)) => match self.named_function_is_async(&expr.node) {
                Some(false) => TaskArgument::SyncFunction(value),
                Some(true) | None => TaskArgument::AsyncFunction(value),
            },
            (ResolvedType::Function(_, _), None) => TaskArgument::FunctionValue,
            _ => TaskArgument::Value(&rendered_type),
        };
        self.errors
            .push(errors::argument_is_not_a_task(callee, argument, expr.span));
        true
    }

    /// Whether `await` would admit this argument: a direct async call, an awaitable type, or a type parameter.
    ///
    /// An unresolved or Rust-origin type is awaitable as far as the checker can tell, as it is for `await`.
    fn task_argument_is_admitted(&mut self, expr: &Spanned<Expr>, ty: &ResolvedType) -> bool {
        if self.expr_is_async_call_realization(expr) {
            return true;
        }
        if matches!(ty, ResolvedType::Function(_, _)) {
            return false;
        }
        if self.generic_placeholder_name(ty).is_some() {
            return true;
        }
        self.await_output_type_from_type(ty).is_some()
    }

    /// Whether a function named by `expr` is `async def`, when `expr` names a declared function directly.
    fn named_function_is_async(&self, expr: &Expr) -> Option<bool> {
        let Expr::Ident(name) = expr else {
            return None;
        };
        match &self.lookup_symbol(name)?.kind {
            SymbolKind::Function(info) => Some(info.is_async),
            _ => None,
        }
    }

    /// Spell a function-valued argument the way the source wrote it, when it is a name or a field path.
    fn function_value_spelling(expr: &Expr) -> Option<String> {
        match expr {
            Expr::Ident(name) => Some(name.clone()),
            Expr::SelfExpr => Some("self".to_string()),
            Expr::Field(base, member) => Some(format!("{}.{member}", Self::function_value_spelling(&base.node)?)),
            _ => None,
        }
    }
}
