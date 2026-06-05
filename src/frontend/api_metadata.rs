//! Checked public API metadata extraction for RFC 048.
//!
//! This module builds a JSON-ready model from parsed and typechecked Incan semantics. It deliberately reuses the
//! manifest type vocabulary instead of stringifying checked types, so package artifacts, CLI output, and later docs
//! tooling can share one structural representation.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::frontend::ast::{
    CallArg, ClassDecl, Declaration, Decorator, DecoratorArg, DecoratorArgValue, DictEntry, EnumDecl, Expr, FieldDecl,
    FunctionDecl, ImportDecl, ImportItem, ImportKind, ListEntry, MethodDecl, ModelDecl, NewtypeDecl, Program, Span,
    Spanned, Statement, TraitDecl, TypeAliasDecl, Visibility,
};
use crate::frontend::decorator_resolution;
use crate::frontend::diagnostics::CompileError;
use crate::frontend::library_exports::{
    CheckedClassExport, CheckedConstExport, CheckedEnumExport, CheckedExportKind, CheckedField, CheckedFunctionExport,
    CheckedMethod, CheckedModelExport, CheckedNamedExport, CheckedNewtypeExport, CheckedPartialExport,
    CheckedPartialTargetKind, CheckedPresetValue, CheckedTraitExport, CheckedTypeAliasExport, CheckedTypeBound,
    CheckedTypeParam, collect_checked_public_exports,
};
use crate::frontend::module::canonicalize_source_module_segments;
use crate::frontend::typechecker::{ConstValue, TypeChecker};
use crate::library_manifest::{
    ClassExport, EnumExport, EnumValueExport, EnumValueTypeExport, EnumVariantAliasExport, EnumVariantExport,
    FieldExport, FieldRequirementExport, FunctionExport, MethodExport, ModelExport, NewtypeExport, ParamExport,
    ParamKindExport, PartialExport, PartialPresetExport, PartialTargetKindExport, PresetDictEntryExport,
    PresetModelFieldExport, PresetValueExport, ReceiverExport, TraitExport, TypeAliasExport, TypeBoundExport,
    TypeParamExport, TypeRef, type_ref_from_resolved,
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
    Partial(ApiPartial),
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trait_adoptions: Vec<TypeBoundExport>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trait_adoptions: Vec<TypeBoundExport>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trait_adoptions: Vec<TypeBoundExport>,
    pub value_type: Option<EnumValueTypeExport>,
    pub variants: Vec<ApiEnumVariant>,
    pub variant_aliases: Vec<ApiEnumVariantAlias>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<ApiMethod>,
    pub derives: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiEnumVariant {
    pub name: String,
    pub fields: Vec<TypeRef>,
    pub value: Option<EnumValueExport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiEnumVariantAlias {
    pub name: String,
    pub target: String,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trait_adoptions: Vec<TypeBoundExport>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected_function: Option<ApiProjectedFunction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiPartial {
    pub name: String,
    pub anchor: SourceAnchor,
    pub target_path: Vec<String>,
    pub target_kind: PartialTargetKindExport,
    pub presets: Vec<PartialPresetExport>,
    pub type_params: Vec<TypeParamExport>,
    pub params: Vec<ParamExport>,
    pub return_type: TypeRef,
    pub is_async: bool,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub type_args: Vec<TypeRef>,
    pub args: Vec<DecoratorArgMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decorated_callable: Option<ApiCallableMetadata>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiProjectedFunction {
    pub source_path: Vec<String>,
    pub callable: ApiCallableMetadata,
    pub decorators: Vec<DecoratorMetadata>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiCallableMetadata {
    pub name: String,
    pub anchor: SourceAnchor,
    pub type_params: Vec<TypeParamExport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<ReceiverExport>,
    pub params: Vec<ParamExport>,
    pub return_type: TypeRef,
    pub is_async: bool,
}

/// Convert a checked API function into manifest-style callable metadata.
pub(crate) fn function_export_from_api(function: &ApiFunction) -> FunctionExport {
    FunctionExport {
        name: function.name.clone(),
        emitted_name: None,
        type_params: function.type_params.clone(),
        params: function.params.clone(),
        return_type: function.return_type.clone(),
        is_async: function.is_async,
    }
}

/// Convert a projected alias callable into manifest-style callable metadata while preserving source callable identity.
pub(crate) fn function_export_from_api_projected(function: &ApiProjectedFunction) -> FunctionExport {
    FunctionExport {
        name: function.callable.name.clone(),
        emitted_name: None,
        type_params: function.callable.type_params.clone(),
        params: function.callable.params.clone(),
        return_type: function.callable.return_type.clone(),
        is_async: function.callable.is_async,
    }
}

/// Convert a checked API partial into manifest-style partial metadata.
pub(crate) fn partial_export_from_api(partial: &ApiPartial) -> PartialExport {
    PartialExport {
        name: partial.name.clone(),
        target_path: partial.target_path.clone(),
        target_kind: partial.target_kind,
        presets: partial.presets.clone(),
        type_params: partial.type_params.clone(),
        params: partial.params.clone(),
        return_type: partial.return_type.clone(),
        is_async: partial.is_async,
    }
}

/// Convert a checked API method into manifest-style method metadata.
pub(crate) fn method_export_from_api(method: &ApiMethod) -> MethodExport {
    MethodExport {
        name: method.name.clone(),
        alias_of: None,
        type_params: method.type_params.clone(),
        receiver: method.receiver.clone(),
        params: method.params.clone(),
        return_type: method.return_type.clone(),
        is_async: method.is_async,
        has_body: method.has_body,
    }
}

/// Convert a checked API model into manifest-style model metadata for public boundary consumers.
pub(crate) fn model_export_from_api(model: &ApiModel) -> ModelExport {
    ModelExport {
        name: model.name.clone(),
        type_params: model.type_params.clone(),
        traits: model.traits.clone(),
        trait_adoptions: model.trait_adoptions.clone(),
        derives: model.derives.clone(),
        fields: model.fields.clone(),
        methods: model.methods.iter().map(method_export_from_api).collect(),
    }
}

/// Convert a checked API class into manifest-style class metadata for public boundary consumers.
pub(crate) fn class_export_from_api(class: &ApiClass) -> ClassExport {
    ClassExport {
        name: class.name.clone(),
        type_params: class.type_params.clone(),
        extends: class.extends.clone(),
        traits: class.traits.clone(),
        trait_adoptions: class.trait_adoptions.clone(),
        derives: class.derives.clone(),
        fields: class.fields.clone(),
        methods: class.methods.iter().map(method_export_from_api).collect(),
    }
}

/// Convert a checked API trait into manifest-style trait metadata for public boundary consumers.
pub(crate) fn trait_export_from_api(trait_decl: &ApiTrait) -> TraitExport {
    TraitExport {
        name: trait_decl.name.clone(),
        source_name: None,
        type_params: trait_decl.type_params.clone(),
        supertraits: trait_decl.supertraits.clone(),
        requires: trait_decl
            .requires
            .iter()
            .map(|field| FieldRequirementExport {
                name: field.name.clone(),
                ty: field.ty.clone(),
            })
            .collect(),
        methods: trait_decl.methods.iter().map(method_export_from_api).collect(),
    }
}

/// Convert a checked API enum into manifest-style enum metadata for public boundary consumers.
pub(crate) fn enum_export_from_api(enum_decl: &ApiEnum) -> EnumExport {
    EnumExport {
        name: enum_decl.name.clone(),
        type_params: enum_decl.type_params.clone(),
        traits: enum_decl.traits.clone(),
        trait_adoptions: enum_decl.trait_adoptions.clone(),
        value_type: enum_decl.value_type,
        ordinal_type_identity: None,
        variants: enum_decl
            .variants
            .iter()
            .map(|variant| EnumVariantExport {
                name: variant.name.clone(),
                fields: variant.fields.clone(),
                value: variant.value.clone(),
            })
            .collect(),
        variant_aliases: enum_decl
            .variant_aliases
            .iter()
            .map(|alias| EnumVariantAliasExport {
                name: alias.name.clone(),
                target: alias.target.clone(),
            })
            .collect(),
        methods: enum_decl.methods.iter().map(method_export_from_api).collect(),
        derives: enum_decl.derives.clone(),
    }
}

/// Convert a checked API newtype into manifest-style newtype metadata for public boundary consumers.
pub(crate) fn newtype_export_from_api(newtype: &ApiNewtype) -> NewtypeExport {
    NewtypeExport {
        name: newtype.name.clone(),
        type_params: newtype.type_params.clone(),
        traits: newtype.traits.clone(),
        trait_adoptions: newtype.trait_adoptions.clone(),
        is_rusttype: newtype.is_rusttype,
        underlying: newtype.underlying.clone(),
        methods: newtype.methods.iter().map(method_export_from_api).collect(),
    }
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
    SymbolRef {
        path: Vec<String>,
    },
    List {
        items: Vec<DecoratorValue>,
    },
    Dict {
        entries: Vec<DecoratorDictEntry>,
    },
    Call {
        callee: Vec<String>,
        type_args: Vec<TypeRef>,
        args: Vec<DecoratorCallArgMetadata>,
    },
    Type {
        ty: TypeRef,
    },
    Unsupported {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecoratorDictEntry {
    pub key: DecoratorValue,
    pub value: DecoratorValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecoratorCallArgMetadata {
    Positional { value: DecoratorValue },
    Named { name: String, value: DecoratorValue },
    PositionalUnpack { value: DecoratorValue },
    KeywordUnpack { value: DecoratorValue },
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

/// Collect checked public API metadata for one parsed and typechecked module.
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
            Declaration::Alias(alias) if public(alias.visibility) => {
                declarations.push(ApiDeclaration::Alias(ApiAlias {
                    name: alias.name.clone(),
                    anchor: anchor(&module_path, &alias.name, decl.span),
                    target_path: alias.target.segments.clone(),
                    projected_function: None,
                }));
            }
            Declaration::Partial(partial) if public(partial.visibility) => {
                if let Some(CheckedExportKind::Partial(export)) = checked_kind(&checked_by_name, &partial.name) {
                    declarations.push(ApiDeclaration::Partial(api_partial(export, decl.span, &module_path)));
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

/// Attach checked function projections to public aliases that target decorated or ordinary public functions.
///
/// Metadata package consumers should not need to force producer module initialization just to discover declaration-side
/// decorator facts. This projection pass resolves aliases across the already checked API package and carries the target
/// function's decorators and checked callable shape onto facade aliases.
pub fn materialize_api_alias_projections(modules: &mut [CheckedApiMetadata]) {
    let mut projections = HashMap::new();
    let mut aliases = Vec::new();

    for module in modules.iter() {
        for declaration in &module.declarations {
            match declaration {
                ApiDeclaration::Function(function) => {
                    projections.insert(
                        declaration_path(&module.module_path, &function.name),
                        ApiProjectedFunction {
                            source_path: declaration_path(&module.module_path, &function.name),
                            callable: callable_from_function(function),
                            decorators: function.decorators.clone(),
                        },
                    );
                }
                ApiDeclaration::Alias(alias) => aliases.push(ApiAliasProjectionRequest {
                    path: declaration_path(&module.module_path, &alias.name),
                    target_path: normalized_api_target_path(&alias.target_path),
                    name: alias.name.clone(),
                    anchor: alias.anchor.clone(),
                }),
                _ => {}
            }
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for alias in &aliases {
            if projections.contains_key(&alias.path) {
                continue;
            }
            if let Some(target) = projections.get(&alias.target_path) {
                projections.insert(alias.path.clone(), projected_function_for_alias(alias, target));
                changed = true;
            }
        }
    }

    for module in modules {
        for declaration in &mut module.declarations {
            if let ApiDeclaration::Alias(alias) = declaration {
                let alias_path = declaration_path(&module.module_path, &alias.name);
                alias.projected_function = projections.get(&alias_path).cloned();
            }
        }
    }
}

#[derive(Debug)]
struct ApiAliasProjectionRequest {
    path: Vec<String>,
    target_path: Vec<String>,
    name: String,
    anchor: SourceAnchor,
}

/// Build the API declaration path for a module-local name.
fn declaration_path(module_path: &[String], name: &str) -> Vec<String> {
    let mut path = module_path.to_vec();
    path.push(name.to_string());
    path
}

/// Normalize an API target path by removing a leading `crate` segment.
fn normalized_api_target_path(path: &[String]) -> Vec<String> {
    if path.first().is_some_and(|segment| segment == "crate") {
        return path[1..].to_vec();
    }
    path.to_vec()
}

/// Build callable metadata from a checked API function export.
fn callable_from_function(function: &ApiFunction) -> ApiCallableMetadata {
    ApiCallableMetadata {
        name: function.name.clone(),
        anchor: function.anchor.clone(),
        type_params: function.type_params.clone(),
        receiver: None,
        params: function.params.clone(),
        return_type: function.return_type.clone(),
        is_async: function.is_async,
    }
}

/// Build projected callable metadata for an alias re-export.
fn projected_function_for_alias(
    alias: &ApiAliasProjectionRequest,
    target: &ApiProjectedFunction,
) -> ApiProjectedFunction {
    let mut projected = target.clone();
    projected.callable.name = alias.name.clone();
    projected.callable.anchor = alias.anchor.clone();
    projected
}

/// Look up the checked export kind for a public name.
fn checked_kind<'a>(exports: &'a HashMap<String, CheckedNamedExport>, name: &str) -> Option<&'a CheckedExportKind> {
    exports.get(name).map(|export| &export.kind)
}

/// Return whether a declaration visibility is public.
fn public(visibility: Visibility) -> bool {
    matches!(visibility, Visibility::Public)
}

/// Convert checked partial export metadata into the checked API package shape.
fn api_partial(export: &CheckedPartialExport, span: Span, module_path: &[String]) -> ApiPartial {
    ApiPartial {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        target_path: export.target_path.clone(),
        target_kind: api_partial_target_kind(export.target_kind),
        presets: export
            .presets
            .iter()
            .map(|preset| PartialPresetExport {
                name: preset.name.clone(),
                ty: type_ref_from_resolved(&preset.ty),
                value: api_preset_value(&preset.value),
            })
            .collect(),
        type_params: type_params(&export.type_params),
        params: params(&export.params),
        return_type: type_ref_from_resolved(&export.return_type),
        is_async: export.is_async,
    }
}

/// Convert frontend partial target kind metadata into manifest/API vocabulary.
fn api_partial_target_kind(kind: CheckedPartialTargetKind) -> PartialTargetKindExport {
    match kind {
        CheckedPartialTargetKind::Function => PartialTargetKindExport::Function,
        CheckedPartialTargetKind::ModelConstructor => PartialTargetKindExport::ModelConstructor,
        CheckedPartialTargetKind::ClassConstructor => PartialTargetKindExport::ClassConstructor,
        CheckedPartialTargetKind::NewtypeConstructor => PartialTargetKindExport::NewtypeConstructor,
        CheckedPartialTargetKind::Partial => PartialTargetKindExport::Partial,
        CheckedPartialTargetKind::Unknown => PartialTargetKindExport::Unknown,
    }
}

/// Convert checked preset values into the serialized API metadata representation.
fn api_preset_value(value: &CheckedPresetValue) -> PresetValueExport {
    match value {
        CheckedPresetValue::Int(value) => PresetValueExport::Int(*value),
        CheckedPresetValue::Float(value) => PresetValueExport::Float(value.to_string()),
        CheckedPresetValue::Bool(value) => PresetValueExport::Bool(*value),
        CheckedPresetValue::String(value) => PresetValueExport::String(value.clone()),
        CheckedPresetValue::Bytes(value) => PresetValueExport::Bytes(value.clone()),
        CheckedPresetValue::None => PresetValueExport::None,
        CheckedPresetValue::List(values) => PresetValueExport::List(values.iter().map(api_preset_value).collect()),
        CheckedPresetValue::Dict(entries) => PresetValueExport::Dict(
            entries
                .iter()
                .map(|(key, value)| PresetDictEntryExport {
                    key: api_preset_value(key),
                    value: api_preset_value(value),
                })
                .collect(),
        ),
        CheckedPresetValue::ConstRef(path) => PresetValueExport::ConstRef(path.clone()),
        CheckedPresetValue::ModelLiteral { name, fields } => PresetValueExport::ModelLiteral {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(field, value)| PresetModelFieldExport {
                    name: field.clone(),
                    value: api_preset_value(value),
                })
                .collect(),
        },
        CheckedPresetValue::Unsupported => PresetValueExport::Unsupported,
    }
}

/// Convert a source function declaration into API metadata.
fn api_function(
    function: &FunctionDecl,
    span: Span,
    export: &CheckedFunctionExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiFunction {
    let docstring = function_docstring(&function.body);
    let callable = api_callable_for_function(function, span, export, checker, module_path);
    ApiFunction {
        name: callable.name.clone(),
        anchor: callable.anchor.clone(),
        docstring_sections: parse_docstring(docstring.as_deref()),
        docstring,
        decorators: decorators_metadata(&function.decorators, checker, Some(&callable)),
        type_params: callable.type_params,
        params: callable.params,
        return_type: callable.return_type,
        is_async: callable.is_async,
    }
}

/// Convert a source function declaration into callable API metadata.
fn api_callable_for_function(
    function: &FunctionDecl,
    span: Span,
    export: &CheckedFunctionExport,
    checker: &TypeChecker,
    module_path: &[String],
) -> ApiCallableMetadata {
    ApiCallableMetadata {
        name: export.name.clone(),
        anchor: anchor(module_path, &export.name, span),
        type_params: type_params(&export.type_params),
        receiver: None,
        params: source_function_params(function, checker),
        return_type: source_function_return_type(function, checker),
        is_async: function.is_async(),
    }
}

/// Build the source-declared callable parameter surface for API documentation metadata.
///
/// User-defined decorators can rebind a public function symbol to an ordinary callable value. That callable type is the
/// right contract for lowering and invocation, but function API docs are attached to the source declaration and should
/// validate against the declaration's named parameters instead of an anonymous function-type projection.
fn source_function_params(function: &FunctionDecl, checker: &TypeChecker) -> Vec<ParamExport> {
    function
        .params
        .iter()
        .map(|param| ParamExport {
            name: param.node.name.clone(),
            ty: type_ref_from_resolved(&crate::frontend::symbols::resolve_type(
                &param.node.ty.node,
                &checker.symbols,
            )),
            kind: match param.node.kind {
                crate::frontend::ast::ParamKind::Normal => ParamKindExport::Normal,
                crate::frontend::ast::ParamKind::RestPositional => ParamKindExport::RestPositional,
                crate::frontend::ast::ParamKind::RestKeyword => ParamKindExport::RestKeyword,
            },
            has_default: param.node.default.is_some(),
            default: None,
        })
        .collect()
}

/// Resolve the source return type used by function API metadata.
fn source_function_return_type(function: &FunctionDecl, checker: &TypeChecker) -> TypeRef {
    type_ref_from_resolved(&crate::frontend::symbols::resolve_type(
        &function.return_type.node,
        &checker.symbols,
    ))
}

/// Convert a source model declaration into API metadata.
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
        decorators: decorators_metadata(&model.decorators, checker, None),
        type_params: type_params(&export.type_params),
        traits: export.traits.clone(),
        trait_adoptions: export.trait_adoptions.iter().map(type_bound).collect(),
        derives: export.derives.clone(),
        fields: fields_in_source_order(&model.fields, &export.fields),
        methods: methods(&model.methods, &export.methods, checker, module_path, &export.name),
    }
}

/// Convert a source class declaration into API metadata.
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
        decorators: decorators_metadata(&class.decorators, checker, None),
        type_params: type_params(&export.type_params),
        extends: export.extends.clone(),
        traits: export.traits.clone(),
        trait_adoptions: export.trait_adoptions.iter().map(type_bound).collect(),
        derives: export.derives.clone(),
        fields: fields_in_source_order(&class.fields, &export.fields),
        methods: methods(&class.methods, &export.methods, checker, module_path, &export.name),
    }
}

/// Convert a checked trait export into API metadata.
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
        decorators: decorators_metadata(&trait_decl.decorators, checker, None),
        type_params: type_params(&export.type_params),
        supertraits: export.supertraits.iter().map(type_bound).collect(),
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

/// Convert a checked enum export into API metadata, preserving canonical variants and public aliases.
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
        decorators: decorators_metadata(&enum_decl.decorators, checker, None),
        type_params: type_params(&export.type_params),
        traits: export.traits.clone(),
        trait_adoptions: export.trait_adoptions.iter().map(type_bound).collect(),
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
        variant_aliases: export
            .variant_aliases
            .iter()
            .map(|alias| ApiEnumVariantAlias {
                name: alias.name.clone(),
                target: alias.target.clone(),
            })
            .collect(),
        methods: methods(&enum_decl.methods, &export.methods, checker, module_path, &export.name),
        derives: export.derives.clone(),
    }
}

/// Convert a source newtype declaration into API metadata.
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
        decorators: decorators_metadata(&newtype.decorators, checker, None),
        type_params: type_params(&export.type_params),
        traits: export.traits.clone(),
        trait_adoptions: export.trait_adoptions.iter().map(type_bound).collect(),
        is_rusttype: export.is_rusttype,
        underlying: type_ref_from_resolved(&export.underlying),
        methods: methods(&newtype.methods, &export.methods, checker, module_path, &export.name),
    }
}

/// Convert a source type alias declaration into API metadata.
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

/// Convert a checked constant declaration into API metadata.
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

/// Convert an import declaration into API alias metadata.
fn api_aliases(import: &ImportDecl, span: Span, module_path: &[String]) -> Vec<ApiAlias> {
    match &import.kind {
        ImportKind::From { module, items } => {
            let base_path =
                canonicalize_source_module_segments(&decorator_resolution::path_segments_with_prefix(module));
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

/// Convert import items into API alias metadata rooted at a base path.
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
                projected_function: None,
            }
        })
        .collect()
}

/// Pair AST method declarations with checked method metadata for documentation output.
fn methods(
    ast_methods: &[Spanned<MethodDecl>],
    checked_methods: &[CheckedMethod],
    checker: &TypeChecker,
    module_path: &[String],
    owner: &str,
) -> Vec<ApiMethod> {
    let mut checked_by_name: HashMap<&str, VecDeque<&CheckedMethod>> = HashMap::new();
    for method in checked_methods {
        checked_by_name
            .entry(method.name.as_str())
            .or_default()
            .push_back(method);
    }
    let mut out = Vec::new();
    for method in ast_methods {
        let Some(candidates) = checked_by_name.get_mut(method.node.name.as_str()) else {
            continue;
        };
        let Some(checked) = take_checked_method_for_ast(&method.node, candidates, checker) else {
            continue;
        };
        let docstring = method.node.body.as_ref().and_then(|body| function_docstring(body));
        let callable = ApiCallableMetadata {
            name: checked.name.clone(),
            anchor: anchor(module_path, &format!("{owner}.{}", checked.name), method.span),
            type_params: type_params(&checked.type_params),
            receiver: checked.receiver.map(|receiver| match receiver {
                crate::frontend::ast::Receiver::Immutable => ReceiverExport::Immutable,
                crate::frontend::ast::Receiver::Mutable => ReceiverExport::Mutable,
            }),
            params: params(&checked.params),
            return_type: type_ref_from_resolved(&checked.return_type),
            is_async: checked.is_async,
        };
        out.push(ApiMethod {
            name: callable.name.clone(),
            anchor: callable.anchor.clone(),
            docstring_sections: parse_docstring(docstring.as_deref()),
            docstring,
            decorators: decorators_metadata(&method.node.decorators, checker, Some(&callable)),
            type_params: callable.type_params,
            receiver: callable.receiver,
            params: callable.params,
            return_type: callable.return_type,
            is_async: callable.is_async,
            has_body: checked.has_body,
        });
    }
    out
}

/// Remove the checked method that best matches one AST method declaration.
fn take_checked_method_for_ast<'a>(
    ast_method: &MethodDecl,
    candidates: &mut VecDeque<&'a CheckedMethod>,
    checker: &TypeChecker,
) -> Option<&'a CheckedMethod> {
    if let Some(index) = candidates
        .iter()
        .position(|checked| checked_method_shape_matches(ast_method, checked, checker))
    {
        return candidates.remove(index);
    }
    if let Some(index) = candidates
        .iter()
        .position(|checked| checked_method_param_names_match(ast_method, checked))
    {
        return candidates.remove(index);
    }
    candidates.pop_front()
}

