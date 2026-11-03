//! Provider-owned generic enum AST declarations. Closed plans retain aliases, never replacement enum storage.

use crate::ast_build::{
    RUST_CLONE_SYMBOL, RUST_FMT_DEBUG_SYMBOL, binding, block, expression, expression_statement, global_path, ident,
    inherited, local, self_value,
};
use crate::declarations;
use crate::generic_layouts::{
    applied_type, clone_signature, layout_type, method_call, parameters, text, trait_implementation, trait_method,
};
use crate::plan::{GenericEnumDeclaration, GenericEnumVariant};
use incan_lang::lang::derives::{self, DeriveId};
use rustc_ast as ast;
use rustc_span::{Ident, Span, kw};
use thin_vec::{ThinVec, thin_vec};

/// Emit the provider template and storage-sensitive automatic derives. Function pointer payloads do not require
/// their argument or result types to implement Clone or Debug.
pub(crate) fn items(
    value: &GenericEnumDeclaration,
    generator: &ast::attr::AttrIdGenerator,
    span: Span,
) -> Vec<Box<ast::Item>> {
    let mut items = vec![declaration(value, generator, span)];
    for name in &value.derives {
        match derives::from_str(name) {
            Some(DeriveId::Clone) => items.push(implementation(
                value,
                &["core", "clone", RUST_CLONE_SYMBOL],
                "clone",
                clone_signature(span),
                value.variants.iter().map(|variant| clone_arm(variant, span)).collect(),
                span,
            )),
            Some(DeriveId::Debug) => items.push(implementation(
                value,
                &["core", "fmt", RUST_FMT_DEBUG_SYMBOL],
                "fmt",
                crate::union_debug::fmt_signature(span),
                value.variants.iter().map(|variant| debug_arm(variant, span)).collect(),
                span,
            )),
            _ => {}
        }
    }
    items
}

/// Bind a checked native trait only through actual payload storage and dispatch over the exact authored variants.
fn implementation(
    value: &GenericEnumDeclaration,
    path: &[&str],
    name: &str,
    signature: ast::FnSig,
    arms: ThinVec<ast::Arm>,
    span: Span,
) -> Box<ast::Item> {
    let fields = value
        .variants
        .iter()
        .flat_map(|variant| variant.fields.iter())
        .collect::<Vec<_>>();
    let matched = expression(
        ast::ExprKind::Match(self_value(span), arms, ast::MatchKind::Prefix),
        span,
    );
    let method = trait_method(
        name,
        signature,
        block(thin_vec![expression_statement(matched, span)], span),
        span,
    );
    trait_implementation(
        applied_type(&value.name, &value.parameters, span),
        parameters(&value.parameters, &fields, Some(path), span),
        path,
        thin_vec![method],
        span,
    )
}

/// Clone variant payloads through Clone's borrowed input; unit variants reconstruct their existing source case.
fn clone_arm(variant: &GenericEnumVariant, span: Span) -> ast::Arm {
    let constructor = expression(ast::ExprKind::Path(None, variant_path(&variant.name, span)), span);
    let value = if variant.fields.is_empty() {
        constructor
    } else {
        let fields = variant
            .fields
            .iter()
            .enumerate()
            .map(|(index, _)| {
                expression(
                    ast::ExprKind::Call(
                        expression(
                            ast::ExprKind::Path(
                                None,
                                global_path(&["core", "clone", RUST_CLONE_SYMBOL, "clone"], span),
                            ),
                            span,
                        ),
                        thin_vec![local(&format!("field_{index}"), span)],
                    ),
                    span,
                )
            })
            .collect();
        expression(ast::ExprKind::Call(constructor, fields), span)
    };
    variant_arm(variant, value, span)
}

/// Match Rust enum Debug's variant spelling and tuple formatting without adding bounds from function signatures.
fn debug_arm(variant: &GenericEnumVariant, span: Span) -> ast::Arm {
    let rendered = if variant.fields.is_empty() {
        method_call(
            local("formatter", span),
            "write_str",
            thin_vec![text(&variant.name, span)],
            span,
        )
    } else {
        let mut tuple = method_call(
            local("formatter", span),
            "debug_tuple",
            thin_vec![text(&variant.name, span)],
            span,
        );
        for (index, _) in variant.fields.iter().enumerate() {
            tuple = method_call(tuple, "field", thin_vec![local(&format!("field_{index}"), span)], span);
        }
        method_call(tuple, "finish", ThinVec::new(), span)
    };
    variant_arm(variant, rendered, span)
}

