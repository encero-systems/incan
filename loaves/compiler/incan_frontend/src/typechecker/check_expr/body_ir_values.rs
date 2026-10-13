//! Preserve checked Result value contracts without changing legacy inference or lexical symbol types.

use crate::ast::{AssignmentStmt, CallArg, Expr, Span, Spanned};
use crate::symbols::{CallableParam, ResolvedType, canonical_builtin_member_identity};
use incan_lang::lang::surface::result_methods::{self, ResultMethodId};

use super::{TypeChecker, fill_open_result_parts};

impl TypeChecker {
    /// Read an additive executable contract before the legacy contextual snapshot.
    pub(in crate::typechecker) fn body_ir_checked_value_type(&self, span: Span) -> Option<&ResolvedType> {
        self.type_info
            .expressions
            .body_ir_expr_types
            .get(&(span.start, span.end))
            .or_else(|| self.type_info.expr_type(span))
    }

    /// Fill only holes in the checked callback return from its checked body; fixed channels are never replaced.
    pub(in crate::typechecker::check_expr) fn retain_body_ir_closure_type(
        &mut self,
        span: Span,
        parameters: &[CallableParam],
        returned: &ResolvedType,
        actual: &ResolvedType,
    ) {
        let mut returned = returned.clone();
        if !self.types_compatible(&returned, actual) {
            return;
        }
        fill_open_result_parts(&mut returned, actual, false);
        self.type_info.expressions.body_ir_expr_types.insert(
            (span.start, span.end),
            ResolvedType::Function(parameters.to_vec(), Box::new(returned)),
        );
    }

    /// Preserve an initializer's executable Result channels under its canonical lexical binding identity.
    pub(in crate::typechecker) fn retain_body_ir_assignment_value(&mut self, assignment: &AssignmentStmt) {
        let Some(actual) = self.body_ir_checked_value_type(assignment.value.span).cloned() else {
            return;
        };
        let Some(identity) = self
            .type_info
            .resolved_write_identity(assignment.name_span, &assignment.name)
            .cloned()
        else {
            return;
        };
        self.type_info
            .expressions
            .body_ir_binding_types
            .insert(identity, actual);
    }

    /// Propagate only exact lexical reads and canonical intrinsic Result contracts. Source methods with equal
    /// spellings, projections without a checked contract, and incompatible channel constraints publish no fact.
    pub(in crate::typechecker::check_expr) fn retain_body_ir_result_value(
        &mut self,
        expression: &Spanned<Expr>,
        recorded: &ResolvedType,
    ) {
        let actual = match &expression.node {
            Expr::Ident(_) => self
                .type_info
                .resolved_identity(expression.span)
                .and_then(|identity| self.type_info.expressions.body_ir_binding_types.get(identity))
                .cloned(),
            Expr::Paren(inner) => self.body_ir_checked_value_type(inner.span).cloned(),
            Expr::MethodCall(receiver, method, _, arguments) => {
                let Some(method_id) = result_methods::from_str(method) else {
                    return;
                };
                if self.type_info.resolved_identity(expression.span)
                    != Some(&canonical_builtin_member_identity("Result", method))
                {
                    return;
                }
                self.body_ir_result_method_type(receiver, method_id, arguments)
            }
            _ => None,
        };
        let Some(actual) = actual else {
            return;
        };
        if !self.types_compatible(recorded, &actual) {
            return;
        }
        let mut closed = recorded.clone();
        if matches!(closed, ResolvedType::Unknown) {
            closed = actual;
        } else {
            fill_open_result_parts(&mut closed, &actual, false);
        }
        self.type_info
            .expressions
            .body_ir_expr_types
            .insert((expression.span.start, expression.span.end), closed);
    }

    /// The registry-selected member fixes which checked callback output replaces which Result channel.
    fn body_ir_result_method_type(
        &self,
        receiver: &Spanned<Expr>,
        method: ResultMethodId,
        arguments: &[CallArg],
    ) -> Option<ResolvedType> {
        let receiver = self.body_ir_checked_value_type(receiver.span)?;
        let ok = receiver.result_ok_type()?;
        let error = receiver.result_err_type()?;
        if matches!(method, ResultMethodId::Unwrap | ResultMethodId::UnwrapOr) {
            return Some(ok.clone());
        }
        if matches!(method, ResultMethodId::Inspect | ResultMethodId::InspectErr) {
            return Some(receiver.clone());
        }
        let [CallArg::Positional(callback)] = arguments else {
            return None;
        };
        let ResolvedType::Function(_, output) = self.body_ir_checked_value_type(callback.span)? else {
            return None;
        };
        let (ok, error) = match method {
            ResultMethodId::Map => (output.as_ref(), error),
            ResultMethodId::MapErr => (ok, output.as_ref()),
            ResultMethodId::AndThen | ResultMethodId::OrElse => return Some(output.as_ref().clone()),
            _ => return None,
        };
        Some(ResolvedType::Generic(
            "Result".to_owned(),
            vec![ok.clone(), error.clone()],
        ))
    }
}
