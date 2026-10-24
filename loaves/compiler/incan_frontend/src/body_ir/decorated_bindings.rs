//! RFC 036 originals with checked module, per-instantiation and per-invocation replacement frames, used exclusively by
//! Body IR.

use super::args::fixed_elements;
use super::*;
use crate::typechecker::MethodDecoratorReceiverRole;
use incan_lang::lang::decorators;

/// Admit a checked, nongeneric nominal method chain whose receiver and ordinary inputs remain unchanged.
pub(super) fn supports_method(method: &ast::MethodDecl, owner: &str, type_info: &TypeCheckInfo) -> bool {
    let Some(binding) = type_info
        .declarations
        .decorated_method_bindings
        .get(&(owner.to_string(), method.name.clone()))
    else {
        return false;
    };
    let (ResolvedType::Function(original, _), ResolvedType::Function(replacement, _)) =
        (&binding.original_unbound_ty, &binding.unbound_ty)
    else {
        return false;
    };
    method.receiver.is_some()
        && method.type_params.is_empty()
        && !method.is_async()
        && original.len() == replacement.len()
        && original
            .iter()
            .zip(replacement)
            .all(|(left, right)| left.ty == right.ty && left.is_mut == right.is_mut && left.kind == right.kind)
        && method.decorators.iter().all(
            |decorator| match decorators::from_segments(&decorator.node.path.segments) {
                Some(decorators::DecoratorId::RustExtern) => false,
                Some(_) => true,
                None => decorator.node.path.segments.len() == 1,
            },
        )
}

/// Rewrite only shared receiver positions that the existing checker planned for a complete method chain.
/// This operates on Body IR types only; the checker and legacy signature tables are never modified.
pub(super) fn planned_type(ty: IncanType, type_info: &TypeCheckInfo) -> IncanType {
    let mut shapes = Vec::new();
    for (span, slot) in &type_info.declarations.method_decorator_receiver_slots {
        if slot.mutable {
            continue;
        }
        let Some(binding) = type_info.declarations.function_bindings_by_span.get(span) else {
            continue;
        };
        let accepted = binding
            .params
            .first()
            .map(|param| parameter_types::checked_semantic_type(&param.ty, type_info));
        let result = parameter_types::checked_semantic_type(&binding.return_type, type_info);
        let replacements = match slot.role {
            MethodDecoratorReceiverRole::Decorator => vec![accepted, Some(result)],
            MethodDecoratorReceiverRole::Factory => {
                if let IncanType::Function { params, return_type } = &result {
                    vec![params.first().map(|param| param.ty.clone()), Some(*return_type.clone())]
                } else {
                    Vec::new()
                }
            }
            MethodDecoratorReceiverRole::Replacement => {
                let original = parameter_types::checked_semantic_type(
                    &ResolvedType::Function(binding.params.clone(), Box::new(binding.return_type.clone())),
                    type_info,
                );
                vec![Some(original)]
            }
        };
        shapes.extend(replacements.into_iter().flatten());
    }
    apply_shared_shapes(ty, &shapes)
}

/// Rewrite a planned callable shape once, including the callable shapes nested in factory results.
fn apply_shared_shapes(mut ty: IncanType, shapes: &[IncanType]) -> IncanType {
    if shapes.contains(&ty) {
        return shared_callable_receiver(ty);
    }
    if let IncanType::Function { params, return_type } = &mut ty {
        for parameter in params {
            parameter.ty = apply_shared_shapes(parameter.ty.clone(), shapes);
        }
        **return_type = apply_shared_shapes(*return_type.clone(), shapes);
    }
    ty
}

/// Replace the first input of a proven shared callable shape with its passing reference.
fn shared_callable_receiver(mut shape: IncanType) -> IncanType {
    if let IncanType::Function { params, .. } = &mut shape
        && let Some(receiver) = params.first_mut()
    {
        receiver.ty = IncanType::Ref(Box::new(receiver.ty.clone()));
    }
    shape
}

