//! The serde implementations legacy writes by hand for a value enum, injected as AST beside its enum.
//!
//! A value enum serializes as its raw value, never its variant name: `Env.Prod` declared `Prod = "production"` writes
//! `"production"`, and reading accepts only a raw value, failing with `invalid value for Env: <value>` exactly as
//! legacy's generated impls fail. A serde derive would write and read variant names instead.

use crate::ast_build::{
    binding, block, expression, expression_statement, global_path, ident, inherited, local, path_ty, self_value,
    trait_bound,
};
use crate::plan::EnumDeclaration;
use rustc_ast as ast;
use rustc_span::{Ident, Span, Symbol, kw, respan};
use thin_vec::{ThinVec, thin_vec};

/// Build legacy's `Serialize`: `match self { Self::V => ::serde::Serializer::serialize_str(serializer, "raw"), .. }`.
///
/// An integer value enum writes its raw values through `serialize_i64` instead. Validation has proved one raw value per
/// unit variant and, for an integer enum, that each parses as an `i64`.
pub fn serialize_impl(value: &EnumDeclaration, span: Span) -> Box<ast::Item> {
    let writer = if integer_backed(value) {
        "serialize_i64"
    } else {
        "serialize_str"
    };
    let arms = value
        .variants
        .iter()
        .zip(&value.raw_values)
        .map(|(variant, raw)| {
            let write = call(
                global_path(&["serde", "Serializer", writer], span),
                thin_vec![local("serializer", span), raw_expression(value, raw, span)],
                span,
            );
            arm(variant_pattern(&variant.name, span), write, span)
        })
        .collect();
    let body = expression(
        ast::ExprKind::Match(self_value(span), arms, ast::MatchKind::Prefix),
        span,
    );
    let output = result_type(projection("__S", "Ok", span), projection("__S", "Error", span), span);
    let signature = signature(
        thin_vec![
            receiver(span),
            parameter("serializer", path_ty(local_path("__S", span), span), span)
        ],
        output,
        span,
    );
    let generics = generics(
        thin_vec![type_parameter("__S", global_path(&["serde", "Serializer"], span), span)],
        span,
    );
    let method = method(
        "serialize",
        generics,
        signature,
        block(thin_vec![expression_statement(body, span)], span),
        span,
    );
    trait_impl(
        ast::Generics::default(),
        global_path(&["serde", "Serialize"], span),
        value,
        method,
        span,
    )
}

/// Build legacy's `Deserialize`: read the raw carrier, then select the variant whose raw value it equals.
///
/// ```text
/// impl<'de> ::serde::Deserialize<'de> for Enum {
///     fn deserialize<__D: ::serde::Deserializer<'de>>(deserializer: __D) -> Result<Self, __D::Error> {
///         let value: String = ::serde::Deserialize::deserialize(deserializer)?;
///         match value.as_str() {
///             "raw" => Ok(Self::V),
///             other => Err(::serde::de::Error::custom(String::from("invalid value for Enum: ") + other)),
///         }
///     }
/// }
/// ```
///
/// An integer value enum reads an `i64` and renders a rejected value through its `Display`, as legacy's `format!`
/// does.
pub fn deserialize_impl(value: &EnumDeclaration, span: Span) -> Box<ast::Item> {
    let integer = integer_backed(value);
    let carrier = if integer {
        path_ty(local_path("i64", span), span)
    } else {
        path_ty(global_path(&["std", "string", "String"], span), span)
    };
    let read = expression(
        ast::ExprKind::Try(call(
            global_path(&["serde", "Deserialize", "deserialize"], span),
            thin_vec![local("deserializer", span)],
            span,
        )),
        span,
    );
    let scrutinee = if integer {
        local("value", span)
    } else {
        expression(
            ast::ExprKind::MethodCall(Box::new(ast::MethodCall {
                seg: ast::PathSegment::from_ident(ident("as_str", span)),
                receiver: local("value", span),
                args: ThinVec::new(),
                span,
            })),
            span,
        )
    };
    let mut arms: ThinVec<ast::Arm> = value
        .variants
        .iter()
        .zip(&value.raw_values)
        .map(|(variant, raw)| {
            let selected = call(
                global_path(&["core", "result", "Result", "Ok"], span),
                thin_vec![expression(
                    ast::ExprKind::Path(None, self_variant(&variant.name, span)),
                    span
                )],
                span,
            );
            let pattern = Box::new(ast::Pat {
                id: ast::DUMMY_NODE_ID,
                kind: ast::PatKind::Expr(raw_expression(value, raw, span)),
                span,
                tokens: None,
            });
            arm(pattern, selected, span)
        })
        .collect();
    arms.push(arm(binding("other", span), rejection(value, integer, span), span));
    let body = expression(ast::ExprKind::Match(scrutinee, arms, ast::MatchKind::Prefix), span);
    let statements = thin_vec![
        ast::Stmt {
            id: ast::DUMMY_NODE_ID,
            kind: ast::StmtKind::Let(Box::new(ast::Local {
                id: ast::DUMMY_NODE_ID,
                super_: None,
                pat: binding("value", span),
                ty: Some(carrier),
                kind: ast::LocalKind::Init(read),
                span,
                colon_sp: None,
                attrs: ThinVec::new(),
                tokens: None,
            })),
            span,
        },
        expression_statement(body, span)
    ];
    let output = result_type(
        path_ty(ast::Path::from_ident(Ident::new(kw::SelfUpper, span)), span),
        projection("__D", "Error", span),
        span,
    );
    let signature = signature(
        thin_vec![parameter("deserializer", path_ty(local_path("__D", span), span), span)],
        output,
        span,
    );
    let deserializer = global_path_with_lifetime(&["serde", "Deserializer"], span);
    let method = method(
        "deserialize",
        generics(thin_vec![type_parameter("__D", deserializer, span)], span),
        signature,
        block(statements, span),
        span,
    );
    trait_impl(
        generics(thin_vec![lifetime_parameter(span)], span),
        global_path_with_lifetime(&["serde", "Deserialize"], span),
        value,
        method,
        span,
    )
}

