use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::type_refs::type_ref_from_resolved;
use super::validation::validate_raw_manifest;
use super::wire::RawLibraryManifest;
use super::{
    COMPILED_PROVIDER_METADATA_SCHEMA_VERSION, DslSurface, LIBRARY_MANIFEST_FORMAT, RUST_ABI_SCHEMA_VERSION,
    VocabKeywordRegistration, VocabProviderManifest,
};
use crate::frontend::api_metadata::CheckedApiMetadataPackage;
use crate::frontend::contract_metadata::ContractMetadataPackage as ModelContractMetadataPackage;
use crate::frontend::library_exports::{
    CheckedAliasExport, CheckedClassExport, CheckedConstExport, CheckedEnumExport, CheckedExportIdentity,
    CheckedExportKind, CheckedExportProjection, CheckedFunctionExport, CheckedModelExport, CheckedNamedExport,
    CheckedNewtypeExport, CheckedParamDefault, CheckedParamDefaultCallSignature, CheckedPartialExport,
    CheckedPartialTargetKind, CheckedPresetValue, CheckedStaticExport, CheckedTraitExport, CheckedTypeAliasExport,
    CheckedTypeBound, CheckedTypeParam,
};
use crate::frontend::registry_metadata::CheckedRegistryMetadataPackage;
use crate::frontend::symbols::{CallableParam, NewtypePrimitiveConstraint, ValueEnumBacking, ValueEnumValue};
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
    /// Complete typed-registry facts checked from producer source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<CheckedRegistryMetadataPackage>,
    /// Stable semantic identities for public exports.
    #[serde(default, skip_serializing_if = "LibraryIdentityGraph::is_empty")]
    pub identity_graph: LibraryIdentityGraph,
    /// Generic compiled-provider facts used by SDK and ordinary package consumers.
    #[serde(default, skip_serializing_if = "CompiledProviderMetadata::is_empty")]
    pub provider: CompiledProviderMetadata,
}

/// Generic backend-neutral provider facts embedded in one checked library artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledProviderMetadata {
    /// Version of this provider metadata payload.
    #[serde(default = "default_compiled_provider_metadata_schema_version")]
    pub schema_version: u32,
    /// Provider-local module claims before the consumer's authorized namespace prefix is applied.
    #[serde(default)]
    pub namespace_claims: Vec<ProviderModuleClaim>,
    /// Public additive package-feature declarations owned by this provider.
    #[serde(default)]
    pub public_features: BTreeMap<String, ProviderFeatureMetadata>,
    /// Feature projection used to build this physical artifact.
    #[serde(default)]
    pub active_features: BTreeSet<String>,
    /// Active Incan provider dependencies captured for this physical feature projection.
    #[serde(default)]
    pub provider_dependencies: Vec<ProviderDependencyMetadata>,
    /// Positive feature requirements attached to checked semantic facts.
    #[serde(default)]
    pub fact_requirements: Vec<ProviderFactRequirement>,
    /// SDK components that must already be enabled while this artifact projection is active.
    #[serde(default)]
    pub required_sdk_components: BTreeSet<String>,
    /// Private backend mappings selected from semantic module and feature use.
    #[serde(default)]
    pub implementation_facets: Vec<ProviderImplementationFacet>,
}

impl Default for CompiledProviderMetadata {
    fn default() -> Self {
        Self {
            schema_version: COMPILED_PROVIDER_METADATA_SCHEMA_VERSION,
            namespace_claims: Vec::new(),
            public_features: BTreeMap::new(),
            active_features: BTreeSet::new(),
            provider_dependencies: Vec::new(),
            fact_requirements: Vec::new(),
            required_sdk_components: BTreeSet::new(),
            implementation_facets: Vec::new(),
        }
    }
}

impl CompiledProviderMetadata {
    /// Return whether the manifest has no provider facts beyond the default schema discriminator.
    pub fn is_empty(&self) -> bool {
        self.namespace_claims.is_empty()
            && self.public_features.is_empty()
            && self.active_features.is_empty()
            && self.provider_dependencies.is_empty()
            && self.fact_requirements.is_empty()
            && self.required_sdk_components.is_empty()
            && self.implementation_facets.is_empty()
    }
}

