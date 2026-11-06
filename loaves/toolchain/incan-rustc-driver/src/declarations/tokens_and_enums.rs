//! Type tokens, derive attributes, numeric and hashed types, enum declarations and zip types.

use super::*;

/// Mirror admitted paths, tuples, references, slices, and function-pointer signatures into procedural derive tokens
/// while preserving explicit lifetimes.
pub(crate) fn type_tokens(ty: &ast::Ty) -> Vec<ast::tokenstream::AttrTokenTree> {
    use ast::token::{Delimiter, Token, TokenKind};
    use ast::tokenstream::{AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, Spacing};
    match &ty.kind {
        ast::TyKind::Path(None, path) => {
            let mut tokens = Vec::new();
            for (index, segment) in path.segments.iter().enumerate() {
                if segment.ident.name == rustc_span::kw::PathRoot {
                    continue;
                }
                if index != 0 {
                    tokens.push(AttrTokenTree::Token(
                        Token::new(TokenKind::PathSep, ty.span),
                        Spacing::Alone,
                    ));
                }
                tokens.push(name_token(segment.ident.name.as_str(), ty.span));
                if let Some(arguments) = &segment.args {
                    tokens.extend(generic_argument_tokens(arguments, ty.span));
                }
            }
            tokens
        }
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
        ast::TyKind::Ref(lifetime, referent) => {
            let mut tokens = vec![AttrTokenTree::Token(
                Token::new(TokenKind::And, ty.span),
                Spacing::Alone,
            )];
            if let Some(lifetime) = lifetime {
                tokens.push(AttrTokenTree::Token(
                    Token::new(
                        TokenKind::Lifetime(lifetime.ident.name, ast::token::IdentIsRaw::No),
                        ty.span,
                    ),
                    Spacing::Alone,
                ));
            }
            if referent.mutbl == ast::Mutability::Mut {
                tokens.push(keyword_token("mut", ty.span));
            }
            tokens.extend(type_tokens(&referent.ty));
            tokens
        }
        ast::TyKind::FnPtr(pointer) => function_pointer_tokens(pointer, ty.span),
        ast::TyKind::Slice(element) => vec![AttrTokenTree::Delimited(
            DelimSpan::from_single(ty.span),
            DelimSpacing::new(Spacing::Alone, Spacing::Alone),
            Delimiter::Bracket,
            AttrTokenStream::new(type_tokens(element)),
        )],
        _ => {
            unreachable!("the admitted AST type builder emits paths, tuples, references, slices, and function pointers")
        }
    }
}

/// Mirror the safe, ordinary function-pointer AST builder's exact inputs and output without inventing token types.
/// Borrowed signature components recurse through the same reference and container token projection as other fields.
pub(super) fn function_pointer_tokens(pointer: &ast::FnPtrTy, span: Span) -> Vec<ast::tokenstream::AttrTokenTree> {
    use ast::token::{Delimiter, Token, TokenKind};
    use ast::tokenstream::{AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, Spacing};

    // ---- Signature inputs ----
    let mut parameters = Vec::new();
    for parameter in &pointer.decl.inputs {
        parameters.extend(type_tokens(&parameter.ty));
        parameters.push(AttrTokenTree::Token(Token::new(TokenKind::Comma, span), Spacing::Alone));
    }
    let mut tokens = vec![
        keyword_token("fn", span),
        AttrTokenTree::Delimited(
            DelimSpan::from_single(span),
            DelimSpacing::new(Spacing::Alone, Spacing::Alone),
            Delimiter::Parenthesis,
            AttrTokenStream::new(parameters),
        ),
    ];

    // ---- Signature output ----
    if let ast::FnRetTy::Ty(output) = &pointer.decl.output {
        tokens.push(AttrTokenTree::Token(
            Token::new(TokenKind::RArrow, span),
            Spacing::Alone,
        ));
        tokens.extend(type_tokens(output));
    }
    tokens
}

