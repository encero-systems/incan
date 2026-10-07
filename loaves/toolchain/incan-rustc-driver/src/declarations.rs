//! Declarations enter as AST items; only MIR supplies executable bodies.

use crate::plan::{Function, ListLeaf, PlanType, SizedNumeric};
use rustc_ast as ast;
use rustc_span::{Ident, Span, Symbol};
use thin_vec::{ThinVec, thin_vec};

/// Create an identifier from a name that preflight validation has already admitted.
fn ident(name: &str, span: Span) -> Ident {
    Ident::new(Symbol::intern(name), span)
}

/// Declare the exact owned or borrowed runtime handle, leaving other plan types to their ordinary declaration path.
fn generator_signature_ty(kind: &PlanType, span: Span) -> Option<Box<ast::Ty>> {
    let (name, leaf, depth, mutability) = match kind {
        PlanType::Generator(leaf, depth) => ("Generator", leaf, *depth, None),
        PlanType::GeneratorMutRef(leaf, depth) => ("Generator", leaf, *depth, Some(ast::Mutability::Mut)),
        PlanType::GeneratorYield(leaf, depth) => ("GeneratorYield", leaf, *depth, None),
        PlanType::GeneratorYieldRef(leaf, depth) => ("GeneratorYield", leaf, *depth, Some(ast::Mutability::Not)),
        _ => return None,
    };
    let owned = generator_ty(name, leaf, depth, span);
    Some(match mutability {
        None => owned,
        Some(mutbl) => Box::new(ast::Ty {
            id: ast::DUMMY_NODE_ID,
            kind: ast::TyKind::Ref(None, ast::MutTy { ty: owned, mutbl }),
            span,
            tokens: None,
        }),
    })
}

/// Construct an admitted native AST type without generating or parsing Rust source.
fn ty(kind: &PlanType, span: Span) -> Box<ast::Ty> {
    if let Some(ty) = generator_signature_ty(kind, span) {
        return ty;
    }
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
        PlanType::Set(leaf) => return hashed_type("HashSet", &[leaf], span),
        PlanType::Dict(key, value) => return hashed_type("HashMap", &[key, value], span),
        PlanType::SetRef(leaf) | PlanType::SetMutRef(leaf) => ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: hashed_type("HashSet", &[leaf], span),
                mutbl: if matches!(kind, PlanType::SetMutRef(_)) {
                    ast::Mutability::Mut
                } else {
                    ast::Mutability::Not
                },
            },
        ),
        PlanType::DictRef(key, value) | PlanType::DictMutRef(key, value) => ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: hashed_type("HashMap", &[key, value], span),
                mutbl: if matches!(kind, PlanType::DictMutRef(_, _)) {
                    ast::Mutability::Mut
                } else {
                    ast::Mutability::Not
                },
            },
        ),
        PlanType::Unit => ast::TyKind::Tup(ThinVec::new()),
        PlanType::ModelRef(index, name) | PlanType::ModelMutRef(index, name) => ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: ty(&PlanType::Model(*index, name.clone()), span),
                mutbl: if matches!(kind, PlanType::ModelMutRef(..)) {
                    ast::Mutability::Mut
                } else {
                    ast::Mutability::Not
                },
            },
        ),
        PlanType::CheckedNumeric(kind) => {
            ast::TyKind::Tup(thin_vec![numeric_ty(kind, span), ty(&PlanType::Bool, span)])
        }
        PlanType::CheckedInt => ast::TyKind::Tup(thin_vec![ty(&PlanType::Int, span), ty(&PlanType::Bool, span)]),
        other => {
            let name = match other {
                PlanType::ISize => "isize",
                PlanType::USize => "usize",
                PlanType::Int => "i64",
                PlanType::Float => "f64",
                PlanType::I8 => "i8",
                PlanType::I16 => "i16",
                PlanType::I32 => "i32",
                PlanType::I128 => "i128",
                PlanType::U8 => "u8",
                PlanType::U16 => "u16",
                PlanType::U32 => "u32",
                PlanType::U64 => "u64",
                PlanType::U128 => "u128",
                PlanType::F32 => "f32",
                PlanType::F64 => "f64",

                PlanType::String => "String",
                PlanType::Model(_, name) | PlanType::Enum(_, name) => name.as_str(),
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

/// Declare the runtime generator's canonical type without creating an alternative wrapper layout.
fn generator_ty(name: &str, leaf: &ListLeaf, depth: i64, span: Span) -> Box<ast::Ty> {
    let element = if depth == 0 {
        match leaf {
            ListLeaf::Int => PlanType::Int,
            ListLeaf::Float => PlanType::Float,
            ListLeaf::Bool => PlanType::Bool,
            ListLeaf::Str => PlanType::String,
        }
    } else {
        PlanType::List(leaf.clone(), depth)
    };
    let mut path = ast::Path::from_ident(ident("incan_std_core", span));
    path.segments.push(ast::PathSegment::from_ident(ident("iter", span)));
    let mut segment = ast::PathSegment::from_ident(ident(name, span));
    segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
        span,
        args: thin_vec![ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty(&element, span)))],
    })));
    path.segments.push(segment);
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, path),
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

