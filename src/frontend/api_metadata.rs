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
use crate::frontend::diagnostics::CompileError;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<CheckedApiPackageIdentity>,
    pub modules: Vec<CheckedApiMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedApiPackageIdentity {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
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
    /// Parsed structured docstring sections, when a docstring is present.
    pub docstring_sections: Option<ApiDocstring>,
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
    /// Parsed structured docstring sections, when a docstring is present.
    pub docstring_sections: Option<ApiDocstring>,
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
    /// Parsed structured docstring sections, when a docstring is present.
    pub docstring_sections: Option<ApiDocstring>,
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
    /// Parsed structured docstring sections, when a docstring is present.
    pub docstring_sections: Option<ApiDocstring>,
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
    /// Parsed structured docstring sections, when a docstring is present.
    pub docstring_sections: Option<ApiDocstring>,
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
    /// Parsed structured docstring sections, when a docstring is present.
    pub docstring_sections: Option<ApiDocstring>,
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
    /// Parsed structured docstring sections, when a docstring is present.
    pub docstring_sections: Option<ApiDocstring>,
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

/// Parsed Incan docstring sections attached to a checked API declaration or method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiDocstring {
    /// Free-form summary text before the first recognized section heading.
    pub summary: Option<String>,
    /// Documented callable parameters from `Args:` or `Parameters:`.
    pub params: Vec<ApiDocstringEntry>,
    /// Documented return section from `Returns:`.
    pub returns: Option<ApiDocstringReturn>,
    /// Documented model, class, or trait fields from `Fields:`.
    pub fields: Vec<ApiDocstringEntry>,
    /// Documented public aliases from `Aliases:`.
    pub aliases: Vec<ApiDocstringEntry>,
    /// Documented decorators from `Decorators:`.
    pub decorators: Vec<ApiDocstringEntry>,
}

/// One named docstring entry from a mechanically checkable section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiDocstringEntry {
    /// The documented parameter, field, alias, or decorator name.
    pub name: String,
    /// Human-readable prose associated with the documented name.
    pub description: String,
}

/// Parsed return documentation, optionally carrying an authored type spelling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiDocstringReturn {
    /// Optional type spelling from a `Returns:` line of the form `type: description`.
    pub ty: Option<String>,
    /// Human-readable return prose.
    pub description: String,
}

