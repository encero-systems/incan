//! Preserve checker-ordered, source-local newtype ingress without changing legacy coercion facts.

use super::args::fixed_elements;
use super::{BodyBuilder, HirSourceSpan, IncanType, SemanticSourceTargetKind, ValidatedNewtypeCoercionInfo, ast, bir};
use crate::typechecker::ValidatedNewtypeCoercionMode;

impl BodyBuilder<'_, '_> {
    /// Evaluate one source input and apply the checker's steps in underlying-to-target order; aggregate errors refuse.
    /// A qualified target must encode the exact selected owner's canonical identity.
    pub(super) fn lower_newtype_ingress(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        coercion: &ValidatedNewtypeCoercionInfo,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let span = super::hir_span(expr.span);
        let target = self.checked_type(&coercion.target_type);

        // ---- Context: model fields require error collection rather than fail-fast checks ----
        if matches!(coercion.mode, ValidatedNewtypeCoercionMode::AggregateField { .. })
            && coercion
                .steps
                .iter()
                .any(|step| step.ctor.is_some() || !step.constraints.is_empty())
        {
            let description = if coercion.steps.iter().any(|step| step.ctor.is_some()) {
                "checked-newtype aggregate-field validation"
            } else {
                "constrained-newtype aggregate-field validation"
            };
            return self.unsupported_typed_operand(description.into(), target, scope, span, out);
        }

        // ---- Context: each step needs its source-local canonical tuple layout ----
        let declarations = coercion
            .steps
            .iter()
            .map(|step| {
                self.local_nominal_declarations
                    .get(&step.newtype_name)
                    .filter(|declaration| {
                        declaration.canonical.kind == SemanticSourceTargetKind::Newtype
                            && declaration.fields == ["0"]
                            && declaration.type_parameters.is_empty()
                    })
                    .cloned()
            })
            .collect::<Option<Vec<_>>>();
        let Some(declarations) = declarations.filter(|declarations| !declarations.is_empty()) else {
            return self.unsupported_typed_operand("validated-newtype coercion owner".into(), target, scope, span, out);
        };
        let target_matches = declarations.last().is_some_and(|declaration| {
            matches!(&target, IncanType::Named(name)
                if name == &declaration.name
                    || bir::nominal_carrier_key(&declaration.canonical).as_ref() == Some(name))
        });
        if !target_matches {
            return self.unsupported_typed_operand(
                "validated-newtype coercion target".into(),
                target,
                scope,
                span,
                out,
            );
        }
        let key = (expr.span.start, expr.span.end);

        // ---- Context: evaluate the original source input once before wrapping ----
        self.coercions_in_progress.insert(key);
        let mut value = self.lower_expr_to_operand(expr, scope, out);
        self.coercions_in_progress.remove(&key);

        // ---- Context: checked underlying-to-target construction order ----
        for (step, declaration) in coercion.steps.iter().zip(declarations) {
            let ty = IncanType::Named(
                self.retain_nominal_carriers
                    .then(|| bir::nominal_carrier_key(&declaration.canonical))
                    .flatten()
                    .unwrap_or_else(|| declaration.name.clone()),
            );
            if step.ctor.is_some() {
                let Some(identity) = step.ctor_identity.as_ref().filter(|identity| {
                    self.type_info
                        .declarations
                        .newtype_construction
                        .get(&step.newtype_name)
                        .is_some_and(|facts| facts.checked_constructor_identity.as_ref() == Some(identity))
                }) else {
                    return self.unsupported_typed_operand(
                        "validated-newtype coercion hook identity".into(),
                        target,
                        scope,
                        span,
                        out,
                    );
                };
                value = self.checked_newtype_hook(&declaration.name, identity.clone(), value, ty, scope, span, out);
            } else {
                if !step.constraints.is_empty() {
                    value =
                        self.constrained_newtype_input(&declaration.name, value, &step.constraints, scope, span, out);
                }
                value = self.wrap_newtype_ingress(declaration, value, ty, scope, span, out);
            }
        }
        value
    }

    /// Retain the checked canonical tuple layout after any validation ingress; this helper does not authorize bypass.
    fn wrap_newtype_ingress(
        &mut self,
        declaration: bir::NominalDeclaration,
        value: bir::Operand,
        ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        self.push_assign_temp(
            bir::Rvalue::Aggregate(
                bir::AggregateKind::Constructor(Box::new(bir::ConstructorTarget {
                    name: declaration.name,
                    canonical: Some(declaration.canonical),
                    direct_declaration_id: Some(declaration.direct_declaration_id),
                    canonical_field_layout: Some(declaration.fields),
                    binding: bir::ArgumentBinding::resolved_positional(1),
                })),
                fixed_elements(vec![value]),
            ),
            ty,
            scope,
            span,
            out,
        )
    }
}
