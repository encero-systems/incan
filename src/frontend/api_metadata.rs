//! Checked public API metadata extraction for RFC 048.
//!
//! This module builds a JSON-ready model from parsed and typechecked Incan semantics. It deliberately reuses the
//! manifest type vocabulary instead of stringifying checked types, so package artifacts, CLI output, and later docs
//! tooling can share one structural representation.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::frontend::ast::{
    ClassDecl, Declaration, Decorator, DecoratorArg, DecoratorArgValue, EnumDecl, Expr, FieldDecl, FunctionDecl,
    ImportDecl, ImportItem, ImportKind, MethodDecl, ModelDecl, NewtypeDecl, Program, Span, Spanned, Statement,
    TraitDecl, TypeAliasDecl, Visibility,
};
use crate::frontend::decorator_resolution;
use crate::frontend::library_exports::{
    CheckedClassExport, CheckedConstExport, CheckedEnumExport, CheckedExportKind, CheckedField, CheckedFunctionExport,
    CheckedMethod, CheckedModelExport, CheckedNamedExport, CheckedNewtypeExport, CheckedTraitExport,
    CheckedTypeAliasExport, CheckedTypeBound, CheckedTypeParam, collect_checked_public_exports,
};
use crate::frontend::typechecker::{ConstValue, TypeChecker};
use crate::library_manifest::{
    EnumValueExport, EnumValueTypeExport, FieldExport, ParamExport, ParamKindExport, ReceiverExport, TypeAliasExport,
    TypeBoundExport, TypeParamExport, TypeRef, type_ref_from_resolved,
};