/// Mirror a path segment's type arguments (`Vec<Tag>`, `HashMap<String, i64>`) into derive input tokens.
///
/// Without them a derive would name a generic struct bare and fail to compile; the admitted type builder spells only
/// type arguments, never lifetimes or constants.
pub(super) fn generic_argument_tokens(
    arguments: &ast::GenericArgs,
    span: Span,
) -> Vec<ast::tokenstream::AttrTokenTree> {
    use ast::token::{Token, TokenKind};
    use ast::tokenstream::{AttrTokenTree, Spacing};
    let ast::GenericArgs::AngleBracketed(angle) = arguments else {
        unreachable!("the admitted AST type builder emits only angle-bracketed type arguments")
    };
    let mut tokens = vec![AttrTokenTree::Token(Token::new(TokenKind::Lt, span), Spacing::Alone)];
    for (index, argument) in angle.args.iter().enumerate() {
        if index != 0 {
            tokens.push(AttrTokenTree::Token(Token::new(TokenKind::Comma, span), Spacing::Alone));
        }
        match argument {
            ast::AngleBracketedArg::Arg(ast::GenericArg::Type(argument)) => tokens.extend(type_tokens(argument)),
            _ => unreachable!("the admitted AST type builder emits only type arguments"),
        }
    }
    tokens.push(AttrTokenTree::Token(Token::new(TokenKind::Gt, span), Spacing::Alone));
    tokens
}

/// Convert checked visibility without widening private fields.
pub(super) fn visibility(public: bool, span: Span) -> ast::Visibility {
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
    if name.starts_with("serde::") {
        tokens.push(AttrTokenTree::Token(
            Token::new(TokenKind::PathSep, span),
            Spacing::Alone,
        ));
    }
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
    for (index, segment) in name.split("::").enumerate() {
        if index != 0 {
            tokens.push(AttrTokenTree::Token(
                Token::new(TokenKind::PathSep, span),
                Spacing::Alone,
            ));
        }
        tokens.push(AttrTokenTree::Token(
            Token::from_ast_ident(ident(segment, span)),
            Spacing::Alone,
        ));
    }
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
pub(super) fn numeric_ty(kind: &SizedNumeric, span: Span) -> Box<ast::Ty> {
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
pub(super) fn hashed_type(name: &str, leaves: &[&ListLeaf], span: Span) -> Box<ast::Ty> {
    let mut path = ast::Path::from_ident(ident("std", span));
    path.segments
        .push(ast::PathSegment::from_ident(ident("collections", span)));
    let mut segment = ast::PathSegment::from_ident(ident(name, span));
    segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
        span,
        args: leaves
            .iter()
            .map(|leaf| ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty(&list_leaf_type((*leaf).clone()), span))))
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
    if value.carrier != Carrier::Declared || !value.library_item.is_empty() {
        return library_alias(&value.name, value.public, span);
    }
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
pub(super) fn enum_tokens(value: &crate::plan::EnumDeclaration, span: Span) -> ast::tokenstream::LazyAttrTokenStream {
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

/// Name the selected zip storage alias and preserve its checked shared or exclusive reference.
pub(super) fn zip_signature_ty(kind: &PlanType, span: Span) -> Option<Box<ast::Ty>> {
    let (left, right, mutability) = match kind {
        PlanType::ZipIterator(left, right) => (left, right, None),
        PlanType::ZipIteratorRef(left, right) => (left, right, Some(ast::Mutability::Not)),
        PlanType::ZipIteratorMutRef(left, right) => (left, right, Some(ast::Mutability::Mut)),
        _ => return None,
    };
    let path = ast::Path::from_ident(ident(&crate::zip_carriers::alias_name(left, right), span));
    let owned = Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, path),
        span,
        tokens: None,
    });
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

/// Name the linked runtime's generic zip wrapper when no governed source result owns the closed carrier.
pub(crate) fn linked_zip_ty(
    left: &crate::plan::TupleElement,
    right: &crate::plan::TupleElement,
    span: Span,
) -> Box<ast::Ty> {
    let mut path = ast::Path::from_ident(ident("incan_native_runtime", span));
    let mut segment = ast::PathSegment::from_ident(ident("ZipLists", span));
    segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
        span,
        args: thin_vec![
            ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty(&tuple_element_type(left.clone()), span))),
            ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty(&tuple_element_type(right.clone()), span))),
        ],
    })));
    path.segments.push(segment);
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, path),
        span,
        tokens: None,
    })
}

/// Declare the SDK's type marker with erased zero-sized storage; the validated plan retains represented identity.
pub(super) fn type_token_ty(elements: &[PlanType], span: Span) -> Box<ast::Ty> {
    let mut path = ast::Path::from_ident(ident("incan_std_primitives", span));
    path.segments
        .push(ast::PathSegment::from_ident(ident("reflection", span)));
    let mut segment = ast::PathSegment::from_ident(ident("TypeToken", span));
    let arguments = elements
        .iter()
        .map(|_| ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty(&PlanType::Unit, span))))
        .collect();
    segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
        span,
        args: arguments,
    })));
    path.segments.push(segment);
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, path),
        span,
        tokens: None,
    })
}
