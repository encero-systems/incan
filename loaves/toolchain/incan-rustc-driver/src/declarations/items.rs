//! Function, partition module, external crate and model items, and name and keyword tokens.

use super::*;

/// Inject one source-named function's declaration with no authored body in the AST. Future boxing constructors tie
/// their explicit reference inputs and boxed result to the same lifetime before rustc checks the native call; a
/// carried view's result exposes its payload loan to the retained lifetime contract.
pub fn function(
    plan: &crate::plan::Plan,
    function: &Function,
    span: Span,
) -> Result<Box<ast::Item>, crate::error::PlanError> {
    let inputs = function
        .parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| ast::Param {
            attrs: ThinVec::new(),
            ty: crate::closures::constructor_parameter_type(function, index, span),
            pat: Box::new(ast::Pat {
                id: ast::DUMMY_NODE_ID,
                kind: ast::PatKind::Ident(ast::BindingMode::NONE, ident(&parameter.name, span), None),
                span,
                tokens: None,
            }),
            id: ast::DUMMY_NODE_ID,
            span,
            is_placeholder: false,
        })
        .collect();
    let output = ast::FnRetTy::Ty(
        crate::closures::constructor_return_type(function, span)
            .or_else(|| crate::futures::constructor_return_type(function, span))
            .or_else(|| {
                crate::validation::carried_output(plan, function).map(|carrier| carried_output_ty(carrier, span))
            })
            .unwrap_or_else(|| ty(&function.return_type, span)),
    );
    let mut declaration = ast::FnDecl { inputs, output };
    crate::signature_lifetimes::prepare_model_lifetimes(plan, &mut declaration, span)?;
    let mut generics = crate::futures::constructor_generics(function, span);
    generics
        .params
        .extend(crate::closures::constructor_generics(function, span).params);
    generics
        .params
        .extend(crate::signature_lifetimes::apply(function, &mut declaration, span)?.params);
    let signature = ast::FnSig {
        header: ast::FnHeader {
            coroutine_kind: function.is_async.then_some(ast::CoroutineKind::Async {
                span,
                closure_id: ast::DUMMY_NODE_ID,
                return_impl_trait_id: ast::DUMMY_NODE_ID,
            }),
            ..ast::FnHeader::default()
        },
        decl: Box::new(declaration),
        span,
    };
    Ok(item(
        ast::ItemKind::Fn(Box::new(ast::Fn {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(&function.name, span),
            generics,
            sig: signature,
            contract: None,
            define_opaque: None,
            body: Some(
                crate::captured_generators::declaration_body(function, span)
                    .or_else(|| crate::closures::declaration_body(function, span))
                    .or_else(|| crate::shared_callables::declaration_body(function, span))
                    .or_else(|| crate::futures::declaration_body(function, span))
                    .unwrap_or_else(|| placeholder(span)),
            ),
            eii_impls: ThinVec::new(),
        })),
        span,
    ))
}

/// Build an item without source tokens; rustc assigns node identities after injection.
pub(crate) fn item(kind: ast::ItemKind, span: Span) -> Box<ast::Item> {
    Box::new(ast::Item {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: ast::Visibility {
            kind: ast::VisibilityKind::Public,
            span,
            tokens: None,
        },
        kind,
        tokens: None,
    })
}

/// Names a crate-root module that holds one partition of a unit's planned callables.
pub(crate) const PARTITION_MODULE_PREFIX: &str = "__incan_unit_";

