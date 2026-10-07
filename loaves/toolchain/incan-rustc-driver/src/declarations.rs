//! Declarations enter as AST items; only MIR supplies executable bodies.

use crate::plan::{Function, ListLeaf, PlanType};
use rustc_ast as ast;
use rustc_span::{Ident, Span, Symbol};
use thin_vec::{ThinVec, thin_vec};

/// Create an identifier from a name that preflight validation has already admitted.
fn ident(name: &str, span: Span) -> Ident {
    Ident::new(Symbol::intern(name), span)
}

/// Construct an admitted scalar, model, or list AST type without generating or parsing Rust source.
fn ty(kind: &PlanType, span: Span) -> Box<ast::Ty> {
    let kind = match kind {
        PlanType::List(leaf, depth) => {
            let mut element = ty(
                &match leaf {
                    ListLeaf::Int => PlanType::Int,
                    ListLeaf::Float => PlanType::Float,
                    ListLeaf::Bool => PlanType::Bool,
                    ListLeaf::Str => PlanType::String,
                },
                span,
            );
            for _ in 0..*depth {
                let mut path = ast::Path::from_ident(ident("Vec", span));
                path.segments[0].args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
                    span,
                    args: thin_vec![ast::AngleBracketedArg::Arg(ast::GenericArg::Type(element))],
                })));
                element = Box::new(ast::Ty {
                    id: ast::DUMMY_NODE_ID,
                    kind: ast::TyKind::Path(None, path),
                    span,
                    tokens: None,
                });
            }
            return element;
        }
        PlanType::ListRef(leaf, depth) | PlanType::ListMutRef(leaf, depth) => ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: ty(&PlanType::List(leaf.clone(), *depth), span),
                mutbl: if matches!(kind, PlanType::ListMutRef(_, _)) {
                    ast::Mutability::Mut
                } else {
                    ast::Mutability::Not
                },
            },
        ),
        PlanType::Unit => ast::TyKind::Tup(ThinVec::new()),
        PlanType::CheckedInt => ast::TyKind::Tup(thin_vec![ty(&PlanType::Int, span), ty(&PlanType::Bool, span)]),
        other => {
            let name = match other {
                PlanType::Int => "i64",
                PlanType::Float => "f64",
                PlanType::String => "String",
                PlanType::Model(_, name) => name.as_str(),
                _ => "bool",
            };
            ast::TyKind::Path(None, ast::Path::from_ident(ident(name, span)))
        }
    };
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind,
        span,
        tokens: None,
    })
}

/// A diverging placeholder satisfies every admitted signature; `mir_built` replaces its body.
fn placeholder(span: Span) -> Box<ast::Block> {
    let empty = Box::new(ast::Block {
        stmts: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        rules: ast::BlockCheckMode::Default,
        span,
        tokens: None,
    });
    let diverge = Box::new(ast::Expr {
        id: ast::DUMMY_NODE_ID,
        kind: ast::ExprKind::Loop(empty, None, span),
        span,
        attrs: ThinVec::new(),
        tokens: None,
    });
    Box::new(ast::Block {
        stmts: thin_vec![ast::Stmt {
            id: ast::DUMMY_NODE_ID,
            kind: ast::StmtKind::Expr(diverge),
            span
        }],
        id: ast::DUMMY_NODE_ID,
        rules: ast::BlockCheckMode::Default,
        span,
        tokens: None,
    })
}

