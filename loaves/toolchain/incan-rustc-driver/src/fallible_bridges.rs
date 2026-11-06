//! Native trait ABI forwarding for standard iterator contracts selected by Incan lowering.

use crate::ast_build::{
    binding, block, expression, expression_statement, global_path, ident, inherited, local, path_ty, self_value,
    trait_bound,
};
use crate::error::PlanError;
use crate::plan::{FallibleIteratorMethods, IteratorMethods, ModelDeclaration, Plan, PlanType};
use rustc_ast as ast;
use rustc_span::{Ident, Span, kw, respan};
use thin_vec::{ThinVec, thin_vec};

/// Require a concrete source layout and a unique mutable source poll function before declaring native authority.
fn validate<'a>(
    plan: &'a Plan,
    owner: usize,
    model: &ModelDeclaration,
    methods: &FallibleIteratorMethods,
) -> Result<&'a crate::plan::Function, PlanError> {
    let refused = || PlanError::Invalid {
        function: model.name.clone(),
        reason: "unsupported native fallible iterator bridge ABI".into(),
    };
    if !model.library_item.is_empty() || !model.derives.iter().any(|derive| derive == "Clone") {
        return Err(refused());
    }
    checked_poll(plan, owner, model, &methods.next)
}

/// Require a unique concrete mutable poll shared by both native iterator ABI adapters.
fn checked_poll<'a>(
    plan: &'a Plan,
    owner: usize,
    model: &ModelDeclaration,
    next: &str,
) -> Result<&'a crate::plan::Function, PlanError> {
    let refused = || PlanError::Invalid {
        function: model.name.clone(),
        reason: "unsupported native iterator poll ABI".into(),
    };
    let owner = i64::try_from(owner).map_err(|_| refused())?;
    if !model.library_item.is_empty() {
        return Err(refused());
    }
    let mut candidates = plan.functions.iter().filter(|function| function.name == next);
    let Some(function) = candidates.next() else {
        return Err(refused());
    };
    if candidates.next().is_some()
        || function.parameters.len() != 1
        || function.parameters[0].ty != PlanType::ModelMutRef(owner, model.name.clone())
    {
        return Err(refused());
    }
    Ok(function)
}

/// Forward only the Incan-selected ordinary poll; rustc checks its Option[item] result and all SDK bounds.
fn ordinary_poll(
    plan: &Plan,
    owner: usize,
    model: &ModelDeclaration,
    methods: &IteratorMethods,
    span: Span,
) -> Result<Box<ast::AssocItem>, PlanError> {
    checked_poll(plan, owner, model, &methods.next)?;
    Ok(method(
        "__next__",
        true,
        ThinVec::new(),
        ThinVec::new(),
        path_ty(
            applied(
                &["std", "option", "Option"],
                vec![crate::declarations::ty(&methods.item, span)],
                span,
            ),
            span,
        ),
        call(&[&methods.next], thin_vec![self_value(span)], span),
        span,
    ))
}

/// Satisfy the ordinary SDK Iterator ABI without adding Clone or replacing the SDK-owned sum default.
pub(crate) fn ordinary_items(plan: &Plan, span: Span) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut result = Vec::new();
    for (owner, model) in plan.models.iter().enumerate() {
        let Some(methods) = &model.iterator else {
            continue;
        };
        let poll = ordinary_poll(plan, owner, model, methods, span)?;
        let path = applied(
            &["incan_stdlib_core", "derives", "Iterator"],
            vec![crate::declarations::ty(&methods.item, span)],
            span,
        );
        let mut item = crate::declarations::item(
            ast::ItemKind::Impl(ast::Impl {
                generics: ast::Generics::default(),
                constness: ast::Const::No,
                of_trait: Some(Box::new(ast::TraitImplHeader {
                    defaultness: ast::Defaultness::Implicit,
                    safety: ast::Safety::Default,
                    polarity: ast::ImplPolarity::Positive,
                    trait_ref: ast::TraitRef {
                        path,
                        ref_id: ast::DUMMY_NODE_ID,
                    },
                })),
                self_ty: named(&model.name, span),
                items: thin_vec![poll],
            }),
            span,
        );
        item.vis = inherited(span);
        result.push(item);
    }
    Ok(result)
}

