//! Decode schema 2 compilation-unit output without exposing the compiler's private declaration model.

use serde::Deserialize;

use crate::ast;

use super::{SourceSpan, SurfaceExpr, SurfaceStatement, inherited_span, statements};

/// One public declaration, independently anchored inside the vocabulary node that produced it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SurfaceDeclaration {
    /// Original source range, falling back to the requesting node.
    span: Option<SourceSpan>,
    /// Declared Incan identifier.
    name: String,
    /// Declaration contract and contents.
    kind: DeclarationKind,
    /// Whether consumers may import the generated declaration.
    is_public: bool,
}

/// Schema 2 declaration alternatives; methods, decorators and type parameters are outside this schema.
#[derive(Deserialize)]
enum DeclarationKind {
    /// Fixed parameters, explicit return type and ordinary body statements.
    Function(Vec<Parameter>, TypeExpr, Vec<SurfaceStatement>),
    /// Public data fields with optional defaults.
    Model(Vec<Field>),
    /// Optional annotation and constant initializer.
    Const(Option<TypeExpr>, SurfaceExpr),
}

/// Named or applied type from `std.vocab.VocabTypeExpr`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeExpr {
    /// Original source range, falling back to the requesting node.
    span: Option<SourceSpan>,
    /// Simple or dotted type name; `None` denotes the unit return type.
    name: String,
    /// Applied type arguments in declaration order.
    arguments: Vec<TypeExpr>,
}

/// Fixed immutable parameter from `std.vocab.VocabParameter`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameter {
    /// Original source range, falling back to the requesting node.
    span: Option<SourceSpan>,
    /// Local parameter identifier.
    name: String,
    /// Optional annotation; absent annotations are inferred by ordinary checking.
    param_type: Option<TypeExpr>,
    /// Optional ordinary default expression.
    default_value: Option<SurfaceExpr>,
}

/// Public model field from `std.vocab.VocabFieldSpec`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Field {
    /// Original source range, falling back to the requesting node.
    span: Option<SourceSpan>,
    /// Declared field identifier.
    name: String,
    /// Required field annotation, validated before constructing compiler syntax.
    field_type: Option<TypeExpr>,
    /// Optional ordinary default expression.
    default_value: Option<SurfaceExpr>,
}

/// Translate declarations after the response's complete source-map and output-position validation.
pub(super) fn declarations(
    values: Vec<SurfaceDeclaration>,
    anchor: ast::Span,
) -> Result<Vec<ast::Spanned<ast::Declaration>>, String> {
    values
        .into_iter()
        .map(|value| {
            let span = inherited_span(value.span, anchor);
            identifier(&value.name)?;
            let visibility = if value.is_public {
                ast::Visibility::Public
            } else {
                ast::Visibility::Private
            };
            let node = match value.kind {
                DeclarationKind::Function(params, result, body) => ast::Declaration::Function(ast::FunctionDecl {
                    visibility,
                    decorators: Vec::new(),
                    surface_modifiers: Vec::new(),
                    name: value.name,
                    type_params: Vec::new(),
                    params: params
                        .into_iter()
                        .map(|param| parameter(param, anchor))
                        .collect::<Result<_, _>>()?,
                    return_type: type_expr(result, anchor)?,
                    body: statements(body, anchor)?,
                }),
                DeclarationKind::Model(fields) => ast::Declaration::Model(ast::ModelDecl {
                    visibility,
                    decorators: Vec::new(),
                    name: value.name,
                    type_params: Vec::new(),
                    traits: Vec::new(),
                    docstring: None,
                    fields: fields
                        .into_iter()
                        .map(|value| field(value, anchor))
                        .collect::<Result<_, _>>()?,
                    method_aliases: Vec::new(),
                    method_partials: Vec::new(),
                    properties: Vec::new(),
                    methods: Vec::new(),
                }),
                DeclarationKind::Const(ty, expr) => ast::Declaration::Const(ast::ConstDecl {
                    visibility,
                    name: value.name,
                    ty: ty.map(|ty| type_expr(ty, anchor)).transpose()?,
                    value: expr.into_ast(anchor)?,
                }),
            };
            Ok(ast::Spanned::new(node, span))
        })
        .collect()
}

/// Preserve named and applied type syntax; source spellings grant no resolved type or storage authority.
fn type_expr(value: TypeExpr, anchor: ast::Span) -> Result<ast::Spanned<ast::Type>, String> {
    let span = inherited_span(value.span, anchor);
    let parts = value.name.split('.').map(str::to_owned).collect::<Vec<_>>();
    for part in &parts {
        identifier(part)?;
    }
    let arguments = value
        .arguments
        .into_iter()
        .map(|ty| type_expr(ty, anchor))
        .collect::<Result<Vec<_>, _>>()?;
    let node = match (parts.as_slice(), arguments.is_empty()) {
        ([name], true) if name == "None" => ast::Type::Unit,
        ([name], true) => ast::Type::Simple(name.clone()),
        ([name], false) => ast::Type::Generic(name.clone(), arguments),
        (_, true) => ast::Type::Dotted(parts),
        (_, false) => ast::Type::DottedGeneric(parts, arguments),
    };
    Ok(ast::Spanned::new(node, span))
}

/// Translate one ordinary immutable parameter, retaining independent annotation and default locations.
fn parameter(value: Parameter, anchor: ast::Span) -> Result<ast::Spanned<ast::Param>, String> {
    let span = inherited_span(value.span, anchor);
    identifier(&value.name)?;
    let ty = value.param_type.map(|ty| type_expr(ty, anchor)).transpose()?;
    Ok(ast::Spanned::new(
        ast::Param {
            is_mut: false,
            kind: ast::ParamKind::Normal,
            name: value.name,
            ty: ty.unwrap_or_else(|| ast::Spanned::new(ast::Type::Infer, span.expansion_child("inferred-type"))),
            default: value.default_value.map(|expr| expr.into_ast(anchor)).transpose()?,
        },
        span,
    ))
}

/// Require a model field's type instead of inventing a semantic contract for untyped storage.
fn field(value: Field, anchor: ast::Span) -> Result<ast::Spanned<ast::FieldDecl>, String> {
    let span = inherited_span(value.span, anchor);
    identifier(&value.name)?;
    let ty = value
        .field_type
        .ok_or("malformed response: model field requires a type")?;
    Ok(ast::Spanned::new(
        ast::FieldDecl {
            visibility: ast::Visibility::Public,
            name: value.name,
            metadata: ast::FieldMetadata::default(),
            ty: type_expr(ty, anchor)?,
            default: value.default_value.map(|expr| expr.into_ast(anchor)).transpose()?,
        },
        span,
    ))
}

/// Refuse malformed identifiers before they can become declaration or type keys in ordinary checking.
fn identifier(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    if chars.next().is_some_and(|first| first == '_' || first.is_alphabetic())
        && chars.all(|character| character == '_' || character.is_alphanumeric())
    {
        return Ok(());
    }
    Err("malformed response: expected an Incan identifier".into())
}
