//! Source-local checked JSON selection projected only into Body IR reconstruction facts.

use super::{
    IncanPrimitiveType, IncanType, SemanticSourceTargetKind, TypeCheckInfo, ast, bodies, newtype_derives,
    parameter_types,
};
use incan_semantics_core::{
    CanonicalSymbolId, CompilerNodeId, RuntimeBuiltinType, SymbolNamespace, body_ir::NewtypeDeserialize,
    canonical_module_identity,
};

/// Authorize only the payload of an already checked source-local string instance of the selected serde owner.
///
/// This profile is independent of raw generic inference. It neither repairs shared field-reference facts nor
/// substitutes an open owner argument; the exact retained binder, layout, and hook selection must already agree.
pub(super) fn has_checked_string_payload(
    declaration: &incan_semantics_core::body_ir::NominalDeclaration,
    arguments: &[IncanType],
    info: &TypeCheckInfo,
    module_identity: &str,
) -> bool {
    let owner = &declaration.canonical;
    let [binder] = declaration.type_parameters.as_slice() else {
        return false;
    };
    let NewtypeDeserialize::Checked(hook) = &declaration.newtype_deserialize else {
        return false;
    };
    let Some(facts) = info.declarations.newtype_construction.get(&declaration.name) else {
        return false;
    };
    arguments == [IncanType::Primitive(IncanPrimitiveType::Str)]
        && owner.kind == SemanticSourceTargetKind::Newtype
        && owner.namespace == SymbolNamespace::OrdinaryLexical
        && owner.scope_discriminant.is_none()
        && owner.declaration_name == declaration.name
        && canonical_module_identity(owner).as_deref() == Some(module_identity)
        && info.declarations.named_type_identities.get(&declaration.name) == Some(owner)
        && declaration.direct_declaration_id
            == CompilerNodeId::declaration_span(
                module_identity,
                owner.declaration_span.start,
                owner.declaration_span.end,
            )
        && !binder.is_empty()
        && declaration.type_parameter_count == 1
        && declaration.fields == ["0"]
        && declaration.field_identities == [owner.clone()]
        && declaration.field_types == [IncanType::TypeVar(binder.clone())]
        && declaration.field_public == [true]
        && declaration.field_aliases.iter().all(Option::is_none)
        && !declaration.has_field_defaults
        && declaration.field_default_body.is_none()
        && declaration.derives.iter().any(|value| value == "serde::Deserialize")
        && facts.type_params == declaration.type_parameters
        && facts.constraints.is_empty()
        && parameter_types::checked_parameter_type(&facts.underlying, info, &facts.type_params)
            == IncanType::TypeVar(binder.clone())
        && facts.checked_constructor_identity.as_ref() == Some(hook)
        && facts.checked_constructor.as_deref() == Some(hook.declaration_name.as_str())
        && hook.kind == SemanticSourceTargetKind::Method
        && hook.namespace == SymbolNamespace::Member
        && hook.origin == owner.origin
        && canonical_module_identity(hook).as_deref() == Some(module_identity)
        && hook.declaration_span.start >= owner.declaration_span.start
        && hook.declaration_span.end <= owner.declaration_span.end
}

/// Retain a canonical local string hook or an unbounded identity-binder hook without changing legacy facts.
///
/// This is the single selection boundary for admission and nominal collection. Source parameter flags must agree
/// with the owned input contract even when normalized call bindings erase mutability. Consumer-side corruption
/// checks independently validate the retained hook body and closed contract before native serde is allowed.
pub(super) fn checked_deserialize_selection(
    source: &ast::NewtypeDecl,
    info: &TypeCheckInfo,
) -> Option<CanonicalSymbolId> {
    let facts = info.declarations.newtype_construction.get(&source.name)?;
    let selections = newtype_derives::checked_newtype_derives(source, info)?;
    if !selections
        .iter()
        .chain(facts.automatic_derives.iter())
        .any(|selection| selection == "serde::Deserialize")
    {
        return None;
    }
    let owner = info.declarations.named_type_identities.get(&source.name)?;
    let owner_module = canonical_module_identity(owner)?;
    if source.is_rusttype
        || owner.kind != SemanticSourceTargetKind::Newtype
        || owner.namespace != SymbolNamespace::OrdinaryLexical
        || owner.scope_discriminant.is_some()
        || owner.declaration_name != source.name
        || info
            .declarations
            .declaration_identities
            .get(&(owner.declaration_span.start, owner.declaration_span.end))
            != Some(owner)
        || facts.type_params
            != source
                .type_params
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect::<Vec<_>>()
        || !facts.constraints.is_empty()
    {
        return None;
    }
    let parameter = parameter_types::checked_parameter_type(&facts.underlying, info, &facts.type_params);
    let owner_type = symbolic_owner(source, &parameter)?;
    let selected_name = facts.checked_constructor.as_ref()?;
    let hook = facts.checked_constructor_identity.as_ref()?;
    let method = source.methods.iter().find(|method| {
        method.node.name == *selected_name
            && method.span.start == hook.declaration_span.start
            && method.span.end == hook.declaration_span.end
    })?;
    if hook.kind != SemanticSourceTargetKind::Method
        || hook.namespace != SymbolNamespace::Member
        || hook.declaration_name != *selected_name
        || canonical_module_identity(hook).as_deref() != Some(owner_module.as_str())
        || hook.origin != owner.origin
        || hook.declaration_span.start < owner.declaration_span.start
        || hook.declaration_span.end > owner.declaration_span.end
        || method.node.receiver.is_some()
        || method.node.receiver_binding.is_some()
        || method.node.trait_target.is_some()
        || !method.node.type_params.is_empty()
        || method.node.body.is_none()
    {
        return None;
    }
    if info
        .declarations
        .method_bindings_by_span
        .values()
        .filter(|binding| binding.identity.as_ref() == Some(hook))
        .count()
        != 1
    {
        return None;
    }
    let binding = info
        .declarations
        .method_bindings_by_span
        .get(&(method.span.start, method.span.end))?;
    let [input] = binding.params.as_slice() else {
        return None;
    };
    if binding.identity.as_ref() != Some(hook)
        || binding.is_async
        || input.kind != ast::ParamKind::Normal
        || input.is_mut
        || input.has_default
        || method.node.params.len() != 1
        || method.node.params[0].node.kind != ast::ParamKind::Normal
        || method.node.params[0].node.is_mut
        || method.node.params[0].node.default.is_some()
        || parameter_types::checked_parameter_type(&input.ty, info, &facts.type_params) != parameter
    {
        return None;
    }
    let result = bodies::close_self_type(
        parameter_types::checked_parameter_type(&binding.return_type, info, &facts.type_params),
        &owner_type,
    );
    let IncanType::Generic { base, args } = &result else {
        return None;
    };
    if base != "Result"
        || args.as_slice()
            != [
                owner_type,
                IncanType::NativeBuiltin(RuntimeBuiltinType::ValidationError),
            ]
    {
        return None;
    }
    Some(hook.clone())
}

/// Keep this profile to str or one exact unbounded declaration binder; no inference is performed here.
fn symbolic_owner(source: &ast::NewtypeDecl, underlying: &IncanType) -> Option<IncanType> {
    match source.type_params.as_slice() {
        [] if underlying == &IncanType::Primitive(IncanPrimitiveType::Str) => {
            Some(IncanType::Named(source.name.clone()))
        }
        [parameter] if parameter.bounds.is_empty() && underlying == &IncanType::TypeVar(parameter.name.clone()) => {
            Some(IncanType::Generic {
                base: source.name.clone(),
                args: vec![underlying.clone()],
            })
        }
        _ => None,
    }
}
