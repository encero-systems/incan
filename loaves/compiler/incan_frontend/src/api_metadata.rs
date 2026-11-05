//! Checked public API metadata extraction for RFC 048.
//!
//! This module builds a JSON-ready model from parsed and typechecked Incan semantics. It deliberately reuses the
//! manifest type vocabulary instead of stringifying checked types, so package artifacts, CLI output, and later docs
//! tooling can share one structural representation.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ast::{
    CallArg, ClassDecl, Declaration, Decorator, DecoratorArg, DecoratorArgValue, DictEntry, EnumDecl, Expr, FieldDecl,
    FunctionDecl, ImportDecl, ImportItem, ImportKind, ListEntry, MethodAliasDecl, MethodDecl, ModelDecl, NewtypeDecl,
    Program, Span, Spanned, Statement, TraitDecl, TypeAliasDecl, Visibility,
};
use crate::decorator_resolution;
use crate::diagnostics::CompileError;
use crate::library_exports::{
    CheckedClassExport, CheckedConstExport, CheckedEnumExport, CheckedExportKind, CheckedExportProjection,
    CheckedField, CheckedFunctionExport, CheckedMethod, CheckedModelExport, CheckedNamedExport, CheckedNewtypeExport,
    CheckedPartialExport, CheckedPartialTargetKind, CheckedPresetValue, CheckedProperty, CheckedTraitExport,
    CheckedTypeAliasExport, CheckedTypeBound, CheckedTypeParam, collect_checked_public_exports,
};
use crate::library_manifest::{
    CanonicalIdentityExport, ClassExport, EnumExport, EnumValueExport, EnumValueTypeExport, EnumVariantAliasExport,
    EnumVariantExport, FieldExport, FieldRequirementExport, FunctionExport, ImplementationAssociatedTypeExport,
    ImplementationTraitBoundExport, ImplementationTraitBoundOriginExport, ImplementationTypeParamExport, MethodExport,
    ModelExport, NewtypeConstraintExport, NewtypeExport, ParamExport, PartialExport, PartialPresetExport,
    PartialTargetKindExport, PresetDictEntryExport, PresetModelFieldExport, PresetValueExport, PropertyExport,
    ReceiverExport, TraitExport, TypeAliasExport, TypeBoundExport, TypeParamExport, TypeRef,
    param_default_from_checked, params_from_checked, type_ref_from_resolved,
};
use crate::module::canonicalize_source_module_segments;
use crate::symbols::{ImplementationTraitBoundInfo, ImplementationTraitBoundOriginInfo, ImplementationTypeParamInfo};
use crate::typechecker::{ConstValue, TypeChecker};
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

mod checked_collection;
mod declaration_builders;
mod decorator_metadata;
mod docstrings;
mod export_conversion;
mod member_metadata;
mod partial_defaults;
mod public_namespaces;

pub use checked_collection::*;
use declaration_builders::*;
use decorator_metadata::*;
pub use docstrings::*;
pub use export_conversion::*;
use member_metadata::*;
pub use partial_defaults::{manifest_partial_with_target_defaults, partial_export_with_target_defaults};
pub use public_namespaces::*;

/// Schema version of checked API metadata packages and modules.
///
/// Version 2 adds the `mut` marker: `is_mut` on a callable parameter and the `MutParam` type reference inside a
/// function type (#1790).
pub const CHECKED_API_METADATA_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckedApiMetadataPackage {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<CheckedApiPackageIdentity>,
    pub modules: Vec<CheckedApiMetadata>,
    /// Materialized public namespace projection consumed at package boundaries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub public_namespaces: Vec<CheckedApiPublicNamespace>,
}

/// One public package namespace materialized by the producer from its checked source-module graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedApiPublicNamespace {
    pub path: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<CheckedApiPublicNamespaceMember>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_modules: Vec<String>,
}

/// One declaration exposed through a materialized public package namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedApiPublicNamespaceMember {
    pub name: String,
    /// Exact producer-local module path followed by the declaration name.
    pub source_path: Vec<String>,
}

/// Failure while materializing or validating one checked package's public namespace graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedApiPublicNamespaceError {
    DuplicateSourceModule { module_path: Vec<String> },
    MissingMaterializedNamespace { module_path: Vec<String> },
    SerializedProjectionMismatch,
}

