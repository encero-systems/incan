//! Method, parameter, function, type alias, model, class, trait, enum, newtype, const and static exports.

use super::*;

/// Exported method signature metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodExport {
    /// Checked explicit access and result-source metadata; absence leaves the ordinary inferred contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership: Option<super::ownership::OwnershipExport>,
    pub name: String,
    /// Canonical member declaration identity, distinct for same-name overloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<CanonicalIdentityExport>,
    /// Source-level method visibility (#2570). Older manifests omitted it, and every method was public to their
    /// consumers, so absence remains public; new manifests serialize only the private case.
    #[serde(default, skip_serializing_if = "MemberVisibilityExport::is_public")]
    pub visibility: MemberVisibilityExport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias_of: Option<String>,
    pub type_params: Vec<TypeParamExport>,
    /// Receiver requirement when the method is invoked on a type instance.
    pub receiver: Option<ReceiverExport>,
    pub params: Vec<ParamExport>,
    pub return_type: TypeRef,
    /// Whether the method is declared `async`.
    pub is_async: bool,
    /// Whether the originating declaration included a body.
    pub has_body: bool,
}

/// One exported callable parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamExport {
    pub name: String,
    pub ty: TypeRef,
    /// Authored alias-bearing type spelling retained only for presentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_type_name: Option<String>,
    #[serde(default)]
    pub kind: ParamKindExport,
    #[serde(default)]
    pub has_default: bool,
    /// Whether the function's type marks the parameter `mut`: a `mut` parameter whose changes the caller sees
    /// (#1790). A `mut` parameter of type `int`, `float` or `bool`, or of a Rust type, is not marked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_mut: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<ParamDefaultExport>,
}

/// Metadata-safe callable parameter default expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ParamDefaultExport {
    Int(i64),
    Float(String),
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
    None,
    List(Vec<ParamDefaultExport>),
    Dict(Vec<ParamDefaultDictEntryExport>),
    ConstRef(Vec<String>),
    Call {
        path: Vec<String>,
        args: Vec<ParamDefaultCallArgExport>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<Box<ParamDefaultCallSignatureExport>>,
    },
    /// A replayable static method call used as a parameter default.
    MethodCall {
        receiver_path: Vec<String>,
        method: String,
        args: Vec<ParamDefaultCallArgExport>,
    },
    Unsupported,
}

impl ParamDefaultExport {
    /// Return whether a consumer can materialize this exported default expression at its own call site.
    pub fn is_materializable(&self) -> bool {
        match self {
            Self::Int(_) | Self::Float(_) | Self::Bool(_) | Self::String(_) | Self::Bytes(_) | Self::None => true,
            Self::ConstRef(path) => !path.is_empty(),
            Self::List(values) => values.iter().all(Self::is_materializable),
            Self::Dict(entries) => entries
                .iter()
                .all(|entry| entry.key.is_materializable() && entry.value.is_materializable()),
            Self::Call { path, args, .. } => !path.is_empty() && args.iter().all(|arg| arg.value.is_materializable()),
            Self::MethodCall {
                receiver_path,
                method,
                args,
            } => {
                !receiver_path.is_empty() && !method.is_empty() && args.iter().all(|arg| arg.value.is_materializable())
            }
            Self::Unsupported => false,
        }
    }
}

/// One metadata-safe dict default entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamDefaultDictEntryExport {
    pub key: ParamDefaultExport,
    pub value: ParamDefaultExport,
}

/// One metadata-safe call default argument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamDefaultCallArgExport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub value: ParamDefaultExport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamDefaultCallSignatureExport {
    pub params: Vec<ParamExport>,
    pub return_type: TypeRef,
}

/// Exported callable parameter kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ParamKindExport {
    #[default]
    Normal,
    RestPositional,
    RestKeyword,
}

/// Exported function signature metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionExport {
    /// Checked explicit access and result-source metadata; absence leaves the ordinary inferred contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership: Option<super::ownership::OwnershipExport>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emitted_name: Option<String>,
    pub type_params: Vec<TypeParamExport>,
    pub params: Vec<ParamExport>,
    pub return_type: TypeRef,
    pub is_async: bool,
}

/// Exported type-alias metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeAliasExport {
    pub name: String,
    pub type_params: Vec<TypeParamExport>,
    pub target: TypeRef,
}

/// Exported model metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelExport {
    pub name: String,
    pub type_params: Vec<TypeParamExport>,
    /// Traits implemented by the model.
    pub traits: Vec<String>,
    /// Traits implemented by the model, including generic trait arguments when present.
    #[serde(default)]
    pub trait_adoptions: Vec<TypeBoundExport>,
    /// `@derive(...)` names (empty for manifests predating this field).
    #[serde(default)]
    pub derives: Vec<String>,
    pub fields: Vec<FieldExport>,
    /// Public computed properties (empty for manifests predating this field).
    #[serde(default)]
    pub properties: Vec<PropertyExport>,
    pub methods: Vec<MethodExport>,
}

/// Exported class metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassExport {
    pub name: String,
    pub type_params: Vec<TypeParamExport>,
    /// Optional base class name.
    pub extends: Option<String>,
    /// Traits implemented by the class.
    pub traits: Vec<String>,
    /// Traits implemented by the class, including generic trait arguments when present.
    #[serde(default)]
    pub trait_adoptions: Vec<TypeBoundExport>,
    /// `@derive(...)` names (empty for manifests predating this field).
    #[serde(default)]
    pub derives: Vec<String>,
    pub fields: Vec<FieldExport>,
    /// Public computed properties (empty for manifests predating this field).
    #[serde(default)]
    pub properties: Vec<PropertyExport>,
    pub methods: Vec<MethodExport>,
}

