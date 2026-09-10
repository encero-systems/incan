//! Pure semantic projections of original, producer-checked inputs.
//!
//! Admission, physical validation, source completeness and leases belong to the caller. These APIs verify the
//! associations within supplied records; they neither establish trust nor discover omitted source files. No native
//! candidate or filesystem path is consulted. The Rust input contract below requires a new producer; existing SDK
//! artifacts and native receipt strings do not implement it merely because they contain a digest.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::wire::RawLibraryManifest;
use super::{
    LibraryManifest, NativeCompilerSupport, NativeGitReference, NativeRequirementRole, NativeRequirementSource,
    NativeSourceCrateKind, NativeSourceRequirement, NativeSourceUnitDefinition, NativeUnionOwnerExport,
    ProviderDependencyKind, TypeRef, VisitTypeRefs,
};
use crate::provider::ProviderIdentity;

/// Independent version; this projection must not be retagged as the previous Cargo-derived v2 identity.
pub(crate) const PROVIDER_SEMANTIC_PROJECTION_VERSION: u32 = 3;
/// New original-producer contract required by the Rust source projection, not an existing native receipt format.
pub(crate) const RUST_SEMANTIC_INPUT_CONTRACT: &str = "incan.rust-semantic-inputs";

/// Missing evidence and unsupported contracts remain distinct from inconsistent supplied records.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SemanticProjectionError {
    #[error("missing semantic evidence: {field}")]
    Missing { field: String },
    #[error("unsupported semantic evidence: {field}")]
    Unsupported { field: String },
    #[error("invalid semantic evidence: {field}")]
    Invalid { field: String },
    #[error("semantic inputs changed inside the fixed context for {identity}")]
    Conflict { identity: String },
    #[error("cannot encode semantic inputs: {0}")]
    Encoding(#[from] serde_json::Error),
}

/// Original selected source provenance. File identities and checked configuration are supplied separately.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum RustSemanticSource<'a> {
    Registry {
        registry: &'a str,
        checksum: &'a str,
    },
    Git {
        url: &'a str,
        reference: &'a NativeGitReference,
        revision: &'a str,
    },
    Path,
    CompilerSupport {
        support: NativeCompilerSupport,
    },
}

/// Original Rust dependency declaration, including build-domain and target-condition distinctions.
///
/// `request` is the producer's portable requirement, not one reconstructed from Cargo by this consumer. A build
/// dependency uses the normal request role and `build = true`; dev-build combinations are unsupported.
#[derive(Debug, Serialize)]
pub(crate) struct RustSemanticDependency<'a> {
    pub request: &'a NativeSourceRequirement,
    pub build: bool,
    pub target_condition: Option<&'a str>,
}

/// Original producer purpose used when deciding whether development dependencies participate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SemanticActivationPurpose {
    Normal,
    Test,
}

/// Effective producer domain bound to every dependency activation outcome.
///
/// This is a new source-input contract, not a recipe inferred from a native candidate. Host/target and feature
/// facts originate in the checked producer; this kernel neither evaluates cfg expressions nor enables features.
#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct SemanticActivationContext<'a> {
    pub schema_version: u32,
    pub host: &'a str,
    pub target: &'a str,
    pub purpose: SemanticActivationPurpose,
    /// Whether the producer selected a build-script unit from the original package inputs.
    pub build_unit_present: bool,
    pub features: &'a BTreeSet<String>,
    pub default_features: bool,
}

/// A checked reason why an original declaration has no selected child in this effective domain.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SemanticInactiveReason {
    OptionalNotEnabled,
    TargetConditionFalse,
    DevelopmentExcluded,
    BuildUnitAbsent,
}

/// One original slot's checked outcome; inactive declarations remain present without invented selected sources.
pub(crate) enum RustSemanticOutcome<'a> {
    Selected(&'a RustSemanticDigest),
    Inactive(SemanticInactiveReason),
}

/// Exact checked activation outcome for one slot in an original Rust definition.
pub(crate) struct RustSemanticSelection<'a> {
    pub definition_digest: &'a str,
    pub activation_digest: &'a str,
    pub requirement_index: usize,
    pub outcome: RustSemanticOutcome<'a>,
}