/// One module holding a group of callables with every crate-root name in scope, re-exported at the crate root so
/// each callable keeps its root path. Returns the module and its root re-export.
pub(crate) fn partition_module(index: usize, items: Vec<Box<ast::Item>>, span: Span) -> [Box<ast::Item>; 2] {
    let name = format!("{PARTITION_MODULE_PREFIX}{index}");
    let name = name.as_str();
    let glob = |prefix: ast::Path| {
        ast::ItemKind::Use(ast::UseTree {
            prefix,
            kind: ast::UseTreeKind::Glob(span),
        })
    };
    // The module only imports the root's names; it re-exports nothing of its own beyond its callables.
    let root = rustc_span::Ident::new(rustc_span::symbol::kw::Super, span);
    let mut scope = item(glob(ast::Path::from_ident(root)), span);
    scope.vis.kind = ast::VisibilityKind::Inherited;
    let mut contents: ThinVec<Box<ast::Item>> = ThinVec::with_capacity(items.len() + 1);
    contents.push(scope);
    contents.extend(items);
    let module = item(
        ast::ItemKind::Mod(
            ast::Safety::Default,
            ident(name, span),
            ast::ModKind::Loaded(
                contents,
                ast::Inline::Yes,
                ast::ModSpans {
                    inner_span: span,
                    inject_use_span: span,
                },
            ),
        ),
        span,
    );
    [module, item(glob(ast::Path::from_ident(ident(name, span))), span)]
}

/// Load metadata without reserving a source type's name at the crate root. The original dependency alias remains
/// available in the extern prelude and on the anonymous HIR declaration for exact native crate resolution.
pub fn external_crate(name: &str, span: Span) -> Box<ast::Item> {
    item(
        ast::ItemKind::ExternCrate(Some(Symbol::intern(name)), ident("_", span)),
        span,
    )
}

/// Name the private namespace containing one source-named concrete layout; preflight checks its root collision.
pub(crate) fn model_module_name(model: &crate::plan::ModelDeclaration) -> String {
    format!("__incan_layout_{}", model.name)
}

/// Keep a concrete struct's source name for derives, exposing its unique physical symbol through a type alias.
/// Ordinary layouts stay at the crate root. Procedural derives operate on a source-named struct in a private namespace
/// for specialized layouts, and MIR uses the alias's identical underlying ADT. Structural Debug and Display are
/// published on that same type, including in compiled libraries.
pub(crate) fn model_items(
    plan: &crate::plan::Plan,
    model: &crate::plan::ModelDeclaration,
    generator: &ast::attr::AttrIdGenerator,
    span: Span,
) -> Result<Vec<Box<ast::Item>>, crate::error::PlanError> {
    if !model.library_item.is_empty() {
        return Ok(vec![library_alias(&model.name, model.public, span)]);
    }
    let mut declaration = self::model(plan, model, span)?;
    let tuple = model.fields.len() == 1 && model.fields[0].name == "0";
    let structural_debug = !tuple
        && model
            .derives
            .iter()
            .any(|derive| derives::from_str(derive) == Some(DeriveId::Debug));
    let stores_exclusive_view = model.fields.iter().any(|field| stores_exclusive_view(&field.ty));
    for derive in model.derives.iter().filter(|derive| {
        derives::from_str(derive) != Some(DeriveId::Display)
            && !(structural_debug && derives::from_str(derive) == Some(DeriveId::Debug))
            && !(stores_exclusive_view && derives::from_str(derive) == Some(DeriveId::Clone))
    }) {
        declaration.attrs.push(derive_attribute(generator, derive, span));
    }
    if !model.checked_deserialize.is_empty() {
        declaration
            .attrs
            .push(crate::checked_json::deserialize_attribute(generator, span));
    }
    let lifetime_count = usize::try_from(model.view_lifetime_count).map_err(|_| crate::error::PlanError::Invalid {
        function: model.name.clone(),
        reason: "model lifetime count exceeds the native index range".into(),
    })?;
    let display = model
        .derives
        .iter()
        .any(|derive| derives::from_str(derive) == Some(DeriveId::Display))
        .then(|| {
            crate::derived_display::implementation(
                model_path_type(&model.name, lifetime_count, span),
                crate::signature_lifetimes::model_lifetime_generics(lifetime_count, span),
                span,
            )
        });
    if !specialized_model(model) {
        let debug = structural_debug.then(|| {
            crate::model_debug::implementation(
                model_source_name(model),
                model.fields.iter().map(|field| field.name.as_str()),
                model_path_type(&model.name, lifetime_count, span),
                crate::signature_lifetimes::model_lifetime_generics(lifetime_count, span),
                span,
            )
        });
        return Ok(std::iter::once(declaration).chain(debug).chain(display).collect());
    }
    let namespace = model_module_name(model);
    let mut contents = thin_vec![declaration];
    if structural_debug {
        contents.push(crate::model_debug::implementation(
            model_source_name(model),
            model.fields.iter().map(|field| field.name.as_str()),
            model_path_type(model_source_name(model), lifetime_count, span),
            crate::signature_lifetimes::model_lifetime_generics(lifetime_count, span),
            span,
        ));
    }
    let module = item(
        ast::ItemKind::Mod(
            ast::Safety::Default,
            ident(&namespace, span),
            ast::ModKind::Loaded(
                contents,
                ast::Inline::Yes,
                ast::ModSpans {
                    inner_span: span,
                    inject_use_span: span,
                },
            ),
        ),
        span,
    );
    let mut path = ast::Path::from_ident(ident(&namespace, span));
    path.segments
        .push(ast::PathSegment::from_ident(ident(&model.source_name, span)));
    apply_model_lifetime_arguments(&mut path, lifetime_count, span);
    let mut alias = item(
        ast::ItemKind::TyAlias(Box::new(ast::TyAlias {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(&model.name, span),
            generics: crate::signature_lifetimes::model_lifetime_generics(lifetime_count, span),
            after_where_clause: ast::WhereClause::default(),
            bounds: Vec::new(),
            ty: Some(Box::new(ast::Ty {
                id: ast::DUMMY_NODE_ID,
                kind: ast::TyKind::Path(None, path),
                span,
                tokens: None,
            })),
        })),
        span,
    );
    alias.vis = visibility(model.public, span);
    Ok(vec![module, alias].into_iter().chain(display).collect())
}

