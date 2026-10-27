//! Validate planned Incan string ingress roots; serde's native TryFrom header forwards to its reconstruction root.

use crate::{
    ast_build::ident,
    declarations,
    error::PlanError,
    plan::{ModelDeclaration, Plan, PlanType},
};
use incan_semantics_core::{SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin, decode_incan_symbol_identity};
use rustc_ast as ast;
use rustc_span::{Span, Symbol};
use thin_vec::{ThinVec, thin_vec};

/// Preserve serde's absolute String decoder selection in both AST attributes and their retained derive tokens.
pub(crate) fn deserialize_attribute(generator: &ast::attr::AttrIdGenerator, span: Span) -> ast::Attribute {
    use ast::token::{Delimiter, Lit, LitKind, Token, TokenKind};
    use ast::tokenstream::{
        AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, LazyAttrTokenStream, Spacing, TokenStream,
    };
    let arguments = AttrTokenStream::new(vec![
        name_token("try_from", span),
        AttrTokenTree::Token(Token::new(TokenKind::Eq, span), Spacing::Alone),
        AttrTokenTree::Token(
            Token::new(
                TokenKind::Literal(Lit::new(LitKind::Str, Symbol::intern("::std::string::String"), None)),
                span,
            ),
            Spacing::Alone,
        ),
    ]);
    let tokens = LazyAttrTokenStream::new_direct(AttrTokenStream::new(vec![
        AttrTokenTree::Token(Token::new(TokenKind::Pound, span), Spacing::JointHidden),
        AttrTokenTree::Delimited(
            DelimSpan::from_single(span),
            DelimSpacing::new(Spacing::JointHidden, Spacing::Alone),
            Delimiter::Bracket,
            AttrTokenStream::new(vec![
                name_token("serde", span),
                AttrTokenTree::Delimited(
                    DelimSpan::from_single(span),
                    DelimSpacing::new(Spacing::Alone, Spacing::Alone),
                    Delimiter::Parenthesis,
                    arguments.clone(),
                ),
            ]),
        ),
    ]));
    ast::attr::mk_attr_from_item(
        generator,
        ast::AttrItem {
            unsafety: ast::Safety::Default,
            path: ast::Path::from_ident(ident("serde", span)),
            args: ast::AttrItemKind::Unparsed(ast::AttrArgs::Delimited(ast::DelimArgs {
                dspan: DelimSpan::from_single(span),
                delim: Delimiter::Parenthesis,
                tokens: TokenStream::new(arguments.to_token_trees()),
            })),
            tokens: None,
        },
        Some(tokens),
        ast::AttrStyle::Outer,
        span,
    )
}

/// Keep one helper attribute identifier's source hygiene without generating source text.
fn name_token(name: &str, span: Span) -> ast::tokenstream::AttrTokenTree {
    ast::tokenstream::AttrTokenTree::Token(
        ast::token::Token::from_ast_ident(ident(name, span)),
        ast::tokenstream::Spacing::Alone,
    )
}

/// Require a unique owned-string root and the exact standard Result carrying this model before injecting a header.
pub(crate) fn validate_roots(plan: &Plan) -> Result<(), PlanError> {
    for (index, model) in plan
        .models
        .iter()
        .enumerate()
        .filter(|(_, model)| !model.checked_deserialize.is_empty())
    {
        let reason = "unsupported checked Deserialize forwarding contract";
        if !matches!(model.fields.as_slice(), [field] if field.ty == PlanType::String)
            || !model.derives.iter().any(|derive| derive == "serde::Deserialize")
        {
            return Err(PlanError::Invalid {
                function: model.name.clone(),
                reason: reason.into(),
            });
        }
        validate_string_root(plan, index, model, &model.checked_deserialize, reason)?;
    }
    Ok(())
}

/// Source conversion authenticates its checked SDK trait and planned root. Source trait calls already dispatch to
/// that Incan root; this contract grants no Rust trait header, unlike serde's native String ingress.
pub(crate) fn validate_source_roots(plan: &Plan) -> Result<(), PlanError> {
    for (index, model) in plan.models.iter().enumerate() {
        if model.string_conversion_root.is_empty() && model.string_conversion_trait.is_empty() {
            continue;
        }
        let reason = "unsupported source string conversion forwarding contract";
        if !is_source_conversion_trait(&model.string_conversion_trait)
            || !model.string_conversion_root.starts_with("__incan_string_try_from_")
            || !matches!(model.fields.as_slice(), [field] if matches!(field.ty, PlanType::Int | PlanType::String | PlanType::Model(_, _)))
        {
            return Err(PlanError::Invalid {
                function: model.name.clone(),
                reason: reason.into(),
            });
        }
        validate_string_root(plan, index, model, &model.string_conversion_root, reason)?;
    }
    Ok(())
}

