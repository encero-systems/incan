//! Native cloning for erased captured environments, preserving the checked `Fn` input and output ABI.
//!
//! Each exact signature has a private object-safe clone operation. The blanket implementation clones the concrete
//! environment, and `Box` implements `Clone` by dispatching through its pointee, so SDK `Clone + Callable1` bounds
//! retain their ordinary native meaning. AST construction is structural; no Rust source is generated or parsed.

use crate::error::PlanError;
use crate::plan::{Plan, PlanType};
use crate::{callees, closures, declarations, types};
use rustc_ast as ast;
use rustc_hir::def::DefKind;
use rustc_hir::def_id::DefId;
use rustc_middle::ty::TyCtxt;
use rustc_span::{Ident, Span, Symbol, kw, respan};
use thin_vec::{ThinVec, thin_vec};

/// Encode the entire typed signature injectively as an identifier, never as native type-selection authority.
pub fn alias_name(signature: &[PlanType]) -> String {
    let mut name = String::from("__incan_cloned_callable_");
    for byte in format!("{signature:?}").bytes() {
        use std::fmt::Write;
        let _ = write!(name, "{byte:02x}");
    }
    name
}

/// Keep the trait and alias namespaces distinct for the same exact signature.
fn trait_name(signature: &[PlanType]) -> String {
    format!("{}_trait", alias_name(signature))
}

/// Collect every environment signature from typed declaration storage, including nested callable inputs.
fn signatures(plan: &Plan) -> Vec<Vec<PlanType>> {
    let mut result = Vec::new();
    for function in &plan.functions {
        for local in &function.locals {
            collect(&local.ty, &mut result);
        }
        for parameter in &function.parameters {
            collect(&parameter.ty, &mut result);
        }
        collect(&function.return_type, &mut result);
    }
    for model in &plan.models {
        for field in &model.fields {
            collect(&field.ty, &mut result);
        }
        for argument in &model.library_type_arguments {
            collect(argument, &mut result);
        }
    }
    for value in &plan.enums {
        for variant in &value.variants {
            for field in &variant.fields {
                collect(field, &mut result);
            }
        }
    }
    for external in &plan.externals {
        for ty in external
            .parameters
            .iter()
            .chain(&external.type_arguments)
            .chain([&external.return_type])
        {
            collect(ty, &mut result);
        }
    }
    result
}

/// Recurse through the plan's typed composite layouts rather than inspecting serialized names or payloads.
fn collect(ty: &PlanType, signatures: &mut Vec<Vec<PlanType>>) {
    match ty {
        PlanType::Closure(signature) | PlanType::ClosureRef(signature) => {
            if !signatures.contains(signature) {
                signatures.push(signature.clone());
            }
            for ty in signature {
                collect(ty, signatures);
            }
        }
        PlanType::FunctionPointer(signature)
        | PlanType::FunctionItem(_, signature)
        | PlanType::CallableArguments(signature) => {
            for ty in signature {
                collect(ty, signatures);
            }
        }
        PlanType::Tuple(elements) => {
            for element in elements {
                collect(&crate::plan::tuple_element_type(element.clone()), signatures);
            }
        }
        PlanType::ZipIterator(left, right)
        | PlanType::ZipIteratorRef(left, right)
        | PlanType::ZipIteratorMutRef(left, right) => {
            collect(&crate::plan::tuple_element_type(left.clone()), signatures);
            collect(&crate::plan::tuple_element_type(right.clone()), signatures);
        }
        PlanType::List(leaf, _)
        | PlanType::ListRef(leaf, _)
        | PlanType::ListMutRef(leaf, _)
        | PlanType::Set(leaf)
        | PlanType::SetRef(leaf)
        | PlanType::SetMutRef(leaf)
        | PlanType::Generator(leaf, _)
        | PlanType::GeneratorMutRef(leaf, _)
        | PlanType::GeneratorYield(leaf, _)
        | PlanType::GeneratorYieldRef(leaf, _) => {
            collect(&crate::plan::list_leaf_type(leaf.clone()), signatures);
        }
        PlanType::Dict(key, value) | PlanType::DictRef(key, value) | PlanType::DictMutRef(key, value) => {
            collect(&crate::plan::list_leaf_type(key.clone()), signatures);
            collect(&crate::plan::list_leaf_type(value.clone()), signatures);
        }
        _ => {}
    }
}

