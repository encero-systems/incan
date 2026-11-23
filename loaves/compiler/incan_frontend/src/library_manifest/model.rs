use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::type_refs::type_ref_from_resolved;
use super::validation::validate_raw_manifest;
use super::wire::{RawLibraryManifest, RawVocabExports};
use super::{
    COMPILED_PROVIDER_METADATA_SCHEMA_VERSION, DslSurface, LIBRARY_MANIFEST_FORMAT, RUST_ABI_SCHEMA_VERSION,
    VocabKeywordRegistration, VocabProviderManifest,
};
use crate::api_metadata::CheckedApiMetadataPackage;
use crate::contract_metadata::ContractMetadataPackage as ModelContractMetadataPackage;
use crate::library_exports::{
    CheckedAliasExport, CheckedClassExport, CheckedConstExport, CheckedEnumExport, CheckedExportIdentity,
    CheckedExportKind, CheckedExportProjection, CheckedFunctionExport, CheckedModelExport, CheckedNamedExport,
    CheckedNewtypeExport, CheckedParamDefault, CheckedParamDefaultCallSignature, CheckedPartialExport,
    CheckedPartialTargetKind, CheckedPresetValue, CheckedProperty, CheckedStaticExport, CheckedTraitExport,
    CheckedTypeAliasExport, CheckedTypeBound, CheckedTypeParam,
};
use crate::registry_metadata::CheckedRegistryMetadataPackage;
use crate::symbols::{
    CallableParam, ImplementationTraitBoundInfo, ImplementationTraitBoundOriginInfo, ImplementationTypeParamInfo,
    NewtypePrimitiveConstraint, ValueEnumBacking, ValueEnumValue,
};
use incan_lang::interop::RustItemMetadata;
use incan_semantics_core::{
    AbiV0RuntimeRequirement, CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
};

use super::{OWNERSHIP_MANIFEST_FORMAT, ownership, retained, type_projection, with_checked_type_origins};

mod checked_conversion;
mod declaration_exports;
mod identity_graph;
mod manifest_impls;
mod provider_metadata;
mod type_exports;

pub use checked_conversion::*;
pub use declaration_exports::*;
pub use identity_graph::*;
pub use provider_metadata::*;
pub use type_exports::*;

/// Errors surfaced while reading, parsing, serializing, or validating `.incnlib` manifests.
#[derive(Debug, thiserror::Error)]
pub enum LibraryManifestError {
    /// Reading manifest contents from disk failed.
    #[error("failed to read {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
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
    /// Private nominal declarations that retain an RFC 150 migration refusal for compiled consumers.
    pub made_private_types: Vec<MadePrivateTypeExport>,
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

/// Published refusal metadata for a nominal type that used to be public.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MadePrivateTypeExport {
    pub module_path: Vec<String>,
    pub name: String,
    pub facts: MadePrivateExport,
}

/// Exported declaration-level alias metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AliasExport {
    pub name: String,
    pub target_path: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected_function: Option<FunctionExport>,
    /// Checked nominal target retained by a public type re-export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected_type: Option<TypeRef>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
    Set(Vec<PresetValueExport>),
    Tuple(Vec<PresetValueExport>),
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

/// Soft keywords that become active when the library is imported.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SoftKeywordExports {
    pub activations: Vec<SoftKeywordActivation>,
}

/// Published producer vocabulary descriptors packaged with the ordinary library manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VocabExports {
    /// The schema of the Incan registration that produced these descriptors.
    pub metadata_version: u32,
    /// Identity of the native producer registration unit, including its imports and compiled inputs.
    pub unit_identity: String,
    /// Project-relative Incan entry path, retained under the historical descriptor field spelling.
    pub crate_path: String,
    /// The Loaf that registered these descriptors.
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

impl VocabExports {
    /// Encode this vocabulary through the same wire conversion used by the complete library manifest.
    ///
    /// This encoding supports retained producer identities; it does not admit a library or validate helper bindings
    /// against exports. Complete publication and consumer loading still validate the enclosing library manifest.
    pub fn to_json_string(&self) -> Result<String, LibraryManifestError> {
        serde_json::to_string(&RawVocabExports::from_semantic(self))
            .map_err(|error| LibraryManifestError::Serialize(error.to_string()))
    }

    /// Encode one vocabulary record through the private wire shape shared with complete library manifests.
    pub fn encode_compiler_record_value(&self) -> Result<Vec<u8>, LibraryManifestError> {
        LibraryManifest::encode_compiler_record_value(&RawVocabExports::from_semantic(self))
    }

    /// Decode one authenticated vocabulary record without exposing the private manifest wire type.
    pub(crate) fn decode_compiler_record_value(content: &[u8]) -> Result<Self, LibraryManifestError> {
        LibraryManifest::decode_compiler_record_value::<RawVocabExports>(content).map(RawVocabExports::into_semantic)
    }
}

/// Packaged compile-time desugarer artifact metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabDesugarerArtifact {
    /// RFC 124 identity of the compiled native vocabulary unit.
    #[serde(default)]
    pub unit_identity: String,
    /// Artifact representation understood by the compiler.
    pub artifact_kind: incan_vocab::DesugarerArtifactKind,
    /// Public syntax version of the worker protocol the artifact speaks.
    pub abi_version: u32,
    /// Normalized relative path from the packaged Loaf root to the artifact file.
    pub relative_path: String,
    /// SHA-256 digest used to verify the packaged artifact on the consumer side.
    pub sha256: String,
}

/// One import-activated soft keyword exported by a library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoftKeywordActivation {
    /// Namespace whose import activates the keyword.
    pub namespace: String,
    /// Soft keyword lexeme activated by that namespace.
    pub keyword: String,
}
