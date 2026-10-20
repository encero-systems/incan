//! Checked Rust callees projected into the existing external callable and named-call machinery (DD-0006).

use super::args::DeclaredSlot;
use super::{BodyBuilder, IncanType, TypeCheckInfo, ast, bir, hir_span, parameter_types};

/// Declare each checked Rust instantiation once, keeping native argument order and reference passing modes.
/// The signature is already instantiated by checking; this boundary only projects its types into Body IR.
pub(super) fn declarations(facts: &TypeCheckInfo) -> Vec<bir::LoafCallable> {
    let mut calls = Vec::new();
    for call in facts.rust.checked_calls.values() {
        let Some(canonical) = bir::rust_item_identity(&call.path) else {
            continue;
        };
        let checked = |ty| parameter_types::checked_semantic_type(ty, facts);
        let declaration = bir::LoafCallable {
            canonical,
            item: call.path.clone(),
            parameters: call.parameters.iter().map(|parameter| checked(&parameter.ty)).collect(),
            return_type: checked(&call.return_type),
            type_parameters: Vec::new(),
            native_type_arguments: call.type_arguments.iter().map(checked).collect(),
            parameter_defaults: Vec::new(),
        };
        if !calls.contains(&declaration) {
            calls.push(declaration);
        }
    }
    calls.sort_by(|left, right| {
        left.item.cmp(&right.item).then_with(|| {
            format!("{:?}", left.native_type_arguments).cmp(&format!("{:?}", right.native_type_arguments))
        })
    });
    calls
}

impl BodyBuilder<'_, '_> {
    /// Lower a metadata-selected Rust call through ordinary declared-slot binding and a named external target.
    /// An absent record grants no foreign call behavior; existing source and builtin resolution keeps priority.
    pub(super) fn lower_checked_rust_call(
        &mut self,
        receiver: Option<&ast::Spanned<ast::Expr>>,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let call = self.type_info.rust.checked_calls.get(&(span.start, span.end))?.clone();
        let canonical = bir::rust_item_identity(&call.path)?;
        let mut arguments = Vec::new();
        if call.has_receiver {
            arguments.push(ast::CallArg::Positional(receiver?.clone()));
        }
        arguments.extend_from_slice(args);
        let slots = call
            .parameters
            .iter()
            .map(|parameter| DeclaredSlot {
                ty: self.checked_type(&parameter.ty),
                ..DeclaredSlot::from_checked_param(parameter)
            })
            .collect();
        let (operands, binding) = match self.bind_declared_args(&call.path, Some(slots), &arguments, scope, out) {
            Ok(bound) => bound,
            Err(reason) => return Some(self.unsupported_operand(reason, scope, hir_span(span), out)),
        };
        let type_args = call
            .type_arguments
            .iter()
            .map(|argument| self.checked_type(argument))
            .collect();
        let result: IncanType = self.checked_type(&call.return_type);
        Some(self.push_call_temp(
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                name: call.path,
                direct_call_id: None,
                builtin: None,
                type_args,
                binding,
                canonical: Some(canonical),
                receiver_type: None,
            })),
            operands,
            result,
            scope,
            hir_span(span),
            false,
            out,
        ))
    }
}
