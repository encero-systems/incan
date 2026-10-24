//! Checked registration effects and explicit subject materialization for the direct route.

use super::*;
use crate::typechecker::RegistryDescriptionRegistry;
use crate::typechecker::RegistryExplicitEntryInfo;
use incan_semantics_core::SemanticRegistrySubjectKind;

/// Select the checked SDK function-description helper; spelling alone never selects a runtime call.
fn function_helper(info: &TypeCheckInfo) -> Option<CanonicalSymbolId> {
    if info
        .registry
        .descriptions
        .iter()
        .all(|entry| entry.subject_kind != SemanticRegistrySubjectKind::Function)
    {
        return None;
    }
    let owner = &info.registry.standard.as_ref()?.registry_class;
    let helper = owner.methods.get("_describe_function")?;
    if helper.receiver != Some(ast::Receiver::Mutable)
        || helper.params.len() != 3
        || helper.return_type != ResolvedType::Unit
        || !helper.type_params.is_empty()
    {
        return None;
    }
    helper.identity.clone()
}

/// Include only checked registry materializers in the ordinary signature-validated SDK call inventory.
pub(super) fn helper_identities(info: &TypeCheckInfo) -> Vec<CanonicalSymbolId> {
    let mut identities = registry_descriptions::helper_identity(info)
        .into_iter()
        .collect::<Vec<_>>();
    identities.extend(function_helper(info));
    identities.extend(
        info.registry
            .explicit_entries
            .iter()
            .map(|entry| entry.checked_constructor_identity.clone()),
    );
    identities
}

/// Match every physical function description against its complete checked attachment and declaration spans.
/// The checker selects `Local { binding }` directly for this declarative argument; it is not an expression read and
/// therefore has no reference-span identity. This frontend projection resolves that retained local binding once.
fn function_attachment<'a>(
    declaration: &ast::Spanned<ast::Declaration>,
    decorator: &ast::Spanned<ast::Decorator>,
    info: &'a TypeCheckInfo,
) -> Option<(&'a str, &'a str)> {
    let ast::Declaration::Function(function) = &declaration.node else {
        return None;
    };
    let [
        ast::DecoratorArg::Positional(registry),
        ast::DecoratorArg::Positional(key),
        ast::DecoratorArg::Positional(descriptor),
    ] = decorator.node.args.as_slice()
    else {
        return None;
    };
    info.registry.descriptions.iter().find(|entry| {
        entry.subject_kind == SemanticRegistrySubjectKind::Function
            && entry.declaration_name == function.name
            && entry.declaration_span == declaration.span
            && entry.decorator_span == decorator.span
            && entry.key_span == key.span
            && entry.descriptor_span == descriptor.span
            && matches!(&entry.registry, RegistryDescriptionRegistry::Local { binding } if binding == &entry.registry_name)
            && matches!(&registry.node, ast::Expr::Ident(name) if name == &entry.registry_name)
            && info.declarations.declaration_identities.values().any(|identity| {
                identity.kind == SemanticSourceTargetKind::Static && identity.declaration_name == entry.registry_name
            })
    }).map(|entry| (entry.registry_name.as_str(), entry.declaration_name.as_str()))
}

/// Refuse every executable body when a checked description has no exact retained effect, including imported targets.
/// This keeps partial or stale facts from turning into a quietly empty registry when another type becomes supported.
pub(super) fn refuse_unretained_functions(program: &ast::Program, info: &TypeCheckInfo, bodies: &mut [bir::Body]) {
    for entry in &info.registry.descriptions {
        if entry.subject_kind != SemanticRegistrySubjectKind::Function {
            continue;
        }
        let retained = function_helper(info).is_some()
            && program.declarations.iter().any(|declaration| {
                let ast::Declaration::Function(function) = &declaration.node else {
                    return false;
                };
                function.decorators.iter().any(|decorator| {
                    function_attachment(declaration, decorator, info)
                        == Some((entry.registry_name.as_str(), entry.declaration_name.as_str()))
                        && program.declarations.iter().any(|declaration| match &declaration.node {
                            ast::Declaration::Static(storage) if storage.name == entry.registry_name => {
                                registry_descriptions::initialization_type(storage, info).is_some()
                            }
                            _ => false,
                        })
                })
            });
        if retained {
            continue;
        }
        for body in bodies {
            body.block.stmts.insert(
                0,
                bir::Statement {
                    kind: bir::StatementKind::Unsupported {
                        description: "function registry description without exact local attachment facts".to_owned(),
                    },
                    span: entry.decorator_span,
                },
            );
        }
        return;
    }
}

