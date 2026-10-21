//! Generic ADT declarations from the native plan. The provider declares storage once; concrete plan layouts and
//! consumers alias its instantiations. A private zero-sized marker retains unused source type parameters.

use crate::ast_build::{
    block, expression, expression_statement, global_path, ident, inherited, local, path_ty, self_value, trait_bound,
};
use crate::plan::{GenericLayoutType, GenericModelDeclaration};
use crate::{declarations, union_debug};
use rustc_ast as ast;
use rustc_middle::ty::{self, Ty, TyCtxt};
use rustc_span::{Ident, Span, Symbol, kw, respan};
use thin_vec::{ThinVec, thin_vec};

/// Declare the generic ADT and its source-shaped Debug implementation, excluding its private marker from output.
pub(crate) fn items(
    model: &GenericModelDeclaration,
    generator: &ast::attr::AttrIdGenerator,
    span: Span,
) -> Vec<Box<ast::Item>> {
    let mut fields: ThinVec<_> = model
        .fields
        .iter()
        .map(|field| field_definition(&field.name, layout_type(&field.ty, span), field.public, span))
        .collect();
    let marker = marker_name(model);
    let parameters = model
        .parameters
        .iter()
        .map(|name| GenericLayoutType::Parameter(name.clone()))
        .collect();
    let marker_type = GenericLayoutType::Applied(
        "core::marker::PhantomData".into(),
        vec![GenericLayoutType::Tuple(parameters)],
    );
    fields.push(field_definition(&marker, layout_type(&marker_type, span), false, span));
    let mut declaration = declarations::item(
        ast::ItemKind::Struct(
            ident(&model.name, span),
            parameters_of(model, None, span),
            ast::VariantData::Struct {
                fields,
                recovered: ast::Recovered::No,
            },
        ),
        span,
    );
    for derive in model
        .derives
        .iter()
        .filter(|derive| !matches!(derive.as_str(), "Debug" | "Clone"))
    {
        declaration
            .attrs
            .push(declarations::derive_attribute(generator, derive, span));
    }
    let mut items = vec![declaration];
    if model.derives.iter().any(|derive| derive == "Debug") {
        items.push(debug_impl(model, span));
    }
    if model.derives.iter().any(|derive| derive == "Clone") {
        items.push(clone_impl(model, span));
    }
    items
}

/// Build one source field or the compiler-private marker without changing source field indices.
fn field_definition(name: &str, ty: Box<ast::Ty>, public: bool, span: Span) -> ast::FieldDef {
    ast::FieldDef {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: ast::Visibility {
            kind: if public {
                ast::VisibilityKind::Public
            } else {
                ast::VisibilityKind::Inherited
            },
            span,
            tokens: None,
        },
        mut_restriction: ast::MutRestriction {
            kind: ast::RestrictionKind::Unrestricted,
            span,
            tokens: None,
        },
        safety: ast::Safety::Default,
        ident: Some(ident(name, span)),
        ty,
        default: None,
        is_placeholder: false,
    }
}

/// Choose a private marker name outside the authored field set.
fn marker_name(model: &GenericModelDeclaration) -> String {
    let mut name = "__incan_phantom".to_owned();
    while model.fields.iter().any(|field| field.name == name) {
        name.push('_');
    }
    name
}

/// Preserve declared binders, bounding only parameters that occur in authored fields. The private marker must not
/// impose Clone or Debug requirements on otherwise unused type arguments.
fn parameters_of(model: &GenericModelDeclaration, bound: Option<&[&str]>, span: Span) -> ast::Generics {
    ast::Generics {
        params: model
            .parameters
            .iter()
            .map(|name| ast::GenericParam {
                id: ast::DUMMY_NODE_ID,
                ident: ident(name, span),
                attrs: ThinVec::new(),
                bounds: if let Some(bound) =
                    bound.filter(|_| model.fields.iter().any(|field| uses_parameter(&field.ty, name)))
                {
                    vec![trait_bound(global_path(bound, span), span)]
                } else {
                    Vec::new()
                },
                is_placeholder: false,
                kind: ast::GenericParamKind::Type { default: None },
                colon_span: None,
            })
            .collect(),
        where_clause: ast::WhereClause::default(),
        span,
    }
}

