//! Contract metadata, executable representation, and compiled provider metadata exports.

use super::*;

/// RFC 048 metadata persisted into `.incnlib`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LibraryContractMetadata {
    /// The producing manifest's declared persistent-record namespace, independent of consumer import aliases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_package: Option<incan_semantics_core::record_identity::RecordPackageIdentity>,
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
    #[serde(default = "legacy_library_identity_graph")]
    pub identity_graph: LibraryIdentityGraph,
    /// Optional executable publication selected by this exact manifest; linking-only packages may omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_representation: Option<ExecutableRepresentationExport>,
    /// Publicly referenced union definitions captured from this artifact's final native emission.
    ///
    /// Entries are owned by this containing artifact and retain the exact emitted name and payload order. Typed
    /// references forwarded from another artifact retain that selected owner and do not add a local definition.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_unions: Vec<NativeUnionExport>,
    /// Generic compiled-provider facts used by SDK and ordinary package consumers.
    #[serde(default, skip_serializing_if = "CompiledProviderMetadata::is_empty")]
    pub provider: CompiledProviderMetadata,
}

/// Immutable binary executable sidecar published before the manifest that selects it.
///
/// The manifest selects immutable semantic content from its checked build, so stale files cannot be selected by a
/// rebuilt manifest. The surrounding library publication restores ordinary failures; it does not promise atomic
/// availability to concurrent readers or across a process crash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutableRepresentationExport {
    /// Encoding version, separate from the manifest and package versions.
    pub representation_version: u32,
    /// SHA-256 content identity, encoded as lowercase hexadecimal and used as the semantic sidecar filename.
    pub content_digest: String,
}

/// Generic backend-neutral provider facts embedded in one checked library artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledProviderMetadata {
    /// Version of this provider metadata payload.
    #[serde(default = "default_compiled_provider_metadata_schema_version")]
    pub schema_version: u32,
    /// Stable digest of the provider's authored manifest and Incan source inputs.
    ///
    /// Physical generated Rust and extracted host ABI metadata remain protected by the artifact digest, while this
    /// source-owned identity lets canonical locks compare equivalent native provider builds across platforms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_source_digest: Option<String>,
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
    /// Checked provider-call declarations that a consumer may lower into provider-operation plans.
    ///
    /// Each record was resolved at the provider declaration site. This payload is metadata, not a source convention:
    /// consumers project it through their checked provider plan rather than matching a provider or operation spelling.
    #[serde(default)]
    pub operation_descriptors: Vec<ProviderOperationMetadata>,
}

impl Default for CompiledProviderMetadata {
    fn default() -> Self {
        Self {
            schema_version: COMPILED_PROVIDER_METADATA_SCHEMA_VERSION,
            semantic_source_digest: None,
            namespace_claims: Vec::new(),
            public_features: BTreeMap::new(),
            active_features: BTreeSet::new(),
            provider_dependencies: Vec::new(),
            fact_requirements: Vec::new(),
            required_sdk_components: BTreeSet::new(),
            operation_descriptors: Vec::new(),
        }
    }
}

impl CompiledProviderMetadata {
    /// Return whether the manifest has no provider facts beyond the default schema discriminator.
    pub fn is_empty(&self) -> bool {
        self.semantic_source_digest.is_none()
            && self.namespace_claims.is_empty()
            && self.public_features.is_empty()
            && self.active_features.is_empty()
            && self.provider_dependencies.is_empty()
            && self.fact_requirements.is_empty()
            && self.required_sdk_components.is_empty()
            && self.operation_descriptors.is_empty()
    }
}

/// One provider function that has a checked RFC 104 operation requirement.
///
/// `CanonicalSymbolId` deliberately preserves the declaration anchors produced by the provider's compilation. A
/// consumer obtains these facts only from the selected, integrity-checked provider manifest and uses them solely as
/// an input to its current compilation's provider plan; it does not reconstruct either identity from a source or
/// generated-name spelling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderOperationMetadata {
    /// Canonical provider-function declaration that identifies the operation.
    pub operation: CanonicalSymbolId,
    /// Canonical RFC 104 capability declaration required for an invocation.
    pub required_capability: CanonicalSymbolId,
    /// Backend-neutral requirements the checked operation declares.
    #[serde(default)]
    pub runtime_requirements: Vec<AbiV0RuntimeRequirement>,
}

impl ProviderOperationMetadata {
    /// Return whether this descriptor carries the two declaration kinds an authority consumer can interpret.
    pub fn has_expected_kinds(&self) -> bool {
        self.operation.kind == SemanticSourceTargetKind::Function
            && self.required_capability.kind == SemanticSourceTargetKind::Capability
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
pub(super) fn provider_dependency_default_features() -> bool {
    true
}

/// Return the current provider metadata schema for serde defaults.
pub(super) fn default_compiled_provider_metadata_schema_version() -> u32 {
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
