//! Arguments that must be a task: the `RuntimeFuture[T]` parameters of `spawn`, `timeout`, `race_timeout` and `arm`.
//!
//! The checker gives an `async def` call's result its output type (`work()` is an `int`), so the type alone cannot tell
//! a task from a value. A task argument is therefore admitted exactly when `await` would admit the same expression: a
//! direct async call, a value whose type is awaitable (a `JoinHandle[T]`, a Rust-origin future), or a type parameter,
//! which its own bounds govern. Anything else is refused with `INCAN-T0115`, naming the argument as written (#1772).

use super::TypeChecker;
use crate::ast::{CallArg, Expr, Spanned};
use crate::diagnostics::errors::{self, TaskArgument};
use crate::symbols::{CallableParam, ResolvedType, SymbolKind, TypeBoundInfo};

impl TypeChecker {
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