/// One active Incan provider dependency frozen into a compiled artifact projection.
///
/// The path is relative to the containing provider artifact root, so moving the complete declared artifact graph keeps
/// both semantic resolution and the generated Cargo dependency graph intact. The digest and identity prevent a
/// relocated path from silently resolving to a different provider.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProviderDependencyMetadata {
    /// Whether this dependency contributes a public package namespace or only implements the containing provider.
    #[serde(default)]
    pub kind: ProviderDependencyKind,
    /// Dependency key used by the provider's own `pub::<key>` imports.
    pub dependency_key: String,
    /// Published provider name from the dependency's checked `.incnlib` manifest.
    pub provider_name: String,
    /// Published provider version from the dependency's checked `.incnlib` manifest.
    pub provider_version: String,
    /// Integrity identity of the complete dependency artifact tree.
    pub artifact_digest: String,
    /// Dependency artifact root relative to the containing provider artifact root.
    pub relative_artifact_path: String,
    /// Public features requested on this active dependency edge.
    #[serde(default)]
    pub requested_features: BTreeSet<String>,
    /// Whether the dependency's conventional `default` feature participates.
    #[serde(default = "provider_dependency_default_features")]
    pub default_features: bool,
    /// Whether a feature-conditioned optional edge activated this dependency.
    #[serde(default)]
    pub optional: bool,
}

/// Semantic visibility of one compiled-provider dependency edge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderDependencyKind {
    /// An ordinary Incan package dependency that receives the consumer-granted `pub::<key>` namespace.
    #[default]
    PublicPackage,
    /// A relocatable checked provider artifact linked only to implement the containing provider.
    PrivateImplementation,
}

/// Preserve the ordinary dependency default-feature behavior when reading older provider metadata.
fn provider_dependency_default_features() -> bool {
    true
}

/// Return the current provider metadata schema for serde defaults.
fn default_compiled_provider_metadata_schema_version() -> u32 {
    COMPILED_PROVIDER_METADATA_SCHEMA_VERSION
}

/// One provider-local module claim and its positive feature requirements.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProviderModuleClaim {
    /// Module path relative to the namespace granted by the consumer or SDK inventory.
    pub module_path: Vec<String>,
    /// Features that must all be active before this module participates.
    #[serde(default)]
    pub required_features: BTreeSet<String>,
}

/// Normalized feature edges persisted independently of compact or expanded authoring syntax.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderFeatureMetadata {
    /// Other features in this provider that become active.
    #[serde(default)]
    pub includes: BTreeSet<String>,
    /// Optional Incan dependencies that become active.
    #[serde(default)]
    pub optional_dependencies: BTreeSet<String>,
    /// Public feature requests grouped by active Incan dependency key.
    #[serde(default)]
    pub dependency_features: BTreeMap<String, BTreeSet<String>>,
    /// SDK components required but never enabled by this feature.
    #[serde(default)]
    pub required_sdk_components: BTreeSet<String>,
}

/// Checked provider fact category used for feature projection and inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFactKind {
    /// A canonical importable module exists in this projection.
    Module,
    /// A checked public declaration or re-export exists in this projection.
    Export,
    /// A checked import edge participates in this projection.
    ProviderDependency,
    /// A semantic fact requires an SDK component selected by the consumer.
    ComponentRequirement,
    /// A checked vocabulary or other parser-owned syntax contribution participates.
    SoftSyntax,
    /// A checked `std.registry` entry participates in this projection.
    RegistryEntry,
    /// Checked documentation is attached to a declaration in this projection.
    Documentation,
    /// A private backend implementation mapping participates in this projection.
    ImplementationFacet,
}

/// Positive feature predicate attached to one checked provider fact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProviderFactRequirement {
    /// Kind of checked fact being conditioned.
    pub kind: ProviderFactKind,
    /// Stable provider-local semantic identity or canonical metadata key.
    pub identity: String,
    /// Features that must all be active.
    #[serde(default)]
    pub required_features: BTreeSet<String>,
}

/// Provider-owned private mapping from semantic use to the current Cargo backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderImplementationFacet {
    /// Stable provider-local facet id.
    pub id: String,
    /// Provider-local modules whose use selects this facet.
    #[serde(default)]
    pub required_modules: BTreeSet<Vec<String>>,
    /// Public features whose activation selects this facet.
    #[serde(default)]
    pub required_features: BTreeSet<String>,
    /// Private Cargo features grouped by generated dependency key.
    #[serde(default)]
    pub cargo_features: BTreeMap<String, BTreeSet<String>>,
    /// Current-backend Cargo dependencies selected by this facet.
    #[serde(default)]
    pub cargo_dependencies: Vec<ProviderCargoDependency>,
}

