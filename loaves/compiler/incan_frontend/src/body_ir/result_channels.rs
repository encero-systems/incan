//! Closed Result constructor channels carried through Body IR without changing legacy inference facts.

use super::primitives::result_type_parts;
use super::{BodyBuilder, IncanPrimitiveType, IncanType, ast, bir, hir_span};
use incan_lang::lang::surface::result_methods::{self, ResultMethodId};
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::{HirSourceSpan, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

impl BodyBuilder<'_, '_> {
    /// Fill only matching Result channel holes from an already typed Body IR value; fixed channels stay fixed.
    pub(super) fn refine_result_channels(&self, recorded: &IncanType, actual: &IncanType) -> IncanType {
        if result_type_parts(recorded).is_none() || result_type_parts(actual).is_none() {
            return recorded.clone();
        }
        let refined = fill_channel_holes(recorded, actual);
        if self.leaves_type_open(&refined) {
            recorded.clone()
        } else {
            refined
        }
    }

    /// Read a whole frame value's retained type, never reconstructing a projected or global carrier.
    pub(super) fn result_operand_type(&self, value: &bir::Operand) -> Option<IncanType> {
        let bir::Operand::Place(read) = value else {
            return None;
        };
        if !read.place.projection.is_empty() {
            return None;
        }
        self.locals
            .get(read.place.local_id()?.index())
            .map(|local| local.ty.clone())
    }

    /// Preserve the closed Result type of a closure's actual result on its checked callable signature.
    pub(super) fn retain_closure_result_channels(&mut self, span: ast::Span, result: &bir::Operand) -> IncanType {
        let mut ty = self.resolve_ty(span);
        let Some(actual) = self.result_operand_type(result) else {
            return ty;
        };
        if let IncanType::Function { return_type, .. } = &mut ty {
            **return_type = self.refine_result_channels(return_type, &actual);
            self.type_overrides.insert((span.start, span.end), ty.clone());
        }
        ty
    }

    /// A canonical read of a local keeps the Result channels its defining Body IR value retained.
    pub(super) fn retained_result_read_type(&self, span: ast::Span, recorded: IncanType) -> IncanType {
        let Some(local) = self
            .type_info
            .resolved_identity(span)
            .and_then(|identity| self.identity_bindings.get(identity))
            .and_then(|local| self.locals.get(local.index()))
        else {
            return recorded;
        };
        self.refine_result_channels(&recorded, &local.ty)
    }

    /// Keep a rebind's carrier from its exact checked local, never a same-spelled shadow. An intrinsic constructor
    /// whose payload legacy left open remains explicitly unsupported: retaining a Body IR type does not prove that
    /// legacy can build that reassignment.
    pub(super) fn retained_result_binding_type(
        &mut self,
        assignment: &ast::AssignmentStmt,
        recorded: IncanType,
        out: &mut Vec<bir::Statement>,
    ) -> IncanType {
        let Some(local) = self
            .type_info
            .resolved_write_identity(assignment.name_span, &assignment.name)
            .and_then(|identity| self.identity_bindings.get(identity))
            .and_then(|local| self.locals.get(local.index()))
        else {
            return recorded;
        };
        let refined = self.refine_result_channels(&recorded, &local.ty);
        let mut value = &assignment.value;
        while let ast::Expr::Paren(inner) = &value.node {
            value = inner;
        }
        if refined != recorded
            && let ast::Expr::Call(callee, type_args, _) = &value.node
            && type_args.is_empty()
            && self.intrinsic_result_constructor(callee, value.span).is_some()
        {
            self.push_unsupported_stmt(
                "Result constructor rebinding without a legacy-retained payload".to_string(),
                hir_span(value.span),
                out,
            );
        }
        refined
    }

    /// Select only a registry-owned builtin Result member, excluding same-spelled source methods.
    fn checked_result_method(&self, span: ast::Span) -> Option<ResultMethodId> {
        let identity = self.type_info.resolved_identity(span)?;
        if identity.origin != SymbolOrigin::Builtin
            || identity.namespace != SymbolNamespace::Member
            || identity.kind != SemanticSourceTargetKind::Method
            || identity.scope_discriminant.is_some()
            || identity.declaration_span != HirSourceSpan::new(0, 0)
        {
            return None;
        }
        let member = identity
            .declaration_name
            .strip_prefix(collections::as_str(CollectionTypeId::Result))?
            .strip_prefix('.')?;
        result_methods::from_str(member)
    }

    /// Carry a checked chaining callback's closed result into its call, retaining the unselected channel.
    pub(super) fn retain_result_call_channels(
        &mut self,
        span: ast::Span,
        receiver: ast::Span,
        arguments: &[bir::ArgumentElement],
    ) {
        // ---- Context: only the checked builtin owns the Result channel rule ----
        let Some(method) = self.checked_result_method(span) else {
            return;
        };

        // ---- Context: retain the selected callback or receiver payload ----
        let actual = match method {
            ResultMethodId::AndThen | ResultMethodId::OrElse => {
                let Some(value) = arguments
                    .get(1)
                    .and_then(bir::ArgumentElement::as_one)
                    .and_then(|value| self.result_operand_type(value))
                else {
                    return;
                };
                let IncanType::Function { return_type, .. } = value else {
                    return;
                };
                *return_type
            }
            ResultMethodId::Unwrap | ResultMethodId::UnwrapOr => {
                let input = self.resolve_ty(receiver);
                let Some((ok, _)) = result_type_parts(&input) else {
                    return;
                };
                if self.leaves_type_open(ok) {
                    return;
                }
                self.retain_scalar_result(span, ok.clone());
                return;
            }
            _ => return,
        };

        // ---- Context: preserve fixed call channels and fill only their holes ----
        let recorded = self.resolve_ty(span);
        let refined = self.refine_result_channels(&recorded, &actual);
        if refined != recorded {
            self.type_overrides.insert((span.start, span.end), refined);
        }
    }

    /// Fill a missing selected payload type from its checked Result member, preserving every fixed type.
    fn retain_scalar_result(&mut self, span: ast::Span, actual: IncanType) -> bool {
        if matches!(self.resolve_ty(span), IncanType::Unknown | IncanType::Infer) && !self.leaves_type_open(&actual) {
            self.type_overrides.insert((span.start, span.end), actual);
            return true;
        }
        false
    }

    /// Preserve selected payload facts before binary admission, which precedes operand evaluation.
    /// Only an int addition with a newly retained Result payload fills its missing scalar result; unrelated unknown
    /// operations retain their existing refusal. This pass records types and never evaluates source expressions.
    pub(super) fn retain_result_scalar_context(&mut self, expression: &ast::Spanned<ast::Expr>) -> bool {
        // ---- Context: payload reads must be known before binary admission ----
        match &expression.node {
            ast::Expr::MethodCall(receiver, _, _, _)
                if matches!(
                    self.checked_result_method(expression.span),
                    Some(ResultMethodId::Unwrap | ResultMethodId::UnwrapOr)
                ) =>
            {
                let input = self.resolve_ty(receiver.span);
                let Some((ok, _)) = result_type_parts(&input) else {
                    return false;
                };
                self.retain_scalar_result(expression.span, ok.clone())
            }
            ast::Expr::Binary(left, op, right) => {
                let left_changed = self.retain_result_scalar_context(left);
                let right_changed = self.retain_result_scalar_context(right);
                let int = IncanType::Primitive(IncanPrimitiveType::Int);
                if (left_changed || right_changed)
                    && *op == ast::BinaryOp::Add
                    && self.resolve_ty(left.span) == int
                    && self.resolve_ty(right.span) == int
                {
                    self.retain_scalar_result(expression.span, int);
                }
                left_changed || right_changed
            }
            ast::Expr::Paren(inner) => self.retain_result_scalar_context(inner),
            _ => false,
        }
    }
}

/// Fill unresolved leaves only, without the destination-driven union widening used by constructor coercions.
fn fill_channel_holes(recorded: &IncanType, actual: &IncanType) -> IncanType {
    match (recorded, actual) {
        (IncanType::Unknown | IncanType::Infer, _) => actual.clone(),
        (
            IncanType::Generic { base, args },
            IncanType::Generic {
                base: actual_base,
                args: actual_args,
            },
        ) if base == actual_base && args.len() == actual_args.len() => IncanType::Generic {
            base: base.clone(),
            args: args
                .iter()
                .zip(actual_args)
                .map(|(recorded, actual)| fill_channel_holes(recorded, actual))
                .collect(),
        },
        (IncanType::Tuple(items), IncanType::Tuple(actual_items)) if items.len() == actual_items.len() => {
            IncanType::Tuple(
                items
                    .iter()
                    .zip(actual_items)
                    .map(|(recorded, actual)| fill_channel_holes(recorded, actual))
                    .collect(),
            )
        }
        _ => recorded.clone(),
    }
}