/// Admit checked source callables without conflating closed cached originals with signatures using declaration binders.
/// Open signatures preserve their return type and use the invocation wrapper. The measured closed generic profile
/// remains a synchronous zero-argument string callable with its own initializer template. Factories retain their
/// validated closed applications; extern delegations cannot bypass the visible replacement.
/// A nongeneric replacement may change required ordinary input types and passing modes; the wrapper retains the
/// original as a separate callable and receives the replacement's checked signature.
pub(super) fn supports_function(
    function: &ast::FunctionDecl,
    span: ast::Span,
    facts: &BodyIrLoweringFacts<'_, '_>,
) -> bool {
    let Some(binding) = facts
        .type_info
        .declarations
        .decorated_function_bindings_by_span
        .get(&(span.start, span.end))
    else {
        return false;
    };
    let (ResolvedType::Function(original, original_return), ResolvedType::Function(replacement, replacement_return)) =
        (&binding.original_ty, &binding.ty)
    else {
        return false;
    };
    let type_parameters = function
        .type_params
        .iter()
        .map(|parameter| parameter.name.clone())
        .collect::<Vec<_>>();
    let signature = parameter_types::checked_parameter_type(&binding.ty, facts.type_info, &type_parameters);
    let profile = type_parameters.is_empty()
        || (original_return == replacement_return
            && (bir::decorated_signature_uses_parameters(&signature, &type_parameters)
                || matches!(&signature, IncanType::Function { params, return_type }
                    if params.is_empty()
                        && **return_type == IncanType::Primitive(IncanPrimitiveType::Str))));
    let preserves_inputs = original
        .iter()
        .zip(replacement)
        .all(|(left, right)| left.ty == right.ty && left.is_mut == right.is_mut && left.kind == right.kind);
    let changes_closed_inputs = type_parameters.is_empty()
        && function.params.iter().all(|parameter| parameter.node.default.is_none())
        && original
            .iter()
            .zip(replacement)
            .all(|(left, right)| left.kind == ast::ParamKind::Normal && right.kind == ast::ParamKind::Normal);
    profile
        && original.len() == replacement.len()
        && (preserves_inputs || changes_closed_inputs)
        && !function.is_async()
        && function.decorators.iter().all(
            |decorator| match decorators::from_segments(&decorator.node.path.segments) {
                Some(decorators::DecoratorId::RustExtern) => false,
                Some(_) => true,
                None => {
                    decorator.node.path.segments.len() == 1
                        || facts
                            .type_info
                            .declarations
                            .body_ir_decorator_static_receivers
                            .contains_key(&(decorator.span.start, decorator.span.end))
                }
            },
        )
}

/// Retain originals and replacement initializers in source order, replacing their visible bodies with wrappers.
/// Closed generic signatures retain initializer templates outside module statics; checked open signatures use
/// invocation frames instead.
pub(super) fn collect(
    program: &ast::Program,
    facts: &BodyIrLoweringFacts<'_, '_>,
    bodies: &mut [bir::Body],
    statics: &mut Vec<bir::StaticDeclaration>,
) -> Vec<bir::DecoratedBinding> {
    let mut retained = Vec::new();
    for declaration in &program.declarations {
        let owner = match &declaration.node {
            ast::Declaration::Class(class) if class.type_params.is_empty() => Some((&class.name, &class.methods)),
            ast::Declaration::Model(model) if model.type_params.is_empty() => Some((&model.name, &model.methods)),
            _ => None,
        };
        if let Some((owner, methods)) = owner {
            for method in methods {
                if !supports_method(&method.node, owner, facts.type_info) {
                    continue;
                }
                let Some(binding) = facts
                    .type_info
                    .declarations
                    .decorated_method_bindings
                    .get(&(owner.clone(), method.node.name.clone()))
                else {
                    continue;
                };
                let Some(body) = bodies.iter_mut().find(|body| body.span == hir_span(method.span)) else {
                    continue;
                };
                retain_binding(
                    &method.node.decorators,
                    method.span,
                    facts,
                    body,
                    parameter_types::checked_semantic_type(&binding.original_unbound_ty, facts.type_info),
                    parameter_types::checked_semantic_type(&binding.unbound_ty, facts.type_info),
                    &mut retained,
                    statics,
                );
            }
        }
        let ast::Declaration::Function(function) = &declaration.node else {
            continue;
        };
        let Some(binding) = facts
            .type_info
            .declarations
            .decorated_function_bindings_by_span
            .get(&(declaration.span.start, declaration.span.end))
        else {
            continue;
        };
        if !supports_function(function, declaration.span, facts) {
            continue;
        }
        let Some(body) = bodies.iter_mut().find(|body| body.span == hir_span(declaration.span)) else {
            continue;
        };
        let ty = parameter_types::checked_parameter_type(&binding.ty, facts.type_info, &body.type_parameters);
        let original_ty =
            parameter_types::checked_parameter_type(&binding.original_ty, facts.type_info, &body.type_parameters);
        retain_binding(
            &function.decorators,
            declaration.span,
            facts,
            body,
            original_ty,
            ty,
            &mut retained,
            statics,
        );
    }
    retained
}