/// Whether an authored field carries a binder; concrete field carriers contain no open parameters.
fn uses_parameter(ty: &GenericLayoutType, parameter: &str) -> bool {
    match ty {
        GenericLayoutType::Parameter(name) => name == parameter,
        GenericLayoutType::Applied(_, arguments)
        | GenericLayoutType::Tuple(arguments)
        | GenericLayoutType::FunctionPointer(arguments) => {
            arguments.iter().any(|argument| uses_parameter(argument, parameter))
        }
        GenericLayoutType::Concrete(_) => false,
    }
}

/// Translate the plan's structural field type to AST while retaining binder names and nominal paths exactly.
fn layout_type(layout: &GenericLayoutType, span: Span) -> Box<ast::Ty> {
    match layout {
        GenericLayoutType::Concrete(ty) => declarations::ty(ty, span),
        GenericLayoutType::Parameter(name) => path_ty(ast::Path::from_ident(ident(name, span)), span),
        GenericLayoutType::Tuple(elements) => Box::new(ast::Ty {
            id: ast::DUMMY_NODE_ID,
            kind: ast::TyKind::Tup(elements.iter().map(|element| layout_type(element, span)).collect()),
            span,
            tokens: None,
        }),
        GenericLayoutType::FunctionPointer(signature) => {
            let Some((result, parameters)) = signature.split_last() else {
                unreachable!("preflight rejects empty generic function-pointer signatures");
            };
            crate::ast_build::function_pointer(
                parameters
                    .iter()
                    .map(|parameter| layout_type(parameter, span))
                    .collect(),
                layout_type(result, span),
                span,
            )
        }
        GenericLayoutType::Applied(name, arguments) => {
            let mut segments = name.split("::");
            let mut path = ast::Path::from_ident(ident(segments.next().unwrap_or_default(), span));
            for segment in segments {
                path.segments.push(ast::PathSegment::from_ident(ident(segment, span)));
            }
            if let Some(last) = path.segments.last_mut() {
                last.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
                    span,
                    args: arguments
                        .iter()
                        .map(|argument| ast::AngleBracketedArg::Arg(ast::GenericArg::Type(layout_type(argument, span))))
                        .collect(),
                })));
            }
            path_ty(path, span)
        }
    }
}

/// Format only authored fields under the source declaration name, keeping the marker and encoded ABI name private.
fn debug_impl(model: &GenericModelDeclaration, span: Span) -> Box<ast::Item> {
    let mut rendered = method_call(
        local("formatter", span),
        "debug_struct",
        thin_vec![text(&model.source_name, span)],
        span,
    );
    for field in &model.fields {
        let read = expression(ast::ExprKind::Field(self_value(span), ident(&field.name, span)), span);
        let borrowed = expression(
            ast::ExprKind::AddrOf(ast::BorrowKind::Ref, ast::Mutability::Not, read),
            span,
        );
        rendered = method_call(rendered, "field", thin_vec![text(&field.name, span), borrowed], span);
    }
    let body = block(
        thin_vec![expression_statement(
            method_call(rendered, "finish", ThinVec::new(), span),
            span
        )],
        span,
    );
    let method = trait_method("fmt", union_debug::fmt_signature(span), body, span);
    trait_impl(model, &["core", "fmt", "Debug"], method, span)
}

/// Clone authored fields and recreate the zero-sized marker without requiring unused parameters to implement Clone.
fn clone_impl(model: &GenericModelDeclaration, span: Span) -> Box<ast::Item> {
    let mut fields: ThinVec<_> = model
        .fields
        .iter()
        .map(|field| {
            let read = expression(ast::ExprKind::Field(self_value(span), ident(&field.name, span)), span);
            initialized_field(&field.name, method_call(read, "clone", ThinVec::new(), span), span)
        })
        .collect();
    fields.push(initialized_field(
        &marker_name(model),
        expression(
            ast::ExprKind::Path(None, global_path(&["core", "marker", "PhantomData"], span)),
            span,
        ),
        span,
    ));
    let value = expression(
        ast::ExprKind::Struct(Box::new(ast::StructExpr {
            qself: None,
            path: ast::Path::from_ident(Ident::new(kw::SelfUpper, span)),
            fields,
            rest: ast::StructRest::None,
        })),
        span,
    );
    let receiver = ast::Param::from_self(
        ThinVec::new(),
        respan(span, ast::SelfKind::Region(None, ast::Mutability::Not)),
        Ident::new(kw::SelfLower, span),
    );
    let signature = ast::FnSig {
        header: ast::FnHeader::default(),
        decl: Box::new(ast::FnDecl {
            inputs: thin_vec![receiver],
            output: ast::FnRetTy::Ty(path_ty(ast::Path::from_ident(Ident::new(kw::SelfUpper, span)), span)),
        }),
        span,
    };
    let body = block(thin_vec![expression_statement(value, span)], span);
    trait_impl(
        model,
        &["core", "clone", "Clone"],
        trait_method("clone", signature, body, span),
        span,
    )
}

