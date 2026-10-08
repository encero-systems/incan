//! Declaration-owned defaults for checked model method partials.

use super::{BodyIrLoweringFacts, IncanType, SemanticSourceTargetKind, ast, bir, hir_span, lower_method_body};

/// Attach only source-local, uniquely checked target frames to already admitted model owners.
/// Method aliases and partials retain the target's semantic identity; the binding name and owner select preset facts.
pub(super) fn attach_model_method_partials(
    program: &ast::Program,
    declarations: &mut [bir::NominalDeclaration],
    facts: &BodyIrLoweringFacts<'_, '_>,
) {
    for declaration in &program.declarations {
        let ast::Declaration::Model(model) = &declaration.node else {
            continue;
        };
        let Some(owner) = declarations
            .iter_mut()
            .find(|owner| owner.canonical.declaration_span == hir_span(declaration.span) && owner.name == model.name)
        else {
            continue;
        };
        for partial in &model.method_partials {
            if let Some(frame) = partial_frame(model, partial, facts) {
                owner.method_partial_defaults.push(frame);
            }
        }
    }
}

/// Preserve the original receiver and canonical parameter types while replacing preset default expressions.
/// Unsupported or ambiguous targets retain no executable frame, so an omitted preset still fails closed at use.
fn partial_frame(
    owner: &ast::ModelDecl,
    partial: &ast::Spanned<ast::MethodPartialDecl>,
    facts: &BodyIrLoweringFacts<'_, '_>,
) -> Option<bir::MethodPartialDefaults> {
    let mut methods = owner
        .methods
        .iter()
        .filter(|method| method.node.name == partial.node.target);
    let method = methods.next()?;
    if methods.next().is_some() {
        return None;
    }
    let binding = facts
        .type_info
        .declarations
        .method_bindings_by_span
        .get(&(method.span.start, method.span.end))?;
    let target = binding.identity.as_ref()?;
    if target.kind != SemanticSourceTargetKind::Method
        || target.declaration_name != method.node.name
        || target.declaration_span != hir_span(method.span)
        || incan_semantics_core::canonical_module_identity(target).as_deref() != Some(facts.module_identity)
        || !owner.type_params.is_empty()
        || !method.node.type_params.is_empty()
        || method.node.receiver.is_none()
    {
        return None;
    }
    let mut defaults = method.node.clone();
    defaults.body = Some(Vec::new());
    for preset in &partial.node.args {
        let parameter = defaults
            .params
            .iter_mut()
            .find(|parameter| parameter.node.name == preset.name)?;
        parameter.node.default = Some(preset.value.clone());
    }
    let frame = lower_method_body(
        &defaults,
        method.span,
        &owner.name,
        &IncanType::Named(owner.name.clone()),
        facts,
    )?;
    Some(bir::MethodPartialDefaults {
        name: partial.node.name.clone(),
        span: hir_span(partial.span),
        target: target.clone(),
        frame: Box::new(frame),
    })
}