/// Build one compiler-owned once-only effect frame after the module's original static and decorator frames.
/// This uses the existing computed-storage phase: its unit-valued cell is appended last and never source-accessible.
pub(super) fn collect(
    program: &ast::Program,
    facts: &BodyIrLoweringFacts<'_, '_>,
    statics: &mut Vec<bir::StaticDeclaration>,
) {
    let Some(helper) = function_helper(facts.type_info) else {
        return;
    };
    let mut builder = BodyBuilder::new(facts, IncanType::Primitive(IncanPrimitiveType::Unit));
    let Some(last) = program.declarations.last() else {
        return;
    };
    // The effect has its own declaration identity but needs a real source character for native diagnostics.
    let span = ast::Span::new(last.span.end.saturating_sub(1), last.span.end).with_expansion(last.span.expansion);
    let scope = builder.new_scope(None, hir_span(span));
    let mut stmts = Vec::new();
    for declaration in &program.declarations {
        let ast::Declaration::Function(function) = &declaration.node else {
            continue;
        };
        for decorator in &function.decorators {
            let Some((registry, name)) = function_attachment(declaration, decorator, facts.type_info) else {
                continue;
            };
            append_description(
                &mut builder,
                program,
                registry,
                name,
                decorator,
                &helper,
                scope,
                &mut stmts,
            );
        }
    }
    if stmts.is_empty() {
        return;
    }
    let Some(mut canonical) = statics.first().map(|storage| storage.canonical.clone()) else {
        return;
    };
    canonical.declaration_name = "__incan_checked_registry_effects".to_owned();
    canonical.declaration_span = hir_span(span);
    if statics
        .iter()
        .any(|storage| storage.canonical.declaration_name == canonical.declaration_name)
    {
        builder.push_unsupported_stmt(
            "reserved registry effect storage collision".to_owned(),
            hir_span(span),
            &mut stmts,
        );
    }
    stmts.push(bir::Statement {
        kind: bir::StatementKind::Return { value: None },
        span: hir_span(span),
    });
    let body = bodies::finish_static_initializer_body(
        builder,
        &canonical,
        &IncanType::Primitive(IncanPrimitiveType::Unit),
        scope,
        hir_span(span),
        stmts,
    );
    statics.push(bir::StaticDeclaration {
        canonical,
        ty: body.return_type.clone(),
        initial: bir::StaticInitializer::Computed(Box::new(body)),
    });
}

/// Evaluate the checked key and descriptor before acquiring the actual registry's mutable guard.
/// Capture a scalar or text key before descriptor evaluation can change the key's global storage.
#[expect(
    clippy::too_many_arguments,
    reason = "The checked source, selected registry identity and mutable statement sink have independent lifetimes."
)]
fn append_description(
    builder: &mut BodyBuilder<'_, '_>,
    program: &ast::Program,
    registry: &str,
    name: &str,
    decorator: &ast::Spanned<ast::Decorator>,
    helper: &CanonicalSymbolId,
    scope: bir::ScopeId,
    out: &mut Vec<bir::Statement>,
) {
    let Some(storage) = program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            ast::Declaration::Static(storage) if storage.name == registry => Some((storage, declaration.span)),
            _ => None,
        })
    else {
        return;
    };
    let Some(identity) = builder.type_info.declarations.declaration_identities.get(&storage.1) else {
        return;
    };
    let Some(ty) = registry_descriptions::initialization_type(storage.0, builder.type_info) else {
        return;
    };
    let Some(global) = builder.global_place(identity.clone(), ty) else {
        return;
    };
    let [
        _,
        ast::DecoratorArg::Positional(key),
        ast::DecoratorArg::Positional(descriptor),
    ] = decorator.node.args.as_slice()
    else {
        return;
    };
    let key_value = builder.lower_expr_to_operand(key, scope, out);
    let key = builder.retain_evaluated_value_read(key, key_value, scope, out);
    let descriptor = builder.lower_expr_to_operand(descriptor, scope, out);
    let qualified = format!("{}.{}", builder.module_identity.replace("::", "."), name);
    let args = vec![
        bir::ArgumentElement::One(bir::Operand::place(
            bir::Place::from_global(global),
            bir::OwnershipFact::MutBorrow,
            false,
        )),
        bir::ArgumentElement::One(key),
        bir::ArgumentElement::One(descriptor),
        bir::ArgumentElement::One(bir::Operand::Constant(bir::Constant::Str(qualified))),
    ];
    builder.push_call_temp(
        bir::Callee::Method(bir::MethodTarget {
            name: helper.declaration_name.clone(),
            canonical: Some(helper.clone()),
            intrinsic_carrier: None,
            reflection: None,
            loaf_trait: None,
            trait_arguments: None,
            native_trait_delegation: None,
            type_args: Vec::new(),
            binding: positional_binding(3),
        }),
        args,
        IncanType::Primitive(IncanPrimitiveType::Unit),
        scope,
        hir_span(decorator.span),
        false,
        out,
    );
}