/// Borrowed source-facet evidence supplied by the original producer under contract version1.
///
/// The producer must supply the complete permitted source-file catalog, checked configuration (including inherited
/// metadata, build-script declaration and selected feature/target conditions), and all declared dependency slots.
/// `files` uses portable logical names and content digests; physical owner locations stay outside this API. The
/// caller visits its existing admitted graph and supplies child results; this module creates no second graph.
pub(crate) struct RustSemanticInputs<'a> {
    pub activation: SemanticActivationContext<'a>,
    pub contract: &'a str,
    pub contract_version: u32,
    pub package: &'a str,
    pub version: &'a str,
    pub crate_name: &'a str,
    pub crate_kind: NativeSourceCrateKind,
    pub edition: &'a str,
    pub entrypoint: &'a str,
    pub source: RustSemanticSource<'a>,
    pub files: &'a BTreeMap<String, String>,
    pub configuration: &'a BTreeMap<String, String>,
    pub features: &'a BTreeSet<String>,
    pub default_features: bool,
    pub dependencies: &'a [RustSemanticDependency<'a>],
    pub selections: &'a [RustSemanticSelection<'a>],
}

/// Retained selected provenance for matching subsequent portable requirements.
#[derive(Debug, Clone)]
enum SelectedRustSource {
    Registry,
    Git { url: String, reference: NativeGitReference },
    Path,
    CompilerSupport(NativeCompilerSupport),
}

/// A result created only by this module from the complete supplied versioned Rust input record.
///
/// It is not proof of external producer admission. Private fields prevent a caller from wrapping an arbitrary
/// native/tree digest as this projection or substituting an output computed under another projection version.
#[derive(Debug, Clone)]
pub(crate) struct RustSemanticDigest {
    projection_version: u32,
    package: String,
    version: String,
    features: BTreeSet<String>,
    default_features: bool,
    source: SelectedRustSource,
    value: String,
}

/// Exact original descriptor index paired with the selected target's result.
pub(crate) struct ProviderSemanticEdge<'a> {
    pub descriptor_index: usize,
    pub selected_identity: &'a ProviderIdentity,
    pub target: &'a ProviderSemanticDigest,
}

/// One selected input associated with the original definition's dependency slot.
pub(crate) enum ProviderRequirementSelection<'a> {
    ProviderEdge { descriptor_index: usize },
    Rust(&'a RustSemanticDigest),
    Inactive(SemanticInactiveReason),
}

/// Binding includes the whole original definition identity so an index cannot silently change meaning.
pub(crate) struct ProviderRequirementBinding<'a> {
    pub definition_digest: &'a str,
    pub activation_digest: &'a str,
    pub requirement_index: usize,
    pub selected: ProviderRequirementSelection<'a>,
}

/// Compiler emission support bound to its original definition position, never inferred from native filenames.
pub(crate) struct ProviderSupportBinding<'a> {
    pub definition_digest: &'a str,
    pub support_index: usize,
    pub selected: &'a RustSemanticDigest,
}

/// Original checked provider records and their complete producer-supplied associations.
///
/// `edges` must cover every manifest descriptor, including SDK-private descriptors. `origins` supplies exact
/// transitive targets already admitted by ProviderPlan when typed public metadata references them. It is only a
/// borrowed lookup projection, not a new provider graph or permission to select additional providers.
pub(crate) struct ProviderSemanticInputs<'a> {
    pub activation: SemanticActivationContext<'a>,
    pub identity: &'a ProviderIdentity,
    pub manifest: &'a LibraryManifest,
    pub definition: Option<&'a NativeSourceUnitDefinition>,
    pub edges: &'a [ProviderSemanticEdge<'a>],
    pub requirements: &'a [ProviderRequirementBinding<'a>],
    pub compiler_support: &'a [ProviderSupportBinding<'a>],
    pub origins: &'a [&'a ProviderSemanticDigest],
}

/// A version-bound provider result paired with its original full physical selection identity.
#[derive(Debug, Clone)]
pub(crate) struct ProviderSemanticDigest {
    projection_version: u32,
    identity: ProviderIdentity,
    value: String,
}

