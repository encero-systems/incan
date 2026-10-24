//! Attribute macros a source declaration applies (#2256).
//!
//! The checker selected each macro from the providing Loaf's facts and kept the written arguments. This module spells
//! them as outer attributes on the injected function and gives the item its tokens, so a procedural attribute macro
//! receives the declaration it decorates exactly as a derive receives its type. rustc expands the macro; nothing here
//! predicts or parses its output, and the planned body still replaces the placeholder at `mir_built`.

use crate::ast_build::{global_path, ident, path_ty};
use crate::declarations::{keyword_token, name_token, ty, type_tokens};
use crate::plan::{AttributeArgument, AttributeValue, Function, NativeAttribute, Plan, PlanType};
use rustc_ast as ast;
use rustc_ast::token::{Delimiter, Lit, LitKind, Token, TokenKind};
use rustc_ast::tokenstream::{
    AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, LazyAttrTokenStream, Spacing, TokenStream,
};
use rustc_span::{Span, Symbol};
use thin_vec::ThinVec;

/// The outer attributes a function's selected attribute macros become, in written order.
pub(crate) fn attributes(
    generator: &ast::attr::AttrIdGenerator,
    function: &Function,
    span: Span,
) -> Vec<ast::Attribute> {
    function
        .attributes
        .iter()
        .map(|attribute| attribute_item(generator, attribute, span))
        .collect()
}

/// One `#[path(arguments)]` attribute, with the tokens a procedural macro invocation reads.
fn attribute_item(generator: &ast::attr::AttrIdGenerator, attribute: &NativeAttribute, span: Span) -> ast::Attribute {
    let segments: Vec<&str> = attribute.path.split("::").collect();
    let mut inside = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        if index != 0 {
            inside.push(punctuation(TokenKind::PathSep, span));
        }
        inside.push(name_token(segment, span));
    }
    let args = if attribute.arguments.is_empty() {
        ast::AttrArgs::Empty
    } else {
        let group = AttrTokenStream::new(argument_tokens(&attribute.arguments, span));
        inside.push(group_tokens(Delimiter::Parenthesis, group.clone(), span));
        ast::AttrArgs::Delimited(ast::DelimArgs {
            dspan: DelimSpan::from_single(span),
            delim: Delimiter::Parenthesis,
            tokens: TokenStream::new(group.to_token_trees()),
        })
    };
    let tokens = LazyAttrTokenStream::new_direct(AttrTokenStream::new(vec![
        AttrTokenTree::Token(Token::new(TokenKind::Pound, span), Spacing::JointHidden),
        group_tokens(Delimiter::Bracket, AttrTokenStream::new(inside), span),
    ]));
    let path = ast::Path {
        span,
        segments: segments
            .iter()
            .map(|segment| ast::PathSegment::from_ident(ident(segment, span)))
            .collect::<ThinVec<_>>(),
        tokens: None,
    };
    ast::attr::mk_attr_from_item(
        generator,
        ast::AttrItem {
            unsafety: ast::Safety::Default,
            path,
            args: ast::AttrItemKind::Unparsed(args),
            tokens: None,
        },
        Some(tokens),
        ast::AttrStyle::Outer,
        span,
    )
}

/// The comma-separated arguments of one attribute, keyword arguments written `name = value`.
fn argument_tokens(arguments: &[AttributeArgument], span: Span) -> Vec<AttrTokenTree> {
    let mut tokens = Vec::new();
    for (index, argument) in arguments.iter().enumerate() {
        if index != 0 {
            tokens.push(punctuation(TokenKind::Comma, span));
        }
        if !argument.name.is_empty() {
            tokens.push(name_token(&argument.name, span));
            tokens.push(punctuation(TokenKind::Eq, span));
        }
        tokens.extend(value_tokens(&argument.value, span));
    }
    tokens
}

/// One literal as Rust literal tokens, or a list as a bracketed group of them.
fn value_tokens(value: &AttributeValue, span: Span) -> Vec<AttrTokenTree> {
    match value {
        AttributeValue::Text(text) => vec![literal(LitKind::Str, &text.escape_default().to_string(), span)],
        AttributeValue::Int(number) => {
            signed_literal(*number < 0, LitKind::Integer, number.unsigned_abs().to_string(), span)
        }
        AttributeValue::Float(number) => {
            signed_literal(*number < 0.0, LitKind::Float, format!("{:?}", number.abs()), span)
        }
        AttributeValue::Bool(flag) => vec![keyword_token(if *flag { "true" } else { "false" }, span)],
        AttributeValue::List(items) => {
            let mut elements = Vec::new();
            for (index, item) in items.iter().enumerate() {
                if index != 0 {
                    elements.push(punctuation(TokenKind::Comma, span));
                }
                elements.extend(value_tokens(item, span));
            }
            vec![group_tokens(Delimiter::Bracket, AttrTokenStream::new(elements), span)]
        }
    }
}