/// Return whether an AST method and checked method have the same parameter names.
fn checked_method_param_names_match(ast_method: &MethodDecl, checked: &CheckedMethod) -> bool {
    ast_method.params.len() == checked.params.len()
        && ast_method
            .params
            .iter()
            .zip(checked.params.iter())
            .all(|(ast_param, checked_param)| checked_param.name() == Some(ast_param.node.name.as_str()))
}

/// Return whether an AST method and checked method have the same callable shape.
fn checked_method_shape_matches(ast_method: &MethodDecl, checked: &CheckedMethod, checker: &TypeChecker) -> bool {
    let ast_type_params: Vec<&str> = ast_method
        .type_params
        .iter()
        .map(|type_param| type_param.name.as_str())
        .collect();
    ast_method.params.len() == checked.params.len()
        && ast_method.receiver == checked.receiver
        && ast_method.is_async() == checked.is_async
        && ast_type_params
            == checked
                .type_params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>()
        && ast_method
            .params
            .iter()
            .zip(checked.params.iter())
            .all(|(ast_param, checked_param)| {
                checked_param.name() == Some(ast_param.node.name.as_str())
                    && checked_param.kind == ast_param.node.kind
                    && checked_param.has_default == ast_param.node.default.is_some()
                    && type_ref_from_resolved(&checked_param.ty)
                        == type_ref_from_resolved(&crate::frontend::symbols::resolve_type(
                            &ast_param.node.ty.node,
                            &checker.symbols,
                        ))
            })
        && type_ref_from_resolved(&checked.return_type)
            == type_ref_from_resolved(&crate::frontend::symbols::resolve_type(
                &ast_method.return_type.node,
                &checker.symbols,
            ))
}

