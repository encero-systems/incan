//! Retain consumer method selection after a provider's private alias return, without changing legacy checker facts.

use super::{BodyBuilder, CanonicalSymbolId, IncanType, SemanticSourceTargetKind, ast, bir};
use incan_semantics_core::SymbolNamespace;

/// One checked, provider-owned immutable method taking only its receiver.
#[derive(Clone)]
pub(super) struct PublishedMethod {
    /// Exact declared owner, including its module and declaration anchor.
    pub(super) owner: CanonicalSymbolId,
    /// Exact checked method declaration; consumers never reconstruct this from an encoded name.
    pub(super) canonical: CanonicalSymbolId,
    /// Declared result carrier retained by the providing body; unresolved storage still refuses at admission.
    pub(super) return_type: IncanType,
}

/// Collect nongeneric, receiver-only immutable method frames within their exact retained owner declarations.
///
/// The declaration anchors, origin, receiver type and canonical member identity must agree. Static methods, mutable
/// receivers and ordinary parameter/default binding are outside this record's contract and remain unselected here.
pub(super) fn collect(context: &[bir::BodyIrModule]) -> Vec<PublishedMethod> {
    let mut methods: Vec<PublishedMethod> = Vec::new();
    for module in context {
        for owner in &module.nominal_declarations {
            for body in module.callable_declaration_frames() {
                let Some(identity) = &body.canonical else {
                    continue;
                };
                if identity.kind != SemanticSourceTargetKind::Method
                    || identity.namespace != SymbolNamespace::Member
                    || identity.origin != owner.canonical.origin
                    || identity.declaration_span.start < owner.canonical.declaration_span.start
                    || identity.declaration_span.end > owner.canonical.declaration_span.end
                    || !body.type_parameters.is_empty()
                {
                    continue;
                }
                let [receiver] = body.params.as_slice() else {
                    continue;
                };
                let IncanType::Named(receiver_name) = &receiver.ty else {
                    continue;
                };
                if receiver.name != "self"
                    || receiver.mutable
                    || body.named_type_identities.get(receiver_name) != Some(&owner.canonical)
                {
                    continue;
                }
                let candidate = PublishedMethod {
                    owner: owner.canonical.clone(),
                    canonical: identity.clone(),
                    return_type: body.return_type.clone(),
                };
                if !methods.iter().any(|known| {
                    known.owner == candidate.owner
                        && known.canonical == candidate.canonical
                        && known.return_type == candidate.return_type
                }) {
                    methods.push(candidate);
                }
            }
        }
    }
    methods
}

impl BodyBuilder<'_, '_> {
    /// Select one exact provider method only when a checked owner survives the private alias return.
    ///
    /// Existing checker selections are never replaced. An absent owner, missing body, competing method or unsupported
    /// argument/receiver contract leaves the call unresolved; a same-spelled owner in another module grants nothing.
    pub(super) fn retained_alias_method(
        &self,
        receiver: &ast::Spanned<ast::Expr>,
        name: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
    ) -> Option<PublishedMethod> {
        if !type_args.is_empty()
            || !args.is_empty()
            || self.type_info.resolved_identity(span).is_some()
            || self.type_info.traits.self_method_identities.contains_key(&span)
        {
            return None;
        }
        let IncanType::Named(carrier) = self
            .published_expression_type(receiver)
            .unwrap_or_else(|| self.resolve_ty(receiver.span))
        else {
            return None;
        };
        let owner = super::parameter_types::retained_nominal_identity(&carrier, self.type_info)?;
        let mut candidates = self
            .published_methods
            .iter()
            .filter(|method| &method.owner == owner && method.canonical.declaration_name == name);
        let selected = candidates.next()?;
        if candidates.next().is_some() {
            return None;
        }
        Some(selected.clone())
    }
}