pub const CHECKED_API_METADATA_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckedApiMetadataPackage {
    pub schema_version: u32,
    pub modules: Vec<CheckedApiMetadata>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckedApiMetadata {
    pub schema_version: u32,
    pub module_path: Vec<String>,
    pub declarations: Vec<ApiDeclaration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceAnchor {
    pub id: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApiDeclaration {
    Function(ApiFunction),
    Model(ApiModel),
    Class(ApiClass),
    Trait(ApiTrait),
    Enum(ApiEnum),
    Newtype(ApiNewtype),
    TypeAlias(ApiTypeAlias),
    Const(ApiConst),
    Static(ApiStatic),
    Alias(ApiAlias),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiFunction {
    pub name: String,
    pub anchor: SourceAnchor,
    pub docstring: Option<String>,
    pub decorators: Vec<DecoratorMetadata>,
    pub type_params: Vec<TypeParamExport>,
    pub params: Vec<ParamExport>,
    pub return_type: TypeRef,
    pub is_async: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiModel {
    pub name: String,
    pub anchor: SourceAnchor,
    pub docstring: Option<String>,
    pub decorators: Vec<DecoratorMetadata>,
    pub type_params: Vec<TypeParamExport>,
    pub traits: Vec<String>,
    pub derives: Vec<String>,
    pub fields: Vec<FieldExport>,
    pub methods: Vec<ApiMethod>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiClass {
    pub name: String,
    pub anchor: SourceAnchor,
    pub docstring: Option<String>,
    pub decorators: Vec<DecoratorMetadata>,
    pub type_params: Vec<TypeParamExport>,
    pub extends: Option<String>,
    pub traits: Vec<String>,
    pub derives: Vec<String>,
    pub fields: Vec<FieldExport>,
    pub methods: Vec<ApiMethod>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiTrait {
    pub name: String,
    pub anchor: SourceAnchor,
    pub docstring: Option<String>,
    pub decorators: Vec<DecoratorMetadata>,
    pub type_params: Vec<TypeParamExport>,
    pub supertraits: Vec<TypeBoundExport>,
    pub requires: Vec<FieldExport>,
    pub methods: Vec<ApiMethod>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiEnum {
    pub name: String,
    pub anchor: SourceAnchor,
    pub docstring: Option<String>,
    pub decorators: Vec<DecoratorMetadata>,
    pub type_params: Vec<TypeParamExport>,
    pub value_type: Option<EnumValueTypeExport>,
    pub variants: Vec<ApiEnumVariant>,
    pub derives: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiEnumVariant {
    pub name: String,
    pub fields: Vec<TypeRef>,
    pub value: Option<EnumValueExport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiNewtype {
    pub name: String,
    pub anchor: SourceAnchor,
    pub docstring: Option<String>,
    pub decorators: Vec<DecoratorMetadata>,
    pub type_params: Vec<TypeParamExport>,
    pub is_rusttype: bool,
    pub underlying: TypeRef,
    pub methods: Vec<ApiMethod>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiTypeAlias {
    pub name: String,
    pub anchor: SourceAnchor,
    pub type_alias: TypeAliasExport,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiConst {
    pub name: String,
    pub anchor: SourceAnchor,
    pub ty: TypeRef,
    pub value: Option<SafeMetadataValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiStatic {
    pub name: String,
    pub anchor: SourceAnchor,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiAlias {
    pub name: String,
    pub anchor: SourceAnchor,
    pub target_path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiMethod {
    pub name: String,
    pub anchor: SourceAnchor,
    pub docstring: Option<String>,
    pub decorators: Vec<DecoratorMetadata>,
    pub type_params: Vec<TypeParamExport>,
    pub receiver: Option<ReceiverExport>,
    pub params: Vec<ParamExport>,
    pub return_type: TypeRef,
    pub is_async: bool,
    pub has_body: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecoratorMetadata {
    pub path: Vec<String>,
    pub source_name: String,
    pub anchor: SourceSpan,
    pub args: Vec<DecoratorArgMetadata>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecoratorArgMetadata {
    Positional { value: DecoratorValue },
    Named { name: String, value: DecoratorValue },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecoratorValue {
    Literal {
        value: SafeMetadataValue,
    },
    ConstRef {
        name: String,
        value: Option<SafeMetadataValue>,
    },
    Type {
        ty: TypeRef,
    },
    Unsupported {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SafeMetadataValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
    None,
}

pub fn collect_checked_api_metadata(
    program: &Program,
    checker: &TypeChecker,
    module_path: Vec<String>,
) -> CheckedApiMetadata {
    let checked_exports = collect_checked_public_exports(program, checker);
    let checked_by_name: HashMap<String, CheckedNamedExport> = checked_exports
        .into_iter()
        .map(|export| (export.name.clone(), export))
        .collect();

    let mut declarations = Vec::new();
    for decl in &program.declarations {
        match &decl.node {
            Declaration::Function(function) if public(function.visibility) => {
                if let Some(CheckedExportKind::Function(export)) = checked_kind(&checked_by_name, &function.name) {
                    declarations.push(ApiDeclaration::Function(api_function(
                        function,
                        decl.span,
                        export,
                        checker,
                        &module_path,
                    )));
                }
            }
            Declaration::Model(model) if public(model.visibility) => {
                if let Some(CheckedExportKind::Model(export)) = checked_kind(&checked_by_name, &model.name) {
                    declarations.push(ApiDeclaration::Model(api_model(
                        model,
                        decl.span,
                        export,
                        checker,
                        &module_path,
                    )));
                }
            }
            Declaration::Class(class) if public(class.visibility) => {
                if let Some(CheckedExportKind::Class(export)) = checked_kind(&checked_by_name, &class.name) {
                    declarations.push(ApiDeclaration::Class(api_class(
                        class,
                        decl.span,
                        export,
                        checker,
                        &module_path,
                    )));
                }
            }
            Declaration::Trait(trait_decl) if public(trait_decl.visibility) => {
                if let Some(CheckedExportKind::Trait(export)) = checked_kind(&checked_by_name, &trait_decl.name) {
                    declarations.push(ApiDeclaration::Trait(api_trait(
                        trait_decl,
                        decl.span,
                        export,
                        checker,
                        &module_path,
                    )));
                }
            }
            Declaration::Enum(enum_decl) if public(enum_decl.visibility) => {
                if let Some(CheckedExportKind::Enum(export)) = checked_kind(&checked_by_name, &enum_decl.name) {
                    declarations.push(ApiDeclaration::Enum(api_enum(
                        enum_decl,
                        decl.span,
                        export,
                        checker,
                        &module_path,
                    )));
                }
            }
            Declaration::Newtype(newtype) if public(newtype.visibility) => {
                if let Some(CheckedExportKind::Newtype(export)) = checked_kind(&checked_by_name, &newtype.name) {
                    declarations.push(ApiDeclaration::Newtype(api_newtype(
                        newtype,
                        decl.span,
                        export,
                        checker,
                        &module_path,
                    )));
                }
            }
            Declaration::TypeAlias(alias) if public(alias.visibility) => {
                if let Some(CheckedExportKind::TypeAlias(export)) = checked_kind(&checked_by_name, &alias.name) {
                    declarations.push(ApiDeclaration::TypeAlias(api_type_alias(
                        alias,
                        decl.span,
                        export,
                        &module_path,
                    )));
                }
            }
            Declaration::Const(konst) if public(konst.visibility) => {
                if let Some(CheckedExportKind::Const(export)) = checked_kind(&checked_by_name, &konst.name) {
                    declarations.push(ApiDeclaration::Const(api_const(
                        &konst.name,
                        decl.span,
                        export,
                        checker,
                        &module_path,
                    )));
                }
            }
            Declaration::Static(static_decl) if public(static_decl.visibility) => {
                if let Some(CheckedExportKind::Static(export)) = checked_kind(&checked_by_name, &static_decl.name) {
                    declarations.push(ApiDeclaration::Static(ApiStatic {
                        name: export.name.clone(),
                        anchor: anchor(&module_path, &export.name, decl.span),
                        ty: type_ref_from_resolved(&export.ty),
                    }));
                }
            }
            Declaration::Import(import) if public(import.visibility) => {
                declarations.extend(
                    api_aliases(import, decl.span, &module_path)
                        .into_iter()
                        .map(ApiDeclaration::Alias),
                );
            }
            _ => {}
        }
    }

    CheckedApiMetadata {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        module_path,
        declarations,
    }
}

fn checked_kind<'a>(exports: &'a HashMap<String, CheckedNamedExport>, name: &str) -> Option<&'a CheckedExportKind> {
    exports.get(name).map(|export| &export.kind)
}

fn public(visibility: Visibility) -> bool {
    matches!(visibility, Visibility::Public)
}

fn api_function(
    function: &FunctionDecl,
    span: Span,
    export: &CheckedFunctionExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiFunction {
    ApiFunction {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring: function_docstring(&function.body),
        decorators: decorators_metadata(&function.decorators, checker),
        type_params: type_params(&export.type_params),
        params: params(&export.params),
        return_type: type_ref_from_resolved(&export.return_type),
        is_async: export.is_async,
    }
}

fn api_model(
    model: &ModelDecl,
    span: Span,
    export: &CheckedModelExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiModel {
    ApiModel {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring: model.docstring.clone(),
        decorators: decorators_metadata(&model.decorators, checker),
        type_params: type_params(&export.type_params),
        traits: export.traits.clone(),
        derives: export.derives.clone(),
        fields: fields_in_source_order(&model.fields, &export.fields),
        methods: methods(&model.methods, &export.methods, checker, module_path, &export.name),
    }
}

fn api_class(
    class: &ClassDecl,
    span: Span,
    export: &CheckedClassExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiClass {
    ApiClass {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring: class.docstring.clone(),
        decorators: decorators_metadata(&class.decorators, checker),
        type_params: type_params(&export.type_params),
        extends: export.extends.clone(),
        traits: export.traits.clone(),
        derives: export.derives.clone(),
        fields: fields_in_source_order(&class.fields, &export.fields),
        methods: methods(&class.methods, &export.methods, checker, module_path, &export.name),
    }
}

fn api_trait(
    trait_decl: &TraitDecl,
    span: Span,
    export: &CheckedTraitExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiTrait {
    ApiTrait {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring: trait_decl.docstring.clone(),
        decorators: decorators_metadata(&trait_decl.decorators, checker),
        type_params: type_params(&export.type_params),
        supertraits: export
            .supertraits
            .iter()
            .map(|(name, args)| TypeBoundExport {
                name: name.clone(),
                type_args: args.iter().map(type_ref_from_resolved).collect(),
            })
            .collect(),
        requires: export
            .requires
            .iter()
            .map(|(name, ty)| FieldExport {
                name: name.clone(),
                ty: type_ref_from_resolved(ty),
                has_default: false,
                alias: None,
                description: None,
            })
            .collect(),
        methods: methods(&trait_decl.methods, &export.methods, checker, module_path, &export.name),
    }
}

fn api_enum(
    enum_decl: &EnumDecl,
    span: Span,
    export: &CheckedEnumExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiEnum {
    ApiEnum {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring: enum_decl.docstring.clone(),
        decorators: decorators_metadata(&enum_decl.decorators, checker),
        type_params: type_params(&export.type_params),
        value_type: export.value_type.map(|value_type| match value_type {
            crate::frontend::symbols::ValueEnumBacking::Str => EnumValueTypeExport::Str,
            crate::frontend::symbols::ValueEnumBacking::Int => EnumValueTypeExport::Int,
        }),
        variants: export
            .variants
            .iter()
            .map(|variant| ApiEnumVariant {
                name: variant.name.clone(),
                fields: variant.fields.iter().map(type_ref_from_resolved).collect(),
                value: variant.value.as_ref().map(|value| match value {
                    crate::frontend::symbols::ValueEnumValue::Str(value) => EnumValueExport::Str(value.clone()),
                    crate::frontend::symbols::ValueEnumValue::Int(value) => EnumValueExport::Int(*value),
                }),
            })
            .collect(),
        derives: export.derives.clone(),
    }
}

fn api_newtype(
    newtype: &NewtypeDecl,
    span: Span,
    export: &CheckedNewtypeExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiNewtype {
    ApiNewtype {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring: newtype.docstring.clone(),
        decorators: decorators_metadata(&newtype.decorators, checker),
        type_params: type_params(&export.type_params),
        is_rusttype: export.is_rusttype,
        underlying: type_ref_from_resolved(&export.underlying),
        methods: methods(&newtype.methods, &export.methods, checker, module_path, &export.name),
    }
}

fn api_type_alias(
    alias: &TypeAliasDecl,
    span: Span,
    export: &CheckedTypeAliasExport,
    module_path: &[String],
) -> ApiTypeAlias {
    ApiTypeAlias {
        name: alias.name.clone(),
        anchor: anchor(module_path, &alias.name, span),
        type_alias: TypeAliasExport {
            name: export.name.clone(),
            type_params: type_params(&export.type_params),
            target: type_ref_from_resolved(&export.target),
        },
    }
}

fn api_const(
    name: &str,
    span: Span,
    export: &CheckedConstExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiConst {
    ApiConst {
        name: export.name.clone(),
        anchor: anchor(module_path, name, span),
        ty: type_ref_from_resolved(&export.ty),
        value: checker.type_info().const_value(name).map(safe_value_from_const),
    }
}

fn api_aliases(import: &ImportDecl, span: Span, module_path: &[String]) -> Vec<ApiAlias> {
    match &import.kind {
        ImportKind::From { module, items } => {
            let base_path = decorator_resolution::path_segments_with_prefix(module);
            aliases_from_items(items, base_path, span, module_path)
        }
        ImportKind::RustFrom {
            crate_name,
            path,
            items,
            ..
        } => {
            let mut base_path = vec!["rust".to_string(), crate_name.clone()];
            base_path.extend(path.iter().cloned());
            aliases_from_items(items, base_path, span, module_path)
        }
        ImportKind::PubFrom { library, items } => {
            let base_path = vec!["pub".to_string(), library.clone()];
            aliases_from_items(items, base_path, span, module_path)
        }
        _ => Vec::new(),
    }
}

fn aliases_from_items(
    items: &[ImportItem],
    base_path: Vec<String>,
    span: Span,
    module_path: &[String],
) -> Vec<ApiAlias> {
    items
        .iter()
        .map(|item| {
            let name = item.alias.as_ref().unwrap_or(&item.name).clone();
            let mut target_path = base_path.clone();
            target_path.push(item.name.clone());
            ApiAlias {
                anchor: anchor(module_path, &name, span),
                name,
                target_path,
            }
        })
        .collect()
}

fn methods(
    ast_methods: &[Spanned<MethodDecl>],
    checked_methods: &[CheckedMethod],
    checker: &TypeChecker,
    module_path: &[String],
    owner: &str,
) -> Vec<ApiMethod> {
    let checked_by_name: HashMap<&str, &CheckedMethod> = checked_methods
        .iter()
        .map(|method| (method.name.as_str(), method))
        .collect();
    let mut out = Vec::new();
    for method in ast_methods {
        let Some(checked) = checked_by_name.get(method.node.name.as_str()) else {
            continue;
        };
        out.push(ApiMethod {
            name: checked.name.clone(),
            anchor: anchor(module_path, &format!("{owner}.{}", checked.name), method.span),
            docstring: method.node.body.as_ref().and_then(|body| function_docstring(body)),
            decorators: decorators_metadata(&method.node.decorators, checker),
            type_params: type_params(&checked.type_params),
            receiver: checked.receiver.map(|receiver| match receiver {
                crate::frontend::ast::Receiver::Immutable => ReceiverExport::Immutable,
                crate::frontend::ast::Receiver::Mutable => ReceiverExport::Mutable,
            }),
            params: params(&checked.params),
            return_type: type_ref_from_resolved(&checked.return_type),
            is_async: checked.is_async,
            has_body: checked.has_body,
        });
    }
    out
}

fn type_params(type_params: &[CheckedTypeParam]) -> Vec<TypeParamExport> {
    type_params
        .iter()
        .map(|type_param| TypeParamExport {
            name: type_param.name.clone(),
            bounds: type_param.bounds.iter().map(type_bound).collect(),
        })
        .collect()
}

fn type_bound(bound: &CheckedTypeBound) -> TypeBoundExport {
    TypeBoundExport {
        name: bound.name.clone(),
        type_args: bound.type_args.iter().map(type_ref_from_resolved).collect(),
    }
}

fn params(params: &[crate::frontend::symbols::CallableParam]) -> Vec<ParamExport> {
    params
        .iter()
        .filter_map(|param| {
            Some(ParamExport {
                name: param.name.clone()?,
                ty: type_ref_from_resolved(&param.ty),
                kind: match param.kind {
                    crate::frontend::ast::ParamKind::Normal => ParamKindExport::Normal,
                    crate::frontend::ast::ParamKind::RestPositional => ParamKindExport::RestPositional,
                    crate::frontend::ast::ParamKind::RestKeyword => ParamKindExport::RestKeyword,
                },
                has_default: param.has_default,
            })
        })
        .collect()
}

fn field(field: &crate::frontend::library_exports::CheckedField) -> FieldExport {
    FieldExport {
        name: field.name.clone(),
        ty: type_ref_from_resolved(&field.ty),
        has_default: field.has_default,
        alias: field.alias.clone(),
        description: field.description.clone(),
    }
}

fn fields_in_source_order(ast_fields: &[Spanned<FieldDecl>], checked_fields: &[CheckedField]) -> Vec<FieldExport> {
    let checked_by_name: HashMap<&str, &CheckedField> = checked_fields
        .iter()
        .map(|field| (field.name.as_str(), field))
        .collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for ast_field in ast_fields {
        if let Some(checked) = checked_by_name.get(ast_field.node.name.as_str()) {
            seen.insert(checked.name.as_str());
            out.push(field(checked));
        }
    }

    for checked in checked_fields {
        if seen.insert(checked.name.as_str()) {
            out.push(field(checked));
        }
    }

    out
}

fn decorators_metadata(decorators: &[Spanned<Decorator>], checker: &TypeChecker) -> Vec<DecoratorMetadata> {
    decorators
        .iter()
        .map(|decorator| {
            let resolved = decorator_resolution::resolve_decorator_path(&decorator.node, &checker.import_aliases);
            DecoratorMetadata {
                path: resolved,
                source_name: decorator.node.path.segments.join("."),
                anchor: source_span(decorator.span),
                args: decorator
                    .node
                    .args
                    .iter()
                    .map(|arg| decorator_arg_metadata(arg, checker))
                    .collect(),
            }
        })
        .collect()
}

fn decorator_arg_metadata(arg: &DecoratorArg, checker: &TypeChecker) -> DecoratorArgMetadata {
    match arg {
        DecoratorArg::Positional(expr) => DecoratorArgMetadata::Positional {
            value: decorator_expr_value(expr, checker),
        },
        DecoratorArg::Named(name, DecoratorArgValue::Expr(expr)) => DecoratorArgMetadata::Named {
            name: name.clone(),
            value: decorator_expr_value(expr, checker),
        },
        DecoratorArg::Named(name, DecoratorArgValue::Type(ty)) => DecoratorArgMetadata::Named {
            name: name.clone(),
            value: DecoratorValue::Type {
                ty: type_ref_from_resolved(&crate::frontend::symbols::resolve_type(&ty.node, &checker.symbols)),
            },
        },
    }
}

fn decorator_expr_value(expr: &Spanned<Expr>, checker: &TypeChecker) -> DecoratorValue {
    match &expr.node {
        Expr::Literal(literal) => DecoratorValue::Literal {
            value: safe_value_from_literal(literal),
        },
        Expr::Ident(name) => DecoratorValue::ConstRef {
            name: name.clone(),
            value: checker.type_info().const_value(name).map(safe_value_from_const),
        },
        _ => DecoratorValue::Unsupported {
            reason: "decorator argument is not a literal, const reference, or type".to_string(),
        },
    }
}

fn safe_value_from_literal(literal: &crate::frontend::ast::Literal) -> SafeMetadataValue {
    match literal {
        crate::frontend::ast::Literal::Int(value) => SafeMetadataValue::Int(value.value),
        crate::frontend::ast::Literal::Float(value) => SafeMetadataValue::Float(value.value),
        crate::frontend::ast::Literal::String(value) => SafeMetadataValue::String(value.clone()),
        crate::frontend::ast::Literal::Bytes(value) => SafeMetadataValue::Bytes(value.clone()),
        crate::frontend::ast::Literal::Bool(value) => SafeMetadataValue::Bool(*value),
        crate::frontend::ast::Literal::None => SafeMetadataValue::None,
    }
}

fn safe_value_from_const(value: &ConstValue) -> SafeMetadataValue {
    match value {
        ConstValue::Int(value) => SafeMetadataValue::Int(*value),
        ConstValue::Float(value) => SafeMetadataValue::Float(*value),
        ConstValue::Bool(value) => SafeMetadataValue::Bool(*value),
        ConstValue::FrozenStr(value) => SafeMetadataValue::String(value.clone()),
        ConstValue::FrozenBytes(value) => SafeMetadataValue::Bytes(value.clone()),
    }
}

fn function_docstring(body: &[Spanned<Statement>]) -> Option<String> {
    let first = body.first()?;
    let Statement::Expr(expr) = &first.node else {
        return None;
    };
    let Expr::Literal(crate::frontend::ast::Literal::String(docstring)) = &expr.node else {
        return None;
    };
    Some(docstring.clone())
}

fn anchor(module_path: &[String], name: &str, span: Span) -> SourceAnchor {
    let mut parts = module_path.to_vec();
    parts.push(name.to_string());
    SourceAnchor {
        id: parts.join("::"),
        span: source_span(span),
    }
}

fn source_span(span: Span) -> SourceSpan {
    SourceSpan {
        start: span.start,
        end: span.end,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{lexer, parser, typechecker};

    fn metadata_for(source: &str) -> Result<CheckedApiMetadata, Vec<crate::frontend::diagnostics::CompileError>> {
        let tokens = lexer::lex(source)?;
        let program = parser::parse(&tokens)?;
        let mut checker = typechecker::TypeChecker::new();
        checker.check_program(&program)?;
        Ok(collect_checked_api_metadata(
            &program,
            &checker,
            vec!["demo".to_string()],
        ))
    }

    fn metadata_for_src_lib(
        source: &str,
    ) -> Result<CheckedApiMetadata, Vec<crate::frontend::diagnostics::CompileError>> {
        let tokens = lexer::lex(source)?;
        let program = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))?;
        let mut checker = typechecker::TypeChecker::new();
        checker.check_program(&program)?;
        Ok(collect_checked_api_metadata(
            &program,
            &checker,
            vec!["lib".to_string()],
        ))
    }

    #[test]
    fn checked_api_metadata_extracts_function_decorator_and_docstring() -> Result<(), String> {
        let source = r#"
@rust.allow("dead_code")
pub def avg(values: List[float]) -> float:
    """
    Return the arithmetic mean.
    """
    return 0.0
"#;
        let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
        let function = metadata
            .declarations
            .iter()
            .find_map(|decl| match decl {
                ApiDeclaration::Function(function) => Some(function),
                _ => None,
            })
            .ok_or_else(|| "expected function metadata".to_string())?;

        assert_eq!(function.name, "avg");
        assert_eq!(function.anchor.id, "demo::avg");
        assert_eq!(
            function.docstring.as_deref().map(str::trim),
            Some("Return the arithmetic mean.")
        );
        assert_eq!(function.params.len(), 1);
        assert_eq!(function.decorators.len(), 1);
        assert_eq!(
            function.decorators[0].path,
            vec!["rust".to_string(), "allow".to_string()]
        );
        assert_eq!(
            function.decorators[0].args,
            vec![DecoratorArgMetadata::Positional {
                value: DecoratorValue::Literal {
                    value: SafeMetadataValue::String("dead_code".to_string()),
                },
            }]
        );
        Ok(())
    }

    #[test]
    fn checked_api_metadata_extracts_model_fields_methods_and_const_values() -> Result<(), String> {
        let source = r#"
pub const DEFAULT_LABEL = "none"

@derive(Clone)
pub model Order:
    """
    Order contract.
    """
    id [description="Stable id"] as "orderId": int
    label: str = DEFAULT_LABEL

    def label(self) -> str:
        """
        Return the display label.
        """
        return DEFAULT_LABEL
"#;
        let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
        let konst = metadata
            .declarations
            .iter()
            .find_map(|decl| match decl {
                ApiDeclaration::Const(konst) => Some(konst),
                _ => None,
            })
            .ok_or_else(|| "expected const metadata".to_string())?;
        assert_eq!(konst.value, Some(SafeMetadataValue::String("none".to_string())));

        let model = metadata
            .declarations
            .iter()
            .find_map(|decl| match decl {
                ApiDeclaration::Model(model) => Some(model),
                _ => None,
            })
            .ok_or_else(|| "expected model metadata".to_string())?;
        assert_eq!(model.docstring.as_deref().map(str::trim), Some("Order contract."));
        assert_eq!(
            model.fields.iter().map(|field| field.name.as_str()).collect::<Vec<_>>(),
            vec!["id", "label"]
        );
        assert_eq!(model.fields[0].alias.as_deref(), Some("orderId"));
        assert_eq!(model.fields[0].description.as_deref(), Some("Stable id"));
        assert_eq!(
            model.methods[0].docstring.as_deref().map(str::trim),
            Some("Return the display label.")
        );
        Ok(())
    }

    #[test]
    fn checked_api_metadata_extracts_public_import_alias_targets() -> Result<(), String> {
        let source = r#"
pub from crate.widgets import Widget as PublicWidget
"#;
        let metadata = metadata_for_src_lib(source).map_err(|errs| format!("{errs:?}"))?;
        let alias = metadata
            .declarations
            .iter()
            .find_map(|decl| match decl {
                ApiDeclaration::Alias(alias) => Some(alias),
                _ => None,
            })
            .ok_or_else(|| "expected alias metadata".to_string())?;

        assert_eq!(alias.name, "PublicWidget");
        assert_eq!(alias.anchor.id, "lib::PublicWidget");
        assert_eq!(
            alias.target_path,
            vec!["crate".to_string(), "widgets".to_string(), "Widget".to_string()]
        );
        Ok(())
    }
}