/// Convert checked type parameters into API metadata exports.
fn type_params(type_params: &[CheckedTypeParam]) -> Vec<TypeParamExport> {
    type_params
        .iter()
        .map(|type_param| TypeParamExport {
            name: type_param.name.clone(),
            bounds: type_param.bounds.iter().map(type_bound).collect(),
        })
        .collect()
}

/// Convert a checked trait bound into the exported API metadata representation.
fn type_bound(bound: &CheckedTypeBound) -> TypeBoundExport {
    TypeBoundExport {
        name: bound.name.clone(),
        source_name: bound.source_name.clone(),
        module_path: bound.module_path.clone(),
        type_args: bound.type_args.iter().map(type_ref_from_resolved).collect(),
    }
}

/// Convert checked callable parameters into API metadata exports.
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
                default: None,
            })
        })
        .collect()
}

/// Convert a checked field into API metadata.
fn field(field: &crate::frontend::library_exports::CheckedField) -> FieldExport {
    FieldExport {
        name: field.name.clone(),
        ty: type_ref_from_resolved(&field.ty),
        has_default: field.has_default,
        alias: field.alias.clone(),
        description: field.description.clone(),
    }
}

/// Return checked fields ordered to match the source declaration.
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

/// Convert source decorators into checked API metadata entries.
fn decorators_metadata(
    decorators: &[Spanned<Decorator>],
    checker: &TypeChecker,
    decorated_callable: Option<&ApiCallableMetadata>,
) -> Vec<DecoratorMetadata> {
    decorators
        .iter()
        .map(|decorator| {
            let resolved = decorator_resolution::resolve_decorator_path(&decorator.node, &checker.import_aliases);
            DecoratorMetadata {
                path: resolved,
                source_name: decorator.node.path.segments.join("."),
                anchor: source_span(decorator.span),
                type_args: decorator
                    .node
                    .type_args
                    .iter()
                    .map(|type_arg| {
                        type_ref_from_resolved(&crate::frontend::symbols::resolve_type(
                            &type_arg.node,
                            &checker.symbols,
                        ))
                    })
                    .collect(),
                args: decorator
                    .node
                    .args
                    .iter()
                    .map(|arg| decorator_arg_metadata(arg, checker))
                    .collect(),
                decorated_callable: decorated_callable.cloned(),
            }
        })
        .collect()
}