/// Authenticate the encoded identity of the selected source conversion trait. Its decoded owner and crate must
/// agree; an old module spelling, equal-named user trait, or malformed identity cannot authorize a conversion root.
fn is_source_conversion_trait(path: &str) -> bool {
    let Some((_, item)) = path.split_once("::") else {
        return false;
    };
    let Ok(Some(identity)) = decode_incan_symbol_identity(item) else {
        return false;
    };
    identity.kind == SemanticSourceTargetKind::Trait
        && identity.namespace == SymbolNamespace::OrdinaryLexical
        && identity.scope_discriminant.is_none()
        && identity.declaration_name == "TryFrom"
        && matches!(&identity.origin, SymbolOrigin::Package { library, module_path }
            if library == "incan_stdlib_core" && module_path == &["traits", "convert"])
        && incan_semantics_core::body_ir::loaf_item_path(&identity).as_deref() == Some(path)
}

/// Share only the owned input and standard Result geometry; each header policy validates its own selection first.
fn validate_string_root(
    plan: &Plan,
    index: usize,
    model: &ModelDeclaration,
    name: &str,
    reason: &str,
) -> Result<(), PlanError> {
    let invalid = || PlanError::Invalid {
        function: model.name.clone(),
        reason: reason.into(),
    };
    let owner = i64::try_from(index).map_err(|_| invalid())?;
    let mut roots = plan.functions.iter().filter(|function| function.name == name);
    let root = roots.next().ok_or_else(invalid)?;
    if roots.next().is_some()
        || !model.library_item.is_empty()
        || model.fields.len() != 1
        || model.fields[0].name != "0"
        || model.field_public != [true]
        || root.parameters.len() != 1
        || root.parameters[0].ty != PlanType::String
        || root.locals.len() < 2
        || root.locals[0].ty != root.return_type
        || root.locals[1].ty != PlanType::String
    {
        return Err(invalid());
    }
    let PlanType::Enum(result_index, result_name) = &root.return_type else {
        return Err(invalid());
    };
    let result = usize::try_from(*result_index)
        .ok()
        .and_then(|index| plan.enums.get(index))
        .ok_or_else(invalid)?;
    if result.name != *result_name
        || result.carrier != "Result"
        || result.variants.len() != 2
        || result.variants[0].name != "Ok"
        || result.variants[0].fields != [PlanType::Model(owner, model.name.clone())]
        || result.variants[1].name != "Err"
        || result.variants[1].fields != [PlanType::String]
    {
        return Err(invalid());
    }
    Ok(())
}

/// Declare std TryFrom<String> with one forwarding call; validation and error rendering remain in the planned root.
pub(crate) fn forwarding_impl(model: &ModelDeclaration, span: Span) -> Box<ast::Item> {
    let mut trait_path = global_path(["std", "convert", "TryFrom"], span);
    if let Some(segment) = trait_path.segments.last_mut() {
        segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
            span,
            args: thin_vec![ast::AngleBracketedArg::Arg(ast::GenericArg::Type(declarations::ty(
                &PlanType::String,
                span
            )))],
        })));
    }
    let items = thin_vec![
        error_association(span),
        forwarding_method(model, &model.checked_deserialize, span)
    ];
    Box::new(ast::Item {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: inherited(span),
        kind: ast::ItemKind::Impl(ast::Impl {
            generics: ast::Generics::default(),
            constness: ast::Const::No,
            of_trait: Some(Box::new(ast::TraitImplHeader {
                defaultness: ast::Defaultness::Implicit,
                safety: ast::Safety::Default,
                polarity: ast::ImplPolarity::Positive,
                trait_ref: ast::TraitRef {
                    path: trait_path,
                    ref_id: ast::DUMMY_NODE_ID,
                },
            })),
            self_ty: named_ty(ast::Path::from_ident(ident("crate", span)), &model.name, span),
            items,
        }),
        tokens: None,
    })
}

