//! Retain checked, effect-free presets for source-local trait partial calls before source facts leave the frontend.

use super::{
    BodyBuilder, CanonicalSymbolId, TypeCheckInfo, ast, bir, has_direct_replacement_literal_trait_partials,
    method_partial_target, primitives, semantic_type_from_resolved,
};

/// A checked local trait target and its partial's literal presets, indexed by ordinary parameter slot.
pub(super) struct LocalTraitPartial {
    target: CanonicalSymbolId,
    name: String,
    presets: Vec<(usize, bir::Constant)>,
}

/// Collect only admitted source-local trait partials with checked method identities and literal parameter values.
pub(super) fn collect(program: &ast::Program, type_info: &TypeCheckInfo) -> Vec<LocalTraitPartial> {
    let mut partials = Vec::new();
    for declaration in &program.declarations {
        let ast::Declaration::Trait(source) = &declaration.node else {
            continue;
        };
        if !has_direct_replacement_literal_trait_partials(source) {
            continue;
        }
        for partial in &source.method_partials {
            let Some(method) = method_partial_target(&source.methods, &source.method_aliases, &partial.node) else {
                continue;
            };
            let Some(target) = type_info
                .declarations
                .method_bindings_by_span
                .get(&(method.span.start, method.span.end))
                .and_then(|binding| binding.identity.clone())
            else {
                continue;
            };
            let presets = partial
                .node
                .args
                .iter()
                .map(|preset| {
                    let slot = method
                        .node
                        .params
                        .iter()
                        .position(|parameter| parameter.node.name == preset.name)?;
                    let ast::Expr::Literal(value) = &preset.value.node else {
                        return None;
                    };
                    let ty = semantic_type_from_resolved(type_info.expr_type(preset.value.span)?);
                    Some((slot, primitives::lower_checked_literal(value, &ty)))
                })
                .collect::<Option<Vec<_>>>();
            if let Some(presets) = presets {
                partials.push(LocalTraitPartial {
                    target,
                    name: partial.node.name.clone(),
                    presets,
                });
            }
        }
    }
    partials
}

impl BodyBuilder<'_, '_> {
    /// Supply an omitted preset only for the exact checked trait target and partial spelling; explicit overrides win.
    /// Literals have no lexical dependencies or observable evaluation order, so the resulting call carries all
    /// executable facts.
    pub(super) fn retain_trait_partial_presets(
        &self,
        name: &str,
        canonical: Option<&CanonicalSymbolId>,
        operands: &mut Vec<bir::ArgumentElement>,
        binding: &mut bir::ArgumentBinding,
    ) {
        let Some(partial) = self
            .local_trait_partials
            .iter()
            .find(|partial| partial.name == name && Some(&partial.target) == canonical)
        else {
            return;
        };
        let bir::ArgumentBinding::Resolved {
            arguments,
            defaulted_slots,
        } = binding
        else {
            return;
        };
        for (slot, value) in &partial.presets {
            if !defaulted_slots.contains(slot) {
                continue;
            }
            let written_position = arguments.len();
            arguments.push(bir::BoundArgument {
                slot: *slot,
                written_position,
            });
            defaulted_slots.retain(|default| default != slot);
            operands.push(bir::ArgumentElement::One(bir::Operand::Constant(value.clone())));
        }
    }
}
