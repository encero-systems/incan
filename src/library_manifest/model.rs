use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::type_refs::type_ref_from_resolved;
use super::validation::validate_raw_manifest;
use super::wire::RawLibraryManifest;
use super::{
    DslSurface, LIBRARY_MANIFEST_FORMAT, RUST_ABI_SCHEMA_VERSION, VocabKeywordRegistration, VocabProviderManifest,
};
use crate::frontend::api_metadata::CheckedApiMetadataPackage;
use crate::frontend::contract_metadata::ContractMetadataPackage as ModelContractMetadataPackage;
use crate::frontend::library_exports::{
    CheckedAliasExport, CheckedClassExport, CheckedConstExport, CheckedEnumExport, CheckedExportKind,
    CheckedFunctionExport, CheckedModelExport, CheckedNamedExport, CheckedNewtypeExport, CheckedParamDefault,
    CheckedParamDefaultCallSignature, CheckedPartialExport, CheckedPartialTargetKind, CheckedPresetValue,
    CheckedStaticExport, CheckedTraitExport, CheckedTypeAliasExport, CheckedTypeBound, CheckedTypeParam,
};
use crate::frontend::symbols::{CallableParam, ValueEnumBacking, ValueEnumValue};
use incan_core::interop::RustItemMetadata;

/// Errors surfaced while reading, writing, parsing, serializing, or validating `.incnlib` manifests.
#[derive(Debug, thiserror::Error)]
pub enum LibraryManifestError {
    /// Reading manifest contents from disk failed.
    #[error("failed to read {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    /// Writing manifest contents to disk failed.
    #[error("failed to write {path}: {source}")]
    Write { path: PathBuf, source: std::io::Error },
    /// The manifest payload could not be decoded from its transport format.
    #[error("failed to parse library manifest: {0}")]
    Parse(String),
    /// The manifest payload could not be encoded into its transport format.
    #[error("failed to serialize library manifest: {0}")]
    Serialize(String),
    /// The manifest decoded successfully but violated semantic validation rules.
    #[error("invalid library manifest: {0}")]
    Invalid(String),
}

/// Semantic representation of one library manifest (`.incnlib`).
///
/// This is the compiler-facing form used after the raw transport payload has been validated and decoded. It captures
/// the exported library surface, optional vocab metadata, and optional soft-keyword activations in a transport-agnostic
/// shape.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryManifest {
    /// Published library name.
    pub name: String,
    /// Published library version.
    pub version: String,
    /// Minimum compiler version expected by the manifest payload.
    pub incan_version: String,
    /// Stable manifest-format discriminator for on-disk compatibility.
    pub manifest_format: u32,
    /// Public exports visible to consumers of the library.
    pub exports: LibraryExports,
    /// Optional vocab-provider metadata for DSL registration and desugaring.
    pub vocab: Option<VocabExports>,
    /// Optional soft-keyword activations exported by this library.
    pub soft_keywords: SoftKeywordExports,
    /// Optional RFC 048 checked metadata embedded in the manifest.
    pub contract_metadata: LibraryContractMetadata,
    /// Optional Rust-backed ABI metadata captured at library publication time.
    pub rust_abi: Option<LibraryRustAbi>,
}

/// Public library exports grouped by declaration kind.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LibraryExports {
    pub aliases: Vec<AliasExport>,
    pub partials: Vec<PartialExport>,
    pub models: Vec<ModelExport>,
    pub classes: Vec<ClassExport>,
    pub functions: Vec<FunctionExport>,
    pub traits: Vec<TraitExport>,
    pub enums: Vec<EnumExport>,
    pub type_aliases: Vec<TypeAliasExport>,
    pub newtypes: Vec<NewtypeExport>,
    pub consts: Vec<ConstExport>,
    pub statics: Vec<StaticExport>,
}

/// Exported declaration-level alias metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AliasExport {
    pub name: String,
    pub target_path: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected_function: Option<FunctionExport>,
}

/// Exported partial callable preset metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialExport {
    pub name: String,
    pub target_path: Vec<String>,
    pub target_kind: PartialTargetKindExport,
    pub presets: Vec<PartialPresetExport>,
    pub type_params: Vec<TypeParamExport>,
    pub params: Vec<ParamExport>,
    pub return_type: TypeRef,
    pub is_async: bool,
}

