//! Pure generated type metadata retains the checker's member and owner identities through generic closure.

use super::*;
use incan_lang::lang::builtins::BuiltinFnId;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Retain a generated member only when the receiver's checked nominal identity is exactly its owner. A type
    /// receiver uses its declaration identity; an instance uses its retained carrier identity, never a bare spelling.
    fn checked_type_metadata_target(
        &self,
        receiver: &ast::Spanned<ast::Expr>,
        span: ast::Span,
        operation: bir::TypeMetadataOperation,
    ) -> Option<bir::TypeMetadataTarget> {
        let member = self.type_info.resolved_identity(span)?;
        let owner = self.type_info.calls.compiler_generated_member_identities.get(member)?;
        let receiver_type = self.metadata_receiver_type(receiver, span, owner)?;
        let expected = match operation {
            bir::TypeMetadataOperation::ClassName => "__class_name__",
            bir::TypeMetadataOperation::FieldCount => "__fields__",
        };
        if member.declaration_name != expected {
            return None;
        }
        Some(bir::TypeMetadataTarget {
            member: member.clone(),
            owner: owner.clone(),
            receiver_type,
            operation,
        })
    }

    /// Read a generated member receiver's nominal carrier from checked type or instance provenance.
    fn metadata_receiver_type(
        &self,
        receiver: &ast::Spanned<ast::Expr>,
        span: ast::Span,
        owner: &CanonicalSymbolId,
    ) -> Option<IncanType> {
        if let Some(ty) = self.static_receiver_type(receiver, span) {
            return (self.type_info.resolved_identity(receiver.span) == Some(owner)).then_some(ty);
        }
        let ty = self.resolve_ty(receiver.span);
        let name = match &ty {
            IncanType::Named(name) | IncanType::Generic { base: name, .. } => name,
            _ => return None,
        };
        (parameter_types::retained_nominal_identity(name, self.type_info) == Some(owner)).then_some(ty)
    }

    /// Emit a compiler-generated class name, evaluating an instance receiver once while leaving type receivers pure.
    pub(super) fn lower_type_class_name(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        if !type_args.is_empty() || !args.is_empty() {
            return None;
        }
        let target = self.checked_type_metadata_target(receiver, span, bir::TypeMetadataOperation::ClassName)?;
        if self.static_receiver_type(receiver, span).is_none() {
            self.lower_expr_to_operand(receiver, scope, out);
        }
        Some(self.push_call_temp(
            bir::Callee::TypeMetadata(target),
            Vec::new(),
            self.resolve_ty(span),
            scope,
            hir_span(span),
            false,
            out,
        ))
    }

    /// Fuse only canonical builtin length of an effect-free generated type field table; no elements escape.
    pub(super) fn lower_type_field_count(
        &mut self,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        if !type_args.is_empty() || self.type_info.resolved_builtin_call(span) != Some(BuiltinFnId::Len) {
            return None;
        }
        let [ast::CallArg::Positional(table)] = args else {
            return None;
        };
        let ast::Expr::MethodCall(receiver, _, types, values) = &table.node else {
            return None;
        };
        if !types.is_empty() || !values.is_empty() {
            return None;
        }
        let target = self.checked_type_metadata_target(receiver, table.span, bir::TypeMetadataOperation::FieldCount)?;
        if self.static_receiver_type(receiver, table.span).is_none() {
            self.lower_expr_to_operand(receiver, scope, out);
        }
        Some(self.push_call_temp(
            bir::Callee::TypeMetadata(target),
            Vec::new(),
            self.resolve_ty(span),
            scope,
            hir_span(span),
            false,
            out,
        ))
    }
}
