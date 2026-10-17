//! Native future type aliases and explicit construction-time boxing at the plan boundary.

use crate::declarations;
use crate::plan::{Callee, CalleeKind, Function, PlanType};
use rustc_ast as ast;
use rustc_span::{Ident, Span, Symbol};
use thin_vec::{ThinVec, thin_vec};

/// Visit retained future outputs and nested callable arguments before AST injection.
fn retain_outputs(ty: &PlanType, outputs: &mut Vec<PlanType>) {
    match ty {
        PlanType::Future(arguments) => {
            if let [output] = arguments.as_slice() {
                retain_outputs(output, outputs);
                if !outputs.contains(output) {
                    outputs.push(output.clone());
                }
            }
        }
        PlanType::FunctionPointer(arguments)
        | PlanType::FunctionItem(_, arguments)
        | PlanType::Closure(arguments)
        | PlanType::ClosureRef(arguments)
        | PlanType::CallableArguments(arguments) => {
            for argument in arguments {
                retain_outputs(argument, outputs);
            }
        }
        _ => {}
    }
}

/// Collect aliases from every frame and external instantiation, including futures used only by a boxing shim.
pub(crate) fn outputs(plan: &crate::plan::Plan) -> Vec<PlanType> {
    let mut outputs = Vec::new();
    for function in &plan.functions {
        retain_outputs(&function.return_type, &mut outputs);
        for local in &function.locals {
            retain_outputs(&local.ty, &mut outputs);
        }
        if let Some(Callee {
            kind: CalleeKind::Instantiated(_, arguments) | CalleeKind::FutureMethod(_, arguments),
            ..
        }) = &function.future_callee
        {
            for argument in arguments {
                retain_outputs(argument, &mut outputs);
            }
        }
    }
    for external in &plan.externals {
        retain_outputs(&external.return_type, &mut outputs);
        for argument in external.parameters.iter().chain(&external.type_arguments) {
            retain_outputs(argument, &mut outputs);
        }
    }
    for model in &plan.models {
        for argument in &model.library_type_arguments {
            retain_outputs(argument, &mut outputs);
        }
        for field in &model.fields {
            retain_outputs(&field.ty, &mut outputs);
        }
    }
    for value in &plan.enums {
        for variant in &value.variants {
            for field in &variant.fields {
                retain_outputs(field, &mut outputs);
            }
        }
    }
    outputs
}

/// Give each retained output representation one unambiguous crate-local boxed-future alias.
pub(crate) fn alias_name(output: &PlanType) -> String {
    let mut name = String::from("__incan_future_type_");
    for byte in format!("{output:?}").bytes() {
        name.push_str(&format!("{byte:02x}"));
    }
    name
}

/// Build a canonical path without parsing generated Rust text.
fn path(segments: &[&str], span: Span) -> ast::Path {
    let mut path = ast::Path {
        span,
        segments: thin_vec![ast::PathSegment::path_root(span)],
        tokens: None,
    };
    for segment in segments {
        path.segments
            .push(ast::PathSegment::from_ident(Ident::new(Symbol::intern(segment), span)));
    }
    path
}

/// Wrap one type in a standard generic constructor, retaining structural type arguments.
fn applied(segments: &[&str], argument: Box<ast::Ty>, span: Span) -> Box<ast::Ty> {
    let mut path = path(segments, span);
    if let Some(segment) = path.segments.last_mut() {
        segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
            span,
            args: thin_vec![ast::AngleBracketedArg::Arg(ast::GenericArg::Type(argument))],
        })));
    }
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, path),
        span,
        tokens: None,
    })
}

/// Declare Pin<Box<dyn Future<Output = T> + Send>>, leaving Future and Send obligations to native checking.
pub(crate) fn alias(output: &PlanType, span: Span) -> Box<ast::Item> {
    alias_with_lifetime(output, false, span)
}

/// Preserve a receiver loan in the boxing shim while native coroutine MIR retains its erased ABI.
pub(crate) fn borrowed_alias(output: &PlanType, span: Span) -> Box<ast::Item> {
    alias_with_lifetime(output, true, span)
}

/// Construct the same pinned representation with either an owned or explicitly borrowed object lifetime.
fn alias_with_lifetime(output: &PlanType, borrowed: bool, span: Span) -> Box<ast::Item> {
    let mut future = path(&["std", "future", "Future"], span);
    if let Some(segment) = future.segments.last_mut() {
        segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
            span,
            args: thin_vec![ast::AngleBracketedArg::Constraint(ast::AssocItemConstraint {
                id: ast::DUMMY_NODE_ID,
                ident: Ident::new(Symbol::intern("Output"), span),
                gen_args: None,
                kind: ast::AssocItemConstraintKind::Equality {
                    term: ast::Term::Ty(declarations::ty(output, span))
                },
                span,
            })],
        })));
    }
    let mut bounds: Vec<_> = [future, path(&["std", "marker", "Send"], span)]
        .into_iter()
        .map(|path| {
            ast::GenericBound::Trait(ast::PolyTraitRef::new(
                ThinVec::new(),
                path,
                ast::TraitBoundModifiers::NONE,
                span,
                ast::Parens::No,
            ))
        })
        .collect();
    let mut generics = ast::Generics::default();
    let mut name = alias_name(output);
    if borrowed {
        name.push_str("_borrowed");
        let ident = Ident::new(Symbol::intern("'future"), span);
        bounds.push(ast::GenericBound::Outlives(ast::Lifetime {
            id: ast::DUMMY_NODE_ID,
            ident,
        }));
        generics.params.push(ast::GenericParam {
            id: ast::DUMMY_NODE_ID,
            ident,
            attrs: ThinVec::new(),
            bounds: Vec::new(),
            is_placeholder: false,
            kind: ast::GenericParamKind::Lifetime,
            colon_span: None,
        });
    }
    let object = Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::TraitObject(bounds, ast::TraitObjectSyntax::Dyn),
        span,
        tokens: None,
    });
    let boxed = applied(&["std", "boxed", "Box"], object, span);
    let pinned = applied(&["std", "pin", "Pin"], boxed, span);
    declarations::item(
        ast::ItemKind::TyAlias(Box::new(ast::TyAlias {
            defaultness: ast::Defaultness::Implicit,
            ident: Ident::new(Symbol::intern(&name), span),
            generics,
            after_where_clause: ast::WhereClause::default(),
            bounds: Vec::new(),
            ty: Some(pinned),
        })),
        span,
    )
}