/// Semantic kind of the callable target projected by a public partial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartialTargetKindExport {
    Function,
    ModelConstructor,
    ClassConstructor,
    NewtypeConstructor,
    Partial,
    Unknown,
}

/// One preset keyword published by a partial callable preset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialPresetExport {
    pub name: String,
    pub ty: TypeRef,
    pub value: PresetValueExport,
}

/// Metadata-safe preset expression value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PresetValueExport {
    Int(i64),
    Float(String),
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
    None,
    List(Vec<PresetValueExport>),
    Dict(Vec<PresetDictEntryExport>),
    ConstRef(Vec<String>),
    ModelLiteral {
        name: String,
        fields: Vec<PresetModelFieldExport>,
    },
    Unsupported,
}

/// One metadata-safe dict preset entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetDictEntryExport {
    pub key: PresetValueExport,
    pub value: PresetValueExport,
}

/// One metadata-safe model-literal preset field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetModelFieldExport {
    pub name: String,
    pub value: PresetValueExport,
}

/// RFC 048 metadata persisted into `.incnlib`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LibraryContractMetadata {
    /// Canonical model bundles that this artifact publishes.
    #[serde(default)]
    pub models: ModelContractMetadataPackage,
    /// Checked public API metadata extracted from the producer source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<CheckedApiMetadataPackage>,
}

/// Versioned Rust ABI payload persisted into `.incnlib`.
///
/// The payload stores the same backend-neutral metadata shape that `rust_inspect` extracts, but ships it with the
/// library artifact so consumers can resolve Rust-backed imports without loading the producer workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryRustAbi {
    /// Serialized ABI schema version.
    #[serde(default = "default_rust_abi_schema_version")]
    pub schema_version: u32,
    /// Canonical Rust item metadata keyed by `RustItemMetadata::canonical_path`.
    #[serde(default)]
    pub items: Vec<RustItemMetadata>,
}

/// Default Rust ABI schema version for manifest payloads that predate explicit serde fields.
fn default_rust_abi_schema_version() -> u32 {
    RUST_ABI_SCHEMA_VERSION
}

impl LibraryRustAbi {
    /// Build a deterministic ABI payload from extracted Rust metadata.
    pub fn from_items(mut items: Vec<RustItemMetadata>) -> Option<Self> {
        items.sort_by(|left, right| left.canonical_path.cmp(&right.canonical_path));
        items.dedup_by(|left, right| left.canonical_path == right.canonical_path);
        if items.is_empty() {
            return None;
        }
        Some(Self {
            schema_version: RUST_ABI_SCHEMA_VERSION,
            items,
        })
    }

    /// Return metadata for one canonical Rust path.
    pub fn get(&self, canonical_path: &str) -> Option<&RustItemMetadata> {
        self.items.iter().find(|item| {
            item.canonical_path == canonical_path || item.definition_path.as_deref() == Some(canonical_path)
        })
    }
}

/// Soft keywords that become active when the library is imported.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SoftKeywordExports {
    pub activations: Vec<SoftKeywordActivation>,
}

/// Optional vocab companion metadata packaged with the library manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VocabExports {
    /// Relative crate path to the vocab companion source inside the producer workspace.
    pub crate_path: String,
    /// Cargo package name of the vocab companion crate.
    pub package_name: String,
    /// Keywords registered by the vocab provider.
    pub keyword_registrations: Vec<VocabKeywordRegistration>,
    /// Declarative surface descriptions exported by the vocab provider.
    pub dsl_surfaces: Vec<DslSurface>,
    /// Provider-side manifest used by desugarers and helper binding resolution.
    pub provider_manifest: VocabProviderManifest,
    /// Optional packaged desugarer artifact used at compile time.
    pub desugarer_artifact: Option<VocabDesugarerArtifact>,
}

/// Packaged compile-time desugarer artifact metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabDesugarerArtifact {
    /// Artifact representation understood by the compiler.
    pub artifact_kind: incan_vocab::DesugarerArtifactKind,
    /// ABI version expected by the artifact/host bridge.
    #[serde(default = "default_wasm_desugar_abi_version")]
    pub abi_version: u32,
    /// Normalized relative path from the packaged crate root to the artifact file.
    pub relative_path: String,
    /// Target triple used to build the artifact.
    pub target: String,
    /// Cargo profile used to build the artifact.
    pub profile: String,
    /// Exported desugarer entrypoint symbol the host should invoke.
    pub entrypoint: String,
    /// SHA-256 digest used to verify the packaged artifact on the consumer side.
    pub sha256: String,
}