/// `Err(::serde::de::Error::custom(String::from("invalid value for Enum: ") + other))`, with an integer rendered
/// through `ToString` first.
fn rejection(value: &EnumDeclaration, integer: bool, span: Span) -> Box<ast::Expr> {
    let prefix = call(
        global_path(&["std", "string", "String", "from"], span),
        thin_vec![string_literal(&format!("invalid value for {}: ", value.name), span)],
        span,
    );
    let rendered = if integer {
        expression(
            ast::ExprKind::AddrOf(
                ast::BorrowKind::Ref,
                ast::Mutability::Not,
                call(
                    global_path(&["std", "string", "ToString", "to_string"], span),
                    thin_vec![expression(
                        ast::ExprKind::AddrOf(ast::BorrowKind::Ref, ast::Mutability::Not, local("other", span)),
                        span,
                    )],
                    span,
                ),
            ),
            span,
        )
    } else {
        local("other", span)
    };
    let message = expression(
        ast::ExprKind::Binary(respan(span, ast::BinOpKind::Add), prefix, rendered),
        span,
    );
    call(
        global_path(&["core", "result", "Result", "Err"], span),
        thin_vec![call(
            global_path(&["serde", "de", "Error", "custom"], span),
            thin_vec![message],
            span
        )],
        span,
    )
}

/// Whether the raw values are integers rather than text.
fn integer_backed(value: &EnumDeclaration) -> bool {
    value.raw_kind == "int"
}

/// A raw value as a literal: an escaped string, or a decimal integer with a leading negation when negative.
fn raw_expression(value: &EnumDeclaration, raw: &str, span: Span) -> Box<ast::Expr> {
    if !integer_backed(value) {
        return string_literal(raw, span);
    }
    let (negative, digits) = match raw.strip_prefix('-') {
        Some(digits) => (true, digits),
        None => (false, raw),
    };
    let literal = expression(
        ast::ExprKind::Lit(ast::token::Lit::new(
            ast::token::LitKind::Integer,
            Symbol::intern(digits),
            None,
        )),
        span,
    );
    if negative {
        expression(ast::ExprKind::Unary(ast::UnOp::Neg, literal), span)
    } else {
        literal
    }
}

/// A string literal whose source spelling escapes the text, so any raw value reads back unchanged.
fn string_literal(text: &str, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::Lit(ast::token::Lit::new(
            ast::token::LitKind::Str,
            Symbol::intern(&text.escape_default().to_string()),
            None,
        )),
        span,
    )
}

/// A call of a function named by a path.
fn call(function: ast::Path, arguments: ThinVec<Box<ast::Expr>>, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::Call(expression(ast::ExprKind::Path(None, function), span), arguments),
        span,
    )
}

/// One match arm without a guard.
fn arm(pattern: Box<ast::Pat>, body: Box<ast::Expr>, span: Span) -> ast::Arm {
    ast::Arm {
        attrs: ThinVec::new(),
        pat: pattern,
        guard: None,
        body: Some(body),
        span,
        id: ast::DUMMY_NODE_ID,
        is_placeholder: false,
    }
}

/// `Self::V`, the path of one unit variant.
fn self_variant(variant: &str, span: Span) -> ast::Path {
    let mut path = ast::Path::from_ident(Ident::new(kw::SelfUpper, span));
    path.segments.push(ast::PathSegment::from_ident(ident(variant, span)));
    path
}

/// The pattern matching one unit variant through `Self`.
fn variant_pattern(variant: &str, span: Span) -> Box<ast::Pat> {
    Box::new(ast::Pat {
        id: ast::DUMMY_NODE_ID,
        kind: ast::PatKind::Path(None, self_variant(variant, span)),
        span,
        tokens: None,
    })
}

/// A one-segment path that resolves in the item's own scope (a type parameter or a primitive type).
fn local_path(name: &str, span: Span) -> ast::Path {
    ast::Path::from_ident(ident(name, span))
}