/// A numeric literal, preceded by a minus sign when the written value was negative.
fn signed_literal(negative: bool, kind: LitKind, digits: String, span: Span) -> Vec<AttrTokenTree> {
    let mut tokens = Vec::new();
    if negative {
        tokens.push(punctuation(TokenKind::Minus, span));
    }
    tokens.push(literal(kind, &digits, span));
    tokens
}

/// The tokens of the injected function declaration an attribute macro takes as its input.
///
/// Native nominal aliases expose their checked public item and type arguments so a procedural macro observes the
/// actual foreign type. The ABI remains identical to the injected alias-based AST signature.
pub(crate) fn function_tokens(plan: &Plan, function: &Function, span: Span) -> LazyAttrTokenStream {
    let mut tokens = vec![keyword_token("pub", span)];
    if function.is_async {
        tokens.push(keyword_token("async", span));
    }
    tokens.push(keyword_token("fn", span));
    tokens.push(name_token(&function.name, span));
    let mut parameters = Vec::new();
    for parameter in &function.parameters {
        parameters.push(name_token(&parameter.name, span));
        parameters.push(punctuation(TokenKind::Colon, span));
        parameters.extend(type_tokens(&macro_signature_type(plan, &parameter.ty, span)));
        parameters.push(punctuation(TokenKind::Comma, span));
    }
    tokens.push(group_tokens(
        Delimiter::Parenthesis,
        AttrTokenStream::new(parameters),
        span,
    ));
    tokens.push(punctuation(TokenKind::RArrow, span));
    tokens.extend(type_tokens(&macro_signature_type(plan, &function.return_type, span)));
    let diverge = vec![
        keyword_token("loop", span),
        group_tokens(Delimiter::Brace, AttrTokenStream::new(Vec::new()), span),
    ];
    tokens.push(group_tokens(Delimiter::Brace, AttrTokenStream::new(diverge), span));
    LazyAttrTokenStream::new_direct(AttrTokenStream::new(tokens))
}

/// Reify a checked external nominal for macro syntax inspection, retaining local types and ordinary ABI aliases.
/// This copies the selected item and arguments; no macro name or source spelling determines native storage.
fn macro_signature_type(plan: &Plan, kind: &PlanType, span: Span) -> Box<ast::Ty> {
    let (item, arguments) = match kind {
        PlanType::Model(index, _) => usize::try_from(*index)
            .ok()
            .and_then(|index| plan.models.get(index))
            .map(|model| (model.library_item.as_str(), model.library_type_arguments.as_slice())),
        PlanType::Enum(index, _) => usize::try_from(*index)
            .ok()
            .and_then(|index| plan.enums.get(index))
            .map(|value| (value.library_item.as_str(), value.library_type_arguments.as_slice())),
        _ => None,
    }
    .unwrap_or(("", &[]));
    if item.is_empty() {
        return ty(kind, span);
    }
    let segments = item
        .split("::")
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let mut path = global_path(&segments, span);
    if !arguments.is_empty()
        && let Some(last) = path.segments.last_mut()
    {
        last.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
            span,
            args: arguments
                .iter()
                .map(|argument| ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty(argument, span))))
                .collect(),
        })));
    }
    path_ty(path, span)
}

/// A delimited group of tokens.
fn group_tokens(delimiter: Delimiter, stream: AttrTokenStream, span: Span) -> AttrTokenTree {
    AttrTokenTree::Delimited(
        DelimSpan::from_single(span),
        DelimSpacing::new(Spacing::Alone, Spacing::Alone),
        delimiter,
        stream,
    )
}

/// One punctuation token.
fn punctuation(kind: TokenKind, span: Span) -> AttrTokenTree {
    AttrTokenTree::Token(Token::new(kind, span), Spacing::Alone)
}

/// One literal token with no suffix.
fn literal(kind: LitKind, text: &str, span: Span) -> AttrTokenTree {
    AttrTokenTree::Token(
        Token::new(TokenKind::Literal(Lit::new(kind, Symbol::intern(text), None)), span),
        Spacing::Alone,
    )
}