/// Inject one source-named function's declaration with no authored body in the AST.
pub fn function(function: &Function, span: Span) -> Box<ast::Item> {
    let inputs = function
        .parameters
        .iter()
        .map(|parameter| ast::Param {
            attrs: ThinVec::new(),
            ty: ty(&parameter.ty, span),
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
    let output = ast::FnRetTy::Ty(ty(&function.return_type, span));
    let signature = ast::FnSig {
        header: ast::FnHeader::default(),
        decl: Box::new(ast::FnDecl { inputs, output }),
        span,
    };
    item(
        ast::ItemKind::Fn(Box::new(ast::Fn {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(&function.name, span),
            generics: ast::Generics::default(),
            sig: signature,
            contract: None,
            define_opaque: None,
            body: Some(placeholder(span)),
            eii_impls: ThinVec::new(),
        })),
        span,
    )
}

/// Build an item without source tokens; rustc assigns node identities after injection.
fn item(kind: ast::ItemKind, span: Span) -> Box<ast::Item> {
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

/// Load each admitted external crate so canonical path resolution sees its module tree.
pub fn external_crate(name: &str, span: Span) -> Box<ast::Item> {
    item(ast::ItemKind::ExternCrate(None, ident(name, span)), span)
}

/// Inject the exact checked field layout, preserving source visibility and declaration order.
pub fn model(model: &crate::plan::ModelDeclaration, span: Span) -> Box<ast::Item> {
    let fields = model
        .fields
        .iter()
        .zip(&model.field_public)
        .map(|(field, public)| ast::FieldDef {
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
            ident: Some(ident(&field.name, span)),
            ty: ty(&field.ty, span),
            default: None,
            is_placeholder: false,
        })
        .collect();
    let mut declaration = item(
        ast::ItemKind::Struct(
            ident(&model.name, span),
            ast::Generics::default(),
            ast::VariantData::Struct {
                fields,
                recovered: ast::Recovered::No,
            },
        ),
        span,
    );
    declaration.vis = visibility(model.public, span);
    declaration.tokens = Some(model_tokens(model, span));
    declaration
}

/// Retain the injected declaration's tokens for procedural derives, without parsing or generating source text.
fn model_tokens(model: &crate::plan::ModelDeclaration, span: Span) -> ast::tokenstream::LazyAttrTokenStream {
    use ast::token::{Delimiter, TokenKind};
    use ast::tokenstream::{AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, LazyAttrTokenStream, Spacing};
    let mut fields = Vec::new();
    for (field, public) in model.fields.iter().zip(&model.field_public) {
        if *public {
            fields.push(keyword_token("pub", span));
        }
        fields.push(name_token(&field.name, span));
        fields.push(AttrTokenTree::Token(
            ast::token::Token::new(TokenKind::Colon, span),
            Spacing::Alone,
        ));
        fields.extend(type_tokens(&ty(&field.ty, span)));
        fields.push(AttrTokenTree::Token(
            ast::token::Token::new(TokenKind::Comma, span),
            Spacing::Alone,
        ));
    }
    let mut tokens = Vec::new();
    if model.public {
        tokens.push(keyword_token("pub", span));
    }
    tokens.push(keyword_token("struct", span));
    tokens.push(name_token(&model.name, span));
    tokens.push(AttrTokenTree::Delimited(
        DelimSpan::from_single(span),
        DelimSpacing::new(Spacing::Alone, Spacing::Alone),
        Delimiter::Brace,
        AttrTokenStream::new(fields),
    ));
    LazyAttrTokenStream::new_direct(AttrTokenStream::new(tokens))
}

/// Turn an admitted AST identifier into a token with the same source span and hygiene.
fn name_token(name: &str, span: Span) -> ast::tokenstream::AttrTokenTree {
    ast::tokenstream::AttrTokenTree::Token(
        ast::token::Token::from_ast_ident(ident(name, span)),
        ast::tokenstream::Spacing::Alone,
    )
}

/// Emit declaration keywords as keywords rather than the raw identifiers inferred for reserved AST names.
fn keyword_token(name: &str, span: Span) -> ast::tokenstream::AttrTokenTree {
    ast::tokenstream::AttrTokenTree::Token(
        ast::token::Token::new(
            ast::token::TokenKind::Ident(Symbol::intern(name), ast::token::IdentIsRaw::No),
            span,
        ),
        ast::tokenstream::Spacing::Alone,
    )
}

/// Mirror the path and tuple types produced by `ty` into tokens consumed by procedural derives.
fn type_tokens(ty: &ast::Ty) -> Vec<ast::tokenstream::AttrTokenTree> {
    use ast::token::{Delimiter, Token, TokenKind};
    use ast::tokenstream::{AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, Spacing};
    match &ty.kind {
        ast::TyKind::Path(None, path) => path
            .segments
            .iter()
            .map(|segment| name_token(segment.ident.name.as_str(), ty.span))
            .collect(),
        ast::TyKind::Tup(elements) => {
            let mut tokens = Vec::new();
            for element in elements {
                tokens.extend(type_tokens(element));
                tokens.push(AttrTokenTree::Token(
                    Token::new(TokenKind::Comma, ty.span),
                    Spacing::Alone,
                ));
            }
            vec![AttrTokenTree::Delimited(
                DelimSpan::from_single(ty.span),
                DelimSpacing::new(Spacing::Alone, Spacing::Alone),
                Delimiter::Parenthesis,
                AttrTokenStream::new(tokens),
            )]
        }
        _ => unreachable!("the admitted AST type builder emits only paths and tuples"),
    }
}

/// Convert checked visibility without widening private fields.
fn visibility(public: bool, span: Span) -> ast::Visibility {
    ast::Visibility {
        kind: if public {
            ast::VisibilityKind::Public
        } else {
            ast::VisibilityKind::Inherited
        },
        span,
        tokens: None,
    }
}

/// Build derive path tokens directly from the admitted registry names, including compiler-owned proc macros.
pub fn derive_attribute(generator: &ast::attr::AttrIdGenerator, name: &str, span: Span) -> ast::Attribute {
    use ast::token::{Delimiter, Token, TokenKind};
    use ast::tokenstream::{
        AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, LazyAttrTokenStream, Spacing, TokenStream,
    };
    let mut tokens = Vec::new();
    if matches!(name, "FieldInfo" | "IncanClass") {
        tokens.push(AttrTokenTree::Token(
            Token::from_ast_ident(ident("incan_derive", span)),
            Spacing::Alone,
        ));
        tokens.push(AttrTokenTree::Token(
            Token::new(TokenKind::PathSep, span),
            Spacing::Alone,
        ));
    }
    tokens.push(AttrTokenTree::Token(
        Token::from_ast_ident(ident(name, span)),
        Spacing::Alone,
    ));
    let arguments = AttrTokenStream::new(tokens);
    let attribute_tokens = LazyAttrTokenStream::new_direct(AttrTokenStream::new(vec![
        AttrTokenTree::Token(Token::new(TokenKind::Pound, span), Spacing::JointHidden),
        AttrTokenTree::Delimited(
            DelimSpan::from_single(span),
            DelimSpacing::new(Spacing::JointHidden, Spacing::Alone),
            Delimiter::Bracket,
            AttrTokenStream::new(vec![
                name_token("derive", span),
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
            path: ast::Path::from_ident(ident("derive", span)),
            args: ast::AttrItemKind::Unparsed(ast::AttrArgs::Delimited(ast::DelimArgs {
                dspan: DelimSpan::from_single(span),
                delim: Delimiter::Parenthesis,
                tokens: TokenStream::new(arguments.to_token_trees()),
            })),
            tokens: None,
        },
        Some(attribute_tokens),
        ast::AttrStyle::Outer,
        span,
    )
}