/// Declare all native slots together; runtime helpers contain the default behavior and rustc checks every bound.
pub(crate) fn items(plan: &Plan, span: Span) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut result = Vec::new();
    for (owner, model) in plan.models.iter().enumerate() {
        let Some(methods) = &model.fallible_iterator else {
            continue;
        };
        let poll = validate(plan, owner, model, methods)?;
        let mut functions = thin_vec![method(
            "__iter__",
            false,
            ThinVec::new(),
            ThinVec::new(),
            named("Self", span),
            cloned_self(span),
            span
        )];
        functions.push(method(
            "__next__",
            true,
            ThinVec::new(),
            ThinVec::new(),
            crate::declarations::ty(&poll.return_type, span),
            call(&[&methods.next], thin_vec![self_value(span)], span),
            span,
        ));
        for name in [
            "map",
            "filter",
            "flat_map",
            "take",
            "inspect",
            "map_err",
            "inspect_err",
            "collect",
            "fold",
        ] {
            functions.push(default_method(name, methods, span)?);
        }
        let path = protocol(
            vec![
                crate::declarations::ty(&methods.item, span),
                crate::declarations::ty(&methods.error, span),
            ],
            span,
        );
        let mut item = crate::declarations::item(
            ast::ItemKind::Impl(ast::Impl {
                generics: ast::Generics::default(),
                constness: ast::Const::No,
                of_trait: Some(Box::new(ast::TraitImplHeader {
                    defaultness: ast::Defaultness::Implicit,
                    safety: ast::Safety::Default,
                    polarity: ast::ImplPolarity::Positive,
                    trait_ref: ast::TraitRef {
                        path,
                        ref_id: ast::DUMMY_NODE_ID,
                    },
                })),
                self_ty: named(&model.name, span),
                items: functions,
            }),
            span,
        );
        item.vis = inherited(span);
        result.push(item);
    }
    Ok(result)
}

/// Build one type path without recovering an ABI from a diagnostic spelling.
fn named(name: &str, span: Span) -> Box<ast::Ty> {
    path_ty(ast::Path::from_ident(ident(name, span)), span)
}

/// Apply explicit type arguments to one structural path.
fn applied(segments: &[&str], arguments: Vec<Box<ast::Ty>>, span: Span) -> ast::Path {
    let mut path = global_path(segments, span);
    crate::shared_callables::type_arguments(&mut path, arguments.into_iter().collect(), span);
    path
}

/// Name the actual SDK trait whose implementation satisfies native adapter bounds.
fn protocol(arguments: Vec<Box<ast::Ty>>, span: Span) -> ast::Path {
    applied(&["incan_stdlib_core", "derives", "FallibleIterator"], arguments, span)
}

/// Forward one function call; a single-segment source symbol remains crate-local.
fn call(segments: &[&str], arguments: ThinVec<Box<ast::Expr>>, span: Span) -> Box<ast::Expr> {
    let target = if let [name] = segments {
        local(name, span)
    } else {
        expression(ast::ExprKind::Path(None, global_path(segments, span)), span)
    };
    expression(ast::ExprKind::Call(target, arguments), span)
}

/// Match the immutable source receiver's value semantics at a default factory boundary.
fn cloned_self(span: Span) -> Box<ast::Expr> {
    call(&["std", "clone", "Clone", "clone"], thin_vec![self_value(span)], span)
}

/// Retain the native Clone and exact callable bounds of a generic SDK slot.
fn parameter(name: &str, callable: Option<Vec<Box<ast::Ty>>>, span: Span) -> ast::GenericParam {
    let mut bounds = vec![trait_bound(global_path(&["std", "clone", "Clone"], span), span)];
    if let Some(arguments) = callable {
        let kind = if arguments.len() == 3 { "Callable2" } else { "Callable1" };
        bounds.push(trait_bound(
            applied(&["incan_stdlib_core", "traits", kind], arguments, span),
            span,
        ));
    }
    ast::GenericParam {
        id: ast::DUMMY_NODE_ID,
        ident: ident(name, span),
        attrs: ThinVec::new(),
        bounds,
        is_placeholder: false,
        kind: ast::GenericParamKind::Type { default: None },
        colon_span: None,
    }
}

/// Preserve an opaque factory result while rustc verifies the concrete runtime helper's implementation.
fn opaque(item: Box<ast::Ty>, error: Box<ast::Ty>, span: Span) -> Box<ast::Ty> {
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::ImplTrait(
            ast::DUMMY_NODE_ID,
            vec![trait_bound(protocol(vec![item, error], span), span)],
        ),
        span,
        tokens: None,
    })
}