impl ProviderSemanticDigest {
    /// Return the new version's portable digest, without changing any physical provider identity.
    pub(crate) fn value(&self) -> &str {
        &self.value
    }

    /// Return the exact original selection, including all active feature dimensions.
    pub(crate) fn identity(&self) -> &ProviderIdentity {
        &self.identity
    }
}

/// Registration phase for one original producer/owner context.
///
/// The caller visits each node of its existing admitted graph once, supplying previously registered child results.
/// Registering a record validates and projects it in full. Re-registering the same full identity also validates and
/// hashes the supplied facts, rejecting a semantic conflict; this is an explicit update check, not the reuse path.
/// Consume the builder with `finish` before repeated consumers query the immutable result context.
#[derive(Default)]
pub(crate) struct SemanticProjectionBuilder {
    results: BTreeMap<ProviderIdentity, ProviderSemanticDigest>,
}

impl SemanticProjectionBuilder {
    /// Begin registration; original admitted owners and physical evidence remain the caller's responsibility.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Validate and register one provider after its selected child results have been computed.
    ///
    /// This returns a typed value for subsequent parent bindings. Repeated references after registration should
    /// use the finalized context's `digest` lookup, rather than submit the same body to this method again.
    pub(crate) fn register(
        &mut self,
        inputs: &ProviderSemanticInputs<'_>,
    ) -> Result<ProviderSemanticDigest, SemanticProjectionError> {
        let value = project_provider(inputs)?;
        if let Some(previous) = self.results.get(inputs.identity) {
            if previous.value != value {
                return Err(SemanticProjectionError::Conflict {
                    identity: inputs.identity.stable_key(),
                });
            }
            return Ok(previous.clone());
        }
        let result = ProviderSemanticDigest {
            projection_version: PROVIDER_SEMANTIC_PROJECTION_VERSION,
            identity: inputs.identity.clone(),
            value,
        };
        self.results.insert(inputs.identity.clone(), result.clone());
        Ok(result)
    }

    /// Freeze the registered semantic snapshot, consuming every ability to register further inputs in this context.
    pub(crate) fn finish(self) -> SemanticProjection {
        SemanticProjection { results: self.results }
    }
}

/// Immutable semantic results for one completed producer input context.
///
/// Each full provider identity names the validated result registered before this context was finalized. Lookup
/// takes no mutable source/body/binding input and performs no projection, serialization or hashing. A caller with
/// changed source or compiler-support facts must create another builder, preserving original admission separately.
/// This table is a result index over the existing graph, not a source graph or a physical-validation cache.
pub(crate) struct SemanticProjection {
    results: BTreeMap<ProviderIdentity, ProviderSemanticDigest>,
}

impl SemanticProjection {
    /// Borrow one previously registered result by its complete selected identity, including feature projection.
    pub(crate) fn digest(
        &self,
        identity: &ProviderIdentity,
    ) -> Result<&ProviderSemanticDigest, SemanticProjectionError> {
        self.results
            .get(identity)
            .ok_or_else(|| missing(format!("registered provider {}", identity.stable_key())))
    }
}

/// Diagnose an inconsistent supplied field without inventing fallback evidence.
fn invalid(field: impl Into<String>) -> SemanticProjectionError {
    SemanticProjectionError::Invalid { field: field.into() }
}

/// Require one named evidence item.
fn missing(field: impl Into<String>) -> SemanticProjectionError {
    SemanticProjectionError::Missing { field: field.into() }
}

/// Require a nonempty producer field; whitespace-only strings are not facts.
fn text(value: &str, field: &str) -> Result<(), SemanticProjectionError> {
    if value.trim().is_empty() {
        return Err(invalid(field));
    }
    Ok(())
}

/// Validate exact digest syntax; validation here does not authenticate its original producer.
fn sha256(value: &str, field: &str) -> Result<(), SemanticProjectionError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(invalid(field));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(field));
    }
    Ok(())
}