/// Relocatable Cargo dependency metadata owned privately by one provider implementation facet.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProviderCargoDependency {
    /// Rust dependency key used by generated code.
    pub crate_name: String,
    /// Published Cargo package name when it differs from the dependency key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// Registry version requirement for registry-backed dependencies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Private Cargo features required on this dependency.
    #[serde(default)]
    pub features: BTreeSet<String>,
    /// Whether the dependency's Cargo default features remain enabled.
    #[serde(default = "provider_cargo_default_features")]
    pub default_features: bool,
    /// Relocatable source class interpreted only by the active backend adapter.
    pub source: ProviderCargoDependencySource,
}

/// Current-backend dependency source without producer-specific absolute paths.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderCargoDependencySource {
    /// A dependency resolved from the Cargo registry using `version`.
    Registry,
    /// A crate shipped within the active Incan toolchain at this toolchain-root-relative path.
    Toolchain { relative_path: String },
}

/// Preserve Cargo's default-feature behavior when reading older private implementation metadata.
fn provider_cargo_default_features() -> bool {
    true
}

/// Serialized schema version for the public export identity graph.
pub const LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION: u32 = 1;

/// Serializable semantic identity graph for exported library declarations.
///
/// The graph separates a public export's spelling from the declaration identity and projection it represents. Consumers
/// should consult this graph before falling back to short-name manifest lookup whenever aliases, reexports, partial
/// presets, decorators, generated helpers, or public package boundaries can observe the symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryIdentityGraph {
    /// Version of the serialized identity graph payload.
    #[serde(default = "default_identity_graph_schema_version")]
    pub schema_version: u32,
    /// Public export entries keyed by public spelling, semantic source path, and projection metadata.
    #[serde(default)]
    pub exports: Vec<ExportIdentity>,
}

impl Default for LibraryIdentityGraph {
    fn default() -> Self {
        Self {
            schema_version: LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION,
            exports: Vec::new(),
        }
    }
}

impl LibraryIdentityGraph {
    /// Build the serialized identity graph from checked public exports while deduplicating overload-set projections.
    pub fn from_checked_exports(package_name: &str, exports: &[CheckedNamedExport]) -> Self {
        let mut graph = Self {
            schema_version: LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION,
            exports: exports
                .iter()
                .map(|export| export_identity_from_checked(package_name, export))
                .collect(),
        };
        graph.exports.sort_by(|left, right| {
            left.public_name
                .cmp(&right.public_name)
                .then(left.source_path.cmp(&right.source_path))
        });
        graph.exports.dedup_by(|left, right| {
            left.public_name == right.public_name
                && left.source_path == right.source_path
                && left.projection == right.projection
        });
        graph
    }

    /// Return whether the graph has no public export identities to serialize.
    pub fn is_empty(&self) -> bool {
        self.exports.is_empty()
    }

    /// Return the first identity entry for a public export name, which represents the shared projection for overload
    /// sets.
    pub fn entry_for_public_name(&self, name: &str) -> Option<&ExportIdentity> {
        self.exports.iter().find(|entry| entry.public_name == name)
    }
}

