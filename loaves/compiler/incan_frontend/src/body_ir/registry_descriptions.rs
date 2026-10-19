//! Retain checked method descriptions in the initialization of a sole source-local registry static.

use super::*;
use crate::typechecker::RegistryDescriptionRegistry;
use incan_semantics_core::SemanticRegistrySubjectKind;

/// Admit a bounded initialization profile with one static and no imported or explicit registry entries.
/// Literal description materialization cannot observe registration before that sole static is published.
fn supported_profile(info: &TypeCheckInfo) -> bool {
    info.registry.definitions.len() == 1
        && info.registry.imported_definitions.is_empty()
        && info.registry.explicit_entries.is_empty()
        && info.declarations.decorated_function_bindings_by_span.is_empty()
        && info.declarations.decorated_method_bindings.is_empty()
        && info
            .declarations
            .declaration_identities
            .values()
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Static)
            .count()
            == 1
        && !info.registry.descriptions.is_empty()
        && info.registry.descriptions.iter().all(|entry| {
            entry.subject_kind == SemanticRegistrySubjectKind::Method
                && matches!(&entry.registry, RegistryDescriptionRegistry::Local { binding }
                    if binding == &entry.registry_name && info.registry.definitions.contains_key(binding))
        })
}

/// Select the compiler-reserved method from the checked stdlib owner; native delegation verifies its full signature.
pub(super) fn helper_identity(info: &TypeCheckInfo) -> Option<CanonicalSymbolId> {
    if !supported_profile(info) {
        return None;
    }
    let owner = &info.registry.standard.as_ref()?.registry_class;
    let method = owner.methods.get("_describe_method")?;
    if method.receiver != Some(ast::Receiver::Mutable)
        || method.params.len() != 3
        || method.return_type != ResolvedType::Unit
        || !method.type_params.is_empty()
    {
        return None;
    }
    method.identity.clone()
}

/// Retain the declared registry carrier only for the exact checked SDK factory and imported SDK owner.
/// The registry definition already records the checked static annotation's arguments; initializer inference may stay
/// open.
pub(super) fn initialization_type(storage: &ast::StaticDecl, info: &TypeCheckInfo) -> Option<IncanType> {
    let definition = info.registry.definitions.get(&storage.name)?;
    let ast::Expr::MethodCall(receiver, _, _, _) = &storage.value.node else {
        return None;
    };
    let selected = info.resolved_identity(storage.value.span)?;
    let receiver_identity = info.resolved_identity(receiver.span)?;
    let standard = info.registry.standard.as_ref()?;
    let owner_identity = standard.registry.clone();
    let factory = standard.registry_class.methods.get("define")?;
    if factory.receiver.is_some()
        || factory.return_type != ResolvedType::SelfType
        || Some(selected) != factory.identity.as_ref()
        || receiver_identity != &owner_identity
    {
        return None;
    }
    Some(IncanType::Generic {
        base: owner_identity.declaration_name,
        args: vec![
            parameter_types::checked_semantic_type(&definition.key_type, info),
            parameter_types::checked_semantic_type(&definition.descriptor_type, info),
        ],
    })
}

/// Match a checked attachment to its exact method, decorator and materialization spans; spelling grants no admission.
fn attachment<'a>(
    owner: &str,
    method: &ast::MethodDecl,
    decorator: &ast::Spanned<ast::Decorator>,
    info: &'a TypeCheckInfo,
) -> Option<(&'a str, &'a str)> {
    let [
        ast::DecoratorArg::Positional(_),
        ast::DecoratorArg::Positional(key),
        ast::DecoratorArg::Positional(descriptor),
    ] = decorator.node.args.as_slice()
    else {
        return None;
    };
    info.registry
        .descriptions
        .iter()
        .find(|entry| {
            entry.declaration_name == format!("{owner}.{}", method.name)
                && entry.decorator_span == (decorator.span.start, decorator.span.end)
                && entry.key_span == (key.span.start, key.span.end)
                && entry.descriptor_span == (descriptor.span.start, descriptor.span.end)
                && literal_materialization(key, info)
                && literal_materialization(descriptor, info)
        })
        .map(|entry| (entry.registry_name.as_str(), entry.declaration_name.as_str()))
}

