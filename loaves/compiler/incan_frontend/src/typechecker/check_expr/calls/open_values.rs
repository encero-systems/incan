//! Close a source factory's unbounded binders from checked uses of its exact result bindings.

use std::collections::HashMap;

use super::TypeChecker;
use super::body_ir_generics::{concrete, normalized};
use crate::ast::{Expr, Span, Spanned};
use crate::diagnostics::errors;
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::symbols::{FunctionInfo, ResolvedType, TypeInfo};
use crate::typechecker::type_info::{BodyIrOpenCall, BodyIrOpenValue};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};

impl TypeChecker {
    /// Retain an independent binder scope only for an already selected source function with unbounded parameters.
    /// The ordinary checker owns arity and argument acceptance; this record supplies no new overload or bound proof.
    pub(in crate::typechecker) fn retain_open_source_call(
        &mut self,
        info: &FunctionInfo,
        names: &[String],
        bindings: &HashMap<String, ResolvedType>,
        span: Span,
        identity: Option<&CanonicalSymbolId>,
    ) {
        if info.is_async
            || names.len() != info.type_params.len()
            || info.type_param_bound_details.values().any(|bounds| !bounds.is_empty())
            || identity.is_none_or(|identity| identity.kind != SemanticSourceTargetKind::Function)
            || self.type_info.expressions.body_ir_open_values.contains_key(&span)
        {
            return;
        }
        let result = normalized(&info.return_type, names, self);
        let group = self.type_info.expressions.body_ir_open_calls.len();
        self.type_info.expressions.body_ir_open_calls.push(BodyIrOpenCall {
            span,
            parameters: names.to_vec(),
            result: result.clone(),
            bindings: bindings.clone(),
            invalidated: false,
            settled: false,
        });
        self.type_info
            .expressions
            .body_ir_open_values
            .insert(span, BodyIrOpenValue { group, ty: result });
    }

    /// Tie one assignment to the initializer's existing binder scope, invalidating a reused lexical storage slot.
    pub(in crate::typechecker) fn bind_open_value(&mut self, value: Span, target: Span, name: &str) {
        let symbolic = self.type_info.expressions.body_ir_open_values.get(&value).cloned();
        self.bind_symbolic_value(symbolic, target, name);
    }

    /// Destructuring preserves the factory's shared binders while assigning a separate template to each tuple slot.
    pub(in crate::typechecker) fn bind_open_tuple_element(
        &mut self,
        value: Span,
        index: usize,
        target: Span,
        name: &str,
    ) {
        let symbolic = self.type_info.expressions.body_ir_open_values.get(&value);
        let symbolic = symbolic.and_then(|value| {
            let parts = match &value.ty {
                ResolvedType::Tuple(parts) => parts,
                ResolvedType::Generic(name, parts) if name == "Tuple" => parts,
                _ => return None,
            };
            Some(BodyIrOpenValue {
                group: value.group,
                ty: parts.get(index)?.clone(),
            })
        });
        self.bind_symbolic_value(symbolic, target, name);
    }

    /// A write cannot merge independent source calls merely because they reuse a name or binder spelling.
    fn bind_symbolic_value(&mut self, symbolic: Option<BodyIrOpenValue>, target: Span, name: &str) {
        let Some(identity) = self.type_info.resolved_write_identity(target, name).cloned() else {
            return;
        };
        if let Some(previous) = self.type_info.expressions.body_ir_open_bindings.remove(&identity) {
            self.type_info.expressions.body_ir_open_calls[previous.group].invalidated = true;
            if let Some(value) = &symbolic {
                self.type_info.expressions.body_ir_open_calls[value.group].invalidated = true;
            }
            return;
        }
        if let Some(symbolic) = symbolic {
            self.type_info
                .expressions
                .body_ir_open_bindings
                .insert(identity, symbolic);
        }
    }

    /// Carry exact lexical aliases and selected nominal methods; constraints come from their checked signatures.
    pub(in crate::typechecker) fn retain_open_value_expression(&mut self, expression: &Spanned<Expr>) {
        let symbolic = match &expression.node {
            Expr::Ident(_) => self
                .type_info
                .resolved_identity(expression.span)
                .and_then(|identity| self.type_info.expressions.body_ir_open_bindings.get(identity))
                .cloned(),
            Expr::Paren(inner) => self.type_info.expressions.body_ir_open_values.get(&inner.span).cloned(),
            Expr::MethodCall(receiver, name, _, args) => self.open_method_value(receiver, name, args, expression.span),
            _ => None,
        };
        if let Some(symbolic) = symbolic {
            let group = symbolic.group;
            self.type_info
                .expressions
                .body_ir_open_values
                .insert(expression.span, symbolic);
            self.publish_open_value_group(group);
        }
    }

    /// Substitute only the receiver owner's binders into the already selected member declaration.
    fn open_method_value(
        &mut self,
        receiver: &Spanned<Expr>,
        name: &str,
        args: &[crate::ast::CallArg],
        span: Span,
    ) -> Option<BodyIrOpenValue> {
        let value = self
            .type_info
            .expressions
            .body_ir_open_values
            .get(&receiver.span)?
            .clone();
        let ResolvedType::Generic(owner, arguments) = &value.ty else {
            return None;
        };
        let (parameters, method) = match self.lookup_semantic_type_info(owner)? {
            TypeInfo::Newtype(info) => (&info.type_params, info.methods.get(name)?),
            TypeInfo::Model(info) => (&info.type_params, info.methods.get(name)?),
            TypeInfo::Class(info) => (&info.type_params, info.methods.get(name)?),
            _ => return None,
        };
        if !method.type_params.is_empty() || method.identity.as_ref() != self.type_info.resolved_identity(span) {
            return None;
        }
        let method = method.clone();
        let bindings = type_param_subst_map(parameters, arguments);
        let names = parameters.clone();
        for (expression, parameter) in Self::arguments_with_parameters(&method.params, args) {
            let expected = substitute_resolved_type(&normalized(&parameter?.ty, &names, self), &bindings);
            let actual = self.body_ir_checked_value_type(expression.span)?.clone();
            self.constrain_open_value(value.group, &expected, &actual, span);
        }
        let (_, result) = self.method_types_substituting_call_site_self(&method, &value.ty);
        let result = substitute_resolved_type(&normalized(&result, &names, self), &bindings);
        Some(BodyIrOpenValue {
            group: value.group,
            ty: result,
        })
    }