/// Attach a replacement to the original declaration's canonical identity with its checked initialization scope.
/// Closed generic originals retain the exact initializer template; open signatures use the checked invocation wrapper,
/// and only nongeneric replacements become module statics.
#[expect(
    clippy::too_many_arguments,
    reason = "Keep original and replacement signatures and their distinct binding and static outputs explicit at this boundary."
)]
fn retain_binding(
    decorators: &[ast::Spanned<ast::Decorator>],
    span: ast::Span,
    facts: &BodyIrLoweringFacts<'_, '_>,
    body: &mut bir::Body,
    original_ty: IncanType,
    ty: IncanType,
    retained: &mut Vec<bir::DecoratedBinding>,
    statics: &mut Vec<bir::StaticDeclaration>,
) {
    let Some(canonical) = body.canonical.clone() else {
        return;
    };
    if !body.type_parameters.is_empty() && bir::decorated_signature_uses_parameters(&ty, &body.type_parameters) {
        if let Some(wrapper) = invocation_wrapper(decorators, span, facts, body, &original_ty, &ty) {
            let original_body = std::mem::replace(body, wrapper);
            retained.push(bir::DecoratedBinding {
                original_body,
                initializer: None,
                initialization: bir::DecoratedInitialization::PerInvocation,
            });
        }
        return;
    }
    let initializer = initializer(decorators, span, facts, body, &original_ty, &ty, false);
    let replacement = bir::StaticDeclaration {
        canonical,
        ty,
        initial: bir::StaticInitializer::Computed(Box::new(initializer)),
    };
    let original_body = body.clone();
    if let Some(wrapper) = wrapper(&original_body, &replacement) {
        *body = wrapper;
        let initializer = if original_body.type_parameters.is_empty() {
            None
        } else if let bir::StaticInitializer::Computed(initializer) = &replacement.initial {
            Some(initializer.clone())
        } else {
            return;
        };
        let module_static = initializer.is_none();
        retained.push(bir::DecoratedBinding {
            original_body,
            initializer,
            initialization: if module_static {
                bir::DecoratedInitialization::ModuleStorage
            } else {
                bir::DecoratedInitialization::PerInstantiation
            },
        });
        if module_static {
            statics.push(replacement);
        }
    }
}