/// Forward each checked standard default to its Incan runtime ABI, preserving generic binders and receiver mutation.
fn default_method(name: &str, methods: &FallibleIteratorMethods, span: Span) -> Result<Box<ast::AssocItem>, PlanError> {
    let item = || crate::declarations::ty(&methods.item, span);
    let error = || crate::declarations::ty(&methods.error, span);
    let mut generics = ThinVec::new();
    let mut inputs = ThinVec::new();
    let mutable = matches!(name, "collect" | "fold");
    let mut arguments = thin_vec![if mutable { self_value(span) } else { cloned_self(span) }];
    let mut output = opaque(item(), error(), span);
    match name {
        "take" => inputs.push(("count", crate::declarations::ty(&PlanType::Int, span))),
        "collect" => {
            let list = path_ty(applied(&["std", "vec", "Vec"], vec![item()], span), span);
            output = path_ty(applied(&["std", "result", "Result"], vec![list, error()], span), span);
        }
        "fold" => {
            generics.push(parameter("U", None, span));
            generics.push(parameter(
                "F",
                Some(vec![named("U", span), item(), named("U", span)]),
                span,
            ));
            inputs.push(("initial", named("U", span)));
            inputs.push(("f", named("F", span)));
            output = path_ty(
                applied(&["std", "result", "Result"], vec![named("U", span), error()], span),
                span,
            );
        }
        _ => {
            let (input, result) = match name {
                "map" | "flat_map" => {
                    generics.push(parameter("U", None, span));
                    output = opaque(named("U", span), error(), span);
                    let result = if name == "flat_map" {
                        path_ty(applied(&["std", "vec", "Vec"], vec![named("U", span)], span), span)
                    } else {
                        named("U", span)
                    };
                    (item(), result)
                }
                "filter" => (item(), crate::declarations::ty(&PlanType::Bool, span)),
                "inspect" => (item(), crate::declarations::ty(&PlanType::Unit, span)),
                "map_err" => {
                    generics.push(parameter("U", None, span));
                    output = opaque(item(), named("U", span), span);
                    (error(), named("U", span))
                }
                "inspect_err" => (error(), crate::declarations::ty(&PlanType::Unit, span)),
                _ => {
                    return Err(PlanError::Invalid {
                        function: name.into(),
                        reason: "unsupported native fallible iterator default slot".into(),
                    });
                }
            };
            generics.push(parameter("F", Some(vec![input, result]), span));
            inputs.push(("f", named("F", span)));
        }
    }
    for (name, _) in &inputs {
        arguments.push(local(name, span));
    }
    let helper = format!("fallible_{name}");
    Ok(method(
        name,
        mutable,
        generics,
        inputs,
        output,
        call(&["incan_native_runtime", &helper], arguments, span),
        span,
    ))
}

/// Declare a structural forwarding method; only rustc's ordinary MIR executes the generic call adapter.
fn method(
    name: &str,
    mutable: bool,
    parameters: ThinVec<ast::GenericParam>,
    arguments: ThinVec<(&str, Box<ast::Ty>)>,
    mut output: Box<ast::Ty>,
    body: Box<ast::Expr>,
    span: Span,
) -> Box<ast::AssocItem> {
    // The SDK excludes the receiver borrow from its opaque factory results. This concrete impl captures only its
    // method type parameters, following the same signature boundary as legacy's method-return emitter.
    if let ast::TyKind::ImplTrait(_, bounds) = &mut output.kind {
        let captures = parameters
            .iter()
            .map(|parameter| ast::PreciseCapturingArg::Arg(ast::Path::from_ident(parameter.ident), ast::DUMMY_NODE_ID))
            .collect();
        bounds.push(ast::GenericBound::Use(captures, span));
    }
    let mut inputs = thin_vec![ast::Param::from_self(
        ThinVec::new(),
        respan(
            span,
            ast::SelfKind::Region(
                None,
                if mutable {
                    ast::Mutability::Mut
                } else {
                    ast::Mutability::Not
                }
            )
        ),
        Ident::new(kw::SelfLower, span)
    )];
    for (name, ty) in arguments {
        inputs.push(ast::Param {
            attrs: ThinVec::new(),
            ty,
            pat: binding(name, span),
            id: ast::DUMMY_NODE_ID,
            span,
            is_placeholder: false,
        });
    }
    Box::new(ast::AssocItem {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: inherited(span),
        tokens: None,
        kind: ast::AssocItemKind::Fn(Box::new(ast::Fn {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(name, span),
            generics: ast::Generics {
                params: parameters,
                where_clause: ast::WhereClause::default(),
                span,
            },
            sig: ast::FnSig {
                header: ast::FnHeader::default(),
                decl: Box::new(ast::FnDecl {
                    inputs,
                    output: ast::FnRetTy::Ty(output),
                }),
                span,
            },
            contract: None,
            define_opaque: None,
            body: Some(block(thin_vec![expression_statement(body, span)], span)),
            eii_impls: ThinVec::new(),
        })),
    })
}