/// Return the current identity graph schema version when deserializing manifests that predate the field.
fn default_identity_graph_schema_version() -> u32 {
    LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportIdentity {
    /// Name that consumers import from the public package surface.
    pub public_name: String,
    /// Package-qualified public path exposed by the `.incnlib` manifest.
    pub public_path: Vec<String>,
    /// Provider-local declaration path that owns the semantic identity.
    pub source_path: Vec<String>,
    /// Coarse export category used by consumers before reconstructing a full checked export surface.
    pub kind: ExportIdentityKind,
    /// Projection layered on top of `source_path`, such as a direct export, alias, reexport, or partial preset.
    pub projection: ExportIdentityProjection,
}

impl ExportIdentity {
    /// Return the projected target path when this public export is an alias, reexport, or partial preset.
    pub fn target_path(&self) -> Option<&[String]> {
        match &self.projection {
            ExportIdentityProjection::Direct => None,
            ExportIdentityProjection::Alias { target_path }
            | ExportIdentityProjection::Reexport { target_path }
            | ExportIdentityProjection::Partial { target_path, .. } => Some(target_path.as_slice()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportIdentityKind {
    /// Public function or overload set.
    Function,
    /// Public partial preset.
    Partial,
    /// Public alias declaration.
    Alias,
    /// Public type alias declaration.
    TypeAlias,
    /// Public model declaration.
    Model,
    /// Public class declaration.
    Class,
    /// Public trait declaration.
    Trait,
    /// Public enum declaration.
    Enum,
    /// Public newtype declaration.
    Newtype,
    /// Public const declaration.
    Const,
    /// Public static declaration.
    Static,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExportIdentityProjection {
    /// The public export exposes its own declaration directly.
    Direct,
    /// The public export is a source-level alias over another declaration or overload set.
    Alias {
        /// Provider-local declaration path that the alias projects.
        target_path: Vec<String>,
    },
    /// The public export forwards another declaration through a facade.
    Reexport {
        /// Provider-local declaration path that the reexport projects.
        target_path: Vec<String>,
    },
    /// The public export is a partial preset over a callable or constructor target.
    Partial {
        /// Provider-local declaration path that the partial preset projects.
        target_path: Vec<String>,
        /// Kind of target being projected, used to rebuild the consumer callable surface.
        target_kind: PartialTargetKindExport,
    },
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
    /// A value-level type token such as `Type[int]`.
    TypeToken { inner: Box<TypeRef> },
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
    /// Source-level field visibility enforced for compiled-library consumers.
    ///
    /// Older manifests omitted this field and exposed every recorded field to consumers, so absence remains public for
    /// backward compatibility. New manifests only need to serialize the private case.
    #[serde(default, skip_serializing_if = "FieldVisibilityExport::is_public")]
    pub visibility: FieldVisibilityExport,
    /// Whether the field has a declared default value.
    pub has_default: bool,
    /// Materializable source default used when a consumer constructs this type through the artifact boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<ParamDefaultExport>,
    /// Optional field alias published by the library surface.
    pub alias: Option<String>,
    /// Optional human-readable field description.
    pub description: Option<String>,
}

/// Source-level visibility of a model or class field published through a library manifest.
///
/// Public models currently emit public fields only. Private visibility is valid for class fields; manifest validation
/// rejects private model fields until the language defines that source state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldVisibilityExport {
    /// Access is limited to the declaring class's own methods.
    Private,
    /// Access is available to source and compiled-library consumers.
    #[default]
    Public,
}

impl FieldVisibilityExport {
    /// Return whether this field retains the legacy public consumer behavior.
    fn is_public(&self) -> bool {
        matches!(self, Self::Public)
    }
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
    pub(crate) fn from_checked(constraint: &NewtypePrimitiveConstraint) -> Self {
        let key = match constraint.key {
            crate::frontend::ast::TypeConstraintKey::Ge => NewtypeConstraintKeyExport::Ge,
            crate::frontend::ast::TypeConstraintKey::Gt => NewtypeConstraintKeyExport::Gt,
            crate::frontend::ast::TypeConstraintKey::Le => NewtypeConstraintKeyExport::Le,
            crate::frontend::ast::TypeConstraintKey::Lt => NewtypeConstraintKeyExport::Lt,
        };
        Self {
            key,
            value: constraint.value,
            repr: constraint.repr.clone(),
        }
    }

    /// Reconstruct the checked frontend predicate represented by this manifest entry.
    pub(crate) fn to_checked(&self) -> NewtypePrimitiveConstraint {
        let key = match self.key {
            NewtypeConstraintKeyExport::Ge => crate::frontend::ast::TypeConstraintKey::Ge,
            NewtypeConstraintKeyExport::Gt => crate::frontend::ast::TypeConstraintKey::Gt,
            NewtypeConstraintKeyExport::Le => crate::frontend::ast::TypeConstraintKey::Le,
            NewtypeConstraintKeyExport::Lt => crate::frontend::ast::TypeConstraintKey::Lt,
        };
        NewtypePrimitiveConstraint {
            key,
            value: self.value,
            repr: self.repr.clone(),
        }
    }
}

/// Preserve the pre-field manifest behavior when older manifests omit the coercion flag.
fn default_newtype_implicit_coercion() -> bool {
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
        manifest.contract_metadata.identity_graph = LibraryIdentityGraph::from_checked_exports(&name, checked_exports);
        manifest
            .exports
            .rewrite_newtype_underlying_names_to_public_exports(checked_exports);
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

    /// Rewrite newtype composition references through the public identity graph selected for this package.
    fn rewrite_newtype_underlying_names_to_public_exports(&mut self, exports: &[CheckedNamedExport]) {
        let mut public_names_by_source: std::collections::HashMap<Vec<String>, Vec<String>> =
            std::collections::HashMap::new();
        for export in exports {
            if !matches!(
                &export.kind,
                CheckedExportKind::Model(_)
                    | CheckedExportKind::Class(_)
                    | CheckedExportKind::Enum(_)
                    | CheckedExportKind::Newtype(_)
            ) {
                continue;
            }
            let source_path = match &export.identity.projection {
                CheckedExportProjection::Direct => &export.identity.source_path,
                CheckedExportProjection::Alias { target_path } | CheckedExportProjection::Reexport { target_path } => {
                    target_path
                }
                CheckedExportProjection::Partial { .. } => continue,
            };
            public_names_by_source
                .entry(source_path.clone())
                .or_default()
                .push(export.name.clone());
        }
        let public_names = public_names_by_source
            .into_iter()
            .filter_map(|(source_path, mut names)| {
                let source_name = source_path.last()?;
                names.sort();
                names.dedup();
                let preferred = names
                    .iter()
                    .find(|name| *name == source_name)
                    .cloned()
                    .unwrap_or_else(|| names[0].clone());
                Some((source_path, preferred))
            })
            .collect::<std::collections::HashMap<_, _>>();
        let mut source_paths_by_leaf: std::collections::HashMap<String, Vec<Vec<String>>> =
            std::collections::HashMap::new();
        for source_path in public_names.keys() {
            if let Some(source_name) = source_path.last() {
                source_paths_by_leaf
                    .entry(source_name.clone())
                    .or_default()
                    .push(source_path.clone());
            }
        }
        for newtype in &mut self.newtypes {
            let owner_module_path = exports
                .iter()
                .find(|export| export.name == newtype.name && matches!(&export.kind, CheckedExportKind::Newtype(_)))
                .map(|export| match &export.identity.projection {
                    CheckedExportProjection::Direct => export.identity.source_path.as_slice(),
                    CheckedExportProjection::Alias { target_path }
                    | CheckedExportProjection::Reexport { target_path } => target_path.as_slice(),
                    CheckedExportProjection::Partial { .. } => export.identity.source_path.as_slice(),
                })
                .and_then(|path| path.split_last().map(|(_, module_path)| module_path.to_vec()))
                .unwrap_or_default();
            rewrite_type_ref_names(
                &mut newtype.underlying,
                &owner_module_path,
                &public_names,
                &source_paths_by_leaf,
            );
        }
    }
}

/// Rewrite source-owned type references to the selected public export names.
fn rewrite_type_ref_names(
    ty: &mut TypeRef,
    owner_module_path: &[String],
    public_names: &std::collections::HashMap<Vec<String>, String>,
    source_paths_by_leaf: &std::collections::HashMap<String, Vec<Vec<String>>>,
) {
    let public_name_for = |name: &str| {
        let mut local_source_path = owner_module_path.to_vec();
        local_source_path.push(name.to_string());
        public_names.get(&local_source_path).or_else(|| {
            let candidates = source_paths_by_leaf.get(name)?;
            (candidates.len() == 1)
                .then(|| public_names.get(&candidates[0]))
                .flatten()
        })
    };
    match ty {
        TypeRef::Named { name } => {
            if let Some(public_name) = public_name_for(name) {
                *name = public_name.clone();
            }
        }
        TypeRef::Applied { name, args } => {
            if let Some(public_name) = public_name_for(name) {
                *name = public_name.clone();
            }
            for arg in args {
                rewrite_type_ref_names(arg, owner_module_path, public_names, source_paths_by_leaf);
            }
        }
        TypeRef::Function { params, return_type } => {
            for param in params {
                rewrite_type_ref_names(param, owner_module_path, public_names, source_paths_by_leaf);
            }
            rewrite_type_ref_names(return_type, owner_module_path, public_names, source_paths_by_leaf);
        }
        TypeRef::TypeToken { inner } | TypeRef::Ref { inner } => {
            rewrite_type_ref_names(inner, owner_module_path, public_names, source_paths_by_leaf);
        }
        TypeRef::Tuple { elements } => {
            for element in elements {
                rewrite_type_ref_names(element, owner_module_path, public_names, source_paths_by_leaf);
            }
        }
        TypeRef::TypeParam { .. } | TypeRef::SelfType | TypeRef::RustPath { .. } | TypeRef::Unknown => {}
    }
}

/// Build one manifest identity-graph entry from checked export metadata.
fn export_identity_from_checked(package_name: &str, export: &CheckedNamedExport) -> ExportIdentity {
    ExportIdentity {
        public_name: export.name.clone(),
        public_path: vec![package_name.to_string(), export.name.clone()],
        source_path: export.identity.source_path.clone(),
        kind: export_identity_kind_from_checked(&export.kind),
        projection: export_identity_projection_from_checked(&export.identity),
    }
}

/// Classify a checked public export for the stable manifest identity graph.
fn export_identity_kind_from_checked(kind: &CheckedExportKind) -> ExportIdentityKind {
    match kind {
        CheckedExportKind::Function(_) => ExportIdentityKind::Function,
        CheckedExportKind::Partial(_) => ExportIdentityKind::Partial,
        CheckedExportKind::Alias(_) => ExportIdentityKind::Alias,
        CheckedExportKind::TypeAlias(_) => ExportIdentityKind::TypeAlias,
        CheckedExportKind::Model(_) => ExportIdentityKind::Model,
        CheckedExportKind::Class(_) => ExportIdentityKind::Class,
        CheckedExportKind::Trait(_) => ExportIdentityKind::Trait,
        CheckedExportKind::Enum(_) => ExportIdentityKind::Enum,
        CheckedExportKind::Newtype(_) => ExportIdentityKind::Newtype,
        CheckedExportKind::Const(_) => ExportIdentityKind::Const,
        CheckedExportKind::Static(_) => ExportIdentityKind::Static,
    }
}

/// Convert checked identity projection metadata into the serializable manifest representation.
fn export_identity_projection_from_checked(identity: &CheckedExportIdentity) -> ExportIdentityProjection {
    match &identity.projection {
        CheckedExportProjection::Direct => ExportIdentityProjection::Direct,
        CheckedExportProjection::Alias { target_path } => ExportIdentityProjection::Alias {
            target_path: target_path.clone(),
        },
        CheckedExportProjection::Reexport { target_path } => ExportIdentityProjection::Reexport {
            target_path: target_path.clone(),
        },
        CheckedExportProjection::Partial {
            target_path,
            target_kind,
        } => ExportIdentityProjection::Partial {
            target_path: target_path.clone(),
            target_kind: partial_target_kind_from_checked(*target_kind),
        },
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
pub(crate) fn param_default_from_checked(value: &CheckedParamDefault) -> Option<ParamDefaultExport> {
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
pub(crate) fn params_from_checked(
    params: &[CallableParam],
    defaults: &[Option<CheckedParamDefault>],
) -> Vec<ParamExport> {
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
        params: params_from_checked(&method.params, &method.param_defaults),
        return_type: type_ref_from_resolved(&method.return_type),
        is_async: method.is_async,
        has_body: method.has_body,
    }
}

/// Convert checked field metadata into artifact metadata, including a materializable default when available.
fn field_from_checked(field: &crate::frontend::library_exports::CheckedField) -> FieldExport {
    let default = field.default.as_ref().and_then(param_default_from_checked);
    FieldExport {
        name: field.name.clone(),
        ty: type_ref_from_resolved(&field.ty),
        visibility: match field.visibility {
            crate::frontend::ast::Visibility::Private => FieldVisibilityExport::Private,
            crate::frontend::ast::Visibility::Public => FieldVisibilityExport::Public,
        },
        has_default: field.has_default,
        default,
        alias: field.alias.clone(),
        description: field.description.clone(),
    }
}

/// Convert a checked source function export into manifest metadata, including the materializable default subset.
pub(super) fn function_export_from_checked(export: &CheckedFunctionExport) -> FunctionExport {
    FunctionExport {
        name: export.name.clone(),
        emitted_name: export.emitted_name.clone(),
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
        derives: export.derives.clone(),
        is_rusttype: export.is_rusttype,
        underlying: type_ref_from_resolved(&export.underlying),
        checked_constructor: export.checked_constructor.clone(),
        constraints: export
            .constraints
            .iter()
            .map(NewtypeConstraintExport::from_checked)
            .collect(),
        implicit_coercion_enabled: export.implicit_coercion_enabled,
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