/// Compute a decorator chain bottom-up from an explicitly identified original callable, without rechecking source.
fn initializer(
    decorators: &[ast::Spanned<ast::Decorator>],
    span: ast::Span,
    facts: &BodyIrLoweringFacts<'_, '_>,
    original: &bir::Body,
    original_ty: &IncanType,
    replacement_ty: &IncanType,
    invocation: bool,
) -> bir::Body {
    let mut builder = BodyBuilder::new(facts, replacement_ty.clone());
    builder.type_parameters = original.type_parameters.clone();
    builder.shared_method_chain = original
        .locals
        .first()
        .is_some_and(|local| matches!(local.origin, bir::LocalOrigin::Receiver { mutable: false }));
    let scope = builder.new_scope(None, hir_span(span));
    if invocation {
        // Seed storage through the allocator without binding input names into the decorator's module scope.
        for local in original.locals.iter().take(original.params.len()) {
            let id = builder.new_temp(local.ty.clone(), scope, local.span);
            builder.locals[id.index()] = local.clone();
        }
    }
    let mut statements = Vec::new();

    // ---- Context: the original callable with its checked passing parameters ----
    let mut parameters = original.params.clone();
    let IncanType::Function {
        params: original_parameters,
        ..
    } = original_ty
    else {
        return original.clone();
    };
    for (parameter, ty) in parameters.iter_mut().zip(original_parameters) {
        if ty.kind == IncanCallableParamKind::Normal {
            parameter.ty = ty.ty.clone();
        }
        parameter.kind = ty.kind;
        parameter.mutable = ty.is_mut;
        parameter.local = builder.new_temp(parameter.ty.clone(), scope, hir_span(span));
    }
    let mut value = builder.push_assign_temp(
        bir::Rvalue::Closure {
            params: parameters,
            captured_operands: Vec::new(),
            body: Box::new(bir::ClosureBody {
                original_callable: original.canonical.clone(),
                function_item: true,
                consumes_environment: false,
                capture_locals: Vec::new(),
                stmts: Vec::new(),
                result: bir::Operand::Constant(bir::Constant::None),
            }),
        },
        original_ty.clone(),
        scope,
        hir_span(span),
        &mut statements,
    );

    // ---- Context: decorators initialize the replacement bottom-up ----
    for decorator in decorators.iter().rev() {
        if decorators::from_segments(&decorator.node.path.segments).is_some() {
            continue;
        }
        let Some(replacement) = lower_application(&mut builder, decorator, value.clone(), scope, &mut statements)
        else {
            break;
        };
        value = replacement;
    }

    // ---- Context: the completed once-initialized replacement frame ----
    statements.push(bir::Statement {
        kind: bir::StatementKind::Return { value: Some(value) },
        span: hir_span(span),
    });
    let mut body = original.clone();
    body.return_type = replacement_ty.clone();
    body.locals = builder.locals;
    if !invocation {
        body.params.clear();
        body.param_locals.clear();
    }
    body.scopes = builder.scopes;
    body.block = bir::Block {
        scope,
        stmts: statements,
    };
    body.callable_representation = Some(bir::CallableRepresentation {
        function_pointer_locals: builder.function_pointer_locals,
        closure_holding_locals: Vec::new(),
        closure_holding_parameters: Vec::new(),
        closure_holding_return: false,
    });
    body
}

/// Lower one validated decorator application, preserving a named refusal for any missing expression or signature.
fn lower_application(
    builder: &mut BodyBuilder<'_, '_>,
    decorator: &ast::Spanned<ast::Decorator>,
    value: bir::Operand,
    scope: bir::ScopeId,
    statements: &mut Vec<bir::Statement>,
) -> Option<bir::Operand> {
    let Some(path) = decorator_expression(decorator) else {
        builder.push_unsupported_stmt(
            "decorator factory arguments".to_string(),
            hir_span(decorator.span),
            statements,
        );
        return None;
    };

    // ---- Context: the factory's result after checked decorator substitution ----
    if let Some(application) = builder
        .type_info
        .declarations
        .body_ir_decorator_applications
        .get(&(decorator.span.start, decorator.span.end))
    {
        let ty = crate::resolved_type_subst::substitute_resolved_type(
            &application.callable_type,
            &application.type_bindings,
        );
        builder
            .type_overrides
            .insert((decorator.span.start, decorator.span.end), builder.checked_type(&ty));
    }
    let callable = builder.lower_expr_to_operand(&path, scope, statements);
    let IncanType::Function { return_type, .. } = builder.resolve_ty(decorator.span) else {
        builder.push_unsupported_stmt(
            "decorator callable signature".to_string(),
            hir_span(decorator.span),
            statements,
        );
        return None;
    };
    let bir::Operand::Place(operand) = callable else {
        builder.push_unsupported_stmt(
            "decorator callable value".to_string(),
            hir_span(decorator.span),
            statements,
        );
        return None;
    };

    // ---- Context: apply the returned callable to the current initialized binding ----
    let callee = bir::Callee::Function(bir::CallableTarget::Local(bir::LocalCallableTarget {
        operand,
        binding: bir::ArgumentBinding::resolved_positional(1),
    }));
    Some(builder.push_call_temp(
        callee,
        fixed_elements(vec![value]),
        *return_type,
        scope,
        hir_span(decorator.span),
        false,
        statements,
    ))
}