    /// Keep an await output in its operand's binder scope without treating independent task outputs as aliases.
    pub(in crate::typechecker) fn retain_open_await_value(&mut self, inner: Span, span: Span) {
        let value = self.type_info.expressions.body_ir_open_values.get(&inner).cloned();
        let Some(value) = value else {
            return;
        };
        if self.type_info.calls.body_ir_async_call_targets.contains_key(&inner) {
            let group = value.group;
            self.type_info.expressions.body_ir_open_values.insert(span, value);
            self.publish_open_value_group(group);
        }
    }

    /// Use ordinary checked destinations as constraints after all lexical references have been collected.
    pub(in crate::typechecker) fn constrain_open_value_destination(&mut self, span: Span, expected: &ResolvedType) {
        if let Some(value) = self.type_info.expressions.body_ir_open_values.get(&span).cloned() {
            self.constrain_open_value(value.group, &value.ty, expected, span);
        }
    }

    /// Apply retained adaptation destinations and publish each complete lexical inference group.
    pub(in crate::typechecker) fn settle_open_value_destinations(&mut self) {
        self.check_work.open_value_settlements += 1;
        self.settle_body_open_values();
    }

    /// Settle one complete lexical owner from its checked destinations; no other body's state is consulted.
    pub(in crate::typechecker) fn settle_body_open_values(&mut self) {
        let destinations = self.type_info.expressions.value_destination_types.clone();
        for (span, actual) in destinations {
            if let Some(value) = self.type_info.expressions.body_ir_open_values.get(&span).cloned() {
                if !self.type_info.expressions.body_ir_open_calls[value.group].settled {
                    self.constrain_open_value(value.group, &value.ty, &actual, span);
                }
            }
        }
        for group in 0..self.type_info.expressions.body_ir_open_calls.len() {
            if !self.type_info.expressions.body_ir_open_calls[group].settled {
                self.publish_open_value_group(group);
                self.type_info.expressions.body_ir_open_calls[group].settled = true;
            }
        }
    }

    /// Apply the shared checker unifier to one group's holes, refusing contradictory concrete constraints.
    fn constrain_open_value(&mut self, group: usize, expected: &ResolvedType, actual: &ResolvedType, span: Span) {
        if !concrete(actual) || self.type_info.expressions.body_ir_open_calls[group].invalidated {
            return;
        }
        let mut bindings = self.type_info.expressions.body_ir_open_calls[group].bindings.clone();
        let expected = substitute_resolved_type(expected, &bindings);
        if concrete(&expected) && !self.types_compatible(&expected, actual) {
            self.errors
                .push(errors::type_mismatch(&expected.to_string(), &actual.to_string(), span));
            self.type_info.expressions.body_ir_open_calls[group].invalidated = true;
            return;
        }
        self.infer_type_param_bindings(&expected, actual, &mut bindings);
        self.type_info.expressions.body_ir_open_calls[group].bindings = bindings;
        self.publish_open_value_group(group);
    }

    /// Publish complete agreed types through the existing checker-owned expression, binding and call contracts.
    fn publish_open_value_group(&mut self, group: usize) {
        let call = self.type_info.expressions.body_ir_open_calls[group].clone();
        if call.invalidated {
            for (span, value) in &self.type_info.expressions.body_ir_open_values {
                if value.group == group {
                    self.type_info.expressions.body_ir_expr_types.remove(span);
                }
            }
            for (identity, value) in &self.type_info.expressions.body_ir_open_bindings {
                if value.group == group {
                    self.type_info.expressions.body_ir_binding_types.remove(identity);
                }
            }
            let span = call.span;
            self.type_info.calls.body_ir_call_type_arguments.remove(&span);
            self.type_info.calls.body_ir_call_result_types.remove(&span);
            return;
        }
        let arguments = call
            .parameters
            .iter()
            .map(|name| call.bindings.get(name).filter(|ty| concrete(ty)).cloned())
            .collect::<Option<Vec<_>>>();
        let Some(arguments) = arguments else {
            return;
        };
        for (span, value) in &self.type_info.expressions.body_ir_open_values {
            if value.group == group {
                let ty = substitute_resolved_type(&value.ty, &call.bindings);
                if concrete(&ty) {
                    self.type_info.expressions.body_ir_expr_types.insert(*span, ty);
                }
            }
        }
        for (identity, value) in &self.type_info.expressions.body_ir_open_bindings {
            if value.group == group {
                let ty = substitute_resolved_type(&value.ty, &call.bindings);
                if concrete(&ty) {
                    self.type_info
                        .expressions
                        .body_ir_binding_types
                        .insert(identity.clone(), ty);
                }
            }
        }
        let span = call.span;
        self.type_info.calls.body_ir_call_type_arguments.insert(span, arguments);
        self.type_info
            .calls
            .body_ir_call_result_types
            .insert(span, substitute_resolved_type(&call.result, &call.bindings));
    }
}