/// Convert a decorator argument into API metadata.
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

/// Convert a decorator expression into a safe metadata value.
fn decorator_expr_value(expr: &Spanned<Expr>, checker: &TypeChecker) -> DecoratorValue {
    match &expr.node {
        Expr::Literal(literal) => DecoratorValue::Literal {
            value: safe_value_from_literal(literal),
        },
        Expr::Ident(name) => DecoratorValue::ConstRef {
            name: name.clone(),
            value: checker.type_info().const_value(name).map(safe_value_from_const),
        },
        Expr::Field(base, field) => {
            let mut path = decorator_expr_path(&base.node);
            if path.is_empty() {
                DecoratorValue::Unsupported {
                    reason: "decorator field expression is not a symbolic path".to_string(),
                }
            } else {
                path.push(field.clone());
                DecoratorValue::SymbolRef { path }
            }
        }
        Expr::List(entries) => DecoratorValue::List {
            items: entries
                .iter()
                .map(|entry| match entry {
                    ListEntry::Element(value) => decorator_expr_value(value, checker),
                    ListEntry::Spread(value) => DecoratorValue::Unsupported {
                        reason: format!(
                            "decorator list spread `{}` is not declaration-safe metadata",
                            decorator_expr_label(&value.node)
                        ),
                    },
                })
                .collect(),
        },
        Expr::Dict(entries) => {
            let mut metadata_entries = Vec::new();
            for entry in entries {
                match entry {
                    DictEntry::Pair(key, value) => metadata_entries.push(DecoratorDictEntry {
                        key: decorator_expr_value(key, checker),
                        value: decorator_expr_value(value, checker),
                    }),
                    DictEntry::Spread(value) => metadata_entries.push(DecoratorDictEntry {
                        key: DecoratorValue::Unsupported {
                            reason: "decorator dict spread has no declaration-safe key".to_string(),
                        },
                        value: decorator_expr_value(value, checker),
                    }),
                }
            }
            DecoratorValue::Dict {
                entries: metadata_entries,
            }
        }
        Expr::Call(callee, type_args, args) => {
            let path = decorator_expr_path(&callee.node);
            if path.is_empty() {
                return DecoratorValue::Unsupported {
                    reason: "decorator call callee is not a symbolic path".to_string(),
                };
            }
            DecoratorValue::Call {
                callee: path,
                type_args: type_args
                    .iter()
                    .map(|type_arg| {
                        type_ref_from_resolved(&crate::frontend::symbols::resolve_type(
                            &type_arg.node,
                            &checker.symbols,
                        ))
                    })
                    .collect(),
                args: args
                    .iter()
                    .map(|arg| decorator_call_arg_metadata(arg, checker))
                    .collect(),
            }
        }
        Expr::Constructor(name, args) => DecoratorValue::Call {
            callee: vec![name.clone()],
            type_args: Vec::new(),
            args: args
                .iter()
                .map(|arg| decorator_call_arg_metadata(arg, checker))
                .collect(),
        },
        _ => DecoratorValue::Unsupported {
            reason: "decorator argument is not a literal, const reference, or type".to_string(),
        },
    }
}