fn default_wasm_desugar_abi_version() -> u32 {
    incan_vocab::WASM_DESUGAR_ABI_VERSION
}

/// One import-activated soft keyword exported by a library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoftKeywordActivation {
    /// Namespace whose import activates the keyword.
    pub namespace: String,
    /// Soft keyword lexeme activated by that namespace.
    pub keyword: String,
}

/// One exported generic type parameter and its bounds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeParamExport {
    pub name: String,
    pub bounds: Vec<TypeBoundExport>,
}

/// One exported generic bound attached to a type parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeBoundExport {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_path: Option<Vec<String>>,
    pub type_args: Vec<TypeRef>,
}

/// Stable manifest-level type reference used by library exports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeRef {
    /// A named non-generic type such as `User` or `int`.
    Named { name: String },
    /// A generic application such as `List[str]`.
    Applied { name: String, args: Vec<TypeRef> },
    /// A function type with positional parameter and return types.
    Function {
        params: Vec<TypeRef>,
        return_type: Box<TypeRef>,
    },
    /// A tuple type.
    Tuple { elements: Vec<TypeRef> },
    /// A generic type parameter reference.
    TypeParam { name: String },
    /// The receiver type used in methods/traits.
    SelfType,
    /// A reference type.
    Ref { inner: Box<TypeRef> },
    /// A canonical Rust path imported through `rust::...`.
    RustPath { path: String },
    /// A placeholder used when the manifest intentionally preserves unknown type information.
    Unknown,
}

/// Exported field metadata for models and classes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldExport {
    pub name: String,
    pub ty: TypeRef,
    /// Whether the field has a declared default value.
    pub has_default: bool,
    /// Optional field alias published by the library surface.
    pub alias: Option<String>,
    /// Optional human-readable field description.
    pub description: Option<String>,
}

/// Receiver mutability for an exported method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiverExport {
    /// Method takes an immutable receiver.
    Immutable,
    /// Method takes a mutable receiver.
    Mutable,
}

/// Exported method signature metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodExport {
    pub name: String,
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
    #[serde(default)]
    pub kind: ParamKindExport,
    #[serde(default)]
    pub has_default: bool,
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
        signature: Option<ParamDefaultCallSignatureExport>,
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
    pub name: String,
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
    /// Whether this newtype is a zero-cost Rust type alias (`type X = rusttype RustX`).
    #[serde(default)]
    pub is_rusttype: bool,
    /// Underlying wrapped type.
    pub underlying: TypeRef,
    pub methods: Vec<MethodExport>,
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

