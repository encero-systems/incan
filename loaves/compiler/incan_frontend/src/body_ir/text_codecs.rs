//! Retain the selected builtin codec's ordered arguments and registry-owned constant defaults.

use super::args::{DeclaredSlot, plan_declared_args};
use super::{BodyBuilder, ast, bir, hir_span};
use incan_lang::lang::text_codecs::{DecodeErrorsPolicy, UTF8_ENCODING_LABEL};
use incan_semantics_core::SymbolOrigin;

impl BodyBuilder<'_, '_> {
    /// Fill builtin codec slots after evaluating the receiver and supplied expressions in source order. Only the
    /// checker's builtin member identity authorizes defaults; application members retain their original binding.
    pub(super) fn lower_text_codec_call(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let identity = self.type_info.resolved_identity(span)?.clone();
        if identity.origin != SymbolOrigin::Builtin {
            return None;
        }
        let defaults = match identity.declaration_name.as_str() {
            "str.encode" | "frozenstr.encode" => vec![("encoding", UTF8_ENCODING_LABEL)],
            "bytes.decode" | "frozenbytes.decode" => vec![
                ("encoding", UTF8_ENCODING_LABEL),
                ("errors", DecodeErrorsPolicy::DEFAULT.as_str()),
            ],
            _ => return None,
        };
        let slots = defaults
            .iter()
            .map(|(name, _)| DeclaredSlot {
                name: Some((*name).to_owned()),
                has_default: true,
                is_partial_preset: false,
                is_rest: false,
                is_mut: false,
            })
            .collect::<Vec<_>>();
        let planned = match plan_declared_args(&identity.declaration_name, &slots, args) {
            Ok(planned) => planned,
            Err(reason) => return Some(self.unsupported_operand(reason, scope, hir_span(span), out)),
        };
        let place = self.lower_expr_to_place(receiver, scope, out);
        let receiver = bir::Operand::place(place, bir::OwnershipFact::Borrow, false);
        let (supplied, binding) = match self.lower_planned_args(&planned, slots.len(), &[], scope, out) {
            Ok(bound) => bound,
            Err(reason) => return Some(self.unsupported_operand(reason, scope, hir_span(span), out)),
        };
        let bir::ArgumentBinding::Resolved { arguments, .. } = binding else {
            return Some(self.unsupported_operand(
                "builtin codec has no checked slot binding".to_owned(),
                scope,
                hir_span(span),
                out,
            ));
        };
        let mut operands = vec![bir::ArgumentElement::One(receiver)];
        for (slot, (_, default)) in defaults.iter().enumerate() {
            let operand = if let Some(index) = arguments.iter().position(|argument| argument.slot == slot) {
                let Some(operand) = supplied.get(index) else {
                    return Some(self.unsupported_operand(
                        "builtin codec is missing a checked operand".to_owned(),
                        scope,
                        hir_span(span),
                        out,
                    ));
                };
                operand.clone()
            } else {
                bir::Operand::Constant(bir::Constant::Str((*default).to_owned()))
            };
            operands.push(bir::ArgumentElement::One(operand));
        }
        Some(self.push_call_temp(
            bir::Callee::Method(bir::MethodTarget {
                name: identity.declaration_name.rsplit('.').next()?.to_owned(),
                intrinsic_carrier: None,
                canonical: Some(identity),
                type_args: Vec::new(),
                binding: bir::ArgumentBinding::resolved_positional(defaults.len()),
            }),
            operands,
            self.method_call_result_type(span),
            scope,
            hir_span(span),
            false,
            out,
        ))
    }
}