/// Bind each authored payload by its unchanged slot through match ergonomics on the shared self receiver.
fn variant_arm(variant: &GenericEnumVariant, body: Box<ast::Expr>, span: Span) -> ast::Arm {
    let path = variant_path(&variant.name, span);
    let kind = if variant.fields.is_empty() {
        ast::PatKind::Path(None, path)
    } else {
        ast::PatKind::TupleStruct(
            None,
            path,
            variant
                .fields
                .iter()
                .enumerate()
                .map(|(index, _)| *binding(&format!("field_{index}"), span))
                .collect(),
        )
    };
    ast::Arm {
        attrs: ThinVec::new(),
        pat: Box::new(ast::Pat {
            id: ast::DUMMY_NODE_ID,
            kind,
            span,
            tokens: None,
        }),
        guard: None,
        body: Some(body),
        span,
        id: ast::DUMMY_NODE_ID,
        is_placeholder: false,
    }
}

/// Name an authored variant under the exact Self ADT of its native trait implementation.
fn variant_path(name: &str, span: Span) -> ast::Path {
    let mut path = ast::Path::from_ident(Ident::new(kw::SelfUpper, span));
    path.segments.push(ast::PathSegment::from_ident(ident(name, span)));
    path
}

/// Materialize checked generic payloads without adding variants or changing source payload positions.
fn declaration(value: &GenericEnumDeclaration, generator: &ast::attr::AttrIdGenerator, span: Span) -> Box<ast::Item> {
    let variants = value
        .variants
        .iter()
        .map(|variant| {
            let fields = variant
                .fields
                .iter()
                .map(|field| ast::FieldDef {
                    attrs: ThinVec::new(),
                    id: ast::DUMMY_NODE_ID,
                    span,
                    vis: inherited(span),
                    mut_restriction: ast::MutRestriction {
                        kind: ast::RestrictionKind::Unrestricted,
                        span,
                        tokens: None,
                    },
                    safety: ast::Safety::Default,
                    ident: None,
                    ty: layout_type(field, span),
                    default: None,
                    is_placeholder: false,
                })
                .collect();
            ast::Variant {
                attrs: ThinVec::new(),
                id: ast::DUMMY_NODE_ID,
                span,
                vis: inherited(span),
                ident: ident(&variant.name, span),
                data: if variant.fields.is_empty() {
                    ast::VariantData::Unit(ast::DUMMY_NODE_ID)
                } else {
                    ast::VariantData::Tuple(fields, ast::DUMMY_NODE_ID)
                },
                disr_expr: None,
                is_placeholder: false,
            }
        })
        .collect();
    let generics = ast::Generics {
        params: value
            .parameters
            .iter()
            .map(|name| ast::GenericParam {
                id: ast::DUMMY_NODE_ID,
                ident: ident(name, span),
                attrs: ThinVec::new(),
                bounds: Vec::new(),
                is_placeholder: false,
                kind: ast::GenericParamKind::Type { default: None },
                colon_span: None,
            })
            .collect(),
        where_clause: ast::WhereClause::default(),
        span,
    };
    let mut item = declarations::item(
        ast::ItemKind::Enum(ident(&value.name, span), generics, ast::EnumDef { variants }),
        span,
    );
    for derive in value
        .derives
        .iter()
        .filter(|name| !matches!(derives::from_str(name), Some(DeriveId::Clone | DeriveId::Debug)))
    {
        item.attrs.push(declarations::derive_attribute(generator, derive, span));
    }
    item.tokens = Some(tokens(value, span));
    item
}

/// Preserve the same generic declaration tokens for procedural derives without parsing generated Rust source.
fn tokens(value: &GenericEnumDeclaration, span: Span) -> ast::tokenstream::LazyAttrTokenStream {
    use ast::token::{Delimiter, Token, TokenKind};
    use ast::tokenstream::{AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, LazyAttrTokenStream, Spacing};
    let punctuation = |kind| AttrTokenTree::Token(Token::new(kind, span), Spacing::Alone);
    let mut variants = Vec::new();
    for variant in &value.variants {
        variants.push(declarations::name_token(&variant.name, span));
        if !variant.fields.is_empty() {
            let mut fields = Vec::new();
            for field in &variant.fields {
                fields.extend(declarations::type_tokens(&layout_type(field, span)));
                fields.push(punctuation(TokenKind::Comma));
            }
            variants.push(AttrTokenTree::Delimited(
                DelimSpan::from_single(span),
                DelimSpacing::new(Spacing::Alone, Spacing::Alone),
                Delimiter::Parenthesis,
                AttrTokenStream::new(fields),
            ));
        }
        variants.push(punctuation(TokenKind::Comma));
    }
    let mut tokens = vec![
        declarations::keyword_token("pub", span),
        declarations::keyword_token("enum", span),
        declarations::name_token(&value.name, span),
        punctuation(TokenKind::Lt),
    ];
    for parameter in &value.parameters {
        tokens.push(declarations::name_token(parameter, span));
        tokens.push(punctuation(TokenKind::Comma));
    }
    tokens.push(punctuation(TokenKind::Gt));
    tokens.push(AttrTokenTree::Delimited(
        DelimSpan::from_single(span),
        DelimSpacing::new(Spacing::Alone, Spacing::Alone),
        Delimiter::Brace,
        AttrTokenStream::new(variants),
    ));
    LazyAttrTokenStream::new_direct(AttrTokenStream::new(tokens))
}