/// Retain the string error of serde's native TryFrom ingress.
fn error_association(span: Span) -> Box<ast::AssocItem> {
    Box::new(ast::AssocItem {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: inherited(span),
        kind: ast::AssocItemKind::Type(Box::new(ast::TyAlias {
            defaultness: ast::Defaultness::Implicit,
            ident: ident("Error", span),
            generics: ast::Generics::default(),
            after_where_clause: ast::WhereClause::default(),
            bounds: Vec::new(),
            ty: Some(declarations::ty(&PlanType::String, span)),
        })),
        tokens: None,
    })
}

/// Match the validated owned-string ABI and forward to the retained crate-root function without reconstruction logic.
fn forwarding_method(model: &ModelDeclaration, root: &str, span: Span) -> Box<ast::AssocItem> {
    let argument = expression(
        ast::ExprKind::Path(None, ast::Path::from_ident(ident("value", span))),
        span,
    );
    let mut path = ast::Path::from_ident(ident("crate", span));
    path.segments.push(ast::PathSegment::from_ident(ident(root, span)));
    let target = expression(ast::ExprKind::Path(None, path), span);
    let call = expression(ast::ExprKind::Call(target, thin_vec![argument]), span);
    Box::new(ast::AssocItem {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: inherited(span),
        kind: ast::AssocItemKind::Fn(Box::new(ast::Fn {
            defaultness: ast::Defaultness::Implicit,
            ident: ident("try_from", span),
            generics: ast::Generics::default(),
            sig: ast::FnSig {
                header: ast::FnHeader::default(),
                decl: Box::new(ast::FnDecl {
                    inputs: thin_vec![ast::Param {
                        attrs: ThinVec::new(),
                        ty: declarations::ty(&PlanType::String, span),
                        pat: Box::new(ast::Pat {
                            id: ast::DUMMY_NODE_ID,
                            kind: ast::PatKind::Ident(ast::BindingMode::NONE, ident("value", span), None),
                            span,
                            tokens: None,
                        }),
                        id: ast::DUMMY_NODE_ID,
                        span,
                        is_placeholder: false,
                    }],
                    output: ast::FnRetTy::Ty(result_ty(model, span)),
                }),
                span,
            },
            contract: None,
            define_opaque: None,
            body: Some(Box::new(ast::Block {
                stmts: thin_vec![ast::Stmt {
                    id: ast::DUMMY_NODE_ID,
                    kind: ast::StmtKind::Expr(call),
                    span
                }],
                id: ast::DUMMY_NODE_ID,
                rules: ast::BlockCheckMode::Default,
                span,
                tokens: None,
            })),
            eii_impls: ThinVec::new(),
        })),
        tokens: None,
    })
}

/// Spell the standard Result ABI already proved against the root, preserving absolute std identity for both carriers.
fn result_ty(model: &ModelDeclaration, span: Span) -> Box<ast::Ty> {
    let mut path = global_path(["std", "result", "Result"], span);
    if let Some(segment) = path.segments.last_mut() {
        segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
            span,
            args: thin_vec![
                ast::AngleBracketedArg::Arg(ast::GenericArg::Type(named_ty(
                    ast::Path::from_ident(ident("crate", span)),
                    &model.name,
                    span
                ))),
                ast::AngleBracketedArg::Arg(ast::GenericArg::Type(declarations::ty(&PlanType::String, span))),
            ],
        })));
    }
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, path),
        span,
        tokens: None,
    })
}

/// Build an absolute trait path without parsing generated source or consulting a caller's lexical aliases.
fn global_path<'segment>(segments: impl IntoIterator<Item = &'segment str>, span: Span) -> ast::Path {
    let mut path = ast::Path {
        span,
        segments: thin_vec![ast::PathSegment::path_root(span)],
        tokens: None,
    };
    for segment in segments {
        path.segments.push(ast::PathSegment::from_ident(ident(segment, span)));
    }
    path
}

/// Append a validated nominal name to its crate-root path.
fn named_ty(mut path: ast::Path, name: &str, span: Span) -> Box<ast::Ty> {
    path.segments.push(ast::PathSegment::from_ident(ident(name, span)));
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, path),
        span,
        tokens: None,
    })
}

/// Preserve inherited visibility for trait items.
fn inherited(span: Span) -> ast::Visibility {
    ast::Visibility {
        kind: ast::VisibilityKind::Inherited,
        span,
        tokens: None,
    }
}

/// Keep forwarding expressions on the retained source span.
fn expression(kind: ast::ExprKind, span: Span) -> Box<ast::Expr> {
    Box::new(ast::Expr {
        id: ast::DUMMY_NODE_ID,
        kind,
        span,
        attrs: ThinVec::new(),
        tokens: None,
    })
}