/// Convert a decorator call argument into API metadata.
fn decorator_call_arg_metadata(arg: &CallArg, checker: &TypeChecker) -> DecoratorCallArgMetadata {
    match arg {
        CallArg::Positional(value) => DecoratorCallArgMetadata::Positional {
            value: decorator_expr_value(value, checker),
        },
        CallArg::Named(name, value) => DecoratorCallArgMetadata::Named {
            name: name.clone(),
            value: decorator_expr_value(value, checker),
        },
        CallArg::PositionalUnpack(value) => DecoratorCallArgMetadata::PositionalUnpack {
            value: decorator_expr_value(value, checker),
        },
        CallArg::KeywordUnpack(value) => DecoratorCallArgMetadata::KeywordUnpack {
            value: decorator_expr_value(value, checker),
        },
    }
}

/// Return the source path represented by a decorator expression.
fn decorator_expr_path(expr: &Expr) -> Vec<String> {
    match expr {
        Expr::Ident(name) => vec![name.clone()],
        Expr::Field(base, field) => {
            let mut path = decorator_expr_path(&base.node);
            if path.is_empty() {
                return Vec::new();
            }
            path.push(field.clone());
            path
        }
        _ => Vec::new(),
    }
}

/// Return a stable label for a decorator expression shape.
fn decorator_expr_label(expr: &Expr) -> &'static str {
    match expr {
        Expr::Ident(_) => "identifier",
        Expr::Literal(_) => "literal",
        Expr::Call(_, _, _) | Expr::Constructor(_, _) => "call",
        Expr::List(_) => "list",
        Expr::Dict(_) => "dict",
        Expr::Field(_, _) => "field",
        _ => "expression",
    }
}

