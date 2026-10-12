//! RFC 036 source originals and once-initialized replacement frames, used exclusively by Body IR.

use super::args::fixed_elements;
use super::*;
use incan_lang::lang::decorators;

/// Admit only nongeneric function decorators whose checked path can be read as a source callable.
/// Generic originals, extern delegations and method receivers retain their existing named refusal until
/// independently proven; an extern's delegation must not bypass the visible replacement wrapper.
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
    let (ResolvedType::Function(original, _), ResolvedType::Function(replacement, _)) =
        (&binding.original_ty, &binding.ty)
    else {
        return false;
    };
    original.len() == replacement.len()
        && original
            .iter()
            .zip(replacement)
            .all(|(left, right)| left.ty == right.ty && left.is_mut == right.is_mut)
        && function.type_params.is_empty()
        && !function.is_async()
        && function.decorators.iter().all(
            |decorator| match decorators::from_segments(&decorator.node.path.segments) {
                Some(decorators::DecoratorId::RustExtern) => false,
                Some(_) => true,
                None => decorator.node.path.segments.len() == 1,
            },
        )
}

/// Retain originals and replacement initializers in source order, replacing their visible bodies with wrappers.
pub(super) fn collect(
    program: &ast::Program,
    facts: &BodyIrLoweringFacts<'_, '_>,
    bodies: &mut [bir::Body],
    statics: &mut Vec<bir::StaticDeclaration>,
) -> Vec<bir::DecoratedBinding> {
    let mut retained = Vec::new();
    for declaration in &program.declarations {
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
        let Some(canonical) = body.canonical.clone() else {
            continue;
        };
        let ty = parameter_types::checked_semantic_type(&binding.ty, facts.type_info);
        let original_ty = parameter_types::checked_semantic_type(&binding.original_ty, facts.type_info);
        let initializer = initializer(function, declaration.span, facts, body, &original_ty, &ty);
        let replacement = bir::StaticDeclaration {
            canonical,
            ty: ty.clone(),
            initial: bir::StaticInitializer::Computed(Box::new(initializer)),
        };
        let original_body = body.clone();
        if let Some(wrapper) = wrapper(&original_body, &replacement) {
            *body = wrapper;
            retained.push(bir::DecoratedBinding { original_body });
            statics.push(replacement);
        }
    }
    retained
}

/// Compute a decorator chain bottom-up from an explicitly identified original callable, without rechecking source.
fn initializer(
    function: &ast::FunctionDecl,
    span: ast::Span,
    facts: &BodyIrLoweringFacts<'_, '_>,
    original: &bir::Body,
    original_ty: &IncanType,
    replacement_ty: &IncanType,
) -> bir::Body {
    let mut builder = BodyBuilder::new(facts, replacement_ty.clone());
    let scope = builder.new_scope(None, hir_span(span));
    let mut statements = Vec::new();
    let mut parameters = original.params.clone();
    for parameter in &mut parameters {
        parameter.local = builder.new_temp(parameter.ty.clone(), scope, hir_span(span));
    }
    let mut value = builder.push_assign_temp(
        bir::Rvalue::Closure {
            params: parameters,
            captured_operands: Vec::new(),
            body: Box::new(bir::ClosureBody {
                original_callable: original.canonical.clone(),
                function_item: true,
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
    for decorator in function.decorators.iter().rev() {
        if decorators::from_segments(&decorator.node.path.segments).is_some() {
            continue;
        }
        let Some(path) = decorator_expression(decorator) else {
            builder.push_unsupported_stmt(
                "decorator factory arguments".to_string(),
                hir_span(decorator.span),
                &mut statements,
            );
            break;
        };
        let callable = builder.lower_expr_to_operand(&path, scope, &mut statements);
        let IncanType::Function { return_type, .. } = builder.resolve_ty(decorator.span) else {
            builder.push_unsupported_stmt(
                "decorator callable signature".to_string(),
                hir_span(decorator.span),
                &mut statements,
            );
            break;
        };
        let bir::Operand::Place(operand) = callable else {
            builder.push_unsupported_stmt(
                "decorator callable value".to_string(),
                hir_span(decorator.span),
                &mut statements,
            );
            break;
        };
        let callee = bir::Callee::Function(bir::CallableTarget::Local(bir::LocalCallableTarget {
            operand,
            binding: bir::ArgumentBinding::resolved_positional(1),
        }));
        value = builder.push_call_temp(
            callee,
            fixed_elements(vec![value]),
            *return_type,
            scope,
            hir_span(decorator.span),
            false,
            &mut statements,
        );
    }
    statements.push(bir::Statement {
        kind: bir::StatementKind::Return { value: Some(value) },
        span: hir_span(span),
    });
    let mut body = original.clone();
    body.return_type = replacement_ty.clone();
    body.locals = builder.locals;
    body.params.clear();
    body.param_locals.clear();
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

/// Recreate only the expression the checker already evaluated at this decorator's own span. Its recorded identities,
/// argument bindings and result type remain authoritative; this helper does not resolve or recheck a factory.
fn decorator_expression(decorator: &ast::Spanned<ast::Decorator>) -> Option<ast::Spanned<ast::Expr>> {
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
    Some(ast::Spanned::new(
        ast::Expr::Call(Box::new(path), decorator.node.type_args.clone(), arguments),
        decorator.span,
    ))
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
        parameter.ty = ty.ty.clone();
        parameter.mutable = ty.is_mut;
        body.locals[parameter.local.index()].ty = ty.ty.clone();
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
    let argument_values = body
        .params
        .iter()
        .map(|parameter| {
            bir::Operand::place(
                bir::Place::from_local(parameter.local),
                if parameter.mutable {
                    bir::OwnershipFact::MutBorrow
                } else if parameter.ty.abi_v0_facts().ownership.is_trivially_copy() {
                    bir::OwnershipFact::Copy
                } else {
                    bir::OwnershipFact::Move
                },
                true,
            )
        })
        .collect();
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
