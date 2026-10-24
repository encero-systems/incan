//! Retain agreed append elements for exact empty list initializers without changing legacy lexical inference.

use super::TypeChecker;
use crate::ast::{AssignmentStmt, BindingKind, CallArg, Expr, Spanned};
use crate::symbols::{ResolvedType, canonical_builtin_member_identity};
use crate::typechecker::helpers::collection_type_id;
use crate::typechecker::type_info::BodyIrEmptyListRequirement;
use incan_lang::lang::builtins::BuiltinFnId;
use incan_lang::lang::types::collections::CollectionTypeId;

impl TypeChecker {
    /// Track only directly mutable, unannotated empty list literals with an unknown checked element. A reassignment
    /// invalidates an existing candidate so its earlier initializer cannot receive a later value's contract.
    pub(in crate::typechecker) fn retain_body_ir_empty_list_initializer(&mut self, assignment: &AssignmentStmt) {
        let Some(identity) = self
            .type_info
            .resolved_write_identity(assignment.name_span, &assignment.name)
            .cloned()
        else {
            return;
        };
        if let Some(candidate) = self
            .type_info
            .expressions
            .body_ir_empty_list_requirements
            .get_mut(&identity)
        {
            candidate.invalidated = true;
            return;
        }
        if assignment.binding != BindingKind::Mutable || assignment.ty.is_some() {
            return;
        }
        if !matches!(&assignment.value.node, Expr::List(elements) if elements.is_empty()) {
            return;
        }
        let Some(ResolvedType::Generic(name, elements)) = self.body_ir_checked_value_type(assignment.value.span) else {
            return;
        };
        if collection_type_id(name) != Some(CollectionTypeId::List) || elements != &[ResolvedType::Unknown] {
            return;
        }
        let candidate = BodyIrEmptyListRequirement {
            initializer: assignment.value.span,
            declaration_write: assignment.name_span,
            list_name: name.clone(),
            element: None,
            invalidated: false,
            permitted_reads: Default::default(),
        };
        self.type_info
            .expressions
            .body_ir_empty_list_requirements
            .insert(identity, candidate);
    }

    /// Collect elements only from successful canonical append calls, and authorize canonical length reads. All other
    /// methods invalidate a tracked receiver; aliases and other escapes fail the final exact-read audit. Captured
    /// uses grant no permissions because this bounded contract does not establish their environment ownership.
    pub(in crate::typechecker::check_expr) fn retain_body_ir_empty_list_use(
        &mut self,
        expression: &Spanned<Expr>,
        errors_before: usize,
    ) {
        if self.errors.len() != errors_before || self.closure_depth != 0 {
            return;
        }
        if self.type_info.resolved_builtin_call(expression.span) == Some(BuiltinFnId::Len) {
            let arguments = match &expression.node {
                Expr::Call(_, _, arguments) | Expr::MethodCall(_, _, _, arguments) => arguments,
                _ => return,
            };
            if let [CallArg::Positional(value)] = arguments.as_slice() {
                self.allow_body_ir_empty_list_read(value);
            }
            return;
        }
        let Expr::MethodCall(receiver, _, type_args, arguments) = &expression.node else {
            return;
        };
        if !matches!(receiver.node, Expr::Ident(_)) {
            return;
        }
        let is_append = type_args.is_empty()
            && self.type_info.resolved_identity(expression.span)
                == Some(&canonical_builtin_member_identity("List", "append"));
        let actual = match arguments.as_slice() {
            [CallArg::Positional(value)] if is_append => self.body_ir_checked_value_type(value.span).cloned(),
            _ => None,
        };
        let actual = actual.filter(|ty| self.body_ir_requirement_is_closed(ty) && !matches!(ty, ResolvedType::Never));
        let key = receiver.span;
        let Some(identity) = self.type_info.references.resolved_identities.get(&key) else {
            return;
        };
        let Some(candidate) = self
            .type_info
            .expressions
            .body_ir_empty_list_requirements
            .get_mut(identity)
        else {
            return;
        };
        candidate.permitted_reads.insert(key);
        match (&candidate.element, actual) {
            (None, Some(actual)) => candidate.element = Some(actual),
            (Some(previous), Some(actual)) if previous == &actual => {}
            _ => candidate.invalidated = true,
        }
    }

    /// Authorize one bare binding read only when its enclosing canonical length call validated successfully.
    fn allow_body_ir_empty_list_read(&mut self, value: &Spanned<Expr>) {
        if !matches!(value.node, Expr::Ident(_)) {
            return;
        }
        let key = value.span;
        let Some(identity) = self.type_info.references.resolved_identities.get(&key) else {
            return;
        };
        if let Some(candidate) = self
            .type_info
            .expressions
            .body_ir_empty_list_requirements
            .get_mut(identity)
        {
            candidate.permitted_reads.insert(key);
        }
    }

    /// Publish each agreed element once, after the complete program succeeds and every lexical read has a validated
    /// bounded use and the declaration remains its only write. No earlier initializer or read receives a speculative
    /// grant that a later conflict could leave stale.
    pub(in crate::typechecker) fn publish_body_ir_empty_lists(&mut self) {
        let candidates = std::mem::take(&mut self.type_info.expressions.body_ir_empty_list_requirements);
        for (identity, candidate) in candidates {
            let Some(element) = candidate.element else {
                continue;
            };
            if candidate.invalidated
                || self
                    .type_info
                    .references
                    .resolved_write_identities
                    .iter()
                    .any(|((span, _), selected)| selected == &identity && *span != candidate.declaration_write)
                || self
                    .type_info
                    .references
                    .resolved_identities
                    .iter()
                    .any(|(span, selected)| selected == &identity && !candidate.permitted_reads.contains(span))
            {
                continue;
            }
            let ty = ResolvedType::Generic(candidate.list_name, vec![element]);
            self.type_info
                .expressions
                .body_ir_expr_types
                .insert(candidate.initializer, ty.clone());
            self.type_info
                .expressions
                .body_ir_binding_types
                .insert(identity.clone(), ty.clone());
            for (span, selected) in &self.type_info.references.resolved_identities {
                if selected == &identity {
                    self.type_info.expressions.body_ir_expr_types.insert(*span, ty.clone());
                }
            }
        }
    }
}