/// One actionable docstring drift diagnostic associated with a metadata module.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiDocstringDiagnostic {
    /// Logical module path for the declaration that emitted the diagnostic.
    pub module_path: Vec<String>,
    /// Source diagnostic anchored to the declaration or method span.
    pub error: CompileError,
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
    let docstring = function_docstring(&function.body);
    ApiFunction {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring_sections: parse_docstring(docstring.as_deref()),
        docstring,
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
    let docstring = model.docstring.clone();
    ApiModel {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring_sections: parse_docstring(docstring.as_deref()),
        docstring,
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
    let docstring = class.docstring.clone();
    ApiClass {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring_sections: parse_docstring(docstring.as_deref()),
        docstring,
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
    let docstring = trait_decl.docstring.clone();
    ApiTrait {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring_sections: parse_docstring(docstring.as_deref()),
        docstring,
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
    let docstring = enum_decl.docstring.clone();
    ApiEnum {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring_sections: parse_docstring(docstring.as_deref()),
        docstring,
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
    let docstring = newtype.docstring.clone();
    ApiNewtype {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        docstring_sections: parse_docstring(docstring.as_deref()),
        docstring,
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
        let docstring = method.node.body.as_ref().and_then(|body| function_docstring(body));
        out.push(ApiMethod {
            name: checked.name.clone(),
            anchor: anchor(module_path, &format!("{owner}.{}", checked.name), method.span),
            docstring_sections: parse_docstring(docstring.as_deref()),
            docstring,
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

/// Validate parsed API docstrings across a checked metadata package.
pub fn validate_checked_api_docstrings(package: &[CheckedApiMetadata]) -> Vec<ApiDocstringDiagnostic> {
    let aliases = package_aliases(package);
    let mut diagnostics = Vec::new();
    for module in package {
        validate_module_docstrings(module, &aliases, &mut diagnostics);
    }
    diagnostics
}

fn parse_docstring(docstring: Option<&str>) -> Option<ApiDocstring> {
    let docstring = docstring?;
    let lines = normalized_docstring_lines(docstring);
    if lines.is_empty() {
        return None;
    }

    let mut parsed = DocstringBuilder::default();
    let mut section = DocstringSection::Summary;
    for line in lines {
        if let Some(next_section) = DocstringSection::from_heading(&line) {
            section = next_section;
            continue;
        }
        parsed.push_line(section, &line);
    }
    Some(parsed.finish())
}

fn normalized_docstring_lines(docstring: &str) -> Vec<String> {
    docstring
        .lines()
        .map(str::trim)
        .skip_while(|line| line.is_empty())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .skip_while(|line| line.is_empty())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocstringSection {
    Summary,
    Params,
    Returns,
    Fields,
    Aliases,
    Decorators,
}

impl DocstringSection {
    fn from_heading(line: &str) -> Option<Self> {
        match line {
            "Args:" | "Parameters:" => Some(Self::Params),
            "Returns:" => Some(Self::Returns),
            "Fields:" => Some(Self::Fields),
            "Aliases:" => Some(Self::Aliases),
            "Decorators:" => Some(Self::Decorators),
            _ => None,
        }
    }
}

#[derive(Default)]
struct DocstringBuilder {
    summary_lines: Vec<String>,
    params: Vec<ApiDocstringEntry>,
    returns: Vec<String>,
    fields: Vec<ApiDocstringEntry>,
    aliases: Vec<ApiDocstringEntry>,
    decorators: Vec<ApiDocstringEntry>,
}

impl DocstringBuilder {
    fn push_line(&mut self, section: DocstringSection, line: &str) {
        match section {
            DocstringSection::Summary => push_prose_line(&mut self.summary_lines, line),
            DocstringSection::Params => push_entry_line(&mut self.params, line),
            DocstringSection::Returns => push_prose_line(&mut self.returns, line),
            DocstringSection::Fields => push_entry_line(&mut self.fields, line),
            DocstringSection::Aliases => push_entry_line(&mut self.aliases, line),
            DocstringSection::Decorators => push_entry_line(&mut self.decorators, line),
        }
    }

    fn finish(self) -> ApiDocstring {
        ApiDocstring {
            summary: joined_non_empty(self.summary_lines),
            params: self.params,
            returns: parse_return_section(self.returns),
            fields: self.fields,
            aliases: self.aliases,
            decorators: self.decorators,
        }
    }
}

fn push_prose_line(lines: &mut Vec<String>, line: &str) {
    if line.is_empty() {
        if !lines.last().is_some_and(String::is_empty) {
            lines.push(String::new());
        }
        return;
    }
    lines.push(line.to_string());
}

fn push_entry_line(entries: &mut Vec<ApiDocstringEntry>, line: &str) {
    if line.is_empty() {
        return;
    }
    if let Some((name, description)) = line.split_once(':') {
        let name = name.trim();
        if !name.is_empty() {
            entries.push(ApiDocstringEntry {
                name: name.to_string(),
                description: description.trim().to_string(),
            });
            return;
        }
    }
    if let Some(last) = entries.last_mut() {
        if !last.description.is_empty() {
            last.description.push(' ');
        }
        last.description.push_str(line.trim());
    }
}

fn parse_return_section(lines: Vec<String>) -> Option<ApiDocstringReturn> {
    let description = joined_non_empty(lines)?;
    if let Some((ty, rest)) = description.split_once(':') {
        let ty = ty.trim();
        if looks_like_type_spelling(ty) {
            return Some(ApiDocstringReturn {
                ty: Some(ty.to_string()),
                description: rest.trim().to_string(),
            });
        }
    }
    Some(ApiDocstringReturn { ty: None, description })
}

fn joined_non_empty(lines: Vec<String>) -> Option<String> {
    let joined = lines.join("\n").trim().to_string();
    if joined.is_empty() { None } else { Some(joined) }
}

fn looks_like_type_spelling(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':' | '[' | ']' | ',' | ' ' | '&'))
}

fn validate_module_docstrings(
    module: &CheckedApiMetadata,
    aliases: &[ApiAlias],
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
) {
    for declaration in &module.declarations {
        match declaration {
            ApiDeclaration::Function(function) => validate_callable_docstring(
                &module.module_path,
                &function.name,
                &function.anchor,
                function.docstring_sections.as_ref(),
                CallableDocFacts {
                    params: &function.params,
                    return_type: &function.return_type,
                    decorators: &function.decorators,
                    aliases: aliases_for_declaration(aliases, &module.module_path, &function.name),
                },
                diagnostics,
            ),
            ApiDeclaration::Model(model) => validate_type_docstring(
                &module.module_path,
                &model.name,
                &model.anchor,
                model.docstring_sections.as_ref(),
                TypeDocFacts {
                    fields: &model.fields,
                    decorators: &model.decorators,
                    aliases: aliases_for_declaration(aliases, &module.module_path, &model.name),
                },
                diagnostics,
            ),
            ApiDeclaration::Class(class) => validate_type_docstring(
                &module.module_path,
                &class.name,
                &class.anchor,
                class.docstring_sections.as_ref(),
                TypeDocFacts {
                    fields: &class.fields,
                    decorators: &class.decorators,
                    aliases: aliases_for_declaration(aliases, &module.module_path, &class.name),
                },
                diagnostics,
            ),
            ApiDeclaration::Trait(trait_decl) => validate_type_docstring(
                &module.module_path,
                &trait_decl.name,
                &trait_decl.anchor,
                trait_decl.docstring_sections.as_ref(),
                TypeDocFacts {
                    fields: &trait_decl.requires,
                    decorators: &trait_decl.decorators,
                    aliases: aliases_for_declaration(aliases, &module.module_path, &trait_decl.name),
                },
                diagnostics,
            ),
            ApiDeclaration::Enum(enum_decl) => validate_declaration_docstring(
                &module.module_path,
                &enum_decl.name,
                &enum_decl.anchor,
                enum_decl.docstring_sections.as_ref(),
                DeclarationDocFacts {
                    decorators: &enum_decl.decorators,
                    aliases: aliases_for_declaration(aliases, &module.module_path, &enum_decl.name),
                },
                diagnostics,
            ),
            ApiDeclaration::Newtype(newtype) => validate_declaration_docstring(
                &module.module_path,
                &newtype.name,
                &newtype.anchor,
                newtype.docstring_sections.as_ref(),
                DeclarationDocFacts {
                    decorators: &newtype.decorators,
                    aliases: aliases_for_declaration(aliases, &module.module_path, &newtype.name),
                },
                diagnostics,
            ),
            _ => {}
        }

        for method in declaration_methods(declaration) {
            validate_callable_docstring(
                &module.module_path,
                &method.name,
                &method.anchor,
                method.docstring_sections.as_ref(),
                CallableDocFacts {
                    params: &method.params,
                    return_type: &method.return_type,
                    decorators: &method.decorators,
                    aliases: Vec::new(),
                },
                diagnostics,
            );
        }
    }
}

struct CallableDocFacts<'a> {
    params: &'a [ParamExport],
    return_type: &'a TypeRef,
    decorators: &'a [DecoratorMetadata],
    aliases: Vec<&'a str>,
}

struct TypeDocFacts<'a> {
    fields: &'a [FieldExport],
    decorators: &'a [DecoratorMetadata],
    aliases: Vec<&'a str>,
}

struct DeclarationDocFacts<'a> {
    decorators: &'a [DecoratorMetadata],
    aliases: Vec<&'a str>,
}

fn validate_callable_docstring(
    module_path: &[String],
    declaration_name: &str,
    anchor: &SourceAnchor,
    docstring: Option<&ApiDocstring>,
    facts: CallableDocFacts<'_>,
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
) {
    let Some(docstring) = docstring else {
        return;
    };
    validate_named_entries(
        module_path,
        anchor,
        declaration_name,
        "parameter",
        &docstring.params,
        facts.params.iter().map(|param| param.name.as_str()).collect(),
        diagnostics,
    );
    validate_return_docstring(
        module_path,
        anchor,
        declaration_name,
        docstring,
        facts.return_type,
        diagnostics,
    );
    validate_decorator_entries(
        module_path,
        anchor,
        declaration_name,
        &docstring.decorators,
        facts.decorators,
        diagnostics,
    );
    validate_alias_entries(
        module_path,
        anchor,
        declaration_name,
        &docstring.aliases,
        facts.aliases,
        diagnostics,
    );
}

fn validate_type_docstring(
    module_path: &[String],
    declaration_name: &str,
    anchor: &SourceAnchor,
    docstring: Option<&ApiDocstring>,
    facts: TypeDocFacts<'_>,
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
) {
    let Some(docstring) = docstring else {
        return;
    };
    validate_named_entries(
        module_path,
        anchor,
        declaration_name,
        "field",
        &docstring.fields,
        facts.fields.iter().map(|field| field.name.as_str()).collect(),
        diagnostics,
    );
    validate_decorator_entries(
        module_path,
        anchor,
        declaration_name,
        &docstring.decorators,
        facts.decorators,
        diagnostics,
    );
    validate_alias_entries(
        module_path,
        anchor,
        declaration_name,
        &docstring.aliases,
        facts.aliases,
        diagnostics,
    );
}

fn validate_declaration_docstring(
    module_path: &[String],
    declaration_name: &str,
    anchor: &SourceAnchor,
    docstring: Option<&ApiDocstring>,
    facts: DeclarationDocFacts<'_>,
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
) {
    let Some(docstring) = docstring else {
        return;
    };
    validate_decorator_entries(
        module_path,
        anchor,
        declaration_name,
        &docstring.decorators,
        facts.decorators,
        diagnostics,
    );
    validate_alias_entries(
        module_path,
        anchor,
        declaration_name,
        &docstring.aliases,
        facts.aliases,
        diagnostics,
    );
}

fn validate_return_docstring(
    module_path: &[String],
    anchor: &SourceAnchor,
    declaration_name: &str,
    docstring: &ApiDocstring,
    return_type: &TypeRef,
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
) {
    let Some(returns) = &docstring.returns else {
        return;
    };
    let Some(documented_type) = &returns.ty else {
        return;
    };
    let checked_type = type_ref_doc_name(return_type);
    if documented_type != &checked_type {
        push_docstring_diagnostic(
            diagnostics,
            module_path,
            anchor,
            format!(
                "API docstring drift for `{declaration_name}`: documented return type `{documented_type}` does not match checked return type `{checked_type}`"
            ),
            "Update the `Returns:` section or the checked function signature.",
        );
    }
}

fn validate_decorator_entries(
    module_path: &[String],
    anchor: &SourceAnchor,
    declaration_name: &str,
    documented: &[ApiDocstringEntry],
    checked: &[DecoratorMetadata],
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
) {
    let names = checked
        .iter()
        .flat_map(|decorator| {
            [
                decorator.source_name.as_str(),
                decorator.path.last().map(String::as_str).unwrap_or_default(),
            ]
        })
        .collect::<Vec<_>>();
    validate_named_entries(
        module_path,
        anchor,
        declaration_name,
        "decorator",
        documented,
        names,
        diagnostics,
    );
}

fn validate_alias_entries(
    module_path: &[String],
    anchor: &SourceAnchor,
    declaration_name: &str,
    documented: &[ApiDocstringEntry],
    checked_aliases: Vec<&str>,
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
) {
    validate_named_entries(
        module_path,
        anchor,
        declaration_name,
        "alias",
        documented,
        checked_aliases,
        diagnostics,
    );
}

/// Validate named docstring entries against checked metadata names and report stale or missing entries.
fn validate_named_entries(
    module_path: &[String],
    anchor: &SourceAnchor,
    declaration_name: &str,
    noun: &str,
    documented: &[ApiDocstringEntry],
    checked_names: Vec<&str>,
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
) {
    if documented.is_empty() {
        return;
    }

    let checked: HashSet<&str> = checked_names.into_iter().filter(|name| !name.is_empty()).collect();
    let mut seen = HashSet::new();
    for entry in documented {
        if !seen.insert(entry.name.as_str()) {
            push_docstring_diagnostic(
                diagnostics,
                module_path,
                anchor,
                format!(
                    "API docstring drift for `{declaration_name}`: docstring documents {noun} `{}` more than once",
                    entry.name
                ),
                format!(
                    "Keep one `{}` entry in the `{}` section.",
                    entry.name,
                    section_name_for_noun(noun)
                ),
            );
            continue;
        }
        if !checked.contains(entry.name.as_str()) {
            push_docstring_diagnostic(
                diagnostics,
                module_path,
                anchor,
                format!(
                    "API docstring drift for `{declaration_name}`: documented {noun} `{}` does not exist in checked metadata",
                    entry.name
                ),
                format!(
                    "Remove `{}` from the `{}` section or update the checked declaration.",
                    entry.name,
                    section_name_for_noun(noun)
                ),
            );
        }
    }

    if matches!(noun, "parameter" | "field") {
        for checked_name in checked {
            if !seen.contains(checked_name) {
                push_docstring_diagnostic(
                    diagnostics,
                    module_path,
                    anchor,
                    format!(
                        "API docstring drift for `{declaration_name}`: checked {noun} `{checked_name}` is missing from the docstring"
                    ),
                    format!(
                        "Add `{checked_name}: ...` to the `{}` section or remove the stale section.",
                        section_name_for_noun(noun)
                    ),
                );
            }
        }
    }
}

fn section_name_for_noun(noun: &str) -> &'static str {
    match noun {
        "parameter" => "Args:",
        "field" => "Fields:",
        "alias" => "Aliases:",
        "decorator" => "Decorators:",
        _ => "docstring",
    }
}

fn push_docstring_diagnostic(
    diagnostics: &mut Vec<ApiDocstringDiagnostic>,
    module_path: &[String],
    anchor: &SourceAnchor,
    message: String,
    hint: impl Into<String>,
) {
    diagnostics.push(ApiDocstringDiagnostic {
        module_path: module_path.to_vec(),
        error: CompileError::new(message, Span::new(anchor.span.start, anchor.span.end)).with_hint(hint),
    });
}

fn declaration_methods(declaration: &ApiDeclaration) -> &[ApiMethod] {
    match declaration {
        ApiDeclaration::Model(model) => &model.methods,
        ApiDeclaration::Class(class) => &class.methods,
        ApiDeclaration::Trait(trait_decl) => &trait_decl.methods,
        ApiDeclaration::Newtype(newtype) => &newtype.methods,
        _ => &[],
    }
}

fn package_aliases(package: &[CheckedApiMetadata]) -> Vec<ApiAlias> {
    package
        .iter()
        .flat_map(|module| module.declarations.iter())
        .filter_map(|declaration| match declaration {
            ApiDeclaration::Alias(alias) => Some(alias.clone()),
            _ => None,
        })
        .collect()
}

fn aliases_for_declaration<'a>(aliases: &'a [ApiAlias], module_path: &[String], name: &str) -> Vec<&'a str> {
    aliases
        .iter()
        .filter(|alias| alias_targets_declaration(alias, module_path, name))
        .map(|alias| alias.name.as_str())
        .collect()
}

fn alias_targets_declaration(alias: &ApiAlias, module_path: &[String], name: &str) -> bool {
    let mut declaration_path = module_path.to_vec();
    declaration_path.push(name.to_string());
    if alias.target_path == declaration_path {
        return true;
    }
    if alias.target_path.first().is_some_and(|segment| segment == "crate") && alias.target_path[1..] == declaration_path
    {
        return true;
    }
    false
}

fn type_ref_doc_name(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Named { name } => name.clone(),
        TypeRef::Applied { name, args } => {
            let args = args.iter().map(type_ref_doc_name).collect::<Vec<_>>().join(", ");
            format!("{name}[{args}]")
        }
        TypeRef::Function { params, return_type } => {
            let params = params.iter().map(type_ref_doc_name).collect::<Vec<_>>().join(", ");
            format!("({params}) -> {}", type_ref_doc_name(return_type))
        }
        TypeRef::Tuple { elements } => {
            let elements = elements.iter().map(type_ref_doc_name).collect::<Vec<_>>().join(", ");
            format!("({elements})")
        }
        TypeRef::TypeParam { name } => name.clone(),
        TypeRef::SelfType => "Self".to_string(),
        TypeRef::Ref { inner } => format!("&{}", type_ref_doc_name(inner)),
        TypeRef::RustPath { path } => path.clone(),
        TypeRef::Unknown => "Unknown".to_string(),
    }
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

    Args:
        values: Input values.

    Returns:
        float: Mean value.

    Decorators:
        rust.allow: Allows generated Rust lint suppression.
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
            Some(
                "Return the arithmetic mean.\n\n    Args:\n        values: Input values.\n\n    Returns:\n        float: Mean value.\n\n    Decorators:\n        rust.allow: Allows generated Rust lint suppression."
            )
        );
        let docstring = function
            .docstring_sections
            .as_ref()
            .ok_or_else(|| "expected parsed docstring sections".to_string())?;
        assert_eq!(docstring.summary.as_deref(), Some("Return the arithmetic mean."));
        assert_eq!(
            docstring.params,
            vec![ApiDocstringEntry {
                name: "values".to_string(),
                description: "Input values.".to_string(),
            }]
        );
        assert_eq!(
            docstring.returns,
            Some(ApiDocstringReturn {
                ty: Some("float".to_string()),
                description: "Mean value.".to_string(),
            })
        );
        assert_eq!(
            docstring.decorators,
            vec![ApiDocstringEntry {
                name: "rust.allow".to_string(),
                description: "Allows generated Rust lint suppression.".to_string(),
            }]
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
    fn checked_api_docstring_validation_reports_signature_drift() -> Result<(), String> {
        let source = r#"
pub def avg(values: List[float]) -> float:
    """
    Return the arithmetic mean.

    Args:
        missing: Stale argument.

    Returns:
        str: Wrong return type.

    Decorators:
        rust.allow: Stale decorator.
    """
    return 0.0
"#;
        let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
        let diagnostics = validate_checked_api_docstrings(&[metadata]);
        let messages = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.error.message.as_str())
            .collect::<Vec<_>>();

        assert!(
            messages
                .iter()
                .any(|message| message.contains("documented parameter `missing` does not exist")),
            "expected unknown parameter diagnostic, got {messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("checked parameter `values` is missing")),
            "expected missing checked parameter diagnostic, got {messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message
                    .contains("documented return type `str` does not match checked return type `float`")),
            "expected return type drift diagnostic, got {messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("documented decorator `rust.allow` does not exist")),
            "expected decorator drift diagnostic, got {messages:?}"
        );
        Ok(())
    }

    #[test]
    fn checked_api_docstring_validation_reports_field_drift() -> Result<(), String> {
        let source = r#"
pub model Order:
    """
    Order contract.

    Fields:
        missing: Stale field documentation.
    """
    id: int
"#;
        let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
        let diagnostics = validate_checked_api_docstrings(&[metadata]);
        let messages = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.error.message.as_str())
            .collect::<Vec<_>>();

        assert!(
            messages
                .iter()
                .any(|message| message.contains("documented field `missing` does not exist")),
            "expected unknown field diagnostic, got {messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("checked field `id` is missing")),
            "expected missing checked field diagnostic, got {messages:?}"
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