/// Convert a literal into the safe metadata subset used by checked API output.
fn safe_value_from_literal(literal: &crate::frontend::ast::Literal) -> SafeMetadataValue {
    match literal {
        crate::frontend::ast::Literal::Int(value) => SafeMetadataValue::Int(value.value),
        crate::frontend::ast::Literal::Float(value) => SafeMetadataValue::Float(value.value),
        crate::frontend::ast::Literal::Decimal(value) => SafeMetadataValue::String(value.repr.clone()),
        crate::frontend::ast::Literal::String(value) => SafeMetadataValue::String(value.clone()),
        crate::frontend::ast::Literal::Bytes(value) => SafeMetadataValue::Bytes(value.clone()),
        crate::frontend::ast::Literal::Bool(value) => SafeMetadataValue::Bool(*value),
        crate::frontend::ast::Literal::None => SafeMetadataValue::None,
    }
}

/// Convert a constant value into a safe metadata value.
fn safe_value_from_const(value: &ConstValue) -> SafeMetadataValue {
    match value {
        ConstValue::Int(value) => SafeMetadataValue::Int(*value),
        ConstValue::Float(value) => SafeMetadataValue::Float(*value),
        ConstValue::Bool(value) => SafeMetadataValue::Bool(*value),
        ConstValue::FrozenStr(value) => SafeMetadataValue::String(value.clone()),
        ConstValue::FrozenBytes(value) => SafeMetadataValue::Bytes(value.clone()),
    }
}

/// Extract the leading function docstring expression, when present.
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

/// Parse a source docstring into structured API documentation.
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

/// Return normalized docstring body lines.
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
    /// Map a docstring section heading to its parser state.
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
    /// Add a normalized docstring line to the active section.
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

    /// Build the completed structured docstring from accumulated lines.
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

/// Append a normalized prose line to a docstring section.
fn push_prose_line(lines: &mut Vec<String>, line: &str) {
    if line.is_empty() {
        if !lines.last().is_some_and(String::is_empty) {
            lines.push(String::new());
        }
        return;
    }
    lines.push(line.to_string());
}

/// Append a normalized entry line to a docstring section.
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

/// Parse a docstring return section into structured API documentation.
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

/// Join non-empty docstring lines into a single paragraph.
fn joined_non_empty(lines: Vec<String>) -> Option<String> {
    let joined = lines.join("\n").trim().to_string();
    if joined.is_empty() { None } else { Some(joined) }
}

/// Return whether a docstring fragment looks like a type spelling.
fn looks_like_type_spelling(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':' | '[' | ']' | ',' | ' ' | '&'))
}

/// Validate docstring coverage for declarations in one API metadata module.
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

/// Validate a callable docstring against its exported API shape.
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

/// Validate a type docstring against its exported API shape.
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

/// Validate one exported declaration docstring.
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

/// Validate the return section for a callable docstring.
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

/// Validate decorator documentation entries for an exported callable.
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

/// Validate alias documentation entries for exported declarations.
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

/// Return the expected docstring section name for a documented noun.
fn section_name_for_noun(noun: &str) -> &'static str {
    match noun {
        "parameter" => "Args:",
        "field" => "Fields:",
        "alias" => "Aliases:",
        "decorator" => "Decorators:",
        _ => "docstring",
    }
}

/// Record an API docstring diagnostic anchored to a source span.
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

/// Return method metadata attached to a class-like declaration.
fn declaration_methods(declaration: &ApiDeclaration) -> &[ApiMethod] {
    match declaration {
        ApiDeclaration::Model(model) => &model.methods,
        ApiDeclaration::Class(class) => &class.methods,
        ApiDeclaration::Trait(trait_decl) => &trait_decl.methods,
        ApiDeclaration::Newtype(newtype) => &newtype.methods,
        _ => &[],
    }
}

/// Return alias metadata exported by a checked package.
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

/// Return aliases that target a specific exported declaration.
fn aliases_for_declaration<'a>(aliases: &'a [ApiAlias], module_path: &[String], name: &str) -> Vec<&'a str> {
    aliases
        .iter()
        .filter(|alias| alias_targets_declaration(alias, module_path, name))
        .map(|alias| alias.name.as_str())
        .collect()
}

/// Return whether an alias path names a specific exported declaration.
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

/// Render a type reference as a docstring-facing type name.
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
        TypeRef::TypeToken { inner } => format!("Type[{}]", type_ref_doc_name(inner)),
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

/// Build a source anchor for an API metadata span.
fn anchor(module_path: &[String], name: &str, span: Span) -> SourceAnchor {
    let mut parts = module_path.to_vec();
    parts.push(name.to_string());
    SourceAnchor {
        id: parts.join("::"),
        span: source_span(span),
    }
}