/// Reject source collisions before adding the private ABI declarations to rustc's type namespace.
pub fn items(plan: &Plan, span: Span) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut items = Vec::new();
    for signature in signatures(plan) {
        let alias = alias_name(&signature);
        let name = trait_name(&signature);
        let occupied = |candidate: &str| {
            plan.models.iter().any(|value| value.name == candidate)
                || plan.enums.iter().any(|value| value.name == candidate)
                || plan.functions.iter().any(|value| value.name == candidate)
                || plan
                    .externals
                    .iter()
                    .any(|value| value.path.split("::").next() == Some(candidate))
        };
        if occupied(&alias) || occupied(&name) {
            return Err(PlanError::Unsupported {
                function: alias,
                reason: "private callable ABI declaration collides with a source declaration".into(),
            });
        }
        items.extend(signature_items(&signature, span));
    }
    Ok(items)
}

/// Declare the object-safe clone trait, its dyn alias, concrete blanket implementation, and boxed Clone implementation.
fn signature_items(signature: &[PlanType], span: Span) -> Vec<Box<ast::Item>> {
    let trait_path = path(&[&trait_name(signature)], span);
    let object = object_ty(signature, span);
    let boxed = boxed(object.clone(), span);
    let mut bounds = vec![
        fn_bound(signature, span),
        bound(path(&["std", "clone", "Clone"], span), span),
    ];
    bounds.push(ast::GenericBound::Outlives(ast::Lifetime {
        id: ast::DUMMY_NODE_ID,
        ident: Ident::new(kw::StaticLifetime, span),
    }));
    let generics = ast::Generics {
        params: thin_vec![ast::GenericParam {
            id: ast::DUMMY_NODE_ID,
            ident: ident("F", span),
            attrs: ThinVec::new(),
            bounds,
            is_placeholder: false,
            kind: ast::GenericParamKind::Type { default: None },
            colon_span: Some(span),
        }],
        ..ast::Generics::default()
    };
    let cloned = method_call(local("self", span), "clone", span);
    let new = call(path(&["std", "boxed", "Box", "new"], span), thin_vec![cloned], span);
    let pointee = method_call(local("self", span), "as_ref", span);
    let dispatched = method_call(pointee, "__incan_clone_environment", span);
    vec![
        item(
            ast::ItemKind::Trait(Box::new(ast::Trait {
                impl_restriction: ast::ImplRestriction {
                    kind: ast::RestrictionKind::Unrestricted,
                    span,
                    tokens: None,
                },
                constness: ast::Const::No,
                safety: ast::Safety::Default,
                is_auto: ast::IsAuto::No,
                ident: ident(&trait_name(signature), span),
                generics: ast::Generics::default(),
                bounds: vec![fn_bound(signature, span)],
                items: thin_vec![method("__incan_clone_environment", boxed.clone(), None, span)],
            })),
            span,
        ),
        item(
            ast::ItemKind::TyAlias(Box::new(ast::TyAlias {
                defaultness: ast::Defaultness::Implicit,
                ident: ident(&alias_name(signature), span),
                generics: ast::Generics::default(),
                after_where_clause: ast::WhereClause::default(),
                bounds: Vec::new(),
                ty: Some(object),
            })),
            span,
        ),
        implementation(
            trait_path,
            path_ty(path(&["F"], span), span),
            generics,
            method("__incan_clone_environment", boxed.clone(), Some(new), span),
            span,
        ),
        implementation(
            path(&["std", "clone", "Clone"], span),
            boxed.clone(),
            ast::Generics::default(),
            method("clone", boxed, Some(dispatched), span),
            span,
        ),
    ]
}

