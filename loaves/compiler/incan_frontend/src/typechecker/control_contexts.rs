//! Protected builtin bindings, loop and await contexts, and branch-value unification.

use super::*;

impl TypeChecker {
    /// Reject a source binding that would replace a protected builtin spelling.
    ///
    /// The registry resolves canonical and alias spellings to one stable builtin identity, while the protected policy
    /// deliberately selects only the identities that must remain globally available.
    ///
    /// Returns whether this call emitted the protected-binding diagnostic, allowing callers with further name-collision
    /// recovery to avoid obscuring the primary error.
    pub(in crate::typechecker) fn validate_protected_builtin_binding(&mut self, name: &str, span: Span) -> bool {
        let Some(builtin) = builtins::protected_binding_builtin(name) else {
            return false;
        };
        self.errors.push(errors::protected_builtin_binding(name, builtin, span));
        true
    }

    /// Push a loop's complete checked identity before checking its body, including generated expansion context.
    ///
    /// Statement loops pass `None` for `expected_break_ty`; expression loops forward whatever result type the
    /// surrounding context expects.
    pub fn push_loop_context(&mut self, kind: LoopContextKind, expected_break_ty: Option<ResolvedType>, span: Span) {
        self.loop_stack.push(LoopContext {
            kind,
            expected_break_ty,
            break_types: Vec::new(),
            span,
            borrowed_iteration: None,
        });
    }

    /// Pop the innermost loop context once the corresponding body has been checked.
    pub fn pop_loop_context(&mut self) -> Option<LoopContext> {
        self.loop_stack.pop()
    }

    /// Borrow the innermost active loop so `break` checking can append inferred break types.
    pub fn current_loop_context_mut(&mut self) -> Option<&mut LoopContext> {
        self.loop_stack.last_mut()
    }

    /// Return whether this call expression is inside the active `await` operand.
    pub fn is_in_await_operand(&self, span: Span) -> bool {
        self.await_operand_span
            .is_some_and(|operand| operand.start <= span.start && span.end <= operand.end)
    }

    /// Emit the missing-await warning for a direct async call when the current expression context should report it.
    pub fn warn_if_unawaited_async_call(&mut self, callable: &str, span: Span) {
        if !self.is_in_await_operand(span) && self.call_argument_depth == 0 {
            self.errors.push(errors::async_call_without_await(callable, span));
        }
    }

    /// Resolve the final type of a `loop:` expression from the `break` types observed in its body.
    ///
    /// The `break` values unify as [`Self::unify_branch_value_types`] states, against the outer expected type when
    /// one exists; a loop with no `break` has no value and is refused.
    pub fn resolve_loop_break_result_type(
        &mut self,
        loop_span: Span,
        expected_break_ty: Option<&ResolvedType>,
        break_types: &[(ResolvedType, Span)],
    ) -> ResolvedType {
        if break_types.is_empty() {
            self.errors.push(errors::loop_expression_requires_break(loop_span));
            return ResolvedType::Unknown;
        }
        self.unify_branch_value_types(break_types, expected_break_ty)
            .unwrap_or(ResolvedType::Unknown)
    }

    /// Return the one type the values of the branches of a value-producing construct unify to: the `break` values of
    /// a `loop:` expression, or the arms of a `match` expression.
    ///
    /// With `expected`, the type of the place the construct is written to, every value must be assignable to it and
    /// the construct yields it. Without it, each value either is assignable to the type unified so far, or the type
    /// unified so far is assignable to it and it becomes the result, so `i8` and `int` values unify to `int`. A
    /// `Result` side or `Option` payload one value leaves open takes the type another value gives it, whichever value
    /// comes first (#1561): `None` and `Some(5)` unify to `Option[int]`, and `Err("x")` and `Ok(1)` to
    /// `Result[int, str]`. A side no value fixes stays open, and the place the construct is written to settles it.
    /// A value that fits neither way is refused with a type mismatch at its span and the construct yields `Unknown`.
    /// An `Unknown` value has already been refused or is not yet known and a `Never` value does not complete, so
    /// neither takes part; `None` means no value took part.
    ///
    /// Every value whose type is not the result is recorded as written to a place of the result type
    /// ([`Self::record_value_destination_if_compatible`]), so lowering widens a narrower numeric value and wraps an
    /// `Option` payload in that branch itself, and its type is recorded with the open parts the result fills, so the
    /// `None` or `Err(...)` of one branch is built with the payload or side another branch gives. The branches of the
    /// generated construct then share one type, as Rust requires of the arms of a `match` and the values of a `loop`.
    pub(in crate::typechecker) fn unify_branch_value_types(
        &mut self,
        values: &[(ResolvedType, Span)],
        expected: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        let mut participating = values
            .iter()
            .filter(|(ty, _)| !matches!(ty, ResolvedType::Unknown | ResolvedType::Never));
        let result_ty = match expected {
            Some(expected) => {
                if let Some((ty, span)) = participating.find(|(ty, _)| !self.types_compatible(ty, expected)) {
                    self.errors
                        .push(errors::type_mismatch(&expected.to_string(), &ty.to_string(), *span));
                    return Some(ResolvedType::Unknown);
                }
                expected.clone()
            }
            None => {
                let (first_ty, _) = participating.next()?;
                let mut result_ty = first_ty.clone();
                for (ty, span) in participating {
                    if self.types_compatible(ty, &result_ty) {
                        check_expr::fill_open_result_parts(&mut result_ty, ty, false);
                        continue;
                    }
                    if self.types_compatible(&result_ty, ty) {
                        let mut widened = ty.clone();
                        check_expr::fill_open_result_parts(&mut widened, &result_ty, false);
                        result_ty = widened;
                        continue;
                    }
                    self.errors
                        .push(errors::type_mismatch(&result_ty.to_string(), &ty.to_string(), *span));
                    return Some(ResolvedType::Unknown);
                }
                result_ty
            }
        };
        for (ty, span) in values {
            if *ty != result_ty {
                self.record_value_destination_if_compatible(*span, ty, &result_ty);
                let mut filled = ty.clone();
                check_expr::fill_open_result_parts(&mut filled, &result_ty, false);
                if filled != *ty {
                    self.record_expr_type(*span, filled);
                }
            }
        }
        Some(result_ty)
    }
}
