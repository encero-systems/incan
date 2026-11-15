//! Preserve aggregate validation when exactly one supplied field can fail.

use super::args::fixed_elements;
use super::{BodyBuilder, CanonicalSymbolId, HirSourceSpan, IncanType, SemanticSourceTargetKind, ast, bir};
use crate::typechecker::ValidatedNewtypeCoercionMode;

/// Canonical authorization for one checked field among fully supplied constructor arguments.
struct CheckedFieldIngress {
    slot: usize,
    owner: String,
    hook: CanonicalSymbolId,
    target: IncanType,
    constructor_label: String,
    field_label: String,
}

impl BodyBuilder<'_, '_> {
    /// Lower a fully supplied constructor with one fallible field, retaining its result until all arguments run.
    ///
    /// Multiple checked fields, constraints, chained coercions, generic owners, and omitted defaults remain on the
    /// ordinary aggregate refusal path. One failure can use the existing field-error extractor after every supplied
    /// argument has been evaluated; it must never short-circuit the remaining argument effects.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn single_checked_field_ingress(
        &mut self,
        canonical: Option<&CanonicalSymbolId>,
        planned: &[(usize, &ast::Spanned<ast::Expr>)],
        field_count: usize,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Option<Result<(Vec<bir::Operand>, bir::ArgumentBinding), String>> {
        let plan = self.checked_field_ingress_plan(canonical?, planned, field_count)?;
        let pending_start = self.pending_argument_reads.len();
        let result = self.lower_checked_field_arguments(plan, planned, field_count, scope, span, out);
        self.pending_argument_reads.truncate(pending_start);
        Some(result)
    }

    /// Authenticate the constructor, field, wrapper, and hook before emitting any argument effects.
    fn checked_field_ingress_plan(
        &self,
        identity: &CanonicalSymbolId,
        planned: &[(usize, &ast::Spanned<ast::Expr>)],
        field_count: usize,
    ) -> Option<CheckedFieldIngress> {
        if planned.len() != field_count
            || !matches!(
                identity.kind,
                SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
            )
        {
            return None;
        }
        let mut slots = std::collections::BTreeSet::new();
        if planned
            .iter()
            .any(|(slot, _)| *slot >= field_count || !slots.insert(*slot))
        {
            return None;
        }
        let mut checked = planned.iter().filter_map(|(slot, expression)| {
            let coercion = self.type_info.validated_newtype_coercion(expression.span)?;
            coercion
                .steps
                .iter()
                .any(|step| step.ctor.is_some() || !step.constraints.is_empty())
                .then_some((*slot, coercion))
        });
        let (slot, coercion) = checked.next()?;
        if checked.next().is_some() {
            return None;
        }
        let ValidatedNewtypeCoercionMode::AggregateField { field_name } = &coercion.mode else {
            return None;
        };
        let [step] = coercion.steps.as_slice() else {
            return None;
        };
        if step.ctor.is_none() || !step.constraints.is_empty() {
            return None;
        }
        let constructor = self.local_nominal_declarations.values().find(|declaration| {
            &declaration.canonical == identity
                && declaration.type_parameters.is_empty()
                && declaration.fields.len() == field_count
                && declaration.fields.get(slot) == Some(field_name)
                && declaration.field_identities.len() == field_count
        })?;
        let wrapper = self
            .local_nominal_declarations
            .get(&step.newtype_name)
            .filter(|declaration| {
                declaration.canonical.kind == SemanticSourceTargetKind::Newtype
                    && declaration.fields == ["0"]
                    && declaration.type_parameters.is_empty()
            })?;
        if [constructor, wrapper].iter().any(|declaration| {
            incan_semantics_core::canonical_module_identity(&declaration.canonical).as_deref()
                != Some(self.module_identity)
        }) {
            return None;
        }
        let target = self.checked_type(&coercion.target_type);
        let mut field_type = constructor.field_types.get(slot)?.clone();
        if self.retain_nominal_carriers {
            super::nominal_carriers::retain_declared_type(&mut field_type, &constructor.named_type_identities);
        }
        if field_type != target
            || !matches!(&target, IncanType::Named(name)
            if name == &wrapper.name || bir::nominal_carrier_key(&wrapper.canonical).as_ref() == Some(name))
        {
            return None;
        }
        let hook = step.ctor_identity.as_ref().filter(|hook| {
            self.type_info
                .declarations
                .newtype_construction
                .get(&step.newtype_name)
                .is_some_and(|facts| {
                    facts.checked_constructor_identity.as_ref() == Some(*hook)
                        && self
                            .type_info
                            .declarations
                            .named_type_identities
                            .get(&step.newtype_name)
                            == Some(&wrapper.canonical)
                })
        })?;
        Some(CheckedFieldIngress {
            slot,
            owner: wrapper.name.clone(),
            hook: hook.clone(),
            target,
            constructor_label: constructor.canonical.declaration_name.clone(),
            field_label: field_name.clone(),
        })
    }

    /// Preserve written argument order, then extract the sole checked result and bind operands by declaration slot.
    #[allow(clippy::too_many_arguments)]
    fn lower_checked_field_arguments(
        &mut self,
        plan: CheckedFieldIngress,
        planned: &[(usize, &ast::Spanned<ast::Expr>)],
        field_count: usize,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::Operand>, bir::ArgumentBinding), String> {
        let mut values: Vec<Option<(bir::Operand, usize)>> = (0..field_count).map(|_| None).collect();
        let mut checked = None;

        // ---- Context: every supplied argument runs before a field error is raised ----
        for (written_position, (slot, expression)) in planned.iter().enumerate() {
            if *slot == plan.slot {
                let key = expression.span;
                self.coercions_in_progress.insert(key);
                let input = self.lower_expr_to_operand(expression, scope, out);
                self.coercions_in_progress.remove(&key);
                checked = Some((
                    self.checked_newtype_hook_call(
                        &plan.owner,
                        plan.hook.clone(),
                        input,
                        &plan.target,
                        scope,
                        span,
                        out,
                    ),
                    written_position,
                ));
            } else {
                let (operands, _) =
                    self.lower_planned_args(&[(*slot, expression)], field_count, &[], &[], scope, out)?;
                let operand = operands
                    .into_iter()
                    .next()
                    .ok_or("missing supplied aggregate field operand")?;
                self.retain_pending_operand_read(&operand);
                values[*slot] = Some((operand, written_position));
            }
        }
        let (checked, written_position) = checked.ok_or("missing checked aggregate field result")?;
        let value = self.push_call_temp(
            bir::Callee::Helper(bir::HelperOp::NewtypeFieldHookExtract),
            fixed_elements(vec![
                checked,
                bir::Operand::Constant(bir::Constant::Str(plan.constructor_label)),
                bir::Operand::Constant(bir::Constant::Str(plan.field_label)),
            ]),
            plan.target,
            scope,
            span,
            true,
            out,
        );
        values[plan.slot] = Some((value, written_position));

        // ---- Context: emitted operands use declaration slots without reordering effects ----
        let mut operands = Vec::with_capacity(field_count);
        let mut arguments = Vec::with_capacity(field_count);
        for (slot, entry) in values.into_iter().enumerate() {
            let (operand, written_position) = entry.ok_or("missing aggregate field binding")?;
            operands.push(operand);
            arguments.push(bir::BoundArgument { slot, written_position });
        }
        Ok((
            operands,
            bir::ArgumentBinding::Resolved {
                arguments,
                defaulted_slots: Vec::new(),
            },
        ))
    }
}