/// Permit only literals and complete nominal literal construction; no call to a source function can observe storage.
fn literal_materialization(expression: &ast::Spanned<ast::Expr>, info: &TypeCheckInfo) -> bool {
    if matches!(expression.node, ast::Expr::Literal(_)) {
        return true;
    }
    let Some(binding) = info.constructor_field_binding(expression.span) else {
        return false;
    };
    if !binding.defaulted_slots.is_empty() || binding.argument_slots.len() != binding.field_count {
        return false;
    }
    let Some(ResolvedType::Named(name)) = info.expr_type(expression.span) else {
        return false;
    };
    if info
        .declarations
        .newtype_construction
        .get(name)
        .is_some_and(|construction| construction.checked_constructor.is_some() || !construction.constraints.is_empty())
    {
        return false;
    }
    let arguments = match &expression.node {
        ast::Expr::Constructor(_, arguments) => arguments,
        ast::Expr::Call(callee, type_args, arguments)
            if type_args.is_empty()
                && info.resolved_identity(callee.span).is_some_and(|identity| {
                    matches!(
                        identity.kind,
                        SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Newtype
                    )
                }) =>
        {
            arguments
        }
        _ => return false,
    };
    arguments.iter().all(|argument| match argument {
        ast::CallArg::Positional(value) | ast::CallArg::Named(_, value) => literal_materialization(value, info),
        _ => false,
    })
}

/// Admit only fully checked method descriptions whose once-only effect the sole registry initializer retains.
pub(super) fn supports_method(method: &ast::MethodDecl, owner: &str, info: &TypeCheckInfo) -> bool {
    supported_profile(info)
        && helper_identity(info).is_some()
        && method.receiver == Some(ast::Receiver::Immutable)
        && method.type_params.is_empty()
        && !method.is_async()
        && !method.decorators.is_empty()
        && method
            .decorators
            .iter()
            .all(|decorator| attachment(owner, method, decorator, info).is_some())
}

/// Append checked registrations before the sole registry's computed initializer transfers its value into storage.
/// The helper mutates that same value, never a clone, and the existing static once guard executes these calls once.
pub(super) fn append_registrations(
    builder: &mut BodyBuilder<'_, '_>,
    program: &ast::Program,
    storage: &ast::StaticDecl,
    value: &bir::Operand,
    scope: bir::ScopeId,
    out: &mut Vec<bir::Statement>,
) {
    let Some(identity) = helper_identity(builder.type_info) else {
        return;
    };
    let bir::Operand::Place(receiver) = value else {
        return;
    };
    for declaration in &program.declarations {
        let (owner, methods) = match &declaration.node {
            ast::Declaration::Model(model) => (&model.name, &model.methods),
            ast::Declaration::Class(class) => (&class.name, &class.methods),
            _ => continue,
        };
        for method in methods {
            for decorator in &method.node.decorators {
                let Some((registry_name, declaration_name)) =
                    attachment(owner, &method.node, decorator, builder.type_info)
                else {
                    continue;
                };
                if registry_name != storage.name {
                    continue;
                }
                let qualified = format!("{}.{}", builder.module_identity.replace("::", "."), declaration_name);
                let [
                    ast::DecoratorArg::Positional(_),
                    ast::DecoratorArg::Positional(key),
                    ast::DecoratorArg::Positional(descriptor),
                ] = decorator.node.args.as_slice()
                else {
                    continue;
                };
                let key = builder.lower_expr_to_operand(key, scope, out);
                let descriptor = builder.lower_expr_to_operand(descriptor, scope, out);
                let arguments = vec![
                    bir::ArgumentElement::One(bir::Operand::place(
                        receiver.place.clone(),
                        bir::OwnershipFact::MutBorrow,
                        false,
                    )),
                    bir::ArgumentElement::One(key),
                    bir::ArgumentElement::One(descriptor),
                    bir::ArgumentElement::One(bir::Operand::Constant(bir::Constant::Str(qualified))),
                ];
                let target = bir::MethodTarget {
                    name: identity.declaration_name.clone(),
                    canonical: Some(identity.clone()),
                    intrinsic_carrier: None,
                    reflection: None,
                    sdk_trait: None,
                    native_trait_delegation: None,
                    type_args: Vec::new(),
                    binding: bir::ArgumentBinding::Resolved {
                        arguments: (0..3)
                            .map(|slot| bir::BoundArgument {
                                slot,
                                written_position: slot,
                            })
                            .collect(),
                        defaulted_slots: Vec::new(),
                    },
                };
                builder.push_call_temp(
                    bir::Callee::Method(target),
                    arguments,
                    IncanType::Primitive(IncanPrimitiveType::Unit),
                    scope,
                    hir_span(decorator.span),
                    false,
                    out,
                );
            }
        }
    }
}
