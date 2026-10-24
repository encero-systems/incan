//! Retain body-validated parameter requirements for implicit closure expressions without changing legacy inference.

use std::collections::hash_map::Entry;

use super::TypeChecker;
use crate::ast::{CallArg, Expr, Param, Span, Spanned, Type};
use crate::symbols::{CallableParam, ResolvedType};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};

impl TypeChecker {
    /// Retain an inline closure parameter's checked nominal owner after its selected caller supplies the channel.
    /// Lexical checking remains unchanged; only this parameter's identity-matching reads and callable storage close.
    pub(super) fn retain_body_ir_callback_parameter(&mut self, callback: &Spanned<Expr>, input: &ResolvedType) {
        if let Expr::Paren(inner) = &callback.node {
            self.retain_body_ir_callback_parameter(inner, input);
            return;
        }
        let Expr::Closure(params, _) = &callback.node else {
            return;
        };
        let [source] = params.as_slice() else { return };
        let Some(ResolvedType::Function(mut parameters, returned)) =
            self.body_ir_checked_value_type(callback.span).cloned()
        else {
            return;
        };
        let [parameter] = parameters.as_mut_slice() else { return };
        let closed = self.body_ir_close_value_parts(&parameter.ty, input);
        if closed == parameter.ty || !Self::is_settled_type_argument(&closed) || !self.types_compatible(&closed, input)
        {
            return;
        }
        let Some(identity) = self
            .type_info
            .resolved_write_identity(source.span, &source.node.name)
            .cloned()
        else {
            return;
        };
        parameter.ty = closed.clone();
        self.retain_body_ir_parameter_reads(&identity, &closed);
        self.type_info.expressions.body_ir_expr_types.insert(
            (callback.span.start, callback.span.end),
            ResolvedType::Function(parameters, returned),
        );
    }

    /// Retain agreed requirements only from a successfully validated, independently identified function call.
    /// Conflicts remain explicit so a later matching call cannot reinstate a previously invalidated grant.
    pub(in crate::typechecker::check_expr) fn retain_body_ir_parameter_requirements(
        &mut self,
        parameters: &[CallableParam],
        args: &[CallArg],
        identity: Option<&CanonicalSymbolId>,
    ) {
        // ---- Selected function declaration ----
        if !identity.is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Function) {
            return;
        }

        // ---- Validated argument requirements ----
        for (expression, parameter) in Self::arguments_with_parameters(parameters, args) {
            let Some(parameter) = parameter.filter(|parameter| parameter.kind == crate::ast::ParamKind::Normal) else {
                continue;
            };
            let expression = bare_parameter_argument(expression);
            if !matches!(expression.node, Expr::Ident(_)) {
                continue;
            }
            let Some(identity) = self
                .type_info
                .resolved_identity(expression.span)
                .filter(|identity| identity.kind == SemanticSourceTargetKind::Parameter)
                .cloned()
            else {
                continue;
            };
            match self
                .type_info
                .expressions
                .body_ir_parameter_requirements
                .entry(identity)
            {
                Entry::Vacant(entry) => {
                    entry.insert(Some(parameter.ty.clone()));
                }
                Entry::Occupied(mut entry)
                    if entry.get().as_ref().is_some_and(|previous| previous != &parameter.ty) =>
                {
                    entry.insert(None);
                }
                _ => {}
            }
        }
    }

    /// Close only this closure's written implicit parameters from their own agreed concrete requirements.
    /// Ordinary expression snapshots, lexical symbols and caller-owned parameter binders remain unchanged.
    pub(in crate::typechecker::check_expr) fn retain_body_ir_implicit_closure_parameters(
        &mut self,
        params: &[Spanned<Param>],
        parameters: &[CallableParam],
        returned: &ResolvedType,
        body: &Spanned<Expr>,
        span: Span,
    ) {
        let mut executable = parameters.to_vec();
        let mut changed = false;

        // ---- Storage for this closure's implicit parameters ----
        for (source, parameter) in params.iter().zip(&mut executable) {
            if !matches!(&source.node.ty.node, Type::Simple(name) if name == "_")
                || !matches!(
                    &parameter.ty,
                    ResolvedType::Unknown | ResolvedType::CallSiteInfer | ResolvedType::TypeVar(_)
                )
                || self.active_type_param_name(&parameter.ty).is_some()
            {
                continue;
            }
            let Some(identity) = self
                .type_info
                .resolved_write_identity(source.span, &source.node.name)
                .cloned()
            else {
                continue;
            };
            let Some(required) = self
                .type_info
                .expressions
                .body_ir_parameter_requirements
                .get(&identity)
                .and_then(Option::as_ref)
                .filter(|required| self.body_ir_requirement_is_closed(required))
                .cloned()
            else {
                continue;
            };
            parameter.ty = required.clone();
            self.retain_body_ir_parameter_reads(&identity, &required);
            changed = true;
        }

        // ---- Existing checked callback output contract ----
        if changed {
            let actual = self.body_ir_checked_value_type(body.span).unwrap_or(returned).clone();
            self.retain_body_ir_closure_type(span, &executable, returned, &actual);
        }
    }

    /// Publish an exact parameter's agreed storage only to its private binding and identity-matching read facts.
    fn retain_body_ir_parameter_reads(&mut self, identity: &CanonicalSymbolId, required: &ResolvedType) {
        self.type_info
            .expressions
            .body_ir_binding_types
            .insert(identity.clone(), required.clone());
        for (span, target) in &self.type_info.references.resolved_identities {
            if target == identity {
                self.type_info
                    .expressions
                    .body_ir_expr_types
                    .insert(*span, required.clone());
            }
        }
    }

    /// Require every structural leaf to be fixed, excluding caller-owned binders, placeholders and unresolved Self.
    pub(in crate::typechecker::check_expr) fn body_ir_requirement_is_closed(&self, ty: &ResolvedType) -> bool {
        if self.active_type_param_name(ty).is_some() || self.is_generic_placeholder_type(ty) {
            return false;
        }
        match ty {
            ResolvedType::Unknown | ResolvedType::CallSiteInfer | ResolvedType::TypeVar(_) | ResolvedType::SelfType => {
                false
            }
            ResolvedType::Generic(_, elements) | ResolvedType::Tuple(elements) => {
                elements.iter().all(|ty| self.body_ir_requirement_is_closed(ty))
            }
            ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner)
            | ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::TypeToken(inner) => self.body_ir_requirement_is_closed(inner),
            ResolvedType::FrozenDict(key, value) => {
                self.body_ir_requirement_is_closed(key) && self.body_ir_requirement_is_closed(value)
            }
            ResolvedType::Function(parameters, result) => {
                parameters
                    .iter()
                    .all(|parameter| self.body_ir_requirement_is_closed(&parameter.ty))
                    && self.body_ir_requirement_is_closed(result)
            }
            _ => true,
        }
    }
}

/// Parentheses preserve one direct parameter read; compound expressions grant no inverse type inference.
fn bare_parameter_argument(mut expression: &Spanned<Expr>) -> &Spanned<Expr> {
    while let Expr::Paren(inner) = &expression.node {
        expression = inner;
    }
    expression
}