/// Report whether a concrete field layout retains an exclusive reference that cannot satisfy Rust's `Clone`
/// contract. Callable signatures bind their own inputs and are deliberately not traversed.
fn stores_exclusive_view(ty: &PlanType) -> bool {
    match ty {
        PlanType::Tuple(elements) => elements
            .iter()
            .any(|element| stores_exclusive_view(&tuple_element_type(element.clone()))),
        PlanType::NativeArray(elements, _) | PlanType::Boxed(elements) => elements.iter().any(stores_exclusive_view),
        PlanType::TupleMutRef(_)
        | PlanType::ScalarMutRef(_)
        | PlanType::StringMutRef
        | PlanType::EnumMutRef(..)
        | PlanType::ModelMutRef(..)
        | PlanType::ListMutRef(..)
        | PlanType::ListIntoIterMutRef(..)
        | PlanType::SetMutRef(..)
        | PlanType::DictMutRef(..)
        | PlanType::ZipIteratorMutRef(..)
        | PlanType::GeneratorMutRef(..)
        | PlanType::SliceMutRef(..)
        | PlanType::NativeArrayMutRef(..) => true,
        _ => false,
    }
}

/// Build one model path with its declaration-owned lifetime arguments in stable field order.
fn model_path_type(name: &str, count: usize, span: Span) -> Box<ast::Ty> {
    let mut path = ast::Path::from_ident(ident(name, span));
    apply_model_lifetime_arguments(&mut path, count, span);
    crate::ast_build::path_ty(path, span)
}

/// Apply the surrounding model's lifetime binders to a specialized declaration or alias path.
fn apply_model_lifetime_arguments(path: &mut ast::Path, count: usize, span: Span) {
    let Some(segment) = path.segments.last_mut() else {
        return;
    };
    if count == 0 {
        return;
    }
    segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
        span,
        args: (0..count)
            .map(|index| {
                ast::AngleBracketedArg::Arg(ast::GenericArg::Lifetime(ast::Lifetime {
                    id: ast::DUMMY_NODE_ID,
                    ident: ident(&format!("'incan_field_{index}"), span),
                }))
            })
            .collect(),
    })));
}