/// Hash deterministic structured input with an explicit domain and format version.
fn hash(domain: &str, value: &impl Serialize) -> Result<String, SemanticProjectionError> {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    hasher.update(serde_json::to_vec(value)?);
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// Validate and bind a checked activation domain without interpreting dependency policy.
pub(crate) fn activation_context_digest(
    context: &SemanticActivationContext<'_>,
) -> Result<String, SemanticProjectionError> {
    if context.schema_version != 1 {
        return Err(SemanticProjectionError::Unsupported {
            field: format!("activation context version {}", context.schema_version),
        });
    }
    text(context.host, "activation host")?;
    text(context.target, "activation target")?;
    for feature in context.features {
        text(feature, "activation feature")?;
    }
    hash("incan-semantic-activation-v1", context)
}

/// Check that a supplied inactive reason applies to its declaration; the producer owns the actual decision.
fn validate_inactive_reason(
    reason: SemanticInactiveReason,
    request: &NativeSourceRequirement,
    target_condition: Option<&str>,
    build_dependency: bool,
    context: &SemanticActivationContext<'_>,
) -> Result<(), SemanticProjectionError> {
    let applicable = match reason {
        SemanticInactiveReason::BuildUnitAbsent => build_dependency && !context.build_unit_present,
        SemanticInactiveReason::OptionalNotEnabled => request.optional,
        SemanticInactiveReason::TargetConditionFalse => target_condition.is_some(),
        SemanticInactiveReason::DevelopmentExcluded => {
            request.role == NativeRequirementRole::Dev && context.purpose == SemanticActivationPurpose::Normal
        }
    };
    if !applicable {
        return Err(invalid(format!("inactive reason for {}", request.alias)));
    }
    Ok(())
}

/// Bind slot indexes to the original portable definition, including its physical source evidence.
pub(crate) fn definition_binding_digest(
    definition: &NativeSourceUnitDefinition,
) -> Result<String, SemanticProjectionError> {
    definition.validate().map_err(|error| invalid(error.to_string()))?;
    hash("incan-provider-definition-binding-v1", definition)
}

/// Encode original Rust declaration/source facts without selected child results.
fn rust_definition(inputs: &RustSemanticInputs<'_>) -> Value {
    json!({
        "contract": inputs.contract, "version": inputs.contract_version, "activation": inputs.activation,
        "package": inputs.package, "package_version": inputs.version,
        "crate": inputs.crate_name, "crate_kind": inputs.crate_kind, "edition": inputs.edition,
        "entrypoint": inputs.entrypoint, "source": inputs.source, "files": inputs.files,
        "configuration": inputs.configuration, "features": inputs.features,
        "default_features": inputs.default_features, "dependencies": inputs.dependencies,
    })
}

/// Bind selected Rust dependency slots to their complete original source-facet record.
pub(crate) fn rust_definition_binding_digest(
    inputs: &RustSemanticInputs<'_>,
) -> Result<String, SemanticProjectionError> {
    hash("incan-rust-definition-binding-v1", &rust_definition(inputs))
}

/// Verify complete positional coverage without selecting, sorting or repairing a producer's dependency graph.
fn slots<'a, T>(
    declared: usize,
    rows: &'a [T],
    index: impl Fn(&T) -> usize,
    label: &str,
) -> Result<BTreeMap<usize, &'a T>, SemanticProjectionError> {
    let mut mapped = BTreeMap::new();
    for row in rows {
        let index = index(row);
        if index >= declared || mapped.insert(index, row).is_some() {
            return Err(invalid(format!("{label} duplicate/extra slot {index}")));
        }
    }
    if mapped.len() != declared {
        return Err(missing(format!("{label} slot binding")));
    }
    Ok(mapped)
}

/// Require a child result from exactly this projection version.
fn same_version(version: u32) -> Result<(), SemanticProjectionError> {
    if version != PROVIDER_SEMANTIC_PROJECTION_VERSION {
        return Err(SemanticProjectionError::Unsupported {
            field: format!("semantic projection version {version}"),
        });
    }
    Ok(())
}

