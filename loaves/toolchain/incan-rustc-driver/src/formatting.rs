//! Native formatting primitives for exact plan carriers; rustc checks their ordinary Debug or Display obligation.

use crate::ast_build::{binding, expression, global_path, ident, path_ty, trait_bound};
use crate::plan::{CalleeKind, Plan, TerminatorKind};
use rustc_ast as ast;
use rustc_ast::token::{Delimiter, IdentIsRaw, Lit, LitKind, Token, TokenKind};
use rustc_ast::tokenstream::{DelimSpan, Spacing, TokenStream, TokenTree};
use rustc_span::{Span, Symbol};
use thin_vec::{ThinVec, thin_vec};

/// Name one native trait primitive; the call's checked carrier is its explicit type argument.
pub(crate) fn name(debug: bool) -> &'static str {
    if debug {
        "__incan_format_debug"
    } else {
        "__incan_format_display"
    }
}

/// Declare each requested trait primitive once. Its type parameter keeps erased local loans out of AST signatures.
pub(crate) fn items(plan: &Plan, span: Span) -> Vec<Box<ast::Item>> {
    let mut retained = Vec::new();
    let mut items = Vec::new();
    for function in &plan.functions {
        for block in &function.blocks {
            let TerminatorKind::Call(callee, ..) = &block.terminator.kind else {
                continue;
            };
            let CalleeKind::FormatValue(_, debug) = &callee.kind else {
                continue;
            };
            let symbol = name(*debug);
            if retained.contains(&symbol) {
                continue;
            }
            items.push(item(symbol, *debug, span));
            retained.push(symbol);
        }
    }
    items
}

/// Invoke the standard formatting macro with three fixed tokens so its expansion owns internal formatting features.
/// The native type remains in the call's type argument; neither token text nor a parsed Rust program selects it.
fn argument(debug: bool, span: Span) -> Box<ast::Expr> {
    let tokens = [
        TokenKind::Literal(Lit {
            kind: LitKind::Str,
            symbol: Symbol::intern(if debug { "{:?}" } else { "{}" }),
            suffix: None,
        }),
        TokenKind::Comma,
        TokenKind::Ident(Symbol::intern("value"), IdentIsRaw::No),
    ];
    expression(
        ast::ExprKind::MacCall(Box::new(ast::MacCall {
            path: global_path(&["std", "format"], span),
            args: Box::new(ast::DelimArgs {
                dspan: DelimSpan::from_single(span),
                delim: Delimiter::Parenthesis,
                tokens: TokenStream::new(
                    tokens
                        .into_iter()
                        .map(|kind| {
                            // This literal has no source spelling: a caller's one-character span cannot describe its
                            // quotes.
                            let token_span = if matches!(kind, TokenKind::Literal(_)) {
                                rustc_span::DUMMY_SP
                            } else {
                                span
                            };
                            TokenTree::Token(Token::new(kind, token_span), Spacing::Alone)
                        })
                        .collect(),
                ),
            }),
        })),
        span,
    )
}

/// Consume one exact instantiation and borrow it only while formatting, under a rustc-checked standard trait bound.
fn item(symbol: &str, debug: bool, span: Span) -> Box<ast::Item> {
    let body = argument(debug, span);
    crate::declarations::item(
        ast::ItemKind::Fn(Box::new(ast::Fn {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(symbol, span),
            generics: generics(debug, span),
            sig: ast::FnSig {
                header: ast::FnHeader::default(),
                decl: Box::new(ast::FnDecl {
                    inputs: thin_vec![ast::Param {
                        attrs: ThinVec::new(),
                        ty: path_ty(ast::Path::from_ident(ident("T", span)), span),
                        pat: binding("value", span),
                        id: ast::DUMMY_NODE_ID,
                        span,
                        is_placeholder: false,
                    }],
                    output: ast::FnRetTy::Ty(path_ty(global_path(&["std", "string", "String"], span), span)),
                }),
                span,
            },
            contract: None,
            define_opaque: None,
            body: Some(Box::new(ast::Block {
                stmts: thin_vec![ast::Stmt {
                    id: ast::DUMMY_NODE_ID,
                    kind: ast::StmtKind::Expr(body),
                    span
                }],
                id: ast::DUMMY_NODE_ID,
                rules: ast::BlockCheckMode::Default,
                span,
                tokens: None,
            })),
            eii_impls: ThinVec::new(),
        })),
        span,
    )
}

/// Keep the formatting obligation on the native generic primitive, independently of the source carrier's spelling.
fn generics(debug: bool, span: Span) -> ast::Generics {
    ast::Generics {
        params: thin_vec![ast::GenericParam {
            id: ast::DUMMY_NODE_ID,
            ident: ident("T", span),
            attrs: ThinVec::new(),
            bounds: vec![trait_bound(
                global_path(&["std", "fmt", if debug { "Debug" } else { "Display" }], span),
                span,
            )],
            is_placeholder: false,
            kind: ast::GenericParamKind::Type { default: None },
            colon_span: None,
        }],
        ..ast::Generics::default()
    }
}