/// Declare a native layout's alias shell. The adapter supplies its exact metadata type through `type_of`, including
/// erased Rust lifetime arguments. Reconstructing its target in AST would create a second generic binding contract
/// and require source lifetime binders that are deliberately absent from Body IR.
pub(super) fn library_alias(name: &str, public: bool, span: Span) -> Box<ast::Item> {
    let mut alias = item(
        ast::ItemKind::TyAlias(Box::new(ast::TyAlias {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(name, span),
            generics: ast::Generics::default(),
            after_where_clause: ast::WhereClause::default(),
            bounds: Vec::new(),
            ty: Some(Box::new(ast::Ty {
                id: ast::DUMMY_NODE_ID,
                kind: ast::TyKind::Tup(ThinVec::new()),
                span,
                tokens: None,
            })),
        })),
        span,
    );
    alias.vis = visibility(public, span);
    alias
}

/// Distinguish specialized layouts from legacy plan records whose source name defaults to their physical name.
pub(super) fn specialized_model(model: &crate::plan::ModelDeclaration) -> bool {
    !model.source_name.is_empty() && model.source_name != model.name
}

/// Choose the validated source identifier only when a distinct concrete layout alias is retained.
pub(super) fn model_source_name(model: &crate::plan::ModelDeclaration) -> &str {
    if specialized_model(model) {
        &model.source_name
    } else {
        &model.name
    }
}

/// Inject the checked layout and field order; a specialized struct is public inside its private namespace.
pub fn model(
    plan: &crate::plan::Plan,
    model: &crate::plan::ModelDeclaration,
    span: Span,
) -> Result<Box<ast::Item>, crate::error::PlanError> {
    let tuple = model.fields.len() == 1 && model.fields[0].name == "0";
    let mut field_types = model.fields.iter().map(|field| ty(&field.ty, span)).collect::<Vec<_>>();
    let generics = crate::signature_lifetimes::apply_model_field_lifetimes(
        plan,
        &mut field_types,
        model.view_lifetime_count,
        span,
    )
    .map_err(|reason| crate::error::PlanError::Invalid {
        function: model.name.clone(),
        reason: reason.into(),
    })?;
    let token_types = field_types.clone();
    let fields = model
        .fields
        .iter()
        .zip(&model.field_public)
        .zip(field_types)
        .map(|((field, public), ty)| ast::FieldDef {
            attrs: ThinVec::new(),
            id: ast::DUMMY_NODE_ID,
            span,
            vis: visibility(*public, span),
            mut_restriction: ast::MutRestriction {
                kind: ast::RestrictionKind::Unrestricted,
                span,
                tokens: None,
            },
            safety: ast::Safety::Default,
            ident: if tuple { None } else { Some(ident(&field.name, span)) },
            ty,
            default: None,
            is_placeholder: false,
        })
        .collect();
    let mut declaration = item(
        ast::ItemKind::Struct(
            ident(model_source_name(model), span),
            generics.clone(),
            if tuple {
                ast::VariantData::Tuple(fields, ast::DUMMY_NODE_ID)
            } else {
                ast::VariantData::Struct {
                    fields,
                    recovered: ast::Recovered::No,
                }
            },
        ),
        span,
    );
    declaration.vis = visibility(model.public || specialized_model(model), span);
    declaration.tokens = Some(model_tokens(model, &token_types, &generics, span));
    Ok(declaration)
}