/// Retain a fixed checked argument order for compiler-selected registry helper calls.
fn positional_binding(count: usize) -> bir::ArgumentBinding {
    bir::ArgumentBinding::Resolved {
        arguments: (0..count)
            .map(|slot| bir::BoundArgument {
                slot,
                written_position: slot,
            })
            .collect(),
        defaulted_slots: Vec::new(),
    }
}

/// Select the exact checked explicit entry by all materialization spans, canonical method and local registry binding.
fn entry_for_call<'a>(
    expression: &ast::Spanned<ast::Expr>,
    info: &'a TypeCheckInfo,
) -> Option<&'a RegistryExplicitEntryInfo> {
    let ast::Expr::MethodCall(receiver, _, types, args) = &expression.node else {
        return None;
    };
    if !types.is_empty() || args.len() != 3 {
        return None;
    }
    info.registry.explicit_entries.iter().find(|entry| {
        info.resolved_identity(expression.span) == Some(&entry.entry_method_identity)
            && matches!(&receiver.node, ast::Expr::Ident(name) if name == &entry.registry_name)
            && args.iter().all(|argument| match argument {
                ast::CallArg::Named(name, value) => {
                    let span = value.span;
                    match name.node.as_str() {
                        "key" => span == entry.key_span,
                        "subject" => span == entry.subject_span,
                        "descriptor" => span == entry.descriptor_span,
                        _ => false,
                    }
                }
                _ => false,
            })
    })
}

/// Retain the explicit entry's checked SDK slots and real mutable static receiver, preserving written argument
/// evaluation order.
pub(super) fn entry(
    builder: &mut BodyBuilder<'_, '_>,
    expression: &ast::Spanned<ast::Expr>,
    scope: bir::ScopeId,
    out: &mut Vec<bir::Statement>,
) -> Option<bir::Operand> {
    let info = builder.type_info;
    let entry = entry_for_call(expression, info)?;
    let ast::Expr::MethodCall(_, _, _, args) = &expression.node else {
        return None;
    };
    let definition = info.registry.definitions.get(&entry.registry_name)?;
    let standard = info.registry.standard.as_ref()?;
    let owner_identity = standard.registry.clone();
    let method = standard
        .registry_class
        .methods
        .get(&entry.entry_method_identity.declaration_name)?;
    if method.receiver != Some(ast::Receiver::Mutable)
        || method.params.len() != 3
        || !method.type_params.is_empty()
        || method.identity.as_ref() != Some(&entry.entry_method_identity)
    {
        return None;
    }
    if method
        .params
        .iter()
        .map(|param| param.name.as_deref())
        .ne([Some("key"), Some("subject"), Some("descriptor")])
    {
        return None;
    }
    let key = parameter_types::checked_semantic_type(&definition.key_type, info);
    let descriptor = parameter_types::checked_semantic_type(&definition.descriptor_type, info);
    let subject = builder.resolve_ty(entry.subject_span);
    let ty = IncanType::Generic {
        base: owner_identity.declaration_name,
        args: vec![key.clone(), descriptor.clone()],
    };
    let identity = info.declarations.declaration_identities.values().find(|identity| {
        identity.kind == SemanticSourceTargetKind::Static && identity.declaration_name == entry.registry_name
    })?;
    let global = builder.global_place(identity.clone(), ty)?;
    let slots = method
        .params
        .iter()
        .zip([key, subject, descriptor])
        .map(|(parameter, ty)| {
            let mut slot = args::DeclaredSlot::from_checked_param(parameter);
            slot.ty = ty;
            slot
        })
        .collect();
    let (ordinary, binding) = match builder.bind_declared_args("checked registry entry", Some(slots), args, scope, out)
    {
        Ok(bound) => bound,
        Err(reason) => return Some(builder.unsupported_operand(reason, scope, hir_span(expression.span), out)),
    };
    let mut arguments = vec![bir::ArgumentElement::One(bir::Operand::place(
        bir::Place::from_global(global),
        bir::OwnershipFact::MutBorrow,
        false,
    ))];
    arguments.extend(ordinary);
    Some(builder.push_call_temp(
        bir::Callee::Method(bir::MethodTarget {
            name: entry.entry_method_identity.declaration_name.clone(),
            canonical: Some(entry.entry_method_identity.clone()),
            intrinsic_carrier: None,
            reflection: None,
            loaf_trait: None,
            trait_arguments: None,
            native_trait_delegation: None,
            type_args: Vec::new(),
            binding,
        }),
        arguments,
        builder.resolve_ty(expression.span),
        scope,
        hir_span(expression.span),
        false,
        out,
    ))
}