impl fmt::Display for CheckedApiPublicNamespaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateSourceModule { module_path } => write!(
                formatter,
                "multiple source files resolve to checked module `{}`",
                module_path.join(".")
            ),
            Self::MissingMaterializedNamespace { module_path } => write!(
                formatter,
                "checked module `{}` is missing from its materialized public namespace graph",
                module_path.join(".")
            ),
            Self::SerializedProjectionMismatch => {
                formatter.write_str("serialized public namespace projection does not match checked API modules")
            }
        }
    }
}

impl std::error::Error for CheckedApiPublicNamespaceError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedApiPackageIdentity {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckedApiMetadata {
    pub schema_version: u32,
    /// Exact module-owned RFC 024 derive membership; names resolve in this module's checked declarations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derivable_traits: Vec<String>,
    pub module_path: Vec<String>,
    pub declarations: Vec<ApiDeclaration>,
}

/// Source coordinates and generated declaration identity retained by checked API publication.
pub use incan_semantics_core::HirSourceSpan as SourceSpan;

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
    /// Explicit checked ownership and result-source metadata, retained through API projections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership: Option<crate::library_manifest::ownership::OwnershipExport>,
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
    /// Canonical Rust path of the attribute macro this `@rust.extern` declaration names, as the providing unit's Rust
    /// facts inspected it. A consumer's decorator that resolves here applies that macro to the decorated item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_attribute: Option<String>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<ApiProperty>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<ApiProperty>,
    pub methods: Vec<ApiMethod>,
}

/// Public computed-property metadata in checked API output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiProperty {
    pub name: String,
    /// Canonical member declaration identity, absent only for legacy or non-package metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<CanonicalIdentityExport>,
    pub return_type: TypeRef,
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
    /// Canonical variant declaration identity, absent only for legacy or non-package metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<CanonicalIdentityExport>,
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
    /// Source-level `@derive(...)` names declared by this newtype.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derives: Vec<String>,
    pub is_rusttype: bool,
    pub underlying: TypeRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_constructor: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<NewtypeConstraintExport>,
    #[serde(default = "default_newtype_implicit_coercion")]
    pub implicit_coercion_enabled: bool,
    pub methods: Vec<ApiMethod>,
}

/// Preserve the pre-field API metadata behavior when older payloads omit the coercion flag.
fn default_newtype_implicit_coercion() -> bool {
    true
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
    /// Whether the source import or alias explicitly participates in the package's public API.
    #[serde(default)]
    pub is_public: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected_function: Option<ApiProjectedFunction>,
    /// Checked nominal target retained by a public type re-export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected_type: Option<TypeRef>,
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
    /// Explicit checked ownership and result-source metadata, retained through API projections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership: Option<crate::library_manifest::ownership::OwnershipExport>,
    pub name: String,
    /// Canonical member declaration identity, distinct for same-name overloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<CanonicalIdentityExport>,
    /// Source-level method visibility (#2570); API metadata that omits it keeps the method public.
    #[serde(
        default,
        skip_serializing_if = "crate::library_manifest::MemberVisibilityExport::is_public"
    )]
    pub visibility: crate::library_manifest::MemberVisibilityExport,
    /// Canonical method targeted by this same-type alias, when this entry projects an alias rather than a body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias_of: Option<String>,
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
    /// Explicit checked ownership and result-source metadata, retained through API projections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership: Option<crate::library_manifest::ownership::OwnershipExport>,
    pub name: String,
    pub anchor: SourceAnchor,
    pub type_params: Vec<TypeParamExport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<ReceiverExport>,
    pub params: Vec<ParamExport>,
    pub return_type: TypeRef,
    pub is_async: bool,
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

#[derive(Debug)]
struct ApiAliasProjectionRequest {
    path: Vec<String>,
    /// The declaration path the alias's target names, resolved against the module that declares the alias.
    target_path: Vec<String>,
    name: String,
    anchor: SourceAnchor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocstringSection {
    Summary,
    Unstructured,
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
            "Example:" => Some(Self::Unstructured),
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
            DocstringSection::Unstructured => {}
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

#[cfg(test)]
mod tests;