/// Count retained nominal loans captured by a native SDK future constructor.
pub(crate) fn borrowed_captures(function: &Function) -> usize {
    function
        .parameters
        .iter()
        .filter(|parameter| matches!(parameter.ty, PlanType::ModelRef(..) | PlanType::ModelMutRef(..)))
        .count()
}

/// Tie a single receiver's boxed future to its input loan through ordinary native lifetime elision.
pub(crate) fn constructor_return_type(function: &Function, span: Span) -> Option<Box<ast::Ty>> {
    function.future_callee.as_ref()?;
    if borrowed_captures(function) != 1 {
        return None;
    }
    let PlanType::Future(outputs) = &function.return_type else {
        return None;
    };
    let [output] = outputs.as_slice() else {
        return None;
    };
    let name = format!("{}_borrowed", alias_name(output));
    let mut path = ast::Path::from_ident(Ident::new(Symbol::intern(&name), span));
    path.segments[0].args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
        span,
        args: thin_vec![ast::AngleBracketedArg::Arg(ast::GenericArg::Lifetime(ast::Lifetime {
            id: ast::DUMMY_NODE_ID,
            ident: Ident::new(Symbol::intern("'_"), span),
        }))],
    })));
    Some(Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, path),
        span,
        tokens: None,
    }))
}

/// Allocate a source-free expression whose node identity rustc assigns after injection.
fn expression(kind: ast::ExprKind, span: Span) -> Box<ast::Expr> {
    Box::new(ast::Expr {
        id: ast::DUMMY_NODE_ID,
        kind,
        span,
        attrs: ThinVec::new(),
        tokens: None,
    })
}

/// Name the exact source or SDK future-producing callee; retained inherent owner arguments belong to its type path.
fn callee_path(callee: &Callee, span: Span) -> Option<ast::Path> {
    let (name, types) = match &callee.kind {
        CalleeKind::Planned(name) => {
            return Some(ast::Path {
                span,
                segments: thin_vec![
                    ast::PathSegment::from_ident(Ident::new(Symbol::intern("crate"), span)),
                    ast::PathSegment::from_ident(Ident::new(Symbol::intern(name), span))
                ],
                tokens: None,
            });
        }
        CalleeKind::External(name) => (name, &[][..]),
        CalleeKind::Instantiated(name, types) => (name, types.as_slice()),
        CalleeKind::FutureMethod(name, types) => (name, types.as_slice()),
        _ => return None,
    };
    let names: Vec<_> = name.split("::").collect();
    let mut path = path(&names, span);
    if !types.is_empty() {
        let position = if matches!(callee.kind, CalleeKind::FutureMethod(..)) {
            path.segments.len().checked_sub(2)?
        } else {
            path.segments.len().checked_sub(1)?
        };
        let segment = path.segments.get_mut(position)?;
        segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
            span,
            args: types
                .iter()
                .map(|ty| ast::AngleBracketedArg::Arg(ast::GenericArg::Type(declarations::ty(ty, span))))
                .collect(),
        })));
    }
    Some(path)
}

/// Construct a boxed future without running its body; only this boxing shim retains native constructor MIR.
pub(crate) fn declaration_body(function: &Function, span: Span) -> Option<Box<ast::Block>> {
    let callee = function.future_callee.as_ref()?;
    let target = expression(ast::ExprKind::Path(None, callee_path(callee, span)?), span);
    let arguments = function
        .parameters
        .iter()
        .map(|parameter| {
            expression(
                ast::ExprKind::Path(
                    None,
                    ast::Path::from_ident(Ident::new(Symbol::intern(&parameter.name), span)),
                ),
                span,
            )
        })
        .collect();
    let future = expression(ast::ExprKind::Call(target, arguments), span);
    let pin = expression(
        ast::ExprKind::Path(None, path(&["std", "boxed", "Box", "pin"], span)),
        span,
    );
    let boxed = expression(ast::ExprKind::Call(pin, thin_vec![future]), span);
    Some(Box::new(ast::Block {
        stmts: thin_vec![ast::Stmt {
            id: ast::DUMMY_NODE_ID,
            kind: ast::StmtKind::Expr(boxed),
            span
        }],
        id: ast::DUMMY_NODE_ID,
        rules: ast::BlockCheckMode::Default,
        span,
        tokens: None,
    }))
}