/// Initialize one generic struct field with a fully constructed expression.
fn initialized_field(name: &str, expr: Box<ast::Expr>, span: Span) -> ast::ExprField {
    ast::ExprField {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        ident: ident(name, span),
        expr,
        is_shorthand: false,
        is_placeholder: false,
    }
}

/// Declare one method in a native trait implementation without authored source tokens.
fn trait_method(name: &str, sig: ast::FnSig, body: Box<ast::Block>, span: Span) -> Box<ast::AssocItem> {
    Box::new(ast::AssocItem {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: inherited(span),
        kind: ast::AssocItemKind::Fn(Box::new(ast::Fn {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(name, span),
            generics: ast::Generics::default(),
            sig,
            contract: None,
            define_opaque: None,
            body: Some(body),
            eii_impls: ThinVec::new(),
        })),
        tokens: None,
    })
}

/// Apply a standard trait to the provider-owned generic ADT, with requirements derived only from authored fields.
fn trait_impl(
    model: &GenericModelDeclaration,
    path: &[&str],
    method: Box<ast::AssocItem>,
    span: Span,
) -> Box<ast::Item> {
    let arguments = model
        .parameters
        .iter()
        .map(|name| GenericLayoutType::Parameter(name.clone()))
        .collect();
    let mut implementation = declarations::item(
        ast::ItemKind::Impl(ast::Impl {
            generics: parameters_of(model, Some(path), span),
            constness: ast::Const::No,
            of_trait: Some(Box::new(ast::TraitImplHeader {
                defaultness: ast::Defaultness::Implicit,
                safety: ast::Safety::Default,
                polarity: ast::ImplPolarity::Positive,
                trait_ref: ast::TraitRef {
                    path: global_path(path, span),
                    ref_id: ast::DUMMY_NODE_ID,
                },
            })),
            self_ty: layout_type(&GenericLayoutType::Applied(model.name.clone(), arguments), span),
            items: thin_vec![method],
        }),
        span,
    );
    implementation.vis = inherited(span);
    implementation
}

/// A source string literal used only by the injected Debug implementation.
fn text(value: &str, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::Lit(ast::token::Lit::new(
            ast::token::LitKind::Str,
            Symbol::intern(value),
            None,
        )),
        span,
    )
}

/// A native method call whose receiver and arguments already carry their source-independent AST structure.
fn method_call(receiver: Box<ast::Expr>, name: &str, args: ThinVec<Box<ast::Expr>>, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::MethodCall(Box::new(ast::MethodCall {
            seg: ast::PathSegment::from_ident(ident(name, span)),
            receiver,
            args,
            span,
        })),
        span,
    )
}

/// Admit only the provider's extra private PhantomData field at the end of its authored fields. Its tuple must name
/// exactly the ADT's type arguments; arbitrary extra zero-sized fields do not authorize a changed source layout.
pub(crate) fn marker_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    adt: ty::AdtDef<'tcx>,
    args: ty::GenericArgsRef<'tcx>,
    source_fields: usize,
) -> Option<Ty<'tcx>> {
    if !adt.is_struct() {
        return None;
    }
    let fields = &adt.non_enum_variant().fields;
    if fields.len() != source_fields + 1 || args.is_empty() {
        return None;
    }
    let field = fields.iter().next_back()?;
    if !field.name.as_str().starts_with("__incan_phantom") || tcx.visibility(field.did).is_public() {
        return None;
    }
    let ty = field.ty(tcx, args).skip_normalization();
    let ty::Adt(marker, parameters) = ty.kind() else {
        return None;
    };
    let tuple = Ty::new_tup(tcx, &args.types().collect::<Vec<_>>());
    (Some(marker.did()) == tcx.lang_items().phantom_data() && parameters.types().eq([tuple])).then_some(ty)
}
