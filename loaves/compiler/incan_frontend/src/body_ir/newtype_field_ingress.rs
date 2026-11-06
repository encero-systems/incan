//! Exact single-field model validation without admitting multi-field fail-fast behavior.

use super::args::fixed_elements;
use super::{BodyBuilder, CanonicalSymbolId, HirSourceSpan, IncanType, SemanticSourceTargetKind, ast, bir};
use crate::typechecker::ValidatedNewtypeCoercionMode;

impl BodyBuilder<'_, '_> {
    /// Specialize only a sole supplied field with one checked hook and no constraints or omitted defaults.
    ///
    /// Multi-field constructors must collect every supplied failure before extracting any value; they remain on the
    /// ordinary named-refusal path. Canonical constructor and field identities authorize these diagnostic labels.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn single_field_checked_ingress(
        &mut self,
        canonical: Option<&CanonicalSymbolId>,
        planned: &[(usize, &ast::Spanned<ast::Expr>)],
        field_count: usize,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let [(0, expression)] = planned else {
            return None;
        };
        if field_count != 1 {
            return None;
        }
        let identity = canonical?;
        if !matches!(
            identity.kind,
            SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
        ) {
            return None;
        }
        let coercion = self.type_info.validated_newtype_coercion(expression.span)?;
        let ValidatedNewtypeCoercionMode::AggregateField { field_name } = &coercion.mode else {
            return None;
        };
        let [step] = coercion.steps.as_slice() else {
            return None;
        };
        if step.ctor.is_none() || !step.constraints.is_empty() {
            return None;
        }
        let constructor = self
            .local_nominal_declarations
            .values()
            .find(|declaration| &declaration.canonical == identity && declaration.fields == [field_name.clone()])?;
        if incan_semantics_core::canonical_module_identity(&constructor.canonical).as_deref()
            != Some(self.module_identity)
        {
            return None;
        }
        let target_label = constructor.canonical.declaration_name.clone();
        let field_label = field_name.clone();
        let wrapper = self
            .local_nominal_declarations
            .get(&step.newtype_name)
            .filter(|declaration| {
                declaration.canonical.kind == SemanticSourceTargetKind::Newtype
                    && declaration.fields == ["0"]
                    && declaration.type_parameters.is_empty()
            })?;
        if incan_semantics_core::canonical_module_identity(&wrapper.canonical).as_deref() != Some(self.module_identity)
        {
            return None;
        }
        let target = self.checked_type(&coercion.target_type);
        let mut field_types = constructor.field_types.clone();
        if self.retain_nominal_carriers {
            for field in &mut field_types {
                super::nominal_carriers::retain_declared_type(field, &constructor.named_type_identities);
            }
        }
        if !matches!(&target, IncanType::Named(name) if name == &wrapper.name || bir::nominal_carrier_key(&wrapper.canonical).as_ref() == Some(name))
            || field_types != [target.clone()]
            || constructor.field_identities.len() != 1
        {
            return None;
        }
        let hook = step.ctor_identity.as_ref().filter(|identity| {
            self.type_info
                .declarations
                .newtype_construction
                .get(&step.newtype_name)
                .is_some_and(|facts| {
                    facts.checked_constructor_identity.as_ref() == Some(identity)
                        && self
                            .type_info
                            .declarations
                            .named_type_identities
                            .get(&step.newtype_name)
                            == Some(&wrapper.canonical)
                })
        })?;
        let owner = wrapper.name.clone();
        let hook = hook.clone();
        let key = (expression.span.start, expression.span.end);
        self.coercions_in_progress.insert(key);
        let input = self.lower_expr_to_operand(expression, scope, out);
        self.coercions_in_progress.remove(&key);
        let checked = self.checked_newtype_hook_call(&owner, hook, input, &target, scope, span, out);
        Some(self.push_call_temp(
            bir::Callee::Helper(bir::HelperOp::NewtypeFieldHookExtract),
            fixed_elements(vec![
                checked,
                bir::Operand::Constant(bir::Constant::Str(target_label)),
                bir::Operand::Constant(bir::Constant::Str(field_label)),
            ]),
            target,
            scope,
            span,
            true,
            out,
        ))
    }
}
