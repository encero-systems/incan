//! Preserve checked executable value contracts without changing legacy inference or lexical symbol types.

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
            .or_else(|| {
                self.type_info
                    .calls
                    .body_ir_constructor_types
                    .get(&(span.start, span.end))
            })
            .or_else(|| {
                self.type_info
                    .calls
                    .body_ir_call_result_types
                    .get(&(span.start, span.end))
            })
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

    /// Preserve an initializer's executable carrier under its canonical lexical binding identity.
    pub(in crate::typechecker) fn retain_body_ir_assignment_value(&mut self, assignment: &AssignmentStmt) {
        self.retain_body_ir_empty_list_initializer(assignment);
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

    /// Propagate exact lexical reads, checked nominal members and canonical intrinsic Result contracts. Incompatible
    /// constraints and members without a checked declaration publish no fact.
    pub(in crate::typechecker::check_expr) fn retain_body_ir_result_value(
        &mut self,
        expression: &Spanned<Expr>,
        recorded: &ResolvedType,
    ) {
        if self
            .type_info
            .calls
            .body_ir_factory_result_conflicts
            .contains(&(expression.span.start, expression.span.end))
        {
            return;
        }
        if matches!(expression.node, Expr::Call(_, _, _) | Expr::MethodCall(_, _, _, _))
            && let Some(actual) = self
                .type_info
                .calls
                .body_ir_call_result_types
                .get(&(expression.span.start, expression.span.end))
                .cloned()
        {
            self.type_info
                .expressions
                .body_ir_expr_types
                .insert((expression.span.start, expression.span.end), actual);
            return;
        }
        let actual = match &expression.node {
            Expr::Ident(_) => self
                .type_info
                .resolved_identity(expression.span)
                .and_then(|identity| self.type_info.expressions.body_ir_binding_types.get(identity))
                .cloned(),
            Expr::Paren(inner) => self.body_ir_checked_value_type(inner.span).cloned(),
            Expr::MethodCall(receiver, method, _, arguments) => {
                if let Some(actual) = self.body_ir_nominal_method_type(receiver, method, arguments, expression.span) {
                    self.type_info
                        .expressions
                        .body_ir_expr_types
                        .insert((expression.span.start, expression.span.end), actual);
                    return;
                }
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
            Expr::Call(_, _, _) | Expr::Constructor(_, _) => self
                .type_info
                .calls
                .body_ir_call_result_types
                .get(&(expression.span.start, expression.span.end))
                .or_else(|| {
                    self.type_info
                        .calls
                        .body_ir_constructor_types
                        .get(&(expression.span.start, expression.span.end))
                })
                .cloned(),
            Expr::Field(receiver, field) => self.body_ir_newtype_field_type(receiver, field),
            Expr::Index(receiver, index) => self.body_ir_tuple_element_type(receiver, index),
            _ => None,
        };
        let Some(actual) = actual else {
            return;
        };
        if !self.types_compatible(recorded, &actual) {
            return;
        }
        let closed = self.body_ir_close_value_parts(recorded, &actual);
        self.type_info
            .expressions
            .body_ir_expr_types
            .insert((expression.span.start, expression.span.end), closed);
    }

    /// Fill compatible holes from a checked value contract while preserving concrete leaves and caller-owned binders.
    pub(in crate::typechecker::check_expr) fn body_ir_close_value_parts(
        &self,
        recorded: &ResolvedType,
        actual: &ResolvedType,
    ) -> ResolvedType {
        match (recorded, actual) {
            (ResolvedType::Unknown | ResolvedType::CallSiteInfer, _) => actual.clone(),
            (ResolvedType::TypeVar(_), _) if self.active_type_param_name(recorded).is_none() => actual.clone(),
            (ResolvedType::Generic(name, parts), ResolvedType::Generic(other, values))
                if name == other && parts.len() == values.len() =>
            {
                ResolvedType::Generic(
                    name.clone(),
                    parts
                        .iter()
                        .zip(values)
                        .map(|(part, value)| self.body_ir_close_value_parts(part, value))
                        .collect(),
                )
            }
            (ResolvedType::Tuple(parts), ResolvedType::Tuple(values)) if parts.len() == values.len() => {
                ResolvedType::Tuple(
                    parts
                        .iter()
                        .zip(values)
                        .map(|(part, value)| self.body_ir_close_value_parts(part, value))
                        .collect(),
                )
            }
            (ResolvedType::Function(params, result), ResolvedType::Function(values, output))
                if params.len() == values.len() =>
            {
                let params = params
                    .iter()
                    .zip(values)
                    .map(|(param, value)| CallableParam {
                        ty: self.body_ir_close_value_parts(&param.ty, &value.ty),
                        ..param.clone()
                    })
                    .collect();
                ResolvedType::Function(params, Box::new(self.body_ir_close_value_parts(result, output)))
            }
            _ => recorded.clone(),
        }
    }

    /// Retain a checked nominal result's annotated specialization when legacy leaves its constructor arguments open.
    pub(in crate::typechecker::check_expr) fn retain_body_ir_value_destination(
        &mut self,
        expression: &Spanned<Expr>,
        recorded: &ResolvedType,
        expected: Option<&ResolvedType>,
    ) {
        if self
            .type_info
            .calls
            .body_ir_factory_result_conflicts
            .contains(&(expression.span.start, expression.span.end))
        {
            return;
        }
        let Some(expected) = expected.filter(|ty| Self::is_settled_type_argument(ty)) else {
            return;
        };
        let (ResolvedType::Generic(owner, _), ResolvedType::Generic(destination, _)) = (recorded, expected) else {
            return;
        };
        if owner != destination
            || self.type_info.resolved_identity(expression.span).is_none()
            || !self.types_compatible(recorded, expected)
        {
            return;
        }
        if self
            .body_ir_checked_value_type(expression.span)
            .is_some_and(|actual| !self.types_compatible(actual, expected))
        {
            self.refuse_body_ir_factory_result(expression.span);
            return;
        }
        let actual = self.body_ir_close_value_parts(recorded, expected);
        self.type_info
            .expressions
            .body_ir_expr_types
            .insert((expression.span.start, expression.span.end), actual);
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