impl LibraryManifest {
    /// Create a new manifest seeded with the current compiler version and format version.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            incan_version: crate::version::INCAN_VERSION.to_string(),
            manifest_format: LIBRARY_MANIFEST_FORMAT,
            exports: LibraryExports::default(),
            vocab: None,
            soft_keywords: SoftKeywordExports::default(),
            contract_metadata: LibraryContractMetadata::default(),
            rust_abi: None,
        }
    }

    /// Build a semantic manifest directly from checked frontend exports.
    ///
    /// This is used by library packaging paths that already hold semantically checked declarations and want a
    /// deterministic manifest surface without going through a raw transport payload.
    pub fn from_checked_exports(
        name: impl Into<String>,
        version: impl Into<String>,
        checked_exports: &[CheckedNamedExport],
    ) -> Self {
        let name = name.into();
        let mut manifest = Self::new(name.clone(), version);
        manifest.exports = LibraryExports::from_checked_exports(checked_exports);
        for enum_export in &mut manifest.exports.enums {
            if enum_export.value_type.is_some() && enum_export.ordinal_type_identity.is_none() {
                enum_export.ordinal_type_identity = Some(format!("{name}.{}", enum_export.name));
            }
        }
        manifest
    }

    /// Serialize, validate, and write the manifest to disk.
    ///
    /// Validation happens before serialization so producer mistakes fail early instead of emitting an invalid
    /// `.incnlib` file.
    pub fn write_to_path(&self, path: &Path) -> Result<(), LibraryManifestError> {
        let raw = RawLibraryManifest::from_semantic(self);
        validate_raw_manifest(&raw)?;
        let content =
            serde_json::to_string_pretty(&raw).map_err(|err| LibraryManifestError::Serialize(err.to_string()))?;
        fs::write(path, format!("{content}\n")).map_err(|source| LibraryManifestError::Write {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }

    /// Read, decode, validate, and convert a manifest from disk.
    pub fn read_from_path(path: &Path) -> Result<Self, LibraryManifestError> {
        let content = fs::read_to_string(path).map_err(|source| LibraryManifestError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_json_str(&content)
    }

    /// Decode, validate, and convert a manifest from JSON text.
    pub fn from_json_str(content: &str) -> Result<Self, LibraryManifestError> {
        let raw: RawLibraryManifest =
            serde_json::from_str(content).map_err(|err| LibraryManifestError::Parse(err.to_string()))?;
        validate_raw_manifest(&raw)?;
        raw.into_semantic()
    }
}

impl LibraryExports {
    /// Build manifest exports from checked frontend exports.
    fn from_checked_exports(exports: &[CheckedNamedExport]) -> Self {
        let mut model = Self::default();

        for export in exports {
            match &export.kind {
                CheckedExportKind::Function(function_export) => {
                    model.functions.push(function_export_from_checked(function_export));
                }
                CheckedExportKind::Partial(partial_export) => {
                    model.partials.push(partial_export_from_checked(partial_export));
                }
                CheckedExportKind::Alias(alias_export) => {
                    model.aliases.push(alias_export_from_checked(alias_export));
                }
                CheckedExportKind::TypeAlias(type_alias_export) => {
                    model
                        .type_aliases
                        .push(type_alias_export_from_checked(type_alias_export));
                }
                CheckedExportKind::Model(model_export) => {
                    model.models.push(model_export_from_checked(model_export));
                }
                CheckedExportKind::Class(class_export) => {
                    model.classes.push(class_export_from_checked(class_export));
                }
                CheckedExportKind::Trait(trait_export) => {
                    model.traits.push(trait_export_from_checked(trait_export));
                }
                CheckedExportKind::Enum(enum_export) => {
                    model.enums.push(enum_export_from_checked(enum_export));
                }
                CheckedExportKind::Newtype(newtype_export) => {
                    model.newtypes.push(newtype_export_from_checked(newtype_export));
                }
                CheckedExportKind::Const(const_export) => {
                    model.consts.push(const_export_from_checked(const_export));
                }
                CheckedExportKind::Static(static_export) => {
                    model.statics.push(static_export_from_checked(static_export));
                }
            }
        }

        model.sort_deterministically();
        model
    }

    /// Sort every export group by stable public name.
    fn sort_deterministically(&mut self) {
        self.models.sort_by(|left, right| left.name.cmp(&right.name));
        self.aliases.sort_by(|left, right| left.name.cmp(&right.name));
        self.partials.sort_by(|left, right| left.name.cmp(&right.name));
        self.classes.sort_by(|left, right| left.name.cmp(&right.name));
        self.functions.sort_by(|left, right| left.name.cmp(&right.name));
        self.traits.sort_by(|left, right| left.name.cmp(&right.name));
        self.enums.sort_by(|left, right| left.name.cmp(&right.name));
        self.type_aliases.sort_by(|left, right| left.name.cmp(&right.name));
        self.newtypes.sort_by(|left, right| left.name.cmp(&right.name));
        self.consts.sort_by(|left, right| left.name.cmp(&right.name));
        self.statics.sort_by(|left, right| left.name.cmp(&right.name));
    }
}

/// Convert checked alias metadata into manifest alias metadata.
fn alias_export_from_checked(export: &CheckedAliasExport) -> AliasExport {
    AliasExport {
        name: export.name.clone(),
        target_path: export.target_path.clone(),
        projected_function: export.projected_function.as_ref().map(function_export_from_checked),
    }
}

/// Convert checked partial metadata into manifest partial export metadata.
fn partial_export_from_checked(export: &CheckedPartialExport) -> PartialExport {
    PartialExport {
        name: export.name.clone(),
        target_path: export.target_path.clone(),
        target_kind: partial_target_kind_from_checked(export.target_kind),
        presets: export
            .presets
            .iter()
            .map(|preset| PartialPresetExport {
                name: preset.name.clone(),
                ty: type_ref_from_resolved(&preset.ty),
                value: preset_value_from_checked(&preset.value),
            })
            .collect(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        params: params_from_checked(&export.params, &[]),
        return_type: type_ref_from_resolved(&export.return_type),
        is_async: export.is_async,
    }
}

/// Convert checked partial target kinds into the manifest vocabulary.
fn partial_target_kind_from_checked(kind: CheckedPartialTargetKind) -> PartialTargetKindExport {
    match kind {
        CheckedPartialTargetKind::Function => PartialTargetKindExport::Function,
        CheckedPartialTargetKind::ModelConstructor => PartialTargetKindExport::ModelConstructor,
        CheckedPartialTargetKind::ClassConstructor => PartialTargetKindExport::ClassConstructor,
        CheckedPartialTargetKind::NewtypeConstructor => PartialTargetKindExport::NewtypeConstructor,
        CheckedPartialTargetKind::Partial => PartialTargetKindExport::Partial,
        CheckedPartialTargetKind::Unknown => PartialTargetKindExport::Unknown,
    }
}

/// Convert checked preset values into the manifest value vocabulary.
fn preset_value_from_checked(value: &CheckedPresetValue) -> PresetValueExport {
    match value {
        CheckedPresetValue::Int(value) => PresetValueExport::Int(*value),
        CheckedPresetValue::Float(value) => PresetValueExport::Float(value.to_string()),
        CheckedPresetValue::Bool(value) => PresetValueExport::Bool(*value),
        CheckedPresetValue::String(value) => PresetValueExport::String(value.clone()),
        CheckedPresetValue::Bytes(value) => PresetValueExport::Bytes(value.clone()),
        CheckedPresetValue::None => PresetValueExport::None,
        CheckedPresetValue::List(values) => {
            PresetValueExport::List(values.iter().map(preset_value_from_checked).collect())
        }
        CheckedPresetValue::Dict(entries) => PresetValueExport::Dict(
            entries
                .iter()
                .map(|(key, value)| PresetDictEntryExport {
                    key: preset_value_from_checked(key),
                    value: preset_value_from_checked(value),
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
                    value: preset_value_from_checked(value),
                })
                .collect(),
        },
        CheckedPresetValue::Unsupported => PresetValueExport::Unsupported,
    }
}

/// Convert checked parameter defaults into the manifest default-expression vocabulary when consumers can materialize
/// them.
fn param_default_from_checked(value: &CheckedParamDefault) -> Option<ParamDefaultExport> {
    match value {
        CheckedParamDefault::Int(value) => Some(ParamDefaultExport::Int(*value)),
        CheckedParamDefault::Float(value) => Some(ParamDefaultExport::Float(value.to_string())),
        CheckedParamDefault::Bool(value) => Some(ParamDefaultExport::Bool(*value)),
        CheckedParamDefault::String(value) => Some(ParamDefaultExport::String(value.clone())),
        CheckedParamDefault::Bytes(value) => Some(ParamDefaultExport::Bytes(value.clone())),
        CheckedParamDefault::None => Some(ParamDefaultExport::None),
        CheckedParamDefault::List(values) => values
            .iter()
            .map(param_default_from_checked)
            .collect::<Option<Vec<_>>>()
            .map(ParamDefaultExport::List),
        CheckedParamDefault::Dict(entries) => entries
            .iter()
            .map(|(key, value)| {
                Some(ParamDefaultDictEntryExport {
                    key: param_default_from_checked(key)?,
                    value: param_default_from_checked(value)?,
                })
            })
            .collect::<Option<Vec<_>>>()
            .map(ParamDefaultExport::Dict),
        CheckedParamDefault::ConstRef(path) => Some(ParamDefaultExport::ConstRef(path.clone())),
        CheckedParamDefault::Call { path, args, signature } => args
            .iter()
            .map(|arg| {
                Some(ParamDefaultCallArgExport {
                    name: arg.name.clone(),
                    value: param_default_from_checked(&arg.value)?,
                })
            })
            .collect::<Option<Vec<_>>>()
            .map(|args| ParamDefaultExport::Call {
                path: path.clone(),
                args,
                signature: signature.as_ref().map(param_default_call_signature_from_checked),
            }),
        CheckedParamDefault::Unsupported => None,
    }
}

/// Convert a checked default-helper callable surface into manifest metadata.
fn param_default_call_signature_from_checked(
    signature: &CheckedParamDefaultCallSignature,
) -> ParamDefaultCallSignatureExport {
    ParamDefaultCallSignatureExport {
        params: params_from_checked(&signature.params, &[]),
        return_type: type_ref_from_resolved(&signature.return_type),
    }
}

fn type_param_from_checked(type_param: &CheckedTypeParam) -> TypeParamExport {
    TypeParamExport {
        name: type_param.name.clone(),
        bounds: type_param.bounds.iter().map(type_bound_from_checked).collect(),
    }
}

/// Convert checked trait-bound metadata into the serialized manifest shape.
fn type_bound_from_checked(bound: &CheckedTypeBound) -> TypeBoundExport {
    TypeBoundExport {
        name: bound.name.clone(),
        source_name: bound.source_name.clone(),
        module_path: bound.module_path.clone(),
        type_args: bound.type_args.iter().map(type_ref_from_resolved).collect(),
    }
}

/// Convert checked callable parameters into library-manifest parameter records.
fn params_from_checked(params: &[CallableParam], defaults: &[Option<CheckedParamDefault>]) -> Vec<ParamExport> {
    params
        .iter()
        .enumerate()
        .filter_map(|param| {
            let (idx, param) = param;
            let default = defaults
                .get(idx)
                .and_then(|default| default.as_ref())
                .and_then(param_default_from_checked);
            let has_default = if defaults.is_empty() {
                param.has_default
            } else {
                default.is_some()
            };
            Some(ParamExport {
                name: param.name.clone()?,
                ty: type_ref_from_resolved(&param.ty),
                kind: param_kind_from_ast(param.kind),
                has_default,
                default,
            })
        })
        .collect()
}

/// Convert an AST parameter kind into a library-manifest parameter kind.
fn param_kind_from_ast(kind: crate::frontend::ast::ParamKind) -> ParamKindExport {
    match kind {
        crate::frontend::ast::ParamKind::Normal => ParamKindExport::Normal,
        crate::frontend::ast::ParamKind::RestPositional => ParamKindExport::RestPositional,
        crate::frontend::ast::ParamKind::RestKeyword => ParamKindExport::RestKeyword,
    }
}

fn receiver_from_checked(receiver: Option<crate::frontend::ast::Receiver>) -> Option<ReceiverExport> {
    receiver.map(|value| match value {
        crate::frontend::ast::Receiver::Immutable => ReceiverExport::Immutable,
        crate::frontend::ast::Receiver::Mutable => ReceiverExport::Mutable,
    })
}

/// Convert checked method metadata into manifest method metadata.
fn method_from_checked(method: &crate::frontend::library_exports::CheckedMethod) -> MethodExport {
    MethodExport {
        name: method.name.clone(),
        alias_of: method.alias_of.clone(),
        type_params: method.type_params.iter().map(type_param_from_checked).collect(),
        receiver: receiver_from_checked(method.receiver),
        params: params_from_checked(&method.params, &[]),
        return_type: type_ref_from_resolved(&method.return_type),
        is_async: method.is_async,
        has_body: method.has_body,
    }
}

fn field_from_checked(field: &crate::frontend::library_exports::CheckedField) -> FieldExport {
    FieldExport {
        name: field.name.clone(),
        ty: type_ref_from_resolved(&field.ty),
        has_default: field.has_default,
        alias: field.alias.clone(),
        description: field.description.clone(),
    }
}

/// Convert a checked source function export into manifest metadata, including the materializable default subset.
pub(super) fn function_export_from_checked(export: &CheckedFunctionExport) -> FunctionExport {
    FunctionExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        params: params_from_checked(&export.params, &export.param_defaults),
        return_type: type_ref_from_resolved(&export.return_type),
        is_async: export.is_async,
    }
}

fn type_alias_export_from_checked(export: &CheckedTypeAliasExport) -> TypeAliasExport {
    TypeAliasExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        target: type_ref_from_resolved(&export.target),
    }
}

/// Convert a checked model export into the serialized manifest model shape.
fn model_export_from_checked(export: &CheckedModelExport) -> ModelExport {
    ModelExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        traits: export.traits.clone(),
        trait_adoptions: export.trait_adoptions.iter().map(type_bound_from_checked).collect(),
        derives: export.derives.clone(),
        fields: export.fields.iter().map(field_from_checked).collect(),
        methods: export.methods.iter().map(method_from_checked).collect(),
    }
}