/// Close an explicit entry's storage from its checked registry contract, never from unresolved factory inference.
pub(super) fn entry_type(storage: &ast::StaticDecl, span: ast::Span, info: &TypeCheckInfo) -> Option<IncanType> {
    let entry = info
        .registry
        .explicit_entries
        .iter()
        .find(|entry| entry.entry_name == storage.name && entry.declaration_span == span)?;
    if info.resolved_identity(storage.value.span) != Some(&entry.entry_method_identity) {
        return None;
    }
    let definition = info.registry.definitions.get(&entry.registry_name)?;
    let owner = info.registry.standard.as_ref()?.entry.clone();
    Some(IncanType::Generic {
        base: owner.declaration_name,
        args: vec![
            parameter_types::checked_semantic_type(&definition.key_type, info),
            parameter_types::checked_semantic_type(&definition.descriptor_type, info),
        ],
    })
}

/// Replace only an exact checked declaration-context subject constructor with its approved materializer.
pub(super) fn subject(
    builder: &mut BodyBuilder<'_, '_>,
    expression: &ast::Spanned<ast::Expr>,
    scope: bir::ScopeId,
    out: &mut Vec<bir::Statement>,
) -> Option<bir::Operand> {
    let entry = builder
        .type_info
        .registry
        .explicit_entries
        .iter()
        .find(|entry| entry.subject_span == expression.span)?;
    if builder.type_info.resolved_identity(expression.span) != Some(&entry.subject_constructor_identity) {
        return Some(builder.unsupported_operand(
            "checked registry subject constructor identity differs".to_owned(),
            scope,
            hir_span(expression.span),
            out,
        ));
    }
    let ast::Expr::MethodCall(_, _, types, args) = &expression.node else {
        return None;
    };
    if !types.is_empty() || !args.is_empty() {
        return None;
    }
    let identity = entry.checked_constructor_identity.clone();
    let qualified = match entry.subject_kind {
        SemanticRegistrySubjectKind::CompilationUnit => builder.module_identity.replace("::", "."),
        SemanticRegistrySubjectKind::Package => match builder.registry_package {
            Some(package) => package.to_owned(),
            None => {
                return Some(builder.unsupported_operand(
                    "_checked_package requires the compilation package identity".to_owned(),
                    scope,
                    hir_span(expression.span),
                    out,
                ));
            }
        },
        _ => return None,
    };
    let ty = builder.resolve_ty(expression.span);
    Some(builder.push_call_temp(
        bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
            name: identity.declaration_name.clone(),
            direct_call_id: None,
            builtin: None,
            type_args: Vec::new(),
            binding: positional_binding(1),
            canonical: Some(identity),
            receiver_type: Some(ty.clone()),
        })),
        vec![bir::ArgumentElement::One(bir::Operand::Constant(bir::Constant::Str(
            qualified,
        )))],
        ty,
        scope,
        hir_span(expression.span),
        false,
        out,
    ))
}