/// Retain the injected declaration's tokens for procedural derives, without parsing or generating source text.
pub(super) fn model_tokens(
    model: &crate::plan::ModelDeclaration,
    field_types: &[Box<ast::Ty>],
    generics: &ast::Generics,
    span: Span,
) -> ast::tokenstream::LazyAttrTokenStream {
    use ast::token::{Delimiter, TokenKind};
    use ast::tokenstream::{AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, LazyAttrTokenStream, Spacing};
    let mut fields = Vec::new();
    let tuple = model.fields.len() == 1 && model.fields[0].name == "0";
    for (slot, ((field, public), ty)) in model
        .fields
        .iter()
        .zip(&model.field_public)
        .zip(field_types)
        .enumerate()
    {
        if let Some(serde_name) = model.field_serde_names.get(slot).filter(|name| !name.is_empty()) {
            fields.extend(serde_rename_tokens(serde_name, span));
        }
        if *public {
            fields.push(keyword_token("pub", span));
        }
        if !tuple {
            fields.push(name_token(&field.name, span));
            fields.push(AttrTokenTree::Token(
                ast::token::Token::new(TokenKind::Colon, span),
                Spacing::Alone,
            ));
        }
        fields.extend(type_tokens(ty));
        fields.push(AttrTokenTree::Token(
            ast::token::Token::new(TokenKind::Comma, span),
            Spacing::Alone,
        ));
    }
    let mut tokens = Vec::new();
    if model.public || specialized_model(model) {
        tokens.push(keyword_token("pub", span));
    }
    tokens.push(keyword_token("struct", span));
    tokens.push(name_token(model_source_name(model), span));
    if !generics.params.is_empty() {
        tokens.push(AttrTokenTree::Token(
            ast::token::Token::new(TokenKind::Lt, span),
            Spacing::Alone,
        ));
        for parameter in &generics.params {
            tokens.push(AttrTokenTree::Token(
                ast::token::Token::new(
                    TokenKind::Lifetime(parameter.ident.name, ast::token::IdentIsRaw::No),
                    span,
                ),
                Spacing::Alone,
            ));
            tokens.push(AttrTokenTree::Token(
                ast::token::Token::new(TokenKind::Comma, span),
                Spacing::Alone,
            ));
        }
        tokens.push(AttrTokenTree::Token(
            ast::token::Token::new(TokenKind::Gt, span),
            Spacing::Alone,
        ));
    }
    tokens.push(AttrTokenTree::Delimited(
        DelimSpan::from_single(span),
        DelimSpacing::new(Spacing::Alone, Spacing::Alone),
        if tuple {
            Delimiter::Parenthesis
        } else {
            Delimiter::Brace
        },
        AttrTokenStream::new(fields),
    ));
    if tuple {
        tokens.push(AttrTokenTree::Token(
            ast::token::Token::new(TokenKind::Semi, span),
            Spacing::Alone,
        ));
    }
    LazyAttrTokenStream::new_direct(AttrTokenStream::new(tokens))
}

/// `#[serde(rename = "name")]` before a field in the derive input, as legacy writes it for an aliased field.
///
/// The attribute exists only for serde's derive, which reads it as its helper; the injected AST field needs none.
pub(crate) fn serde_rename_tokens(name: &str, span: Span) -> Vec<ast::tokenstream::AttrTokenTree> {
    use ast::token::{Delimiter, Lit, LitKind, Token, TokenKind};
    use ast::tokenstream::{AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, Spacing};
    let literal = Lit::new(LitKind::Str, Symbol::intern(&name.escape_default().to_string()), None);
    let arguments = AttrTokenStream::new(vec![
        name_token("rename", span),
        AttrTokenTree::Token(Token::new(TokenKind::Eq, span), Spacing::Alone),
        AttrTokenTree::Token(Token::new(TokenKind::Literal(literal), span), Spacing::Alone),
    ]);
    vec![
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
                    arguments,
                ),
            ]),
        ),
    ]
}

/// Turn an admitted AST identifier into a token with the same source span and hygiene.
pub(crate) fn name_token(name: &str, span: Span) -> ast::tokenstream::AttrTokenTree {
    ast::tokenstream::AttrTokenTree::Token(
        ast::token::Token::from_ast_ident(ident(name, span)),
        ast::tokenstream::Spacing::Alone,
    )
}

/// Emit declaration keywords as keywords rather than the raw identifiers inferred for reserved AST names.
pub(crate) fn keyword_token(name: &str, span: Span) -> ast::tokenstream::AttrTokenTree {
    ast::tokenstream::AttrTokenTree::Token(
        ast::token::Token::new(
            ast::token::TokenKind::Ident(Symbol::intern(name), ast::token::IdentIsRaw::No),
            span,
        ),
        ast::tokenstream::Spacing::Alone,
    )
}
