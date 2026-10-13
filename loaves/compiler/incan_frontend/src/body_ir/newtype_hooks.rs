//! Source-local, checker-selected fail-fast newtype validation ingress.

use super::args::fixed_elements;
use super::{BodyBuilder, CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanType, bir};

impl BodyBuilder<'_, '_> {
    /// Replace outside-owner explicit construction with its checked hook; raw constructions stay aggregates.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn checked_newtype_construction(
        &mut self,
        name: &str,
        canonical: Option<&CanonicalSymbolId>,
        operands: &[bir::Operand],
        ty: &IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let identity = canonical.filter(|identity| identity.kind == super::SemanticSourceTargetKind::Newtype)?;
        if self.newtype_implementation.as_ref() == Some(identity) {
            return None;
        }
        let declaration = self
            .local_nominal_declarations
            .values()
            .find(|declaration| &declaration.canonical == identity)?;
        let facts = self
            .type_info
            .declarations
            .newtype_construction
            .get(name)
            .filter(|_| self.type_info.declarations.named_type_identities.get(name) == Some(identity));
        let Some(facts) = facts else {
            return Some(self.unsupported_typed_operand(
                "newtype construction without checked ingress metadata".into(),
                ty.clone(),
                scope,
                span,
                out,
            ));
        };
        facts.checked_constructor.as_ref()?;
        if incan_semantics_core::canonical_module_identity(&declaration.canonical).as_deref()
            != Some(self.module_identity)
        {
            return Some(self.unsupported_typed_operand(
                "checked-newtype imported hook body".into(),
                ty.clone(),
                scope,
                span,
                out,
            ));
        }
        let Some(identity) = facts.checked_constructor_identity.as_ref() else {
            return Some(self.unsupported_typed_operand(
                "checked-newtype constructor without a checked hook identity".into(),
                ty.clone(),
                scope,
                span,
                out,
            ));
        };
        if operands.len() != 1 || !facts.type_params.is_empty() {
            return Some(self.unsupported_operand("checked-newtype constructor profile".into(), scope, span, out));
        }
        Some(self.checked_newtype_hook(
            name,
            identity.clone(),
            operands[0].clone(),
            ty.clone(),
            scope,
            span,
            out,
        ))
    }

    /// Invoke exactly the source hook selected by checking, then unwrap its Result with legacy's failure primitive.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn checked_newtype_hook(
        &mut self,
        owner: &str,
        identity: CanonicalSymbolId,
        value: bir::Operand,
        ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hook_name = identity.declaration_name.clone();
        let checked = self.checked_newtype_hook_call(owner, identity, value, &ty, scope, span, out);
        self.push_call_temp(
            bir::Callee::Helper(bir::HelperOp::NewtypeHookExtract),
            fixed_elements(vec![
                checked,
                bir::Operand::Constant(bir::Constant::Str(owner.to_owned())),
                bir::Operand::Constant(bir::Constant::Str(hook_name)),
            ]),
            ty,
            scope,
            span,
            true,
            out,
        )
    }

    /// Call the canonical source hook without choosing a failure strategy; its caller owns Result extraction.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn checked_newtype_hook_call(
        &mut self,
        owner: &str,
        identity: CanonicalSymbolId,
        value: bir::Operand,
        ty: &IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let Some(((start, end), binding)) = self
            .type_info
            .declarations
            .method_bindings_by_span
            .iter()
            .find(|(_, binding)| binding.identity.as_ref() == Some(&identity))
        else {
            return self.unsupported_operand(
                "checked-newtype hook without its source binding".into(),
                scope,
                span,
                out,
            );
        };
        if binding.params.len() != 1 || binding.is_async {
            return self.unsupported_operand("checked-newtype hook signature".into(), scope, span, out);
        }
        let direct_call_id = CompilerNodeId::declaration_span(self.module_identity, *start, *end);
        let returned = super::bodies::close_self_type(self.checked_type(&binding.return_type), ty);
        let IncanType::Generic { base, args } = &returned else {
            return self.unsupported_operand("checked-newtype hook return".into(), scope, span, out);
        };
        if base != "Result" || args.len() != 2 || &args[0] != ty {
            return self.unsupported_operand("checked-newtype hook result carrier".into(), scope, span, out);
        }
        let hook_name = identity.declaration_name.clone();
        self.push_call_temp(
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                receiver_type: Some(ty.clone()),
                name: format!("{owner}.{hook_name}"),
                direct_call_id: Some(direct_call_id),
                canonical: Some(identity),
                builtin: None,
                type_args: Vec::new(),
                binding: bir::ArgumentBinding::resolved_positional(1),
            })),
            fixed_elements(vec![value]),
            returned,
            scope,
            span,
            false,
            out,
        )
    }
}