/// Recreate only the expression the checker already evaluated at this decorator's own span. Its recorded identities,
/// argument bindings and result type remain authoritative; this helper does not resolve or recheck a factory.
fn decorator_expression(decorator: &ast::Spanned<ast::Decorator>) -> Option<ast::Spanned<ast::Expr>> {
    if decorator.node.path.segments.len() > 2 {
        return None;
    }
    let path = ast::Spanned::new(
        ast::Expr::Ident(decorator.node.path.segments.first()?.clone()),
        decorator.span,
    );
    if !decorator.node.is_call {
        return Some(path);
    }
    let mut arguments = Vec::with_capacity(decorator.node.args.len());
    for argument in &decorator.node.args {
        match argument {
            ast::DecoratorArg::Positional(value) => arguments.push(ast::CallArg::Positional(value.clone())),
            ast::DecoratorArg::Named(name, ast::DecoratorArgValue::Expr(value)) => arguments.push(ast::CallArg::Named(
                ast::Spanned::new(name.clone(), ast::Span::default()),
                value.clone(),
            )),
            ast::DecoratorArg::Named(_, ast::DecoratorArgValue::Type(_)) => return None,
        }
    }
    let expression = match decorator.node.path.segments.as_slice() {
        [_, method] => ast::Expr::MethodCall(
            Box::new(path),
            method.clone(),
            decorator.node.type_args.clone(),
            arguments,
        ),
        [_] => ast::Expr::Call(Box::new(path), decorator.node.type_args.clone(), arguments),
        _ => return None,
    };
    Some(ast::Spanned::new(expression, decorator.span))
}

/// Apply the checked decorator chain in the wrapper's own invocation frame, then forward its original arguments.
/// Open signatures use this frame; closed generic signatures keep their distinct per-instantiation initializer cache.
fn invocation_wrapper(
    decorators: &[ast::Spanned<ast::Decorator>],
    span: ast::Span,
    facts: &BodyIrLoweringFacts<'_, '_>,
    original: &bir::Body,
    original_ty: &IncanType,
    replacement_ty: &IncanType,
) -> Option<bir::Body> {
    let IncanType::Function { return_type, .. } = replacement_ty else {
        return None;
    };
    let mut body = initializer(decorators, span, facts, original, original_ty, replacement_ty, true);
    let returned = body.block.stmts.pop()?;
    let bir::StatementKind::Return {
        value: Some(bir::Operand::Place(callable)),
    } = returned.kind
    else {
        return None;
    };
    body.return_type = *return_type.clone();
    let result = bir::LocalId(body.locals.len().try_into().ok()?);
    body.locals.push(bir::LocalDecl {
        id: result,
        name: None,
        identity: None,
        ty: body.return_type.clone(),
        origin: bir::LocalOrigin::Temporary,
        scope: body.block.scope,
        span: body.span,
    });
    let arguments = forwarded_arguments(&body);
    body.block.stmts.push(bir::Statement {
        kind: bir::StatementKind::Call {
            destination: Some(bir::Place::from_local(result)),
            callee: bir::Callee::Function(bir::CallableTarget::Local(bir::LocalCallableTarget {
                operand: callable,
                binding: bir::ArgumentBinding::resolved_positional(body.params.len()),
            })),
            args: fixed_elements(arguments),
            may_panic: false,
        },
        span: body.span,
    });
    body.block.stmts.push(bir::Statement {
        kind: bir::StatementKind::Return {
            value: Some(bir::Operand::place(
                bir::Place::from_local(result),
                bir::OwnershipFact::Move,
                true,
            )),
        },
        span: body.span,
    });
    Some(body)
}

