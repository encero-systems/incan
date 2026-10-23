//! Checker-selected fail-fast newtype validation ingress, through local bodies or published Loaf signatures.

use super::args::fixed_elements;
use super::{BodyBuilder, CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanType, bir};
use crate::typechecker::FunctionBindingInfo;

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
        self.local_nominal_declarations
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

    /// Invoke exactly the hook selected by checking, then extract its Result with the admitted validation-failure
    /// primitive.
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

    /// Find a hook's exact checked signature, retaining a direct span only for declarations in this module.
    /// Imported signatures come from the constructor's checked owner binding and must carry the selected hook
    /// identity. Another alias of the same declaration may use a different local result name and cannot substitute.
    fn checked_newtype_hook_binding(
        &self,
        owner: &str,
        identity: &CanonicalSymbolId,
    ) -> Option<(Option<CompilerNodeId>, &FunctionBindingInfo)> {
        if incan_semantics_core::canonical_module_identity(identity).as_deref() == Some(self.module_identity) {
            let ((start, end), binding) = self
                .type_info
                .declarations
                .method_bindings_by_span
                .iter()
                .find(|(_, binding)| binding.identity.as_ref() == Some(identity))?;
            return Some((
                Some(CompilerNodeId::declaration_span(self.module_identity, *start, *end)),
                binding,
            ));
        }
        let binding = self
            .type_info
            .declarations
            .newtype_construction
            .get(owner)?
            .checked_constructor_binding
            .as_ref()
            .filter(|binding| binding.identity.as_ref() == Some(identity))?;
        Some((None, binding))
    }

    /// Call the canonical checked hook without choosing a failure strategy; its caller owns Result extraction.
    /// A published hook has no consumer-local declaration span. Its exact return carrier comes from the retained
    /// Loaf signature, and native metadata verifies the argument and result contract before the call is admitted.
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
        let (direct_call_id, returned) = if let Some((direct_call_id, binding)) =
            self.checked_newtype_hook_binding(owner, &identity)
        {
            if binding.params.len() != 1 || binding.is_async {
                return self.unsupported_operand("checked-newtype hook signature".into(), scope, span, out);
            }
            (
                direct_call_id,
                super::bodies::close_self_type(self.checked_type(&binding.return_type), ty),
            )
        } else if let Some(returned) = self.published_callable_returns.get(&identity) {
            (None, returned.clone())
        } else {
            return self.unsupported_operand("checked-newtype hook without its declaration".into(), scope, span, out);
        };
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
                direct_call_id,
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