/// Build the exact checked `Fn(A, ...) -> R` supertrait, retaining borrowed-input lifetime elision for rustc.
fn fn_bound(signature: &[PlanType], span: Span) -> ast::GenericBound {
    let mut path = path(&["std", "ops", "Fn"], span);
    if let (Some(segment), Some((result, inputs))) = (path.segments.last_mut(), signature.split_last()) {
        segment.args = Some(Box::new(ast::GenericArgs::Parenthesized(ast::ParenthesizedArgs {
            span,
            inputs: inputs.iter().map(|ty| declarations::ty(ty, span)).collect(),
            inputs_span: span,
            output: ast::FnRetTy::Ty(declarations::ty(result, span)),
        })));
    }
    bound(path, span)
}

/// Represent the private object type without exposing its generated name to source selection.
fn object_ty(signature: &[PlanType], span: Span) -> Box<ast::Ty> {
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::TraitObject(
            vec![bound(path(&[&trait_name(signature)], span), span)],
            ast::TraitObjectSyntax::Dyn,
        ),
        span,
        tokens: None,
    })
}

/// Retain an ordinary trait bound without manufacturing any resolution or inferred type arguments.
fn bound(path: ast::Path, span: Span) -> ast::GenericBound {
    ast::GenericBound::Trait(ast::PolyTraitRef::new(
        ThinVec::new(),
        path,
        ast::TraitBoundModifiers::NONE,
        span,
        ast::Parens::No,
    ))
}

/// Resolve only the injected trait's object-safe clone slot, independently of source methods with the same spelling.
pub fn clone_method(tcx: TyCtxt<'_>, signature: &[PlanType]) -> Result<DefId, PlanError> {
    let name = trait_name(signature);
    let owner = tcx
        .hir_crate_items(())
        .free_items()
        .map(|item| item.owner_id.to_def_id())
        .find(|def| tcx.def_kind(*def) == DefKind::Trait && tcx.item_name(*def).as_str() == name)
        .ok_or_else(|| PlanError::UnknownCallee(name))?;
    tcx.associated_item_def_ids(owner)
        .iter()
        .copied()
        .find(|def| tcx.item_name(*def).as_str() == "__incan_clone_environment")
        .ok_or_else(|| PlanError::UnknownCallee("private callable clone slot".into()))
}

/// Check concrete capture clonability before rustc typechecks the blanket conversion, keeping gaps named refusals.
pub fn verify_captures(tcx: TyCtxt<'_>, plan: &Plan) -> Result<(), PlanError> {
    let clone = callees::clone_method(tcx)?;
    for function in &plan.functions {
        if closures::constructor(function).is_none() {
            continue;
        }
        for capture in &function.parameters {
            let ty = types::native_type(tcx, &capture.ty)?;
            callees::verify_bounds(tcx, clone, tcx.mk_args(&[ty.into()])).map_err(|_| PlanError::Unsupported {
                function: function.name.clone(),
                reason: "captured environment has a value without a native Clone implementation".into(),
            })?;
        }
    }
    Ok(())
}

/// Build one object-safe `fn method(&self) -> Box<dyn Trait>`, optionally with its structural native glue body.
fn method(name: &str, output: Box<ast::Ty>, body: Option<Box<ast::Expr>>, span: Span) -> Box<ast::AssocItem> {
    let receiver = ast::Param::from_self(
        ThinVec::new(),
        respan(span, ast::SelfKind::Region(None, ast::Mutability::Not)),
        Ident::new(kw::SelfLower, span),
    );
    Box::new(ast::AssocItem {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: inherited(span),
        tokens: None,
        kind: ast::AssocItemKind::Fn(Box::new(ast::Fn {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(name, span),
            generics: ast::Generics::default(),
            sig: ast::FnSig {
                header: ast::FnHeader::default(),
                decl: Box::new(ast::FnDecl {
                    inputs: thin_vec![receiver],
                    output: ast::FnRetTy::Ty(output),
                }),
                span,
            },
            contract: None,
            define_opaque: None,
            eii_impls: ThinVec::new(),
            body: body.map(|body| {
                Box::new(ast::Block {
                    stmts: thin_vec![ast::Stmt {
                        id: ast::DUMMY_NODE_ID,
                        kind: ast::StmtKind::Expr(body),
                        span
                    }],
                    id: ast::DUMMY_NODE_ID,
                    rules: ast::BlockCheckMode::Default,
                    span,
                    tokens: None,
                })
            }),
        })),
    })
}