/// Forward each wrapper input once under its existing checked receiver and ownership contract.
fn forwarded_arguments(body: &bir::Body) -> Vec<bir::Operand> {
    body.params
        .iter()
        .map(|parameter| {
            let fact = match body.locals[parameter.local.index()].origin {
                bir::LocalOrigin::Receiver { mutable: true } => bir::OwnershipFact::MutBorrow,
                bir::LocalOrigin::Receiver { mutable: false } => bir::OwnershipFact::Borrow,
                _ if parameter.mutable => bir::OwnershipFact::MutBorrow,
                _ if parameter.ty.abi_v0_facts().ownership.is_trivially_copy() => bir::OwnershipFact::Copy,
                _ => bir::OwnershipFact::Move,
            };
            bir::Operand::place(bir::Place::from_local(parameter.local), fact, true)
        })
        .collect()
}

/// Build the visible callable with the final checked signature; every invocation reads the initialized replacement.
fn wrapper(original: &bir::Body, replacement: &bir::StaticDeclaration) -> Option<bir::Body> {
    let IncanType::Function { params, return_type } = &replacement.ty else {
        return None;
    };
    if params.len() != original.params.len() {
        return None;
    }
    let mut body = original.clone();
    body.return_type = *return_type.clone();
    body.locals.truncate(body.params.len());
    for (parameter, ty) in body.params.iter_mut().zip(params) {
        if matches!(
            body.locals[parameter.local.index()].origin,
            bir::LocalOrigin::Receiver { .. }
        ) {
            continue;
        }
        if ty.kind == IncanCallableParamKind::Normal {
            parameter.ty = ty.ty.clone();
        }
        parameter.kind = ty.kind;
        parameter.mutable = ty.is_mut;
        body.locals[parameter.local.index()].ty = parameter.ty.clone();
    }
    let scope = body.block.scope;
    let callable = bir::LocalId(body.locals.len().try_into().ok()?);
    body.locals.push(bir::LocalDecl {
        id: callable,
        name: None,
        identity: None,
        ty: replacement.ty.clone(),
        origin: bir::LocalOrigin::Temporary,
        scope,
        span: body.span,
    });
    let result = bir::LocalId(body.locals.len().try_into().ok()?);
    body.locals.push(bir::LocalDecl {
        id: result,
        name: None,
        identity: None,
        ty: body.return_type.clone(),
        origin: bir::LocalOrigin::Temporary,
        scope,
        span: body.span,
    });
    let global = bir::GlobalPlace {
        identity: replacement.canonical.clone(),
        ty: replacement.ty.clone(),
        write_policy: bir::GlobalWritePolicy::ReadOnly,
    };
    let argument_values = forwarded_arguments(&body);
    let stmts = wrapper_statements(&body, callable, result, global, argument_values);
    body.block = bir::Block { scope, stmts };
    body.callable_representation = Some(bir::CallableRepresentation {
        function_pointer_locals: vec![callable],
        closure_holding_locals: Vec::new(),
        closure_holding_parameters: Vec::new(),
        closure_holding_return: false,
    });
    Some(body)
}

/// Invoke the replacement with the wrapper frame and return its result without re-evaluating arguments.
fn wrapper_statements(
    body: &bir::Body,
    callable: bir::LocalId,
    result: bir::LocalId,
    global: bir::GlobalPlace,
    argument_values: Vec<bir::Operand>,
) -> Vec<bir::Statement> {
    vec![
        bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(callable),
                rvalue: bir::Rvalue::Use(bir::Operand::place(
                    bir::Place::from_global(global),
                    bir::OwnershipFact::Copy,
                    false,
                )),
            },
            span: body.span,
        },
        bir::Statement {
            kind: bir::StatementKind::Call {
                destination: Some(bir::Place::from_local(result)),
                callee: bir::Callee::Function(bir::CallableTarget::Local(bir::LocalCallableTarget {
                    operand: bir::PlaceOperand {
                        place: bir::Place::from_local(callable),
                        fact: bir::OwnershipFact::Copy,
                        last_use: true,
                    },
                    binding: bir::ArgumentBinding::resolved_positional(body.params.len()),
                })),
                args: fixed_elements(argument_values),
                may_panic: false,
            },
            span: body.span,
        },
        bir::Statement {
            kind: bir::StatementKind::Return {
                value: Some(bir::Operand::place(
                    bir::Place::from_local(result),
                    bir::OwnershipFact::Move,
                    true,
                )),
            },
            span: body.span,
        },
    ]
}