/// `P::Item`, an associated type of a type parameter.
fn projection(parameter: &str, item: &str, span: Span) -> Box<ast::Ty> {
    let mut path = local_path(parameter, span);
    path.segments.push(ast::PathSegment::from_ident(ident(item, span)));
    path_ty(path, span)
}

/// `::core::result::Result<T, E>`.
fn result_type(ok: Box<ast::Ty>, error: Box<ast::Ty>, span: Span) -> Box<ast::Ty> {
    let mut path = global_path(&["core", "result", "Result"], span);
    if let Some(last) = path.segments.last_mut() {
        last.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
            span,
            args: thin_vec![
                ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ok)),
                ast::AngleBracketedArg::Arg(ast::GenericArg::Type(error))
            ],
        })));
    }
    path_ty(path, span)
}

/// A crate-graph path whose last segment takes the impl's `'de` lifetime, as `::serde::Deserialize<'de>`.
fn global_path_with_lifetime(segments: &[&str], span: Span) -> ast::Path {
    let mut path = global_path(segments, span);
    if let Some(last) = path.segments.last_mut() {
        last.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
            span,
            args: thin_vec![ast::AngleBracketedArg::Arg(ast::GenericArg::Lifetime(ast::Lifetime {
                id: ast::DUMMY_NODE_ID,
                ident: ident("'de", span),
            }))],
        })));
    }
    path
}

/// The impl's `'de` lifetime parameter.
fn lifetime_parameter(span: Span) -> ast::GenericParam {
    ast::GenericParam {
        id: ast::DUMMY_NODE_ID,
        ident: ident("'de", span),
        attrs: ThinVec::new(),
        bounds: Vec::new(),
        is_placeholder: false,
        kind: ast::GenericParamKind::Lifetime,
        colon_span: None,
    }
}

/// A method type parameter bounded by one trait path, which may carry generic arguments.
fn type_parameter(name: &str, bound: ast::Path, span: Span) -> ast::GenericParam {
    ast::GenericParam {
        id: ast::DUMMY_NODE_ID,
        ident: ident(name, span),
        attrs: ThinVec::new(),
        bounds: vec![trait_bound(bound, span)],
        is_placeholder: false,
        kind: ast::GenericParamKind::Type { default: None },
        colon_span: None,
    }
}

/// Generic parameters without a where clause.
fn generics(params: ThinVec<ast::GenericParam>, span: Span) -> ast::Generics {
    ast::Generics {
        params,
        where_clause: ast::WhereClause::default(),
        span,
    }
}

/// The `&self` receiver.
fn receiver(span: Span) -> ast::Param {
    ast::Param::from_self(
        ThinVec::new(),
        respan(span, ast::SelfKind::Region(None, ast::Mutability::Not)),
        Ident::new(kw::SelfLower, span),
    )
}

/// One by-value parameter.
fn parameter(name: &str, ty: Box<ast::Ty>, span: Span) -> ast::Param {
    ast::Param {
        attrs: ThinVec::new(),
        ty,
        pat: binding(name, span),
        id: ast::DUMMY_NODE_ID,
        span,
        is_placeholder: false,
    }
}

/// A method signature with the given parameters and return type.
fn signature(inputs: ThinVec<ast::Param>, output: Box<ast::Ty>, span: Span) -> ast::FnSig {
    ast::FnSig {
        header: ast::FnHeader::default(),
        decl: Box::new(ast::FnDecl {
            inputs,
            output: ast::FnRetTy::Ty(output),
        }),
        span,
    }
}

/// One trait method with its own generics and body.
fn method(
    name: &str,
    generics: ast::Generics,
    sig: ast::FnSig,
    body: Box<ast::Block>,
    span: Span,
) -> Box<ast::AssocItem> {
    Box::new(ast::AssocItem {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: inherited(span),
        kind: ast::AssocItemKind::Fn(Box::new(ast::Fn {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(name, span),
            generics,
            sig,
            contract: None,
            define_opaque: None,
            body: Some(body),
            eii_impls: ThinVec::new(),
        })),
        tokens: None,
    })
}

/// `impl<generics> Trait for Enum { method }`.
fn trait_impl(
    generics: ast::Generics,
    trait_path: ast::Path,
    value: &EnumDeclaration,
    method: Box<ast::AssocItem>,
    span: Span,
) -> Box<ast::Item> {
    let header = ast::TraitImplHeader {
        defaultness: ast::Defaultness::Implicit,
        safety: ast::Safety::Default,
        polarity: ast::ImplPolarity::Positive,
        trait_ref: ast::TraitRef {
            path: trait_path,
            ref_id: ast::DUMMY_NODE_ID,
        },
    };
    Box::new(ast::Item {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: inherited(span),
        kind: ast::ItemKind::Impl(ast::Impl {
            generics,
            constness: ast::Const::No,
            of_trait: Some(Box::new(header)),
            self_ty: path_ty(ast::Path::from_ident(ident(&value.name, span)), span),
            items: thin_vec![method],
        }),
        tokens: None,
    })
}