/// Convert a checked class export into the serialized manifest class shape.
fn class_export_from_checked(export: &CheckedClassExport) -> ClassExport {
    ClassExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        extends: export.extends.clone(),
        traits: export.traits.clone(),
        trait_adoptions: export.trait_adoptions.iter().map(type_bound_from_checked).collect(),
        derives: export.derives.clone(),
        fields: export.fields.iter().map(field_from_checked).collect(),
        methods: export.methods.iter().map(method_from_checked).collect(),
    }
}

/// Convert a checked trait export into the serialized manifest trait shape.
fn trait_export_from_checked(export: &CheckedTraitExport) -> TraitExport {
    TraitExport {
        name: export.name.clone(),
        source_name: (export.source_name != export.name).then(|| export.source_name.clone()),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        supertraits: export.supertraits.iter().map(type_bound_from_checked).collect(),
        requires: export
            .requires
            .iter()
            .map(|(name, ty)| FieldRequirementExport {
                name: name.clone(),
                ty: type_ref_from_resolved(ty),
            })
            .collect(),
        methods: export.methods.iter().map(method_from_checked).collect(),
    }
}

/// Convert a checked enum export into the manifest enum contract.
fn enum_export_from_checked(export: &CheckedEnumExport) -> EnumExport {
    EnumExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        traits: export.traits.clone(),
        trait_adoptions: export.trait_adoptions.iter().map(type_bound_from_checked).collect(),
        value_type: export.value_type.map(value_enum_type_from_checked),
        ordinal_type_identity: None,
        variants: export
            .variants
            .iter()
            .map(|variant| EnumVariantExport {
                name: variant.name.clone(),
                fields: variant.fields.iter().map(type_ref_from_resolved).collect(),
                value: variant.value.as_ref().map(value_enum_value_from_checked),
            })
            .collect(),
        variant_aliases: export
            .variant_aliases
            .iter()
            .map(|alias| EnumVariantAliasExport {
                name: alias.name.clone(),
                target: alias.target.clone(),
            })
            .collect(),
        methods: export.methods.iter().map(method_from_checked).collect(),
        derives: export.derives.clone(),
    }
}

