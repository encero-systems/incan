use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::type_refs::type_ref_from_resolved;
use super::validation::validate_raw_manifest;
use super::wire::RawLibraryManifest;
use super::{DslSurface, LIBRARY_MANIFEST_FORMAT, VocabKeywordRegistration, VocabProviderManifest};
use crate::frontend::library_exports::{
    CheckedClassExport, CheckedConstExport, CheckedEnumExport, CheckedExportKind, CheckedFunctionExport,
    CheckedModelExport, CheckedNamedExport, CheckedNewtypeExport, CheckedTraitExport, CheckedTypeAliasExport,
    CheckedTypeBound, CheckedTypeParam,
};
use crate::frontend::symbols::ResolvedType;

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
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

/// Public library exports grouped by declaration kind.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LibraryExports {
    pub models: Vec<ModelExport>,
    pub classes: Vec<ClassExport>,
    pub functions: Vec<FunctionExport>,
    pub traits: Vec<TraitExport>,
    pub enums: Vec<EnumExport>,
    pub type_aliases: Vec<TypeAliasExport>,
    pub newtypes: Vec<NewtypeExport>,
    pub consts: Vec<ConstExport>,
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
    /// Receiver requirement when the method is invoked on a type instance.
    pub receiver: Option<ReceiverExport>,
    pub params: Vec<ParamExport>,
    pub return_type: TypeRef,
    /// Whether the method is declared `async`.
    pub is_async: bool,
    /// Whether the originating declaration included a body.
    pub has_body: bool,
}

/// One exported positional parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamExport {
    pub name: String,
    pub ty: TypeRef,
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
    pub fields: Vec<FieldExport>,
    pub methods: Vec<MethodExport>,
}

/// Exported trait metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraitExport {
    pub name: String,
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
    pub variants: Vec<EnumVariantExport>,
    /// `@derive(...)` names (empty for manifests predating this field).
    #[serde(default)]
    pub derives: Vec<String>,
}

/// One exported enum variant and its positional payload fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumVariantExport {
    pub name: String,
    pub fields: Vec<TypeRef>,
}

/// Exported newtype metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewtypeExport {
    pub name: String,
    pub type_params: Vec<TypeParamExport>,
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
        let mut manifest = Self::new(name, version);
        manifest.exports = LibraryExports::from_checked_exports(checked_exports);
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
    fn from_checked_exports(exports: &[CheckedNamedExport]) -> Self {
        let mut model = Self::default();

        for export in exports {
            match &export.kind {
                CheckedExportKind::Function(function_export) => {
                    model.functions.push(function_export_from_checked(function_export));
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
            }
        }

        model.sort_deterministically();
        model
    }

    fn sort_deterministically(&mut self) {
        self.models.sort_by(|left, right| left.name.cmp(&right.name));
        self.classes.sort_by(|left, right| left.name.cmp(&right.name));
        self.functions.sort_by(|left, right| left.name.cmp(&right.name));
        self.traits.sort_by(|left, right| left.name.cmp(&right.name));
        self.enums.sort_by(|left, right| left.name.cmp(&right.name));
        self.type_aliases.sort_by(|left, right| left.name.cmp(&right.name));
        self.newtypes.sort_by(|left, right| left.name.cmp(&right.name));
        self.consts.sort_by(|left, right| left.name.cmp(&right.name));
    }
}

fn type_param_from_checked(type_param: &CheckedTypeParam) -> TypeParamExport {
    TypeParamExport {
        name: type_param.name.clone(),
        bounds: type_param.bounds.iter().map(type_bound_from_checked).collect(),
    }
}

fn type_bound_from_checked(bound: &CheckedTypeBound) -> TypeBoundExport {
    TypeBoundExport {
        name: bound.name.clone(),
        type_args: bound.type_args.iter().map(type_ref_from_resolved).collect(),
    }
}

fn params_from_checked(params: &[(String, ResolvedType)]) -> Vec<ParamExport> {
    params
        .iter()
        .map(|(name, ty)| ParamExport {
            name: name.clone(),
            ty: type_ref_from_resolved(ty),
        })
        .collect()
}

fn receiver_from_checked(receiver: Option<crate::frontend::ast::Receiver>) -> Option<ReceiverExport> {
    receiver.map(|value| match value {
        crate::frontend::ast::Receiver::Immutable => ReceiverExport::Immutable,
        crate::frontend::ast::Receiver::Mutable => ReceiverExport::Mutable,
    })
}

fn method_from_checked(method: &crate::frontend::library_exports::CheckedMethod) -> MethodExport {
    MethodExport {
        name: method.name.clone(),
        receiver: receiver_from_checked(method.receiver),
        params: params_from_checked(&method.params),
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

fn function_export_from_checked(export: &CheckedFunctionExport) -> FunctionExport {
    FunctionExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        params: params_from_checked(&export.params),
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

fn model_export_from_checked(export: &CheckedModelExport) -> ModelExport {
    ModelExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        traits: export.traits.clone(),
        fields: export.fields.iter().map(field_from_checked).collect(),
        methods: export.methods.iter().map(method_from_checked).collect(),
    }
}

fn class_export_from_checked(export: &CheckedClassExport) -> ClassExport {
    ClassExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        extends: export.extends.clone(),
        traits: export.traits.clone(),
        fields: export.fields.iter().map(field_from_checked).collect(),
        methods: export.methods.iter().map(method_from_checked).collect(),
    }
}

fn trait_export_from_checked(export: &CheckedTraitExport) -> TraitExport {
    TraitExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        supertraits: export
            .supertraits
            .iter()
            .map(|(trait_name, args)| TypeBoundExport {
                name: trait_name.clone(),
                type_args: args.iter().map(type_ref_from_resolved).collect(),
            })
            .collect(),
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

fn enum_export_from_checked(export: &CheckedEnumExport) -> EnumExport {
    EnumExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        variants: export
            .variants
            .iter()
            .map(|variant| EnumVariantExport {
                name: variant.name.clone(),
                fields: variant.fields.iter().map(type_ref_from_resolved).collect(),
            })
            .collect(),
        derives: export.derives.clone(),
    }
}

fn newtype_export_from_checked(export: &CheckedNewtypeExport) -> NewtypeExport {
    NewtypeExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
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