/// Inject the exact checked nominal layout, preserving visibility and field order; the sole `0` field is a tuple slot.
pub fn model(model: &crate::plan::ModelDeclaration, span: Span) -> Box<ast::Item> {
    let tuple = model.fields.len() == 1 && model.fields[0].name == "0";
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
            ident: if tuple { None } else { Some(ident(&field.name, span)) },
            ty: ty(&field.ty, span),
            default: None,
            is_placeholder: false,
        })
        .collect();
    let mut declaration = item(
        ast::ItemKind::Struct(
            ident(&model.name, span),
            ast::Generics::default(),
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
    declaration.vis = visibility(model.public, span);
    declaration.tokens = Some(model_tokens(model, span));
    declaration
}

/// Retain the injected declaration's tokens for procedural derives, without parsing or generating source text.
fn model_tokens(model: &crate::plan::ModelDeclaration, span: Span) -> ast::tokenstream::LazyAttrTokenStream {
    use ast::token::{Delimiter, TokenKind};
    use ast::tokenstream::{AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, LazyAttrTokenStream, Spacing};
    let mut fields = Vec::new();
    let tuple = model.fields.len() == 1 && model.fields[0].name == "0";
    for (field, public) in model.fields.iter().zip(&model.field_public) {
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

/// Construct the type of an admitted overflow-pair value.
fn numeric_ty(kind: &SizedNumeric, span: Span) -> Box<ast::Ty> {
    let kind = match kind {
        SizedNumeric::I8 => PlanType::I8,
        SizedNumeric::I16 => PlanType::I16,
        SizedNumeric::I32 => PlanType::I32,
        SizedNumeric::I128 => PlanType::I128,
        SizedNumeric::U8 => PlanType::U8,
        SizedNumeric::U16 => PlanType::U16,
        SizedNumeric::U32 => PlanType::U32,
        SizedNumeric::U64 => PlanType::U64,
        SizedNumeric::U128 => PlanType::U128,
        SizedNumeric::F32 => PlanType::F32,
        SizedNumeric::F64 => PlanType::F64,
        SizedNumeric::ISize => PlanType::ISize,
        SizedNumeric::USize => PlanType::USize,
    };
    ty(&kind, span)
}

/// Build the standard collection path with explicit checked primitive type arguments.
fn hashed_type(name: &str, leaves: &[&ListLeaf], span: Span) -> Box<ast::Ty> {
    let mut path = ast::Path::from_ident(ident("std", span));
    path.segments
        .push(ast::PathSegment::from_ident(ident("collections", span)));
    let mut segment = ast::PathSegment::from_ident(ident(name, span));
    segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
        span,
        args: leaves
            .iter()
            .map(|leaf| {
                ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty(
                    &match leaf {
                        ListLeaf::Int => PlanType::Int,
                        ListLeaf::Float => PlanType::Float,
                        ListLeaf::Bool => PlanType::Bool,
                        ListLeaf::Str => PlanType::String,
                    },
                    span,
                )))
            })
            .collect(),
    })));
    path.segments.push(segment);
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, path),
        span,
        tokens: None,
    })
}

/// Inject source-ordered unit and tuple variants as AST nodes; executable bodies still enter only through MIR.
pub fn enum_declaration(value: &crate::plan::EnumDeclaration, span: Span) -> Box<ast::Item> {
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
                    vis: visibility(false, span),
                    mut_restriction: ast::MutRestriction {
                        kind: ast::RestrictionKind::Unrestricted,
                        span,
                        tokens: None,
                    },
                    safety: ast::Safety::Default,
                    ident: None,
                    ty: ty(field, span),
                    default: None,
                    is_placeholder: false,
                })
                .collect();
            ast::Variant {
                attrs: ThinVec::new(),
                id: ast::DUMMY_NODE_ID,
                span,
                vis: visibility(false, span),
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
    let mut declaration = item(
        ast::ItemKind::Enum(
            ident(&value.name, span),
            ast::Generics::default(),
            ast::EnumDef { variants },
        ),
        span,
    );
    declaration.vis = visibility(value.public, span);
    declaration.tokens = Some(enum_tokens(value, span));
    declaration
}

/// Preserve enum tokens for derives from the exact planned layout without parsing generated Rust source.
fn enum_tokens(value: &crate::plan::EnumDeclaration, span: Span) -> ast::tokenstream::LazyAttrTokenStream {
    use ast::token::{Delimiter, Token, TokenKind};
    use ast::tokenstream::{AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, LazyAttrTokenStream, Spacing};
    let mut variants = Vec::new();
    for variant in &value.variants {
        variants.push(name_token(&variant.name, span));
        if !variant.fields.is_empty() {
            let mut fields = Vec::new();
            for field in &variant.fields {
                fields.extend(type_tokens(&ty(field, span)));
                fields.push(AttrTokenTree::Token(Token::new(TokenKind::Comma, span), Spacing::Alone));
            }
            variants.push(AttrTokenTree::Delimited(
                DelimSpan::from_single(span),
                DelimSpacing::new(Spacing::Alone, Spacing::Alone),
                Delimiter::Parenthesis,
                AttrTokenStream::new(fields),
            ));
        }
        variants.push(AttrTokenTree::Token(Token::new(TokenKind::Comma, span), Spacing::Alone));
    }
    let mut tokens = Vec::new();
    if value.public {
        tokens.push(keyword_token("pub", span));
    }
    tokens.push(keyword_token("enum", span));
    tokens.push(name_token(&value.name, span));
    tokens.push(AttrTokenTree::Delimited(
        DelimSpan::from_single(span),
        DelimSpacing::new(Spacing::Alone, Spacing::Alone),
        Delimiter::Brace,
        AttrTokenStream::new(variants),
    ));
    LazyAttrTokenStream::new_direct(AttrTokenStream::new(tokens))
}