/// Convert checked value-enum backing metadata into the manifest representation.
fn value_enum_type_from_checked(value_type: ValueEnumBacking) -> EnumValueTypeExport {
    match value_type {
        ValueEnumBacking::Str => EnumValueTypeExport::Str,
        ValueEnumBacking::Int => EnumValueTypeExport::Int,
    }
}

/// Convert one checked value-enum raw value into the manifest representation.
fn value_enum_value_from_checked(value: &ValueEnumValue) -> EnumValueExport {
    match value {
        ValueEnumValue::Str(value) => EnumValueExport::Str(value.clone()),
        ValueEnumValue::Int(value) => EnumValueExport::Int(*value),
    }
}

/// Convert a checked newtype export into the serialized manifest shape.
fn newtype_export_from_checked(export: &CheckedNewtypeExport) -> NewtypeExport {
    NewtypeExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        traits: export.traits.clone(),
        trait_adoptions: export.trait_adoptions.iter().map(type_bound_from_checked).collect(),
        is_rusttype: export.is_rusttype,
        underlying: type_ref_from_resolved(&export.underlying),
        methods: export.methods.iter().map(method_from_checked).collect(),
    }
}

fn const_export_from_checked(export: &CheckedConstExport) -> ConstExport {
    ConstExport {
        name: export.name.clone(),
        ty: type_ref_from_resolved(&export.ty),
    }
}

fn static_export_from_checked(export: &CheckedStaticExport) -> StaticExport {
    StaticExport {
        name: export.name.clone(),
        ty: type_ref_from_resolved(&export.ty),
    }
}