/// Match a producer-selected Rust result to the original request without resolving a different package.
fn require_rust_selection(
    request: &NativeSourceRequirement,
    selected: &RustSemanticDigest,
) -> Result<(), SemanticProjectionError> {
    same_version(selected.projection_version)?;
    if request.package.as_deref().unwrap_or(&request.alias) != selected.package
        || request
            .features
            .iter()
            .any(|feature| !selected.features.contains(feature))
        || (request.default_features && !selected.default_features)
    {
        return Err(invalid(format!("Rust selection for {}", request.alias)));
    }
    if let Some(requirement) = &request.version_requirement {
        let range = semver::VersionReq::parse(requirement).map_err(|_| invalid("Rust version requirement"))?;
        let version = semver::Version::parse(&selected.version).map_err(|_| invalid("selected Rust version"))?;
        if !range.matches(&version) {
            return Err(invalid("selected Rust version differs from request"));
        }
    }
    let matches = match (&request.source, &selected.source) {
        (NativeRequirementSource::RegistryRequest, SelectedRustSource::Registry) => true,
        (
            NativeRequirementSource::GitRequest { url, reference },
            SelectedRustSource::Git {
                url: actual,
                reference: original,
            },
        ) => url == actual && reference == original,
        (
            NativeRequirementSource::AuthoredPathRequest { .. } | NativeRequirementSource::UnboundPathRequest { .. },
            SelectedRustSource::Path,
        ) => true,
        _ => false,
    };
    if !matches {
        return Err(invalid(format!("selected source kind for {}", request.alias)));
    }
    Ok(())
}

/// Require portable declaration facts for an external Rust source unit before associating selected children.
fn validate_rust_request(request: &NativeSourceRequirement) -> Result<(), SemanticProjectionError> {
    text(&request.alias, "Rust dependency alias")?;
    if let Some(package) = &request.package {
        text(package, "Rust dependency package")?;
    }
    if let Some(version) = &request.version_requirement {
        semver::VersionReq::parse(version).map_err(|_| invalid("Rust version requirement"))?;
    }
    if request.features.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(invalid("Rust request features must be sorted and unique"));
    }
    for feature in &request.features {
        text(feature, "Rust request feature")?;
    }
    match &request.source {
        NativeRequirementSource::ProviderEdge { .. } => {
            return Err(SemanticProjectionError::Unsupported {
                field: "provider edge inside external Rust source contract".to_string(),
            });
        }
        NativeRequirementSource::UnboundPathRequest { request_key, .. } => {
            if request_key != &format!("{}:{}", request.role.as_str(), request.alias) {
                return Err(invalid("Rust source request slot key"));
            }
        }
        NativeRequirementSource::AuthoredPathRequest { path, .. } => {
            text(path, "authored path request")?;
            if path.starts_with('/') || path.contains('\\') || path.contains(':') {
                return Err(invalid("authored path request coordinate"));
            }
        }
        NativeRequirementSource::GitRequest { url, reference } => {
            text(url, "Git request URL")?;
            let (NativeGitReference::Branch(value) | NativeGitReference::Tag(value) | NativeGitReference::Rev(value)) =
                reference;
            text(value, "Git request reference")?;
        }
        NativeRequirementSource::RegistryRequest => {}
    }
    Ok(())
}

