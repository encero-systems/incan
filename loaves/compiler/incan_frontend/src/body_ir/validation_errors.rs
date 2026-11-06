//! Checked builtin validation error construction, distinct from same-spelled source declarations.

use super::args::fixed_elements;
use super::{BodyBuilder, IncanType, ast, bir};
use incan_semantics_core::RuntimeBuiltinType;

impl BodyBuilder<'_, '_> {
    /// Preserve legacy's message-first and final-code evaluation after both checked identities select the builtin.
    /// Capture a scalar or text message before evaluating a code expression that can mutate its global storage.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_validation_error_construction(
        &mut self,
        callee: &ast::Spanned<ast::Expr>,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let selected = |span| {
            self.type_info
                .resolved_identity(span)
                .and_then(RuntimeBuiltinType::from_identity)
                == Some(RuntimeBuiltinType::ValidationError)
        };
        let callee_selected = selected(callee.span);
        let call_selected = selected(span);
        if !callee_selected && !call_selected {
            return None;
        }
        let ty = IncanType::NativeBuiltin(RuntimeBuiltinType::ValidationError);
        let source_span = super::hir_span(span);
        if !callee_selected || !call_selected || self.resolve_ty(span) != ty || !type_args.is_empty() {
            return Some(self.unsupported_typed_operand(
                "builtin validation error construction without complete checked identity".into(),
                ty,
                scope,
                source_span,
                out,
            ));
        }
        let mut message = None;
        let mut code = None;
        for argument in args {
            match argument {
                ast::CallArg::Positional(value) if message.is_none() => message = Some(value),
                ast::CallArg::Named(name, value) if name.node == "message" && message.is_none() => {
                    message = Some(value)
                }
                ast::CallArg::Named(name, value) if name.node == "code" => code = Some(value),
                _ => {
                    return Some(self.unsupported_typed_operand(
                        "builtin validation error constructor argument profile".into(),
                        ty,
                        scope,
                        source_span,
                        out,
                    ));
                }
            }
        }
        let Some(message) = message else {
            return Some(self.unsupported_typed_operand(
                "builtin validation error constructor without its message".into(),
                ty,
                scope,
                source_span,
                out,
            ));
        };
        let message_value = self.lower_expr_to_operand(message, scope, out);
        let message_value = self.retain_evaluated_value_read(message, message_value, scope, out);
        let mut operands = vec![message_value];
        let operation = if let Some(code) = code {
            operands.push(self.lower_expr_to_operand(code, scope, out));
            bir::HelperOp::ValidationErrorWithCode
        } else {
            bir::HelperOp::ValidationErrorNew
        };
        Some(self.push_call_temp(
            bir::Callee::Helper(operation),
            fixed_elements(operands),
            ty,
            scope,
            source_span,
            false,
            out,
        ))
    }
}