/// Add a trait implementation with exactly its one generated ABI method.
fn implementation(
    path: ast::Path,
    self_ty: Box<ast::Ty>,
    generics: ast::Generics,
    method: Box<ast::AssocItem>,
    span: Span,
) -> Box<ast::Item> {
    item(
        ast::ItemKind::Impl(ast::Impl {
            generics,
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
            self_ty,
            items: thin_vec![method],
        }),
        span,
    )
}

/// Wrap an AST declaration using inherited visibility and ordinary node allocation.
fn item(kind: ast::ItemKind, span: Span) -> Box<ast::Item> {
    Box::new(ast::Item {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: inherited(span),
        kind,
        tokens: None,
    })
}

/// Use inherited visibility for private ABI declarations and trait implementation slots.
fn inherited(span: Span) -> ast::Visibility {
    ast::Visibility {
        kind: ast::VisibilityKind::Inherited,
        span,
        tokens: None,
    }
}

/// Allocate a controlled private identifier or an absolute standard path segment.
fn ident(name: &str, span: Span) -> Ident {
    Ident::new(Symbol::intern(name), span)
}

/// Root standard names absolutely; private ABI and generic names stay in the generated crate's namespace.
fn path(names: &[&str], span: Span) -> ast::Path {
    let mut segments = ThinVec::new();
    if names.first() == Some(&"std") {
        segments.push(ast::PathSegment::path_root(span));
    }
    for name in names {
        segments.push(ast::PathSegment::from_ident(ident(name, span)));
    }
    ast::Path {
        span,
        segments,
        tokens: None,
    }
}

/// Name a native type structurally without parsing source.
fn path_ty(path: ast::Path, span: Span) -> Box<ast::Ty> {
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, path),
        span,
        tokens: None,
    })
}

/// Wrap the object in the standard owned box, keeping MIR's builtin dereference representation.
fn boxed(ty: Box<ast::Ty>, span: Span) -> Box<ast::Ty> {
    let mut path = declarations::boxed_path(span);
    if let Some(segment) = path.segments.last_mut() {
        segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
            span,
            args: thin_vec![ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty))],
        })));
    }
    path_ty(path, span)
}

/// Allocate an AST expression for native cloning glue.
fn expression(kind: ast::ExprKind, span: Span) -> Box<ast::Expr> {
    Box::new(ast::Expr {
        id: ast::DUMMY_NODE_ID,
        kind,
        span,
        attrs: ThinVec::new(),
        tokens: None,
    })
}

/// Read the private glue method's receiver.
fn local(name: &str, span: Span) -> Box<ast::Expr> {
    expression(ast::ExprKind::Path(None, path(&[name], span)), span)
}

/// Invoke a concrete or dyn receiver method through rustc's ordinary resolution.
fn method_call(receiver: Box<ast::Expr>, name: &str, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::MethodCall(Box::new(ast::MethodCall {
            seg: ast::PathSegment::from_ident(ident(name, span)),
            receiver,
            args: ThinVec::new(),
            span,
        })),
        span,
    )
}

/// Construct the standard box constructor call with one cloned concrete environment.
fn call(path: ast::Path, args: ThinVec<Box<ast::Expr>>, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::Call(expression(ast::ExprKind::Path(None, path), span), args),
        span,
    )
}