/// Hash a supplied Rust source unit and already selected children under the explicit new producer contract.
///
/// This function does not claim that a registry catalog or an old native receipt implements this contract. The
/// future producer must retain and authenticate the complete original record; malformed coverage cannot be patched
/// by adding a digest at this consumer. Build effects and native execution authority remain separate contracts.
pub(crate) fn digest_rust_source_inputs(
    inputs: &RustSemanticInputs<'_>,
) -> Result<RustSemanticDigest, SemanticProjectionError> {
    if inputs.contract != RUST_SEMANTIC_INPUT_CONTRACT || inputs.contract_version != 1 {
        return Err(SemanticProjectionError::Unsupported {
            field: format!(
                "Rust input contract {} version {}",
                inputs.contract, inputs.contract_version
            ),
        });
    }
    for (name, value) in [
        ("package", inputs.package),
        ("version", inputs.version),
        ("crate", inputs.crate_name),
        ("edition", inputs.edition),
    ] {
        text(value, name)?;
    }
    semver::Version::parse(inputs.version).map_err(|_| invalid("selected Rust package version"))?;
    if !matches!(inputs.edition, "2015" | "2018" | "2021" | "2024") {
        return Err(SemanticProjectionError::Unsupported {
            field: "Rust edition".to_string(),
        });
    }
    if !inputs.files.contains_key(inputs.entrypoint) {
        return Err(missing("Rust entrypoint file evidence"));
    }
    for (path, digest) in inputs.files {
        if path.is_empty()
            || path.contains('\\')
            || path.contains(':')
            || path.split('/').any(|part| matches!(part, "" | "." | ".."))
        {
            return Err(invalid("portable Rust input path"));
        }
        sha256(digest, "Rust input file digest")?;
    }
    let source = match inputs.source {
        RustSemanticSource::Registry { registry, checksum } => {
            text(registry, "selected registry")?;
            sha256(&format!("sha256:{checksum}"), "registry archive checksum")?;
            SelectedRustSource::Registry
        }
        RustSemanticSource::Git {
            url,
            reference,
            revision,
        } => {
            text(url, "selected Git URL")?;
            if !matches!(revision.len(), 40 | 64) || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(invalid("selected Git commit"));
            }
            SelectedRustSource::Git {
                url: url.to_string(),
                reference: reference.clone(),
            }
        }
        RustSemanticSource::Path => SelectedRustSource::Path,
        RustSemanticSource::CompilerSupport { support } => SelectedRustSource::CompilerSupport(support),
    };
    let activation_digest = activation_context_digest(&inputs.activation)?;
    if inputs.activation.features != inputs.features || inputs.activation.default_features != inputs.default_features {
        return Err(invalid("Rust activation feature association"));
    }
    let definition = rust_definition_binding_digest(inputs)?;
    let selections = slots(
        inputs.dependencies.len(),
        inputs.selections,
        |row| row.requirement_index,
        "Rust dependency",
    )?;
    let mut selected = Vec::new();
    let mut declared_slots = BTreeSet::new();
    for (index, dependency) in inputs.dependencies.iter().enumerate() {
        validate_rust_request(dependency.request)?;
        if !declared_slots.insert((
            dependency.request.role.as_str(),
            dependency.build,
            dependency.target_condition,
            &dependency.request.alias,
        )) {
            return Err(invalid("duplicate declared Rust dependency slot"));
        }
        if dependency.build && dependency.request.role == NativeRequirementRole::Dev {
            return Err(SemanticProjectionError::Unsupported {
                field: "development build dependency".to_string(),
            });
        }
        if let Some(condition) = dependency.target_condition {
            text(condition, "target condition")?;
        }
        let binding = selections.get(&index).ok_or_else(|| missing("Rust dependency"))?;
        if binding.definition_digest != definition {
            return Err(invalid("Rust dependency original definition binding"));
        }
        if binding.activation_digest != activation_digest {
            return Err(invalid("Rust dependency activation binding"));
        }
        let outcome = match &binding.outcome {
            RustSemanticOutcome::Selected(child) => {
                require_rust_selection(dependency.request, child)?;
                json!({ "selected": child.value })
            }
            RustSemanticOutcome::Inactive(reason) => {
                validate_inactive_reason(
                    *reason,
                    dependency.request,
                    dependency.target_condition,
                    dependency.build,
                    &inputs.activation,
                )?;
                json!({ "inactive": reason })
            }
        };
        selected.push(outcome);
    }
    let value = hash(
        "incan-rust-semantic-inputs-v1",
        &json!({ "definition": rust_definition(inputs), "selected_dependencies": selected }),
    )?;
    Ok(RustSemanticDigest {
        projection_version: PROVIDER_SEMANTIC_PROJECTION_VERSION,
        package: inputs.package.to_string(),
        version: inputs.version.to_string(),
        features: inputs.features.clone(),
        default_features: inputs.default_features,
        source,
        value,
    })
}

/// Normalize only typed provider identities through supplied exact results, preserving all feature dimensions.
fn normalize_identity(
    identity: &mut ProviderIdentity,
    origins: &BTreeMap<ProviderIdentity, &ProviderSemanticDigest>,
) -> Result<(), SemanticProjectionError> {
    let selected = origins
        .get(identity)
        .ok_or_else(|| missing(format!("typed provider origin {}", identity.stable_key())))?;
    same_version(selected.projection_version)?;
    identity.digest.clone_from(&selected.value);
    Ok(())
}