/// Convert a concrete span into an API metadata source span.
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
    fn checked_api_metadata_preserves_decorated_function_source_signature() -> Result<(), String> {
        let source = r#"
def keep(func: (int) -> int) -> (int) -> int:
    return func

@keep
pub def decorated(value: int) -> int:
    """Return the input value.

    Args:
        value: Input value.
    """
    return value
"#;
        let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
        let function = metadata
            .declarations
            .iter()
            .find_map(|decl| match decl {
                ApiDeclaration::Function(function) if function.name == "decorated" => Some(function),
                _ => None,
            })
            .ok_or_else(|| "expected decorated function metadata".to_string())?;

        assert_eq!(function.params.len(), 1);
        assert_eq!(function.params[0].name, "value");
        assert_eq!(
            function.params[0].ty,
            TypeRef::Named {
                name: "int".to_string(),
            }
        );

        let diagnostics = validate_checked_api_docstrings(&[metadata]);
        assert!(
            diagnostics.is_empty(),
            "expected decorated source signature to satisfy docstring validation, got {diagnostics:?}"
        );
        Ok(())
    }

    #[test]
    fn checked_api_metadata_preserves_generic_decorator_factory_source_signature() -> Result<(), String> {
        let source = r#"
model ColumnExpr:
    name: str

def registered[F](name: str) -> ((F) -> F):
    return (func) => func

@registered("inql.functions.col")
pub def col(name: str) -> ColumnExpr:
    """Build a column expression.

    Args:
        name: Column name.
    """
    return ColumnExpr(name=name)
"#;
        let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
        let function = metadata
            .declarations
            .iter()
            .find_map(|decl| match decl {
                ApiDeclaration::Function(function) if function.name == "col" => Some(function),
                _ => None,
            })
            .ok_or_else(|| "expected decorated function metadata".to_string())?;

        assert_eq!(function.params.len(), 1);
        assert_eq!(function.params[0].name, "name");
        assert_eq!(
            function.params[0].ty,
            TypeRef::Named {
                name: "str".to_string(),
            }
        );
        assert_eq!(
            function.return_type,
            TypeRef::Named {
                name: "ColumnExpr".to_string(),
            }
        );

        let diagnostics = validate_checked_api_docstrings(&[metadata]);
        assert!(
            diagnostics.is_empty(),
            "expected generic decorator factory source signature to satisfy docstring validation, got {diagnostics:?}"
        );
        Ok(())
    }

    #[test]
    fn checked_api_metadata_projects_decorated_callable_context_issue694() -> Result<(), String> {
        let source = r#"
const EQUAL_FUNCTION_ANCHOR = "substrait.equal"

model ColumnExpr:
    name: str

model FunctionLifecycle:
    since: str
    changed: List[str]
    deprecated: Option[str]

def extension_mapping(name: str, anchor: str) -> str:
    return name

def deterministic_spec(kind: str, lifecycle: FunctionLifecycle, mapping: str) -> str:
    return kind

def registered[F](spec: str) -> ((F) -> F):
    return (func) => func

@registered(deterministic_spec("scalar", FunctionLifecycle(since="v0.3", changed=[], deprecated=None), extension_mapping("equal", EQUAL_FUNCTION_ANCHOR)))
pub def eq(left: ColumnExpr, right: ColumnExpr) -> ColumnExpr:
    return left
"#;
        let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
        let function = metadata
            .declarations
            .iter()
            .find_map(|decl| match decl {
                ApiDeclaration::Function(function) if function.name == "eq" => Some(function),
                _ => None,
            })
            .ok_or_else(|| "expected decorated function metadata".to_string())?;
        let decorator = function
            .decorators
            .first()
            .ok_or_else(|| "expected decorator metadata".to_string())?;
        let callable = decorator
            .decorated_callable
            .as_ref()
            .ok_or_else(|| "expected decorated callable context".to_string())?;

        assert_eq!(callable.name, "eq");
        assert_eq!(
            callable
                .params
                .iter()
                .map(|param| (param.name.as_str(), &param.ty))
                .collect::<Vec<_>>(),
            vec![
                (
                    "left",
                    &TypeRef::Named {
                        name: "ColumnExpr".to_string(),
                    },
                ),
                (
                    "right",
                    &TypeRef::Named {
                        name: "ColumnExpr".to_string(),
                    },
                ),
            ]
        );
        assert_eq!(
            callable.return_type,
            TypeRef::Named {
                name: "ColumnExpr".to_string(),
            }
        );

        let [
            DecoratorArgMetadata::Positional {
                value: DecoratorValue::Call { callee, args, .. },
            },
        ] = decorator.args.as_slice()
        else {
            return Err(format!(
                "expected structured decorator call metadata, got {decorator:?}"
            ));
        };
        assert_eq!(callee, &vec!["deterministic_spec".to_string()]);
        let lifecycle_args = args
            .iter()
            .find_map(|arg| match arg {
                DecoratorCallArgMetadata::Positional {
                    value: DecoratorValue::Call { callee, args, .. },
                } if callee == &vec!["FunctionLifecycle".to_string()] => Some(args),
                _ => None,
            })
            .ok_or_else(|| format!("expected nested lifecycle constructor call metadata, got {args:?}"))?;
        assert!(
            lifecycle_args.iter().any(|arg| matches!(
                arg,
                DecoratorCallArgMetadata::Named {
                    name,
                    value: DecoratorValue::List { items },
                } if name == "changed" && items.is_empty()
            )),
            "expected lifecycle `changed=[]` metadata, got {lifecycle_args:?}"
        );
        assert!(
            lifecycle_args.iter().any(|arg| matches!(
                arg,
                DecoratorCallArgMetadata::Named {
                    name,
                    value: DecoratorValue::Literal {
                        value: SafeMetadataValue::None,
                    },
                } if name == "deprecated"
            )),
            "expected lifecycle `deprecated=None` metadata, got {lifecycle_args:?}"
        );
        assert!(
            args.iter().any(|arg| matches!(
                arg,
                DecoratorCallArgMetadata::Positional {
                    value: DecoratorValue::Call { callee, args, .. },
                } if callee == &vec!["extension_mapping".to_string()]
                    && args.iter().any(|arg| matches!(
                        arg,
                        DecoratorCallArgMetadata::Positional {
                            value: DecoratorValue::ConstRef {
                                name,
                                value: Some(SafeMetadataValue::String(value)),
                            },
                        } if name == "EQUAL_FUNCTION_ANCHOR" && value == "substrait.equal"
                    ))
            )),
            "expected nested extension mapping call metadata with checked const ref, got {args:?}"
        );
        Ok(())
    }

    #[test]
    fn checked_api_metadata_rejects_non_symbolic_decorator_field_metadata() -> Result<(), String> {
        let source = r#"
model Holder:
    value: str

def holder() -> Holder:
    return Holder(value="equal")

def registered[F](name: str) -> ((F) -> F):
    return (func) => func

@registered(holder().value)
pub def eq(left: int, right: int) -> int:
    return left
"#;
        let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
        let function = metadata
            .declarations
            .iter()
            .find_map(|decl| match decl {
                ApiDeclaration::Function(function) if function.name == "eq" => Some(function),
                _ => None,
            })
            .ok_or_else(|| "expected decorated function metadata".to_string())?;
        let [
            DecoratorArgMetadata::Positional {
                value: DecoratorValue::Unsupported { reason },
            },
        ] = function.decorators[0].args.as_slice()
        else {
            return Err(format!(
                "expected non-symbolic field decorator argument to stay unsupported, got {:?}",
                function.decorators[0].args
            ));
        };

        assert_eq!(reason, "decorator field expression is not a symbolic path");
        Ok(())
    }

    #[test]
    fn checked_api_docstring_validation_matches_overloaded_method_by_params() -> Result<(), String> {
        let source = r#"
pub class Writer:
    def write(self, data: bytes) -> Result[int, str]:
        """
        Write raw bytes.

        Args:
            data: Bytes to write.
        """
        return Ok(len(data))

    def write(self, value: u8, _endian: str) -> Result[None, str]:
        return Ok(None)
"#;
        let tokens = lexer::lex(source).map_err(|errs| format!("{errs:?}"))?;
        let program = parser::parse(&tokens).map_err(|errs| format!("{errs:?}"))?;
        let class = program
            .declarations
            .iter()
            .find_map(|decl| match &decl.node {
                Declaration::Class(class) => Some(class),
                _ => None,
            })
            .ok_or_else(|| "expected class declaration".to_string())?;
        let checked_methods = vec![
            CheckedMethod {
                name: "write".to_string(),
                alias_of: None,
                type_params: Vec::new(),
                receiver: Some(crate::frontend::ast::Receiver::Immutable),
                params: vec![
                    crate::frontend::symbols::CallableParam::named(
                        "value",
                        crate::frontend::symbols::ResolvedType::Int,
                        crate::frontend::ast::ParamKind::Normal,
                    ),
                    crate::frontend::symbols::CallableParam::named(
                        "_endian",
                        crate::frontend::symbols::ResolvedType::Str,
                        crate::frontend::ast::ParamKind::Normal,
                    ),
                ],
                return_type: crate::frontend::symbols::ResolvedType::Unit,
                is_async: false,
                has_body: true,
            },
            CheckedMethod {
                name: "write".to_string(),
                alias_of: None,
                type_params: Vec::new(),
                receiver: Some(crate::frontend::ast::Receiver::Immutable),
                params: vec![crate::frontend::symbols::CallableParam::named(
                    "data",
                    crate::frontend::symbols::ResolvedType::Bytes,
                    crate::frontend::ast::ParamKind::Normal,
                )],
                return_type: crate::frontend::symbols::ResolvedType::Int,
                is_async: false,
                has_body: true,
            },
        ];
        let checker = typechecker::TypeChecker::new();
        let api_methods = methods(
            &class.methods,
            &checked_methods,
            &checker,
            &["demo".to_string()],
            "Writer",
        );
        assert_eq!(api_methods.len(), 2);
        assert_eq!(
            api_methods[0]
                .params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>(),
            vec!["data"]
        );
        assert_eq!(
            api_methods[1]
                .params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>(),
            vec!["value", "_endian"]
        );
        Ok(())
    }

    #[test]
    fn checked_api_docstring_validation_matches_overloaded_method_by_type_shape() -> Result<(), String> {
        let source = r#"
pub class Parser:
    def parse(self, value: str) -> str:
        """
        Parse text.
        """
        return value

    def parse(self, value: bytes) -> bytes:
        """
        Parse bytes.
        """
        return value
"#;
        let tokens = lexer::lex(source).map_err(|errs| format!("{errs:?}"))?;
        let program = parser::parse(&tokens).map_err(|errs| format!("{errs:?}"))?;
        let class = program
            .declarations
            .iter()
            .find_map(|decl| match &decl.node {
                Declaration::Class(class) => Some(class),
                _ => None,
            })
            .ok_or_else(|| "expected class declaration".to_string())?;
        let checked_methods = vec![
            CheckedMethod {
                name: "parse".to_string(),
                alias_of: None,
                type_params: Vec::new(),
                receiver: Some(crate::frontend::ast::Receiver::Immutable),
                params: vec![crate::frontend::symbols::CallableParam::named(
                    "value",
                    crate::frontend::symbols::ResolvedType::Bytes,
                    crate::frontend::ast::ParamKind::Normal,
                )],
                return_type: crate::frontend::symbols::ResolvedType::Bytes,
                is_async: false,
                has_body: true,
            },
            CheckedMethod {
                name: "parse".to_string(),
                alias_of: None,
                type_params: Vec::new(),
                receiver: Some(crate::frontend::ast::Receiver::Immutable),
                params: vec![crate::frontend::symbols::CallableParam::named(
                    "value",
                    crate::frontend::symbols::ResolvedType::Str,
                    crate::frontend::ast::ParamKind::Normal,
                )],
                return_type: crate::frontend::symbols::ResolvedType::Str,
                is_async: false,
                has_body: true,
            },
        ];
        let checker = typechecker::TypeChecker::new();
        let api_methods = methods(
            &class.methods,
            &checked_methods,
            &checker,
            &["demo".to_string()],
            "Parser",
        );
        assert_eq!(api_methods.len(), 2);
        assert_eq!(
            api_methods[0].return_type,
            TypeRef::Named {
                name: "str".to_string()
            }
        );
        assert_eq!(
            api_methods[0].docstring.as_deref().unwrap_or_default().trim(),
            "Parse text."
        );
        assert_eq!(
            api_methods[1].return_type,
            TypeRef::Named {
                name: "bytes".to_string()
            }
        );
        assert_eq!(
            api_methods[1].docstring.as_deref().unwrap_or_default().trim(),
            "Parse bytes."
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

pub trait Labelled:
    def label(self) -> str: ...


@derive(Clone)
pub model Order with Labelled:
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
        assert_eq!(model.traits, vec!["Labelled".to_string()]);
        assert_eq!(model.trait_adoptions.len(), 1);
        assert_eq!(model.trait_adoptions[0].name, "Labelled");
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

    #[test]
    fn checked_api_metadata_extracts_public_partial_callable_preset() -> Result<(), String> {
        let source = r#"
pub def route(method: str, path: str = "/") -> str:
    return path

pub get = partial route(method="GET")
"#;
        let metadata = metadata_for(source).map_err(|errs| format!("{errs:?}"))?;
        let partial = metadata
            .declarations
            .iter()
            .find_map(|decl| match decl {
                ApiDeclaration::Partial(partial) => Some(partial),
                _ => None,
            })
            .ok_or_else(|| "expected partial metadata".to_string())?;

        assert_eq!(partial.name, "get");
        assert_eq!(partial.anchor.id, "demo::get");
        assert_eq!(partial.target_path, vec!["route".to_string()]);
        assert_eq!(partial.target_kind, PartialTargetKindExport::Function);
        assert_eq!(partial.presets.len(), 1);
        assert_eq!(partial.presets[0].name, "method");
        assert_eq!(
            partial
                .params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>(),
            vec!["method", "path"]
        );
        assert!(
            partial.params[0].has_default,
            "partial-projected callable params should preserve ordinary default display metadata"
        );
        assert!(
            partial.params[1].has_default,
            "ordinary target defaults should remain visually distinct on partial metadata"
        );
        Ok(())
    }
}