/// Exported trait metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraitExport {
    pub name: String,
    /// Original source declaration name before a library reexport alias, when it differs from `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_name: Option<String>,
    pub type_params: Vec<TypeParamExport>,
    /// Direct supertraits from the trait's `with` clause (RFC 042).
    #[serde(default)]
    pub supertraits: Vec<TypeBoundExport>,
    /// Required fields a conforming type must provide.
    pub requires: Vec<FieldRequirementExport>,
    pub methods: Vec<MethodExport>,
}

/// One required field published by an exported trait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldRequirementExport {
    pub name: String,
    pub ty: TypeRef,
}

/// Exported enum metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumExport {
    pub name: String,
    pub type_params: Vec<TypeParamExport>,
    /// Traits implemented by the enum.
    #[serde(default)]
    pub traits: Vec<String>,
    /// Traits implemented by the enum, including generic trait arguments when present.
    #[serde(default)]
    pub trait_adoptions: Vec<TypeBoundExport>,
    /// Primitive backing type for RFC 032 value enums.
    #[serde(default)]
    pub value_type: Option<EnumValueTypeExport>,
    /// Stable `OrdinalKey` type identity used by value-enum serialized maps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinal_type_identity: Option<String>,
    pub variants: Vec<EnumVariantExport>,
    /// Variant aliases exposed by this enum.
    #[serde(default)]
    pub variant_aliases: Vec<EnumVariantAliasExport>,
    /// Methods and associated functions exposed by the enum.
    #[serde(default)]
    pub methods: Vec<MethodExport>,
    /// `@derive(...)` names (empty for manifests predating this field).
    #[serde(default)]
    pub derives: Vec<String>,
}

/// Exported backing type for a value enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnumValueTypeExport {
    #[serde(rename = "str")]
    Str,
    #[serde(rename = "int")]
    Int,
}

/// One exported enum variant, including positional payload fields and optional value-enum metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumVariantExport {
    pub name: String,
    /// Canonical enum-member declaration identity, absent only for legacy manifests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<CanonicalIdentityExport>,
    pub fields: Vec<TypeRef>,
    /// Raw value for RFC 032 value enum variants.
    #[serde(default)]
    pub value: Option<EnumValueExport>,
}

/// Exported alias for one enum variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumVariantAliasExport {
    pub name: String,
    pub target: String,
}

/// Exported raw value for one value enum variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EnumValueExport {
    Str(String),
    Int(i64),
}

/// Exported newtype metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewtypeExport {
    pub name: String,
    pub type_params: Vec<TypeParamExport>,
    /// Direct trait names adopted by this newtype/rusttype.
    #[serde(default)]
    pub traits: Vec<String>,
    /// Direct trait adoptions, preserving type arguments for generic traits.
    #[serde(default)]
    pub trait_adoptions: Vec<TypeBoundExport>,
    /// Source-level `@derive(...)` names declared by this newtype.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derives: Vec<String>,
    /// Whether this newtype is a zero-cost Rust type alias (`type X = rusttype RustX`).
    #[serde(default)]
    pub is_rusttype: bool,
    /// Underlying wrapped type.
    pub underlying: TypeRef,
    /// Canonical checked-construction hook selected by the producer typechecker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_constructor: Option<String>,
    /// Checked RFC 017 constrained-primitive predicates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<NewtypeConstraintExport>,
    /// Whether ordinary implicit construction from the underlying value is allowed.
    #[serde(default = "default_newtype_implicit_coercion")]
    pub implicit_coercion_enabled: bool,
    pub methods: Vec<MethodExport>,
}

/// Serialized comparison key for one constrained-newtype predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewtypeConstraintKeyExport {
    Ge,
    Gt,
    Le,
    Lt,
}

/// One checked constrained-primitive predicate exported for package consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewtypeConstraintExport {
    pub key: NewtypeConstraintKeyExport,
    pub value: i64,
    pub repr: String,
}

impl NewtypeConstraintExport {
    /// Convert a checked frontend predicate into its stable manifest representation.
    pub fn from_checked(constraint: &NewtypePrimitiveConstraint) -> Self {
        let key = match constraint.key {
            crate::ast::TypeConstraintKey::Ge => NewtypeConstraintKeyExport::Ge,
            crate::ast::TypeConstraintKey::Gt => NewtypeConstraintKeyExport::Gt,
            crate::ast::TypeConstraintKey::Le => NewtypeConstraintKeyExport::Le,
            crate::ast::TypeConstraintKey::Lt => NewtypeConstraintKeyExport::Lt,
        };
        Self {
            key,
            value: constraint.value,
            repr: constraint.repr.clone(),
        }
    }

    /// Reconstruct the checked frontend predicate represented by this manifest entry.
    pub fn to_checked(&self) -> NewtypePrimitiveConstraint {
        let key = match self.key {
            NewtypeConstraintKeyExport::Ge => crate::ast::TypeConstraintKey::Ge,
            NewtypeConstraintKeyExport::Gt => crate::ast::TypeConstraintKey::Gt,
            NewtypeConstraintKeyExport::Le => crate::ast::TypeConstraintKey::Le,
            NewtypeConstraintKeyExport::Lt => crate::ast::TypeConstraintKey::Lt,
        };
        NewtypePrimitiveConstraint {
            key,
            value: self.value,
            repr: self.repr.clone(),
        }
    }
}

/// Preserve the pre-field manifest behavior when older manifests omit the coercion flag.
pub(super) fn default_newtype_implicit_coercion() -> bool {
    true
}

/// Exported constant metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstExport {
    pub name: String,
    pub ty: TypeRef,
}

/// Exported static metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticExport {
    pub name: String,
    pub ty: TypeRef,
}