/// Normalize foreign type leaves and union owners with the shared visitor, without rewriting arbitrary strings.
fn normalize_types(
    value: &mut impl VisitTypeRefs,
    origins: &BTreeMap<ProviderIdentity, &ProviderSemanticDigest>,
) -> Result<(), SemanticProjectionError> {
    let mut error = None;
    value.visit_type_refs(&mut |ty| {
        if error.is_some() {
            return;
        }
        let result = match ty {
            TypeRef::Named {
                origin: Some(origin), ..
            }
            | TypeRef::Applied {
                origin: Some(origin), ..
            } => normalize_identity(&mut origin.provider, origins),
            TypeRef::NativeUnion(union) => match &mut union.owner {
                NativeUnionOwnerExport::SelectedArtifact(identity) => normalize_identity(identity, origins),
                NativeUnionOwnerExport::ContainingArtifact => Ok(()),
            },
            _ => Ok(()),
        };
        if let Err(failure) = result {
            error = Some(failure);
        }
    });
    error.map_or(Ok(()), Err)
}

/// Validate exact provider edges, source slots and compiler support before constructing the portable hash input.
fn project_provider(inputs: &ProviderSemanticInputs<'_>) -> Result<String, SemanticProjectionError> {
    let definition = inputs
        .definition
        .ok_or_else(|| missing("native source-unit definition"))?;
    if !matches!(definition.schema_version, 1 | 2 | 3) {
        return Err(SemanticProjectionError::Unsupported {
            field: format!("source-unit schema {}", definition.schema_version),
        });
    }
    if inputs
        .manifest
        .contract_metadata
        .provider
        .semantic_source_digest
        .is_none()
    {
        return Err(missing("authored provider source digest"));
    }
    definition
        .validate_against_manifest(inputs.manifest)
        .map_err(|error| invalid(error.to_string()))?;
    sha256(&inputs.identity.digest, "provider artifact identity")?;
    if inputs.identity.name != inputs.manifest.name || inputs.identity.version != inputs.manifest.version {
        return Err(invalid("provider identity and manifest"));
    }
    let activation_digest = activation_context_digest(&inputs.activation)?;
    if inputs.activation.features != &inputs.identity.feature_projection {
        return Err(invalid("provider activation feature association"));
    }
    let definition_digest = definition_binding_digest(definition)?;
    let supports = definition
        .compiler_support
        .as_ref()
        .ok_or_else(|| missing("compiler-support declarations"))?;
    let mut manifest = inputs.manifest.clone();
    let descriptors = &mut manifest.contract_metadata.provider.provider_dependencies;
    let edges = slots(
        descriptors.len(),
        inputs.edges,
        |edge| edge.descriptor_index,
        "provider edge",
    )?;
    let mut origins = BTreeMap::new();
    for selected in inputs
        .origins
        .iter()
        .copied()
        .chain(inputs.edges.iter().map(|edge| edge.target))
    {
        same_version(selected.projection_version)?;
        if let Some(previous) = origins.insert(selected.identity.clone(), selected)
            && previous.value != selected.value
        {
            return Err(invalid("conflicting selected origin results"));
        }
    }
    for (index, descriptor) in descriptors.iter_mut().enumerate() {
        let binding = edges.get(&index).ok_or_else(|| missing("provider edge"))?;
        let target = binding.target;
        if binding.selected_identity != &target.identity
            || descriptor.provider_name != target.identity.name
            || descriptor.provider_version != target.identity.version
            || descriptor.artifact_digest != target.identity.digest
        {
            return Err(invalid(format!("provider edge {index} exact target")));
        }
        let valid_features = match descriptor.kind {
            ProviderDependencyKind::PublicPackage => descriptor
                .requested_features
                .is_subset(&target.identity.feature_projection),
            ProviderDependencyKind::PrivateImplementation => {
                descriptor.requested_features == target.identity.feature_projection
                    && !descriptor.default_features
                    && !descriptor.optional
            }
        };
        if !valid_features {
            return Err(invalid(format!("provider edge {index} selected feature association")));
        }
        descriptor.artifact_digest.clone_from(&target.value);
        let mut semantic_identity = target.identity.clone();
        semantic_identity.digest.clone_from(&target.value);
        descriptor.relative_artifact_path = format!("incan-semantic-provider://{}", semantic_identity.stable_key());
    }
    let bindings = slots(
        definition.requirements.len(),
        inputs.requirements,
        |row| row.requirement_index,
        "provider requirement",
    )?;
    let mut selected_requirements = Vec::new();
    for (index, request) in definition.requirements.iter().enumerate() {
        let binding = bindings.get(&index).ok_or_else(|| missing("provider requirement"))?;
        if binding.definition_digest != definition_digest {
            return Err(invalid("provider requirement original definition binding"));
        }
        if binding.activation_digest != activation_digest {
            return Err(invalid("provider requirement activation binding"));
        }
        if let ProviderRequirementSelection::Inactive(reason) = &binding.selected {
            validate_inactive_reason(*reason, request, None, false, &inputs.activation)?;
            selected_requirements.push(json!({ "inactive": reason }));
            continue;
        }
        let digest = match (&request.source, &binding.selected) {
            (
                NativeRequirementSource::ProviderEdge {
                    edge_kind,
                    dependency_key,
                },
                ProviderRequirementSelection::ProviderEdge { descriptor_index },
            ) => {
                let descriptor = inputs
                    .manifest
                    .contract_metadata
                    .provider
                    .provider_dependencies
                    .get(*descriptor_index)
                    .ok_or_else(|| invalid("provider slot edge index"))?;
                if descriptor.kind != *edge_kind || descriptor.dependency_key != *dependency_key {
                    return Err(invalid("provider slot edge association"));
                }
                &edges
                    .get(descriptor_index)
                    .ok_or_else(|| missing("provider slot selected target"))?
                    .target
                    .value
            }
            (_, ProviderRequirementSelection::Rust(selected)) => {
                require_rust_selection(request, selected)?;
                &selected.value
            }
            _ => return Err(invalid("provider requirement selection kind")),
        };
        selected_requirements.push(json!({ "selected": digest }));
    }
    let support_bindings = slots(
        supports.len(),
        inputs.compiler_support,
        |row| row.support_index,
        "compiler support",
    )?;
    let mut selected_support = Vec::new();
    for (index, request) in supports.iter().enumerate() {
        let binding = support_bindings
            .get(&index)
            .ok_or_else(|| missing("compiler support"))?;
        same_version(binding.selected.projection_version)?;
        if binding.definition_digest != definition_digest
            || !matches!(binding.selected.source, SelectedRustSource::CompilerSupport(support) if support == request.support)
            || request
                .features
                .iter()
                .any(|feature| !binding.selected.features.contains(feature))
        {
            return Err(invalid("compiler support original source association"));
        }
        selected_support.push(&binding.selected.value);
    }
    normalize_types(&mut manifest.exports, &origins)?;
    normalize_types(&mut manifest.contract_metadata.api, &origins)?;
    for union in &mut manifest.contract_metadata.native_unions {
        if let NativeUnionOwnerExport::SelectedArtifact(identity) = &mut union.owner {
            normalize_identity(identity, &origins)?;
        }
        normalize_types(&mut union.members, &origins)?;
    }
    // Generated physical/host ABI facts remain checked by artifact admission; authored and producer facts own this
    // projection. Opaque executable and native representation evidence in the checked contract stays intact.
    manifest.rust_abi = None;
    hash(
        "incan-provider-semantic-artifact-v3",
        &json!({
            "version": PROVIDER_SEMANTIC_PROJECTION_VERSION,
            "manifest": RawLibraryManifest::from_semantic(&manifest),
            "selected_features": inputs.identity.feature_projection, "activation": inputs.activation,
            "unit": { "package": definition.package, "crate": definition.crate_name, "kind": definition.crate_kind, "edition": definition.edition },
            "requirements": definition.requirements, "selected_requirements": selected_requirements,
            "compiler_support": supports, "selected_support": selected_support,
        }),
    )
}

#[cfg(test)]
mod tests;
